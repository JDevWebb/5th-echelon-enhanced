//! Support: one conversation per player with the network's admins, on the community
//! network's coordinator only (with the roadmap, `--roadmap`).
//!
//! * `POST /v1/support` (the player's launcher): `{identity, name, server, launcher, time,
//!   text, files: [{name, size, gzip_base64}], signature}`, signed with the player's identity
//!   over [`identity::support_message`] for this coordinator's name. Only from an identity
//!   linked to an account on a member server (a real player, not a key made up for it).
//! * `GET /v1/support/mine?identity&time&signature[&read=1]`: the conversation, signed over
//!   [`identity::support_read_message`]; `read` marks the admins' answers read (the launcher's
//!   Support page is open).
//! * Admins read and answer on the admin UI's Support page (`admin/support.rs`); each server's
//!   pulse answer says which of its players online have answers unread (for the overlay).
//!
//! Kept until a conversation has been quiet for [`KEEP_FOR`]; deleted with the player.

use base64::Engine as _;
use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

use crate::reports::printable;
use crate::Coordinator;

/// The longest message, either way.
pub const MAX_TEXT: usize = 2000;
/// Files on one message, each at most this large gzipped and unpacked, and all together.
pub const MAX_FILES: usize = 8;
pub const MAX_FILE_GZIP: usize = 3 * 1024 * 1024;
pub const MAX_FILE_SIZE: usize = 16 * 1024 * 1024;
pub const MAX_FILES_GZIP: usize = 4 * 1024 * 1024;
/// The largest request body (the files in base64, and the rest).
pub const MAX_BODY: usize = 6 * 1024 * 1024;
/// Messages a player may send an hour, and from one address.
pub const PER_PLAYER_AN_HOUR: usize = 20;
pub const PER_ADDRESS_AN_HOUR: usize = 40;
/// The most the files may take here in all: past it, a message keeps its text only.
const FILES_CAP: i64 = 1024 * 1024 * 1024;
/// A conversation quiet this long goes (with the daily cleanup).
pub const KEEP_FOR: i64 = 180 * 86_400;
/// Messages a conversation shows (the newest).
const SHOWN: i64 = 200;
pub const STATUSES: [&str; 3] = ["open", "waiting", "resolved"];

/// A player's message, as their launcher sends it.
#[derive(Debug, Deserialize)]
pub struct Sent {
    pub identity: String,
    pub name: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub launcher: String,
    pub time: i64,
    pub text: String,
    #[serde(default)]
    pub files: Vec<SentFile>,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
pub struct SentFile {
    pub name: String,
    pub size: u64,
    pub gzip_base64: String,
}

/// A file, checked: its name, size unpacked and the gzip as sent.
pub struct File {
    pub name: String,
    pub size: i64,
    pub gzip: Vec<u8>,
}

/// One line of at most `max` characters, nothing hidden in it.
fn one_line(text: &str, max: usize) -> bool {
    printable(text, max, false)
}

impl Sent {
    /// Checks it was sent to `host` by the identity named, now, and is within bounds. Answers
    /// the files, unpacked enough to know they're what they say.
    pub fn check(&self, host: &str, now: i64) -> Result<Vec<File>, &'static str> {
        if !identity::is_global_id(&self.identity) {
            return Err("not a valid request");
        }
        let text = self.text.trim();
        if text.is_empty() || !printable(text, MAX_TEXT, true) {
            return Err("a message is 1-2000 characters");
        }
        if self.name.trim().is_empty() || !one_line(&self.name, 32) || !one_line(&self.server, 253) || !one_line(&self.launcher, 32) {
            return Err("not a valid request");
        }
        if !identity::fresh(self.time, now) {
            return Err("this PC's clock is off; set it right and send again");
        }
        if self.files.len() > MAX_FILES {
            return Err("at most 8 files");
        }
        let mut files = Vec::new();
        let mut total = 0;
        for f in &self.files {
            if !crate::reports::valid_file_name(&f.name) || files.iter().any(|g: &File| g.name == f.name) {
                return Err("a file's name isn't valid");
            }
            let gzip = base64::engine::general_purpose::STANDARD.decode(&f.gzip_base64).map_err(|_| "a file isn't base64")?;
            total += gzip.len();
            if gzip.len() > MAX_FILE_GZIP || total > MAX_FILES_GZIP {
                return Err("the files are too large");
            }
            let unpacked = crate::reports::gunzip(&gzip, MAX_FILE_SIZE).map_err(|_| "a file isn't gzip within 16 MB")?;
            if unpacked.len() as u64 != f.size {
                return Err("a file isn't the size it says");
            }
            files.push(File {
                name: f.name.clone(),
                size: i64::try_from(f.size).unwrap_or(i64::MAX),
                gzip,
            });
        }
        let digests: Vec<(&str, [u8; 32])> = files.iter().map(|f| (f.name.as_str(), identity::digest(&f.gzip))).collect();
        if !identity::verify(&self.identity, &identity::support_message(host, self.time, &self.text, &digests), &self.signature) {
            return Err("the signature doesn't match");
        }
        Ok(files)
    }
}

impl Coordinator {
    /// Whether `identity` has an account on a member server: only a real player writes in.
    pub(crate) async fn is_linked_player(&self, identity: &str) -> sqlx::Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM links WHERE global_id = ?")
            .bind(identity)
            .fetch_one(&self.pool)
            .await?
            > 0)
    }

    /// Keeps a player's message (reopening their conversation). Answers its id, and whether
    /// its files were kept (not past the files' cap).
    pub(crate) async fn add_support_message(&self, s: &Sent, files: &[File], now: i64) -> sqlx::Result<(i64, bool)> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT INTO support_threads (identity, name, server, launcher, status, updated_at) VALUES (?1, ?2, ?3, ?4, 'open', ?5)
             ON CONFLICT (identity) DO UPDATE SET name = ?2, server = ?3, launcher = ?4, status = 'open', updated_at = ?5",
        )
        .bind(&s.identity)
        .bind(s.name.trim())
        .bind(s.server.trim())
        .bind(s.launcher.trim())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let id: i64 = sqlx::query_scalar("INSERT INTO support_messages (identity, at, admin, body) VALUES (?, ?, NULL, ?) RETURNING id")
            .bind(&s.identity)
            .bind(now)
            .bind(s.text.trim())
            .fetch_one(&mut *tx)
            .await?;
        let used: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(length(gzip)), 0) FROM support_files").fetch_one(&mut *tx).await?;
        let size: i64 = files.iter().map(|f| f.gzip.len() as i64).sum();
        let keep = used + size <= FILES_CAP;
        if keep {
            for f in files {
                sqlx::query("INSERT INTO support_files (message_id, name, size, gzip) VALUES (?, ?, ?, ?)")
                    .bind(id)
                    .bind(&f.name)
                    .bind(f.size)
                    .bind(&f.gzip)
                    .execute(&mut *tx)
                    .await?;
            }
        } else if !files.is_empty() {
            tracing::warn!("support: files take {} MB here; message {id} kept without its files", used >> 20);
        }
        tx.commit().await?;
        Ok((id, keep))
    }

    /// An admin's answer: the player sees it in their launcher (and is told in the overlay).
    /// An admin writing first (asking a player for something) opens the conversation: only
    /// with a player the network knows (an account with that identity on a member server),
    /// named as their newest account is.
    pub(crate) async fn answer_support(&self, identity: &str, admin: &str, text: &str, now: i64) -> sqlx::Result<Option<i64>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let account: Option<(String, String)> = sqlx::query_as("SELECT name, server_id FROM players WHERE identity = ? ORDER BY last_seen DESC LIMIT 1")
            .bind(identity)
            .fetch_optional(&mut *tx)
            .await?;
        if let Some((name, server)) = account {
            sqlx::query("INSERT INTO support_threads (identity, name, server, status, updated_at) VALUES (?, ?, ?, 'waiting', ?) ON CONFLICT (identity) DO NOTHING")
                .bind(identity)
                .bind(name)
                .bind(server)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
        let found = sqlx::query("UPDATE support_threads SET status = 'waiting', updated_at = ? WHERE identity = ?")
            .bind(now)
            .bind(identity)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if found == 0 {
            return Ok(None);
        }
        let id: i64 = sqlx::query_scalar("INSERT INTO support_messages (identity, at, admin, body) VALUES (?, ?, ?, ?) RETURNING id")
            .bind(identity)
            .bind(now)
            .bind(admin)
            .bind(text.trim())
            .fetch_one(&mut *tx)
            .await?;
        // Answering is reading what came before.
        sqlx::query("UPDATE support_threads SET admin_read = ? WHERE identity = ?")
            .bind(id)
            .bind(identity)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(id))
    }

    pub(crate) async fn set_support_status(&self, identity: &str, status: &str) -> sqlx::Result<bool> {
        Ok(sqlx::query("UPDATE support_threads SET status = ? WHERE identity = ?")
            .bind(status)
            .bind(identity)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    /// A conversation's messages, oldest first (the newest [`SHOWN`]), with their files' names
    /// and sizes.
    async fn support_messages(&self, identity: &str) -> sqlx::Result<Vec<Value>> {
        let rows: Vec<(i64, i64, Option<String>, String)> =
            sqlx::query_as("SELECT id, at, admin, body FROM (SELECT id, at, admin, body FROM support_messages WHERE identity = ? ORDER BY id DESC LIMIT ?) ORDER BY id")
                .bind(identity)
                .bind(SHOWN)
                .fetch_all(&self.pool)
                .await?;
        let files: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT f.message_id, f.name, f.size FROM support_files f JOIN support_messages m ON m.id = f.message_id WHERE m.identity = ? ORDER BY f.message_id, f.name",
        )
        .bind(identity)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, at, admin, body)| {
                let mine: Vec<Value> = files.iter().filter(|f| f.0 == id).map(|f| json!({ "name": f.1, "size": f.2 })).collect();
                json!({ "id": id, "at": at, "from": if admin.is_some() { "admin" } else { "player" }, "admin": admin, "text": body, "files": mine })
            })
            .collect())
    }

    /// The player's own conversation (their launcher), and with `read`, their having read it.
    pub(crate) async fn support_of(&self, identity: &str, read: bool) -> sqlx::Result<Value> {
        let thread: Option<(String, i64)> = sqlx::query_as("SELECT status, player_read FROM support_threads WHERE identity = ?")
            .bind(identity)
            .fetch_optional(&self.pool)
            .await?;
        let Some((status, player_read)) = thread else {
            return Ok(json!({ "status": null, "messages": [], "unread": 0 }));
        };
        let messages = self.support_messages(identity).await?;
        let last_admin = messages.iter().filter(|m| m["from"] == "admin").filter_map(|m| m["id"].as_i64()).max().unwrap_or(0);
        let unread = messages.iter().filter(|m| m["from"] == "admin" && m["id"].as_i64().unwrap_or(0) > player_read).count();
        if read && last_admin > player_read {
            sqlx::query("UPDATE support_threads SET player_read = ? WHERE identity = ?")
                .bind(last_admin)
                .bind(identity)
                .execute(&self.pool)
                .await?;
        }
        Ok(json!({ "status": status, "messages": messages, "unread": if read { 0 } else { unread } }))
    }

    /// The admins' list: every conversation, newest first, with what's unread for them.
    pub(crate) async fn support_list(&self, status: &str) -> sqlx::Result<Value> {
        let rows: Vec<(String, String, String, String, i64, i64, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT t.identity, t.name, t.server, t.status, t.updated_at,
                    (SELECT COUNT(*) FROM support_messages m WHERE m.identity = t.identity AND m.admin IS NULL AND m.id > t.admin_read),
                    (SELECT body FROM support_messages m WHERE m.identity = t.identity ORDER BY id DESC LIMIT 1),
                    (SELECT admin FROM support_messages m WHERE m.identity = t.identity ORDER BY id DESC LIMIT 1)
             FROM support_threads t WHERE (?1 = 'all' OR t.status = ?1 OR (?1 = 'active' AND t.status != 'resolved'))
             ORDER BY t.updated_at DESC LIMIT 500",
        )
        .bind(status)
        .fetch_all(&self.pool)
        .await?;
        let threads: Vec<Value> = rows
            .into_iter()
            .map(|(identity, name, server, status, updated_at, unread, last, last_admin)| {
                json!({ "identity": identity, "name": name, "server": server, "status": status, "updated_at": updated_at, "unread": unread,
                        "last": last.map(|t| t.chars().take(140).collect::<String>()), "last_from_admin": last_admin.is_some() })
            })
            .collect();
        Ok(json!({ "threads": threads }))
    }

    /// Conversations with player messages the admins haven't read (the rail's badge).
    pub(crate) async fn support_unread_threads(&self) -> sqlx::Result<i64> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM support_threads t WHERE EXISTS (SELECT 1 FROM support_messages m WHERE m.identity = t.identity AND m.admin IS NULL AND m.id > t.admin_read)",
        )
        .fetch_one(&self.pool)
        .await
    }

    /// One conversation for the admins (marking it read), with the player's accounts; and
    /// whether that read anything new (other admins' pages then update).
    pub(crate) async fn support_thread(&self, identity: &str) -> sqlx::Result<Option<(Value, bool)>> {
        let thread: Option<(String, String, String, String, i64)> = sqlx::query_as("SELECT name, server, launcher, status, updated_at FROM support_threads WHERE identity = ?")
            .bind(identity)
            .fetch_optional(&self.pool)
            .await?;
        let Some((name, server, launcher, status, updated_at)) = thread else { return Ok(None) };
        let messages = self.support_messages(identity).await?;
        let mut read_now = false;
        if let Some(last) = messages.iter().filter_map(|m| m["id"].as_i64()).max() {
            read_now = sqlx::query("UPDATE support_threads SET admin_read = ?1 WHERE identity = ?2 AND admin_read < ?1")
                .bind(last)
                .bind(identity)
                .execute(&self.pool)
                .await?
                .rows_affected()
                > 0;
        }
        let accounts: Vec<(String, i64, String)> = sqlx::query_as("SELECT server_id, id, name FROM players WHERE identity = ? ORDER BY server_id")
            .bind(identity)
            .fetch_all(&self.pool)
            .await?;
        let thread = json!({
            "identity": identity, "short": identity::short(identity), "name": name, "server": server, "launcher": launcher, "status": status, "updated_at": updated_at,
            "messages": messages,
            "accounts": accounts.into_iter().map(|(server, id, name)| json!({ "server": server, "id": id, "name": name })).collect::<Vec<_>>(),
        });
        Ok(Some((thread, read_now)))
    }

    /// A file of a message, unpacked (for the admins).
    pub(crate) async fn support_file(&self, message: i64, name: &str) -> sqlx::Result<Option<Vec<u8>>> {
        let gzip: Option<Vec<u8>> = sqlx::query_scalar("SELECT gzip FROM support_files WHERE message_id = ? AND name = ?")
            .bind(message)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(gzip.and_then(|g| crate::reports::gunzip(&g, MAX_FILE_SIZE).ok()))
    }

    /// For a server's pulse answer: those of its players online (`ids`, its account ids) with
    /// answers they haven't read, as `[{player, unread}]`.
    pub(crate) async fn support_unread_for(&self, server: &str, ids: &[i64]) -> sqlx::Result<Vec<Value>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let list = ids.iter().take(500).map(i64::to_string).collect::<Vec<_>>().join(",");
        let rows: Vec<(i64, i64)> = sqlx::query_as(&format!(
            "SELECT p.id, (SELECT COUNT(*) FROM support_messages m WHERE m.identity = t.identity AND m.admin IS NOT NULL AND m.id > t.player_read)
             FROM players p JOIN support_threads t ON t.identity = p.identity
             WHERE p.server_id = ? AND p.id IN ({list})"
        ))
        .bind(server)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(player, unread)| json!({ "player": player, "unread": unread }))
            .collect())
    }

    /// Deletes conversations quiet past [`KEEP_FOR`] (with the daily cleanup).
    pub(crate) async fn prune_support(&self) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM support_threads WHERE updated_at < ?")
            .bind(identity::now() - KEEP_FOR)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Posts a player's message to the alert webhook, unless report alerts are off: their
    /// name and the first line, never the files.
    pub(crate) async fn notify_support(&self, name: &str, first: &str) {
        if self.report_alert_mode().await.is_ok_and(|m| m == "off") {
            return;
        }
        let link = self.admin_origin().await.map(|o| format!("\n{}/#/support", o.trim_end_matches('/'))).unwrap_or_default();
        self.notify(&format!(
            "Support: {} wrote: {}{link}",
            crate::reports::chat_safe(name, 32),
            crate::reports::chat_safe(first, 200)
        ))
        .await;
    }
}

/// Deletes a player's conversation (bound to their identity) once no account of theirs is
/// left on any server.
pub(crate) const FORGET: &str = "DELETE FROM support_threads WHERE identity = ?1 AND NOT EXISTS (SELECT 1 FROM players WHERE identity = ?1)";
