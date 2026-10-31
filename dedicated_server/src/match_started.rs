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

/// How many matches one player's refusals mark as started within [`FOR`]: a player is turned
/// away from a started match once or twice, and one making reports up shouldn't close every
/// match to Join.
const PER_REPORTER: usize = 3;

/// How long a player's marks are remembered. A player marks a room once: their later refusals
/// there neither renew it nor, once it has lapsed, mark it again (room ids aren't reused), so
/// one player making reports up can't keep a match out of searches past [`FOR`]. Another
/// player's refusal keeps a match that's still under way marked.
const MARKS_KEPT: Duration = Duration::from_secs(60 * 60);

/// How long a host's word on its match counts without being said again (the game says it every
/// 30 s while under way).
const HOST_SAYS_FOR: Duration = Duration::from_secs(90);

#[derive(Default)]
struct State {
    rooms: HashMap<u32, Instant>,
    /// What the hosts' games said of their matches (0.4.8 plan M2: the round under way, read
    /// from the game's own match state), and when.
    hosts: HashMap<u32, (bool, Instant)>,
    /// Which rooms each player marked when, within [`MARKS_KEPT`].
    marks: HashMap<u32, Vec<(u32, Instant)>>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}

/// `reporter`'s join into `room` was refused as locked: it counts as started for [`FOR`], unless
/// `reporter` marked it before (see [`MARKS_KEPT`]). Whether it was taken (not past
/// [`PER_REPORTER`], and the first from this player there).
pub fn refused(room: u32, reporter: u32) -> bool {
    refused_at(room, reporter, Instant::now())
}

fn refused_at(room: u32, reporter: u32, now: Instant) -> bool {
    let mut state = state().lock().unwrap_or_else(PoisonError::into_inner);
    let State { rooms, marks, .. } = &mut *state;
    if rooms.len() >= MAX_ROOMS {
        rooms.retain(|_, at| now.saturating_duration_since(*at) < FOR);
        marks.retain(|_, m| {
            m.retain(|(_, at)| now.saturating_duration_since(*at) < MARKS_KEPT);
            !m.is_empty()
        });
    }
    let mine = marks.entry(reporter).or_default();
    mine.retain(|(_, at)| now.saturating_duration_since(*at) < MARKS_KEPT);
    if let Some((_, at)) = mine.iter().find(|(r, _)| *r == room) {
        // Still marked by this player: counted already, and not renewed.
        return now.saturating_duration_since(*at) < FOR;
    }
    if mine.iter().filter(|(_, at)| now.saturating_duration_since(*at) < FOR).count() >= PER_REPORTER {
        return false;
    }
    if rooms.len() >= MAX_ROOMS {
        return false;
    }
    mine.push((room, now));
    rooms.insert(room, now);
    true
}

/// The host's game said whether the match in `rooms` (the ones it hosts) is under way.
pub fn host_says(rooms: &[u32], under_way: bool) {
    let now = Instant::now();
    let mut state = state().lock().unwrap_or_else(PoisonError::into_inner);
    if state.hosts.len() >= MAX_ROOMS {
        state.hosts.retain(|_, (_, at)| now.saturating_duration_since(*at) < HOST_SAYS_FOR);
    }
    for room in rooms {
        if state.hosts.len() < MAX_ROOMS || state.hosts.contains_key(room) {
            state.hosts.insert(*room, (under_way, now));
        }
    }
}

/// Whether `room`'s host's game says its match is under way now (M2): what it does then is
/// done mid-match (F4). Not refusal marks, which are about joins.
pub fn host_says_under_way(room: u32) -> bool {
    let state = state().lock().unwrap_or_else(PoisonError::into_inner);
    state
        .hosts
        .get(&room)
        .is_some_and(|(under_way, at)| *under_way && Instant::now().saturating_duration_since(*at) < HOST_SAYS_FOR)
}

/// Whether `room`'s match looks started (see the module's comment), or its host's game says
/// it's under way.
pub fn started(room: u32) -> bool {
    started_at(room, Instant::now())
}

fn started_at(room: u32, now: Instant) -> bool {
    let mut state = state().lock().unwrap_or_else(PoisonError::into_inner);
    if state
        .hosts
        .get(&room)
        .is_some_and(|(under_way, at)| *under_way && now.saturating_duration_since(*at) < HOST_SAYS_FOR)
    {
        return true;
    }
    let rooms = &mut state.rooms;
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
    fn a_hosts_word_holds_while_said_again() {
        let room = 3_100_001;
        assert!(!started(room));
        host_says(&[room], true);
        assert!(started(room), "the host's game says it's under way");
        assert!(!started_at(room, Instant::now() + HOST_SAYS_FOR), "not said again: forgotten");
        host_says(&[room], false);
        assert!(!started(room), "joinable again");
    }

    #[test]
    fn a_refused_room_counts_as_started_for_a_while() {
        let (room, now) = (3_000_001, Instant::now());
        assert!(!started_at(room, now), "nothing refused yet");
        refused_at(room, 1, now);
        assert!(started_at(room, now + Duration::from_secs(60)));
        assert!(!started_at(3_000_009, now), "only that room");
        assert!(!started_at(room, now + FOR), "forgotten after a while");
        assert!(!started_at(room, now), "and gone");
    }

    #[test]
    fn another_players_later_refusal_keeps_it_started() {
        let (room, now) = (3_000_002, Instant::now());
        refused_at(room, 2, now);
        refused_at(room, 5, now + Duration::from_secs(300));
        assert!(started_at(room, now + FOR), "counted from the last refusal");
    }

    #[test]
    fn one_player_cant_keep_a_match_marked() {
        let (room, now) = (3_000_003, Instant::now());
        assert!(refused_at(room, 6, now));
        assert!(refused_at(room, 6, now + Duration::from_secs(300)), "still theirs");
        assert!(!started_at(room, now + FOR), "not renewed by the same player");
        assert!(!refused_at(room, 6, now + FOR), "nor marked again once it lapsed");
        assert!(!started_at(room, now + FOR + Duration::from_secs(1)));
        assert!(refused_at(room, 6, now + MARKS_KEPT), "an hour on, a room id is someone else's game");
    }

    #[test]
    fn one_player_marks_only_a_few_matches() {
        let (reporter, now) = (3, Instant::now());
        for room in 3_000_100..3_000_100 + PER_REPORTER as u32 {
            assert!(refused_at(room, reporter, now));
        }
        assert!(!refused_at(3_000_200, reporter, now), "one more is refused");
        assert!(!started_at(3_000_200, now));
        assert!(refused_at(3_000_100, reporter, now), "one already marked still counts");
        assert!(refused_at(3_000_200, reporter, now + FOR), "and once they've lapsed, more");
        assert!(refused_at(3_000_200, 4, now), "another player's count is their own");
    }
}
