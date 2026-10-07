//! Whether a member server is where its listing says it is.
//!
//! A member's listing names the host and ports players connect to, and a member can write
//! anything there: a compromised one could list itself at another server's address, under
//! its own name. So the coordinator asks the listed host for `/api/info` (over HTTPS on the
//! listed `api_tls` port when the host is a name, then over HTTP on port 80, where every
//! server's config server answers it, behind Caddy or not; the listed `api` port is the
//! launcher's gRPC, 50051 without Caddy, which doesn't) and
//! lists the member only when the server answering there gives the member's own id. Every
//! server says its id there (players sign it into their links), so this needs nothing new
//! of them. The server at another's address answers with its own id, which isn't the
//! member's.
//!
//! The result is kept with the server (`servers.host_check`), so a restart of the coordinator
//! doesn't empty the directory. A listing is checked when its host or ports change, again
//! every [`CHECK_AGAIN`], and every [`RETRY`] while the check fails.
//!
//! A member can point the coordinator at any host this way, so it only asks public addresses
//! (as for its pings, [`crate::public_ip`]): never this machine or its network, unless the
//! network is a LAN or test one (`--check-private-hosts`; addresses that never mean a server,
//! such as link-local where cloud machines keep their metadata service, still aren't asked).
//! It asks one path, follows no redirect, reads at most [`MAX_ANSWER`] bytes, and tells the
//! member only that the check failed: the details (which could map what answers where) are
//! for the coordinator's log and its admins. Every address a name resolves to that answers
//! must answer as the server, so a name that also points at another server's address doesn't
//! pass; one that doesn't answer (IPv6 the coordinator's machine can't reach) is passed over.

use std::net::IpAddr;
use std::net::SocketAddr;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use crate::Listing;

/// A host that passed is checked again this often (seconds).
pub const CHECK_AGAIN: i64 = 6 * 3600;
/// One that didn't is asked again this often (seconds).
pub const RETRY: i64 = 120;
/// The most of an answer read.
const MAX_ANSWER: usize = 64 * 1024;
/// How long one answer may take (a server on the other side of the world, over HTTPS).
const TIMEOUT: Duration = Duration::from_secs(10);

/// A check's result, as kept with the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    /// What was checked: [`target`] of the listing.
    pub target: String,
    pub ok: bool,
    /// When (Unix seconds).
    pub at: i64,
    /// Why it failed, for the admin UI and the server's log.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// What a listing's check covers: its host (as names compare) and its HTTPS port, if any.
pub fn target(listing: &Listing) -> String {
    let tls = listing.ports.as_ref().and_then(|p| p.api_tls);
    format!("{} {}", identity::host_key(&listing.host), tls.map_or_else(|| "-".to_string(), |p| p.to_string()))
}

/// Whether `checked` (the kept result, as text) says `listing` is where it says.
pub fn passed(checked: Option<&str>, listing: &Listing) -> bool {
    checked
        .and_then(|c| serde_json::from_str::<Checked>(c).ok())
        .is_some_and(|c| c.ok && c.target == target(listing))
}

/// Whether `listing` is due a check, given the kept result.
pub fn due(checked: Option<&str>, listing: &Listing, now: i64) -> bool {
    match checked.and_then(|c| serde_json::from_str::<Checked>(c).ok()) {
        None => true,
        Some(c) if c.target != target(listing) => true,
        Some(c) => now - c.at >= if c.ok { CHECK_AGAIN } else { RETRY },
    }
}

/// The addresses `/api/info` is asked at, in order: HTTPS first (only a host name has a
/// certificate), then plain HTTP on `http_port` (80; another in tests).
fn urls(listing: &Listing, http_port: u16) -> Vec<(bool, String, u16)> {
    let host = listing.host.trim_start_matches('[').trim_end_matches(']').to_string();
    let is_ip = host.parse::<IpAddr>().is_ok();
    let tls = listing.ports.as_ref().and_then(|p| p.api_tls);
    let mut out = Vec::new();
    if let (Some(tls), false) = (tls, is_ip) {
        out.push((true, host.clone(), tls));
    }
    out.push((false, host, http_port));
    out
}

/// The most addresses of one name asked.
const MAX_ADDRESSES: usize = 4;

/// Whether the coordinator may ask `ip` (see the module's notes): a public address, or with
/// `private`, any but those that never mean a server.
fn askable(ip: IpAddr, private: bool) -> bool {
    let special = match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_unspecified() || v4.is_multicast() || v4.is_link_local() || v4.is_broadcast(),
        IpAddr::V6(v6) => v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xffc0) == 0xfe80,
    };
    !special && (private || crate::public_ip(ip))
}

/// Asks `host` (each of its addresses) for the id of the server there: the one all that
/// answer give.
async fn ask(https: bool, host: &str, port: u16, private: bool) -> Result<String, String> {
    let mut addrs: Vec<SocketAddr> = tokio::time::timeout(TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| "its name didn't resolve in time".to_string())?
        .map_err(|e| format!("its name doesn't resolve ({e})"))?
        .collect();
    addrs.sort();
    addrs.dedup();
    if addrs.is_empty() || addrs.len() > MAX_ADDRESSES {
        return Err(format!("{host} has {} addresses", addrs.len()));
    }
    if let Some(a) = addrs.iter().find(|a| !askable(a.ip(), private)) {
        return Err(format!("{host} is at {}, not a public address", a.ip()));
    }
    ask_each(https, host, port, addrs).await
}

/// [`ask`], at these addresses of `host`. One that doesn't answer (IPv6 this machine can't
/// reach, say) says nothing; one answering as another server does.
async fn ask_each(https: bool, host: &str, port: u16, addrs: Vec<SocketAddr>) -> Result<String, String> {
    let (mut id, mut failed) = (None, Vec::new());
    for addr in addrs {
        match ask_at(https, host, port, addr).await {
            Ok(theirs) => match &id {
                None => id = Some(theirs),
                Some(first) if *first == theirs => {}
                Some(first) => return Err(format!("{host}'s addresses are different servers ({first:?} and {theirs:?})")),
            },
            Err(e) => failed.push(e),
        }
    }
    id.ok_or_else(|| failed.join("; "))
}

/// Asks one of `host`'s addresses for the id of the server there.
async fn ask_at(https: bool, host: &str, port: u16, addr: SocketAddr) -> Result<String, String> {
    let shown = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    let url = format!("{}://{shown}:{port}/api/info", if https { "https" } else { "http" });
    let client = reqwest::Client::builder()
        .user_agent(concat!("5th-echelon-coordinator/", env!("FE_RELEASE")))
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        // This address, not another the name might resolve to next time.
        .resolve(host, addr)
        .build()
        .map_err(|e| e.to_string())?;
    let url_at = format!("{url} ({})", addr.ip());
    let url = url.as_str();
    let mut resp = client.get(url).header("accept", "application/json").send().await.map_err(|e| format!("{url_at}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{url_at} answered {}", resp.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("{url_at}: {e}"))? {
        if body.len() + chunk.len() > MAX_ANSWER {
            return Err(format!("{url_at} answered with too much"));
        }
        body.extend_from_slice(&chunk);
    }
    let info: serde_json::Value = serde_json::from_slice(&body).map_err(|_| format!("{url_at} isn't a game server's answer"))?;
    info["id"].as_str().map(str::to_string).ok_or_else(|| format!("{url_at} gives no server id"))
}

/// Checks that the server answering where `listing` says is `server` (on a private address
/// too, with `private`), its plain HTTP on `http_port` (80).
pub async fn check(server: &str, listing: &Listing, now: i64, private: bool, http_port: u16) -> Checked {
    let mut why = Vec::new();
    for (https, host, port) in urls(listing, http_port) {
        match ask(https, &host, port, private).await {
            Ok(id) if id == server => {
                return Checked {
                    target: target(listing),
                    ok: true,
                    at: now,
                    why: String::new(),
                }
            }
            // Another server's id: the listing names someone else's address.
            Ok(id) => why.push(format!("the server at {host}:{port} is {id:?}, not this one")),
            Err(e) => why.push(e),
        }
    }
    Checked {
        target: target(listing),
        ok: false,
        at: now,
        why: why.join("; "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(host: &str, api: u16, tls: Option<u16>) -> Listing {
        serde_json::from_value(serde_json::json!({
            "name": "A", "host": host, "ports": { "api": api, "login": 21126, "api_tls": tls }
        }))
        .unwrap()
    }

    #[test]
    fn a_result_holds_for_what_was_checked() {
        let l = listing("Play.Example.org", 80, Some(443));
        let ok = serde_json::to_string(&Checked {
            target: target(&l),
            ok: true,
            at: 100,
            why: String::new(),
        })
        .unwrap();
        assert!(passed(Some(&ok), &l));
        assert!(passed(Some(&ok), &listing("PLAY.example.ORG", 80, Some(443))), "the same name, spelled otherwise");
        assert!(!passed(Some(&ok), &listing("other.example.org", 80, Some(443))), "another host");
        assert!(!passed(Some(&ok), &listing("play.example.org", 80, Some(8443))), "another HTTPS port");
        assert!(passed(Some(&ok), &listing("play.example.org", 50051, Some(443))), "the gRPC port isn't asked");
        assert!(!passed(None, &l));
        assert!(!due(Some(&ok), &l, 100 + CHECK_AGAIN - 1));
        assert!(due(Some(&ok), &l, 100 + CHECK_AGAIN));
        assert!(due(Some(&ok), &listing("other.example.org", 80, Some(443)), 101), "a new host at once");
        let failed = serde_json::to_string(&Checked {
            target: target(&l),
            ok: false,
            at: 100,
            why: "no".into(),
        })
        .unwrap();
        assert!(!passed(Some(&failed), &l));
        assert!(due(Some(&failed), &l, 100 + RETRY));
    }

    #[test]
    fn https_only_for_a_host_name() {
        assert_eq!(
            urls(&listing("play.example.org", 80, Some(443)), 80),
            [(true, "play.example.org".into(), 443), (false, "play.example.org".into(), 80)]
        );
        // The API port is the launcher's gRPC (50051 without Caddy): /api/info is on 80.
        assert_eq!(urls(&listing("203.0.113.5", 50051, Some(443)), 80), [(false, "203.0.113.5".into(), 80)]);
        assert_eq!(urls(&listing("[2001:db8::1]", 80, None), 80), [(false, "2001:db8::1".into(), 80)]);
    }

    #[tokio::test]
    async fn an_address_that_doesnt_answer_is_passed_over_but_another_server_isnt() {
        let serve = |id: &'static str, at: SocketAddr| async move {
            let listener = tokio::net::TcpListener::bind(at).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let app = axum::Router::new().route("/api/info", axum::routing::get(move || async move { axum::Json(serde_json::json!({ "id": id })) }));
            tokio::spawn(async move { axum::serve(listener, app).await });
            port
        };
        let port = serve("me", "127.0.0.1:0".parse().unwrap()).await;
        let at = |ip: &str| SocketAddr::new(ip.parse().unwrap(), port);
        // 127.0.0.2: nothing listens there.
        assert_eq!(ask_each(false, "x.test", port, vec![at("127.0.0.1"), at("127.0.0.2")]).await, Ok("me".into()));
        assert!(ask_each(false, "x.test", port, vec![at("127.0.0.2")]).await.is_err(), "nobody answering isn't a pass");
        // Another server at the name's other address.
        serve("someone else", at("127.0.0.3")).await;
        let both = vec![at("127.0.0.1"), at("127.0.0.3")];
        assert!(ask_each(false, "x.test", port, both).await.unwrap_err().contains("different servers"));
    }

    #[test]
    fn only_public_addresses_are_asked() {
        for ip in ["169.254.169.254", "0.0.0.0", "224.0.0.1", "255.255.255.255", "fe80::1", "::", "::ffff:169.254.169.254"] {
            assert!(!askable(ip.parse().unwrap(), true), "{ip}: never");
        }
        for ip in ["10.0.0.5", "127.0.0.1", "192.168.1.2", "100.64.0.1", "::1", "fd00::1"] {
            assert!(!askable(ip.parse().unwrap(), false), "{ip}: this machine or its network");
            assert!(askable(ip.parse().unwrap(), true), "{ip}: on a LAN or test network");
        }
        for ip in ["8.8.8.8", "2606:4700::1111"] {
            assert!(askable(ip.parse().unwrap(), false), "{ip}");
        }
    }
}
