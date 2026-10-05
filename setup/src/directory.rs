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
    /// Maintenance booked on it: the next few windows, by start (older coordinators
    /// don't say).
    #[serde(default, deserialize_with = "windows", skip_serializing_if = "Vec::is_empty")]
    pub maintenance: Vec<Window>,
}

/// A time a server (or the whole network) is booked to be down, from `start` to `end`
/// (Unix seconds), with a word for players. A notice only: nothing is stopped by it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Window {
    pub start: i64,
    pub end: i64,
    #[serde(default)]
    pub note: String,
}

/// The most windows read for a server.
const MAX_WINDOWS: usize = 3;
/// The longest window believed (admins book up to a day).
const MAX_WINDOW: i64 = 2 * DAY;

impl Window {
    /// The window as read, if it makes sense: its note on one line and cut.
    fn checked(self) -> Option<Self> {
        (self.start > 0 && self.end > self.start && self.end - self.start <= MAX_WINDOW).then(|| Self {
            note: hooks_config::text::clip(&self.note, 100),
            ..self
        })
    }
}

/// A server's windows: those that read as one, by start, at most [`MAX_WINDOWS`]. Anything
/// else in the field is let go rather than losing the server.
fn windows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Window>, D::Error> {
    let list = match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(list) => list,
        _ => return Ok(Vec::new()),
    };
    let mut windows: Vec<Window> = list.into_iter().filter_map(|v| serde_json::from_value::<Window>(v).ok()?.checked()).collect();
    windows.sort_by_key(|w| (w.start, w.end));
    windows.truncate(MAX_WINDOWS);
    Ok(windows)
}

/// The network's own window (its coordinator: friends across servers, the directory, new
/// names), from the directory's answer.
pub fn parse_network_maintenance(json: &str) -> Option<Window> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    serde_json::from_value::<Window>(v.get("network_maintenance")?.clone()).ok()?.checked()
}

/// How long after a window's end it's kept, for a server that isn't back.
pub const ENDED_KEPT: i64 = 12 * 3600;

/// Keeps the windows `previous` listed that have ended since (within [`ENDED_KEPT`]) on
/// the same servers in `servers`: the directory stops listing a window when it ends, but
/// a server that isn't back by then is overrunning it. A window gone before its end was
/// cancelled, and stays gone.
pub fn carry_ended(previous: &[Listing], servers: &mut [Listing], now: i64) {
    for server in servers.iter_mut() {
        let Some(before) = previous.iter().find(|p| p.host.eq_ignore_ascii_case(&server.host)) else {
            continue;
        };
        for w in before.maintenance.iter().rev() {
            if w.end <= now && now - w.end < ENDED_KEPT && !server.maintenance.contains(w) {
                server.maintenance.insert(0, w.clone());
            }
        }
    }
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

/// A release going out to the network's servers, as the directory says while it lasts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rollout {
    pub release: String,
    pub stage: Stage,
    /// When the stage began (Unix seconds).
    pub stage_started: i64,
    /// The server it's tried on first (its id).
    pub canary: Option<String>,
    /// How long the first server must run it fine before the rest update (seconds).
    pub healthy_for: i64,
    /// How long into the last stage a server with players on is updated anyway (seconds).
    pub quiet_wait: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The first server is installing it.
    Canary,
    /// The first server runs it, and must keep running fine for `healthy_for`.
    Verifying,
    /// Every other server updates, as soon as nobody is playing on it.
    Rolling,
}

/// The rollout in a directory's answer, if one is going out (and it reads as one).
pub fn parse_rollout(json: &str) -> Option<Rollout> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let r = v.get("rollout")?;
    let release = r["release"]
        .as_str()
        .filter(|t| !t.is_empty() && t.len() <= 32 && t.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')))?;
    let stage = match r["stage"].as_str()? {
        "canary" => Stage::Canary,
        "verifying" => Stage::Verifying,
        "rolling" => Stage::Rolling,
        _ => return None,
    };
    let within = |key: &str, max: i64| r[key].as_i64().filter(|n| (0..=max).contains(n));
    Some(Rollout {
        release: release.to_string(),
        stage,
        stage_started: within("stage_started", i64::MAX).filter(|t| *t > 0)?,
        canary: r["canary"].as_str().map(|c| clean(c)).filter(|c| !c.is_empty()),
        healthy_for: within("healthy_for", DAY)?,
        quiet_wait: within("quiet_wait", 7 * DAY)?,
    })
}

const DAY: i64 = 86_400;

/// Whether `host` may be offered to players by a server or a directory: a
/// host name, or an address on the internet (never this PC or its network).
/// A name that resolves into a private network is caught when it's used
/// (the launcher checks the address it resolved to).
pub fn listable_host(host: &str) -> bool {
    crate::net::valid_host(host)
        && host
            .parse::<std::net::IpAddr>()
            .map_or(host != "localhost" && !host.ends_with(".localhost"), crate::net::is_public)
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
        maintenance: Vec::new(),
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
    /// The network's own window then, so it's still known while the coordinator is down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    network_maintenance: Option<Window>,
}

fn read_cache(dir: &Path) -> Vec<Cached> {
    std::fs::read(dir.join(CACHE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<Cached>>(&b).ok())
        .unwrap_or_default()
}

/// Keeps `servers` as `coordinator`'s last list (players online and all, as they were
/// then), with the network's own maintenance window.
pub fn save_cache(dir: &Path, coordinator: &str, servers: &[Listing], network: Option<&Window>, now: i64) {
    let mut cache = read_cache(dir);
    cache.retain(|c| !same_directory(&c.directory, coordinator));
    cache.insert(
        0,
        Cached {
            directory: coordinator.trim().trim_end_matches('/').to_string(),
            saved_at: now,
            servers: servers.iter().take(MAX_SERVERS).cloned().collect(),
            network_maintenance: network.cloned(),
        },
    );
    cache.truncate(MAX_CACHED);
    if let Ok(json) = serde_json::to_vec(&cache) {
        let _ = std::fs::create_dir_all(dir);
        let _ = crate::write_atomic(&dir.join(CACHE_FILE), &json);
    }
}

/// `coordinator`'s last list as kept, for its windows: the servers then, and the network's
/// own window.
pub fn cached(dir: &Path, coordinator: &str) -> Option<(Vec<Listing>, Option<Window>)> {
    read_cache(dir)
        .into_iter()
        .find(|c| same_directory(&c.directory, coordinator))
        .map(|c| (c.servers, c.network_maintenance))
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
        return Some((
            servers,
            format!("The server directory didn't answer, so these are the servers it listed {}.", ago(now - c.saved_at)),
        ));
    }
    let built_in = built_in(coordinator);
    (!built_in.is_empty()).then(|| {
        (
            built_in,
            "The server directory didn't answer, so these are the community servers the launcher knows.".to_string(),
        )
    })
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

    #[test]
    fn rollouts_read_from_the_directory() {
        let json = r#"{"servers":[],"rollout":{"release":"0.4.2","stage":"verifying","stage_started":1791189000,"canary":"eu","healthy_for":600,"quiet_wait":7200}}"#;
        assert_eq!(
            parse_rollout(json),
            Some(Rollout {
                release: "0.4.2".into(),
                stage: Stage::Verifying,
                stage_started: 1_791_189_000,
                canary: Some("eu".into()),
                healthy_for: 600,
                quiet_wait: 7200,
            })
        );
        assert_eq!(parse_rollout(r#"{"servers":[]}"#), None, "nothing going out");
        for bad in [
            r#"{"rollout":{"release":"0.4.2","stage":"done","stage_started":1,"healthy_for":600,"quiet_wait":7200}}"#,
            r#"{"rollout":{"release":"<b>","stage":"rolling","stage_started":1,"healthy_for":600,"quiet_wait":7200}}"#,
            r#"{"rollout":{"release":"0.4.2","stage":"rolling","stage_started":1,"healthy_for":-1,"quiet_wait":7200}}"#,
            r#"{"rollout":{"release":"0.4.2","stage":"rolling","stage_started":"soon","healthy_for":600,"quiet_wait":7200}}"#,
        ] {
            assert_eq!(parse_rollout(bad), None, "{bad}");
        }
    }

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
            maintenance: Vec::new(),
        }
    }

    #[test]
    fn maintenance_read_from_the_directory() {
        let json = r#"{"servers":[
            {"id":"a","name":"A","host":"a.example.com","maintenance":[
                {"start":1791273600,"end":1791277200,"note":"Moving\nto a faster machine"},
                {"start":1791270000,"end":1791273600},
                {"start":1791280000,"end":1791270000,"note":"ends before it starts"},
                {"start":"soon","end":1791277200}]},
            {"id":"b","name":"B","host":"b.example.com","maintenance":"tonight"},
            {"id":"c","name":"C","host":"c.example.com"}],
            "network_maintenance":{"start":1791270000,"end":1791273600,"note":"New coordinator"}}"#;
        let list = parse(json).unwrap();
        assert_eq!(list.len(), 3, "a field that doesn't read loses only the field");
        let windows = &list[0].maintenance;
        assert_eq!(windows.len(), 2);
        assert_eq!((windows[0].start, windows[0].note.as_str()), (1_791_270_000, ""), "by start");
        assert_eq!(windows[1].note, "Movingto a faster machine", "one line");
        assert!(list[1].maintenance.is_empty() && list[2].maintenance.is_empty());
        assert_eq!(parse_network_maintenance(json).map(|w| w.note), Some("New coordinator".into()));
        assert_eq!(parse_network_maintenance(r#"{"servers":[]}"#), None);
        // Kept in the cache with the list, and the network's window beside it.
        let dir = std::env::temp_dir().join(format!("fe-dir-windows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let network = parse_network_maintenance(json);
        save_cache(&dir, COMMUNITY, &list, network.as_ref(), 1000);
        let (servers, kept) = cached(&dir, COMMUNITY).unwrap();
        assert_eq!((servers[0].maintenance.len(), kept), (2, network));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ended_windows_stay_for_a_server_not_back() {
        let window = |start: i64, end: i64| Window { start, end, note: String::new() };
        let mut before = listing("a", 0);
        before.maintenance = vec![window(1000, 2000), window(5000, 6000), window(9000, 9500)];
        let mut now_listed = vec![listing("a", 0), listing("b", 0)];
        now_listed[0].maintenance = vec![window(9000, 9500)];
        // At 7000: the first two ended (the directory no longer lists them); the third is
        // listed still.
        carry_ended(std::slice::from_ref(&before), &mut now_listed, 7000);
        assert_eq!(now_listed[0].maintenance, [window(1000, 2000), window(5000, 6000), window(9000, 9500)]);
        assert!(now_listed[1].maintenance.is_empty());
        // Not long after it ended: kept. Cancelled before its end: gone.
        let mut later = vec![listing("a", 0)];
        carry_ended(std::slice::from_ref(&before), &mut later, 2000 + ENDED_KEPT);
        assert_eq!(later[0].maintenance, [window(5000, 6000), window(9000, 9500)]);
        let mut cancelled = vec![listing("a", 0)];
        carry_ended(std::slice::from_ref(&before), &mut cancelled, 9200);
        assert_eq!(cancelled[0].maintenance, [window(1000, 2000), window(5000, 6000)]);
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
        save_cache(&dir, "https://other.example.com", &[listing("kiwi", 7)], None, 1000);
        let (servers, note) = fallback(Some(&dir), "https://OTHER.example.com/", 1000 + 3 * 3600).unwrap();
        assert_eq!((servers[0].host.as_str(), servers[0].players_online), ("kiwi.example.com", 0));
        assert!(note.contains("3 hours ago"), "{note}");
        assert!(known_network(Some(&dir), "https://other.example.com"));
        // The community's own last list wins over the built-in one.
        save_cache(&dir, COMMUNITY, &[listing("eu9", 1)], None, 1000);
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
