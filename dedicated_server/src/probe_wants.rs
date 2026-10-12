//! Which players' games asked to reach which, lately: a probe between two players of a public
//! match, neither of them its host, goes ahead only when the target's game asked to reach the
//! caller too (`nat_traversal.rs`).
//!
//! A probe hands each side the other's address, and anyone may become a member of a public
//! match without playing in it (JoinSession makes a guest, and AddParticipants of oneself
//! is only refused in private rooms), so membership alone would let a stranger collect the
//! addresses of everyone in any public match. Two players really in one match both want to
//! reach each other: their games each ask to probe the other (eu1, 2026-10-11: the
//! newcomer's game asks for every player of the match, and theirs ask for the newcomer,
//! within seconds). A stranger's request is noted and refused; nobody's game asks back.
//! They still reach each other through the relay, which shows neither address.

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How long a request to reach someone counts.
pub const FOR: Duration = Duration::from_secs(60);
/// Requests kept per caller, and in all, at most (the oldest go first).
const PER_CALLER: usize = 32;
const ALL: usize = 4096;

#[derive(Debug)]
struct Want {
    caller: u32,
    target: u32,
    at: Instant,
}

/// Who asked to reach whom ([`Wants::note`], [`Wants::wanted`]); no global state, so it can
/// be tested.
#[derive(Debug, Default)]
pub struct Wants {
    entries: Vec<Want>,
}

impl Wants {
    /// `caller`'s game asked to probe `target`.
    pub fn note(&mut self, caller: u32, target: u32, now: Instant) {
        self.entries.retain(|w| now.duration_since(w.at) < FOR && !(w.caller == caller && w.target == target));
        if self.entries.iter().filter(|w| w.caller == caller).count() >= PER_CALLER {
            if let Some(oldest) = self.entries.iter().position(|w| w.caller == caller) {
                self.entries.remove(oldest);
            }
        }
        if self.entries.len() >= ALL {
            self.entries.remove(0);
        }
        self.entries.push(Want { caller, target, at: now });
    }

    /// Whether `caller`'s game asked to probe `target` within [`FOR`].
    pub fn wanted(&self, caller: u32, target: u32, now: Instant) -> bool {
        self.entries.iter().any(|w| w.caller == caller && w.target == target && now.duration_since(w.at) < FOR)
    }
}

fn wants() -> std::sync::MutexGuard<'static, Wants> {
    static WANTS: Mutex<Wants> = Mutex::new(Wants { entries: Vec::new() });
    WANTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`Wants::note`], now.
pub fn note(caller: u32, target: u32) {
    wants().note(caller, target, Instant::now());
}

/// [`Wants::wanted`], now.
pub fn wanted(caller: u32, target: u32) -> bool {
    wants().wanted(caller, target, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_counts_one_way_for_a_while() {
        let mut w = Wants::default();
        let now = Instant::now();
        w.note(1, 2, now);
        assert!(w.wanted(1, 2, now));
        assert!(!w.wanted(2, 1, now), "only the way it was asked");
        assert!(!w.wanted(1, 3, now));
        assert!(!w.wanted(1, 2, now + FOR), "only for a while");
    }

    #[test]
    fn what_is_kept_is_bounded() {
        let mut w = Wants::default();
        let now = Instant::now();
        for target in 0..100 {
            w.note(1, target, now);
        }
        assert_eq!(w.entries.len(), PER_CALLER);
        assert!(w.wanted(1, 99, now) && !w.wanted(1, 0, now), "the oldest went");
        for caller in 2..6000 {
            w.note(caller, 1, now);
        }
        assert!(w.entries.len() <= ALL);
    }
}
