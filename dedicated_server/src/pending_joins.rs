//! Players on their way into a public match, before their join reaches the server.
//!
//! A player who finds a public match by search has their game probe its host first, and the
//! host's game then tells the match's other players to connect to the newcomer, 12–13 s
//! before the newcomer's JoinSession arrives (eu1, 2026-10-09). Until then the newcomer shares
//! no room with them, so their probes and relayed traffic to each other were refused. When a
//! probe is allowed because the target hosts a public match, the prober is noted here as
//! joining it, with who's in it then: for [`FOR`], the relay carries them and those players
//! to each other ([`together`], used by `Storage::may_relay`). Not probes: a probe hands each
//! side the other's addresses, and anyone may probe a public host without joining, so that
//! would give out the addresses of everyone in any public match; the relay shows neither
//! one's. Nobody else gets anything: a stranger could join the match anyway, and reaches only
//! its players, through the relay, briefly.

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How long a player stays on their way in (their join comes within seconds).
pub const FOR: Duration = Duration::from_secs(30);
/// Players on their way into one host's matches at once, at most (the oldest go first).
const PER_HOST: usize = 16;
/// Players a match is noted with, at most (a match has a dozen at most).
const MEMBERS: usize = 32;
/// Entries in all, at most.
const ALL: usize = 2048;

#[derive(Debug)]
struct Joining {
    host: u32,
    joiner: u32,
    /// Who was in the host's public matches when the joiner probed the host (the host too).
    members: Vec<u32>,
    at: Instant,
}

/// Who's joining whose public match ([`note`], [`together`]); no global state, so it can be
/// tested.
#[derive(Debug, Default)]
pub struct Joins {
    entries: Vec<Joining>,
}

impl Joins {
    /// `joiner` is on their way into `host`'s public matches, whose players are `members`.
    pub fn note(&mut self, host: u32, joiner: u32, members: &[u32], now: Instant) {
        self.entries.retain(|e| now.duration_since(e.at) < FOR && !(e.host == host && e.joiner == joiner));
        if self.entries.iter().filter(|e| e.host == host).count() >= PER_HOST {
            if let Some(oldest) = self.entries.iter().position(|e| e.host == host) {
                self.entries.remove(oldest);
            }
        }
        if self.entries.len() >= ALL {
            self.entries.remove(0);
        }
        let mut members: Vec<u32> = members.iter().copied().filter(|m| *m != joiner).take(MEMBERS).collect();
        if !members.contains(&host) {
            members.push(host);
        }
        self.entries.push(Joining { host, joiner, members, at: now });
    }

    /// Whether one of `a` and `b` is on their way into a public match the other was in.
    pub fn together(&self, a: u32, b: u32, now: Instant) -> bool {
        a != b
            && self
                .entries
                .iter()
                .any(|e| now.duration_since(e.at) < FOR && ((e.joiner == a && e.members.contains(&b)) || (e.joiner == b && e.members.contains(&a))))
    }
}

fn joins() -> std::sync::MutexGuard<'static, Joins> {
    static JOINS: Mutex<Joins> = Mutex::new(Joins { entries: Vec::new() });
    JOINS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`Joins::note`], now.
pub fn note(host: u32, joiner: u32, members: &[u32]) {
    joins().note(host, joiner, members, Instant::now());
}

/// [`Joins::together`], now.
pub fn together(a: u32, b: u32) -> bool {
    joins().together(a, b, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A newcomer and the players of the match they're joining reach each other, both ways,
    /// for a while; nobody else does.
    #[test]
    fn a_joiner_meets_the_match_they_join_for_a_while() {
        let mut j = Joins::default();
        let now = Instant::now();
        let (host, guest, joiner, stranger) = (1, 2, 3, 4);
        assert!(!j.together(guest, joiner, now));
        j.note(host, joiner, &[host, guest], now);
        assert!(j.together(guest, joiner, now), "a guest reaches the newcomer");
        assert!(j.together(joiner, guest, now), "and the newcomer the guest");
        assert!(j.together(host, joiner, now), "the host too");
        assert!(!j.together(stranger, joiner, now), "not someone outside the match");
        assert!(!j.together(stranger, guest, now), "nor anyone to a guest");
        assert!(!j.together(joiner, joiner, now));
        assert!(!j.together(guest, joiner, now + FOR), "only for a while");
        // Probing again starts it again.
        j.note(host, joiner, &[host, guest], now + FOR);
        assert!(j.together(guest, joiner, now + FOR));
        assert_eq!(j.entries.len(), 1, "the old one went");
    }

    /// However many players probe a host, and however many hosts, what's kept is bounded.
    #[test]
    fn what_is_kept_is_bounded() {
        let mut j = Joins::default();
        let now = Instant::now();
        let crowd: Vec<u32> = (100..200).collect();
        for joiner in 1000..1100 {
            j.note(1, joiner, &crowd, now);
        }
        assert_eq!(j.entries.len(), PER_HOST, "per host");
        assert!(j.entries.iter().all(|e| e.members.len() <= MEMBERS + 1));
        assert!(j.together(1099, 1, now), "the latest kept");
        assert!(!j.together(1000, 1, now), "the oldest went");
        for host in 2..3000 {
            j.note(host, 5000 + host, &[host], now);
        }
        assert!(j.entries.len() <= ALL, "in all");
        // Old ones go when anything is noted.
        j.note(7, 8, &[7], now + FOR);
        assert_eq!(j.entries.len(), 1);
    }
}
