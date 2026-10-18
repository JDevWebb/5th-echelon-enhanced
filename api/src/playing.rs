//! What's being played on a server, for players deciding where to play: its public matches
//! (game mode, map, players and seats, never names), how many private matches there are, and
//! how many players wait in lobbies. A server sends it with its heartbeat (the directory lists
//! it with the server) and answers `GET /api/playing`; the launcher and the overlay show it.
//! The same numbers as a search in the game shows anyone.

use serde::Deserialize;
use serde::Serialize;

use crate::game_names;

/// The most matches listed (the busiest first).
pub const MAX_MATCHES: usize = 12;
/// The most players or seats a match is believed to have (the game's largest room has 8).
const MAX_SEATS: u32 = 16;
/// The most of anything counted.
const MAX_COUNT: u32 = 10_000;

/// A public match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    /// The game mode (attribute 102; [`game_names::mode`]), when the room said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_mode: Option<u32>,
    /// "svm" or "coop", for a mode with no name.
    #[serde(default)]
    pub kind: String,
    /// The map (attribute 101; [`game_names::map`]), when the room said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<u32>,
    #[serde(default)]
    pub players: u32,
    /// Its seats, public and private (0 when the room didn't say).
    #[serde(default)]
    pub seats: u32,
}

impl Match {
    /// "SvM Classic", "Grim (Ghost)", or "Spies vs Mercs" when the mode has no name.
    pub fn mode_name(&self) -> &'static str {
        game_names::match_mode(self.game_mode, &self.kind).unwrap_or("A match")
    }

    /// "6/8", or "6 players" when the seats aren't known.
    fn players_text(&self) -> String {
        match self.seats {
            0 => plural(self.players, "player"),
            seats => format!("{}/{seats}", self.players),
        }
    }

    /// "SvM Classic · Virus Vault · 6/8".
    pub fn line(&self) -> String {
        let mut parts = vec![self.mode_name().to_string()];
        parts.extend(self.map.and_then(game_names::map).map(String::from));
        parts.push(self.players_text());
        parts.join(" · ")
    }
}

/// What's being played on one server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playing {
    /// Public matches, the busiest first, at most [`MAX_MATCHES`].
    #[serde(default)]
    pub matches: Vec<Match>,
    /// Public matches past [`MAX_MATCHES`], and their players.
    #[serde(default)]
    pub more_matches: u32,
    #[serde(default)]
    pub more_players: u32,
    /// Private matches (invite only), and their players: counted, nothing more.
    #[serde(default)]
    pub private_matches: u32,
    #[serde(default)]
    pub private_players: u32,
    /// Players in lobbies (parties) and in no match.
    #[serde(default)]
    pub lobby_players: u32,
}

impl Playing {
    /// As read from a server: the numbers bounded and the list cut, so a server can't make
    /// the launcher or the overlay show nonsense.
    pub fn checked(self) -> Self {
        let mut matches = self.matches;
        matches.truncate(MAX_MATCHES);
        for m in &mut matches {
            if m.kind != "svm" && m.kind != "coop" {
                m.kind.clear();
            }
            m.players = m.players.min(MAX_SEATS);
            m.seats = m.seats.min(MAX_SEATS);
        }
        Self {
            matches,
            more_matches: self.more_matches.min(MAX_COUNT),
            more_players: self.more_players.min(MAX_COUNT),
            private_matches: self.private_matches.min(MAX_COUNT),
            private_players: self.private_players.min(MAX_COUNT),
            lobby_players: self.lobby_players.min(MAX_COUNT),
        }
    }

    /// Nobody playing.
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty() && self.more_matches == 0 && self.private_matches == 0 && self.lobby_players == 0
    }

    /// One line for a server's row: "SvM Classic 6/8 · 2 Grim (Ghost) matches, 4 players ·
    /// 1 private match · 3 in lobbies". Empty when nobody's playing.
    pub fn summary(&self) -> String {
        // By mode, in the order first listed (the busiest first).
        let mut modes: Vec<(&'static str, Vec<&Match>)> = Vec::new();
        for m in &self.matches {
            match modes.iter_mut().find(|(name, _)| *name == m.mode_name()) {
                Some((_, list)) => list.push(m),
                None => modes.push((m.mode_name(), vec![m])),
            }
        }
        let mut parts: Vec<String> = modes
            .into_iter()
            .map(|(name, list)| match list.as_slice() {
                [one] => format!("{name} {}", one.players_text()),
                many => format!("{} {name} matches, {}", many.len(), plural(many.iter().map(|m| m.players).sum(), "player")),
            })
            .collect();
        if self.more_matches > 0 {
            let matches = if self.more_matches == 1 { "match" } else { "matches" };
            parts.push(format!("{} more {matches} ({})", self.more_matches, plural(self.more_players, "player")));
        }
        if self.private_matches > 0 {
            parts.push(plural(self.private_matches, "private match"));
        }
        if self.lobby_players > 0 {
            parts.push(format!("{} in lobbies", self.lobby_players));
        }
        parts.join(" · ")
    }
}

/// What a friend is playing, for their line: "SvM Classic · Virus Vault", or "Co-op" when
/// the room says no more; `None` in a lobby (servers call a lobby co-op, but it's for
/// either).
pub fn activity_what(a: &crate::friends::Activity) -> Option<String> {
    if a.room != "match" {
        return None;
    }
    let game_mode = (a.game_mode != 0).then_some(a.game_mode);
    let name = game_names::match_mode(game_mode, &a.mode)?;
    let map = (a.map != 0).then(|| game_names::map(a.map)).flatten();
    Some(match map {
        Some(map) => format!("{name} · {map}"),
        None => name.to_string(),
    })
}

/// "1 player", "3 players"; "1 match", "2 matches".
fn plural(n: u32, one: &str) -> String {
    match (n, one.ends_with("ch")) {
        (1, _) => format!("1 {one}"),
        (n, true) => format!("{n} {one}es"),
        (n, false) => format!("{n} {one}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svm(game_mode: u32, players: u32, seats: u32) -> Match {
        Match {
            game_mode: Some(game_mode),
            kind: "svm".into(),
            map: Some(0x05F6_9010),
            players,
            seats,
        }
    }

    #[test]
    fn a_server_reads_in_one_line() {
        let mut p = Playing {
            matches: vec![svm(8, 6, 8), svm(7, 3, 8), svm(7, 1, 0)],
            private_matches: 1,
            private_players: 2,
            lobby_players: 3,
            ..Playing::default()
        };
        assert_eq!(p.summary(), "SvM Classic 6/8 · 2 SvM Blacklist matches, 4 players · 1 private match · 3 in lobbies");
        p.matches.push(Match {
            kind: "coop".into(),
            players: 1,
            ..Match::default()
        });
        p.more_matches = 2;
        p.more_players = 5;
        assert!(p.summary().contains("Co-op 1 player · 2 more matches (5 players)"), "{}", p.summary());
        assert_eq!(Playing::default().summary(), "");
        assert!(Playing::default().is_empty());
    }

    #[test]
    fn a_match_reads_with_its_map() {
        assert_eq!(svm(8, 6, 8).line(), "SvM Classic · Virus Vault · 6/8");
        let unknown = Match {
            game_mode: Some(99),
            kind: "coop".into(),
            map: Some(1),
            players: 2,
            seats: 0,
        };
        assert_eq!(unknown.line(), "Co-op · 2 players");
    }

    #[test]
    fn what_a_server_sends_is_bounded() {
        let wild = Playing {
            matches: (0..100)
                .map(|_| Match {
                    kind: "<b>".into(),
                    players: u32::MAX,
                    seats: u32::MAX,
                    ..Match::default()
                })
                .collect(),
            lobby_players: u32::MAX,
            ..Playing::default()
        }
        .checked();
        assert_eq!(wild.matches.len(), MAX_MATCHES);
        assert!(wild.matches.iter().all(|m| m.kind.is_empty() && m.players == MAX_SEATS && m.seats == MAX_SEATS));
        assert_eq!(wild.lobby_players, MAX_COUNT);
        assert_eq!(wild.matches[0].line(), "A match · 16/16");
        // Older servers say nothing of it.
        assert_eq!(serde_json::from_str::<Playing>("{}").unwrap(), Playing::default());
    }

    #[test]
    fn a_friend_reads_with_mode_and_map() {
        let a = |mode: &str, game_mode: u32, map: u32| crate::friends::Activity {
            mode: mode.into(),
            room: if mode.is_empty() { "lobby" } else { "match" }.into(),
            with: vec![],
            map,
            game_mode,
            joinable: false,
        };
        assert_eq!(activity_what(&a("svm", 8, 0x05F6_9010)).as_deref(), Some("SvM Classic · Virus Vault"));
        assert_eq!(activity_what(&a("coop", 3, 0)).as_deref(), Some("Grim (Ghost)"));
        assert_eq!(activity_what(&a("svm", 0, 0)).as_deref(), Some("Spies vs Mercs"), "an older server");
        assert_eq!(activity_what(&a("", 0, 0)), None, "a lobby");
    }
}
