//! Reboots for the system's own updates, only when they hurt nobody (0.5.2's M4 and M5).
//!
//! Unattended upgrades install security fixes, and some (a new kernel) need a reboot:
//! `/run/reboot-required` says so. The installer no longer lets unattended upgrades reboot at a
//! fixed hour. Instead this server tells the coordinator in each heartbeat that a reboot is
//! pending and how long it has been empty ([`State`]); the coordinator answers `reboot` when
//! it's in a booked window, or has been empty long enough in its quiet hours. Then the server
//! drains (refuses new sign-ins for the couple of minutes it takes, and a relay is left out of
//! new matches), checks it's still empty, and asks the root updater to reboot, as it asks for
//! a release (`self_update.rs`): the word `reboot` in its request file.

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;
use slog::info;
use slog::warn;
use slog::Logger;

/// Says an update of the system needs a reboot (Debian and Ubuntu's update-notifier).
const REBOOT_REQUIRED: &str = "/run/reboot-required";
/// What the updater is asked for.
const REQUEST: &str = "reboot";
/// How long draining waits before the last check and the reboot.
pub const DRAIN_FOR: Duration = Duration::from_secs(20);
/// What a player signing in while it drains is told.
pub const DRAINING_MESSAGE: &str = "The server is restarting for an update of its system: back in about 2 minutes.";

/// What the heartbeat says about reboots.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct State {
    /// Since when a reboot has been pending (Unix seconds), if one is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_since: Option<i64>,
    /// Since when nobody has played here or been relayed by it, if nobody is now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_since: Option<i64>,
    /// Refusing new players for a reboot just asked for.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub draining: bool,
}

struct Tracked {
    idle_since: Option<i64>,
    draining: Option<Instant>,
}

static TRACKED: Mutex<Tracked> = Mutex::new(Tracked { idle_since: None, draining: None });

fn tracked() -> std::sync::MutexGuard<'static, Tracked> {
    TRACKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Since when a reboot has been pending, if one is.
pub fn pending_since() -> Option<i64> {
    let modified = std::fs::metadata(REBOOT_REQUIRED).ok()?.modified().ok()?;
    let secs = modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}

/// Notes whether anyone uses the server now (players signed in, matches, relayed players):
/// the idle time starts when nobody does, and ends when anyone does.
pub fn note_activity(busy: bool, now: i64) {
    let mut t = tracked();
    t.idle_since = match (busy, t.idle_since) {
        (true, _) => None,
        (false, Some(since)) => Some(since),
        (false, None) => Some(now),
    };
}

/// The heartbeat's part.
pub fn state() -> State {
    let t = tracked();
    State {
        pending_since: pending_since(),
        idle_since: t.idle_since,
        draining: t.draining.is_some(),
    }
}

/// Whether it's draining for a reboot: new players are refused ([`DRAINING_MESSAGE`]).
pub fn draining() -> bool {
    tracked().draining.is_some()
}

/// The coordinator's answer says to reboot (`why`: "window" in a booked window, else idle in
/// its quiet hours). Starts draining; at the next heartbeat after [`DRAIN_FOR`], if a reboot is
/// still pending and (outside a window) nobody came, asks the updater. Someone who came stops
/// it, to be tried another time.
pub fn answer(logger: &Logger, why: Option<&str>, now: i64) {
    let Some(why) = why else {
        // Not (or no longer) allowed: stop draining.
        if tracked().draining.take().is_some() {
            info!(logger, "Reboot: called off by the coordinator; taking players again");
        }
        return;
    };
    if pending_since().is_none() || !crate::self_update::updater_installed() {
        return;
    }
    let mut t = tracked();
    match t.draining {
        None => {
            t.draining = Some(Instant::now());
            info!(logger, "Reboot: an update of the system needs one, and the coordinator says now ({why}); draining");
        }
        Some(since) if since.elapsed() >= DRAIN_FOR => {
            let _ = now;
            let empty = t.idle_since.is_some();
            if why != "window" && !empty {
                t.draining = None;
                info!(logger, "Reboot: someone came while draining; called off for now");
                return;
            }
            drop(t);
            ask_updater(logger);
        }
        Some(_) => {}
    }
}

fn ask_updater(logger: &Logger) {
    let file = crate::self_update::REQUEST_FILE;
    let tmp = format!("{file}.tmp");
    match std::fs::write(&tmp, format!("{REQUEST}\n")).and_then(|()| std::fs::rename(&tmp, file)) {
        Ok(()) => info!(logger, "Reboot: asked the updater to reboot the machine"),
        Err(e) => warn!(logger, "Reboot: couldn't ask the updater: {e}"),
    }
}

/// Whether anyone uses the server, from what the heartbeat counts.
pub fn busy(players_online: u32, relayed: u32, playing: bool) -> bool {
    players_online > 0 || relayed > 0 || playing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_time_starts_when_nobody_plays_and_ends_when_someone_does() {
        note_activity(true, 100);
        assert_eq!(state().idle_since, None);
        note_activity(false, 200);
        note_activity(false, 260);
        assert_eq!(state().idle_since, Some(200), "kept from when it went quiet");
        note_activity(true, 300);
        assert_eq!(state().idle_since, None);
        assert!(busy(0, 1, false) && busy(1, 0, false) && busy(0, 0, true) && !busy(0, 0, false));
    }
}
