//! Far rooms in quick match (the 0.4.8 plan's G3): with one matchmaking server for the world, a
//! search finds hosts on every continent. The game QoS-pings the results and prefers the near
//! ones (E3), but it joins a far one when nothing nearer answers, and only the first
//! [`crate::game_session::MAX_SEARCH_RESULTS`] reach it at all. So with `[matchmaking] far_ms`
//! set, a searcher's results come nearest first, and rooms whose host is estimated farther than
//! that (a round trip) are left out while a nearer one matches; with `far_strict`, always, so
//! a far player hosts their own match instead.
//!
//! The estimate is the distance between the two players' places (the geolocation database's,
//! looked up here, never sent), as [`crate::nat_helper`]'s joins do: light in fibre and a fixed
//! cost for the last miles. A host or searcher it can't place is never left out.

use std::net::IpAddr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering;

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
