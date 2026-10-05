//! Players' feedback and problem reports. After a session with problems (or now and then
//! after a normal one), the launcher asks the player how it went and offers to attach their
//! logs, redacted on their PC. It sends the report to the server they played on, which adds
//! its own log lines about that player and forwards it here:
//!
//! * `POST /v1/reports` (a member): one report, `{id, created_at, player, rating, problems,
//!   comment, triggers, client, summary, files, server_log}`, up to [`MAX_BODY`]. The same id
//!   again is the same report (answered `{ok: true}` and not stored twice). [`PER_HOUR`] new
//!   reports an hour per server.
//!
//! Reports are kept [`KEEP_FOR`]; their files on disk (`reports/<id>/<name>.gz` in the
//! coordinator's folder, gzip as received) up to [`STORAGE_CAP`] in all, the oldest reports'
//! files going first. Admins read, resolve and delete them in the admin UI, and new ones are
//! posted to the alert webhook (see [`Batcher`]).

use std::collections::VecDeque;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::json;
use serde_json::Value;
use sqlx::Row as _;

use crate::Coordinator;

/// The largest report request.
pub const MAX_BODY: usize = 8 * 1024 * 1024;
/// Files a report may carry, and each one's size uncompressed.
pub const MAX_FILES: usize = 8;
pub const MAX_FILE_SIZE: usize = 4 * 1024 * 1024;
const MAX_COMMENT: usize = 2000;
const MAX_SUMMARY: usize = 64 * 1024;
const MAX_SERVER_LOG: usize = 1024 * 1024;
const MAX_TRIGGERS: usize = 16;
const MAX_TRIGGER: usize = 40;
const MAX_CLIENT_FIELDS: usize = 16;
const MAX_CLIENT_FIELD: usize = 64;
/// The longest note an admin may leave on a report.
pub const MAX_NOTE: usize = 500;
/// Reports (and their files) are kept this long.
pub const KEEP_FOR: i64 = 90 * 86_400;
/// The most the reports' files may take on disk (as stored, compressed).
pub const STORAGE_CAP: u64 = 2 * 1024 * 1024 * 1024;
/// New reports one server may send in an hour.
pub const PER_HOUR: usize = 30;
/// Rows on a page of the report list.
pub const PAGE: i64 = 50;
/// What a player can tick.
pub const PROBLEMS: [&str; 7] = ["join", "lag", "crash", "connection", "version", "signin", "other"];
/// The setting for posting reports to the alert webhook: off, problems (bad-rated or with
/// problems ticked; the default) or all.
pub const ALERTS_SETTING: &str = "report_alerts";
pub const ALERT_MODES: [&str; 3] = ["off", "problems", "all"];
/// More than this many reports posted within [`BATCH_WINDOW`] seconds, and the rest wait
/// to go in one message.
const BATCH_AFTER: usize = 5;
const BATCH_WINDOW: i64 = 60;

/// A file of a report, checked: its name, the gzip as sent, and its size uncompressed.
#[derive(Debug)]
pub(crate) struct File {
    pub name: String,
    pub gzip: Vec<u8>,
    pub size: u64,
}

/// A report as a server sent it, checked.
#[derive(Debug)]
pub(crate) struct Report {
    pub id: String,
    pub created_at: i64,
    pub player_id: i64,
    pub player_name: String,
    pub identity: Option<String>,
    pub rating: Option<String>,
    pub problems: Vec<String>,
    pub comment: String,
    pub triggers: Vec<String>,
    pub client: serde_json::Map<String, Value>,
    pub summary: Value,
    pub files: Vec<File>,
    pub server_log: String,
}

/// Whether `id` is spelled like a report id: 32 hex digits.
pub(crate) fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `name` may be a report file's name.
pub(crate) fn valid_file_name(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) && name != "." && name != ".."
}

/// Printable text of at most `max` characters (line breaks and tabs only when `lines`).
fn printable(text: &str, max: usize, lines: bool) -> bool {
    text.chars().count() <= max
        && !text
            .chars()
            .any(|c| (c.is_control() && !(lines && matches!(c, '\n' | '\r' | '\t'))) || crate::hidden_char(c))
}

/// `text` with the characters that hide in text, and control characters other than line
/// breaks and tabs, taken out (a log may carry colour codes).
fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| !crate::hidden_char(*c) && (!c.is_control() || matches!(c, '\n' | '\r' | '\t')))
        .collect()
}

/// The text of a gzip file, at most `max` bytes of it uncompressed.
pub(crate) fn gunzip(data: &[u8], max: usize) -> Result<Vec<u8>, &'static str> {
    // A gzip header and trailer at least (an empty input would read as an empty file).
    if data.len() < 18 || data[..2] != [0x1f, 0x8b] {
        return Err("a file isn't gzip");
    }
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(max as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| "a file isn't valid gzip")?;
    if out.len() > max {
        return Err("a file is over 4 MB uncompressed");
    }
    Ok(out)
}

/// Checks a report a server sent: everything in its place, sizes within bounds, each file's
/// gzip decompressed to its stated size. The reason is short, for the server's log.
pub(crate) fn check(v: &Value, now: i64) -> Result<Report, String> {
    let bad = |why: &str| Err(why.to_string());
    if !v.is_object() {
        return bad("not a report");
    }
    let Some(id) = v["id"].as_str().filter(|id| valid_id(id)) else {
        return bad("id is 32 hex digits");
    };
    let Some(created_at) = v["created_at"].as_i64().filter(|t| (now - KEEP_FOR..=now + 86_400).contains(t)) else {
        return bad("created_at is a time in the last 90 days");
    };
    let player = &v["player"];
    let Some(player_id) = player["id"].as_i64().filter(|id| (0..=i64::from(u32::MAX)).contains(id)) else {
        return bad("player.id is an account number");
    };
    let Some(player_name) = player["name"].as_str().map(str::trim).filter(|n| !n.is_empty() && printable(n, 64, false)) else {
        return bad("player.name is 1-64 printable characters");
    };
    let identity = match &player["identity"] {
        Value::Null => None,
        Value::String(s) if (1..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric()) => Some(s.clone()),
        _ => return bad("player.identity is null or 1-128 letters and digits"),
    };
    let rating = match &v["rating"] {
        Value::Null => None,
        Value::String(s) if s == "good" || s == "bad" => Some(s.clone()),
        _ => return bad("rating is good, bad or null"),
    };
    let mut problems: Vec<String> = Vec::new();
    match &v["problems"] {
        Value::Null => {}
        Value::Array(list) if list.len() <= PROBLEMS.len() * 2 => {
            for p in list {
                match p.as_str().filter(|p| PROBLEMS.contains(p)) {
                    Some(p) if !problems.iter().any(|q| q == p) => problems.push(p.to_string()),
                    Some(_) => {}
                    None => return bad("problems are join, lag, crash, connection, version, signin or other"),
                }
            }
        }
        _ => return bad("problems is a list"),
    }
    let comment = match &v["comment"] {
        Value::Null => String::new(),
        Value::String(s) if s.chars().count() <= MAX_COMMENT => plain(s).trim().to_string(),
        _ => return bad("comment is at most 2000 characters"),
    };
    let mut triggers = Vec::new();
    match &v["triggers"] {
        Value::Null => {}
        Value::Array(list) if list.len() <= MAX_TRIGGERS => {
            for t in list {
                match t.as_str().filter(|t| !t.is_empty() && printable(t, MAX_TRIGGER, false)) {
                    Some(t) => triggers.push(t.to_string()),
                    None => return bad("triggers are 1-40 printable characters each"),
                }
            }
        }
        _ => return bad("triggers is a list of at most 16"),
    }
    let mut client = serde_json::Map::new();
    match &v["client"] {
        Value::Null => {}
        Value::Object(fields) if fields.len() <= MAX_CLIENT_FIELDS => {
            for (k, val) in fields {
                if !(1..=32).contains(&k.len()) || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                    return bad("client's fields are named with up to 32 letters, digits and _");
                }
                match val {
                    Value::Null => {}
                    Value::String(s) if printable(s, MAX_CLIENT_FIELD, false) => {
                        client.insert(k.clone(), json!(s));
                    }
                    _ => return bad("client's fields are strings of at most 64 printable characters"),
                }
            }
        }
        _ => return bad("client is an object of at most 16 strings"),
    }
    let summary = match &v["summary"] {
        Value::Null => json!({}),
        s @ Value::Object(_) if s.to_string().len() <= MAX_SUMMARY => s.clone(),
        Value::Object(_) => return bad("summary is at most 64 KB"),
        _ => return bad("summary is an object"),
    };
    let server_log = match &v["server_log"] {
        Value::Null => String::new(),
        Value::String(s) if s.len() <= MAX_SERVER_LOG => plain(s),
        Value::String(_) => return bad("server_log is at most 1 MB"),
        _ => return bad("server_log is text"),
    };
    let mut files: Vec<File> = Vec::new();
    match &v["files"] {
        Value::Null => {}
        Value::Array(list) if list.len() <= MAX_FILES => {
            for f in list {
                let Some(name) = f["name"].as_str().filter(|n| valid_file_name(n)) else {
                    return bad("a file's name is 1-64 letters, digits, . _ and -");
                };
                if files.iter().any(|g| g.name == name) {
                    return bad("two files have the same name");
                }
                let Some(size) = f["size"].as_u64().filter(|s| *s <= MAX_FILE_SIZE as u64) else {
                    return bad("a file's size is at most 4 MB");
                };
                let Some(gzip) = f["gzip_base64"].as_str().and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()) else {
                    return bad("a file's gzip_base64 isn't base64");
                };
                let text = gunzip(&gzip, MAX_FILE_SIZE)?;
                if text.len() as u64 != size {
                    return bad("a file's size isn't what it decompresses to");
                }
                files.push(File {
                    name: name.to_string(),
                    gzip,
                    size,
                });
            }
        }
        _ => return bad("files is a list of at most 8"),
    }
    Ok(Report {
        id: id.to_ascii_lowercase(),
        created_at,
        player_id,
        player_name: player_name.to_string(),
        identity,
        rating,
        problems,
        comment,
        triggers,
        client,
        summary,
        files,
        server_log,
    })
}

/// A report's list columns.
const COLUMNS: &str = "r.id, r.server_id, r.created_at, r.received_at, r.player_id, r.player_name, r.player_identity, r.rating, r.problems,
       r.triggers, r.client, r.comment, r.status, r.note, r.resolved_by, r.resolved_at,
       (SELECT json_group_array(json_object('name', f.name, 'size', f.size, 'dropped', json(CASE WHEN f.dropped = 1 THEN 'true' ELSE 'false' END))) FROM player_report_files f WHERE f.report_id = r.id) AS files";

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// A report's row, as the list shows it (the comment's first `comment_max` characters).
fn row_json(r: &sqlx::sqlite::SqliteRow, comment_max: usize) -> Value {
    let comment: String = r.get("comment");
    let files: Option<String> = r.get("files");
    json!({
        "id": r.get::<String, _>("id"),
        "server": r.get::<String, _>("server_id"),
        "player": { "id": r.get::<i64, _>("player_id"), "name": r.get::<String, _>("player_name"), "identity": r.get::<Option<String>, _>("player_identity") },
        "created_at": r.get::<i64, _>("created_at"),
        "received_at": r.get::<i64, _>("received_at"),
        "rating": r.get::<Option<String>, _>("rating"),
        "problems": parsed(&r.get::<String, _>("problems")),
        "triggers": parsed(&r.get::<String, _>("triggers")),
        "client": parsed(&r.get::<String, _>("client")),
        "comment": comment.chars().take(comment_max).collect::<String>(),
        "comment_cut": comment.chars().count() > comment_max,
        "files": files.map_or(json!([]), |f| parsed(&f)),
        "status": r.get::<String, _>("status"),
        "note": r.get::<String, _>("note"),
        "resolved_by": r.get::<Option<String>, _>("resolved_by"),
        "resolved_at": r.get::<Option<i64>, _>("resolved_at"),
    })
}

/// A report list query: status (open, resolved or all), server, problem, search and page.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub problem: String,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub page: i64,
}

impl Coordinator {
    /// Where a report's files are.
    fn report_dir(&self, id: &str) -> PathBuf {
        self.files_dir.join("reports").join(id)
    }

    /// Whether a report with this id is here already.
    pub(crate) async fn report_exists(&self, id: &str) -> sqlx::Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM player_reports WHERE id = ?")
            .bind(id.to_ascii_lowercase())
            .fetch_one(&self.pool)
            .await?
            > 0)
    }

    /// Stores a checked report from `server`: the report, then its files on disk. Answers
    /// whether it's new (the same id twice is one report).
    pub(crate) async fn store_report(&self, server: &str, r: &Report) -> Result<bool, String> {
        let now = identity::now();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(|e| e.to_string())?;
        let added = sqlx::query(
            "INSERT OR IGNORE INTO player_reports (id, server_id, created_at, received_at, player_id, player_name, player_identity, rating,
                                                   problems, triggers, client, summary, comment, server_log)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&r.id)
        .bind(server)
        .bind(r.created_at)
        .bind(now)
        .bind(r.player_id)
        .bind(&r.player_name)
        .bind(&r.identity)
        .bind(&r.rating)
        .bind(json!(r.problems).to_string())
        .bind(json!(r.triggers).to_string())
        .bind(Value::Object(r.client.clone()).to_string())
        .bind(r.summary.to_string())
        .bind(&r.comment)
        .bind(&r.server_log)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if added.rows_affected() == 0 {
            return Ok(false);
        }
        for f in &r.files {
            sqlx::query("INSERT INTO player_report_files (report_id, name, size, stored) VALUES (?, ?, ?, ?)")
                .bind(&r.id)
                .bind(&f.name)
                .bind(f.size as i64)
                .bind(f.gzip.len() as i64)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        }
        // The files before the report counts as stored: a report is never there without them.
        if !r.files.is_empty() {
            let dir = self.report_dir(&r.id);
            let written = async {
                tokio::fs::create_dir_all(&dir).await?;
                for f in &r.files {
                    tokio::fs::write(dir.join(format!("{}.gz", f.name)), &f.gzip).await?;
                }
                std::io::Result::Ok(())
            }
            .await;
            if let Err(e) = written {
                let _ = tokio::fs::remove_dir_all(&dir).await;
                return Err(format!("couldn't store a report's files in {}: {e}", dir.display()));
            }
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        self.cap_report_storage().await.map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// Drops the oldest reports' files while they take more than the cap (the reports stay).
    pub(crate) async fn cap_report_storage(&self) -> sqlx::Result<()> {
        let cap = self.report_storage_cap.load(std::sync::atomic::Ordering::Relaxed);
        let used: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(stored), 0) FROM player_report_files WHERE dropped = 0")
            .fetch_one(&self.pool)
            .await?;
        let mut used = u64::try_from(used).unwrap_or(0);
        if used <= cap {
            return Ok(());
        }
        let oldest: Vec<(String, i64)> = sqlx::query_as(
            "SELECT f.report_id, SUM(f.stored) FROM player_report_files f JOIN player_reports r ON r.id = f.report_id
              WHERE f.dropped = 0 GROUP BY f.report_id ORDER BY MIN(r.received_at), f.report_id",
        )
        .fetch_all(&self.pool)
        .await?;
        for (id, stored) in oldest {
            if used <= cap {
                break;
            }
            let _ = tokio::fs::remove_dir_all(self.report_dir(&id)).await;
            sqlx::query("UPDATE player_report_files SET dropped = 1 WHERE report_id = ?")
                .bind(&id)
                .execute(&self.pool)
                .await?;
            used = used.saturating_sub(u64::try_from(stored).unwrap_or(0));
            tracing::info!("report {id}: its files went to keep the reports under the storage cap");
        }
        Ok(())
    }

    /// Removes reports past keeping, with their files. Run with the hourly rollup.
    pub(crate) async fn prune_reports(&self) -> sqlx::Result<()> {
        let old: Vec<String> = sqlx::query_scalar("DELETE FROM player_reports WHERE received_at < ? RETURNING id")
            .bind(identity::now() - KEEP_FOR)
            .fetch_all(&self.pool)
            .await?;
        for id in old {
            let _ = tokio::fs::remove_dir_all(self.report_dir(&id)).await;
        }
        self.cap_report_storage().await
    }

    /// Open reports, for the nav's badge.
    pub(crate) async fn open_reports(&self) -> sqlx::Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM player_reports WHERE status = 'open'").fetch_one(&self.pool).await
    }

    /// A page of reports, newest first.
    pub async fn report_list(&self, q: &ListQuery) -> sqlx::Result<Value> {
        let status = match q.status.as_str() {
            "resolved" => "resolved",
            "all" => "",
            _ => "open",
        };
        let search = q.q.trim();
        let like = format!("%{}%", search.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let filter = "FROM player_reports r
             WHERE (?1 = '' OR r.status = ?1)
               AND (?2 = '' OR r.server_id = ?2)
               AND (?3 = '' OR EXISTS (SELECT 1 FROM json_each(r.problems) WHERE json_each.value = ?3))
               AND (?4 = '' OR r.player_name LIKE ?5 ESCAPE '\\' OR r.comment LIKE ?5 ESCAPE '\\' OR r.id = lower(?4)
                    OR r.player_identity = ?4 OR CAST(r.player_id AS TEXT) = ?4)";
        let page = q.page.clamp(0, 100_000);
        let rows = sqlx::query(&format!("SELECT {COLUMNS} {filter} ORDER BY r.created_at DESC, r.id LIMIT ?6 OFFSET ?7"))
            .bind(status)
            .bind(q.server.trim())
            .bind(q.problem.trim())
            .bind(search)
            .bind(&like)
            .bind(PAGE)
            .bind(page * PAGE)
            .fetch_all(&self.pool)
            .await?;
        let total: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) {filter}"))
            .bind(status)
            .bind(q.server.trim())
            .bind(q.problem.trim())
            .bind(search)
            .bind(&like)
            .fetch_one(&self.pool)
            .await?;
        Ok(json!({
            "reports": rows.iter().map(|r| row_json(r, 200)).collect::<Vec<_>>(),
            "total": total,
            "page": page,
            "per_page": PAGE,
        }))
    }

    /// One report: everything, with the player's other reports (the same account, or the same
    /// identity), newest first.
    pub async fn report_detail(&self, id: &str) -> sqlx::Result<Option<Value>> {
        let Some(row) = sqlx::query(&format!("SELECT {COLUMNS}, r.summary, r.server_log FROM player_reports r WHERE r.id = ?1"))
            .bind(id.to_ascii_lowercase())
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        let mut report = row_json(&row, usize::MAX);
        report["summary"] = parsed(&row.get::<String, _>("summary"));
        report["server_log"] = json!(row.get::<String, _>("server_log"));
        let (server, player) = (row.get::<String, _>("server_id"), row.get::<i64, _>("player_id"));
        let identity: Option<String> = row.get("player_identity");
        let others = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM player_reports r
              WHERE r.id != ?1 AND ((r.server_id = ?2 AND r.player_id = ?3) OR (?4 IS NOT NULL AND r.player_identity = ?4))
              ORDER BY r.created_at DESC LIMIT 50"
        ))
        .bind(report["id"].as_str())
        .bind(&server)
        .bind(player)
        .bind(&identity)
        .fetch_all(&self.pool)
        .await?;
        let known: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM players WHERE server_id = ? AND id = ?")
            .bind(&server)
            .bind(player)
            .fetch_one(&self.pool)
            .await?;
        report["others"] = json!(others.iter().map(|r| row_json(r, 200)).collect::<Vec<_>>());
        report["player_known"] = json!(known > 0);
        Ok(Some(report))
    }

    /// A report's file, decompressed. None: no such report or file; Some(None): its files went
    /// (the storage cap).
    pub async fn report_file(&self, id: &str, name: &str) -> Result<Option<Option<Vec<u8>>>, String> {
        let id = id.to_ascii_lowercase();
        if !valid_id(&id) || !valid_file_name(name) {
            return Ok(None);
        }
        let dropped: Option<bool> = sqlx::query_scalar("SELECT dropped = 1 FROM player_report_files WHERE report_id = ? AND name = ?")
            .bind(&id)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        match dropped {
            None => Ok(None),
            Some(true) => Ok(Some(None)),
            Some(false) => {
                let gzip = match tokio::fs::read(self.report_dir(&id).join(format!("{name}.gz"))).await {
                    Ok(g) => g,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(None)),
                    Err(e) => return Err(e.to_string()),
                };
                gunzip(&gzip, MAX_FILE_SIZE).map(|text| Some(Some(text))).map_err(str::to_string)
            }
        }
    }

    /// Opens or resolves a report (resolving notes who, unless it was resolved already), with
    /// a new note if given. Answers its status before, and its row; None for no such report.
    pub async fn set_report_status(&self, id: &str, status: &str, note: Option<&str>, by: &str) -> sqlx::Result<Option<(String, Value)>> {
        let id = id.to_ascii_lowercase();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let before: Option<String> = sqlx::query_scalar("SELECT status FROM player_reports WHERE id = ?")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(before) = before else { return Ok(None) };
        sqlx::query(
            "UPDATE player_reports SET note = COALESCE(?2, note),
                    resolved_by = CASE WHEN ?1 = 'resolved' THEN (CASE WHEN status = 'resolved' THEN resolved_by ELSE ?3 END) END,
                    resolved_at = CASE WHEN ?1 = 'resolved' THEN (CASE WHEN status = 'resolved' THEN resolved_at ELSE ?4 END) END,
                    status = ?1
              WHERE id = ?5",
        )
        .bind(status)
        .bind(note)
        .bind(by)
        .bind(identity::now())
        .bind(&id)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(&format!("SELECT {COLUMNS} FROM player_reports r WHERE r.id = ?1"))
            .bind(&id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some((before, row_json(&row, 200))))
    }

    /// Deletes a report and its files. Answers who it was from, or None for no such report.
    pub async fn delete_report(&self, id: &str) -> sqlx::Result<Option<(String, String)>> {
        let id = id.to_ascii_lowercase();
        let gone: Option<(String, String)> = sqlx::query_as("DELETE FROM player_reports WHERE id = ? RETURNING player_name, server_id")
            .bind(&id)
            .fetch_optional(&self.pool)
            .await?;
        if gone.is_some() && valid_id(&id) {
            let _ = tokio::fs::remove_dir_all(self.report_dir(&id)).await;
        }
        Ok(gone)
    }

    /// How new reports are posted to the webhook: off, problems or all.
    pub(crate) async fn report_alert_mode(&self) -> sqlx::Result<String> {
        Ok(self
            .setting(ALERTS_SETTING)
            .await?
            .filter(|m| ALERT_MODES.contains(&m.as_str()))
            .unwrap_or_else(|| "problems".into()))
    }

    /// Posts a new report to the alert webhook, as the setting says: now, or (past
    /// [`BATCH_AFTER`] in a minute) with the others in one message a minute later.
    pub(crate) async fn alert_report(self: &Arc<Self>, server: &str, r: &Report) {
        let mode = self.report_alert_mode().await.unwrap_or_default();
        if !wanted(&mode, r.rating.as_deref(), &r.problems) {
            return;
        }
        let listing: Option<Option<String>> = sqlx::query_scalar("SELECT listing FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(&self.pool)
            .await
            .unwrap_or(None);
        let server_name = listing
            .flatten()
            .and_then(|l| serde_json::from_str::<Value>(&l).ok())
            .and_then(|l| l["name"].as_str().map(str::to_string))
            .unwrap_or_else(|| server.to_string());
        let item = Alerted {
            id: r.id.clone(),
            player: r.player_name.clone(),
            server: server_name,
            rating: r.rating.clone(),
            problems: r.problems.clone(),
            comment: r.comment.clone(),
        };
        let decision = self.report_batch.lock().unwrap_or_else(std::sync::PoisonError::into_inner).add(identity::now(), item);
        let origin = self.admin_origin().await;
        match decision {
            Decision::Now(item) => self.notify(&report_message(&item, origin.as_deref())).await,
            Decision::Held { flush_later: false } => {}
            Decision::Held { flush_later: true } => {
                let c = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(BATCH_WINDOW as u64)).await;
                    let held = c.report_batch.lock().unwrap_or_else(std::sync::PoisonError::into_inner).flush(identity::now());
                    if !held.is_empty() {
                        c.notify(&batch_message(&held, origin.as_deref())).await;
                    }
                });
            }
        }
    }

    /// Where admins open the admin UI, if known.
    async fn admin_origin(&self) -> Option<String> {
        match self.admin.get() {
            Some(c) => Some(c.origin.clone()),
            None => self.setting("admin_origin").await.ok().flatten(),
        }
    }
}

/// Whether a report is posted, by the setting: never (off), when rated bad or with problems
/// ticked (problems), or always (all).
pub(crate) fn wanted(mode: &str, rating: Option<&str>, problems: &[String]) -> bool {
    match mode {
        "off" => false,
        "all" => true,
        _ => rating == Some("bad") || !problems.is_empty(),
    }
}

/// What a report's alert says about it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Alerted {
    pub id: String,
    pub player: String,
    pub server: String,
    pub rating: Option<String>,
    pub problems: Vec<String>,
    pub comment: String,
}

fn problem_label(p: &str) -> &str {
    match p {
        "join" => "couldn't join",
        "lag" => "lag",
        "crash" => "crash",
        "connection" => "connection problems",
        "version" => "a different game version",
        "signin" => "couldn't sign in",
        "other" => "other problems",
        other => other,
    }
}

/// Text from a player, safe in a chat message: one line, no mentions (`@everyone` pings
/// nobody), at most `max` characters.
fn chat_safe(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = one_line.chars().take(max).collect::<String>().replace('@', "@\u{200b}").replace('`', "'");
    if one_line.chars().count() > max {
        out.push('…');
    }
    out
}

fn what(a: &Alerted) -> String {
    if !a.problems.is_empty() {
        a.problems.iter().map(|p| problem_label(p)).collect::<Vec<_>>().join(", ")
    } else {
        match a.rating.as_deref() {
            Some("bad") => "rated the session bad".into(),
            Some("good") => "rated the session good".into(),
            _ => "feedback".into(),
        }
    }
}

/// One report's message, e.g. `📝 Report from Kiwi on eu1: couldn't join, lag — "couldn't
/// join my friend"`, with a link to it when the admin UI's address is known.
pub(crate) fn report_message(a: &Alerted, origin: Option<&str>) -> String {
    let comment = if a.comment.is_empty() {
        String::new()
    } else {
        format!(" — \"{}\"", chat_safe(&a.comment, 200))
    };
    let link = origin.map(|o| format!("\n{o}/#/reports/{}", a.id)).unwrap_or_default();
    format!("📝 Report from {} on {}: {}{comment}{link}", chat_safe(&a.player, 64), chat_safe(&a.server, 64), what(a))
}

/// Several reports in one message: `📝 7 new reports: …`, the first few named.
pub(crate) fn batch_message(held: &[Alerted], origin: Option<&str>) -> String {
    const NAMED: usize = 5;
    let mut lines: Vec<String> = held
        .iter()
        .take(NAMED)
        .map(|a| format!("• {} on {}: {}", chat_safe(&a.player, 64), chat_safe(&a.server, 64), what(a)))
        .collect();
    if held.len() > NAMED {
        lines.push(format!("… and {} more", held.len() - NAMED));
    }
    let link = origin.map(|o| format!("\n{o}/#/reports")).unwrap_or_default();
    format!("📝 {} new reports\n{}{link}", held.len(), lines.join("\n"))
}

/// What to do with a new report's alert.
#[derive(Debug, PartialEq)]
pub(crate) enum Decision {
    /// Post it now.
    Now(Alerted),
    /// Held for a batch; the first one held schedules the batch's message.
    Held { flush_later: bool },
}

/// Keeps report alerts from flooding the chat: up to [`BATCH_AFTER`] are posted one by one
/// within [`BATCH_WINDOW`] seconds; past that they're held, and posted together a minute later.
#[derive(Debug, Default)]
pub(crate) struct Batcher {
    sent: VecDeque<i64>,
    held: Vec<Alerted>,
}

impl Batcher {
    pub(crate) fn add(&mut self, now: i64, item: Alerted) -> Decision {
        while self.sent.front().is_some_and(|t| now - t >= BATCH_WINDOW) {
            self.sent.pop_front();
        }
        if !self.held.is_empty() {
            self.held.push(item);
            return Decision::Held { flush_later: false };
        }
        if self.sent.len() < BATCH_AFTER {
            self.sent.push_back(now);
            return Decision::Now(item);
        }
        self.held.push(item);
        Decision::Held { flush_later: true }
    }

    /// The held reports, to post in one message (which counts as one sent).
    pub(crate) fn flush(&mut self, now: i64) -> Vec<Alerted> {
        let held = std::mem::take(&mut self.held);
        if !held.is_empty() {
            self.sent.push_back(now);
        }
        held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alerted(player: &str, problems: &[&str], comment: &str) -> Alerted {
        Alerted {
            id: "0123456789abcdef0123456789abcdef".into(),
            player: player.into(),
            server: "eu1".into(),
            rating: Some("bad".into()),
            problems: problems.iter().map(|p| p.to_string()).collect(),
            comment: comment.into(),
        }
    }

    #[test]
    fn report_messages_read_well() {
        let a = alerted("ijsman5530", &["join", "lag"], "couldn't join my friend");
        assert_eq!(
            report_message(&a, Some("https://admin.example")),
            "📝 Report from ijsman5530 on eu1: couldn't join, lag — \"couldn't join my friend\"\nhttps://admin.example/#/reports/0123456789abcdef0123456789abcdef"
        );
        // No problems ticked, no comment, no admin address: what the rating says, no link.
        let mut b = alerted("Kiwi", &[], "");
        assert_eq!(report_message(&b, None), "📝 Report from Kiwi on eu1: rated the session bad");
        b.rating = Some("good".into());
        assert_eq!(report_message(&b, None), "📝 Report from Kiwi on eu1: rated the session good");
        // A player's words can't ping the channel, break lines, or run on.
        let c = alerted("Kiwi", &["crash"], &format!("@everyone look\n\n{}", "x".repeat(300)));
        let m = report_message(&c, None);
        assert!(!m.contains("@everyone") && m.contains("@\u{200b}everyone look x"), "{m}");
        assert!(m.ends_with("…\"") && m.lines().count() == 1, "{m}");
    }

    #[test]
    fn batches_name_the_first_few() {
        let held: Vec<Alerted> = (0..7).map(|i| alerted(&format!("p{i}"), &["crash"], "")).collect();
        let m = batch_message(&held, Some("https://admin.example"));
        assert!(m.starts_with("📝 7 new reports\n• p0 on eu1: crash\n"), "{m}");
        assert!(m.contains("• p4 on eu1: crash\n… and 2 more\nhttps://admin.example/#/reports"), "{m}");
        assert!(!m.contains("p5"));
    }

    #[test]
    fn past_five_a_minute_reports_wait_for_one_message() {
        let mut b = Batcher::default();
        let item = |i: usize| alerted(&format!("p{i}"), &["lag"], "");
        for i in 0..5 {
            assert_eq!(b.add(1000 + i as i64, item(i)), Decision::Now(item(i)));
        }
        assert_eq!(b.add(1010, item(5)), Decision::Held { flush_later: true });
        assert_eq!(b.add(1011, item(6)), Decision::Held { flush_later: false });
        // Even once the minute's over, while some wait, a new one joins them.
        assert_eq!(b.add(1100, item(7)), Decision::Held { flush_later: false });
        let held = b.flush(1110);
        assert_eq!(held.iter().map(|a| a.player.as_str()).collect::<Vec<_>>(), ["p5", "p6", "p7"]);
        assert!(b.flush(1111).is_empty());
        // After the batch: one by one again (the batch counts as one message).
        assert!(matches!(b.add(1120, item(8)), Decision::Now(_)));
    }

    #[test]
    fn the_setting_says_which_reports_are_posted() {
        let none: Vec<String> = vec![];
        let lag = vec!["lag".to_string()];
        assert!(wanted("problems", Some("bad"), &none));
        assert!(wanted("problems", None, &lag));
        assert!(!wanted("problems", Some("good"), &none));
        assert!(!wanted("problems", None, &none));
        assert!(wanted("all", Some("good"), &none));
        assert!(!wanted("off", Some("bad"), &lag));
    }

    #[test]
    fn file_names_are_plain() {
        for good in ["bl-tracing.log", "a", "x_1.TXT", &"n".repeat(64)] {
            assert!(valid_file_name(good), "{good}");
        }
        for bad in ["", ".", "..", "a/b", "a\\b", "a b", &"n".repeat(65), "ünï.log"] {
            assert!(!valid_file_name(bad), "{bad}");
        }
    }
}
