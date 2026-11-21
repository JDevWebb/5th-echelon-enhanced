//! A join into a public match the player asked for (`JoinNow` into a friend's, `JoinRoom` from
//! the lobby browser), kept for their game's friend search instead of an invitation.
//!
//! The game joins a friend through the invitation route whatever started it: the add-on raises
//! the "invitation accepted" event for the friend, and the game searches for them
//! (`SearchSessionsWithParticipants`), joining the match its answer names (`game_session.rs`).
//! A public match needs no invitation to get in (anyone may join one), so a caller whose game
//! raises that event itself (`JoinNowRequest.direct`) gets none: the server keeps the room here
//! and answers that search with it as it would an invitation's. The caller's pending
//! invitations, and the game's, stay as they were.
//!
//! Kept until the player joins the room, asks for something else, or for [`FOR`]; at most
//! [`MAX_PLAYERS`] players.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How long a join holds: time for a game the launcher started to boot, sign in and search.
pub const FOR: Duration = Duration::from_secs(240);
/// The most players kept; past it, the lapsed joins go, and then new ones are refused.
const MAX_PLAYERS: usize = 10_000;

/// The room a player asked to join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub session_type: u32,
    pub room: u32,
    /// Its host.
    pub host: u32,
    /// The member they join through, when it isn't the host: the one who'd have sent the
    /// invitation, so the join holds while they're in the match (`invite_holds_sql`).
    pub via: Option<u32>,
    /// Whom they asked to join (the host or a member): the game searches for them.
    pub friend: u32,
}

impl Target {
    /// Whether the game's search for `participants` is the search for this join: for the friend
    /// the game "accepted the invitation" of, or the host.
    pub fn searched_by(&self, participants: &[u32]) -> bool {
        participants.contains(&self.friend) || participants.contains(&self.host)
    }

    /// Who the invitation it stands in for would be from: the member joined through, else the
    /// host.
    pub fn sender(&self) -> u32 {
        self.via.unwrap_or(self.host)
    }
}

fn joins() -> std::sync::MutexGuard<'static, HashMap<u32, (Target, Instant)>> {
    static JOINS: std::sync::LazyLock<Mutex<HashMap<u32, (Target, Instant)>>> = std::sync::LazyLock::new(Mutex::default);
    JOINS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `user` asked to join `target` (a later ask replaces it). False when too many are held.
pub fn ask(user: u32, target: Target) -> bool {
    let mut all = joins();
    if all.len() >= MAX_PLAYERS && !all.contains_key(&user) {
        all.retain(|_, (_, at)| at.elapsed() < FOR);
        if all.len() >= MAX_PLAYERS {
            return false;
        }
    }
    all.insert(user, (target, Instant::now()));
    true
}

/// `user`'s join, while it holds.
pub fn target(user: u32) -> Option<Target> {
    let mut all = joins();
    match all.get(&user) {
        Some((target, at)) if at.elapsed() < FOR => Some(*target),
        Some(_) => {
            all.remove(&user);
            None
        }
        None => None,
    }
}

/// `user` got into `room`: their join is done if it was that room. Whether it was.
pub fn joined(user: u32, room: u32) -> bool {
    let mut all = joins();
    match all.get(&user) {
        Some((target, at)) if target.room == room => {
            let fresh = at.elapsed() < FOR;
            all.remove(&user);
            fresh
        }
        _ => false,
    }
}

/// A host migration moved room `from` on to `to`: a join into `from` is one into `to`, its new
/// host's (as `Storage::rebind_invites` moves invitations along); the game still searches for
/// the friend it was given.
pub fn moved(from: u32, to: u32, new_host: u32) {
    for (target, _) in joins().values_mut() {
        if target.room == from {
            target.room = to;
            if target.via == Some(new_host) {
                target.via = None;
            }
            target.host = new_host;
        }
    }
}

/// `user` asked for something else (a new match): their join goes.
pub fn forget(user: u32) {
    joins().remove(&user);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(room: u32, host: u32, via: Option<u32>) -> Target {
        Target {
            session_type: 1,
            room,
            host,
            via,
            friend: via.unwrap_or(host),
        }
    }

    #[test]
    fn a_join_is_found_by_the_search_for_its_friend() {
        let user = 7_200_001;
        assert_eq!(super::target(user), None, "nothing asked");
        assert!(ask(user, target(71, 1001, None)));
        assert_eq!(super::target(user), Some(target(71, 1001, None)));
        assert!(super::target(user).unwrap().searched_by(&[1001]));
        assert!(!super::target(user).unwrap().searched_by(&[1002]), "a search for someone else");
        // Through a member: the search for the member, or for the host.
        assert!(ask(user, target(72, 1001, Some(1003))));
        let through = super::target(user).unwrap();
        assert!(through.searched_by(&[1003]) && through.searched_by(&[1001]));
        assert_eq!(through.room, 72, "the later ask replaces the earlier one");
    }

    #[test]
    fn a_join_ends_in_its_room_only() {
        let user = 7_200_002;
        ask(user, target(80, 1001, None));
        assert!(!joined(user, 81), "another room (the game's own split party)");
        assert!(super::target(user).is_some(), "still waiting");
        assert!(joined(user, 80));
        assert_eq!(super::target(user), None, "once");
        assert!(!joined(user, 80));
        ask(user, target(80, 1001, None));
        forget(user);
        assert_eq!(super::target(user), None, "asked for a new match");
    }

    #[test]
    fn a_join_lapses() {
        let user = 7_200_003;
        ask(user, target(90, 1001, None));
        joins().get_mut(&user).unwrap().1 = Instant::now() - FOR;
        assert_eq!(super::target(user), None);
        ask(user, target(90, 1001, None));
        joins().get_mut(&user).unwrap().1 = Instant::now() - FOR;
        assert!(!joined(user, 90), "a lapsed join isn't the one asked for");
    }

    #[test]
    fn a_migrated_room_is_joined_as_the_new_one() {
        let (a, b) = (7_200_004, 7_200_005);
        ask(a, target(7_777_101, 1001, None));
        ask(b, target(7_777_101, 1001, Some(1002)));
        moved(7_777_101, 7_777_102, 1002);
        let (a, b) = (super::target(a).unwrap(), super::target(b).unwrap());
        assert_eq!((a.room, a.host, a.sender()), (7_777_102, 1002, 1002));
        assert!(a.searched_by(&[1001]), "the game still searches for the host it was given");
        assert_eq!((b.room, b.host, b.sender()), (7_777_102, 1002, 1002), "the friend took it over");
        assert!(b.searched_by(&[1002]));
    }
}
