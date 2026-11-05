//! "New match": a player who asked (the launcher's or the overlay's, through the API) has their
//! quick match searches find nothing for a while, so the game hosts a fresh public match
//! instead of joining an open one, which other players' searches then find as usual. For
//! players avoiding someone who sits in a public lobby all day (2026-10-31).
//!
//! The game hosts after two empty searches (E3, the 0.5.0 plan), so the ask holds until the
//! player hosts a match, or for [`FOR`] at most: an ask they never followed up never keeps
//! them from finding matches later. Kept for at most [`MAX_PLAYERS`] players.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How long an ask holds: time for the launcher to start the game (about a minute to its
/// lobby), the player to start matchmaking, and the game's two searches.
pub const FOR: Duration = Duration::from_secs(300);
/// The most players kept; past it, the lapsed asks go, and then new ones are refused.
const MAX_PLAYERS: usize = 10_000;

fn asks() -> std::sync::MutexGuard<'static, HashMap<u32, Instant>> {
    static ASKS: std::sync::LazyLock<Mutex<HashMap<u32, Instant>>> = std::sync::LazyLock::new(Mutex::default);
    ASKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `user` wants their next searches to start a new match. False when too many asks are held.
pub fn new_match(user: u32) -> bool {
    let mut all = asks();
    if all.len() >= MAX_PLAYERS && !all.contains_key(&user) {
        all.retain(|_, at| at.elapsed() < FOR);
        if all.len() >= MAX_PLAYERS {
            return false;
        }
    }
    all.insert(user, Instant::now());
    true
}

/// Whether `user`'s search now should find nothing.
pub fn wants_new(user: u32) -> bool {
    let mut all = asks();
    match all.get(&user) {
        Some(at) if at.elapsed() < FOR => true,
        Some(_) => {
            all.remove(&user);
            false
        }
        None => false,
    }
}

/// `user` hosted a match (or changed their mind): their searches find matches again.
pub fn done(user: u32) {
    asks().remove(&user);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ask_holds_until_the_player_hosts() {
        let user = 7_200_001;
        assert!(!wants_new(user));
        assert!(new_match(user));
        assert!(wants_new(user));
        assert!(wants_new(user), "for both of the game's searches");
        done(user);
        assert!(!wants_new(user));
    }

    #[test]
    fn a_lapsed_ask_is_forgotten() {
        let user = 7_200_002;
        asks().insert(user, Instant::now() - FOR - Duration::from_secs(1));
        assert!(!wants_new(user));
        assert!(!asks().contains_key(&user));
    }
}
