//! The Server screen: run a 5th Echelon server on this PC, and manage a
//! server (this one or another) through its admin API: players, games, logs.

use std::io::BufRead as _;
use std::io::BufReader;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;

use dedicated_server_config::Config as ServerConfig;
use serde::Deserialize;

pub const SERVER_EXE: &str = crate::updater::SERVER_FILE;

/// The server next to the launcher (`dedicated_server.exe`), if it's there.
pub fn server_exe() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(SERVER_EXE))).filter(|p| p.is_file())
}

/// The server's `service.toml` (the defaults when there's none yet).
pub fn load_config(exe: &std::path::Path) -> Result<ServerConfig, String> {
    let path = exe.with_file_name("service.toml");
    let cfg = if path.exists() {
        ServerConfig::load_from_file(&path)
    } else {
        Ok(ServerConfig::default())
    };
    cfg.map_err(|e| format!("{}: {e}", path.display()))
}

/// The subnet whose players always advertise the address the server sees.
pub fn trusted_subnet(cfg: &ServerConfig) -> String {
    secure_settings(cfg).and_then(|s| s.get("trusted_subnet").cloned()).unwrap_or_default()
}

/// Writes the addresses and the trusted subnet into `service.toml`, and `cfg` with them. It's
/// read again first, so changes made by hand while the launcher was open stay, with `cfg`'s
/// community API switches on top. `listen`: None for every address; `public_ip`: what players
/// connect to, behind NAT.
pub fn prepare(exe: &std::path::Path, cfg: &mut ServerConfig, listen: Option<IpAddr>, public_ip: &str, trusted: &str) -> Result<(), String> {
    let path = exe.with_file_name("service.toml");
    if path.exists() {
        let mut fresh = ServerConfig::load_from_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::mem::swap(&mut fresh.community_api, &mut cfg.community_api);
        *cfg = fresh;
    }
    let ip = listen.unwrap_or(IpAddr::from([0, 0, 0, 0]));
    let public = match public_ip.trim() {
        "" => listen.unwrap_or(ip),
        s => s.parse().map_err(|_| format!("\"{s}\" isn't an IP address."))?,
    };
    set_ips(cfg, ip, public);
    if let Some(settings) = secure_settings_mut(cfg) {
        match trusted.trim() {
            "" => {
                settings.remove("trusted_subnet");
            }
            s => {
                settings.insert("trusted_subnet".into(), s.to_string());
            }
        }
    }
    cfg.save_to_file(&path).map_err(|e| format!("Couldn't save service.toml: {e}"))
}

/// Starts the server, which prints its admin key on start (`--launcher`): it, and the key.
pub fn spawn(exe: &std::path::Path) -> Result<(Child, String), String> {
    let mut child = Command::new(exe)
        .arg("--launcher")
        .current_dir(exe.parent().unwrap_or(std::path::Path::new(".")))
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start the server: {e}"))?;
    let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
    let key = lines.by_ref().map_while(Result::ok).find_map(|l| l.strip_prefix("Admin Key: ").map(str::to_string));
    // Keep reading, so a full pipe never blocks the server.
    std::thread::spawn(move || lines.for_each(drop));
    match key {
        Some(key) => Ok((child, key)),
        None => {
            let _ = child.kill();
            Err("The server stopped while starting. See its log below.".into())
        }
    }
}

/// The server's admin API on this PC.
pub fn local_api(cfg: &ServerConfig) -> String {
    let mut addr: SocketAddr = cfg.api_server;
    if addr.ip().is_unspecified() {
        addr.set_ip(IpAddr::from([127, 0, 0, 1]));
    }
    format!("http://{addr}")
}

/// Downloads the server from the latest release, next to the launcher.
pub fn download() -> anyhow::Result<()> {
    let latest = crate::updater::latest()?;
    anyhow::ensure!(latest.not_older(), "the latest release ({}) is older than this launcher", latest.version);
    let to = std::env::current_exe()?.with_file_name(SERVER_EXE);
    crate::updater::download(&latest, crate::updater::SERVER_ASSET, &to)
}

/// The last lines (at most `max`) of this PC's server log (`server.log.json`), from `min` up.
pub fn read_logs(exe: &std::path::Path, min: LogLevel, max: usize) -> Vec<LogItem> {
    let Ok(text) = std::fs::read_to_string(exe.with_file_name("server.log.json")) else {
        return Vec::new();
    };
    let items: Vec<LogItem> = text.lines().filter_map(|l| serde_json::from_str::<LogItem>(l).ok()).filter(|i| i.level >= min).collect();
    let skip = items.len().saturating_sub(max);
    items.into_iter().skip(skip).collect()
}

fn secure_settings(cfg: &ServerConfig) -> Option<&std::collections::HashMap<String, String>> {
    cfg.quazal.service.values().find_map(|s| match s {
        quazal::Service::Secure(ctx) => Some(&ctx.settings),
        _ => None,
    })
}

fn secure_settings_mut(cfg: &mut ServerConfig) -> Option<&mut std::collections::HashMap<String, String>> {
    cfg.quazal.service.values_mut().find_map(|s| match s {
        quazal::Service::Secure(ctx) => Some(&mut ctx.settings),
        _ => None,
    })
}

/// Listens on `ip` and tells clients to connect to `public` (they differ
/// behind NAT).
fn set_ips(cfg: &mut ServerConfig, ip: IpAddr, public: IpAddr) {
    cfg.set_addresses(ip, public);
}

/// The admin API's URL for what the player typed (`host`, `host:port` or a
/// URL), and whether it's plain HTTP to another machine that isn't on a
/// private network.
pub fn admin_url(typed: &str) -> Result<(String, bool), String> {
    let typed = typed.trim();
    let with_scheme = if typed.starts_with("http://") || typed.starts_with("https://") {
        typed.to_string()
    } else {
        format!("http://{typed}")
    };
    let mut url = url::Url::parse(&with_scheme).map_err(|_| format!("\"{typed}\" isn't a server address."))?;
    let host = url.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_string();
    if !setup::net::valid_host(&host) || !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
        return Err(format!("\"{typed}\" isn't a server address."));
    }
    if url.port().is_none() && url.scheme() == "http" && !typed.starts_with("http://") {
        let _ = url.set_port(Some(setup::API_PORT));
    }
    let local = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| !setup::net::is_public(ip));
    let plain = url.scheme() == "http" && !local;
    Ok((url.as_str().trim_end_matches('/').to_string(), plain))
}

#[derive(Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default)]
pub enum LogLevel {
    #[serde(rename = "TRCE")]
    Trace,
    #[serde(rename = "DEBG")]
    Debug,
    #[default]
    #[serde(rename = "INFO")]
    Info,
    #[serde(rename = "WARN")]
    Warn,
    #[serde(rename = "ERRO")]
    Error,
    #[serde(rename = "CRIT")]
    Critical,
}

#[derive(Debug, Deserialize)]
pub struct LogItem {
    pub msg: String,
    pub level: LogLevel,
    pub ts: String,
}

#[cfg(test)]
mod tests {
    use super::admin_url;

    #[test]
    fn admin_addresses() {
        assert_eq!(admin_url("10.8.0.10").unwrap(), ("http://10.8.0.10:50051".to_string(), false));
        assert_eq!(
            admin_url("play.example.org:9000").unwrap(),
            ("http://play.example.org:9000".to_string(), true),
            "plain over the internet"
        );
        assert_eq!(admin_url("https://play.example.org").unwrap(), ("https://play.example.org".to_string(), false));
        assert!(admin_url("localhost").is_ok_and(|(_, plain)| !plain));
        assert!(admin_url("http://user:pw@play.example.org").is_err());
        assert!(admin_url("a b").is_err());
    }
}
