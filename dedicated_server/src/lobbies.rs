//! The public matches in full, for the launcher's Matches tab (friends.proto's `Lobbies`): who's
//! in each (its host, who came as a party, the caller's friends, each one's side as the host's
//! game says), its state and settings as the host's game last said them, and how far each
//! player is from the caller. Unlike `/api/playing` (anyone, no names), only for signed-in
//! players, and a match with someone the caller blocked, or who blocked them, is left out (only
//! the caller's own blocks counted: nobody learns who blocked them, as JoinRoom's "gone").
//! Distances are coarse: the caller's to a match's host and to each player, in [`PING_STEP`]s.
//! They were given for friends only (2026-11-04: so the list couldn't be used to find where a
//! stranger is); the user, 2026-11-07: a public match's pings are what a player picks it by. A
//! 25 ms step says a region of the world at best, the numbers are the server's estimates (by
//! relays and by address), not a probe of the player's PC, and the game's own scoreboard shows
//! everyone's.
//!
//! The host's game says its match's state, settings and sides with each MatchState ([`said`]);
//! [`list`] puts the rest together from the live rooms.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use server_api::friends::LobbiesResponse;
use server_api::friends::Lobby;
use server_api::friends::LobbyPlayer;

use crate::game_session::attribute_value;
use crate::storage::LiveSession;
use crate::storage::Relation;

/// How long what a host's game said of its match holds without being said again (it says it
/// every 30 s while a round runs, and at each change).
const HOLDS: Duration = Duration::from_secs(10 * 60);
/// The most rooms remembered.
const MOST_ROOMS: usize = 4096;
/// Pings are told in steps of this (ms): about 2,500 km of distance a step.
const PING_STEP: u32 = 25;

/// A ping as told: rounded up to the next [`PING_STEP`] (0 stays unknown).
fn coarse(ms: u32) -> u32 {
    ms.div_ceil(PING_STEP) * PING_STEP
}

/// The most matches answered, and players a match.
const MOST_LOBBIES: usize = 50;
const MOST_PLAYERS: usize = 24;

/// What a match's host's game last said of it.
#[derive(Debug, Clone, Default)]
struct Said {
    state: String,
    settings: BTreeMap<String, f32>,
    /// Each player's side ("spy", "merc"), by name.
    sides: HashMap<String, String>,
    /// Rounds started: each time its state went to RoundState_Started.
    round: u32,
    at: Option<Instant>,
}

static ROOMS: Mutex<Option<HashMap<u32, Said>>> = Mutex::new(None);

/// The host's game said of its match `rooms` (the ones it hosts): its state, settings and who's
/// on which side.
pub fn said(rooms: &[u32], state: &str, settings: &HashMap<String, f32>, team: &[server_api::misc::TeamPlayer]) {
    let now = Instant::now();
    let mut kept = ROOMS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = kept.get_or_insert_with(HashMap::new);
    if map.len() >= MOST_ROOMS {
        map.retain(|_, s| s.at.is_some_and(|at| now.duration_since(at) < HOLDS));
    }
    let sides: HashMap<String, String> = team
        .iter()
        .filter(|t| matches!(t.side.as_str(), "spy" | "merc") && !t.name.is_empty() && t.name.len() <= 32)
        .take(MOST_PLAYERS)
        .map(|t| (t.name.to_lowercase(), t.side.clone()))
        .collect();
    for room in rooms {
        if map.len() >= MOST_ROOMS && !map.contains_key(room) {
            continue;
        }
        let s = map.entry(*room).or_default();
        if state == "RoundState_Started" && s.state != state {
            s.round += 1;
        }
        s.state = state.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(40).collect();
        s.settings = settings
            .iter()
            .filter(|(k, v)| k.len() <= 40 && v.is_finite())
            .take(32)
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        // Sides are only said while characters are about: a lobby's word keeps the last.
        if !sides.is_empty() {
            s.sides = sides.clone();
        }
        s.at = Some(now);
    }
}

/// A room is gone: what was said of it goes.
pub fn ended(room: u32) {
    if let Some(map) = ROOMS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut() {
        map.remove(&room);
    }
}

fn said_of(room: u32) -> Option<Said> {
    let kept = ROOMS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.as_ref()?.get(&room).filter(|s| s.at.is_some_and(|at| at.elapsed() < HOLDS)).cloned()
}

/// What's known of a room beyond the database: whether it's under way and for how long, and
/// each player's and the host's distance from the caller (ms).
pub struct Live<'a> {
    pub under_way: &'a dyn Fn(u32) -> Option<Duration>,
    pub ping: &'a dyn Fn(u32) -> Option<u32>,
    /// Whether a host migration moved a room's players on to another room: the old one is
    /// left out. Its silent host stays in it until their connection lapses (a minute), and it
    /// was listed meanwhile beside the room its players are in, "under way" (the user,
    /// watching Matches through a host killed mid-round, 2026-11-07).
    pub moved_on: &'a dyn Fn(u32) -> bool,
    /// How many wait to join a room (match_queue.rs), and the caller's place among them.
    pub queue: &'a dyn Fn(u32) -> (usize, usize),
}

/// The public matches in `sessions` (the live rooms) as `me` sees them, with `relations` (from
/// `me`'s side), at `now` (Unix seconds).
pub fn list(me: u32, sessions: &[LiveSession], relations: &HashMap<u32, Relation>, live: &Live, now: i64) -> LobbiesResponse {
    let is_match = |s: &LiveSession| attribute_value(&s.attributes, 113) == Some(0);
    let mut out = LobbiesResponse::default();
    let in_matches: HashSet<u32> = sessions.iter().filter(|s| is_match(s)).flat_map(|s| s.player_ids.iter().copied()).collect();
    let parties: Vec<&LiveSession> = sessions.iter().filter(|s| !is_match(s)).collect();
    out.party_players = u32::try_from(parties.iter().flat_map(|s| &s.player_ids).filter(|p| !in_matches.contains(p)).collect::<HashSet<_>>().len()).unwrap_or(u32::MAX);
    let mut lobbies: Vec<(bool, Lobby)> = Vec::new();
    for s in sessions.iter().filter(|s| is_match(s)) {
        if (live.moved_on)(s.id) {
            continue;
        }
        if s.private {
            out.private_matches += 1;
            out.private_players += u32::try_from(s.players.len()).unwrap_or(u32::MAX);
            continue;
        }
        // Someone the caller blocked: left out and counted. Someone who blocked the caller: left
        // out without a word, so the count can't tell them who.
        if s.player_ids.iter().any(|p| matches!(relations.get(p), Some(Relation::BlockedBy))) {
            continue;
        }
        if s.player_ids.iter().any(|p| matches!(relations.get(p), Some(Relation::Blocked))) {
            out.hidden_blocked += 1;
            continue;
        }
        // Who came together: two or more of this match's players in one party.
        let mut party_of: HashMap<u32, u32> = HashMap::new();
        let mut next = 1;
        for party in &parties {
            let here: Vec<u32> = party.player_ids.iter().copied().filter(|p| s.player_ids.contains(p)).collect();
            if here.len() >= 2 && here.iter().all(|p| !party_of.contains_key(p)) {
                for p in here {
                    party_of.insert(p, next);
                }
                next += 1;
            }
        }
        let said = said_of(s.id).unwrap_or_default();
        let (waiting, place) = (live.queue)(s.id);
        let under_way = (live.under_way)(s.id);
        let mut players: Vec<LobbyPlayer> = s
            .player_ids
            .iter()
            .zip(&s.players)
            .take(MOST_PLAYERS)
            .map(|(id, name)| {
                let friend = matches!(relations.get(id), Some(Relation::Friend));
                LobbyPlayer {
                    name: name.clone(),
                    host: *id == s.host_id,
                    party: party_of.get(id).copied().unwrap_or(0),
                    friend,
                    team: said.sides.get(&name.to_lowercase()).cloned().unwrap_or_default(),
                    // Everyone's but the caller's own, coarse.
                    ping_ms: if *id != me { coarse((live.ping)(*id).unwrap_or(0)) } else { 0 },
                }
            })
            .collect();
        // The host first, then by party, then by name.
        players.sort_by_key(|p| (!p.host, p.party == 0, p.party, p.name.to_lowercase()));
        let seats = [3, 4].into_iter().map(|a| attribute_value(&s.attributes, a).unwrap_or(0)).fold(0u32, u32::saturating_add);
        let friends_in = players.iter().any(|p| p.friend);
        lobbies.push((
            friends_in,
            Lobby {
                room: s.id,
                kind: crate::game_session::match_mode(&s.attributes).unwrap_or_default().to_string(),
                game_mode: attribute_value(&s.attributes, 102).unwrap_or(0),
                map: attribute_value(&s.attributes, 101).unwrap_or(0),
                seats,
                mode_size: crate::game_session::mode_size(&s.attributes).unwrap_or(0),
                open_secs: u32::try_from((now - s.created).max(0)).unwrap_or(u32::MAX),
                state: said.state.clone(),
                under_way: under_way.is_some(),
                under_way_secs: under_way.map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX)),
                round: said.round,
                settings: said.settings.iter().map(|(k, v)| (k.clone(), *v)).collect(),
                ping_ms: coarse((live.ping)(s.host_id).unwrap_or(0)),
                players,
                waiting: u32::try_from(waiting).unwrap_or(u32::MAX),
                queue_place: u32::try_from(place).unwrap_or(u32::MAX),
            },
        ));
    }
    // The caller's friends' matches first, then the busiest, then the newest.
    lobbies.sort_by_key(|(friends, l)| (!friends, std::cmp::Reverse(l.players.len()), l.open_secs));
    out.lobbies = lobbies.into_iter().take(MOST_LOBBIES).map(|(_, l)| l).collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: u32, attributes: &str, host: u32, players: &[(u32, &str)], private: bool) -> LiveSession {
        LiveSession {
            id,
            attributes: attributes.into(),
            host_id: host,
            created: 1000,
            private,
            player_ids: players.iter().map(|(i, _)| *i).collect(),
            players: players.iter().map(|(_, n)| (*n).to_string()).collect(),
            ..LiveSession::default()
        }
    }

    /// Names, the host, a party, friends and sides; a match with someone blocked either way left
    /// out and counted; private matches and parties only counted; friends' matches first.
    #[test]
    fn public_matches_in_full_as_the_caller_sees_them() {
        const SVM: &str = "113 => 0;103 => 1;3 => 8;4 => 0;101 => 100044816;102 => 7";
        let sessions = vec![
            room(10, SVM, 1, &[(1, "Liara"), (2, "Tank"), (3, "mpw"), (4, "Jgz24")], false),
            room(11, SVM, 5, &[(5, "Ma1orenko"), (6, "Tea_bag")], false),
            room(12, SVM, 7, &[(7, "Griefer"), (8, "Other")], false),
            room(13, SVM, 9, &[(9, "Blocker")], false),
            room(14, SVM, 20, &[(20, "Priv"), (21, "Ate")], true),
            // mpw and Jgz24 came as a party; someone waiting in a party of their own.
            room(30, "113 => 1", 3, &[(3, "mpw"), (4, "Jgz24")], false),
            room(31, "113 => 1", 40, &[(40, "Waiting")], false),
        ];
        let relations: HashMap<u32, Relation> = [(2, Relation::Friend), (7, Relation::Blocked), (9, Relation::BlockedBy)].into_iter().collect();
        said(&[11], "RoundState_PreRound", &HashMap::new(), &[]);
        said(
            &[11],
            "RoundState_Started",
            &[("m_fSpyRespawnTime".to_string(), 7.0)].into_iter().collect(),
            &[
                server_api::misc::TeamPlayer {
                    name: "Ma1orenko".into(),
                    side: "spy".into(),
                },
                server_api::misc::TeamPlayer {
                    name: "tea_bag".into(),
                    side: "merc".into(),
                },
                server_api::misc::TeamPlayer {
                    name: "x".into(),
                    side: "pirate".into(),
                },
            ],
        );
        let under_way = |room: u32| (room == 11).then_some(Duration::from_secs(360));
        let ping = |id: u32| (id != 4).then_some(30 + id);
        let r = list(
            99,
            &sessions,
            &relations,
            &Live {
                under_way: &under_way,
                ping: &ping,
                moved_on: &|_| false,
                queue: &|room| if room == 10 { (2, 1) } else { (0, 0) },
            },
            1240,
        );
        assert_eq!(
            (r.hidden_blocked, r.private_matches, r.private_players, r.party_players),
            (1, 1, 2, 1),
            "only the caller's own block counted: one who blocked them is left out without a word"
        );
        assert_eq!(r.lobbies.iter().map(|l| l.room).collect::<Vec<_>>(), vec![10, 11], "a friend's match first");
        let first = &r.lobbies[0];
        let who: Vec<(&str, bool, u32, bool, u32)> = first.players.iter().map(|p| (p.name.as_str(), p.host, p.party, p.friend, p.ping_ms)).collect();
        assert_eq!(
            who,
            vec![
                ("Liara", true, 0, false, 50),
                ("Jgz24", false, 1, false, 0),
                ("mpw", false, 1, false, 50),
                ("Tank", false, 0, true, 50)
            ],
            "every player's ping (bar their own), in 25 ms steps"
        );
        assert_eq!((first.seats, first.open_secs, first.ping_ms, first.under_way), (8, 240, 50, false), "the host's, in steps");
        assert_eq!((first.waiting, first.queue_place), (2, 1), "who waits for it, and the caller's place");
        let second = &r.lobbies[1];
        assert_eq!(
            (second.state.as_str(), second.round, second.under_way, second.under_way_secs),
            ("RoundState_Started", 1, true, 360)
        );
        assert_eq!(second.settings.get("m_fSpyRespawnTime"), Some(&7.0));
        assert_eq!(second.players.iter().map(|p| p.team.as_str()).collect::<Vec<_>>(), vec!["spy", "merc"]);
        ended(11);
        let r = list(
            99,
            &sessions,
            &relations,
            &Live {
                under_way: &under_way,
                ping: &ping,
                moved_on: &|_| false,
                queue: &|room| if room == 10 { (2, 1) } else { (0, 0) },
            },
            1240,
        );
        assert_eq!(r.lobbies[1].state, "", "an ended room's word goes with it");
        let r = list(
            99,
            &sessions,
            &relations,
            &Live {
                under_way: &under_way,
                ping: &ping,
                moved_on: &|room| room == 10,
                queue: &|_| (0, 0),
            },
            1240,
        );
        assert_eq!(r.lobbies.iter().map(|l| l.room).collect::<Vec<_>>(), vec![11], "a room whose players moved on is left out");
    }
}
