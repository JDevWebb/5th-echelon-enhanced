//! Players who asked to join a friend's game from the launcher or the overlay (`JoinNow`).
//!
//! `JoinNow` binds a force-join invitation from the friend to the caller, like any invitation,
//! and marks the caller here. The next time that caller polls for events, the invitation is sent
//! with `force_join = true` (see `api.rs`'s `event`), so their running game joins without a
//! click — the one difference from an ordinary invitation, which the player must accept. The mark
//! is one-shot (cleared when taken) and expires on its own, so a stale request can't force a join
//! into some later, unrelated invitation.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

/// How long a request stays good. Long enough that a game the launcher just started can finish
/// booting and reach its first event poll (Way 1), but short enough that it can't attach to some
/// much later, unrelated invitation. The bound invitation it pairs with is the intended one, and
/// the mark is cleared the moment it's taken.
const FOR: Duration = Duration::from_secs(240);

fn requests() -> &'static Mutex<HashMap<u32, Instant>> {
    static REQUESTS: OnceLock<Mutex<HashMap<u32, Instant>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Notes that `user` asked to force-join (the next invitation delivered to them is a join).
pub fn mark(user: u32) {
    if let Ok(mut map) = requests().lock() {
        let now = Instant::now();
        map.retain(|_, at| now.duration_since(*at) < FOR);
        map.insert(user, now);
    }
}

/// Whether `user` has a fresh force-join request, clearing it (one-shot).
pub fn take(user: u32) -> bool {
    let Ok(mut map) = requests().lock() else { return false };
    match map.remove(&user) {
        Some(at) => Instant::now().duration_since(at) < FOR,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marked_request_is_taken_once() {
        let user = 987_654;
        assert!(!take(user), "nothing marked");
        mark(user);
        assert!(take(user), "the mark is taken");
        assert!(!take(user), "and only once");
    }
}
