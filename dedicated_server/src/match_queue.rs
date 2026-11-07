//! Players waiting to join a public match that can't take them yet: under way (its host's game
//! turns joiners away until the round is over) or full. The launcher's Matches puts a player in
//! a match's queue ("Join when this round ends", "Join when a seat opens", QueueRoom) and keeps
//! them there by asking again while it waits; when the match can take players, the first in
//! line, as many as it has free seats, are let in (JoinRoom), and everyone else's joins and
//! quick-match searches leave it alone until the queue has gone through. Without it, everyone
//! waiting tried at once when the round ended: the first in took the seats, the rest were told
//! the match was full, and a player who clicked Join just then went before them all.
//!
//! In memory only: a restart forgets the queues, and the launchers put their players back in
//! line with their next ask.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// The most players waiting for one match: a full match's worth.
pub const MOST_WAITING: usize = 8;
/// How long a place is kept without the launcher asking again (it asks every few seconds while
/// it waits): a closed launcher's player leaves the queue by then.
const KEPT: Duration = Duration::from_secs(30);
/// How long a place is kept after the player was let in (JoinRoom answered): their game joins
/// within this, and isn't counted in the room before.
const LET_IN_FOR: Duration = Duration::from_secs(45);
/// The most rooms with a queue (each a few dozen bytes); full, the stalest are let go.
const MOST_ROOMS: usize = 10_000;

#[derive(Debug, Clone, Copy)]
struct Waiting {
    user: u32,
    /// When they last asked to stay.
    seen: Instant,
    /// When JoinRoom let them in, if it did.
    let_in: Option<Instant>,
}

static QUEUES: Mutex<Option<HashMap<u32, Vec<Waiting>>>> = Mutex::new(None);

fn queues() -> std::sync::MutexGuard<'static, Option<HashMap<u32, Vec<Waiting>>>> {
    QUEUES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Lets go of places nobody kept, and those let in a while ago.
fn prune(line: &mut Vec<Waiting>, now: Instant) {
    line.retain(|w| match w.let_in {
        Some(at) => now.duration_since(at) < LET_IN_FOR,
        None => now.duration_since(w.seen) < KEPT,
    });
}

/// Why a player couldn't take a place in a match's queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// [`MOST_WAITING`] are waiting already.
    Full,
}

/// Puts `user` in `room`'s queue, or keeps their place there (they asked again), at `now`; out
/// of any other queue they were in (one match at a time). Their place (1 = next) and how many
/// are waiting.
pub fn wait(room: u32, user: u32, now: Instant) -> Result<(usize, usize), Refused> {
    let mut guard = queues();
    let all = guard.get_or_insert_with(HashMap::new);
    for (r, line) in all.iter_mut() {
        if *r != room {
            line.retain(|w| w.user != user);
        }
    }
    all.retain(|_, line| !line.is_empty());
    if all.len() >= MOST_ROOMS && !all.contains_key(&room) {
        all.retain(|_, line| line.iter().any(|w| now.duration_since(w.seen) < KEPT));
        if all.len() >= MOST_ROOMS {
            all.clear();
        }
    }
    let line = all.entry(room).or_default();
    prune(line, now);
    if let Some(w) = line.iter_mut().find(|w| w.user == user) {
        w.seen = now;
    } else {
        if line.len() >= MOST_WAITING {
            return Err(Refused::Full);
        }
        line.push(Waiting { user, seen: now, let_in: None });
    }
    let place = line.iter().position(|w| w.user == user).map_or(0, |p| p + 1);
    Ok((place, line.len()))
}

/// Takes `user` out of `room`'s queue (they left it, or are in the match now).
pub fn leave(room: u32, user: u32) {
    let mut guard = queues();
    if let Some(all) = guard.as_mut() {
        if let Some(line) = all.get_mut(&room) {
            line.retain(|w| w.user != user);
            if line.is_empty() {
                all.remove(&room);
            }
        }
    }
}

/// The players in `room` now (`members`) are out of its line: they're in.
pub fn seated(room: u32, members: &[u32]) {
    let mut guard = queues();
    if let Some(all) = guard.as_mut() {
        if let Some(line) = all.get_mut(&room) {
            line.retain(|w| !members.contains(&w.user));
            if line.is_empty() {
                all.remove(&room);
            }
        }
    }
}

/// `room` is gone: its queue with it.
pub fn ended(room: u32) {
    if let Some(all) = queues().as_mut() {
        all.remove(&room);
    }
}

/// How many wait for `room`, and `user`'s place among them (1 = next; 0: not waiting).
pub fn of(room: u32, user: u32, now: Instant) -> (usize, usize) {
    let mut guard = queues();
    let Some(line) = guard.as_mut().and_then(|all| all.get_mut(&room)) else { return (0, 0) };
    prune(line, now);
    let place = line.iter().position(|w| w.user == user).map_or(0, |p| p + 1);
    (line.len(), place)
}

/// Whether `user` may join `room` now, with `free` seats in it: anyone while nobody waits;
/// otherwise only the first `free` in line (a player let in keeps their place, and their seat,
/// until their game is in). Someone not in line waits behind them, so only seats beyond the
/// queue are theirs.
pub fn may_join(room: u32, user: u32, free: usize, now: Instant) -> bool {
    let (waiting, place) = of(room, user, now);
    if place > 0 {
        place <= free
    } else {
        waiting < free
    }
}

/// [`may_join`], and the player let in noted: their place holds a little longer (their game
/// takes a moment to join), then goes.
pub fn let_in(room: u32, user: u32, free: usize, now: Instant) -> bool {
    if !may_join(room, user, free, now) {
        return false;
    }
    if let Some(w) = queues()
        .as_mut()
        .and_then(|all| all.get_mut(&room))
        .and_then(|line| line.iter_mut().find(|w| w.user == user))
    {
        w.let_in = Some(now);
    }
    true
}

/// Whether a quick-match search by `user` should leave `room` alone: its free seats are
/// promised to the players waiting for it.
pub fn holds_seats(room: u32, user: u32, free: usize, now: Instant) -> bool {
    !may_join(room, user, free, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_in_line_get_the_free_seats() {
        let room = 900_001;
        let now = Instant::now();
        assert_eq!(wait(room, 1, now), Ok((1, 1)));
        assert_eq!(wait(room, 2, now), Ok((2, 2)));
        assert_eq!(wait(room, 3, now), Ok((3, 3)));
        assert_eq!(wait(room, 2, now), Ok((2, 3)), "asking again keeps the place");
        // One seat free: only the first.
        assert!(may_join(room, 1, 1, now));
        assert!(!may_join(room, 2, 1, now), "the second waits for the next seat");
        assert!(!may_join(room, 9, 1, now), "someone not in line doesn't jump it");
        assert!(!may_join(room, 9, 3, now), "three waiting take three seats");
        assert!(may_join(room, 9, 4, now), "a seat beyond the queue is anyone's");
        // The first let in: the next moves up once they're in the match.
        assert!(let_in(room, 1, 1, now));
        seated(room, &[1, 50]);
        assert_eq!(of(room, 2, now), (2, 1));
        assert!(may_join(room, 2, 1, now));
        ended(room);
        assert_eq!(of(room, 2, now), (0, 0));
    }

    #[test]
    fn a_full_queue_refuses_and_a_place_not_kept_goes() {
        let room = 900_002;
        let now = Instant::now();
        for user in 0..MOST_WAITING as u32 {
            assert!(wait(room, 100 + user, now).is_ok());
        }
        assert_eq!(wait(room, 999, now), Err(Refused::Full));
        // Nobody asked again for longer than KEPT: everyone's place goes.
        let later = now + KEPT + Duration::from_secs(1);
        assert_eq!(of(room, 100, later), (0, 0));
        assert_eq!(wait(room, 999, later), Ok((1, 1)));
        ended(room);
    }

    #[test]
    fn one_queue_at_a_time() {
        let now = Instant::now();
        assert_eq!(wait(900_003, 7, now), Ok((1, 1)));
        assert_eq!(wait(900_004, 7, now), Ok((1, 1)));
        assert_eq!(of(900_003, 7, now), (0, 0), "waiting for another match left this one");
        ended(900_004);
    }

    #[test]
    fn a_player_let_in_keeps_their_seat_a_while() {
        let room = 900_005;
        let now = Instant::now();
        wait(room, 1, now).unwrap();
        wait(room, 2, now).unwrap();
        assert!(let_in(room, 1, 1, now));
        // Their game hasn't joined yet: the seat is still theirs, and nobody else's.
        wait(room, 2, now + Duration::from_secs(20)).unwrap();
        let soon = now + Duration::from_secs(40);
        assert!(!may_join(room, 2, 1, soon), "number 2 still waits (they keep asking)");
        let gone = now + LET_IN_FOR + Duration::from_secs(1);
        wait(room, 2, gone).unwrap();
        assert_eq!(of(room, 2, gone), (1, 1), "a let-in place that never joined goes");
        ended(room);
    }
}
