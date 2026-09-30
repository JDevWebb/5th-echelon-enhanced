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
    servers: Vec<Listing>,
}

/// The directory from the coordinator's answer.
pub fn parse(json: &str) -> anyhow::Result<Vec<Listing>> {
    Ok(serde_json::from_str::<Directory>(json)?.servers)
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
