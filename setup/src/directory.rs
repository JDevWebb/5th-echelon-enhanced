//! The server directory a coordinator keeps (`GET /v1/servers`): community
//! servers that share friends, with how many players are on each. The
//! launcher measures its ping to each and suggests the best.
//!
//! The last list each directory gave is kept (`directory-cache.json` in the
//! launcher's folder), and the community's servers are built in: when the
//! directory doesn't answer, players still get its servers to choose from.

use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

/// The community network's coordinator, the launcher's directory unless the
/// player chose another.
pub const COMMUNITY: &str = "https://play.scbl.jdevwebb.net";
/// The file, in the launcher's folder, keeping each directory's last list.
pub const CACHE_FILE: &str = "directory-cache.json";
/// The most directories kept in the cache.
const MAX_CACHED: usize = 8;

/// One server in the directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Listing {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub region: String,
    /// What players type to join (a host name or address).
    pub host: String,
    #[serde(default)]
    pub ports: Option<crate::server_info::Ports>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub players_online: u32,
    #[serde(default)]
    pub players_total: u32,
    /// "everyone" or "mutual".
    #[serde(default)]
    pub friends_mode: String,
}

#[derive(Debug, Deserialize)]
struct Directory {
    servers: Vec<serde_json::Value>,
}

/// The most servers read from a directory.
pub const MAX_SERVERS: usize = 200;
/// The largest directory answer read, in bytes.
pub const MAX_BYTES: usize = 1024 * 1024;
/// Names and regions longer than this are cut.
const MAX_TEXT: usize = 64;

/// The directory from the coordinator's answer: at most [`MAX_SERVERS`],
/// skipping entries that don't parse or whose host isn't a public server
/// address (a directory must not point launchers at their own network).
pub fn parse(json: &str) -> anyhow::Result<Vec<Listing>> {
    let servers = serde_json::from_str::<Directory>(json)?.servers;
    Ok(servers
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Listing>(v).ok())
        .filter(|l| listable_host(&l.host))
        .map(|mut l| {
            l.name = clean(&l.name);
            l.region = clean(&l.region);
            l
        })
        .take(MAX_SERVERS)
        .collect())
}

/// Whether `host` may be offered to players by a server or a directory: a
/// host name, or an address on the internet (never this PC or its network).
/// A name that resolves into a private network is caught when it's used
/// (the launcher checks the address it resolved to).
pub fn listable_host(host: &str) -> bool {
    crate::net::valid_host(host) && host.parse::<std::net::IpAddr>().map_or(host != "localhost" && !host.ends_with(".localhost"), crate::net::is_public)
}

/// One line, no characters that turn the text around it (see `hooks_config::text`).
fn clean(text: &str) -> String {
    hooks_config::text::clip(text, MAX_TEXT)
}

/// Whether `coordinator` can be used as a directory: an `https://` address
/// with a host, so nobody on the way can change the list.
pub fn valid_coordinator(coordinator: &str) -> bool {
    url::Url::parse(coordinator.trim()).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some_and(|h| !h.is_empty()) && u.username().is_empty() && u.password().is_none())
}

/// The directory's address on a coordinator.
pub fn url(coordinator: &str) -> String {
    format!("{}/v1/servers", coordinator.trim().trim_end_matches('/'))
}

/// The server to suggest: the lowest ping, then the most players online.
/// Servers that didn't answer (`None`) only if none did. Pings within 15 ms
/// count as the same, so a busier server nearby wins over an empty one a
/// hair closer.
pub fn best(servers: &[(Listing, Option<u32>)]) -> Option<usize> {
    let fastest = servers.iter().filter_map(|(_, ping)| *ping).min();
    let pick = |(i, (listing, ping)): (usize, &(Listing, Option<u32>))| -> Option<(u32, std::cmp::Reverse<u32>, usize)> {
        match (fastest, ping) {
            (Some(f), Some(p)) if *p <= f + 15 => Some((0, std::cmp::Reverse(listing.players_online), i)),
            (Some(_), _) => None,
            (None, _) => Some((1, std::cmp::Reverse(listing.players_online), i)),
        }
    };
    servers.iter().enumerate().filter_map(pick).min().map(|(_, _, i)| i)
}

/// The community's servers, built in for when its directory doesn't answer
/// (none for any other directory).
pub fn built_in(coordinator: &str) -> Vec<Listing> {
    if !same_directory(coordinator, COMMUNITY) {
        return Vec::new();
    }
    let ports = crate::server_info::Ports {
        api: 80,
        login: 21126,
        nat: Some(21128),
        api_tls: Some(443),
    };
    [
        ("tqzfwr7a4dhc67lp", "5th Echelon Community EU", "Falkenstein, Germany", "eu1.scbl.jdevwebb.net"),
        ("gbkmhlwthml6pggn", "5th Echelon Community North America", "Beauharnois, Canada", "na1.scbl.jdevwebb.net"),
        ("qzydgaemomioqsyb", "5th Echelon Community Oceania", "Sydney, Australia", "oceania.scbl.jdevwebb.net"),
    ]
    .into_iter()
    .map(|(id, name, region, host)| Listing {
        id: id.into(),
        name: name.into(),
        region: region.into(),
        host: host.into(),
        ports: Some(ports),
        version: String::new(),
        players_online: 0,
        players_total: 0,
        friends_mode: "mutual".into(),
    })
    .collect()
}

fn same_directory(a: &str, b: &str) -> bool {
    let key = |u: &str| u.trim().trim_end_matches('/').to_ascii_lowercase();
    key(a) == key(b)
}

/// One directory's last list, as kept in [`CACHE_FILE`].
#[derive(Debug, Clone, Deserialize, Serialize)]
struct Cached {
    directory: String,
    /// Unix seconds.
    saved_at: i64,
    servers: Vec<Listing>,
}

fn read_cache(dir: &Path) -> Vec<Cached> {
    std::fs::read(dir.join(CACHE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<Cached>>(&b).ok())
        .unwrap_or_default()
}

/// Keeps `servers` as `coordinator`'s last list (players online and all, as they were then).
pub fn save_cache(dir: &Path, coordinator: &str, servers: &[Listing], now: i64) {
    let mut cache = read_cache(dir);
    cache.retain(|c| !same_directory(&c.directory, coordinator));
    cache.insert(
        0,
        Cached {
            directory: coordinator.trim().trim_end_matches('/').to_string(),
            saved_at: now,
            servers: servers.iter().take(MAX_SERVERS).cloned().collect(),
        },
    );
    cache.truncate(MAX_CACHED);
    if let Ok(json) = serde_json::to_vec(&cache) {
        let _ = std::fs::create_dir_all(dir);
        let _ = crate::write_atomic(&dir.join(CACHE_FILE), &json);
    }
}

/// The servers to offer when `coordinator` doesn't answer: its last list, else the
/// built-in one, with a note to show the player. None when there's neither.
pub fn fallback(dir: Option<&Path>, coordinator: &str, now: i64) -> Option<(Vec<Listing>, String)> {
    let cached = dir.and_then(|d| read_cache(d).into_iter().find(|c| same_directory(&c.directory, coordinator)));
    if let Some(c) = cached.filter(|c| !c.servers.is_empty()) {
        // Players online then isn't players online now.
        let servers = c
            .servers
            .into_iter()
            .filter(|l| listable_host(&l.host))
            .map(|l| Listing { players_online: 0, ..l })
            .collect();
        return Some((servers, format!("The server directory didn't answer, so these are the servers it listed {}.", ago(now - c.saved_at))));
    }
    let built_in = built_in(coordinator);
    (!built_in.is_empty()).then(|| (built_in, "The server directory didn't answer, so these are the community servers the launcher knows.".to_string()))
}

/// Whether `coordinator` is a directory known here (built in, or listed before), so an
/// address that doesn't answer as one is still taken for a network.
pub fn known_network(dir: Option<&Path>, coordinator: &str) -> bool {
    !built_in(coordinator).is_empty() || dir.is_some_and(|d| read_cache(d).iter().any(|c| same_directory(&c.directory, coordinator)))
}

/// How long ago, `secs` seconds back: "within the last hour", "5 hours ago", "3 days ago".
pub fn ago(secs: i64) -> String {
    match secs.max(0) {
        s if s < 3600 => "within the last hour".into(),
        s if s < 2 * 86_400 => format!("{} hours ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(name: &str, players: u32) -> Listing {
        Listing {
            id: name.into(),
            name: name.into(),
            region: String::new(),
            host: format!("{name}.example.com"),
            ports: None,
            version: String::new(),
            players_online: players,
            players_total: 0,
            friends_mode: String::new(),
        }
    }

    #[test]
    fn a_directory_that_doesnt_answer_falls_back_to_its_last_list_then_the_built_in_one() {
        let dir = std::env::temp_dir().join(format!("fe-dir-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The community's servers are built in; another directory's aren't.
        let (servers, note) = fallback(Some(&dir), "https://play.scbl.jdevwebb.net/", 1000).unwrap();
        assert_eq!(servers.len(), 3);
        assert!(servers.iter().all(|l| listable_host(&l.host) && l.ports.is_some()));
        assert!(note.contains("launcher knows"), "{note}");
        assert!(fallback(Some(&dir), "https://other.example.com", 1000).is_none());
        assert!(known_network(None, COMMUNITY) && !known_network(Some(&dir), "https://other.example.com"));
        // A list it gave is kept, and comes back (without the players online then).
        save_cache(&dir, "https://other.example.com", &[listing("kiwi", 7)], 1000);
        let (servers, note) = fallback(Some(&dir), "https://OTHER.example.com/", 1000 + 3 * 3600).unwrap();
        assert_eq!((servers[0].host.as_str(), servers[0].players_online), ("kiwi.example.com", 0));
        assert!(note.contains("3 hours ago"), "{note}");
        assert!(known_network(Some(&dir), "https://other.example.com"));
        // The community's own last list wins over the built-in one.
        save_cache(&dir, COMMUNITY, &[listing("eu9", 1)], 1000);
        assert_eq!(fallback(Some(&dir), COMMUNITY, 1000).unwrap().0[0].host, "eu9.example.com");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_the_coordinators_answer() {
        let json = r#"{"servers":[{"id":"a1","name":"Kiwi Ops","region":"Sydney","host":"bl.example.com",
            "ports":{"api":80,"login":21126,"nat":21128},"version":"0.3.0","players_online":12,"players_total":80,
            "friends_mode":"mutual","listed":true,"seen_secs_ago":4}]}"#;
        let list = parse(json).unwrap();
        assert_eq!(list[0].name, "Kiwi Ops");
        assert_eq!(list[0].ports.unwrap().api, 80);
        assert_eq!(url("https://c.example.com/"), "https://c.example.com/v1/servers");
    }

    #[test]
    fn skips_bad_and_private_entries() {
        let json = r#"{"servers":[
            {"id":"a","name":"Good","host":"bl.example.com"},
            {"id":"b","name":"LAN","host":"192.168.1.10"},
            {"id":"c","name":"Here","host":"localhost"},
            {"id":"d","name":"Port","host":"bl.example.com:80"},
            {"id":"e","name":42,"host":"x.example.com"},
            {"id":"f","name":"Public IP\u0007\u202e","host":"203.0.114.9"}]}"#;
        let list = parse(json).unwrap();
        let names: Vec<_> = list.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Good", "Public IP"]);
        let many = format!(r#"{{"servers":[{}]}}"#, vec![r#"{"id":"a","name":"n","host":"a.example.com"}"#; 500].join(","));
        assert_eq!(parse(&many).unwrap().len(), MAX_SERVERS);
    }

    #[test]
    fn directories_need_https() {
        assert!(valid_coordinator("https://c.example.com"));
        assert!(!valid_coordinator("http://c.example.com"));
        assert!(!valid_coordinator("https://user:pw@c.example.com"));
        assert!(!valid_coordinator("c.example.com"));
    }

    #[test]
    fn picks_near_then_busy() {
        let servers = vec![
            (listing("far", 50), Some(180)),
            (listing("near-empty", 0), Some(20)),
            (listing("near-busy", 30), Some(30)),
            (listing("down", 99), None),
        ];
        assert_eq!(best(&servers), Some(2), "within 15 ms, the busier one");
        let servers = vec![(listing("near", 0), Some(20)), (listing("farther", 30), Some(60))];
        assert_eq!(best(&servers), Some(0));
        let servers = vec![(listing("a", 1), None), (listing("b", 5), None)];
        assert_eq!(best(&servers), Some(1), "nobody answered: the busiest");
        assert_eq!(best(&[]), None);
    }
}
