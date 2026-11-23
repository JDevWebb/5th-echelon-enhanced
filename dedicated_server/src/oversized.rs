//! A match past its mode's size (0.5.6's R3). A host can raise a match's seats past what its mode
//! was made for (the overlay's "Most players", or the game's own lobby setting), and above it some
//! players' games crash or freeze more often (the 20 Oct review's oversized lobbies). Decided: warn
//! only, never refused. Each player in such a match is told once (an overlay notice), and the
//! match is noted for the Sessions page (`oversized`, once a room).

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::PoisonError;

/// The (player, room) pairs told, and the rooms noted.
static TOLD: Mutex<Option<(HashSet<(u32, u32)>, HashSet<u32>)>> = Mutex::new(None);
const MOST_KEPT: usize = 50_000;

/// `user`'s events poll found them in `room`, with `players` players in a mode made for `size`.
pub fn seen(user: u32, room: u32, size: u32, players: u32) {
    let (tell, note) = first(user, room);
    if note {
        crate::session_events::note(
            crate::session_events::Who::Id(user),
            "oversized",
            serde_json::json!({ "room": room, "size": size, "players": players }),
        );
    }
    if tell {
        crate::reports::tell(
            user,
            String::from("This match is past its mode's size"),
            format!("{players} players are in a match made for {size}. Above that some players' games crash or freeze more often. You can still play."),
            None,
        );
    }
}

/// Whether to tell `user` about `room`, and whether to note the room: each once.
fn first(user: u32, room: u32) -> (bool, bool) {
    let mut told = TOLD.lock().unwrap_or_else(PoisonError::into_inner);
    let (players, rooms) = told.get_or_insert_with(Default::default);
    if players.len() > MOST_KEPT {
        players.clear();
    }
    if rooms.len() > MOST_KEPT {
        rooms.clear();
    }
    (players.insert((user, room)), rooms.insert(room))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_player_is_told_once_and_the_room_noted_once() {
        assert_eq!(first(900_001, 77_001), (true, true));
        assert_eq!(first(900_001, 77_001), (false, false), "again: nothing");
        assert_eq!(first(900_002, 77_001), (true, false), "another player in it: told, the room noted already");
        assert_eq!(first(900_001, 77_002), (true, true), "another room");
    }
}
