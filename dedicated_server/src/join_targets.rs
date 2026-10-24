//! The room each player was last given to join, for a failed join the game reports against
//! the wrong room. The game often names session 0, or the party it just split off for
//! itself, rather than the room it was trying to reach: PlaySkill's refused join to
//! Theusma01's room came in against PlaySkill's own split party, and a quick match into a
//! full room against session 0. The server knows better, for a moment: the room an
//! invitation's search was answered with, the rooms a matchmaking search offered, and the
//! host the game then asked to probe. `game_session.rs` records a failure against that room
//! (`room`), with the game's own value as `reported_room`.
//!
//! Kept for [`FRESH_FOR`] and for at most [`MAX_PLAYERS`] players.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How long after the offer a failed join is taken to be about it: the game answers a join
/// in seconds (a full room 1 s after the probe, a refused one about 1 s after the invite's
/// search, a quick match's within 6 s of its search).
const FRESH_FOR: Duration = Duration::from_secs(60);
/// The most players kept; past it, the stale ones go.
const MAX_PLAYERS: usize = 10_000;

/// What a player was last given to join.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Offer {
    /// One room: an invitation's, a search's only result, or the one whose host they probed.
    Room(u32),
    /// A matchmaking search's results, room and host, until a probe picks one.
    Rooms(Vec<(u32, u32)>),
    /// A host they probed, with no search to say which of their rooms.
    Host(u32),
}

fn offers() -> std::sync::MutexGuard<'static, HashMap<u32, (Offer, Instant)>> {
    static OFFERS: std::sync::LazyLock<Mutex<HashMap<u32, (Offer, Instant)>>> = std::sync::LazyLock::new(Mutex::default);
    OFFERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn put(user: u32, offer: Offer) {
    let mut all = offers();
    if all.len() >= MAX_PLAYERS && !all.contains_key(&user) {
        all.retain(|_, (_, at)| at.elapsed() < FRESH_FOR);
        if all.len() >= MAX_PLAYERS {
            return;
        }
    }
    all.insert(user, (offer, Instant::now()));
}

/// `user`'s search for an invitation was answered with room `session`.
pub fn invited(user: u32, session: u32) {
    put(user, Offer::Room(session));
}

/// A host migration moved room `from` on to `to`: whoever was given `from` to join is given
/// `to`, the room its host took over.
pub fn moved(from: u32, to: u32) {
    for (offer, _) in offers().values_mut() {
        match offer {
            Offer::Room(room) if *room == from => *room = to,
            Offer::Rooms(rooms) => rooms.iter_mut().filter(|(room, _)| *room == from).for_each(|(room, _)| *room = to),
            _ => {}
        }
    }
}

/// `user`'s matchmaking search offered these rooms (room, host). Nothing found leaves what
/// they had.
pub fn searched(user: u32, rooms: impl IntoIterator<Item = (u32, u32)>) {
    let rooms: Vec<(u32, u32)> = rooms.into_iter().filter(|(_, host)| *host != user).collect();
    match rooms[..] {
        [] => {}
        [(room, _)] => put(user, Offer::Room(room)),
        _ => put(user, Offer::Rooms(rooms)),
    }
}

/// `user`'s game asked to probe `other`: if `other` hosts one of the rooms their search
/// offered, that's the one they're joining. A probe also goes to the players of a room
/// they're already in, so it never replaces a room they were given.
pub fn probed(user: u32, other: u32) {
    let mut all = offers();
    match all.get_mut(&user) {
        Some((offer, at)) if at.elapsed() < FRESH_FOR => {
            if let Offer::Rooms(rooms) = offer {
                if let Some(&(room, _)) = rooms.iter().find(|(_, host)| *host == other) {
                    *offer = Offer::Room(room);
                    *at = Instant::now();
                }
            }
        }
        _ => {
            drop(all);
            put(user, Offer::Host(other));
        }
    }
}

/// What `user` was last given to join, while fresh: a room, or a host whose live room it
/// is (`host_room` finds it).
pub fn target(user: u32, host_room: impl FnOnce(u32) -> Option<u32>) -> Option<u32> {
    let offer = offers().get(&user).filter(|(_, at)| at.elapsed() < FRESH_FOR).map(|(o, _)| o.clone())?;
    match offer {
        Offer::Room(room) => Some(room),
        // Several and no probe: which one isn't known.
        Offer::Rooms(_) => None,
        Offer::Host(host) => host_room(host),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_room_given_last_is_the_target() {
        let user = 7_100_001;
        assert_eq!(target(user, |_| None), None, "nothing given");
        invited(user, 71);
        assert_eq!(target(user, |_| None), Some(71));
        // A probe to a player of the room they were given changes nothing.
        probed(user, 1005);
        assert_eq!(target(user, |_| None), Some(71));
        // A search with one result: that room.
        searched(user, [(87, 1006)]);
        assert_eq!(target(user, |_| None), Some(87));
        // Several: the one whose host they probe.
        searched(user, [(90, 1007), (91, 1008), (92, user)]);
        assert_eq!(target(user, |_| None), None, "which one isn't known yet");
        probed(user, 1009);
        assert_eq!(target(user, |_| None), None, "not one of them");
        probed(user, 1008);
        assert_eq!(target(user, |_| None), Some(91));
        // Nothing found leaves it.
        searched(user, []);
        assert_eq!(target(user, |_| None), Some(91));
        // Their own room is never offered as a target.
        searched(user, [(93, user)]);
        assert_eq!(target(user, |_| None), Some(91));
    }

    #[test]
    fn a_migrated_room_is_given_as_the_new_one() {
        let (a, b) = (7_100_004, 7_100_005);
        invited(a, 7_777_001);
        searched(b, [(7_777_001, 1), (7_777_002, 2)]);
        moved(7_777_001, 7_777_003);
        assert_eq!(target(a, |_| None), Some(7_777_003));
        probed(b, 1);
        assert_eq!(target(b, |_| None), Some(7_777_003));
    }

    #[test]
    fn a_probe_alone_names_the_hosts_room() {
        let user = 7_100_002;
        probed(user, 1010);
        assert_eq!(target(user, |host| (host == 1010).then_some(233)), Some(233));
        assert_eq!(target(user, |_| None), None, "the host has no live room");
    }

    #[test]
    fn an_offer_goes_stale() {
        let user = 7_100_003;
        invited(user, 5);
        offers().get_mut(&user).unwrap().1 = Instant::now() - FRESH_FOR;
        assert_eq!(target(user, |_| None), None);
        // A stale offer is replaced by a probe alone.
        probed(user, 1011);
        assert_eq!(target(user, |_| Some(6)), Some(6));
    }
}
