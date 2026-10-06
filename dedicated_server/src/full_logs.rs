//! The game's whole log when something goes wrong, for players who agreed (launcher
//! Settings › Feedback, off by default): the game says so with its diagnostics
//! (`Misc.ClientLog`, `full_logs`), and when a problem is noted for that player here (a failed
//! join, a lapsed registration, their relayed traffic stopping, a game that dropped and came
//! back), the next answer to its diagnostics asks for its log from 15 minutes before. The
//! game sends it as a report with the trigger `auto`, which only goes through when it was
//! asked for: then it goes as reports do, with this server's side added.
//!
//! At most once an hour per player, and four a day.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How much of the game's log before the problem is wanted.
const BEFORE: i64 = 15 * 60;
/// A player counts as agreeing while their game said so this recently.
const AGREED_FOR: Duration = Duration::from_secs(120);
/// An ask not answered by then is let go.
const ASK_LASTS: Duration = Duration::from_secs(600);
const EVERY: Duration = Duration::from_secs(3600);
const PER_DAY: usize = 4;

#[derive(Default)]
struct State {
    /// Players whose game said they agree, when it last said so, by id; and their names.
    agreed: HashMap<u32, Instant>,
    names: HashMap<String, u32>,
    /// What's wanted of whom: since when (Unix seconds), the problem, when it was asked
    /// (`None`: not yet told).
    wanted: HashMap<u32, (i64, &'static str, Option<Instant>)>,
    /// When each player was asked, the last day.
    asked: HashMap<u32, Vec<Instant>>,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    static STATE: std::sync::LazyLock<Mutex<State>> = std::sync::LazyLock::new(Mutex::default);
    STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `user`'s game sent its diagnostics, saying whether the player agreed.
pub fn diagnostics(user: u32, name: Option<&str>, agreed: bool) {
    let mut st = state();
    if agreed {
        st.agreed.insert(user, Instant::now());
        if let Some(name) = name {
            st.names.insert(name.to_lowercase(), user);
        }
    } else {
        st.agreed.remove(&user);
        st.wanted.remove(&user);
    }
}

/// The problem a session event is, if it's one worth the game's log.
pub fn problem_of(kind: &str, detail: &serde_json::Value) -> Option<&'static str> {
    match kind {
        "join_failed" => Some("join_failed"),
        "nat_lost" => Some("nat_lost"),
        "relay_drop" if detail["direction"] == "sending" && detail["after"].as_f64() == Some(0.0) => Some("relay_stopped"),
        "leave" if detail["how"] == "dropped" => Some("restarted"),
        _ => None,
    }
}

/// A problem was noted for the player `user` (or `name`): their log is wanted, if they
/// agreed and weren't asked lately.
pub fn noted(user: Option<u32>, name: Option<&str>, problem: &'static str, now_unix: i64) {
    let mut st = state();
    let Some(user) = user.or_else(|| name.and_then(|n| st.names.get(&n.to_lowercase()).copied())) else {
        return;
    };
    if !st.agreed.get(&user).is_some_and(|at| at.elapsed() < AGREED_FOR) || st.wanted.contains_key(&user) {
        return;
    }
    let asked = st.asked.entry(user).or_default();
    asked.retain(|at| at.elapsed() < Duration::from_secs(86_400));
    if asked.len() >= PER_DAY || asked.iter().any(|at| at.elapsed() < EVERY) {
        return;
    }
    st.wanted.insert(user, (now_unix - BEFORE, problem, None));
}

/// What to ask `user`'s game for in this answer to its diagnostics: its log since then, and
/// why. Asked once; the ask lasts [`ASK_LASTS`].
pub fn ask(user: u32) -> Option<(i64, &'static str)> {
    let mut st = state();
    let (since, problem, told) = *st.wanted.get(&user)?;
    match told {
        Some(at) if at.elapsed() >= ASK_LASTS => {
            st.wanted.remove(&user);
            None
        }
        Some(_) => None,
        None => {
            st.wanted.insert(user, (since, problem, Some(Instant::now())));
            st.asked.entry(user).or_default().push(Instant::now());
            Some((since, problem))
        }
    }
}

/// `user`'s game sent a report with the trigger `auto`: the problem it was asked for, if it
/// was (the ask is used up).
pub fn answered(user: u32) -> Option<&'static str> {
    let mut st = state();
    match st.wanted.get(&user) {
        Some((_, problem, Some(at))) if at.elapsed() < ASK_LASTS => {
            let problem = *problem;
            st.wanted.remove(&user);
            Some(problem)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_is_asked_for_once_when_the_player_agreed() {
        let (me, other) = (900_001, 900_002);
        // Not agreed: nothing asked.
        noted(Some(other), None, "join_failed", 10_000);
        assert_eq!(ask(other), None);
        diagnostics(me, Some("Oni-Test"), true);
        assert_eq!(answered(me), None, "a report nobody asked for isn't an answer");
        // By name too (nat_lost comes by name).
        noted(None, Some("oni-test"), "nat_lost", 10_000);
        assert_eq!(ask(me), Some((10_000 - BEFORE, "nat_lost")));
        assert_eq!(ask(me), None, "asked once");
        assert_eq!(answered(me), Some("nat_lost"));
        assert_eq!(answered(me), None, "used up");
        // Not again within the hour.
        noted(Some(me), None, "join_failed", 10_100);
        assert_eq!(ask(me), None);
        // Saying no takes it all back.
        diagnostics(me, None, false);
        assert_eq!(
            problem_of("relay_drop", &serde_json::json!({ "direction": "sending", "after": 0.0 })),
            Some("relay_stopped")
        );
        assert_eq!(problem_of("relay_drop", &serde_json::json!({ "direction": "sending", "after": 4.0 })), None);
        assert_eq!(problem_of("leave", &serde_json::json!({ "how": "dropped" })), Some("restarted"));
    }
}
