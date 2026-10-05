//! The central module for network-related functionality.
//!
//! This file aggregates functions from its submodules (`discover`, `quazal`, `rpc`)
//! and defines a common `Error` enum for all network operations.

mod discover;
mod nat;
mod quazal;
mod rpc;

/// The game's Quazal port on the server, and the local port the game uses.
pub const QUAZAL_DEFAULT_LOCAL_PORT: u16 = 3074;

// Re-export public functions from submodules.
pub use discover::try_locate_server;
/// What this launcher signs in to servers as: they refuse launchers older than they allow.
pub const CLIENT: &str = concat!("launcher/", env!("FE_RELEASE"));

pub use nat::test_nat_helper;
pub use quazal::test_p2p;
pub use quazal::test_quazal_login;
pub use rpc::account_id;
pub use rpc::identity_login;
pub use rpc::link_identity;
pub use rpc::name_available;
pub use rpc::register;
pub use rpc::relationships;
pub use rpc::rename;
pub use rpc::report_size;
pub use rpc::send_report;
pub use rpc::session_summary;
pub use rpc::sign_in;
pub use rpc::test_login;

/// A gRPC endpoint for `url`: HTTPS (with the usual root certificates) when
/// the URL says so.
pub fn endpoint(url: &str) -> Result<tonic::transport::Endpoint, Error> {
    let endpoint = tonic::transport::Endpoint::from_shared(url.to_string()).map_err(|_| Error::ConnectionFailed)?;
    if url.starts_with("https://") {
        return endpoint
            .tls_config(tonic::transport::ClientTlsConfig::new().with_webpki_roots())
            .map_err(|_| Error::ConnectionFailed);
    }
    Ok(endpoint)
}

/// Tests the connection to the configuration server.
///
/// This function sends a GET request to the `GetOnlineConfig` endpoint of the
/// specified server to verify that it is reachable and responding correctly.
pub async fn test_cfg_server(hostname: &str) -> Result<(), Error> {
    let url = format!("http://{hostname}/OnlineConfigService.svc/GetOnlineConfig");
    let resp = reqwest::Client::new().get(url).send().await.map_err(Error::ConfigServer)?;
    resp.error_for_status().map_err(Error::ConfigServer)?;
    Ok(())
}

/// A unified error type for all network operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid password")]
    InvalidPassword,
    #[error("Username not found")]
    UserNotFound,
    #[error("Connection failed")]
    ConnectionFailed,
    #[error("Error when sending request")]
    SendingRequestFailed,
    #[error("{0}")]
    ServerFailure(String),
    #[error("Username already taken or Ubisoft ID already registered")]
    UsernameAlreadyTaken,
    #[error("I/O error: {0}")]
    IO(#[from] std::io::Error),
    #[error("RMC error: {0}")]
    Rmc(#[from] ::quazal::rmc::Error),
    #[error("Quazal error: {0}")]
    Quazal(#[from] ::quazal::Error),
    #[error("Quazal error: {0}")]
    Prudp(#[from] ::quazal::prudp::packet::Error),
    #[error("Connection attempt timed out")]
    TimedOut,
    #[error("RPC error: {0}")]
    Rpc(#[from] tonic::Status),
    #[error("Challenge mismatch")]
    ChallengeMismatch,
    #[error("P2P error: {0}")]
    P2P(#[from] Box<Error>),
    #[error("Config server: {0}")]
    ConfigServer(#[from] reqwest::Error),
}

/// How long a server's `/api/info` may take. A secure connection to a server on the other
/// side of the world takes several round trips before the answer, and more on a connection
/// that drops packets: 3 s wasn't enough for some players.
pub const INFO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// What a server says about itself, asked over HTTPS on `port` (443, or the
/// port it gives its HTTPS API): None when it has no certificate for `host`
/// or doesn't answer there. Unlike the plain answer, nobody on the way can
/// change this one.
pub async fn server_info_tls(host: &str, port: u16) -> Option<setup::server_info::ServerInfo> {
    if !setup::net::valid_host(host) || host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let url = if port == 443 {
        format!("https://{host}/api/info")
    } else {
        format!("https://{host}:{port}/api/info")
    };
    let client = reqwest::Client::builder().timeout(INFO_TIMEOUT).redirect(reqwest::redirect::Policy::none()).build().ok()?;
    let mut resp = client.get(url).header("accept", "application/json").send().await.ok()?.error_for_status().ok()?;
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if body.len() + chunk.len() > 64 * 1024 {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    // Another site's JSON on the same name isn't a game server's answer.
    setup::server_info::parse_json(std::str::from_utf8(&body).ok()?).filter(|i| !i.version.is_empty())
}

/// Whether `url` (https://host) is a network's coordinator: its `/v1/info`
/// answers with the coordinator's name and how many servers it has.
pub async fn is_coordinator(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return false;
    };
    let Ok(resp) = client.get(format!("{}/v1/info", url.trim_end_matches('/'))).send().await else {
        return false;
    };
    if !resp.status().is_success() || resp.content_length().is_some_and(|n| n > 64 * 1024) {
        return false;
    }
    let Ok(body) = resp.text().await else {
        return false;
    };
    let Ok(info) = serde_json::from_str::<serde_json::Value>(&body) else {
        return false;
    };
    info["servers"].is_u64() && info["name"].as_str().is_some_and(|n| n.to_ascii_lowercase().contains("coordinator"))
}

/// A network's coordinator, from what the player typed (`play.example.org`, or its
/// `https://` address): it must answer `/v1/info` as one and list its servers
/// (`/v1/servers`). Answers its address and how many servers it lists.
pub async fn check_network(typed: &str) -> Result<(String, usize), String> {
    let typed = typed.trim().trim_end_matches('/');
    if typed.starts_with("http://") {
        return Err("A network's address starts with https://, so nobody on the way can change its list.".into());
    }
    let url = if typed.starts_with("https://") { typed.to_string() } else { format!("https://{typed}") };
    if !setup::directory::valid_coordinator(&url) || url.trim_start_matches("https://").contains('/') {
        return Err(format!("\"{typed}\" isn't a network's address: type its host, like play.example.org."));
    }
    let host = url.trim_start_matches("https://").to_string();
    if !is_coordinator(&url).await {
        return Err(format!("{host} doesn't answer as a network of servers (its /v1/info)."));
    }
    let Listed { servers, .. } = fetch_directory(&url).await.map_err(|e| format!("{host}'s list of servers didn't work: {e}."))?;
    Ok((url, servers.len()))
}

/// A directory's servers, each with this PC's ping to it (None: no answer), and a
/// note when they're its last list or the built-in one because it didn't answer.
#[derive(Debug, Clone)]
pub struct Browsed {
    pub servers: Vec<(setup::directory::Listing, Option<u32>)>,
    pub note: Option<String>,
    /// A release going out to the network's servers, while it lasts.
    pub rollout: Option<setup::directory::Rollout>,
    /// The network's own maintenance window (its coordinator), when one is booked.
    pub network_maintenance: Option<setup::directory::Window>,
}

/// The servers in a coordinator's directory, each with this PC's ping to it, measured in
/// parallel. When the directory doesn't answer: the list it gave last, else the community's
/// built-in servers (see `setup::directory::fallback`), pinged the same way.
pub async fn server_directory(coordinator: &str) -> Result<Browsed, String> {
    if !setup::directory::valid_coordinator(coordinator) {
        return Err("the server directory must be an https:// address".into());
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let dir = setup::app_data_dir();
    let (servers, note, rollout, network_maintenance) = match fetch_directory(coordinator).await {
        Ok(Listed {
            mut servers,
            rollout,
            network_maintenance,
        }) => {
            if let Some(dir) = &dir {
                // Windows that just ended aren't listed any more, but a server not back from
                // one is overrunning it.
                if let Some((previous, _)) = setup::directory::cached(dir, coordinator) {
                    setup::directory::carry_ended(&previous, &mut servers, now);
                }
                setup::directory::save_cache(dir, coordinator, &servers, network_maintenance.as_ref(), now);
            }
            (servers, None, rollout, network_maintenance)
        }
        Err(e) => match setup::directory::fallback(dir.as_deref(), coordinator, now) {
            Some((servers, note)) => {
                tracing::warn!("Server directory {coordinator}: {e}; using {} servers it listed before", servers.len());
                // The network's window as last listed: likely why it doesn't answer.
                let network = dir.as_deref().and_then(|d| setup::directory::cached(d, coordinator)).and_then(|(_, w)| w);
                (servers, Some(note), None, network)
            }
            None => return Err(e),
        },
    };
    // All at once: a server that doesn't answer costs two seconds, not two each.
    let handles: Vec<_> = servers.iter().map(|s| tokio::spawn(ping(s.host.clone(), s.ports.and_then(|p| p.nat)))).collect();
    let mut pings = Vec::with_capacity(handles.len());
    for handle in handles {
        pings.push(handle.await.unwrap_or(Ping::NoAnswer));
    }
    // A name that resolves into this PC's network isn't listed: a directory can't point
    // players at their own machines.
    let (servers, pings): (Vec<_>, Vec<_>) = servers
        .into_iter()
        .zip(pings)
        .filter_map(|(s, ping)| match ping {
            Ping::Private => None,
            Ping::NoAnswer => Some((s, None)),
            Ping::Ms(ms) => Some((s, Some(ms))),
        })
        .unzip();
    // Pings to a directory that didn't answer would go nowhere.
    if note.is_none() {
        report_pings(coordinator, &servers, &pings);
    }
    Ok(Browsed {
        servers: servers.into_iter().zip(pings).collect(),
        note,
        rollout,
        network_maintenance,
    })
}

/// What a directory's answer says: its servers, the rollout of a release if one is going
/// out, and the network's own maintenance window if one is booked.
struct Listed {
    servers: Vec<setup::directory::Listing>,
    rollout: Option<setup::directory::Rollout>,
    network_maintenance: Option<setup::directory::Window>,
}

fn listed(body: &str) -> Result<Listed, String> {
    Ok(Listed {
        servers: setup::directory::parse(body).map_err(|e| format!("the directory's answer isn't one: {e}"))?,
        rollout: setup::directory::parse_rollout(body),
        network_maintenance: setup::directory::parse_network_maintenance(body),
    })
}

/// The directory's list (`GET /v1/servers`), read and checked.
async fn fetch_directory(coordinator: &str) -> Result<Listed, String> {
    // Debug builds can read it from a file instead, for screenshots of a made-up network.
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("FE_DIRECTORY_FILE") {
        return listed(&std::fs::read_to_string(path).map_err(|e| e.to_string())?);
    }
    let mut resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?
        .get(setup::directory::url(coordinator))
        .send()
        .await
        .map_err(|e| format!("the directory didn't answer: {e}"))?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > setup::directory::MAX_BYTES {
            return Err("the directory's answer is too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(body).map_err(|_| "the directory's answer isn't text".to_string())?;
    listed(&body)
}

/// How a directory's server answered [`ping`].
enum Ping {
    /// Its name resolves to an address that isn't on the internet.
    Private,
    NoAnswer,
    Ms(u32),
}

/// Tells the directory's coordinator how long the round trip to each server
/// was from here, for its operators' metrics (it notes the city this comes
/// from, not the address). In the background; it doesn't matter if it fails.
fn report_pings(coordinator: &str, servers: &[setup::directory::Listing], pings: &[Option<u32>]) {
    let list: Vec<serde_json::Value> = servers
        .iter()
        .zip(pings)
        .filter_map(|(s, ms)| ms.map(|ms| serde_json::json!({ "server": s.id, "ms": ms })))
        .collect();
    if list.is_empty() {
        return;
    }
    let url = format!("{}/v1/pings", coordinator.trim_end_matches('/'));
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build();
        if let Ok(client) = client {
            let body = serde_json::json!({ "pings": list }).to_string();
            let _ = client.post(url).header("content-type", "application/json").body(body).send().await;
        }
    });
}

/// The round trip to a server: its NAT helper's answer (the path game
/// traffic takes), or else the time to open its port 80. Only public
/// addresses, and the NAT helper only on its usual port: a directory can't
/// aim every launcher's packets at something else.
async fn ping(host: String, nat_port: Option<u16>) -> Ping {
    let ms = |t: std::time::Instant| u32::try_from(t.elapsed().as_millis()).unwrap_or(u32::MAX);
    let Some(ip) = tokio::net::lookup_host((host.as_str(), setup::CONFIG_PORT))
        .await
        .ok()
        .and_then(|addrs| addrs.map(|a| a.ip()).find(std::net::IpAddr::is_ipv4))
    else {
        return Ping::NoAnswer;
    };
    if !setup::net::is_public(ip) {
        return Ping::Private;
    }
    if nat_port == Some(setup::NAT_PORT) {
        let started = std::time::Instant::now();
        if tokio::time::timeout(std::time::Duration::from_secs(2), nat::probe_once(&ip.to_string(), setup::NAT_PORT))
            .await
            .is_ok_and(|r| r.is_ok())
        {
            return Ping::Ms(ms(started));
        }
    }
    let started = std::time::Instant::now();
    match tokio::time::timeout(std::time::Duration::from_secs(2), tokio::net::TcpStream::connect((ip, setup::CONFIG_PORT))).await {
        Ok(Ok(_)) => Ping::Ms(ms(started)),
        _ => Ping::NoAnswer,
    }
}
