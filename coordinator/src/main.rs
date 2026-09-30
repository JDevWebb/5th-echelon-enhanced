//! The coordinator binary. See the library for what it does, and
//! docs/friends.md for running one.

use std::path::PathBuf;
use std::sync::Arc;

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
    #[argh(subcommand)]
    command: Option<Command>,
}

#[derive(argh::FromArgs)]
#[argh(subcommand)]
enum Command {
    RemoveServer(RemoveServer),
    NewToken(NewToken),
}

#[derive(argh::FromArgs)]
#[argh(subcommand, name = "remove-server")]
/// remove a member server: its links, and the names only it used
struct RemoveServer {
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
    let user = std::env::var("USERDOMAIN").ok().zip(std::env::var("USERNAME").ok()).map(|(domain, user)| format!("{domain}\\{user}:F"));
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

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args: Args = argh::from_env();
    std::fs::create_dir_all(&args.data)?;
    let db = args.data.join("coordinator.db");
    match args.command {
        Some(Command::NewToken(_)) => {
            new_token(&args.data.join(JOIN_TOKEN_FILE))?;
            println!("A new join token is in {}; restart the coordinator to use it.", args.data.join(JOIN_TOKEN_FILE).display());
            return Ok(());
        }
        Some(Command::RemoveServer(r)) => {
            let c = coordinator::Coordinator::open(&db.to_string_lossy(), String::new()).await?;
            if c.remove_server(&r.id).await? {
                println!("Removed server {} with its links. Rotate the join token (new-token) if it could join again.", r.id);
            } else {
                println!("No server {}.", r.id);
            }
            return Ok(());
        }
        None => {}
    }
    let token = join_token(&args.data)?;
    let coordinator = Arc::new(coordinator::Coordinator::open(&db.to_string_lossy(), token).await?);
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
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(
        "Listening on {}; servers join with the token in {}",
        args.listen,
        args.data.join(JOIN_TOKEN_FILE).display()
    );
    axum::serve(listener, Arc::clone(&coordinator).router().into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
