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
}

/// The file with the join token that servers need to join. Made on the first
/// start; delete it to make a new one (servers that joined keep working).
const JOIN_TOKEN_FILE: &str = "join-token.txt";

fn join_token(dir: &std::path::Path) -> eyre::Result<String> {
    let path = dir.join(JOIN_TOKEN_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        if !text.trim().is_empty() {
            return Ok(text.trim().to_string());
        }
    }
    let mut bytes = [0u8; 20];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    let token = identity::base32_encode(&bytes);
    write_private(&path, &format!("{token}\n"))?;
    Ok(token)
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
    std::fs::write(path, text)
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args: Args = argh::from_env();
    std::fs::create_dir_all(&args.data)?;
    let token = join_token(&args.data)?;
    let db = args.data.join("coordinator.db");
    let coordinator = Arc::new(coordinator::Coordinator::open(&db.to_string_lossy(), token).await?);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(
        "Listening on {}; servers join with the token in {}",
        args.listen,
        args.data.join(JOIN_TOKEN_FILE).display()
    );
    axum::serve(listener, coordinator.router())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
