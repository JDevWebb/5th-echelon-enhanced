//! The add-on's fair-play heartbeat (0.5.2's H): while a match is under way, each game's add-on
//! says every minute that its checks run (`Misc.FairPlayCheck` with check `heartbeat`, its
//! count as the value). A game whose heartbeats stop while it still says its match is under
//! way (`Misc.MatchState`, every few seconds) had its checks switched off or their thread
//! killed: noted once per gap (a `fair_play` session event, check `heartbeat`), for the admins.
//!
//! Only a game that has beaten this session is watched: a game from before 0.5.2 never does.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How long without a heartbeat, in a match under way, before it's noted (three missed).
pub const SILENT_FOR: Duration = Duration::from_secs(180);
/// The fewest seconds between two heartbeats taken from one game.
pub const BEAT_EVERY_AT_LEAST: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Beats {
    /// Per player: the last heartbeat, its count, and whether the gap since was noted.
    last: HashMap<u32, (Instant, u64, bool)>,
}

impl Beats {
    /// A heartbeat from `user`'s add-on: false when it came too soon after the last.
    pub fn beat(&mut self, user: u32, count: u64, now: Instant) -> bool {
        if self.last.get(&user).is_some_and(|(at, ..)| now.duration_since(*at) < BEAT_EVERY_AT_LEAST) {
            return false;
        }
        if self.last.len() >= 100_000 {
            self.last.retain(|_, (at, ..)| now.duration_since(*at) < SILENT_FOR * 10);
        }
        self.last.insert(user, (now, count, false));
        true
    }

    /// `user`'s game said whether its match is under way: how long its heartbeats have been
    /// silent, the first time a gap reaches [`SILENT_FOR`] in a match under way.
    pub fn match_said(&mut self, user: u32, under_way: bool, now: Instant) -> Option<Duration> {
        let (at, _, noted) = self.last.get_mut(&user)?;
        let silent = now.duration_since(*at);
        if !under_way || silent < SILENT_FOR || *noted {
            return None;
        }
        *noted = true;
        Some(silent)
    }

    /// `user` signed out: a new session starts unwatched.
    pub fn signed_out(&mut self, user: u32) {
        self.last.remove(&user);
    }
}

static BEATS: std::sync::LazyLock<Mutex<Beats>> = std::sync::LazyLock::new(Mutex::default);

pub fn beats() -> std::sync::MutexGuard<'static, Beats> {
    BEATS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heartbeat_that_stops_in_a_match_is_noted_once() {
        let mut b = Beats::default();
        let t = Instant::now();
        assert_eq!(b.match_said(5, true, t + SILENT_FOR * 2), None, "never beat: a game before 0.5.2");
        assert!(b.beat(5, 1, t));
        assert!(!b.beat(5, 2, t + Duration::from_secs(5)), "too soon");
        assert_eq!(b.match_said(5, true, t + Duration::from_secs(60)), None);
        assert_eq!(b.match_said(5, false, t + SILENT_FOR * 2), None, "not in a match: no beats expected");
        assert_eq!(b.match_said(5, true, t + SILENT_FOR), Some(SILENT_FOR));
        assert_eq!(b.match_said(5, true, t + SILENT_FOR * 2), None, "once per gap");
        assert!(b.beat(5, 3, t + SILENT_FOR * 3), "it came back");
        assert_eq!(b.match_said(5, true, t + SILENT_FOR * 3 + Duration::from_secs(10)), None);
        b.signed_out(5);
        assert_eq!(b.match_said(5, true, t + SILENT_FOR * 9), None);
    }
}
