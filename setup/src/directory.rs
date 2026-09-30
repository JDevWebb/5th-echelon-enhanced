//! The server directory a coordinator keeps (`GET /v1/servers`): community
//! servers that share friends, with how many players are on each. The
//! launcher measures its ping to each and suggests the best.

use serde::Deserialize;

/// One server in the directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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

fn listable_host(host: &str) -> bool {
    crate::net::valid_host(host) && host.parse::<std::net::IpAddr>().map_or(host != "localhost" && !host.ends_with(".localhost"), crate::net::is_public)
}

fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect()
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
            {"id":"f","name":"Public IP\u0007","host":"203.0.114.9"}]}"#;
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
