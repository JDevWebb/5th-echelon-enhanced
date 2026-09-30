#![deny(clippy::pedantic)]
#![feature(iter_intersperse)]

#[macro_use]
extern crate quazal_macros;
#[macro_use]
extern crate slog;

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
fn login_required<T>(ci: &ClientInfo<T>) -> quazal::rmc::Result<u32> {
    ci.user_id.ok_or(quazal::rmc::Error::AccessDenied)
}

mod api;
mod challenge;
mod clan;
mod community_api;
mod config;
mod game_session;
mod game_session_ex;
mod keys;
mod ladder;
mod locale;
mod nat_helper;
mod nat_traversal;
mod overlord_challenge;
mod overlord_core;
mod overlord_news;
mod player_stats;
mod privileges;
mod rate_limit;
mod secure;
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
        handler.register_protocol(nat_traversal::new_protocol());
        handler.register_protocol(overlord_challenge::new_protocol());
        handler.register_protocol(overlord_core::new_protocol());
        handler.register_protocol(overlord_news::new_protocol());
        handler.register_protocol(player_stats::new_protocol());
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
                info!(logger, "Cleaning old session of user {user_id}");
                if let Err(e) = storage.delete_user_session(user_id) {
                    error!(logger, "session clean error: {e}");
                }
            }
        });
        server.disconnect_handler = Some(|ci: ClientInfo| {
            if let Some(user_id) = ci.user_id {
                info!(logger, "Cleaning closed session of user {user_id}");
                if let Err(e) = storage.delete_user_session(user_id) {
                    error!(logger, "session clean error: {e}");
                }
            }
        });
    }
    if is_secure {
        server.user_handler = Some(handle_user_packet);
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

    let mut response = packet.payload;
    write!(&mut response, "udp:/address={};port={}\0", client.ip(), client.port()).expect("writing to a Vec can't fail");
    if let Err(e) = socket.send_to(&response, client) {
        warn!(logger, "user packet reply to {client} failed: {e}");
    }
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
        _ => sloggers::types::Severity::Trace,
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

/// Ensures that the necessary data directory and files exist.
fn ensure_data_dir() -> io::Result<()> {
    let mp_ini = std::env::current_exe()?.parent().unwrap().join("data").join("mp_balancing.ini");
    if mp_ini.exists() {
        return Ok(());
    }
    fs::create_dir_all(mp_ini.parent().unwrap())?;
    fs::write(mp_ini, DEFAULT_MP_DATA)?;
    Ok(())
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

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let args = argh::from_env::<Args>();

    let logger = if args.launcher {
        Logger::root(build_file_logger(), o!())
    } else {
        let logger = build_term_logger();
        Logger::root(slog::Duplicate(logger, build_file_logger()).fuse(), o!())
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

    ensure_data_dir()?;

    warn!(logger, "Clearing stale sessions");
    storage.invalidate_sessions()?;

    let debug_config = Arc::new(config.debug);
    let admin_api = args.launcher || config.admin.enabled;
    rate_limit::configure(config.limits);
    for bad in rate_limit::trust_proxies(&config.public.proxies) {
        warn!(logger, "Ignoring [public] proxies entry {bad:?} (expected an address or a subnet like 172.17.0.0/16)");
    }
    let community_api = config.community_api;
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
                if cfg.listen.port() != 80 {
                    warn!(
                        logger,
                        "Unexpected port {} used for the config server. Clients are expecting port 80. Adjust in the service config or make sure to redirect traffic accordingly",
                        cfg.listen.port()
                    );
                }
                if let Err(e) = simple_http::serve(&logger, cfg.listen, &cfg.content(), Some(community_api::routes(Arc::clone(&storage), community_api))) {
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

    threads.push(
        std::thread::Builder::new()
            .name(String::from("api"))
            .spawn(move || {
                let logger = logger.new(o!("service" => "api"));
                if let Err(e) = tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(api::start_server(logger.clone(), storage, config.api_server, debug_config, admin_api, config.debug.grpc_reflection))
                {
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
