//! Names shown to others that aren't the player's (0.5.6's N1). A cheat table rewrites the name a
//! player's own game sends the others (in the lobby, the scoreboard and the chat); the server
//! knows each player's real name, by their number in the match's session ([`crate::peer_names`]).
//! A game that showed a chat line from number N under another name says so (`NameSeen`): a host's
//! (a line reaches the host from its sender, the others get it passed on from number 0). And since
//! 0.5.6 any game whose session's player list (Storm's player descriptions, the name every other
//! game shows in the lobby and the scoreboard) has another name for N, on two reads in a row. One witness makes it a `fair_play` note on that
//! player (check `name`, report only, for the admins); two (the same name in two players'
//! games) and every player in the match is told who it really is, once a match.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::PoisonError;

/// Witnesses needed: one game's word could be its own mistake (or a lie).
pub const WITNESSES: usize = 2;
const MOST_KEPT: usize = 20_000;

/// (room, player, the name shown, lowercase) → the witnesses, and whether it was told.
type Seen = HashMap<(u32, u32, String), (HashSet<u32>, bool)>;
static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

/// What a witness's word comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen2 {
    Nothing,
    /// The first witness: a note for the admins.
    Note,
    /// Enough witnesses: tell the match (once).
    Tell,
}

/// `witness` saw `target` (real name `real`) shown as `shown` in `room`.
pub fn seen(witness: u32, room: u32, target: u32, real: &str, shown: &str) -> Seen2 {
    if witness == target || shown.trim().is_empty() || shown.trim().eq_ignore_ascii_case(real.trim()) {
        return Seen2::Nothing;
    }
    let mut all = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    let all = all.get_or_insert_with(HashMap::new);
    if all.len() >= MOST_KEPT {
        all.clear();
    }
    let entry = all.entry((room, target, shown.trim().to_lowercase())).or_default();
    if !entry.0.insert(witness) {
        return Seen2::Nothing;
    }
    if entry.0.len() >= WITNESSES && !entry.1 {
        entry.1 = true;
        return Seen2::Tell;
    }
    if entry.0.len() == 1 {
        Seen2::Note
    } else {
        Seen2::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_witnesses_tell_the_match_once() {
        assert_eq!(seen(1, 500, 9, "Tea_bag", "tea_bag"), Seen2::Nothing, "their own name, any case");
        assert_eq!(seen(9, 500, 9, "Tea_bag", "Admin"), Seen2::Nothing, "not their own witness");
        assert_eq!(seen(1, 500, 9, "Tea_bag", "Admin"), Seen2::Note, "one witness: a note");
        assert_eq!(seen(1, 500, 9, "Tea_bag", "Admin"), Seen2::Nothing, "the same one again");
        assert_eq!(seen(2, 500, 9, "Tea_bag", "ADMIN"), Seen2::Tell, "a second: told");
        assert_eq!(seen(3, 500, 9, "Tea_bag", "Admin"), Seen2::Nothing, "told once");
        assert_eq!(seen(1, 501, 9, "Tea_bag", "Admin"), Seen2::Note, "another match starts again");
    }
}
