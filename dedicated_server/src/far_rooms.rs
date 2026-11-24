//! Far rooms in quick match (the 0.5.0 plan's G3): with one matchmaking server for the world, a
//! search finds hosts on every continent. The game QoS-pings the results and prefers the near
//! ones (E3), but it joins a far one when nothing nearer answers, and only the first
//! [`crate::game_session::MAX_SEARCH_RESULTS`] reach it at all. So a searcher's results come
//! nearest first ([`order`]), the fuller room first when two are as near, and the hosts it can't
//! place last. That is all a search does by default: far hosts come later, none is turned away.
//! With `[matchmaking] far_ms` set (a round trip; 0, the default: off), rooms whose host is
//! estimated farther than that are also left out while a nearer one matches; with `far_strict`,
//! always, so a far player hosts their own match instead.
//!
//! The estimate, best first:
//! - **Through the relays** (0.5.0 games): each game's sign-in carries the launcher's round trip
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
    if let Some(detail) = pings_event(&list).filter(|_| first_today(user_id, identity::now())) {
        crate::session_events::note(crate::session_events::Who::Id(user_id), "relay_pings", detail);
    }
    let mut kept = PINGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = kept.get_or_insert_with(HashMap::new);
    if list.is_empty() {
        map.remove(&user_id);
    } else {
        map.insert(user_id, list);
    }
}

/// The day each account's relay pings were last noted as a session event.
static NOTED_ON: Mutex<Option<HashMap<u32, i64>>> = Mutex::new(None);

/// Whether `user_id`'s pings haven't been noted yet on the day of `now` (Unix seconds, UTC),
/// and from now on they have.
fn first_today(user_id: u32, now: i64) -> bool {
    let day = now.div_euclid(86_400);
    let mut noted = NOTED_ON.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = noted.get_or_insert_with(HashMap::new);
    if map.len() > 200_000 {
        map.retain(|_, d| *d == day);
    }
    map.insert(user_id, day) != Some(day)
}

/// A game's round trips to the relays as a `relay_pings` session event (0.5.6's RL), once a day
/// an account: every relay ("id:ms", nearest first, 16 at most), the nearest and the next. Join
/// failures and drops can then be read against how far a player is from each relay.
fn pings_event(list: &[(String, u32)]) -> Option<serde_json::Value> {
    let mut sorted: Vec<&(String, u32)> = list.iter().collect();
    sorted.sort_by_key(|(_, ms)| *ms);
    let (best, best_ms) = sorted.first()?;
    Some(serde_json::json!({
        "pings": sorted.iter().take(16).map(|(r, ms)| format!("{r}:{ms}")).collect::<Vec<_>>(),
        "best": best,
        "best_ms": best_ms,
        "second_ms": sorted.get(1).map(|(_, ms)| *ms),
    }))
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

/// A room a search found, as [`order`] weighs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    /// The estimated round trip between the searcher and its host (ms), None if unknown.
    pub ms: Option<u32>,
    /// Who's in it (participants and guests).
    pub members: u32,
}

/// `rooms` nearest first, the fuller first when as near, and those it can't place last (never
/// left out); the far ones (past `[matchmaking] far_ms`) left out as configured. Also how many
/// were left out. Rooms that tie keep the order they came in (newest first).
pub fn order<T>(rooms: Vec<(T, Found)>) -> (Vec<T>, usize) {
    order_with(rooms, u32::from(FAR_MS.load(Ordering::Relaxed)), STRICT.load(Ordering::Relaxed))
}

fn order_with<T>(rooms: Vec<(T, Found)>, far_ms: u32, strict: bool) -> (Vec<T>, usize) {
    let far = |f: &Found| far_ms > 0 && f.ms.is_some_and(|ms| ms > far_ms);
    let mut rooms = rooms;
    rooms.sort_by_key(|(_, f)| (f.ms.is_none(), f.ms.unwrap_or(0), std::cmp::Reverse(f.members)));
    let near = rooms.iter().filter(|(_, f)| !far(f)).count();
    let left_out = if strict || near > 0 { rooms.len() - near } else { 0 };
    if left_out > 0 {
        rooms.retain(|(_, f)| !far(f));
    }
    (rooms.into_iter().map(|(r, _)| r).collect(), left_out)
}

#[cfg(test)]
mod tests {
    /// RL: a game's relay pings are an event once a day an account, nearest first.
    #[test]
    fn relay_pings_are_noted_once_a_day() {
        use super::first_today;
        use super::pings_event;
        let day = 20_000 * 86_400;
        assert!(first_today(4242, day + 10));
        assert!(!first_today(4242, day + 80_000), "the same day");
        assert!(first_today(4243, day + 10), "another account");
        assert!(first_today(4242, day + 86_400), "the next day");
        let e = pings_event(&[("de1".into(), 180), ("au1".into(), 21), ("sg1".into(), 95)]).unwrap();
        assert_eq!(e["pings"], serde_json::json!(["au1:21", "sg1:95", "de1:180"]));
        assert_eq!((e["best"].as_str(), e["best_ms"].as_u64(), e["second_ms"].as_u64()), (Some("au1"), Some(21), Some(95)));
        assert!(pings_event(&[]).is_none(), "none: no event");
    }

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

    fn room(ms: Option<u32>, members: u32) -> Found {
        Found { ms, members }
    }

    #[test]
    fn nearest_first_the_fuller_first_when_as_near_and_ties_keep_their_order() {
        let rooms = vec![
            (1, room(Some(50), 2)),
            (2, room(None, 7)),
            (3, room(Some(50), 5)),
            (4, room(Some(50), 2)),
            (5, room(Some(51), 8)),
            (6, room(Some(12), 1)),
        ];
        assert_eq!(order_with(rooms, 120, true), (vec![6, 3, 1, 4, 5, 2], 0), "no bands: 51 ms is farther than 50");
    }

    #[test]
    fn past_the_cutoff_rooms_are_left_out_but_never_one_it_cant_place() {
        let rooms = vec![(1, room(Some(121), 7)), (2, room(Some(120), 1)), (3, room(None, 1)), (4, room(Some(400), 3))];
        assert_eq!(order_with(rooms, 120, true), (vec![2, 3], 2));
        // Strict: left out even when nothing nearer matches; not strict, kept then.
        let far = || vec![(1, room(Some(280), 1)), (2, room(Some(190), 1))];
        assert_eq!(order_with(far(), 120, true), (vec![], 2));
        assert_eq!(order_with(far(), 120, false), (vec![2, 1], 0));
        assert_eq!(
            order_with(vec![(1, room(Some(280), 1)), (2, room(None, 1))], 120, false),
            (vec![2], 1),
            "one it can't place counts as near"
        );
    }

    #[test]
    fn off_leaves_nothing_out_but_still_orders() {
        let rooms = vec![(1, room(Some(300), 1)), (2, room(Some(40), 1)), (3, room(None, 1))];
        assert_eq!(order_with(rooms, 0, true), (vec![2, 1, 3], 0));
    }

    #[test]
    fn distance_to_round_trip() {
        // Sydney to London, ~17,000 km: about 190 ms (players see ~280 through real routes).
        assert_eq!(from_km(17_000.0), 190);
        assert_eq!(from_km(0.0), BASE_MS);
    }
}
