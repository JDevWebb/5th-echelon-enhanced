//! Match recordings brought here for the admin UI's replay viewer (the replay research, step
//! 2). Players' games send them to their own server, which keeps them a week; an admin asks
//! for one (`send_recording`, an action through the server's pulse), and the server posts it
//! (`POST /v1/recordings/<id>`). Only one asked for in the last [`ASKED_WITHIN`] is taken, and
//! only once it reads as a recording. Kept as files in the data folder's `recordings`
//! (never in the database, so the backups stay small), for [`KEEP_FOR`] and at most
//! [`MOST_KEPT_BYTES`] in all; the browser unpacks them itself.

use std::path::PathBuf;

use serde_json::json;

use crate::Coordinator;

/// The most a recording may be, as sent (the servers' own limit).
pub const MAX_BODY: usize = 8 * 1024 * 1024;
const ASKED_WITHIN: i64 = 2 * 3600;
const KEEP_FOR: i64 = 14 * 86_400;
const MOST_KEPT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// A recording's id as servers make them: 16 hex characters.
pub fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Why a posted recording wasn't taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    NotAsked,
    NotARecording(String),
    NoFolder,
}

impl Coordinator {
    fn recordings_dir(&self) -> Option<PathBuf> {
        Some(self.data_dir.get()?.join("recordings"))
    }

    /// Where `server`'s recording `id` is kept here (server ids are the coordinator's own:
    /// letters, digits and dashes; anything else has none).
    pub(crate) fn recording_path(&self, server: &str, id: &str) -> Option<PathBuf> {
        let safe = !server.is_empty() && server.len() <= 64 && server.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        (safe && valid_id(id))
            .then(|| self.recordings_dir())
            .flatten()
            .map(|d| d.join(format!("{server}-{id}.{}", replay::EXTENSION)))
    }

    /// Takes `server`'s recording `id` if an admin asked that server for it lately.
    pub(crate) async fn take_recording(&self, server: &str, id: &str, data: Vec<u8>) -> sqlx::Result<Result<(), Refused>> {
        if !valid_id(id) {
            return Ok(Err(Refused::NotAsked));
        }
        let Some(path) = self.recording_path(server, id) else {
            return Ok(Err(Refused::NoFolder));
        };
        let asked: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM player_actions WHERE server_id = ? AND kind = 'send_recording' AND json_extract(args, '$.recording') = ? AND created_at >= ?",
        )
        .bind(server)
        .bind(id)
        .bind(identity::now() - ASKED_WITHIN)
        .fetch_one(&self.pool)
        .await?;
        if !asked {
            return Ok(Err(Refused::NotAsked));
        }
        let checked = tokio::task::spawn_blocking(move || {
            replay::read(&data[..]).map_err(|e| e.to_string())?;
            let dir = path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(&path, &data).map_err(|e| e.to_string())?;
            prune(&dir, std::time::SystemTime::now());
            Ok::<(), String>(())
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        Ok(checked.map_err(Refused::NotARecording))
    }

    /// Asks `server` for `player`'s recording `id` (its id for the admin to follow).
    pub(crate) async fn ask_for_recording(&self, server: &str, player: i64, id: &str, by: &str) -> sqlx::Result<i64> {
        self.queue_action(server, player, "send_recording", &json!({ "recording": id }), by).await
    }

    /// Whether `server` told of `player`'s game sending recording `id` (its `recording` event).
    pub(crate) async fn recording_known(&self, server: &str, player: i64, id: &str) -> sqlx::Result<bool> {
        sqlx::query_scalar("SELECT COUNT(*) > 0 FROM session_events WHERE server_id = ? AND player = ? AND kind = 'recording' AND json_extract(detail, '$.id') = ?")
            .bind(server)
            .bind(player)
            .bind(id)
            .fetch_one(&self.pool)
            .await
    }

    /// How the latest ask for `server`'s recording `id` went: its status and message.
    pub(crate) async fn recording_asked(&self, server: &str, id: &str) -> sqlx::Result<Option<(String, Option<String>)>> {
        sqlx::query_as(
            "SELECT status, message FROM player_actions WHERE server_id = ? AND kind = 'send_recording' AND json_extract(args, '$.recording') = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(server)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }
}

/// Recordings older than [`KEEP_FOR`] deleted, then the oldest until they fit [`MOST_KEPT_BYTES`].
fn prune(dir: &std::path::Path, now: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    let oldest = now - std::time::Duration::from_secs(KEEP_FOR as u64);
    for (at, size, path) in files {
        if at >= oldest && total <= MOST_KEPT_BYTES {
            break;
        }
        total = total.saturating_sub(size);
        let _ = std::fs::remove_file(path);
    }
}
