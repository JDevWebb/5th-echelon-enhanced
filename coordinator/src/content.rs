//! What players' games upload, from their servers (`POST /v1/content`): today only the
//! ShadowNet companion snapshot (UserStorage type 0x80000003), the player's loadouts, owned
//! items, purchases and challenge progress as JSON, ~135 KB, at most hourly. The game never
//! reads it back; it's kept here, each player's latest, for the admins only (the player's
//! page in the admin UI), until what it's worth is clear.
//!
//! `{player: {id, name, identity?}, type, size, updated_at, gzip_base64}`.

use base64::Engine as _;
use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// The ShadowNet companion snapshot.
pub const SHADOWNET: i64 = 0x8000_0003;
/// The largest snapshot taken, uncompressed (the game's are ~135 KB).
pub const MAX_SIZE: usize = 1024 * 1024;
/// The largest request body (base64 of the gzip, and the rest).
pub const MAX_BODY: usize = 2 * 1024 * 1024;
/// Snapshots a server may send an hour.
pub const PER_HOUR: usize = 600;
/// A snapshot not updated for this long goes (with the daily cleanup).
const KEEP_FOR: i64 = 90 * 86_400;

/// A checked snapshot.
pub struct Content {
    pub player_id: i64,
    pub player_name: String,
    pub identity: Option<String>,
    pub type_id: i64,
    pub size: i64,
    pub gzip: Vec<u8>,
    pub updated_at: i64,
}

/// Checks a server's `POST /v1/content`: a known type, a player, and gzip of JSON within
/// [`MAX_SIZE`] that says what it said it was.
pub fn check(v: &Value, now: i64) -> Result<Content, String> {
    let type_id = v["type"].as_i64().filter(|t| *t == SHADOWNET).ok_or("type isn't one kept here")?;
    let player_id = v["player"]["id"]
        .as_i64()
        .filter(|id| *id > 0 && *id <= i64::from(u32::MAX))
        .ok_or("player.id is missing")?;
    let player_name: String = v["player"]["name"].as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(32).collect();
    if player_name.is_empty() {
        return Err("player.name is missing".into());
    }
    let identity = v["player"]["identity"].as_str().filter(|i| identity::is_global_id(i)).map(str::to_string);
    let updated_at = v["updated_at"].as_i64().filter(|t| (0..=now + 300).contains(t)).ok_or("updated_at isn't a time")?;
    let gzip = base64::engine::general_purpose::STANDARD
        .decode(v["gzip_base64"].as_str().unwrap_or_default())
        .map_err(|_| "gzip_base64 isn't base64")?;
    let text = crate::reports::gunzip(&gzip, MAX_SIZE).map_err(|_| "not gzip within 1 MB")?;
    if serde_json::from_slice::<Value>(&text).map_or(true, |j| !j.is_object()) {
        return Err("not a JSON object".into());
    }
    let size = i64::try_from(text.len()).unwrap_or(i64::MAX);
    Ok(Content {
        player_id,
        player_name,
        identity,
        type_id,
        size,
        gzip,
        updated_at,
    })
}

/// What a snapshot holds, counted: owned items, loadouts, purchases and challenges (each a
/// JSON object keyed by the game's ids), and the challenges with any progress.
pub fn summary(snapshot: &Value) -> Value {
    let count = |key: &str| snapshot[key].as_object().map_or(0, serde_json::Map::len);
    let challenges = snapshot["Challenges"].as_object();
    let started = challenges.map_or(0, |c| c.values().filter(|v| v["T"].as_i64().unwrap_or(0) > 0 || v["P"].as_i64().unwrap_or(0) > 0).count());
    json!({
        "items": count("Loadout:Items"),
        "loadouts": count("Loadout:Loadouts"),
        "purchases": count("Purchase"),
        "challenges": count("Challenges"),
        "challenges_started": started,
        "parts": snapshot.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),
    })
}

impl Coordinator {
    /// Keeps `server`'s player's snapshot, replacing an older one.
    pub(crate) async fn store_content(&self, server: &str, c: &Content) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO player_content (server_id, player_id, type_id, player_name, player_identity, size, gzip, updated_at, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (server_id, player_id, type_id) DO UPDATE SET
               player_name = excluded.player_name, player_identity = excluded.player_identity, size = excluded.size,
               gzip = excluded.gzip, updated_at = excluded.updated_at, received_at = excluded.received_at
             WHERE excluded.updated_at >= player_content.updated_at",
        )
        .bind(server)
        .bind(c.player_id)
        .bind(c.type_id)
        .bind(&c.player_name)
        .bind(&c.identity)
        .bind(c.size)
        .bind(&c.gzip)
        .bind(c.updated_at)
        .bind(identity::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A player's snapshot for the admin UI: when, how big, what it holds, and the JSON.
    pub(crate) async fn player_content(&self, server: &str, player: i64) -> sqlx::Result<Option<Value>> {
        let row: Option<(i64, Vec<u8>, i64, i64)> =
            sqlx::query_as("SELECT size, gzip, updated_at, received_at FROM player_content WHERE server_id = ? AND player_id = ? AND type_id = ?")
                .bind(server)
                .bind(player)
                .bind(SHADOWNET)
                .fetch_optional(&self.pool)
                .await?;
        let Some((size, gzip, updated_at, received_at)) = row else { return Ok(None) };
        let snapshot = crate::reports::gunzip(&gzip, MAX_SIZE)
            .ok()
            .and_then(|t| serde_json::from_slice::<Value>(&t).ok())
            .unwrap_or(Value::Null);
        Ok(Some(json!({
            "type": "shadownet",
            "size": size,
            "updated_at": updated_at,
            "received_at": received_at,
            "summary": summary(&snapshot),
            "snapshot": snapshot,
        })))
    }

    /// Deletes snapshots past [`KEEP_FOR`] (with the daily cleanup).
    pub(crate) async fn prune_content(&self) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM player_content WHERE updated_at < ?")
            .bind(identity::now() - KEEP_FOR)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_is_counted() {
        let s = json!({
            "Loadout:Items": { "1": ["2", "3"], "4": [] },
            "Loadout:Loadouts": { "5": {} },
            "Purchase": { "6": 1, "7": 1, "8": 1 },
            "Challenges": { "9": { "T": 2, "P": 40 }, "10": { "T": 0, "P": 0 } },
        });
        let got = summary(&s);
        assert_eq!(
            (
                got["items"].as_u64(),
                got["loadouts"].as_u64(),
                got["purchases"].as_u64(),
                got["challenges"].as_u64(),
                got["challenges_started"].as_u64()
            ),
            (Some(2), Some(1), Some(3), Some(2), Some(1))
        );
    }
}
