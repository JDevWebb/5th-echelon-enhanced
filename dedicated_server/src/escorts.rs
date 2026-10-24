//! A party's leader invited into someone else's private match, and the members that follow it.
//!
//! The game moves a party into a match over Storm, on the leader's word: the leader sends the
//! members a `FollowLeader` message naming the match, and each joins it as the leader did (the
//! leader's game says so only when it *hosts* the match; the client hook `party_follow`, always on,
//! has it say so for a match it was invited to). Nothing of this passes the server, which sees
//! only the members' joins afterwards: the match's host adds each member that comes in
//! (`AddParticipants`, whose private-match notification opens the member's join gate), or the
//! member adds itself. Both are refused for a stranger, and the members are strangers to the
//! host, so the server must know who may come:
//!
//! 1. **Boarding.** The leader's friend search for the inviting host is answered with the
//!    private match; the members of the party it hosts are noted then, while they are still in
//!    it.
//! 2. **Admission.** A companion noted may be added to that match, by anyone allowed to add
//!    players to it or by itself, for [`BOARDING_FOR`].
//!
//! 3. **Return.** The game does not take the party back. When the match is dissolved, the
//!    leader's game leaves its party and hosts a new one, and the members, who saw their party's
//!    host go, migrate the old party alone (live, 2026-10-23: two single-member parties). The
//!    members that came into the match ([`came`]) are therefore asked into the leader's new
//!    party, once ([`back`]), by an invitation the server queues for them.
//!
//! A boarding is kept per leader, bounded in time and number.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// From the leader's search to the members' joins: seconds in the game (the leader's 15 s wait
/// for them), and then some for a slow start.
const BOARDING_FOR: Duration = Duration::from_secs(10 * 60);
/// From the leader's acceptance to its new party after the match: a match's length, and then
/// some. The admission ([`BOARDING_FOR`]) is over long before.
const AWAY_FOR: Duration = Duration::from_secs(4 * 60 * 60);
/// Members noted along, at most (a party has eight seats; the leader takes one).
pub const MAX_COMPANIONS: usize = 8;
/// Leaders with a boarding kept at once, at most.
const MAX_BOARDINGS: usize = 1024;
/// From the leader leaving the match to its new party: the game hosts it right after the match
/// (seconds). A party opened much later is no return from this match - the members may be in
/// the middle of another one - so they are not asked.
const BACK_WITHIN: Duration = Duration::from_secs(5 * 60);

#[derive(Debug)]
struct Boarding {
    /// The private match the leader was invited into.
    room: u32,
    /// The members of the leader's party when it accepted.
    companions: Vec<u32>,
    /// Of those, the ones that came into the match.
    came: Vec<u32>,
    /// When the leader accepted.
    at: Instant,
    /// When the leader last left the match, if the server saw it.
    left_at: Option<Instant>,
}

/// The boardings of leaders to private matches ([`Escorts::board`], [`Escorts::allowed`]); no
/// global state, so it can be tested.
#[derive(Debug, Default)]
pub struct Escorts {
    by_leader: HashMap<u32, Boarding>,
}

impl Escorts {
    /// `leader` accepted an invitation into private match `room` while the players
    /// `companions` were in its party. A search repeated after the party broke up (no
    /// companions any more) keeps what an earlier one noted for the same match.
    pub fn board(&mut self, leader: u32, room: u32, companions: &[u32], now: Instant) {
        let mut companions: Vec<u32> = companions.iter().copied().filter(|c| *c != leader).collect();
        companions.sort_unstable();
        companions.dedup();
        companions.truncate(MAX_COMPANIONS);
        if companions.is_empty() {
            if self.by_leader.get(&leader).is_some_and(|b| b.room == room && now.duration_since(b.at) < BOARDING_FOR) {
                return;
            }
            self.by_leader.remove(&leader);
            return;
        }
        self.by_leader.retain(|_, b| now.duration_since(b.at) < AWAY_FOR);
        if self.by_leader.len() >= MAX_BOARDINGS && !self.by_leader.contains_key(&leader) {
            return;
        }
        self.by_leader.insert(
            leader,
            Boarding {
                room,
                companions,
                came: Vec::new(),
                at: now,
                left_at: None,
            },
        );
    }

    /// The leader `member` follows into `room`: the one whose boarding for it is recent and
    /// names `member`.
    pub fn boarded_by(&self, room: u32, member: u32, now: Instant) -> Option<u32> {
        self.by_leader
            .iter()
            .find(|(_, b)| b.room == room && now.duration_since(b.at) < BOARDING_FOR && b.companions.contains(&member))
            .map(|(leader, _)| *leader)
    }

    /// `member` came into `room`: if it is a companion of a boarding for it, it is to be asked
    /// back with its leader.
    pub fn came(&mut self, room: u32, member: u32, now: Instant) {
        for b in self.by_leader.values_mut() {
            if b.room == room && now.duration_since(b.at) < AWAY_FOR && b.companions.contains(&member) && !b.came.contains(&member) {
                b.came.push(member);
            }
        }
    }

    /// `leader` left `room`: noted when it is the match it boarded (the last time counts).
    pub fn left(&mut self, leader: u32, room: u32, now: Instant) {
        if let Some(b) = self.by_leader.get_mut(&leader).filter(|b| b.room == room) {
            b.left_at = Some(now);
        }
    }

    /// `leader` has a new party. If the match it boarded is over (`over` says so of the room),
    /// the members who came into it, once: they are to be asked into the party. A match still
    /// going leaves the boarding for later.
    pub fn back(&mut self, leader: u32, now: Instant, over: impl FnOnce(u32) -> bool) -> Vec<u32> {
        let Some(b) = self.by_leader.get(&leader) else { return Vec::new() };
        if now.duration_since(b.at) >= AWAY_FOR {
            self.by_leader.remove(&leader);
            return Vec::new();
        }
        if b.left_at.is_some_and(|left| now.duration_since(left) >= BACK_WITHIN) {
            // The party comes long after the match: not its return.
            self.by_leader.remove(&leader);
            return Vec::new();
        }
        if b.came.is_empty() || !over(b.room) {
            return Vec::new();
        }
        self.by_leader.remove(&leader).map(|b| b.came).unwrap_or_default()
    }

    /// Whether `member` may be added to `room` as a companion of a leader invited into it.
    pub fn allowed(&self, room: u32, member: u32, now: Instant) -> bool {
        self.boarded_by(room, member, now).is_some()
    }
}

fn escorts() -> std::sync::MutexGuard<'static, Escorts> {
    static ESCORTS: std::sync::OnceLock<Mutex<Escorts>> = std::sync::OnceLock::new();
    ESCORTS.get_or_init(Mutex::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`Escorts::board`], now.
pub fn board(leader: u32, room: u32, companions: &[u32]) {
    escorts().board(leader, room, companions, Instant::now());
}

/// [`Escorts::boarded_by`], now.
pub fn boarded_by(room: u32, member: u32) -> Option<u32> {
    escorts().boarded_by(room, member, Instant::now())
}

/// [`Escorts::came`], now.
pub fn came(room: u32, member: u32) {
    escorts().came(room, member, Instant::now());
}

/// [`Escorts::left`], now.
pub fn left(leader: u32, room: u32) {
    escorts().left(leader, room, Instant::now());
}

/// [`Escorts::back`], now.
pub fn back(leader: u32, over: impl FnOnce(u32) -> bool) -> Vec<u32> {
    escorts().back(leader, Instant::now(), over)
}

/// The invitations queued to bring a member back into its party (most recent ones), which the
/// game is to take without a click like a join the player asked for: the invitation itself is an
/// ordinary one from the party's host, so that it holds for a party announced invite-only.
static RETURN_INVITES: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// Notes invitation `id` as a return into the party.
pub fn mark_return_invite(id: i64) {
    let mut ids = RETURN_INVITES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if ids.len() >= 256 {
        ids.remove(0);
    }
    ids.push(id);
}

/// Whether invitation `id` was queued by [`mark_return_invite`].
pub fn is_return_invite(id: i64) -> bool {
    RETURN_INVITES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&id)
}

/// [`Escorts::allowed`], now.
pub fn allowed(room: u32, member: u32) -> bool {
    escorts().allowed(room, member, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEADER: u32 = 10;
    const MEMBER: u32 = 11;
    const OTHER: u32 = 12;
    const STRANGER: u32 = 99;
    const ROOM: u32 = 500;

    #[test]
    fn the_companions_of_a_boarding_are_allowed_into_its_match() {
        let mut e = Escorts::default();
        let now = Instant::now();
        assert!(!e.allowed(ROOM, MEMBER, now), "nobody boarded");
        e.board(LEADER, ROOM, &[MEMBER, OTHER, LEADER, MEMBER], now);
        assert!(e.allowed(ROOM, MEMBER, now) && e.allowed(ROOM, OTHER, now), "the members, as often as they ask");
        assert!(!e.allowed(ROOM, LEADER, now), "the leader is no companion of itself");
        assert!(!e.allowed(ROOM, STRANGER, now), "nor is a stranger");
        assert!(!e.allowed(ROOM + 1, MEMBER, now), "another match isn't the one boarded");
        assert_eq!(e.boarded_by(ROOM, MEMBER, now), Some(LEADER));
        assert_eq!(e.boarded_by(ROOM, STRANGER, now), None);
    }

    #[test]
    fn a_boarding_goes_stale() {
        let mut e = Escorts::default();
        let now = Instant::now();
        e.board(LEADER, ROOM, &[MEMBER], now);
        assert!(e.allowed(ROOM, MEMBER, now + BOARDING_FOR - Duration::from_secs(1)));
        assert!(!e.allowed(ROOM, MEMBER, now + BOARDING_FOR));
    }

    #[test]
    fn a_search_after_the_party_broke_up_keeps_what_was_noted() {
        let mut e = Escorts::default();
        let now = Instant::now();
        e.board(LEADER, ROOM, &[MEMBER], now);
        e.board(LEADER, ROOM, &[], now);
        assert!(e.allowed(ROOM, MEMBER, now));
        // Another match without a party forgets the earlier boarding.
        e.board(LEADER, ROOM + 1, &[], now);
        assert!(!e.allowed(ROOM, MEMBER, now));
    }

    #[test]
    fn the_members_that_came_are_taken_back_once_the_match_is_over() {
        let mut e = Escorts::default();
        let now = Instant::now();
        e.board(LEADER, ROOM, &[MEMBER, OTHER], now);
        e.came(ROOM, MEMBER, now);
        e.came(ROOM, MEMBER, now);
        e.came(ROOM, STRANGER, now);
        e.came(ROOM + 1, OTHER, now);
        assert!(e.back(STRANGER, now, |_| true).is_empty(), "no boarding, nobody");
        assert!(e.back(LEADER, now, |_| false).is_empty(), "the match is still going");
        assert_eq!(e.back(LEADER, now, |room| room == ROOM), vec![MEMBER], "only who came, and as asked");
        assert!(e.back(LEADER, now, |_| true).is_empty(), "once");
        // Nobody came: nothing to bring back, and a later arrival after the window isn't noted.
        e.board(LEADER, ROOM, &[MEMBER], now);
        assert!(e.back(LEADER, now, |_| true).is_empty());
        e.came(ROOM, MEMBER, now + AWAY_FOR);
        assert!(e.back(LEADER, now + AWAY_FOR, |_| true).is_empty(), "too late");
    }

    #[test]
    fn a_party_long_after_the_leader_left_the_match_is_no_return() {
        let mut e = Escorts::default();
        let now = Instant::now();
        e.board(LEADER, ROOM, &[MEMBER], now);
        e.came(ROOM, MEMBER, now);
        e.left(LEADER, ROOM + 1, now);
        e.left(LEADER, ROOM, now);
        // Match still going, the leader's party only hours later: nobody is pulled out of it.
        assert!(e.back(LEADER, now + BACK_WITHIN, |_| true).is_empty());
        assert!(e.back(LEADER, now + BACK_WITHIN, |_| true).is_empty(), "and the boarding is gone");
        // Promptly after leaving: as before.
        e.board(LEADER, ROOM, &[MEMBER], now);
        e.came(ROOM, MEMBER, now);
        e.left(LEADER, ROOM, now);
        assert_eq!(e.back(LEADER, now + Duration::from_secs(20), |_| true), vec![MEMBER]);
        // The leader's leave that was never seen: as before.
        e.board(LEADER, ROOM, &[MEMBER], now);
        e.came(ROOM, MEMBER, now);
        assert_eq!(e.back(LEADER, now + Duration::from_secs(3600), |_| true), vec![MEMBER]);
    }

    #[test]
    fn what_is_kept_is_bounded() {
        let mut e = Escorts::default();
        let now = Instant::now();
        let crowd: Vec<u32> = (100..200).collect();
        e.board(1, ROOM, &crowd, now);
        assert!(e.allowed(ROOM, 100, now) && !e.allowed(ROOM, 100 + MAX_COMPANIONS as u32, now), "a party has eight seats");
        for leader in 1000..1000 + MAX_BOARDINGS as u32 + 50 {
            e.board(leader, ROOM, &[MEMBER], now);
        }
        assert!(e.by_leader.len() <= MAX_BOARDINGS);
        // Old ones go when another is noted.
        e.board(7, ROOM, &[MEMBER], now + AWAY_FOR);
        assert!(e.by_leader.len() <= 2);
    }
}
