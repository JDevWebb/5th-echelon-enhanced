//! The coordinator binary. See the library for what it does, and
//! docs/friends.md for running one.

use std::path::PathBuf;
use std::sync::Arc;

use axum::serve::ListenerExt;

#[derive(argh::FromArgs)]
/// Shares friends between 5th Echelon servers and lists them in a server directory.
struct Args {
    /// address to listen on (default 127.0.0.1:8700; put a reverse proxy with
    /// HTTPS in front)
    #[argh(option, default = "String::from(\"127.0.0.1:8700\")")]
    listen: String,
    /// folder for the database and the join token (default: the current one)
    #[argh(option, default = "PathBuf::from(\".\")")]
    data: PathBuf,
    /// address for the admin web UI (e.g. 127.0.0.1:8701; off without it).
    /// Put it behind Cloudflare and Caddy, on a host name of its own
    #[argh(option)]
    admin_listen: Option<String>,
    /// where admins open the web UI, e.g. https://scbl-metrics.jdevwebb.net
    /// (passkeys are made for it, and requests must come from it)
    #[argh(option)]
    admin_origin: Option<String>,
    /// the admin UI's requests come straight from browsers, not through a
    /// proxy on this machine passing on the client's address and country
    #[argh(switch)]
    admin_direct: bool,
    /// keep the roadmap and take players' suggestions for it: the community
    /// network's coordinator only, whose roadmap every launcher reads
    #[argh(switch)]
    roadmap: bool,
    /// a name launchers reach this coordinator by, e.g. play.scbl.jdevwebb.net
    /// (repeat for more): players' signed requests must be made for one of them.
    /// Without it, for whatever name a request says
    #[argh(option)]
    name: Vec<String>,
    #[argh(subcommand)]
    command: Option<Command>,
}

#[derive(argh::FromArgs)]
#[argh(subcommand)]
enum Command {
    RemoveServer(RemoveServer),
    PurgeNames(PurgeNames),
    NewToken(NewToken),
    Admin(Admin),
    Standby(Standby),
}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "standby")]
/// a standby coordinator for a failover group (as root; docs/failover.md):
/// runs the coordinator here when its record points here, and takes over
/// when the coordinator is down
struct Standby {
    /// the group's settings (written by install-server.sh)
    #[argh(option, default = "PathBuf::from(\"/etc/5th-echelon/standby.conf\")")]
    config: PathBuf,
    /// say what this standby sees, and stop
    #[argh(switch)]
    status: bool,
    /// move the coordinator here now (from the server it runs on), and stop
    #[argh(switch)]
    take_over: bool,
}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "admin")]
/// manage the admins of the web UI
struct Admin {
    #[argh(subcommand)]
    command: AdminCommand,
}

#[derive(argh::FromArgs)]
#[argh(subcommand)]
enum AdminCommand {
    Add(AdminAdd),
    Reset(AdminReset),
    List(AdminList),
    OpenAccess(AdminOpenAccess),
}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "add")]
/// add an admin: prints a one-time link (24 hours) to choose a password and
/// add a passkey or authenticator app
struct AdminAdd {
    #[argh(positional)]
    username: String,
}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "reset")]
/// for an admin who lost their second factor: clears their password,
/// passkeys and authenticator, ends their sessions, prints a new setup link
struct AdminReset {
    #[argh(positional)]
    username: String,
}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "list")]
/// list the admins
struct AdminList {}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "open-access")]
/// clear the sign-in restrictions (networks and countries), for when every
/// admin is locked out
struct AdminOpenAccess {}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "remove-server")]
/// remove a member server: its links, and the names only it used
struct RemoveServer {
    /// the server's id (in the directory and its server-id.txt)
    #[argh(positional)]
    id: String,
}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "purge-names")]
/// release the names a member server reserved for identities that never
/// played: its links made over an hour ago for identities never seen online
/// and linked on no other server
struct PurgeNames {
    /// the server's id (in the directory and its server-id.txt)
    #[argh(positional)]
    id: String,
}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "new-token")]
/// make a new join token (servers that joined keep working); restart the
/// coordinator afterwards
struct NewToken {}

/// The file with the join token that servers need to join. Made on the first
/// start; `new-token` replaces it (servers that joined keep working).
const JOIN_TOKEN_FILE: &str = "join-token.txt";
/// Connections at once on each listener, and from one address (a proxy on this machine
/// isn't counted per address).
const MAX_CONNECTIONS: usize = 2048;
const MAX_CONNECTIONS_PER_IP: usize = 64;

fn new_token(path: &std::path::Path) -> eyre::Result<String> {
    let mut bytes = [0u8; 20];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    let token = identity::base32_encode(&bytes);
    write_private(path, &format!("{token}\n"))?;
    Ok(token)
}

fn join_token(dir: &std::path::Path) -> eyre::Result<String> {
    let path = dir.join(JOIN_TOKEN_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        if !text.trim().is_empty() {
            return Ok(text.trim().to_string());
        }
    }
    new_token(&path)
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, "")?;
    #[cfg(windows)]
    restrict_to_owner(path);
    std::fs::write(path, text)
}

/// Windows: lets only this user, the file's owner and SYSTEM read `path`
/// (no inherited access for other users of the PC). The owner and SYSTEM by
/// SID, so any Windows language works; this user too, so a first run as
/// administrator doesn't lock out a later normal one. Best effort: without
/// icacls (Wine), the file keeps its folder's permissions.
#[cfg(windows)]
fn restrict_to_owner(path: &std::path::Path) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let user = std::env::var("USERDOMAIN")
        .ok()
        .zip(std::env::var("USERNAME").ok())
        .map(|(domain, user)| format!("{domain}\\{user}:F"));
    // With this user named, then (a name icacls doesn't know, e.g. a service's) without.
    for with_user in [user, None] {
        let mut command = std::process::Command::new("icacls");
        command.arg(path).args(["/inheritance:r", "/grant:r", "*S-1-3-4:F", "/grant:r", "*S-1-5-18:F"]);
        if let Some(user) = &with_user {
            command.args(["/grant:r", user]);
        }
        let done = command
            .arg("/q")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .is_ok_and(|s| s.success());
        if done {
            return;
        }
    }
}

/// The coordinator's background work: releases and their rollout, metrics
/// rollups, pings to the servers, expired admin sessions.
fn spawn_jobs(c: &Arc<coordinator::Coordinator>) {
    let every = |period: std::time::Duration,
                 c: &Arc<coordinator::Coordinator>,
                 name: &'static str,
                 job: fn(Arc<coordinator::Coordinator>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>| {
        let c = Arc::clone(c);
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(period);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                if let Err(e) = job(Arc::clone(&c)).await {
                    tracing::warn!("{name}: {e}");
                }
            }
        });
    };
    every(coordinator::updates::CHECK_EVERY, c, "looking for releases", |c| {
        Box::pin(async move {
            let (version, page, published) = match coordinator::updates::latest_signed(&coordinator::http()).await {
                Ok(found) => found,
                // Nothing published yet: not worth a warning every ten minutes.
                Err(e) if e.contains("404") => {
                    tracing::debug!("no release on GitHub yet");
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
            c.release_found(&version, &page, &published).await.map(drop).map_err(|e| e.to_string())
        })
    });
    every(std::time::Duration::from_secs(30), c, "rollout", |c| {
        Box::pin(async move { c.tick_rollout().await.map_err(|e| e.to_string()) })
    });
    every(std::time::Duration::from_secs(3600), c, "metrics rollup", |c| {
        Box::pin(async move { c.roll_up().await.map_err(|e| e.to_string()) })
    });
    every(std::time::Duration::from_secs(60), c, "pinging servers", |c| {
        Box::pin(async move { c.ping_servers().await.map_err(|e| e.to_string()) })
    });
    every(std::time::Duration::from_secs(60), c, "alerts", |c| {
        Box::pin(async move { c.check_alerts().await.map_err(|e| e.to_string()) })
    });
    every(std::time::Duration::from_secs(600), c, "admin sessions", |c| {
        Box::pin(async move { c.sweep_admin().await.map_err(|e| e.to_string()) })
    });
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args: Args = argh::from_env();
    if let Some(Command::Standby(s)) = &args.command {
        return coordinator::standby::main(&s.config, s.status, s.take_over).await;
    }
    std::fs::create_dir_all(&args.data)?;
    let db = args.data.join("coordinator.db");
    match args.command {
        Some(Command::NewToken(_)) => {
            new_token(&args.data.join(JOIN_TOKEN_FILE))?;
            if let Ok(c) = coordinator::Coordinator::open(&db.to_string_lossy(), String::new()).await {
                c.audit("console", None, "made a new join token", "").await;
            }
            println!("A new join token is in {}; restart the coordinator to use it.", args.data.join(JOIN_TOKEN_FILE).display());
            return Ok(());
        }
        Some(Command::Admin(a)) => {
            let c = coordinator::Coordinator::open(&db.to_string_lossy(), String::new()).await?;
            // Where the admin UI is: as given, or as the running coordinator saved it.
            let origin = match &args.admin_origin {
                Some(o) => Some(o.clone()),
                None => c.setting("admin_origin").await?,
            };
            if let Some(origin) = origin {
                let _ = c.admin.set(coordinator::admin::Config::new(&origin, !args.admin_direct).map_err(|e| eyre::eyre!(e))?);
            }
            match a.command {
                AdminCommand::Add(add) => {
                    let link = c.admin_setup_link(&add.username, false).await.map_err(|e| eyre::eyre!(e))?;
                    c.audit("console", None, "added an admin", &add.username).await;
                    println!("{} can set up their sign-in at this link, within 24 hours (it works once):\n\n  {link}", add.username);
                }
                AdminCommand::Reset(reset) => {
                    let link = c.admin_setup_link(&reset.username, true).await.map_err(|e| eyre::eyre!(e))?;
                    c.audit("console", None, "reset admin", &reset.username).await;
                    println!(
                        "{} was signed out and can set up their sign-in again at this link, within 24 hours:\n\n  {link}",
                        reset.username
                    );
                }
                AdminCommand::List(_) => {
                    for (name, disabled, totp, passkeys) in c.admin_list().await? {
                        let factors = match (totp, passkeys) {
                            (false, 0) => String::from("no second factor yet"),
                            (true, 0) => String::from("authenticator app"),
                            (t, n) => format!("{n} passkey(s){}", if t { " and an authenticator app" } else { "" }),
                        };
                        println!("{name}{}: {factors}", if disabled { " (disabled)" } else { "" });
                    }
                }
                AdminCommand::OpenAccess(_) => {
                    c.save_restrictions(&coordinator::admin::auth::Restrictions::default()).await?;
                    c.audit("console", None, "cleared the sign-in restrictions", "").await;
                    println!("Admins may sign in from anywhere again. Set restrictions again under Security.");
                }
            }
            return Ok(());
        }
        Some(Command::RemoveServer(r)) => {
            let c = coordinator::Coordinator::open(&db.to_string_lossy(), String::new()).await?;
            if c.remove_server(&r.id).await? {
                c.audit("console", None, "removed a server", &r.id).await;
                println!("Removed server {} with its links. Rotate the join token (new-token) if it could join again.", r.id);
            } else {
                println!("No server {}.", r.id);
            }
            return Ok(());
        }
        Some(Command::PurgeNames(p)) => {
            let c = coordinator::Coordinator::open(&db.to_string_lossy(), String::new()).await?;
            let n = c.purge_unused_names(&p.id).await?;
            c.audit("console", None, "released a server's unused names", &format!("{}: {n} links", p.id)).await;
            println!("Removed {n} unused links of {}, and the names only they held.", p.id);
            return Ok(());
        }
        Some(Command::Standby(_)) | None => {}
    }
    let token = join_token(&args.data)?;
    let coordinator = Arc::new(coordinator::Coordinator::open(&db.to_string_lossy(), token).await?);
    let _ = coordinator.data_dir.set(std::fs::canonicalize(&args.data).unwrap_or_else(|_| args.data.clone()));
    if args.roadmap {
        coordinator.enable_roadmap();
        if args.name.is_empty() {
            tracing::warn!("--roadmap without --name: players' signed requests are checked against the name each request says, which a replay can set");
        }
    }
    let _ = coordinator.names.set(args.name.iter().map(|n| identity::host_key(n)).collect());
    // Where launchers' ping reports and admins come from: DB-IP's city database, kept current.
    let geo = Arc::new(geo::Geo::new(args.data.join("geoip")));
    let _ = coordinator.geo.set(Arc::clone(&geo));
    tokio::spawn(async move { geo.keep_current(concat!("5th-echelon-coordinator/", env!("FE_RELEASE"))).await });
    spawn_jobs(&coordinator);
    // Names claimed for accounts that never linked go after an hour.
    {
        let coordinator = Arc::clone(&coordinator);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
                match coordinator.sweep().await {
                    Ok(n) if n > 0 => tracing::info!("released {n} names nobody uses"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("sweeping names failed: {e}"),
                }
            }
        });
    }
    if let Some(listen) = &args.admin_listen {
        let origin = args
            .admin_origin
            .as_deref()
            .ok_or_else(|| eyre::eyre!("--admin-listen needs --admin-origin (where admins open it)"))?;
        let config = coordinator::admin::Config::new(origin, !args.admin_direct).map_err(|e| eyre::eyre!(e))?;
        coordinator.set_setting("admin_origin", &config.origin).await?;
        let _ = coordinator.admin.set(config);
        let admin = tokio::net::TcpListener::bind(listen).await?;
        tracing::info!("Admin UI on {listen}, for {origin}");
        let app = coordinator::admin::router(Arc::clone(&coordinator)).into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move {
            let admin = coordinator::limits::Limited::new(admin, MAX_CONNECTIONS, MAX_CONNECTIONS_PER_IP).tap_io(|_| {});
            if let Err(e) = axum::serve(admin, app).await {
                tracing::error!("admin UI: {e}");
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!("Listening on {}; servers join with the token in {}", args.listen, args.data.join(JOIN_TOKEN_FILE).display());
    let listener = coordinator::limits::Limited::new(listener, MAX_CONNECTIONS, MAX_CONNECTIONS_PER_IP).tap_io(|_| {});
    axum::serve(listener, Arc::clone(&coordinator).router().into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
