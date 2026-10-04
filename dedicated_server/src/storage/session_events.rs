//! Players' session events (`session_events`; see `crate::session_events`): saved, sent to
//! the coordinator, and deleted after a while.

use serde::Serialize;
use serde_json::json;
use serde_json::Value;

use super::Result;
use super::Storage;

/// Kinds whose repeat (the same detail, for the same player) within this many seconds is
/// counted on the earlier event instead of making one of its own.
const REPEATS: &[(&str, i64)] = &[("signin_refused", 600), ("request_error", 600), ("stats", 120), ("relay_drop", 60)];
/// Sent events are kept this long (for a look on the server itself), unsent ones longer.
const KEEP_SENT_SECS: i64 = 3 * 86_400;
const KEEP_UNSENT_SECS: i64 = 14 * 86_400;
/// Detail keys that name a player by account id; each gets a `<key>_name`.
const PLAYER_KEYS: &[&str] = &["host", "to", "from", "by"];

/// An event as noted, before it's saved.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSessionEvent {
    pub at: i64,
    pub user_id: Option<u32>,
    pub name: Option<String>,
    pub kind: &'static str,
    pub detail: Value,
}

/// A saved event, as sent to the coordinator.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionEvent {
    pub id: i64,
    pub at: i64,
    pub last_at: i64,
    /// The player's account id here, and name.
    pub player: Option<u32>,
    pub name: Option<String>,
    pub kind: String,
    pub detail: Value,
    pub count: u32,
}

impl Storage {
    /// Saves noted events: who they are about (by id and name), what their room was, the
    /// names of the players they mention; a repeat is counted on the event it repeats. The
    /// server's own accounts (the game's tracking sign-in) are left out.
    pub async fn save_session_events_async(&self, events: Vec<NewSessionEvent>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for mut event in events {
            let (user_id, name) = match (event.user_id, event.name.take()) {
                (Some(id), _) => {
                    let name: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ? AND ubi_id IS NOT NULL AND ubi_id != ''")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await?;
                    let Some(name) = name else { continue };
                    (Some(id), Some(name))
                }
                (None, Some(name)) => {
                    let found: Option<(u32, String)> = sqlx::query_as("SELECT id, username FROM users WHERE name_key = ? AND ubi_id IS NOT NULL AND ubi_id != ''")
                        .bind(super::name_key(&name))
                        .fetch_optional(&mut *tx)
                        .await?;
                    match found {
                        Some((id, name)) => (Some(id), Some(name)),
                        None => (None, Some(name)),
                    }
                }
                (None, None) => (None, None),
            };
            let mut detail = event.detail;
            if !detail.is_object() {
                detail = json!({});
            }
            if let Some(room) = detail["room"].as_u64() {
                let row: Option<(Option<String>, u32, i64)> =
                    sqlx::query_as("SELECT attributes, creator_id, CAST(strftime('%s', created_at) AS INTEGER) FROM game_sessions WHERE id = ?")
                        .bind(i64::try_from(room).unwrap_or_default())
                        .fetch_optional(&mut *tx)
                        .await?;
                if let Some((attributes, creator, since)) = row {
                    // Ids come again once old rooms are gone: the time it was made tells them apart.
                    detail["since"] = json!(since);
                    if let (Value::Object(d), Value::Object(r)) = (&mut detail, crate::session_events::room_detail(&attributes.unwrap_or_default())) {
                        for (k, v) in r {
                            d.entry(k).or_insert(v);
                        }
                        d.entry("host").or_insert(json!(creator));
                    }
                }
            }
            for key in PLAYER_KEYS {
                if let Some(id) = detail[*key].as_u64() {
                    let name: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
                        .bind(i64::try_from(id).unwrap_or_default())
                        .fetch_optional(&mut *tx)
                        .await?;
                    if let Some(name) = name {
                        detail[format!("{key}_name")] = json!(name);
                    }
                }
            }
            let detail = detail.to_string();
            if let Some((_, window)) = REPEATS.iter().find(|(k, _)| *k == event.kind) {
                let repeated = sqlx::query(
                    "UPDATE session_events SET count = count + 1, last_at = MAX(last_at, ?1), sent = 0
                     WHERE id = (SELECT id FROM session_events WHERE kind = ?2 AND user_id IS ?3 AND name IS ?4 AND detail = ?5
                                 AND last_at >= ?1 - ?6 ORDER BY id DESC LIMIT 1)",
                )
                .bind(event.at)
                .bind(event.kind)
                .bind(user_id)
                .bind(&name)
                .bind(&detail)
                .bind(window)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if repeated > 0 {
                    continue;
                }
            }
            sqlx::query("INSERT INTO session_events (at, last_at, user_id, name, kind, detail) VALUES (?1, ?1, ?2, ?3, ?4, ?5)")
                .bind(event.at)
                .bind(user_id)
                .bind(&name)
                .bind(event.kind)
                .bind(&detail)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The oldest events the coordinator hasn't got yet (or not as they are now).
    pub async fn unsent_session_events_async(&self, limit: u32) -> Result<Vec<SessionEvent>> {
        let rows: Vec<(i64, i64, i64, Option<u32>, Option<String>, String, String, u32)> =
            sqlx::query_as("SELECT id, at, last_at, user_id, name, kind, detail, count FROM session_events WHERE sent = 0 ORDER BY id LIMIT ?")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(id, at, last_at, player, name, kind, detail, count)| SessionEvent {
                id,
                at,
                last_at,
                player,
                name,
                kind,
                detail: serde_json::from_str(&detail).unwrap_or_else(|_| json!({})),
                count,
            })
            .collect())
    }

    /// Marks events sent, unless they changed since (a repeat counted meanwhile goes again).
    pub async fn mark_session_events_sent_async(&self, events: &[SessionEvent]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for event in events {
            sqlx::query("UPDATE session_events SET sent = 1 WHERE id = ? AND count = ? AND last_at = ?")
                .bind(event.id)
                .bind(event.count)
                .bind(event.last_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Deletes sent events after a few days, and any after two weeks.
    pub async fn prune_session_events_async(&self) -> Result<()> {
        sqlx::query("DELETE FROM session_events WHERE (sent = 1 AND last_at < strftime('%s', 'now') - ?) OR last_at < strftime('%s', 'now') - ?")
            .bind(KEEP_SENT_SECS)
            .bind(KEEP_UNSENT_SECS)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::NewSessionEvent;
    use crate::storage::run;
    use crate::storage::tests::temp_storage;

    fn event(at: i64, user_id: Option<u32>, name: Option<&str>, kind: &'static str, detail: serde_json::Value) -> NewSessionEvent {
        NewSessionEvent {
            at,
            user_id,
            name: name.map(str::to_string),
            kind,
            detail,
        }
    }

    #[test]
    fn events_are_saved_described_counted_and_sent_once() {
        let (storage, dir) = temp_storage("session-events");
        storage.register_user("Host1", "pw", Some("Host1-UBI")).unwrap();
        storage.register_user("Guest1", "pw", Some("Guest1-UBI")).unwrap();
        let host = storage.find_user_id_by_name("Host1").unwrap().unwrap();
        let guest = storage.find_user_id_by_name("Guest1").unwrap().unwrap();
        let coop = "113 => 0;3 => 0;4 => 2;101 => 3578398534;102 => 3;103 => 0;105 => 2";
        let room = storage.create_game_session(host, 1, coop.into()).unwrap();

        run(storage.save_session_events_async(vec![
            event(100, Some(guest), None, "join", json!({ "room": room, "via": "invite" })),
            event(101, None, Some("guest1"), "relay_drop", json!({ "direction": "sending", "before": 60, "after": 4 })),
            event(102, None, Some("Stranger"), "signin_refused", json!({ "reason": "too_many", "via": "game" })),
            event(103, None, Some("Stranger"), "signin_refused", json!({ "reason": "too_many", "via": "game" })),
            // The server's own accounts aren't players.
            event(104, Some(105), None, "signout", json!({ "how": "closed" })),
        ]))
        .unwrap()
        .unwrap();

        let events = run(storage.unsent_session_events_async(100)).unwrap().unwrap();
        assert_eq!(events.len(), 3, "{events:?}");
        let join = &events[0];
        assert_eq!((join.player, join.name.as_deref()), (Some(guest), Some("Guest1")));
        assert_eq!(join.detail["room_kind"], "match");
        assert_eq!(join.detail["mode"], "coop");
        assert_eq!(join.detail["private"], true);
        assert_eq!(join.detail["host_name"], "Host1");
        assert!(join.detail["since"].as_i64().is_some_and(|t| t > 0));
        assert_eq!((events[1].player, events[1].name.as_deref()), (Some(guest), Some("Guest1")), "found by name");
        assert_eq!((events[2].player, events[2].count, events[2].last_at), (None, 2, 103), "a repeat is counted");

        run(storage.mark_session_events_sent_async(&events)).unwrap().unwrap();
        assert!(run(storage.unsent_session_events_async(100)).unwrap().unwrap().is_empty());
        // Another repeat: the same event goes again, with its new count.
        run(storage.save_session_events_async(vec![event(500, None, Some("Stranger"), "signin_refused", json!({ "reason": "too_many", "via": "game" }))]))
            .unwrap()
            .unwrap();
        let again = run(storage.unsent_session_events_async(100)).unwrap().unwrap();
        assert_eq!((again.len(), again[0].id, again[0].count), (1, events[2].id, 3));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
