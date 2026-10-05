//! Maintenance windows: an admin books a time a server (or the whole network, the
//! coordinator itself) will be down. It's a notice: launchers warn the server's players on the
//! day (`/v1/servers`), the game's overlay warns them in the last minutes (each server hears
//! its windows in its heartbeat answer, and tells its games in `/api/info`), and rollouts don't
//! start an update on a server during its window. Nothing is stopped.

use serde::Serialize;
use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// How far ahead windows are told to launchers and servers.
pub const AHEAD: i64 = 7 * 86_400;
/// The longest window.
pub const MAX_LENGTH: i64 = 24 * 3600;
/// How far ahead one may be booked.
pub const MAX_LEAD: i64 = 90 * 86_400;
/// The note for players, at most this many characters.
pub const MAX_NOTE: usize = 100;
/// Windows a server lists in the directory, and tells its games (its own and the network's).
const PER_SERVER: usize = 3;
const PER_HEARTBEAT: usize = 4;
/// Ended windows the admin UI still lists.
const LISTED_AFTER: i64 = 30 * 86_400;

/// A window as players' launchers and games get it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Window {
    pub start: i64,
    pub end: i64,
    pub note: String,
}

/// What an admin books: windows for these servers, and the network's when `network`.
#[derive(Debug, serde::Deserialize)]
pub struct Booking {
    #[serde(default)]
    pub servers: Vec<String>,
    #[serde(default)]
    pub network: bool,
    pub start: i64,
    pub end: i64,
    #[serde(default)]
    pub note: String,
}

impl Booking {
    /// Why it can't be booked at `now`, if it can't.
    pub fn check(&self, now: i64) -> Result<(), &'static str> {
        if self.servers.is_empty() && !self.network {
            return Err("choose at least one server, or the whole network");
        }
        if self.servers.len() > 64 {
            return Err("too many servers");
        }
        if self.start < now - 300 {
            return Err("the start is in the past");
        }
        if self.start > now + MAX_LEAD {
            return Err("book at most 90 days ahead");
        }
        if self.end <= self.start {
            return Err("the end is before the start");
        }
        if self.end - self.start > MAX_LENGTH {
            return Err("a window is at most 24 hours");
        }
        if !crate::valid_text(&self.note, MAX_NOTE) {
            return Err("the note is up to 100 printable characters");
        }
        Ok(())
    }
}

/// Windows not cancelled nor ended, starting within [`AHEAD`] of `now`, in order: (server or
/// None for the network, window).
type Upcoming = Vec<(Option<String>, Window)>;

impl Coordinator {
    pub(crate) async fn upcoming_maintenance(&self, now: i64) -> sqlx::Result<Upcoming> {
        let rows: Vec<(Option<String>, i64, i64, String)> = sqlx::query_as(
            "SELECT server_id, starts, ends, note FROM maintenance
              WHERE cancelled_at IS NULL AND ends > ? AND starts < ? ORDER BY starts, id",
        )
        .bind(now)
        .bind(now + AHEAD)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(server, start, end, note)| (server, Window { start, end, note })).collect())
    }

    /// For the directory: each server's windows, and the network's next one.
    pub(crate) async fn directory_maintenance(&self, now: i64) -> sqlx::Result<(std::collections::HashMap<String, Vec<Window>>, Option<Window>)> {
        let mut servers: std::collections::HashMap<String, Vec<Window>> = std::collections::HashMap::new();
        let mut network = None;
        for (server, w) in self.upcoming_maintenance(now).await? {
            match server {
                Some(id) => {
                    let list = servers.entry(id).or_default();
                    if list.len() < PER_SERVER {
                        list.push(w);
                    }
                }
                None if network.is_none() => network = Some(w),
                None => {}
            }
        }
        Ok((servers, network))
    }

    /// For a server's heartbeat answer: its windows and the network's, marked.
    pub(crate) async fn heartbeat_maintenance(&self, server: &str, now: i64) -> sqlx::Result<Value> {
        let list: Vec<Value> = self
            .upcoming_maintenance(now)
            .await?
            .into_iter()
            .filter(|(s, _)| s.as_deref().is_none_or(|s| s == server))
            .take(PER_HEARTBEAT)
            .map(|(s, w)| json!({ "start": w.start, "end": w.end, "note": w.note, "network": s.is_none() }))
            .collect();
        Ok(json!(list))
    }

    /// Whether `server` (or the whole network) is in a window at `now`: no update starts then.
    pub(crate) async fn in_maintenance(&self, server: &str, now: i64) -> sqlx::Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM maintenance
              WHERE cancelled_at IS NULL AND starts <= ?1 AND ends > ?1 AND (server_id IS NULL OR server_id = ?2)",
        )
        .bind(now)
        .bind(server)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    /// The admin UI's list: windows not ended or ended lately, newest start first.
    pub(crate) async fn maintenance_list(&self, now: i64) -> sqlx::Result<Value> {
        let rows: Vec<(i64, Option<String>, Option<String>, i64, i64, String, String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT m.id, m.server_id, s.listing, m.starts, m.ends, m.note, m.created_by, m.created_at, m.cancelled_at
               FROM maintenance m LEFT JOIN servers s ON s.id = m.server_id
              WHERE m.ends > ? ORDER BY m.starts DESC, m.id DESC LIMIT 200",
        )
        .bind(now - LISTED_AFTER)
        .fetch_all(&self.pool)
        .await?;
        let windows: Vec<Value> = rows
            .into_iter()
            .map(|(id, server, listing, start, end, note, created_by, created_at, cancelled_at)| {
                let name = match &server {
                    None => String::from("Whole network"),
                    Some(id) => listing
                        .and_then(|l| serde_json::from_str::<Value>(&l).ok())
                        .and_then(|l| l["name"].as_str().map(String::from))
                        .unwrap_or_else(|| id.clone()),
                };
                json!({
                    "id": id, "server": server, "server_name": name, "start": start, "end": end, "note": note,
                    "created_by": created_by, "created_at": created_at, "cancelled_at": cancelled_at,
                })
            })
            .collect();
        Ok(json!({ "windows": windows }))
    }

    /// Books `b` (checked) for `by`: one window per server, and one for the network. Answers
    /// their ids, or None when a server isn't a member.
    pub(crate) async fn book_maintenance(&self, b: &Booking, by: &str, now: i64) -> sqlx::Result<Option<Vec<i64>>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut ids = Vec::new();
        let mut servers: Vec<&str> = b.servers.iter().map(String::as_str).collect();
        servers.sort_unstable();
        servers.dedup();
        let mut targets: Vec<Option<&str>> = servers.into_iter().map(Some).collect();
        if b.network {
            targets.push(None);
        }
        for server in targets {
            if let Some(id) = server {
                let known: Option<String> = sqlx::query_scalar("SELECT id FROM servers WHERE id = ?").bind(id).fetch_optional(&mut *tx).await?;
                if known.is_none() {
                    return Ok(None);
                }
            }
            let id: i64 = sqlx::query_scalar("INSERT INTO maintenance (server_id, starts, ends, note, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?) RETURNING id")
                .bind(server)
                .bind(b.start)
                .bind(b.end)
                .bind(b.note.trim())
                .bind(by)
                .bind(now)
                .fetch_one(&mut *tx)
                .await?;
            ids.push(id);
        }
        tx.commit().await?;
        Ok(Some(ids))
    }

    /// Cancels a window: answers whether there was one not cancelled yet.
    pub(crate) async fn cancel_maintenance(&self, id: i64, now: i64) -> sqlx::Result<bool> {
        let done = sqlx::query("UPDATE maintenance SET cancelled_at = ? WHERE id = ? AND cancelled_at IS NULL")
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}

/// `t` (Unix seconds) as "2026-10-06 07:00 UTC", for the audit log.
pub fn utc(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", secs / 3600, secs % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_times_for_the_audit_log() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_791_270_000), "2026-10-06 07:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn a_booking_is_checked() {
        let now = 1_000_000;
        let b = |servers: &[&str], network: bool, start: i64, end: i64, note: &str| Booking {
            servers: servers.iter().map(|s| s.to_string()).collect(),
            network,
            start,
            end,
            note: note.into(),
        };
        assert!(b(&["eu1"], false, now + 60, now + 3660, "Moving").check(now).is_ok());
        assert!(b(&[], true, now, now + 60, "").check(now).is_ok());
        assert!(b(&[], false, now + 60, now + 120, "").check(now).is_err(), "no target");
        assert!(b(&["eu1"], false, now - 3600, now, "").check(now).is_err(), "in the past");
        assert!(b(&["eu1"], false, now + 60, now + 60, "").check(now).is_err(), "no length");
        assert!(b(&["eu1"], false, now, now + MAX_LENGTH + 1, "").check(now).is_err(), "too long");
        assert!(b(&["eu1"], false, now + MAX_LEAD + 1, now + MAX_LEAD + 60, "").check(now).is_err(), "too far ahead");
        assert!(b(&["eu1"], false, now, now + 60, &"x".repeat(MAX_NOTE + 1)).check(now).is_err(), "long note");
        assert!(b(&["eu1"], false, now, now + 60, "a\u{202e}b").check(now).is_err(), "hidden characters");
    }
}
