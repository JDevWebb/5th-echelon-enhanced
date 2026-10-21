//! What's being played here, for players deciding where to play (`server_api::playing`): the
//! public matches by mode, map, players and seats, the private ones and the lobbies only
//! counted, and no names. Sent with the heartbeat (the directory lists it with this server)
//! and answered at `GET /api/playing`. A search in the game shows anyone as much.

use std::collections::HashSet;
use std::time::Duration;

use server_api::playing::Match;
use server_api::playing::Playing;
use server_api::playing::MAX_MATCHES;

use crate::game_session::attribute_value;
use crate::storage::LiveSession;

/// How long `/api/playing` answers from what it last worked out.
pub const FRESH_FOR: Duration = Duration::from_secs(5);

/// Session attributes (see game_session.rs): seats (public, private), map, game mode, room
/// kind (0 a match, 1 a lobby).
const ATTR_PUBLIC_SEATS: u32 = 3;
const ATTR_PRIVATE_SEATS: u32 = 4;
const ATTR_MAP: u32 = 101;
const ATTR_GAME_MODE: u32 = 102;
const ATTR_ROOM_KIND: u32 = 113;

/// What's being played in `sessions` (the live ones, with their players).
pub fn of(sessions: &[LiveSession]) -> Playing {
    let is_match = |s: &&LiveSession| attribute_value(&s.attributes, ATTR_ROOM_KIND) == Some(0);
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let mut playing = Playing::default();
    let mut public: Vec<Match> = Vec::new();
    for s in sessions.iter().filter(is_match) {
        if s.private {
            playing.private_matches += 1;
            playing.private_players += count(s.players.len());
            continue;
        }
        let seats = [ATTR_PUBLIC_SEATS, ATTR_PRIVATE_SEATS]
            .into_iter()
            .map(|a| attribute_value(&s.attributes, a).unwrap_or(0))
            .fold(0u32, u32::saturating_add);
        public.push(Match {
            game_mode: attribute_value(&s.attributes, ATTR_GAME_MODE),
            kind: crate::game_session::match_mode(&s.attributes).unwrap_or_default().to_string(),
            map: attribute_value(&s.attributes, ATTR_MAP),
            players: count(s.players.len()),
            seats,
            mode_size: crate::game_session::mode_size(&s.attributes).unwrap_or(0),
        });
    }
    // The busiest first; then the newest (sessions come newest first, and the sort keeps it).
    public.sort_by_key(|m| std::cmp::Reverse(m.players));
    for m in public.drain(MAX_MATCHES.min(public.len())..) {
        playing.more_matches += 1;
        playing.more_players += m.players;
    }
    playing.matches = public;
    // In a lobby and in no match: waiting, or choosing what to play.
    let in_matches: HashSet<&String> = sessions.iter().filter(is_match).flat_map(|s| &s.players).collect();
    let waiting: HashSet<&String> = sessions
        .iter()
        .filter(|s| !is_match(s))
        .flat_map(|s| &s.players)
        .filter(|p| !in_matches.contains(p))
        .collect();
    playing.lobby_players = count(waiting.len());
    playing.checked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(attributes: &str, players: &[&str], private: bool) -> LiveSession {
        LiveSession {
            attributes: attributes.into(),
            players: players.iter().map(|p| p.to_string()).collect(),
            private,
            ..LiveSession::default()
        }
    }

    #[test]
    fn public_matches_are_listed_and_the_rest_counted() {
        let sessions = vec![
            session("113 => 0;103 => 1;3 => 8;4 => 0;101 => 100044816;102 => 8", &["A", "B", "C"], false),
            session("113 => 0;3 => 0;4 => 2;105 => 2;102 => 3", &["D", "E"], true),
            session("113 => 0;3 => 2;4 => 0;105 => 2;101 => 3578398534;102 => 3", &["F"], false),
            // A's party around the match, and two waiting in a lobby.
            session("113 => 1", &["A", "B"], false),
            session("113 => 1", &["G", "H"], false),
        ];
        let p = of(&sessions);
        assert_eq!(p.matches.len(), 2, "public matches only");
        assert_eq!(
            p.matches[0].line(),
            "SvM Classic · Virus Vault · 3/8 (made for 4)",
            "the busiest first, raised past its mode's 4"
        );
        assert_eq!(p.matches[1].line(), "Grim (Ghost) · Hawkins Seafort · 1/2");
        assert_eq!((p.private_matches, p.private_players), (1, 2), "private ones counted, nothing more");
        assert_eq!(p.lobby_players, 2, "those in a match aren't also waiting");
        assert_eq!(p.summary(), "SvM Classic 3/8 (made for 4) · Grim (Ghost) 1/2 · 1 private match · 2 in lobbies");
        assert!(of(&[]).is_empty());
    }

    #[test]
    fn many_matches_are_cut_and_counted() {
        let sessions: Vec<LiveSession> = (0..MAX_MATCHES + 3)
            .map(|i| session("113 => 0;103 => 1;102 => 7", if i == 0 { &["A", "B"] } else { &["X"] }, false))
            .collect();
        let p = of(&sessions);
        assert_eq!(p.matches.len(), MAX_MATCHES);
        assert_eq!(p.matches[0].players, 2);
        assert_eq!((p.more_matches, p.more_players), (3, 3));
    }
}
