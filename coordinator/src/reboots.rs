//! When a member may reboot for the system's updates (0.5.2's M4 and M5).
//!
//! Each heartbeat says whether a reboot is pending and since when nobody has used the server
//! (`reboot` in its listing). The answer says `reboot: {why}` when it may, now:
//!
//! - in a booked window covering it (an update or other maintenance, its own or the
//!   network's): "window";
//! - or when nobody has used it for [`IDLE_FOR`], in its quiet hours: "quiet". The quiet hours
//!   are an admin's (`servers.quiet_hours`, "4-7" in UTC) or learned from the last four weeks
//!   of play: the hours of the day with under [`QUIET_SHARE`] of the busiest hour's players,
//!   from this server's own play when it has enough, else the network's;
//! - or when an admin said "reboot when empty, any hour" (until a time): "asked".
//!
//! The server drains and checks again before it reboots. A reboot pending for longer than
//! [`OVERDUE`] is flagged on the Servers page, to book a window.

use serde::Deserialize;
use serde::Serialize;

use crate::Coordinator;

/// How long a server must have been empty.
pub const IDLE_FOR: i64 = 10 * 60;
/// An hour is quiet when it has under this share of the busiest hour's players.
pub const QUIET_SHARE: f64 = 0.10;
/// A pending reboot older than this is flagged.
pub const OVERDUE: i64 = 3 * 86_400;
/// How far back play is counted for the quiet hours.
const LEARN_FROM: i64 = 28 * 86_400;
/// Enough play on a server (player-hours in [`LEARN_FROM`]) to learn its own hours.
const ENOUGH_HOURS: f64 = 100.0;
/// How long "reboot when empty, any hour" lasts.
pub const ANY_HOUR_FOR: i64 = 3 * 86_400;

/// What a server's heartbeat says about reboots (dedicated_server's reboot.rs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_since: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub draining: bool,
}

/// The hours of the day (UTC) a server may reboot in when empty.
pub type Quiet = [bool; 24];

/// Why it may reboot now, if it may.
pub fn why(state: &State, now: i64, in_window: bool, quiet: &Quiet, any_hour: bool) -> Option<&'static str> {
    state.pending_since?;
    if in_window {
        return Some("window");
    }
    let empty = state.idle_since.is_some_and(|since| now - since >= IDLE_FOR);
    if !empty {
        return None;
    }
    if any_hour {
        return Some("asked");
    }
    let hour = usize::try_from(now.rem_euclid(86_400) / 3600).unwrap_or(0);
    quiet[hour].then_some("quiet")
}

/// The quiet hours from player-seconds by hour of the day: those under [`QUIET_SHARE`] of
/// the busiest; with no play at all, none (a new server reboots only in windows or when asked).
pub fn quiet_from(by_hour: &[f64; 24]) -> Quiet {
    let peak = by_hour.iter().copied().fold(0.0, f64::max);
    let mut quiet = [false; 24];
    if peak <= 0.0 {
        return quiet;
    }
    for (h, q) in quiet.iter_mut().enumerate() {
        *q = by_hour[h] < peak * QUIET_SHARE;
    }
    quiet
}

/// An admin's "4-7" (from 04:00 to 07:00 UTC; "22-3" wraps past midnight).
pub fn parse_hours(text: &str) -> Option<Quiet> {
    let (from, to) = text.trim().split_once('-')?;
    let (from, to): (usize, usize) = (from.trim().parse().ok()?, to.trim().parse().ok()?);
    if from > 23 || to > 24 || from == to {
        return None;
    }
    let mut quiet = [false; 24];
    let mut h = from;
    while h != to % 24 {
        quiet[h] = true;
        h = (h + 1) % 24;
    }
    Some(quiet)
}

/// The quiet hours as text, "04-07 UTC" or "04-07, 22-01 UTC", for the Servers page.
pub fn hours_text(quiet: &Quiet) -> String {
    let mut spans = Vec::new();
    let start = (0..24).find(|&h| !quiet[h]);
    let Some(start) = start else { return String::from("all day") };
    let mut h = start;
    let mut open: Option<usize> = None;
    for _ in 0..24 {
        h = (h + 1) % 24;
        match (quiet[h], open) {
            (true, None) => open = Some(h),
            (false, Some(from)) => {
                spans.push(format!("{from:02}-{h:02}"));
                open = None;
            }
            _ => {}
        }
    }
    if spans.is_empty() {
        String::from("none")
    } else {
        format!("{} UTC", spans.join(", "))
    }
}

/// Player-seconds by hour of the day (UTC) from sessions (start, end), clipped to `[from, to)`.
pub fn by_hour(sessions: &[(i64, i64)], from: i64, to: i64) -> [f64; 24] {
    let mut hours = [0.0; 24];
    for &(start, end) in sessions {
        let (mut t, end) = (start.max(from), end.min(to));
        while t < end {
            let next = (t / 3600 + 1) * 3600;
            let upto = next.min(end);
            #[allow(clippy::cast_precision_loss)]
            {
                hours[usize::try_from(t.rem_euclid(86_400) / 3600).unwrap_or(0)] += (upto - t) as f64;
            }
            t = upto;
        }
    }
    hours
}

impl Coordinator {
    /// `server`'s quiet hours and where they come from ("set", "learned", "the network's").
    pub(crate) async fn quiet_hours(&self, server: &str, now: i64) -> sqlx::Result<(Quiet, &'static str)> {
        let set: Option<String> = sqlx::query_scalar("SELECT quiet_hours FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
        if let Some(quiet) = set.as_deref().and_then(parse_hours) {
            return Ok((quiet, "set"));
        }
        let from = now - LEARN_FROM;
        let own: Vec<(i64, i64)> = sqlx::query_as("SELECT started, COALESCE(ended, ?) FROM play_sessions WHERE server_id = ? AND started < ? AND COALESCE(ended, ?) > ?")
            .bind(now)
            .bind(server)
            .bind(now)
            .bind(now)
            .bind(from)
            .fetch_all(&self.pool)
            .await?;
        let hours = by_hour(&own, from, now);
        if hours.iter().sum::<f64>() / 3600.0 >= ENOUGH_HOURS {
            return Ok((quiet_from(&hours), "learned"));
        }
        let all: Vec<(i64, i64)> = sqlx::query_as("SELECT started, COALESCE(ended, ?) FROM play_sessions WHERE started < ? AND COALESCE(ended, ?) > ?")
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(from)
            .fetch_all(&self.pool)
            .await?;
        Ok((quiet_from(&by_hour(&all, from, now)), "the network's"))
    }

    /// The heartbeat answer's `reboot`, for `server` saying `state`.
    pub(crate) async fn reboot_answer(&self, server: &str, state: &State, now: i64) -> sqlx::Result<Option<&'static str>> {
        if state.pending_since.is_none() {
            return Ok(None);
        }
        let in_window = self.in_window(server, crate::maintenance::UPDATE, now).await? || self.in_window(server, crate::maintenance::MAINTENANCE, now).await?;
        let any_until: Option<i64> = sqlx::query_scalar("SELECT reboot_any_hour_until FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
        let (quiet, _) = self.quiet_hours(server, now).await?;
        Ok(why(state, now, in_window, &quiet, any_until.is_some_and(|t| t > now)))
    }

    /// An admin's quiet hours for `server` ("4-7"), or none to learn them: false when the text
    /// doesn't read as hours.
    pub(crate) async fn set_quiet_hours(&self, server: &str, text: Option<&str>) -> sqlx::Result<bool> {
        let text = text.map(str::trim).filter(|t| !t.is_empty());
        if text.is_some_and(|t| parse_hours(t).is_none()) {
            return Ok(false);
        }
        sqlx::query("UPDATE servers SET quiet_hours = ? WHERE id = ?").bind(text).bind(server).execute(&self.pool).await?;
        Ok(true)
    }

    /// "Reboot when empty, any hour" for [`ANY_HOUR_FOR`], or off.
    pub(crate) async fn set_reboot_any_hour(&self, server: &str, on: bool, now: i64) -> sqlx::Result<()> {
        sqlx::query("UPDATE servers SET reboot_any_hour_until = ? WHERE id = ?")
            .bind(on.then_some(now + ANY_HOUR_FOR))
            .bind(server)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// For the Servers page: the reboot state, quiet hours and whether it's overdue.
    pub(crate) async fn reboot_view(&self, server: &str, state: &State, now: i64) -> sqlx::Result<serde_json::Value> {
        let (quiet, source) = self.quiet_hours(server, now).await?;
        let any_until: Option<i64> = sqlx::query_scalar("SELECT reboot_any_hour_until FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
        let set: Option<String> = sqlx::query_scalar("SELECT quiet_hours FROM servers WHERE id = ?").bind(server).fetch_optional(&self.pool).await?.flatten();
        Ok(serde_json::json!({
            "pending_since": state.pending_since,
            "overdue": state.pending_since.is_some_and(|t| now - t > OVERDUE),
            "idle_since": state.idle_since,
            "draining": state.draining,
            "quiet": hours_text(&quiet),
            "quiet_source": source,
            "quiet_set": set,
            "any_hour_until": any_until.filter(|t| *t > now),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY0: i64 = 1_794_528_000; // 2026-11-13 00:00 UTC

    #[test]
    fn a_reboot_waits_for_a_window_or_an_empty_quiet_hour() {
        let quiet = parse_hours("4-7").unwrap();
        let at = |h: i64| DAY0 + h * 3600;
        let pending = State { pending_since: Some(DAY0 - 3600), idle_since: Some(at(5) - 900), draining: false };
        assert_eq!(why(&State { pending_since: None, ..pending }, at(5), true, &quiet, true), None, "nothing pending");
        assert_eq!(why(&pending, at(5), false, &quiet, false), Some("quiet"));
        assert_eq!(why(&pending, at(12), false, &quiet, false), None, "empty, but a busy hour");
        assert_eq!(why(&pending, at(12), false, &quiet, true), Some("asked"));
        assert_eq!(why(&pending, at(12), true, &quiet, false), Some("window"));
        let busy = State { idle_since: None, ..pending };
        assert_eq!(why(&busy, at(5), false, &quiet, false), None, "someone on it");
        assert_eq!(why(&busy, at(12), true, &quiet, false), Some("window"), "a booked window: players were told");
        let just = State { idle_since: Some(at(5) - 300), ..pending };
        assert_eq!(why(&just, at(5), false, &quiet, false), None, "empty for only 5 minutes");
    }

    #[test]
    fn quiet_hours_are_learned_from_play_and_read_from_an_admin() {
        // Busy 16:00-23:00, a little at 02:00; nothing else.
        let sessions = vec![(DAY0 + 16 * 3600, DAY0 + 23 * 3600), (DAY0 + 16 * 3600, DAY0 + 23 * 3600), (DAY0 + 2 * 3600, DAY0 + 2 * 3600 + 600)];
        let hours = by_hour(&sessions, DAY0, DAY0 + 86_400);
        assert!((hours[16] - 7200.0).abs() < 1e-6 && (hours[2] - 600.0).abs() < 1e-6);
        let quiet = quiet_from(&hours);
        assert!(quiet[2] && quiet[5] && !quiet[16] && !quiet[22], "{quiet:?}");
        assert_eq!(quiet_from(&[0.0; 24]), [false; 24], "no play: no quiet hours");
        assert_eq!(hours_text(&parse_hours("4-7").unwrap()), "04-07 UTC");
        assert_eq!(hours_text(&parse_hours("22-2").unwrap()), "22-02 UTC");
        assert!(parse_hours("4-4").is_none() && parse_hours("25-3").is_none() && parse_hours("x").is_none());
    }
}
