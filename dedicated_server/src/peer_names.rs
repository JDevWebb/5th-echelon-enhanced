//! Each player's number in their match's game session (the 0.4.8 recordings and witness
//! reports): a game only has its own player's name in memory, not the others', but every
//! character carries its owner's number (Actor.NetInstanceId's top byte: the session peer index
//! + 1, the same in every game). Each game says its own with its match state; the server answers
//! with the numbers of everyone in its room, so each game can name every character.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// A number said longer ago than this is forgotten: games say theirs every 30 s while a round
/// runs, and an older one (a match before) would only clash with this one's.
const KEPT_FOR: Duration = Duration::from_secs(3 * 60);
/// Players remembered at most (the oldest go).
const MOST: usize = 50_000;
/// The numbers a game session has (Blacklist's sessions are 16 players at most; room to spare).
pub const MOST_PEER: u32 = 32;

static SAID: LazyLock<Mutex<HashMap<u32, (u32, Instant)>>> = LazyLock::new(Default::default);

/// `user` said their number is `peer` (0: not known, nothing kept).
pub fn said(user: u32, peer: u32) {
    if peer == 0 || peer > MOST_PEER {
        return;
    }
    let mut all = SAID.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if all.len() >= MOST && !all.contains_key(&user) {
        let now = Instant::now();
        all.retain(|_, (_, at)| now.duration_since(*at) < KEPT_FOR);
        if all.len() >= MOST {
            return;
        }
    }
    all.insert(user, (peer, Instant::now()));
}

/// The numbers of `players` (id, name) who said one lately. A number is the game's own claim
/// (the server can't see the players' session), so a number two of them claim names neither:
/// a player can't take another's, putting their name on someone else's character in the
/// others' recordings and witness reports, or leave their own unnamed.
pub fn of(players: &[(u32, String)]) -> Vec<(u32, String)> {
    let all = SAID.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    let mut by_peer: HashMap<u32, Option<String>> = HashMap::new();
    for (id, name) in players {
        let Some((peer, _)) = all.get(id).filter(|(_, at)| now.duration_since(*at) < KEPT_FOR) else {
            continue;
        };
        by_peer.entry(*peer).and_modify(|n| *n = None).or_insert_with(|| Some(name.clone()));
    }
    let mut out: Vec<(u32, String)> = by_peer.into_iter().filter_map(|(peer, name)| Some((peer, name?))).collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rooms_players_are_named_by_the_numbers_they_said() {
        said(900_001, 1);
        said(900_002, 3);
        said(900_003, 0);
        said(900_004, 99);
        let room = |ids: &[u32]| ids.iter().map(|id| (*id, format!("p{id}"))).collect::<Vec<_>>();
        assert_eq!(
            of(&room(&[900_001, 900_002, 900_003, 900_004, 900_005])),
            vec![(1, "p900001".into()), (3, "p900002".into())]
        );
        // A number two players in the room claim names neither (one of them isn't telling the
        // truth); the others are still named.
        said(900_006, 3);
        assert_eq!(of(&room(&[900_001, 900_002, 900_006])), vec![(1, "p900001".into())]);
        // Saying another number later (a new match) is theirs alone again.
        said(900_006, 4);
        assert_eq!(of(&room(&[900_002, 900_006])), vec![(3, "p900002".into()), (4, "p900006".into())]);
    }
}
