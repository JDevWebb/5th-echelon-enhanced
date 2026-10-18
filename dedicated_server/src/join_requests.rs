//! Players who asked to join a friend's game from the launcher or the overlay (`JoinNow`).
//!
//! `JoinNow` queues an invitation from the friend to the caller, like any invitation, and
//! marks the caller here with that invitation: its id, sender and room. When that very
//! invitation is delivered (see `api.rs`'s `event`), it's sent with `requested = true`, so the
//! caller's game joins without a click — the one difference from an ordinary invitation, which
//! the player must accept. Any other invitation delivered meanwhile (one that was already
//! waiting, say) goes out as it is. The mark is one-shot (cleared when its invitation is taken)
//! and expires on its own, so a stale request can't attach to some later, unrelated invitation.
//!
//! The mark is set before the invitation is committed (`Storage::add_join_request_async`), so
//! no event poll can take the invitation before it's marked. Invitation ids are SQLite rowids,
//! which come back once the newest row is gone (an invitation replaced, or one whose insert
//! failed), so the sender and the room are compared too: an unrelated invitation that got the
//! same id isn't taken for the requested one.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

/// How long a request stays good. Long enough that a game the launcher just started can finish
/// booting and reach its first event poll, but short enough that it can't attach to some much
/// later, unrelated invitation. The bound invitation it pairs with is the intended one, and the
/// mark is cleared the moment it's taken.
const FOR: Duration = Duration::from_secs(240);

/// The invitation a join request stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requested {
    /// The invitation's id.
    pub invite: i64,
    /// Who it's from: the friend joined, or the friend joined through.
    pub sender: u32,
    /// The room it's bound to.
    pub room: u32,
}

fn requests() -> &'static Mutex<HashMap<u32, (Requested, Instant)>> {
    static REQUESTS: OnceLock<Mutex<HashMap<u32, (Requested, Instant)>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Notes that `user` asked to join: the invitation `requested` is to be delivered as a join
/// they requested. A newer request replaces an older one.
pub fn mark(user: u32, requested: Requested) {
    if let Ok(mut map) = requests().lock() {
        let now = Instant::now();
        map.retain(|_, (_, at)| now.duration_since(*at) < FOR);
        map.insert(user, (requested, now));
    }
}

/// Whether the invitation `delivered` (its id, sender and room) going to `user` is the one they
/// asked for, clearing the request (one-shot). Another invitation leaves the request waiting
/// for its own.
pub fn take(user: u32, delivered: Requested) -> bool {
    let Ok(mut map) = requests().lock() else { return false };
    match map.get(&user) {
        Some((requested, at)) if *requested == delivered => {
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

    fn r(invite: i64, sender: u32, room: u32) -> Requested {
        Requested { invite, sender, room }
    }

    #[test]
    fn a_marked_request_is_taken_once_by_its_invitation() {
        let user = 987_654;
        assert!(!take(user, r(1, 5, 9)), "nothing marked");
        mark(user, r(7, 5, 9));
        assert!(!take(user, r(3, 6, 10)), "another invitation isn't the requested join");
        assert!(take(user, r(7, 5, 9)), "the requested invitation is");
        assert!(!take(user, r(7, 5, 9)), "and only once");
    }

    #[test]
    fn a_newer_request_replaces_the_older() {
        let user = 987_655;
        mark(user, r(1, 5, 9));
        mark(user, r(2, 5, 9));
        assert!(!take(user, r(1, 5, 9)), "the older request is gone");
        assert!(take(user, r(2, 5, 9)));
    }

    #[test]
    fn an_invitation_that_got_a_reused_id_isnt_the_requested_one() {
        let user = 987_656;
        mark(user, r(7, 5, 9));
        assert!(!take(user, r(7, 6, 9)), "the same id from someone else");
        assert!(!take(user, r(7, 5, 10)), "the same id and sender, another room");
        assert!(take(user, r(7, 5, 9)), "the request is still waiting for its own");
    }
}
