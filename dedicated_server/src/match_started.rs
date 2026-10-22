//! Matches that look started: a game's join into them was just refused as locked
//! (`reports::LOCKED_SESSION`, the host's game turning a joiner away once its match is under
//! way).
//!
//! The server isn't told when a match starts: the room's attributes stay as they were (as far
//! as anyone has seen). So it learns from the refusals: the first player who joins a started
//! match is turned away by its host's game, and after that the friend list stops offering Join
//! there and `JoinNow` says the match has started, instead of sending everyone else into the
//! same refusal. Forgotten [`FOR`] after the last refusal, for the match is over by then or the
//! next try finds out again.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::PoisonError;
use std::time::Duration;
use std::time::Instant;

/// How long after a refusal the match counts as started: about a Spies vs Mercs match, so a
/// lobby that went back to waiting doesn't stay closed to Join for long.
const FOR: Duration = Duration::from_secs(10 * 60);

/// At most this many rooms remembered (a game reports a few failures a minute at most).
const MAX_ROOMS: usize = 4096;

fn rooms() -> &'static Mutex<HashMap<u32, Instant>> {
    static ROOMS: OnceLock<Mutex<HashMap<u32, Instant>>> = OnceLock::new();
    ROOMS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A join into `room` was refused as locked: it counts as started for [`FOR`].
pub fn refused(room: u32) {
    refused_at(room, Instant::now());
}

fn refused_at(room: u32, now: Instant) {
    let mut rooms = rooms().lock().unwrap_or_else(PoisonError::into_inner);
    if rooms.len() >= MAX_ROOMS {
        rooms.retain(|_, at| now.saturating_duration_since(*at) < FOR);
    }
    if rooms.len() < MAX_ROOMS {
        rooms.insert(room, now);
    }
}

/// Whether `room`'s match looks started (see the module's comment).
pub fn started(room: u32) -> bool {
    started_at(room, Instant::now())
}

fn started_at(room: u32, now: Instant) -> bool {
    let mut rooms = rooms().lock().unwrap_or_else(PoisonError::into_inner);
    match rooms.get(&room) {
        Some(at) if now.saturating_duration_since(*at) < FOR => true,
        Some(_) => {
            rooms.remove(&room);
            false
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_room_counts_as_started_for_a_while() {
        let (room, now) = (3_000_001, Instant::now());
        assert!(!started_at(room, now), "nothing refused yet");
        refused_at(room, now);
        assert!(started_at(room, now + Duration::from_secs(60)));
        assert!(!started_at(3_000_009, now), "only that room");
        assert!(!started_at(room, now + FOR), "forgotten after a while");
        assert!(!started_at(room, now), "and gone");
    }

    #[test]
    fn a_later_refusal_keeps_it_started() {
        let (room, now) = (3_000_002, Instant::now());
        refused_at(room, now);
        refused_at(room, now + Duration::from_secs(300));
        assert!(started_at(room, now + FOR), "counted from the last refusal");
    }
}
