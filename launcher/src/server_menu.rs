//! The server menu on the Play screen: the network's servers, best first, picked in one
//! click. Anything else (a server of your own, a LAN party's, another network) is on the
//! Servers screen.

use setup::directory::Listing;

/// One server in the menu.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Row {
    pub host: String,
    /// Where it is ("Sydney, Australia"), or its name when it doesn't say.
    pub place: String,
    /// The network's name for it, without the words every server's name starts with
    /// ("Oceania" for "5th Echelon Community Oceania").
    pub name: String,
    /// Its whole name with where it is ("5th Echelon Community EU (Falkenstein, Germany)"), as
    /// the guided setup lists it.
    pub title: String,
    pub ping: Option<u32>,
    pub players: u32,
    /// What's being played there ("SvM Classic 6/8 · 3 in lobbies"), shown on hover; empty
    /// when nobody is, or the server doesn't say.
    pub playing: String,
    /// The player is on it.
    pub current: bool,
    pub best: bool,
    /// Only friends of friends can join its players' games.
    pub friends_only: bool,
    /// Its maintenance, when a window shows ("Maintenance 8 pm", "Back 9 pm").
    pub maintenance: Option<String>,
}

/// The servers in the order to offer them: the best first ([`setup::directory::best`]),
/// then by ping (no answer last), then the busiest.
pub fn ranked(servers: &[(Listing, Option<u32>)]) -> Vec<usize> {
    let best = setup::directory::best(servers);
    let mut order: Vec<usize> = (0..servers.len()).collect();
    order.sort_by_key(|&i| {
        let (listing, ping) = &servers[i];
        (Some(i) != best, ping.is_none(), ping.unwrap_or(u32::MAX), std::cmp::Reverse(listing.players_online), i)
    });
    order
}

/// The menu's rows, best first, for the player on `current` (a host, as their settings
/// have it), with each server's maintenance as it shows `now` on `clock`.
pub fn rows(servers: &[(Listing, Option<u32>)], current: &str, now: i64, clock: &setup::clock::Clock) -> Vec<Row> {
    let best = setup::directory::best(servers);
    let names = short_names(&servers.iter().map(|(s, _)| s.name.as_str()).collect::<Vec<_>>());
    ranked(servers)
        .into_iter()
        .map(|i| {
            let (s, ping) = &servers[i];
            Row {
                host: s.host.clone(),
                place: if s.region.is_empty() { s.name.clone() } else { s.region.clone() },
                name: names[i].clone(),
                title: if s.region.is_empty() { s.name.clone() } else { format!("{} ({})", s.name, s.region) },
                ping: *ping,
                players: s.players_online,
                playing: s.playing.as_ref().map(server_api::playing::Playing::summary).unwrap_or_default(),
                current: s.host.eq_ignore_ascii_case(current.trim()),
                best: best == Some(i),
                friends_only: s.friends_mode == "mutual",
                maintenance: crate::maintenance::tag(s, *ping, now, clock),
            }
        })
        .collect()
}

/// The names without the words they all start with, when there are several and something
/// is left of each.
fn short_names(names: &[&str]) -> Vec<String> {
    let words: Vec<Vec<&str>> = names.iter().map(|n| n.split_whitespace().collect()).collect();
    let shortest = words.iter().map(Vec::len).min().unwrap_or(0);
    let common = (0..shortest).take_while(|&k| words.iter().all(|w| w[k] == words[0][k])).count();
    if names.len() < 2 || common == 0 || words.iter().any(|w| w.len() == common) {
        return names.iter().map(|n| (*n).to_string()).collect();
    }
    words.iter().map(|w| w[common..].join(" ")).collect()
}

/// What the player did in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Server(String),
    PingAgain,
    ServersScreen,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(name: &str, region: &str, host: &str, players: u32) -> Listing {
        Listing {
            id: host.into(),
            name: name.into(),
            region: region.into(),
            host: host.into(),
            also: Vec::new(),
            ports: None,
            version: String::new(),
            players_online: players,
            players_total: 0,
            friends_mode: String::new(),
            maintenance: Vec::new(),
            playing: None,
        }
    }

    fn utc() -> setup::clock::Clock {
        setup::clock::Clock::utc()
    }

    #[test]
    fn maintenance_tags_the_server() {
        let mut servers = network();
        // 20:00 to 21:00 UTC on the first day.
        servers[2].0.maintenance = vec![setup::directory::Window {
            start: 20 * 3600,
            end: 21 * 3600,
            note: String::new(),
            update: false,
            closed: false,
        }];
        let tags = |now: i64| rows(&servers, "", now, &utc()).into_iter().map(|r| r.maintenance).collect::<Vec<_>>();
        assert_eq!(tags(3600), [Some("Maintenance 20:00".to_string()), None, None, None]);
        assert_eq!(tags(20 * 3600 + 60)[0].as_deref(), Some("Back 21:00"));
        assert_eq!(tags(22 * 3600)[0], None, "it answers: back");
    }

    fn network() -> Vec<(Listing, Option<u32>)> {
        vec![
            (listing("Example Net Europe", "Falkenstein, Germany", "eu.example.net", 0), Some(366)),
            (listing("Example Net North America", "Beauharnois, Canada", "na.example.net", 2), Some(213)),
            (listing("Example Net Oceania", "Sydney, Australia", "oce.example.net", 0), Some(45)),
            (listing("Example Net Asia", "", "asia.example.net", 9), None),
        ]
    }

    #[test]
    fn best_first_then_by_ping() {
        let rows = rows(&network(), "na.example.net", 0, &utc());
        let hosts: Vec<&str> = rows.iter().map(|r| r.host.as_str()).collect();
        assert_eq!(hosts, ["oce.example.net", "na.example.net", "eu.example.net", "asia.example.net"]);
        assert!(rows[0].best && !rows[1].best);
        assert!(rows[1].current && !rows[0].current);
        assert_eq!(ranked(&network()), [2, 1, 0, 3]);
    }

    #[test]
    fn labels_say_where_and_which() {
        let rows = rows(&network(), "OCE.example.net ", 0, &utc());
        assert_eq!(rows[0].place, "Sydney, Australia");
        assert_eq!(rows[0].name, "Oceania");
        assert_eq!(rows[1].name, "North America");
        assert!(rows[0].current, "hosts compare without case or spaces");
        // No region: its name stands in.
        assert_eq!(rows[3].place, "Example Net Asia");
        assert_eq!((rows[3].ping, rows[3].players), (None, 9));
    }

    #[test]
    fn busier_server_wins_a_close_ping() {
        let servers = vec![(listing("A", "", "a.example.net", 0), Some(40)), (listing("B", "", "b.example.net", 5), Some(50))];
        let rows = rows(&servers, "", 0, &utc());
        assert_eq!(rows[0].host, "b.example.net");
        assert!(rows[0].best);
    }

    #[test]
    fn names_lose_only_the_words_they_share() {
        assert_eq!(short_names(&["5th Echelon EU", "5th Echelon NA"]), ["EU", "NA"]);
        assert_eq!(short_names(&["Alpha", "Beta"]), ["Alpha", "Beta"]);
        // Nothing would be left of one: all kept as they are.
        assert_eq!(short_names(&["Net", "Net Two"]), ["Net", "Net Two"]);
        assert_eq!(short_names(&["Only One Server"]), ["Only One Server"]);
        assert!(short_names(&[]).is_empty());
    }
}
