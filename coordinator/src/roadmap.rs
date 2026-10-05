//! The project's roadmap, as admins keep it, and players' suggestions for it.
//!
//! Admins keep items in four lanes (shipping, next, later, requested) in the admin UI; the
//! public ones are what players' launchers show (`GET /v1/roadmap`). Players suggest features
//! and improvements from the launcher (`POST /v1/suggestions`), signed with their identity, and
//! read theirs back with the admins' reply (`GET /v1/suggestions/mine`). Launchers always ask
//! the community network's coordinator: the roadmap is the project's, whatever network a
//! player is on.

use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// The lanes, in order: (id, title).
pub const LANES: [(&str, &str); 4] = [("shipping", "Shipping"), ("next", "Next"), ("later", "Later"), ("requested", "Requested")];
/// What a player can suggest about.
pub const AREAS: [&str; 5] = ["Launcher", "In the game", "Matches and joining", "Servers", "Other"];
/// Suggestions a player may send in a day, and an address.
pub const PER_PLAYER_A_DAY: i64 = 3;
pub const PER_ADDRESS_A_DAY: usize = 10;
/// A suggestion's statuses.
pub const STATUSES: [&str; 4] = ["new", "planned", "done", "declined"];
const MAX_TITLE: usize = 80;
const MAX_TEXT: usize = 1000;
const MAX_BODY: usize = 600;
const MAX_REPLY: usize = 300;
const DAY: i64 = 86_400;

fn release_key(lane: &str) -> String {
    format!("roadmap.release.{lane}")
}

fn one_line(s: &str, max: usize) -> bool {
    crate::valid_text(s, max)
}

/// Text that may run over lines: printable characters and newlines.
fn lines(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(|c| (c.is_control() && c != '\n') || crate::hidden_char(c))
}

/// A suggestion as a launcher sends it.
#[derive(Debug, Deserialize)]
pub struct Suggestion {
    pub identity: String,
    pub name: String,
    pub area: String,
    pub title: String,
    pub text: String,
    pub time: i64,
    pub signature: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub launcher: String,
}

impl Suggestion {
    /// Why it can't be taken at `now`, if it can't (the signature included).
    pub fn check(&self, now: i64) -> Result<(), &'static str> {
        if !AREAS.contains(&self.area.as_str()) {
            return Err("unknown area");
        }
        let title = self.title.trim();
        if title.chars().count() < 3 || !one_line(title, MAX_TITLE) {
            return Err("the title is 3-80 characters, on one line");
        }
        if self.text.trim().is_empty() || !lines(&self.text, MAX_TEXT) {
            return Err("the details are 1-1000 characters");
        }
        if self.name.trim().is_empty() || !one_line(&self.name, 32) || !one_line(&self.server, 253) || !one_line(&self.launcher, 32) {
            return Err("not a valid request");
        }
        if !identity::fresh(self.time, now) {
            return Err("this PC's clock is off; set it right and send again");
        }
        if !identity::verify(
            &self.identity,
            &identity::suggestion_message(self.time, &self.area, &self.title, &self.text),
            &self.signature,
        ) {
            return Err("the signature doesn't match");
        }
        Ok(())
    }
}

/// An item as an admin writes it.
#[derive(Debug, Deserialize)]
pub struct ItemEdit {
    pub lane: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub position: Option<i64>,
}

impl ItemEdit {
    pub fn check(&self) -> Result<(), &'static str> {
        if !LANES.iter().any(|(id, _)| *id == self.lane) {
            return Err("unknown lane");
        }
        if self.title.trim().is_empty() || !one_line(&self.title, MAX_TITLE) {
            return Err("the title is 1-80 characters, on one line");
        }
        if !lines(&self.body, MAX_BODY) {
            return Err("the text is up to 600 characters");
        }
        if self.tags.len() > 5 || self.tags.iter().any(|t| t.trim().is_empty() || !one_line(t, 20)) {
            return Err("up to 5 tags of up to 20 characters");
        }
        if !one_line(&self.status, 24) || !one_line(&self.source, 60) {
            return Err("the status is up to 24 characters, and who asked up to 60");
        }
        Ok(())
    }
}

type ItemRow = (i64, String, String, String, String, String, i64, bool, String, Option<i64>, i64, String);
const ITEM_COLUMNS: &str = "id, lane, title, body, tags, status, position, public, source, suggestion_id, updated_at, updated_by";

fn item_json((id, lane, title, body, tags, status, position, public, source, suggestion, updated_at, updated_by): ItemRow, admin: bool) -> Value {
    let tags: Value = serde_json::from_str(&tags).unwrap_or_else(|_| json!([]));
    let mut v = json!({ "id": id, "title": title, "body": body, "tags": tags, "status": status });
    if admin {
        v["lane"] = json!(lane);
        v["position"] = json!(position);
        v["public"] = json!(public);
        v["source"] = json!(source);
        v["suggestion"] = json!(suggestion);
        v["updated_at"] = json!(updated_at);
        v["updated_by"] = json!(updated_by);
    }
    v
}

type SuggestionRow = (i64, String, String, String, String, String, String, String, i64, String, String, Option<i64>);
const SUGGESTION_COLUMNS: &str = "id, global_id, name, server, launcher, area, title, text, created_at, status, reply, item_id";

fn suggestion_json((id, identity, name, server, launcher, area, title, text, created_at, status, reply, item): SuggestionRow, admin: bool) -> Value {
    let mut v = json!({ "id": id, "area": area, "title": title, "text": text, "created_at": created_at, "status": status, "reply": reply });
    if admin {
        v["identity"] = json!(identity);
        v["name"] = json!(name);
        v["server"] = json!(server);
        v["launcher"] = json!(launcher);
        v["item"] = json!(item);
    }
    v
}

impl Coordinator {
    async fn lane_releases(&self) -> sqlx::Result<Vec<(String, String, String)>> {
        let mut lanes = Vec::new();
        for (id, title) in LANES {
            lanes.push((id.to_string(), title.to_string(), self.setting(&release_key(id)).await?.unwrap_or_default()));
        }
        Ok(lanes)
    }

    async fn items(&self, public_only: bool) -> sqlx::Result<Vec<ItemRow>> {
        sqlx::query_as(&format!(
            "SELECT {ITEM_COLUMNS} FROM roadmap_items {} ORDER BY position, id",
            if public_only { "WHERE public = 1" } else { "" }
        ))
        .fetch_all(&self.pool)
        .await
    }

    /// What launchers show: the lanes with their public items.
    pub(crate) async fn public_roadmap(&self) -> sqlx::Result<Value> {
        let items = self.items(true).await?;
        let updated: Option<i64> = sqlx::query_scalar("SELECT MAX(updated_at) FROM roadmap_items WHERE public = 1")
            .fetch_one(&self.pool)
            .await?;
        let lanes: Vec<Value> = self
            .lane_releases()
            .await?
            .into_iter()
            .map(|(id, title, release)| {
                let list: Vec<Value> = items.iter().filter(|i| i.1 == id).cloned().map(|i| item_json(i, false)).collect();
                json!({ "id": id, "title": title, "release": release, "items": list })
            })
            .collect();
        Ok(json!({ "lanes": lanes, "updated": updated.unwrap_or(0) }))
    }

    /// The admin UI's roadmap: every item, the lanes' releases and how many suggestions are new.
    pub(crate) async fn admin_roadmap(&self) -> sqlx::Result<Value> {
        let items: Vec<Value> = self.items(false).await?.into_iter().map(|i| item_json(i, true)).collect();
        let lanes: Vec<Value> = self
            .lane_releases()
            .await?
            .into_iter()
            .map(|(id, title, release)| json!({ "id": id, "title": title, "release": release }))
            .collect();
        let new: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM suggestions WHERE status = 'new'").fetch_one(&self.pool).await?;
        Ok(json!({ "lanes": lanes, "items": items, "suggestions": { "new": new } }))
    }

    async fn item(&self, id: i64) -> sqlx::Result<Option<Value>> {
        let row: Option<ItemRow> = sqlx::query_as(&format!("SELECT {ITEM_COLUMNS} FROM roadmap_items WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| item_json(r, true)))
    }

    /// Adds an item (checked) at the end of its lane.
    pub(crate) async fn add_item(&self, e: &ItemEdit, by: &str, suggestion: Option<i64>, now: i64) -> sqlx::Result<Value> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let last: Option<i64> = sqlx::query_scalar("SELECT MAX(position) FROM roadmap_items WHERE lane = ?")
            .bind(&e.lane)
            .fetch_one(&mut *tx)
            .await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO roadmap_items (lane, title, body, tags, status, position, public, source, suggestion_id, created_at, updated_at, updated_by)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&e.lane)
        .bind(e.title.trim())
        .bind(e.body.trim())
        .bind(json!(e.tags.iter().map(|t| t.trim()).collect::<Vec<_>>()).to_string())
        .bind(e.status.trim())
        .bind(e.position.unwrap_or_else(|| last.unwrap_or(0) + 1))
        .bind(e.public)
        .bind(e.source.trim())
        .bind(suggestion)
        .bind(now)
        .bind(now)
        .bind(by)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(self.item(id).await?.unwrap_or_default())
    }

    /// Replaces an item's fields (checked); None for no such item.
    pub(crate) async fn edit_item(&self, id: i64, e: &ItemEdit, by: &str, now: i64) -> sqlx::Result<Option<Value>> {
        let done = sqlx::query(
            "UPDATE roadmap_items SET lane = ?, title = ?, body = ?, tags = ?, status = ?, position = COALESCE(?, position),
                    public = ?, source = ?, updated_at = ?, updated_by = ? WHERE id = ?",
        )
        .bind(&e.lane)
        .bind(e.title.trim())
        .bind(e.body.trim())
        .bind(json!(e.tags.iter().map(|t| t.trim()).collect::<Vec<_>>()).to_string())
        .bind(e.status.trim())
        .bind(e.position)
        .bind(e.public)
        .bind(e.source.trim())
        .bind(now)
        .bind(by)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Ok(None);
        }
        self.item(id).await
    }

    /// Deletes an item: answers its title, or None for no such item.
    pub(crate) async fn remove_item(&self, id: i64) -> sqlx::Result<Option<String>> {
        sqlx::query_scalar("DELETE FROM roadmap_items WHERE id = ? RETURNING title")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Sets the release a lane is for ("" for none).
    pub(crate) async fn set_lane_release(&self, lane: &str, release: &str) -> sqlx::Result<()> {
        self.set_setting(&release_key(lane), release.trim()).await
    }

    /// Takes a player's suggestion (checked): its id, or Err with what to tell them when
    /// they've sent their day's share.
    pub(crate) async fn add_suggestion(&self, s: &Suggestion, now: i64) -> sqlx::Result<Result<i64, String>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let today: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM suggestions WHERE global_id = ? AND created_at > ?")
            .bind(&s.identity)
            .bind(now - DAY)
            .fetch_one(&mut *tx)
            .await?;
        if today >= PER_PLAYER_A_DAY {
            return Ok(Err(format!("You've sent {PER_PLAYER_A_DAY} suggestions today; try again tomorrow.")));
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO suggestions (global_id, name, server, launcher, area, title, text, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&s.identity)
        .bind(s.name.trim())
        .bind(s.server.trim())
        .bind(s.launcher.trim())
        .bind(&s.area)
        .bind(s.title.trim())
        .bind(s.text.trim())
        .bind(now)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Ok(id))
    }

    /// A player's suggestions, newest first.
    pub(crate) async fn suggestions_of(&self, identity: &str) -> sqlx::Result<Value> {
        let rows: Vec<SuggestionRow> = sqlx::query_as(&format!("SELECT {SUGGESTION_COLUMNS} FROM suggestions WHERE global_id = ? ORDER BY id DESC LIMIT 50"))
            .bind(identity)
            .fetch_all(&self.pool)
            .await?;
        Ok(json!({ "suggestions": rows.into_iter().map(|r| suggestion_json(r, false)).collect::<Vec<_>>() }))
    }

    /// The admin UI's suggestions: the new ones, or all, newest first.
    pub(crate) async fn suggestion_list(&self, all: bool) -> sqlx::Result<Value> {
        let rows: Vec<SuggestionRow> = sqlx::query_as(&format!(
            "SELECT {SUGGESTION_COLUMNS} FROM suggestions {} ORDER BY id DESC LIMIT 500",
            if all { "" } else { "WHERE status = 'new'" }
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(json!({ "suggestions": rows.into_iter().map(|r| suggestion_json(r, true)).collect::<Vec<_>>() }))
    }

    async fn suggestion(&self, id: i64) -> sqlx::Result<Option<SuggestionRow>> {
        sqlx::query_as(&format!("SELECT {SUGGESTION_COLUMNS} FROM suggestions WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Sets a suggestion's status and the reply the player sees; None for no such one.
    pub(crate) async fn answer_suggestion(&self, id: i64, status: &str, reply: &str, now: i64) -> sqlx::Result<Option<Value>> {
        let done = sqlx::query("UPDATE suggestions SET status = ?, reply = ?, updated_at = ? WHERE id = ?")
            .bind(status)
            .bind(reply.trim())
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(self.suggestion(id).await?.map(|r| suggestion_json(r, true)))
    }

    /// Makes a roadmap item of a suggestion, in `lane`, not public yet; the suggestion is then
    /// planned. None for no such suggestion.
    pub(crate) async fn promote_suggestion(&self, id: i64, lane: &str, by: &str, now: i64) -> sqlx::Result<Option<Value>> {
        let Some((_, _, name, _, _, area, title, text, ..)) = self.suggestion(id).await? else {
            return Ok(None);
        };
        let body: String = text.chars().take(MAX_BODY).collect();
        let item = self
            .add_item(
                &ItemEdit {
                    lane: lane.to_string(),
                    title,
                    body,
                    tags: vec![area],
                    status: String::from("Requested"),
                    public: false,
                    source: format!("Suggested by {name}").chars().take(60).collect(),
                    position: None,
                },
                by,
                Some(id),
                now,
            )
            .await?;
        sqlx::query("UPDATE suggestions SET status = 'planned', item_id = ?, updated_at = ? WHERE id = ?")
            .bind(item["id"].as_i64())
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(Some(item))
    }
}

/// Whether `status` and `reply` may be set on a suggestion.
pub fn valid_answer(status: &str, reply: &str) -> bool {
    STATUSES.contains(&status) && lines(reply, MAX_REPLY)
}

/// Whether `lane` is one of the roadmap's.
pub fn valid_lane(lane: &str) -> bool {
    LANES.iter().any(|(id, _)| *id == lane)
}
