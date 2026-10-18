//! Players who asked to join a friend's game from the launcher or the overlay (`JoinNow`).
//!
//! `JoinNow` queues an invitation from the friend to the caller, like any invitation, and
//! marks the caller here with that invitation's id. When that very invitation is delivered
//! (see `api.rs`'s `event`), it's sent with `requested = true`, so the caller's game joins
//! without a click — the one difference from an ordinary invitation, which the player must
//! accept. Any other invitation delivered meanwhile (one that was already waiting, say) goes
//! out as it is. The mark is one-shot (cleared when its invitation is taken) and expires on its
//! own, so a stale request can't attach to some later, unrelated invitation.

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

fn requests() -> &'static Mutex<HashMap<u32, (i64, Instant)>> {
    static REQUESTS: OnceLock<Mutex<HashMap<u32, (i64, Instant)>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Notes that `user` asked to join: the invitation `invite` (its id) is to be delivered as a
/// join they requested. A newer request replaces an older one.
pub fn mark(user: u32, invite: i64) {
    if let Ok(mut map) = requests().lock() {
        let now = Instant::now();
        map.retain(|_, (_, at)| now.duration_since(*at) < FOR);
        map.insert(user, (invite, now));
    }
}

/// Whether the invitation `invite` being delivered to `user` is the one they asked for,
/// clearing the request (one-shot). Another invitation leaves the request waiting for its own.
pub fn take(user: u32, invite: i64) -> bool {
    let Ok(mut map) = requests().lock() else { return false };
    match map.get(&user) {
        Some((id, at)) if *id == invite => {
            let fresh = Instant::now().duration_since(*at) < FOR;
            map.remove(&user);
            fresh
        }
        Some((_, at)) if Instant::now().duration_since(*at) >= FOR => {
            map.remove(&user);
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marked_request_is_taken_once_by_its_invitation() {
        let user = 987_654;
        assert!(!take(user, 1), "nothing marked");
        mark(user, 7);
        assert!(!take(user, 3), "another invitation isn't the requested join");
        assert!(take(user, 7), "the requested invitation is");
        assert!(!take(user, 7), "and only once");
    }

    #[test]
    fn a_newer_request_replaces_the_older() {
        let user = 987_655;
        mark(user, 1);
        mark(user, 2);
        assert!(!take(user, 1), "the older request is gone");
        assert!(take(user, 2));
    }
}
