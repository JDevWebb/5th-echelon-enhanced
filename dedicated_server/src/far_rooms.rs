//! Far rooms in quick match (the 0.4.8 plan's G3): with one matchmaking server for the world, a
//! search finds hosts on every continent. The game QoS-pings the results and prefers the near
//! ones (E3), but it joins a far one when nothing nearer answers, and only the first
//! [`crate::game_session::MAX_SEARCH_RESULTS`] reach it at all. So with `[matchmaking] far_ms`
//! set, a searcher's results come nearest first, and rooms whose host is estimated farther than
//! that (a round trip) are left out while a nearer one matches; with `far_strict`, always, so
//! a far player hosts their own match instead.
//!
//! The estimate, best first:
//! - **Through the relays** (0.4.8 games): each game's sign-in carries the launcher's round trip
//!   to every relay of the network ([`note_pings`]); two players who both reached a relay are at
//!   most their two round trips to it apart, so the least over the relays both reached is how
//!   far apart they'd play (relayed, as most do; a direct path is no longer). Measured, so it
//!   follows the real routes, which run about 30% past the distance's share.
//! - **From where they are:** the distance between the two players' places (the geolocation
//!   database's, looked up here, never sent), as [`crate::nat_helper`]'s joins do: light in fibre
//!   and a fixed cost for the last miles.
//!
//! A host or searcher neither can place is never left out.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

/// Rooms farther than this (estimated round trip, ms) are far; 0: off.
static FAR_MS: AtomicU16 = AtomicU16::new(0);
/// Far rooms are left out even when nothing nearer matches.
static STRICT: AtomicBool = AtomicBool::new(false);

/// About 200 km a millisecond in fibre, there and back: as `nat_helper`'s `direct_ms`.
const KM_PER_RTT_MS: f64 = 100.0;
/// The players' own lines and the routes' detours, added to the distance's share.
const BASE_MS: u32 = 20;
/// Hosts within this of each other count as equally near, so newer rooms keep their place.
const BAND_MS: u32 = 30;

/// Each account's game's round trips to the relays (relay id, ms), from its last sign-in.
static PINGS: Mutex<Option<HashMap<u32, Vec<(String, u32)>>>> = Mutex::new(None);
/// The most relays kept for one account, and the longest round trip believed.
const MOST_RELAYS: usize = 32;
const LONGEST_MS: u32 = 2_000;

/// Notes the round trips `user_id`'s game said it has to the relays; none forgets the last.
pub fn note_pings<'a>(user_id: u32, pings: impl Iterator<Item = (&'a str, u32)>) {
    let list: Vec<(String, u32)> = pings
        .filter(|(relay, ms)| !relay.is_empty() && relay.len() <= 64 && *ms > 0 && *ms <= LONGEST_MS)
        .take(MOST_RELAYS)
        .map(|(relay, ms)| (relay.to_string(), ms))
        .collect();
    let mut kept = PINGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = kept.get_or_insert_with(HashMap::new);
    if list.is_empty() {
        map.remove(&user_id);
    } else {
        map.insert(user_id, list);
    }
}

/// The round trip between two players' games through the relay they'd both reach soonest: the
/// least sum of their round trips to one relay. None unless both games said, with a relay in
/// common.
pub fn estimate_by_relays(a: u32, b: u32) -> Option<u32> {
    let kept = PINGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = kept.as_ref()?;
    let (pa, pb) = (map.get(&a)?, map.get(&b)?);
    pa.iter().filter_map(|(relay, ms)| pb.iter().find(|(r, _)| r == relay).map(|(_, other)| ms + other)).min()
}

/// From `[matchmaking]`, once at start.
pub fn init(far_ms: u16, strict: bool) {
    FAR_MS.store(far_ms, Ordering::Relaxed);
    STRICT.store(strict, Ordering::Relaxed);
}

/// The estimated round trip between two players' games, from where they are.
pub fn estimate_ms(a: IpAddr, b: IpAddr) -> Option<u32> {
    if a == b {
        return Some(BASE_MS);
    }
    let (a, b) = (crate::metrics::place(a)?, crate::metrics::place(b)?);
    (a.located() && b.located()).then(|| from_km(a.km_to(&b)))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn from_km(km: f64) -> u32 {
    (km / KM_PER_RTT_MS).round() as u32 + BASE_MS
}

/// `rooms` (with each one's estimated round trip to the searcher, None if unknown) nearest first,
/// in bands of [`BAND_MS`] and otherwise as they came; the far ones left out as configured. Also
/// how many were left out. Off, `rooms` as they came.
pub fn order<T>(rooms: Vec<(T, Option<u32>)>) -> (Vec<T>, usize) {
    order_with(rooms, u32::from(FAR_MS.load(Ordering::Relaxed)), STRICT.load(Ordering::Relaxed))
}

/// Whether far rooms are looked at at all (the caller can skip estimating them).
pub fn enabled() -> bool {
    FAR_MS.load(Ordering::Relaxed) > 0
}

fn order_with<T>(rooms: Vec<(T, Option<u32>)>, far_ms: u32, strict: bool) -> (Vec<T>, usize) {
    if far_ms == 0 {
        return (rooms.into_iter().map(|(r, _)| r).collect(), 0);
    }
    let far = |ms: &Option<u32>| ms.is_some_and(|ms| ms > far_ms);
    let mut rooms = rooms;
    // Near ones by band, then those it can't place, then the far ones nearest first.
    rooms.sort_by_key(|(_, ms)| match ms {
        Some(ms) if *ms <= far_ms => (0, ms / BAND_MS),
        None => (1, 0),
        Some(ms) => (2, *ms),
    });
    let near = rooms.iter().filter(|(_, ms)| !far(ms)).count();
    let left_out = if strict || near > 0 { rooms.len() - near } else { 0 };
    rooms.truncate(rooms.len() - left_out);
    (rooms.into_iter().map(|(r, _)| r).collect(), left_out)
}

#[cfg(test)]
mod tests {
    /// Two players' estimate through the relays is the least sum over a relay both reached;
    /// none in common, or a game that didn't say, is no estimate; nonsense is left out.
    #[test]
    fn players_are_as_far_apart_as_their_nearest_shared_relay() {
        use super::estimate_by_relays;
        use super::note_pings;
        note_pings(800_001, [("syd", 12), ("lax", 160), ("fra", 290)].into_iter());
        note_pings(800_002, [("syd", 140), ("lax", 15)].into_iter());
        note_pings(800_003, [("fra", 20), ("bad", 0), ("", 5), ("far", 99_999)].into_iter());
        assert_eq!(estimate_by_relays(800_001, 800_002), Some(152), "syd 12+140, lax 160+15: 152");
        assert_eq!(estimate_by_relays(800_002, 800_001), Some(152));
        assert_eq!(estimate_by_relays(800_001, 800_003), Some(310));
        assert_eq!(estimate_by_relays(800_002, 800_003), None, "no relay in common");
        assert_eq!(estimate_by_relays(800_001, 800_009), None, "a game that didn't say");
        note_pings(800_001, std::iter::empty());
        assert_eq!(estimate_by_relays(800_001, 800_002), None, "a sign-in without pings forgets them");
    }

    use super::*;

    #[test]
    fn off_keeps_everything_in_order() {
        let (rooms, left_out) = order_with(vec![(1, Some(300)), (2, Some(40)), (3, None)], 0, true);
        assert_eq!((rooms, left_out), (vec![1, 2, 3], 0));
    }

    #[test]
    fn nearest_first_by_band_and_far_left_out_while_a_near_one_matches() {
        // 2 and 4 share a band (30-59 ms): they keep their order (newest first, as given).
        let rooms = vec![(1, Some(280)), (2, Some(55)), (3, None), (4, Some(31)), (5, Some(12))];
        assert_eq!(order_with(rooms, 150, false), (vec![5, 2, 4, 3], 1));
    }

    #[test]
    fn far_ones_stay_when_nothing_nearer_matches_unless_strict() {
        let rooms = || vec![(1, Some(280)), (2, Some(190))];
        assert_eq!(order_with(rooms(), 150, false), (vec![2, 1], 0));
        assert_eq!(order_with(rooms(), 150, true), (vec![], 2));
    }

    #[test]
    fn a_room_it_cant_place_counts_as_near() {
        assert_eq!(order_with(vec![(1, Some(280)), (2, None)], 150, false), (vec![2], 1));
    }

    #[test]
    fn distance_to_round_trip() {
        // Sydney to London, ~17,000 km: about 190 ms (players see ~280 through real routes).
        assert_eq!(from_km(17_000.0), 190);
        assert_eq!(from_km(0.0), BASE_MS);
    }
}
