//! Moving a direct player to the relay when their joins keep failing (0.5.2's J2, 0.5.5's JC1).
//!
//! A player the server treats as direct (their router forwards a port) may still be unreachable:
//! a firewall on their PC, or a second router that UPnP doesn't speak to (Iceman, 12 Nov: seven
//! joins in twenty minutes, none in). A Storm connection is made from both ends, so nobody can
//! join them and they can join nobody, relayed host or not.
//!
//! The rules:
//! - a **joiner** whose joins failed with **two different hosts** over the last [`WITHIN`], while
//!   someone else got into each of those hosts' rooms, is moved to the relay;
//! - a **joiner** who failed **twice with one host** over the last [`WITHIN`] (two attempts, not
//!   one reported twice: [`SAME_ATTEMPT`]), while someone else got into that host's rooms, is
//!   moved too (JC1). Most failures are one player retrying the one match that's up: on ca1,
//!   10-17 Nov 2026, Player failed into Gbane's match 13 times and JeezyWeezy 7, while relayed
//!   players got in, and the rule above never fired;
//! - a **host** whom **two different players** failed to join over the last [`WITHIN`], while each
//!   of those players got into someone else's room, is moved too;
//! - a **host** whom **three different players** failed to join over the last [`QUIET_WITHIN`],
//!   with at least as many failed attempts as joins that got in, is moved too (JC1; 0.5.4 had it
//!   only for a host nobody got into). A host only some can reach: in Gbane's session of 15 Nov
//!   2026, 12 got in and 26 attempts failed, and nothing moved him;
//! - a **peer** that two different players' games found unreachable over the last [`WITHIN`] is
//!   moved too (JC4: the game names the peer it couldn't reach, which may be any member of the
//!   match, not only its host).
//!
//! One joiner's failures alone never move a host, and nobody is moved back to direct in their
//! session (and for a week on the same network, [`crate::relay_moves`]). The caller moves only
//! direct players.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How far back joins count.
pub const WITHIN: Duration = Duration::from_secs(10 * 60);
/// How far back joins count for a host nobody got into (the quiet-night rule).
pub const QUIET_WITHIN: Duration = Duration::from_secs(30 * 60);
/// How many different players must have failed to join that host.
const QUIET_JOINERS: usize = 3;
/// A player's failures into one host this close together are one attempt: the game reports some
/// twice (Gexiii into tiger's match, 14 Nov: two reports 2 s apart).
pub const SAME_ATTEMPT: Duration = Duration::from_secs(15);
/// How many failed attempts into one host move the joiner (JC1).
const PAIR_ATTEMPTS: usize = 2;
/// How many different players must have found one peer unreachable (JC4).
const UNREACHABLE_REPORTERS: usize = 2;
/// The most joins kept (each kind).
const MAX_KEPT: usize = 20_000;

#[derive(Default)]
pub struct Joins {
    /// (when, joiner, room) of joins that failed, and of joins that worked.
    failed: Vec<(Instant, u32, u32)>,
    worked: Vec<(Instant, u32, u32)>,
    /// (when, reporter, peer): a player's game found another player unreachable (JC4).
    unreachable: Vec<(Instant, u32, u32)>,
    /// Players moved already (this session of theirs).
    moved: HashSet<u32>,
}

impl Joins {
    fn prune(&mut self, now: Instant) {
        let fresh = |(at, ..): &(Instant, u32, u32)| now.duration_since(*at) < WITHIN.max(QUIET_WITHIN);
        self.failed.retain(fresh);
        self.worked.retain(fresh);
        self.unreachable.retain(fresh);
        if self.unreachable.len() > MAX_KEPT {
            self.unreachable.drain(..self.unreachable.len() - MAX_KEPT);
        }
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
        let recent = |(at, ..): &&(Instant, u32, u32)| now.duration_since(*at) < WITHIN;

        // The joiner: two hosts they failed with, each joined by someone else.
        let failed_hosts: HashSet<u32> = self
            .failed
            .iter()
            .filter(recent)
            .filter(|(_, j, _)| *j == joiner)
            .filter_map(|&(_, _, r)| host(r))
            .filter(|h| *h != joiner)
            .collect();
        let others_got_in = |h: u32, host: &mut dyn FnMut(u32) -> Option<u32>| self.worked.iter().filter(recent).any(|&(_, j, r)| j != joiner && host(r) == Some(h));
        let proven: Vec<u32> = failed_hosts.iter().copied().filter(|h| others_got_in(*h, &mut host)).collect();
        // Or one host, twice, that someone else got into (JC1).
        let tries: Vec<Instant> = self
            .failed
            .iter()
            .filter(recent)
            .filter(|&&(_, j, r)| j == joiner && host(r) == Some(this_host))
            .map(|&(at, ..)| at)
            .collect();
        let pair = this_host != joiner && attempts(tries) >= PAIR_ATTEMPTS && others_got_in(this_host, &mut host);
        if (proven.len() >= 2 || pair) && !self.moved.contains(&joiner) {
            moves.push(joiner);
        }

        // The host: two players failed to join them, each of whom got in elsewhere.
        let failed_joiners: HashSet<u32> = self
            .failed
            .iter()
            .filter(recent)
            .filter(|&&(_, j, r)| j != this_host && host(r) == Some(this_host))
            .map(|&(_, j, _)| j)
            .collect();
        let joined_elsewhere =
            |j: u32, host: &mut dyn FnMut(u32) -> Option<u32>| self.worked.iter().filter(recent).any(|&(_, w, r)| w == j && host(r).is_some_and(|h| h != this_host));
        let proven: Vec<u32> = failed_joiners.iter().copied().filter(|j| joined_elsewhere(*j, &mut host)).collect();

        // Three players failed to join them over the last half hour, more often than anyone got
        // in (JC1; with nobody getting in, 0.5.4's quiet night).
        let mut tries: HashMap<u32, Vec<Instant>> = HashMap::new();
        for &(at, j, r) in &self.failed {
            if j != this_host && host(r) == Some(this_host) {
                tries.entry(j).or_default().push(at);
            }
        }
        let failed_attempts: usize = tries.values().map(|t| attempts(t.clone())).sum();
        let got_in = self.worked.iter().filter(|&&(_, w, r)| w != this_host && host(r) == Some(this_host)).count();
        let mostly_failing = tries.len() >= QUIET_JOINERS && failed_attempts >= got_in;

        if (proven.len() >= 2 || mostly_failing) && !self.moved.contains(&this_host) && !moves.contains(&this_host) {
            moves.push(this_host);
        }
        self.moved.extend(&moves);
        moves
    }

    /// `reporter`'s game couldn't reach `peer` (JC4): the players to move to the relay now (the
    /// peer, once two different players' games found it unreachable within [`WITHIN`]).
    pub fn unreachable(&mut self, reporter: u32, peer: u32, now: Instant) -> Vec<u32> {
        if reporter == peer {
            return vec![];
        }
        self.prune(now);
        self.unreachable.push((now, reporter, peer));
        let reporters: HashSet<u32> = self
            .unreachable
            .iter()
            .filter(|&&(at, _, p)| p == peer && now.duration_since(at) < WITHIN)
            .map(|&(_, r, _)| r)
            .collect();
        if reporters.len() >= UNREACHABLE_REPORTERS && self.moved.insert(peer) {
            return vec![peer];
        }
        vec![]
    }

    /// `user` signed out: a later session of theirs starts as their connection says.
    pub fn signed_out(&mut self, user: u32) {
        self.moved.remove(&user);
    }
}

/// How many separate attempts these failure times are: one within [`SAME_ATTEMPT`] of the last
/// counts with it.
fn attempts(mut times: Vec<Instant>) -> usize {
    times.sort();
    let mut n = 0;
    let mut last: Option<Instant> = None;
    for at in times {
        if last.is_none_or(|l| at.duration_since(l) >= SAME_ATTEMPT) {
            n += 1;
            last = Some(at);
        }
    }
    n
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
    fn a_host_nobody_got_into_is_moved_after_three_players_failed() {
        // Gexiii on ca1, 16 Nov 2026: the only public match, joins failing minutes apart.
        let mut j = Joins::default();
        let t = Instant::now();
        let min = |m: u64| t + Duration::from_secs(m * 60);
        assert!(j.failed(7, 301, min(0), host_of).is_empty());
        assert!(j.failed(7, 301, min(2), host_of).is_empty(), "one player twice");
        assert!(j.failed(8, 301, min(10), host_of).is_empty(), "two players");
        assert_eq!(j.failed(9, 301, min(28), host_of), vec![3], "a third, within half an hour");
        assert!(j.failed(10, 301, min(29), host_of).is_empty(), "moved once");
    }

    #[test]
    fn a_host_most_joiners_get_into_isnt_moved() {
        let mut j = Joins::default();
        let t = Instant::now();
        for joiner in 11..=14 {
            j.worked(joiner, 301, t);
        }
        assert!(j.failed(7, 301, t, host_of).is_empty());
        assert!(j.failed(8, 301, t, host_of).is_empty());
        assert!(j.failed(9, 301, t, host_of).is_empty(), "four got in, three failed: the joiners may be at fault");
    }

    #[test]
    fn a_host_only_some_can_reach_is_moved() {
        // Gbane on ca1, 15 Nov 2026: 12 got in, 26 attempts failed.
        let mut j = Joins::default();
        let t = Instant::now();
        let min = |m: u64| t + Duration::from_secs(m * 60);
        j.worked(1, 301, min(0));
        j.worked(2, 301, min(1));
        assert!(j.failed(7, 301, min(2), host_of).is_empty());
        assert!(j.failed(8, 301, min(12), host_of).is_empty(), "two players");
        assert_eq!(j.failed(9, 301, min(20), host_of), vec![3], "a third: three failed, two got in");
    }

    #[test]
    fn a_joiner_failing_twice_with_a_host_others_get_into_is_moved() {
        // Player into Gbane's match on ca1, 15 Nov 2026: 13 failures, while relayed players got in.
        let mut j = Joins::default();
        let t = Instant::now();
        let s = |secs: u64| t + Duration::from_secs(secs);
        j.worked(1, 301, t);
        assert!(j.failed(9, 301, s(10), host_of).is_empty(), "once");
        assert!(j.failed(9, 301, s(14), host_of).is_empty(), "the same attempt reported twice");
        assert_eq!(j.failed(9, 302, s(60), host_of), vec![9], "a second attempt, at another of that host's rooms");
        assert!(j.failed(9, 301, s(90), host_of).is_empty(), "moved once");
    }

    #[test]
    fn a_joiner_failing_twice_with_a_host_nobody_got_into_isnt_moved_for_it() {
        let mut j = Joins::default();
        let t = Instant::now();
        assert!(j.failed(9, 301, t, host_of).is_empty());
        assert!(j.failed(9, 301, t + Duration::from_secs(60), host_of).is_empty(), "maybe the host's fault");
    }

    #[test]
    fn a_peer_two_players_found_unreachable_is_moved() {
        let mut j = Joins::default();
        let t = Instant::now();
        assert!(j.unreachable(7, 5, t).is_empty(), "one player");
        assert!(j.unreachable(7, 5, t).is_empty(), "the same player again");
        assert!(j.unreachable(5, 5, t).is_empty(), "itself");
        assert_eq!(j.unreachable(8, 5, t + Duration::from_secs(30)), vec![5]);
        assert!(j.unreachable(9, 5, t + Duration::from_secs(40)).is_empty(), "moved once");
        let later = t + WITHIN + Duration::from_secs(60);
        assert!(j.unreachable(7, 6, t).is_empty());
        assert!(j.unreachable(8, 6, later).is_empty(), "the first lapsed");
    }

    #[test]
    fn attempts_close_together_are_one() {
        let t = Instant::now();
        let s = |secs: u64| t + Duration::from_secs(secs);
        assert_eq!(attempts(vec![]), 0);
        assert_eq!(attempts(vec![s(0), s(2), s(14)]), 1);
        assert_eq!(attempts(vec![s(32), s(0), s(16)]), 3);
        assert_eq!(attempts(vec![s(0), s(16), s(30)]), 2, "30 is within 15 s of 16");
    }

    #[test]
    fn the_quiet_rule_forgets_after_half_an_hour() {
        let mut j = Joins::default();
        let t = Instant::now();
        assert!(j.failed(7, 301, t, host_of).is_empty());
        assert!(j.failed(8, 301, t, host_of).is_empty());
        let later = t + QUIET_WITHIN + Duration::from_secs(1);
        assert!(j.failed(9, 301, later, host_of).is_empty(), "the first two lapsed");
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
