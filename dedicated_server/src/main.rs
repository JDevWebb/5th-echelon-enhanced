#![deny(clippy::pedantic)]
#![feature(iter_intersperse)]

#[macro_use]
extern crate quazal_macros;
#[macro_use]
extern crate slog;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::io::Write;
use std::net::SocketAddr;
use std::net::UdpSocket;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use quazal::prudp::packet::QPacket;
use quazal::ClientInfo;
use quazal::Context;
use sc_bl_protocols as protocols;
use slog::Drain;
use slog::Logger;
use sloggers::Build;
use storage::Storage;

const DEFAULT_MP_DATA: &str = include_str!("../../data/mp_balancing.ini");
const DEFAULT_NEWS: &str = include_str!("../../data/news.json");
/// Upstream's news, which servers wrote as their default before 5th Echelon Enhanced had
/// its own: replaced by the new default when found unchanged.
const UPSTREAM_NEWS: &str = include_str!("news_upstream.json");
/// 5th Echelon Enhanced's first news (0.3.150): replaced like upstream's, unless edited.
const FIRST_NEWS: &str = include_str!("news_0.3.150.json");
const DEFAULT_CHALLENGES: &str = include_str!("../../data/challenges.json");

const SERVER_PID: u32 = 0x1000;

macro_rules! rmc_err {
    ($e:expr, $log:expr, $msg:literal) => (
        $e.map_err(|e| {
            slog::error!($log, $msg; "error" => ?e);
            quazal::rmc::Error::InternalError
        })
    )
}

/// Checks if a client is logged in and returns their user ID.
///
/// Returns an `AccessDenied` error if the client is not logged in.
/// The server's own accounts: `Server` (1) and `Tracking` (105), the game's
/// telemetry login, whose password is the game's and so public. They may
/// connect and send telemetry, nothing else.
const SERVICE_ACCOUNTS: [u32; 2] = [1, 105];

/// The signed-in player: refuses anonymous connections and the service
/// accounts.
fn login_required<T>(ci: &ClientInfo<T>) -> quazal::rmc::Result<u32> {
    match ci.user_id {
        Some(id) if !SERVICE_ACCOUNTS.contains(&id) => Ok(id),
        _ => Err(quazal::rmc::Error::AccessDenied),
    }
}

/// Any signed-in connection, service accounts included: for the connection
/// itself and telemetry.
fn login_or_service_required<T>(ci: &ClientInfo<T>) -> quazal::rmc::Result<u32> {
    ci.user_id.ok_or(quazal::rmc::Error::AccessDenied)
}

mod api;
mod challenge;
mod clan;
mod clients;
mod community_api;
mod config;
mod federation;
mod friends_policy;
mod game_session;
mod game_session_ex;
mod keys;
mod ladder;
mod locale;
mod metrics;
mod nat_helper;
mod nat_traversal;
mod overlord_challenge;
mod overlord_core;
mod overlord_news;
mod player_stats;
mod players;
mod privileges;
mod rate_limit;
mod recent_log;
mod reports;
mod secure;
mod self_update;
mod session_events;
mod simple_http;
mod storage;
mod ticket;
mod tracking;
mod tracking_ext;
mod ubi_acc_mgmt;
mod uplay_win;
mod user_storage;

use crate::config::Config;
use crate::config::DebugConfig;

/// A player's game connection closed: their play session ends.
fn end_play(logger: &slog::Logger, storage: &Storage, user_id: u32) {
    if let Ok(Some(name)) = storage.find_username_by_user_id(user_id) {
        nat_helper::game_signed_out(&name);
    }
    if let Err(e) = storage.end_play(user_id) {
        error!(logger, "ending the play session of {user_id} failed: {e}");
    }
    players::changed(user_id);
}

/// Starts a Quazal server (either secure or authentication).
///
/// This function sets up the necessary protocols and handlers for the server
/// and then enters the server loop.
fn start_server(logger: &slog::Logger, ctx: &Context, storage: &Arc<Storage>, debug_config: &Arc<DebugConfig>, is_secure: bool) -> io::Result<()> {
    use quazal::prudp::packet::StreamHandlerRegistry;
    use quazal::prudp::packet::StreamType;
    use quazal::prudp::packet::VPort;
    use quazal::prudp::Server;
    use quazal::rmc::RVSecHandler;

    let mut handler = RVSecHandler::<()>::new(logger.clone());

    if is_secure {
        handler.register_protocol(challenge::new_protocol());
        handler.register_protocol(clan::new_protocol());
        handler.register_protocol(game_session_ex::new_protocol(Arc::clone(storage)));
        handler.register_protocol(game_session::new_protocol(Arc::clone(storage), Arc::clone(debug_config)));
        handler.register_protocol(ladder::new_protocol());
        handler.register_protocol(locale::new_protocol());
        handler.register_protocol(nat_traversal::new_protocol(Arc::clone(storage)));
        handler.register_protocol(overlord_challenge::new_protocol());
        handler.register_protocol(overlord_core::new_protocol());
        handler.register_protocol(overlord_news::new_protocol());
        handler.register_protocol(player_stats::new_protocol(Arc::clone(storage)));
        handler.register_protocol(privileges::new_protocol());
        handler.register_protocol(secure::new_protocol());
        handler.register_protocol(tracking_ext::new_protocol());
        handler.register_protocol(tracking::new_protocol());
        handler.register_protocol(ubi_acc_mgmt::new_protocol(Arc::clone(storage)));
        handler.register_protocol(uplay_win::new_protocol());
        handler.register_protocol(user_storage::new_protocol());
    } else {
        handler.register_protocol(ticket::new_protocol(Arc::clone(storage)));
    }

    let mut registry = StreamHandlerRegistry::new(logger.clone());
    registry.register(
        VPort {
            stream_type: StreamType::RVSec,
            port: ctx.vport,
        },
        Box::new(handler),
    );

    let mut server = Server::new(logger.clone(), ctx, registry);
    // Per-user state (station URLs, lobbies, ticket session keys) belongs to
    // the secure connection. The auth connection ends or times out right after
    // login, and cleaning up then wiped what the player had just registered on
    // the secure server.
    if is_secure {
        server.expired_client_handler = Some(|ci: ClientInfo| {
            if let Some(user_id) = ci.user_id {
                metrics::game_logout(user_id);
                end_play(logger, storage, user_id);
                session_events::note(session_events::Who::Id(user_id), "signout", serde_json::json!({ "how": "timed_out" }));
                info!(logger, "Cleaning old session of user {user_id}");
                if let Err(e) = storage.delete_user_session(user_id) {
                    error!(logger, "session clean error: {e}");
                }
            }
        });
        server.disconnect_handler = Some(|ci: ClientInfo| {
            if let Some(user_id) = ci.user_id {
                metrics::game_logout(user_id);
                end_play(logger, storage, user_id);
                session_events::note(session_events::Who::Id(user_id), "signout", serde_json::json!({ "how": "closed" }));
                info!(logger, "Cleaning closed session of user {user_id}");
                if let Err(e) = storage.delete_user_session(user_id) {
                    error!(logger, "session clean error: {e}");
                }
            }
        });
    }
    // The service accounts are shared (Tracking's password is the game's own): signing in to
    // one must not close everyone else's connections to it.
    server.newest_sign_in_wins = |user_id| !SERVICE_ACCOUNTS.contains(&user_id);
    if is_secure {
        // An admin's kick or ban (players.rs).
        server.sign_outs = Some(players::sign_outs());
        server.user_handler = Some(handle_user_packet);
        // Online means a signed-in connection here, not just a ticket from the auth server.
        let (storage, logger) = (Arc::clone(storage), logger.clone());
        server.login_handler = Some(Box::new(move |user_id, from: std::net::SocketAddr| {
            // Each game also signs in as the shared tracking account: a player, not two.
            if SERVICE_ACCOUNTS.contains(&user_id) {
                return;
            }
            // A ticket from before a ban still connects: signed out again at once.
            if storage.banned(user_id).unwrap_or(false) {
                info!(logger, "{user_id} is banned; signing them out");
                players::sign_out(user_id);
                return;
            }
            // First: friends and searches see them online as soon as they're signed in.
            if let Err(e) = storage.set_online(user_id) {
                error!(logger, "marking user {user_id} online failed: {e}");
            }
            let new_session = storage.start_play(user_id).unwrap_or_else(|e| {
                error!(logger, "starting the play session of {user_id} failed: {e}");
                false
            });
            metrics::game_login(user_id, from.ip(), new_session);
            // It should register with the NAT helper next: noticed when it doesn't.
            if let Ok(Some(name)) = storage.find_username_by_user_id(user_id) {
                nat_helper::game_signed_in(&name);
            }
            players::changed(user_id);
            federation::stats_soon(user_id);
        }));
    }
    server.bind(ctx.listen)?;
    server.serve();
    Ok(())
}

/// Handles user-specific RMC packets.
///
/// This function is a placeholder for handling user-specific RMC packets.
fn handle_user_packet(logger: &Logger, packet: QPacket, client: SocketAddr, socket: &UdpSocket) {
    if packet.source.port != 1 || packet.destination.port != 1 {
        warn!(logger, "ignoring user packet for ports {} -> {}", packet.source.port, packet.destination.port);
        return;
    }

    // The game takes its address for other players from this echo. What this socket saw is a
    // port its router opened for this server alone, which behind a NAT that gives every
    // destination its own (a VPN, a mobile network) nobody else reaches. The NAT helper knows
    // the address that works - the player's checked public one, or the relay's - when it's
    // the only player there.
    //
    // The game asks the moment its socket opens, a little before its hook has reached the
    // helper, and asks again every quarter second until answered: so it isn't answered until
    // the helper's address for it is final (see `advertised_for_ip`), for up to ECHO_WAIT,
    // and then with what was seen.
    let reply = match nat_helper::advertised_for_ip(client.ip()) {
        Some(advertise) => SocketAddr::V4(advertise),
        None if nat_helper::relay_ip().is_some() && echo_waiting(client.ip()) => return,
        None => client,
    };
    let mut response = packet.payload;
    write!(&mut response, "udp:/address={};port={}\0", reply.ip(), reply.port()).expect("writing to a Vec can't fail");
    if let Err(e) = socket.send_to(&response, client) {
        warn!(logger, "user packet reply to {client} failed: {e}");
    }
}

/// How long the address echo waits for the NAT helper to know a player.
const ECHO_WAIT: std::time::Duration = std::time::Duration::from_secs(4);

/// When each address first asked for its echo (forgotten after a minute), and when the
/// table was last swept.
struct EchoWaits {
    first_asked: HashMap<std::net::IpAddr, std::time::Instant>,
    swept: std::time::Instant,
}

impl EchoWaits {
    const FORGET: std::time::Duration = std::time::Duration::from_secs(60);
    const MAX: usize = 10_000;

    /// Whether an echo from `ip` should go unanswered for now: it first asked less than
    /// [`ECHO_WAIT`] ago (asking again a minute later starts over).
    ///
    /// The address's own entry is looked at first. Old entries are swept at most once a
    /// second, not on every packet (a pass over up to [`Self::MAX`] for each forged
    /// source); a full table answers new addresses at once.
    fn waiting(&mut self, ip: std::net::IpAddr, now: std::time::Instant) -> bool {
        if let Some(first) = self.first_asked.get_mut(&ip) {
            if now.duration_since(*first) >= Self::FORGET {
                *first = now;
            }
            return now.duration_since(*first) < ECHO_WAIT;
        }
        if self.first_asked.len() >= Self::MAX && now.duration_since(self.swept) >= std::time::Duration::from_secs(1) {
            self.first_asked.retain(|_, t| now.duration_since(*t) < Self::FORGET);
            self.swept = now;
        }
        if self.first_asked.len() >= Self::MAX {
            return false;
        }
        self.first_asked.insert(ip, now);
        true
    }
}

/// [`EchoWaits::waiting`], for the service's one table.
fn echo_waiting(ip: std::net::IpAddr) -> bool {
    static WAITS: std::sync::Mutex<Option<EchoWaits>> = std::sync::Mutex::new(None);
    let now = std::time::Instant::now();
    let mut waits = WAITS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    waits
        .get_or_insert_with(|| EchoWaits {
            first_asked: HashMap::new(),
            swept: now,
        })
        .waiting(ip, now)
}

/// Log level from the environment variable `var` (default: info).
fn severity_from_env(var: &str) -> sloggers::types::Severity {
    #[allow(clippy::match_same_arms)]
    match std::env::var(var).unwrap_or_else(|_| String::from("info")).as_str() {
        "debug" => sloggers::types::Severity::Debug,
        "trace" => sloggers::types::Severity::Trace,
        "info" => sloggers::types::Severity::Info,
        "error" => sloggers::types::Severity::Error,
        "critical" => sloggers::types::Severity::Critical,
        "warning" => sloggers::types::Severity::Warning,
        // Not trace: trace logs raw packets, logins included.
        _ => sloggers::types::Severity::Info,
    }
}

/// Builds a terminal logger with a configurable log level.
fn build_term_logger() -> Logger {
    sloggers::terminal::TerminalLoggerBuilder::new()
        .level(severity_from_env("RUST_LOG"))
        .format(sloggers::types::Format::Compact)
        .build()
        .unwrap()
}

/// Rotates log files, keeping a specified number of backups.
fn rotate_log_files<S: AsRef<Path>>(fname: S, i: i32) -> io::Result<PathBuf> {
    let fname = fname.as_ref();
    if fname.exists() {
        let maybe_iteration = fname.extension().and_then(|s| s.to_str()).and_then(|s| s.parse::<i32>().ok());
        let new_fname = match maybe_iteration {
            Some(j) if j == i - 1 => format!("{}.{}", fname.file_stem().and_then(|f| f.to_str()).unwrap(), i),
            _ => format!("{}.{}", fname.display(), i),
        };
        if i < 10 {
            rotate_log_files(&new_fname, i + 1)?;
        }
        fs::rename(fname, new_fname)?;
    }
    Ok(fname.to_path_buf())
}

/// Builds a file logger that writes to a JSON file.
fn build_file_logger() -> Logger {
    let fname = rotate_log_files("server.log.json", 1).unwrap();
    // Upstream logged every packet at trace level into one file per run with
    // no size limit. Level from RUST_LOG_FILE (default info), rotated by size.
    sloggers::file::FileLoggerBuilder::new(fname)
        .truncate()
        .level(severity_from_env("RUST_LOG_FILE"))
        .rotate_size(20 * 1024 * 1024)
        .rotate_keep(3)
        .format(sloggers::types::Format::Json)
        .build()
        .unwrap()
}

/// Writes the default data files the game asks for (multiplayer balancing,
/// news, challenges) where the services read them, when they're missing:
/// `data/` in the working folder, and the content service's own paths. A
/// server run from another folder than its program's (the Linux installer's
/// service runs in /var/lib/5th-echelon) used to have none there: "Could not
/// download latest multiplayer data". Files already there are never replaced.
fn ensure_data_dir(content_files: &[std::path::PathBuf]) -> io::Result<()> {
    let data = std::path::Path::new("data");
    let defaults = [("mp_balancing.ini", DEFAULT_MP_DATA), ("news.json", DEFAULT_NEWS), ("challenges.json", DEFAULT_CHALLENGES)];
    for (name, contents) in defaults {
        write_if_missing(&data.join(name), contents)?;
    }
    // Upstream's placeholder news, or an earlier release's default, never edited: this
    // release's news instead.
    let news = data.join("news.json");
    if fs::read_to_string(&news).is_ok_and(|n| n == UPSTREAM_NEWS || n == FIRST_NEWS) {
        fs::write(&news, DEFAULT_NEWS)?;
    }
    for path in content_files.iter().filter(|p| p.file_name().is_some_and(|n| n == "mp_balancing.ini")) {
        write_if_missing(path, DEFAULT_MP_DATA)?;
    }
    Ok(())
}

fn write_if_missing(path: &std::path::Path, contents: &str) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, contents)
}

#[derive(argh::FromArgs)]
/// dedicated server
struct Args {
    /// path to config file (default: service.toml)
    #[argh(option, short = 'c', long = "config")]
    config_path: Option<PathBuf>,

    /// started through launcher
    #[argh(switch)]
    launcher: bool,

    /// the address players connect to (this server's public or VPN
    /// address); rewrites service.toml on every start. Also FE_PUBLIC_ADDRESS.
    #[argh(option)]
    public_address: Option<std::net::IpAddr>,

    /// the address to listen on (default: every address, 0.0.0.0). Also
    /// FE_LISTEN.
    #[argh(option)]
    listen: Option<std::net::IpAddr>,
}

/// An address from a command-line option or else an environment variable.
fn address_setting(arg: Option<std::net::IpAddr>, env: &str) -> eyre::Result<Option<std::net::IpAddr>> {
    if arg.is_some() {
        return Ok(arg);
    }
    match std::env::var(env) {
        Ok(s) if !s.trim().is_empty() => Ok(Some(s.trim().parse().map_err(|e| eyre::eyre!("{env}={s:?}: {e}"))?)),
        _ => Ok(None),
    }
}

/// glibc keeps memory it has served big blocks from: after the first 19 MiB
/// password hash is freed it raises its mmap threshold, and every later hash
/// comes from (and stays in) one of up to 8 arenas per core. A few logins
/// then leave hundreds of MB resident on a server that needs about 10. A
/// fixed threshold returns big blocks to the system when freed, and fewer
/// arenas keep the rest small.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_allocator() {
    // SAFETY: mallopt only changes allocator settings; called before any
    // other thread starts.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 1024 * 1024);
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn tune_allocator() {}

fn main() -> color_eyre::Result<()> {
    tune_allocator();
    color_eyre::install()?;
    let args = argh::from_env::<Args>();

    // Every line is also kept in memory for a while, for players' problem reports
    // (recent_log.rs).
    let logger = if args.launcher {
        Logger::root(slog::Duplicate(build_file_logger(), recent_log::Recent).fuse(), o!())
    } else {
        let logger = build_term_logger();
        Logger::root(slog::Duplicate(slog::Duplicate(logger, build_file_logger()), recent_log::Recent).fuse(), o!())
    };

    {
        let logger = logger.clone();
        let old_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            error!(logger, "Panic occurred: {panic_info}");
            old_hook(panic_info);
        }));
    }

    let storage = Arc::new(Storage::init(logger.clone())?);

    let config_filename = args.config_path.unwrap_or_else(|| PathBuf::from("service.toml"));

    let mut config = Config::load_from_file_or_default(&logger, &config_filename)?;
    let listen = address_setting(args.listen, "FE_LISTEN")?;
    if let Some(public) = address_setting(args.public_address, "FE_PUBLIC_ADDRESS")? {
        let listen = listen.unwrap_or(std::net::IpAddr::from([0, 0, 0, 0]));
        info!(logger, "Listening on {listen}; players connect to {public}");
        config.set_addresses(listen, public);
        config.save_to_file(&config_filename)?;
    }

    // Behind a proxy or remapped ports: hand out the ports players use.
    let public_ports = config.public_ports();
    if !config.public.is_default() {
        info!(logger, "Players connect to {:?} on {public_ports:?}", config.public.host);
    }
    config.apply_public();
    community_api::publish(public_ports, config.public.host.clone());

    let content_files: Vec<std::path::PathBuf> = config
        .quazal
        .service
        .values()
        .filter_map(|s| match s {
            quazal::Service::Content(c) => Some(c.files.values().cloned().collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect();
    ensure_data_dir(&content_files)?;

    session_events::watch_request_failures();
    session_events::note(session_events::Who::Server, "server_start", serde_json::json!({ "version": env!("FE_RELEASE") }));
    warn!(logger, "Clearing stale sessions");
    storage.invalidate_sessions()?;
    // Play sessions the server didn't see end (it stopped) end when last seen.
    storage.close_stale_play()?;

    let debug_config = Arc::new(config.debug);
    let admin_api = args.launcher || config.admin.enabled;
    rate_limit::configure(config.limits);
    match clients::configure(&config.clients) {
        Ok(Some(minimum)) => info!(logger, "Clients older than {minimum} are refused"),
        Ok(None) => warn!(logger, "Clients of any version may sign in ([clients] minimum_version = \"off\")"),
        Err(complaint) => warn!(logger, "{complaint}"),
    }
    for bad in rate_limit::trust_proxies(&config.public.proxies) {
        warn!(logger, "Ignoring [public] proxies entry {bad:?} (expected an address or a subnet like 172.17.0.0/16)");
    }
    let community_api = config.community_api;
    let friends_mode = config.friends.mode;
    friends_policy::set_mode(friends_mode);
    let federation_config = config.federation.clone();
    let server_id = federation::load_or_create_server_id(Path::new(federation::SERVER_ID_FILE))?;
    federation::init(server_id.clone(), federation_config.enabled());
    community_api::publish_friends(
        server_id,
        friends_mode,
        federation_config.enabled().then(|| federation_config.coordinator.trim().to_string()),
    );
    let nat = config.nat;
    // Relay addresses use the address players reach this server on.
    let relay_ip = nat
        .public_address
        .or_else(|| {
            config.quazal.service.values().find_map(|svc| match svc {
                quazal::Service::Authentication(ctx) => ctx.secure_server_addr.map(|a| a.ip()),
                _ => None,
            })
        })
        .and_then(|ip| match ip {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(_) => None,
        })
        .unwrap_or(std::net::Ipv4Addr::LOCALHOST);

    let mut threads = vec![];
    if nat.enabled {
        match nat_helper::start(&logger.new(o!("service" => "nat")), nat, relay_ip) {
            Ok(t) => threads.extend(t),
            Err(e) => crit!(logger, "Couldn't start the NAT helper on UDP {}: {e}", nat.listen),
        }
    }
    for (name, svc) in config.quazal.into_services()? {
        let logger = logger.new(o!("service" => name.clone()));
        info!(logger, "Loaded service {:#?}", svc);
        let storage = Arc::clone(&storage);
        let debug_config = Arc::clone(&debug_config);
        let handle = match svc {
            quazal::Service::Authentication(ctx) => std::thread::Builder::new().name(name).spawn(move || {
                if let Err(e) = start_server(&logger, &ctx, &storage, &debug_config, false) {
                    crit!(logger, "Error running authentication server: {e:?}");
                }
            }),
            quazal::Service::Secure(ctx) => std::thread::Builder::new().name(name).spawn(move || {
                if let Err(e) = start_server(&logger, &ctx, &storage, &debug_config, true) {
                    crit!(logger, "Error running secure server: {e:?}");
                }
            }),
            quazal::Service::Config(cfg) => std::thread::Builder::new().name(name).spawn(move || {
                // Behind a reverse proxy (listening on loopback) the proxy has port 80.
                if cfg.listen.port() != 80 && !cfg.listen.ip().is_loopback() {
                    warn!(
                        logger,
                        "Unexpected port {} used for the config server. Clients are expecting port 80. Adjust in the service config or make sure to redirect traffic accordingly",
                        cfg.listen.port()
                    );
                }
                if let Err(e) = simple_http::serve(
                    &logger,
                    cfg.listen,
                    &cfg.content(),
                    Some(community_api::routes(Arc::clone(&storage), community_api)),
                    community_api::takes_body,
                ) {
                    crit!(logger, "Error running config server: {e:?}");
                }
            }),
            quazal::Service::Content(srv) => std::thread::Builder::new().name(name).spawn(move || {
                if let Err(e) = simple_http::serve_many(&logger, srv.listen, &srv.files) {
                    crit!(logger, "Error running content server: {e:?}");
                }
            }),
        };
        threads.push(handle.unwrap());
    }

    // The names players reach this server by: what their identity signatures must name.
    federation::set_own_names(
        config
            .public
            .host
            .iter()
            .cloned()
            .chain([relay_ip.to_string(), String::from("127.0.0.1"), String::from("localhost")])
            .chain(config.public.aliases.iter().cloned()),
    );
    // What the coordinator's server directory shows about this server.
    let listing = {
        let host = config.public.host.clone().unwrap_or_else(|| relay_ip.to_string());
        let federation_config = federation_config.clone();
        move || federation::Listing {
            names: federation::own_names().into_iter().filter(|n| n != "127.0.0.1" && n != "localhost").collect(),
            name: if federation_config.name.is_empty() {
                host.clone()
            } else {
                federation_config.name.clone()
            },
            region: federation_config.region.clone(),
            listed: federation_config.listed,
            host: host.clone(),
            ports: public_ports,
            version: community_api::RELEASE.to_string(),
            players_online: 0,
            players_total: 0,
            friends_mode,
            auto_update: federation_config.auto_update && self_update::updater_installed(),
            online: Vec::new(),
        }
    };

    threads.push(
        std::thread::Builder::new()
            .name(String::from("api"))
            .spawn(move || {
                let logger = logger.new(o!("service" => "api"));
                let rt = tokio::runtime::Runtime::new().unwrap();
                // Ended sessions and old invitations go every few minutes, not only at a restart.
                {
                    let storage = Arc::clone(&storage);
                    let logger = logger.clone();
                    rt.spawn(async move {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                            if let Err(e) = storage.purge_stale_async().await {
                                warn!(logger, "Purging old sessions failed: {e}");
                            }
                        }
                    });
                }
                // Players' session events, saved for the admin UI (sent by the federation).
                rt.spawn(session_events::run(logger.new(o!("service" => "session_events")), Arc::clone(&storage)));
                // Play sessions still going, every minute: where one ends if the server stops.
                {
                    let storage = Arc::clone(&storage);
                    let logger = logger.clone();
                    rt.spawn(async move {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                            let storage = Arc::clone(&storage);
                            if let Ok(Err(e)) = tokio::task::spawn_blocking(move || storage.touch_play()).await {
                                warn!(logger, "Noting play sessions failed: {e}");
                            }
                        }
                    });
                }
                // Where players are, by city, for the coordinator's metrics: looked up here, in
                // DB-IP's database (kept in geoip/), never sent as addresses.
                let geo = Arc::new(geo::Geo::new("geoip"));
                metrics::start(Arc::clone(&geo));
                if federation_config.enabled() {
                    rt.spawn(async move { geo.keep_current(concat!("5th-echelon-server/", env!("FE_RELEASE"))).await });
                    rt.spawn(federation::run(logger.new(o!("service" => "federation")), Arc::clone(&storage), federation_config, listing));
                }
                if let Err(e) = rt.block_on(api::start_server(
                    logger.clone(),
                    storage,
                    config.api_server,
                    debug_config,
                    admin_api,
                    config.debug.grpc_reflection,
                    friends_mode,
                    args.launcher,
                )) {
                    crit!(logger, "Error running api server: {e:?}");
                }
            })
            .unwrap(),
    );

    // Every service runs until the process ends. If one stops (an error or a
    // panic outside packet handling), exit so the supervisor (Docker's restart
    // policy, the launcher) restarts the whole server, instead of staying up
    // with that service dead, e.g. logins working but matchmaking gone.
    loop {
        if let Some(t) = threads.iter().find(|t| t.is_finished()) {
            let name = t.thread().name().unwrap_or("unnamed").to_owned();
            eprintln!("service {name} stopped; exiting so the server is restarted");
            std::process::exit(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_echo_waits_once_per_address_and_never_sweeps_per_packet() {
        let start = std::time::Instant::now();
        let mut waits = EchoWaits {
            first_asked: HashMap::new(),
            swept: start,
        };
        let ip = |n: u32| std::net::IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + n));
        assert!(waits.waiting(ip(0), start));
        assert!(waits.waiting(ip(0), start + std::time::Duration::from_secs(1)), "still within ECHO_WAIT");
        assert!(!waits.waiting(ip(0), start + ECHO_WAIT), "then answered");
        assert!(waits.waiting(ip(0), start + EchoWaits::FORGET + ECHO_WAIT), "a minute later it starts over");
        for n in 1..EchoWaits::MAX as u32 {
            waits.waiting(ip(n), start);
        }
        assert!(!waits.waiting(ip(u32::MAX >> 8), start), "a full table answers new addresses at once");
        let later = start + EchoWaits::FORGET + std::time::Duration::from_secs(1);
        assert!(waits.waiting(ip(u32::MAX >> 8), later), "after a sweep there is room");
        assert!(waits.first_asked.len() < 10, "the old entries were swept");
    }
}
