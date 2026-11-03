//! Who's on which side of the match, as this game has the players' characters (pawns.rs): each
//! named character's player and side, spy or merc by its class. The host's game sends it with
//! its match state (round_state.rs), for the launcher's Matches tab. A character whose player
//! isn't named here (a remote one, if the game doesn't keep their controller) is left out.

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use crate::pawns::Sample;
use crate::pawns::Tables;
use crate::pawns::Watcher;

/// How often the roster is read from the samples, and how long it holds without a character.
const EVERY: Duration = Duration::from_secs(2);
const HOLDS: Duration = Duration::from_secs(20);
/// The most players kept (a lobby pushed past its size has a few more than 8).
const MOST: usize = 24;

static ROSTER: Mutex<(Vec<(String, &'static str)>, Option<Instant>)> = Mutex::new((Vec::new(), None));

/// The side a player character's class is on.
pub fn side(class: &str) -> Option<&'static str> {
    if class.contains("Spy") {
        Some("spy")
    } else if class.contains("Merc") {
        Some("merc")
    } else {
        None
    }
}

/// The players and their sides, as last seen (empty once nobody's been seen for a while).
pub fn current() -> Vec<(String, &'static str)> {
    let r = ROSTER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match r.1 {
        Some(at) if at.elapsed() < HOLDS => r.0.clone(),
        _ => Vec::new(),
    }
}

#[derive(Default)]
pub struct Roster {
    last: Option<Instant>,
}

impl Watcher for Roster {
    fn sample(&mut self, _tables: &Tables, now: Instant, characters: &[Sample]) {
        if self.last.is_some_and(|at| now.duration_since(at) < EVERY) {
            return;
        }
        self.last = Some(now);
        let mut list: Vec<(String, &'static str)> = characters
            .iter()
            .filter_map(|c| Some((c.control.as_ref()?.name.clone().filter(|n| !n.is_empty())?, side(c.class)?)))
            .take(MOST)
            .collect();
        list.sort();
        list.dedup_by(|a, b| a.0 == b.0);
        if !list.is_empty() {
            *ROSTER.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = (list, Some(now));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_characters_class_says_its_side() {
        assert_eq!(super::side("EMPSpy"), Some("spy"));
        assert_eq!(super::side("EPlayerSpy"), Some("spy"));
        assert_eq!(super::side("EMPFirstPersonMerc"), Some("merc"));
        assert_eq!(super::side("ESam"), None);
    }
}
