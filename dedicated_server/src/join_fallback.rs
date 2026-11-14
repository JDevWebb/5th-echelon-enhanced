//! Moving a direct player to the relay when their joins keep failing (0.5.2's J2).
//!
//! A player the server treats as direct (their router forwards a port) may still be unreachable:
//! a firewall on their PC, or a second router that UPnP doesn't speak to (Iceman, 12 Nov: seven
//! joins in twenty minutes, none in). A Storm connection is made from both ends, so nobody can
//! join them and they can join nobody, relayed host or not.
//!
//! The rule, over the last [`WITHIN`]:
//! - a **joiner** whose joins failed with **two different hosts**, while someone else got into
//!   each of those hosts' rooms, is moved to the relay;
//! - a **host** whom **two different players** failed to join, while each of those players got
//!   into someone else's room, is moved too.
//!
//! One host's failures alone never move anyone (that host may be the one at fault), and nobody
//! is moved back to direct in their session. The caller moves only direct players.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How far back joins count.
pub const WITHIN: Duration = Duration::from_secs(10 * 60);
/// The most joins kept (each kind).
const MAX_KEPT: usize = 20_000;

#[derive(Default)]
pub struct Joins {
    /// (when, joiner, room) of joins that failed, and of joins that worked.
    failed: Vec<(Instant, u32, u32)>,
    worked: Vec<(Instant, u32, u32)>,
    /// Players moved already (this session of theirs).
    moved: HashSet<u32>,
}

impl Joins {
    fn prune(&mut self, now: Instant) {
        let fresh = |(at, ..): &(Instant, u32, u32)| now.duration_since(*at) < WITHIN;
        self.failed.retain(fresh);
        self.worked.retain(fresh);
        if self.failed.len() > MAX_KEPT {
            self.failed.drain(..self.failed.len() - MAX_KEPT);
        }
        if self.worked.len() > MAX_KEPT {
            self.worked.drain(..self.worked.len() - MAX_KEPT);
        }
    }

    /// `joiner` got into `room`.
    pub fn worked(&mut self, joiner: u32, room: u32, now: Instant) {
        self.prune(now);
        self.worked.push((now, joiner, room));
    }

    /// `joiner` didn't get into `room`: the players to move to the relay now (`host_of` says
    /// whose room each is; none for a room it doesn't know).
    pub fn failed(&mut self, joiner: u32, room: u32, now: Instant, host_of: impl Fn(u32) -> Option<u32>) -> Vec<u32> {
        self.prune(now);
        self.failed.push((now, joiner, room));
        let mut hosts: HashMap<u32, Option<u32>> = HashMap::new();
        let mut host = |room: u32| *hosts.entry(room).or_insert_with(|| host_of(room));
        let Some(this_host) = host(room) else { return vec![] };
        let mut moves = Vec::new();

        // The joiner: two hosts they failed with, each joined by someone else.
        let failed_hosts: HashSet<u32> = self
            .failed
            .iter()
            .filter(|(_, j, _)| *j == joiner)
            .filter_map(|&(_, _, r)| host(r))
            .filter(|h| *h != joiner)
            .collect();
        let others_got_in = |h: u32, host: &mut dyn FnMut(u32) -> Option<u32>| self.worked.iter().any(|&(_, j, r)| j != joiner && host(r) == Some(h));
        let proven: Vec<u32> = failed_hosts.iter().copied().filter(|h| others_got_in(*h, &mut host)).collect();
        if proven.len() >= 2 && !self.moved.contains(&joiner) {
            moves.push(joiner);
        }

        // The host: two players failed to join them, each of whom got in elsewhere.
        let failed_joiners: HashSet<u32> = self
            .failed
            .iter()
            .filter(|&&(_, j, r)| j != this_host && host(r) == Some(this_host))
            .map(|&(_, j, _)| j)
            .collect();
        let joined_elsewhere = |j: u32, host: &mut dyn FnMut(u32) -> Option<u32>| self.worked.iter().any(|&(_, w, r)| w == j && host(r).is_some_and(|h| h != this_host));
        let proven: Vec<u32> = failed_joiners.iter().copied().filter(|j| joined_elsewhere(*j, &mut host)).collect();
        if proven.len() >= 2 && !self.moved.contains(&this_host) && !moves.contains(&this_host) {
            moves.push(this_host);
        }
        self.moved.extend(&moves);
        moves
    }

    /// `user` signed out: a later session of theirs starts as their connection says.
    pub fn signed_out(&mut self, user: u32) {
        self.moved.remove(&user);
    }
}

static JOINS: std::sync::LazyLock<Mutex<Joins>> = std::sync::LazyLock::new(Mutex::default);

pub fn joins() -> std::sync::MutexGuard<'static, Joins> {
    JOINS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the overlay tells a player moved to the relay.
pub const MOVED_TITLE: &str = "Your games now go through the server";
pub const MOVED_TEXT: &str =
    "Your joins kept failing, so your games now go through the server. If this keeps happening, choose Always through the server in the launcher's Settings › Network.";

#[cfg(test)]
mod tests {
    use super::*;

    /// Rooms 1xx are host 1's, 2xx host 2's, 3xx host 3's.
    fn host_of(room: u32) -> Option<u32> {
        Some(room / 100)
    }

    #[test]
    fn a_joiner_failing_with_two_hosts_others_got_into_is_moved() {
        let mut j = Joins::default();
        let t = Instant::now();
        // Others got into hosts 1 and 2.
        j.worked(7, 101, t);
        j.worked(8, 201, t);
        assert!(j.failed(9, 101, t, host_of).is_empty(), "one host isn't enough");
        assert!(j.failed(9, 102, t, host_of).is_empty(), "the same host again isn't either");
        assert_eq!(j.failed(9, 201, t, host_of), vec![9]);
        assert!(j.failed(9, 202, t, host_of).is_empty(), "moved once");
        j.signed_out(9);
    }

    #[test]
    fn hosts_nobody_else_got_into_dont_count() {
        let mut j = Joins::default();
        let t = Instant::now();
        j.worked(7, 101, t);
        assert!(j.failed(9, 101, t, host_of).is_empty());
        assert!(j.failed(9, 301, t, host_of).is_empty(), "nobody got into host 3's room: maybe host 3's fault");
    }

    #[test]
    fn a_host_two_players_failed_to_join_is_moved_when_they_got_in_elsewhere() {
        let mut j = Joins::default();
        let t = Instant::now();
        j.worked(7, 201, t);
        j.worked(8, 201, t);
        assert!(j.failed(7, 301, t, host_of).is_empty());
        assert_eq!(j.failed(8, 302, t, host_of), vec![3]);
    }

    #[test]
    fn old_joins_lapse() {
        let mut j = Joins::default();
        let t = Instant::now();
        j.worked(7, 101, t);
        j.worked(8, 201, t);
        assert!(j.failed(9, 101, t, host_of).is_empty());
        let later = t + WITHIN + Duration::from_secs(1);
        j.worked(8, 201, later);
        assert!(j.failed(9, 201, later, host_of).is_empty(), "the first failure lapsed");
    }
}
