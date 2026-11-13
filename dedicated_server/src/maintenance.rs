//! Maintenance windows that close the server (0.5.2's M1): while a window the coordinator marked
//! `closed` is under way (this server's own, or the whole network's), sign-ins and the API are
//! refused to everyone but the testers it named, and the others are signed out when it starts.
//!
//! The windows and testers come with each heartbeat answer (`federation.rs`), and are kept in
//! [`FILE`] in the server's directory, so the gate holds when the server restarts while the
//! coordinator can't be reached (a network window is often the coordinator's own maintenance).
//! Support stays open: the launcher writes to the coordinator for it, not to this server.

use std::collections::HashSet;
use std::sync::Mutex;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Where the last windows and testers heard are kept, in the server's working directory.
pub const FILE: &str = "maintenance.json";

/// A window as kept: only what the gate needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub start: i64,
    pub end: i64,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub closed: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Kept {
    windows: Vec<Window>,
    testers: Vec<u32>,
}

/// The gate's state: the last windows and testers heard, and whether a closed window was under
/// way at the last [`Gate::sign_outs_due`].
pub struct Gate {
    kept: Mutex<Option<Kept>>,
    was_closed: Mutex<bool>,
    path: std::path::PathBuf,
}

/// The server's gate.
static GATE: std::sync::LazyLock<Gate> = std::sync::LazyLock::new(|| {
    // Tests that hear windows mustn't write into the source tree.
    if cfg!(test) {
        Gate::new(std::env::temp_dir().join(format!("fe-test-{}-{FILE}", std::process::id())))
    } else {
        Gate::new(FILE)
    }
});

impl Gate {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { kept: Mutex::new(None), was_closed: Mutex::new(false), path: path.into() }
    }

    fn kept(&self) -> std::sync::MutexGuard<'_, Option<Kept>> {
        self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads its file: the gate holds before the first heartbeat answer.
    pub fn load(&self) {
        if let Some(k) = std::fs::read_to_string(&self.path).ok().and_then(|s| serde_json::from_str::<Kept>(&s).ok()) {
            *self.kept() = Some(k);
        }
    }

    /// Keeps a heartbeat answer's windows (already checked by `federation::note_maintenance`)
    /// and testers, written to its file when they changed. An answer without testers (an older
    /// coordinator) has none.
    pub fn note(&self, windows: &[Value], answer: &Value) {
        let new = Kept {
            windows: windows.iter().filter_map(|w| serde_json::from_value(w.clone()).ok()).collect(),
            testers: answer["maintenance_testers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_u64().and_then(|t| u32::try_from(t).ok()))
                .take(1000)
                .collect(),
        };
        let mut k = self.kept();
        if k.as_ref() != Some(&new) {
            if let Ok(s) = serde_json::to_string(&new) {
                let _ = std::fs::write(&self.path, s);
            }
            *k = Some(new);
        }
    }

    /// The closed window under way at `now`, if any.
    pub fn closed_now(&self, now: i64) -> Option<Window> {
        closed_in(self.kept().as_ref()?, now)
    }

    /// Whether `user_id` may sign in and use the server at `now`: an error with what to tell
    /// the player while a closed window keeps them out.
    pub fn may_play(&self, user_id: u32, now: i64) -> Result<(), String> {
        let k = self.kept();
        let Some(k) = k.as_ref() else { return Ok(()) };
        match closed_in(k, now) {
            Some(w) if !k.testers.contains(&user_id) => Err(message(&w)),
            _ => Ok(()),
        }
    }

    /// When a closed window has just started: the players of `online` to sign out (all but the
    /// testers). Each window's start counts once; nothing while none is under way.
    pub fn sign_outs_due(&self, online: &[u32], now: i64) -> Vec<u32> {
        let k = self.kept();
        let closed = k.as_ref().and_then(|k| closed_in(k, now));
        let mut was = self.was_closed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let starting = closed.is_some() && !*was;
        *was = closed.is_some();
        if !starting {
            return vec![];
        }
        let testers: HashSet<u32> = k.as_ref().map(|k| k.testers.iter().copied().collect()).unwrap_or_default();
        online.iter().copied().filter(|u| !testers.contains(u)).collect()
    }
}

fn closed_in(k: &Kept, now: i64) -> Option<Window> {
    k.windows.iter().filter(|w| w.closed && w.start <= now && now < w.end).max_by_key(|w| w.end).cloned()
}

/// The server's gate (see [`Gate`]).
pub fn gate() -> &'static Gate {
    &GATE
}

/// Whether `user_id` may play now (the server's gate): not while a closed window keeps them
/// out, nor while the server drains for a reboot (reboot.rs).
pub fn may_play(user_id: u32) -> Result<(), String> {
    if crate::reboot::draining() {
        return Err(crate::reboot::DRAINING_MESSAGE.to_string());
    }
    GATE.may_play(user_id, identity::now())
}

/// What a player kept out is told. It starts with "Closed for maintenance" (the launcher and the
/// overlay know it by that) and ends with the end time, in UTC (the launcher shows its own time
/// from `/api/info`'s windows).
pub fn message(w: &Window) -> String {
    let note = w.note.trim();
    let what = if note.is_empty() { String::new() } else { format!(" ({note})") };
    format!(
        "Closed for maintenance{what} until {}. Only testers can play until then; Support in the launcher still works.",
        utc_time(w.end)
    )
}

/// `t` as "21:30 UTC" (with the date when it isn't today's).
fn utc_time(t: i64) -> String {
    let now_day = identity::now().div_euclid(86_400);
    let (day, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let time = format!("{:02}:{:02} UTC", secs / 3600, secs % 3600 / 60);
    if day == now_day {
        time
    } else {
        // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
        let z = day + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        format!("{} {d} {}", ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(m - 1) as usize], time)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_closed_window_keeps_all_but_testers_out_and_signs_them_out_once() {
        let path = std::env::temp_dir().join(format!("fe-maintenance-{}.json", std::process::id()));
        let gate = Gate::new(&path);
        let now = identity::now();
        // Nothing heard: open.
        assert!(gate.may_play(5, now).is_ok());
        let windows = vec![
            json!({ "start": now - 60, "end": now + 3600, "note": "Moving to a faster machine", "closed": true, "network": false, "update": false }),
            json!({ "start": now - 60, "end": now + 7200, "note": "Notice only", "closed": false, "network": true, "update": false }),
        ];
        gate.note(&windows, &json!({ "maintenance_testers": [1006] }));
        let why = gate.may_play(5, now).unwrap_err();
        assert!(why.starts_with("Closed for maintenance (Moving to a faster machine) until "), "{why}");
        assert!(gate.may_play(1006, now).is_ok(), "a tester");
        assert!(gate.may_play(5, now + 3600).is_ok(), "over at its end; the notice-only window doesn't close");
        assert!(gate.may_play(5, now - 120).is_ok(), "not yet");

        // Its start signs out everyone but the testers, once.
        assert_eq!(gate.sign_outs_due(&[5, 1006, 7], now), vec![5, 7]);
        assert!(gate.sign_outs_due(&[5, 1006, 7], now + 30).is_empty(), "once");
        assert!(gate.sign_outs_due(&[5], now + 3700).is_empty(), "over");

        // Kept on disk: read back at a restart.
        let restarted = Gate::new(&path);
        restarted.load();
        assert!(restarted.may_play(5, now).is_err());
        assert!(restarted.may_play(1006, now).is_ok());
        // An older coordinator's answer has no testers.
        restarted.note(&windows, &json!({}));
        assert!(restarted.may_play(1006, now).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_end_is_said_in_utc() {
        let w = Window { start: 0, end: 86_400 * 3 + 21 * 3600 + 30 * 60, note: String::new(), closed: true };
        assert_eq!(message(&w), "Closed for maintenance until Jan 4 21:30 UTC. Only testers can play until then; Support in the launcher still works.");
    }
}
