//! Urgent notices (0.5.2's U): an admin tells every player at once what the network knows of
//! ("A hotfix is coming tonight", "We're looking into connection problems in Europe"), so players
//! know it's known. Posted, edited and ended in the admin UI (Updates › Notices); every launcher
//! shows the live ones above its Play button (`/v1/servers`, `notices`), and picks up a change
//! within a minute.

use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// The longest notice, in characters.
pub const MAX_TEXT: usize = 200;
/// Live notices at once.
pub const MAX_LIVE: i64 = 3;
/// The longest a notice can be set to last by itself, in hours.
pub const MAX_HOURS: i64 = 14 * 24;
/// What a notice says of itself: news, something wrong that's known, or a fix.
pub const KINDS: [&str; 3] = ["info", "issue", "fixed"];
/// Ended notices the admin UI still lists.
const LISTED_AFTER: i64 = 14 * 86_400;

/// A live notice, as launchers get it. `version` changes with every edit, so a notice a player
/// closed shows again when it's changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notice {
    pub id: i64,
    pub text: String,
    pub kind: String,
    pub posted: i64,
    pub version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends: Option<i64>,
}

/// What an admin posts or edits: the text, its kind, and how many hours until it ends by
/// itself (none: until ended).
#[derive(Debug, Deserialize)]
pub struct Post {
    pub text: String,
    pub kind: String,
    #[serde(default)]
    pub hours: Option<i64>,
}

impl Post {
    pub fn check(&self) -> Result<(), &'static str> {
        let text = self.text.trim();
        if text.is_empty() {
            return Err("a notice needs its text");
        }
        if text.chars().count() > MAX_TEXT {
            return Err("a notice is 200 characters at most");
        }
        if text.chars().any(char::is_control) {
            return Err("a notice is one line");
        }
        if !KINDS.contains(&self.kind.as_str()) {
            return Err("kind is info, issue or fixed");
        }
        if self.hours.is_some_and(|h| !(1..=MAX_HOURS).contains(&h)) {
            return Err("a notice ends by itself in 1 hour to 14 days, or not at all");
        }
        Ok(())
    }

    fn ends(&self, now: i64) -> Option<i64> {
        self.hours.map(|h| now + h * 3600)
    }
}

type Row = (i64, String, String, String, i64, Option<i64>, Option<i64>, Option<i64>);

impl Coordinator {
    /// The live notices, newest first.
    pub(crate) async fn live_notices(&self, now: i64) -> sqlx::Result<Vec<Notice>> {
        let rows: Vec<(i64, String, String, i64, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT id, text, kind, posted_at, edited_at, ends_at FROM notices
              WHERE ended_at IS NULL AND (ends_at IS NULL OR ends_at > ?) ORDER BY posted_at DESC, id DESC LIMIT ?",
        )
        .bind(now)
        .bind(MAX_LIVE)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, text, kind, posted, edited, ends)| Notice {
                id,
                text,
                kind,
                posted,
                version: edited.unwrap_or(posted),
                ends,
            })
            .collect())
    }

    /// The admin UI's list: live notices, and those ended lately.
    pub(crate) async fn notice_list(&self, now: i64) -> sqlx::Result<Value> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, text, kind, posted_by, posted_at, edited_at, ends_at, ended_at FROM notices
              WHERE COALESCE(ended_at, ends_at, ?1) >= ?1 - ?2 ORDER BY posted_at DESC, id DESC LIMIT 50",
        )
        .bind(now)
        .bind(LISTED_AFTER)
        .fetch_all(&self.pool)
        .await?;
        let notices: Vec<Value> = rows
            .into_iter()
            .map(|(id, text, kind, by, posted, edited, ends, ended)| {
                let live = ended.is_none() && ends.is_none_or(|e| e > now);
                json!({ "id": id, "text": text, "kind": kind, "posted_by": by, "posted": posted, "edited": edited, "ends": ends, "ended": ended.or(ends.filter(|e| *e <= now)), "live": live })
            })
            .collect();
        Ok(json!({ "notices": notices, "max_live": MAX_LIVE, "max_text": MAX_TEXT }))
    }

    /// Posts `p` (checked) for `by`: its id, or None when [`MAX_LIVE`] are live already.
    pub(crate) async fn post_notice(&self, p: &Post, by: &str, now: i64) -> sqlx::Result<Option<i64>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notices WHERE ended_at IS NULL AND (ends_at IS NULL OR ends_at > ?)")
            .bind(now)
            .fetch_one(&mut *tx)
            .await?;
        if live >= MAX_LIVE {
            return Ok(None);
        }
        let id: i64 = sqlx::query_scalar("INSERT INTO notices (text, kind, posted_by, posted_at, ends_at) VALUES (?, ?, ?, ?, ?) RETURNING id")
            .bind(p.text.trim())
            .bind(&p.kind)
            .bind(by)
            .bind(now)
            .bind(p.ends(now))
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(id))
    }

    /// Edits a live notice: its text, kind and end. Answers whether there was one.
    pub(crate) async fn edit_notice(&self, id: i64, p: &Post, now: i64) -> sqlx::Result<bool> {
        let done = sqlx::query(
            "UPDATE notices SET text = ?, kind = ?, ends_at = ?, edited_at = MAX(?, posted_at + 1)
              WHERE id = ? AND ended_at IS NULL AND (ends_at IS NULL OR ends_at > ?)",
        )
        .bind(p.text.trim())
        .bind(&p.kind)
        .bind(p.ends(now))
        .bind(now)
        .bind(id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Ends a notice now: answers whether it was live.
    pub(crate) async fn end_notice(&self, id: i64, now: i64) -> sqlx::Result<bool> {
        let done = sqlx::query("UPDATE notices SET ended_at = ? WHERE id = ? AND ended_at IS NULL AND (ends_at IS NULL OR ends_at > ?)")
            .bind(now)
            .bind(id)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_is_one_short_line_of_a_known_kind() {
        let p = |text: &str, kind: &str, hours| Post {
            text: text.into(),
            kind: kind.into(),
            hours,
        };
        assert!(p("A hotfix is coming tonight", "info", None).check().is_ok());
        assert!(p("Looking into it", "issue", Some(6)).check().is_ok());
        assert!(p("  ", "info", None).check().is_err());
        assert!(p(&"x".repeat(201), "info", None).check().is_err());
        assert!(p("two\nlines", "info", None).check().is_err());
        assert!(p("hi", "urgent", None).check().is_err());
        assert!(p("hi", "fixed", Some(0)).check().is_err());
        assert!(p("hi", "fixed", Some(MAX_HOURS + 1)).check().is_err());
    }
}
