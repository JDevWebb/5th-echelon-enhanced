//! The admin UI's Moderation page: chat (on the community network's coordinator, `--chat`)
//! and names (on any).
//!
//! * `GET /api/moderation`: open reports, each with the message, what came before it (in a
//!   direct conversation only its neighbours) and the sender's accounts and mutes; messages
//!   refused for a link; the players with the most masked words this week; mutes in force;
//!   players' names that fail the rules; the rules' settings.
//! * `POST /api/moderation/reports/<message>` `{action, note?}`: `delete` (the message goes for
//!   everyone), `resolve` (dealt with, the message stays) or `dismiss`. Deleting wants a second
//!   factor proved lately.
//! * `POST /api/moderation/mutes` `{identity, room, hours?, reason?}`: mutes a player in a room
//!   (`global`, `direct`, `lobby`) or everywhere (`*`), for so many hours or for good (a second
//!   factor proved lately). `DELETE /api/moderation/mutes/<identity>/<room>` lifts one.
//! * `PUT /api/moderation/settings` `{allow, extra, discord_invites}`: words and names cleared,
//!   words of the network's own, whether Discord invites may be sent (a second factor proved
//!   lately). Servers have them with their next pulse.
//!
//! Bans are the players' page's (the sender's accounts link there). Everything done here goes
//! to the audit log.

use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::delete;
use axum::routing::get;
use axum::routing::post;
use axum::Extension;
use axum::Json;
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

use super::fail;
use super::internal;
use super::ok;
use super::Client;
use super::Shared;
use crate::admin::live::Event;
use crate::Coordinator;

pub(super) fn routes() -> Router<Shared> {
    Router::new()
        .route("/moderation", get(overview))
        .route("/moderation/reports/{message}", post(handle_report))
        .route("/moderation/mutes", post(mute))
        .route("/moderation/mutes/{identity}/{room}", delete(unmute))
        .route("/moderation/settings", axum::routing::put(set_settings))
}

/// Rooms a mute can be for ("*": everywhere).
const MUTE_ROOMS: [&str; 4] = ["*", "global", "direct", "lobby"];
/// What's listed at most: reports, refused messages, failing names.
const LIST: i64 = 100;
/// Messages shown before a reported one (Global and lobbies), and around it (direct).
const BEFORE: i64 = 20;
const AROUND: i64 = 2;
/// The longest list of allowed or extra words, and the longest word.
const MAX_WORDS: usize = 2000;
const MAX_WORD: usize = 64;

fn no_chat() -> Response {
    fail(StatusCode::NOT_FOUND, "chat is on the community network's coordinator only")
}

type MessageRow = (i64, String, String, i64, String, String, String, Option<String>, String, Option<i64>, Option<String>);

const MESSAGE: &str = "id, room, conv, at, sender, sender_name, server, recipient, body, deleted_at, deleted_by";

fn message_json(m: &MessageRow) -> Value {
    let (id, room, conv, at, sender, sender_name, server, recipient, body, deleted_at, deleted_by) = m;
    json!({
        "id": id, "room": room, "conv": conv, "at": at, "from": sender, "from_name": sender_name,
        "server": server, "to": recipient, "body": body, "deleted_at": deleted_at,
        // A player's own deletion is theirs; an admin's says who.
        "deleted_by": deleted_by.as_deref().map(|b| if b.starts_with("admin:") { b } else { "sender" }),
    })
}

impl Coordinator {
    /// Open chat reports: how many messages (the rail's badge).
    pub(crate) async fn open_chat_reports(&self) -> sqlx::Result<i64> {
        if !self.has_chat() {
            return Ok(0);
        }
        sqlx::query_scalar("SELECT COUNT(DISTINCT message_id) FROM chat_reports WHERE status = 'open'")
            .fetch_one(&self.pool)
            .await
    }

    /// A name for an identity: one it's linked under.
    async fn identity_name(&self, global_id: &str) -> sqlx::Result<String> {
        Ok(sqlx::query_scalar("SELECT username FROM links WHERE global_id = ? ORDER BY linked_at DESC LIMIT 1")
            .bind(global_id)
            .fetch_optional(&self.pool)
            .await?
            .unwrap_or_default())
    }

    /// An identity's accounts, for the players' page (bans), and its mutes in force.
    async fn identity_detail(&self, global_id: &str, now: i64) -> sqlx::Result<Value> {
        let accounts: Vec<(String, i64, String)> = sqlx::query_as("SELECT server_id, id, name FROM players WHERE identity = ? ORDER BY server_id")
            .bind(global_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(json!({
            "accounts": accounts.into_iter().map(|(server, id, name)| json!({ "server": server, "id": id, "name": name })).collect::<Vec<_>>(),
            "mutes": self.mutes(Some(global_id), now).await?,
        }))
    }

    async fn mutes(&self, global_id: Option<&str>, now: i64) -> sqlx::Result<Vec<Value>> {
        let rows: Vec<(String, String, Option<i64>, String, String, i64)> = sqlx::query_as(
            "SELECT global_id, room, until, by_admin, reason, at FROM chat_mutes
             WHERE (until IS NULL OR until > ?) AND (?2 IS NULL OR global_id = ?2) ORDER BY at DESC LIMIT 500",
        )
        .bind(now)
        .bind(global_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (who, room, until, by, reason, at) in rows {
            let name = self.identity_name(&who).await?;
            out.push(json!({ "identity": who, "name": name, "room": room, "until": until, "by": by, "reason": reason, "at": at }));
        }
        Ok(out)
    }

    /// The open reports, oldest first, one entry a message.
    async fn chat_report_list(&self, now: i64) -> sqlx::Result<Vec<Value>> {
        let rows: Vec<MessageRow> = sqlx::query_as(&format!(
            "SELECT {MESSAGE} FROM chat_messages m WHERE id IN (SELECT message_id FROM chat_reports WHERE status = 'open')
             ORDER BY (SELECT MIN(at) FROM chat_reports r WHERE r.message_id = m.id AND r.status = 'open') LIMIT ?"
        ))
        .bind(LIST)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for m in &rows {
            let (id, room, conv, ..) = m;
            let reports: Vec<(String, String, String, i64)> =
                sqlx::query_as("SELECT reporter, reporter_server, why, at FROM chat_reports WHERE message_id = ? AND status = 'open' ORDER BY at")
                    .bind(id)
                    .fetch_all(&self.pool)
                    .await?;
            let mut by = Vec::with_capacity(reports.len());
            for (reporter, server, why, at) in reports {
                let name = self.identity_name(&reporter).await?;
                by.push(json!({ "identity": reporter, "name": name, "server": server, "why": why, "at": at }));
            }
            // What came before (a direct conversation is private: only the messages around it).
            let (before, after) = if room == "direct" { (AROUND, AROUND) } else { (BEFORE, 0) };
            let mut context: Vec<MessageRow> = sqlx::query_as(&format!(
                "SELECT {MESSAGE} FROM chat_messages WHERE conv = ? AND id < ? AND deletes IS NULL ORDER BY id DESC LIMIT ?"
            ))
            .bind(conv)
            .bind(id)
            .bind(before)
            .fetch_all(&self.pool)
            .await?;
            context.reverse();
            let later: Vec<MessageRow> = sqlx::query_as(&format!(
                "SELECT {MESSAGE} FROM chat_messages WHERE conv = ? AND id > ? AND deletes IS NULL ORDER BY id LIMIT ?"
            ))
            .bind(conv)
            .bind(id)
            .bind(after)
            .fetch_all(&self.pool)
            .await?;
            out.push(json!({
                "message": message_json(m),
                "reports": by,
                "before": context.iter().map(message_json).collect::<Vec<_>>(),
                "after": later.iter().map(message_json).collect::<Vec<_>>(),
                "sender": self.identity_detail(&m.4, now).await?,
            }));
        }
        Ok(out)
    }

    /// Players' names that fail the rules, as their servers last reported them.
    pub(crate) async fn failing_names(&self) -> sqlx::Result<Vec<Value>> {
        let rules = self.chat_rules().await;
        let all: Vec<(String, i64, String, Option<String>, Option<i64>)> = sqlx::query_as("SELECT server_id, id, name, identity, last_seen FROM players ORDER BY last_seen DESC")
            .fetch_all(&self.pool)
            .await?;
        Ok(all
            .into_iter()
            .filter_map(|(server, id, name, identity, last_seen)| {
                let why = rules.check_name(&name).err()?;
                Some(json!({ "server": server, "id": id, "name": name, "identity": identity, "last_seen": last_seen, "why": why.says() }))
            })
            .take(LIST as usize)
            .collect())
    }

    async fn moderation_overview(&self) -> sqlx::Result<Value> {
        let now = identity::now();
        let (allow, extra, discord_invites) = self.chat_settings().await;
        let mut v = json!({
            "chat": self.has_chat(),
            "names": self.failing_names().await?,
            "settings": { "allow": allow, "extra": extra, "discord_invites": discord_invites },
        });
        if !self.has_chat() {
            return Ok(v);
        }
        v["reports"] = json!(self.chat_report_list(now).await?);
        v["mutes"] = json!(self.mutes(None, now).await?);
        let refused: Vec<(i64, i64, String, String, String, String, String, String)> =
            sqlx::query_as("SELECT id, at, sender, sender_name, server, room, reason, body FROM chat_refused ORDER BY id DESC LIMIT ?")
                .bind(LIST)
                .fetch_all(&self.pool)
                .await?;
        v["refused"] = json!(refused
            .into_iter()
            .map(|(id, at, sender, name, server, room, reason, body)| json!({
                "id": id, "at": at, "from": sender, "from_name": name, "server": server, "room": room, "reason": reason, "body": body
            }))
            .collect::<Vec<_>>());
        let masked: Vec<(String, i64)> = sqlx::query_as("SELECT global_id, SUM(count) FROM chat_masked WHERE day >= ? GROUP BY global_id ORDER BY 2 DESC LIMIT 20")
            .bind(now / 86_400 - 6)
            .fetch_all(&self.pool)
            .await?;
        let mut most = Vec::with_capacity(masked.len());
        for (who, count) in masked {
            most.push(json!({ "identity": who, "name": self.identity_name(&who).await?, "count": count }));
        }
        v["masked"] = json!(most);
        Ok(v)
    }

    /// Marks a message's open reports dealt with: `status` deleted, resolved or dismissed.
    async fn close_chat_reports(&self, message: i64, status: &str, admin: &str, note: &str) -> sqlx::Result<u64> {
        Ok(
            sqlx::query("UPDATE chat_reports SET status = ?, resolved_by = ?, resolved_at = ?, note = ? WHERE message_id = ? AND status = 'open'")
                .bind(status)
                .bind(admin)
                .bind(identity::now())
                .bind(note)
                .bind(message)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }
}

async fn overview(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.moderation_overview().await.map_or_else(internal, ok)
}

#[derive(Deserialize)]
struct ReportAction {
    action: String,
    #[serde(default)]
    note: String,
}

async fn handle_report(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(message): Path<i64>, Json(req): Json<ReportAction>) -> Response {
    let s = match if req.action == "delete" {
        c.recent(&headers, &client).await
    } else {
        c.full(&headers, &client).await
    } {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !c.has_chat() {
        return no_chat();
    }
    let note = req.note.trim();
    if !crate::reports::printable(note, 300, false) {
        return fail(StatusCode::BAD_REQUEST, "a note is up to 300 printable characters");
    }
    let status = match req.action.as_str() {
        "delete" => "deleted",
        "resolve" => "resolved",
        "dismiss" => "dismissed",
        _ => return fail(StatusCode::BAD_REQUEST, "an action is delete, resolve or dismiss"),
    };
    let sender: Option<(String, String)> = match sqlx::query_as("SELECT sender_name, room FROM chat_messages WHERE id = ? AND deletes IS NULL")
        .bind(message)
        .fetch_optional(&c.pool)
        .await
    {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let Some((sender_name, room)) = sender else {
        return fail(StatusCode::NOT_FOUND, "no such message");
    };
    // The reports first: with none open (another admin, or an old tab, got there first) the
    // message stays, where it went before with nothing in the audit log.
    match c.close_chat_reports(message, status, &s.username, note).await {
        Ok(0) => fail(StatusCode::CONFLICT, "no open report on that message"),
        Ok(_) => {
            // Already gone (its sender deleted it) is fine: the reports are closed all the same.
            if status == "deleted" {
                if let Err(e) = c.chat_delete(message, None, &format!("admin:{}", s.username)).await {
                    return internal(e);
                }
            }
            let detail = format!(
                "message {message} by {sender_name} in {room}{}",
                if note.is_empty() { String::new() } else { format!(": {note}") }
            );
            c.audit(&s.username, Some(&client), &format!("chat: report {status}"), &detail).await;
            c.publish(Event::Moderation);
            ok(json!({ "status": status }))
        }
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct MuteRequest {
    identity: String,
    room: String,
    /// None: for good.
    #[serde(default)]
    hours: Option<i64>,
    #[serde(default)]
    reason: String,
}

async fn mute(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<MuteRequest>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !c.has_chat() {
        return no_chat();
    }
    if !identity::is_global_id(&req.identity) {
        return fail(StatusCode::BAD_REQUEST, "not an identity");
    }
    if !MUTE_ROOMS.contains(&req.room.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "a room is global, direct, lobby or * (everywhere)");
    }
    let reason = req.reason.trim();
    if !crate::reports::printable(reason, 200, false) {
        return fail(StatusCode::BAD_REQUEST, "a reason is up to 200 printable characters");
    }
    let now = identity::now();
    let until = match req.hours {
        None => None,
        Some(h) if (1..=24 * 3650).contains(&h) => Some(now + h * 3600),
        Some(_) => return fail(StatusCode::BAD_REQUEST, "a mute is 1 hour to ten years, or for good"),
    };
    let done = sqlx::query("INSERT OR REPLACE INTO chat_mutes (global_id, room, until, by_admin, reason, at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(&req.identity)
        .bind(&req.room)
        .bind(until)
        .bind(&s.username)
        .bind(reason)
        .bind(now)
        .execute(&c.pool)
        .await;
    if let Err(e) = done {
        return internal(e);
    }
    let name = c.identity_name(&req.identity).await.unwrap_or_default();
    let detail = format!(
        "{name} ({}) in {}{}{}",
        identity::short(&req.identity),
        if req.room == "*" { "every room" } else { req.room.as_str() },
        req.hours.map_or(" for good".to_string(), |h| format!(" for {h} h")),
        if reason.is_empty() { String::new() } else { format!(": {reason}") }
    );
    c.audit(&s.username, Some(&client), "chat: muted", &detail).await;
    c.publish(Event::Moderation);
    ok(json!({ "until": until }))
}

async fn unmute(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path((who, room)): Path<(String, String)>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !c.has_chat() {
        return no_chat();
    }
    match sqlx::query("DELETE FROM chat_mutes WHERE global_id = ? AND room = ?")
        .bind(&who)
        .bind(&room)
        .execute(&c.pool)
        .await
    {
        Ok(done) if done.rows_affected() > 0 => {
            let name = c.identity_name(&who).await.unwrap_or_default();
            c.audit(&s.username, Some(&client), "chat: unmuted", &format!("{name} ({}) in {room}", identity::short(&who)))
                .await;
            c.publish(Event::Moderation);
            ok(json!({}))
        }
        Ok(_) => fail(StatusCode::NOT_FOUND, "no such mute"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct Settings {
    allow: Vec<String>,
    extra: Vec<String>,
    discord_invites: bool,
}

/// A list of words as kept: one a line, trimmed, lower case, no repeats.
fn word_list(words: &[String]) -> Result<String, &'static str> {
    let mut out: Vec<String> = Vec::new();
    for w in words {
        let w = w.trim().to_lowercase();
        if w.is_empty() {
            continue;
        }
        if w.chars().count() > MAX_WORD || w.chars().any(|ch| ch.is_control() || crate::hidden_char(ch)) {
            return Err("a word or name is up to 64 printable characters");
        }
        if !out.contains(&w) {
            out.push(w);
        }
    }
    if out.len() > MAX_WORDS {
        return Err("a list has at most 2000 words");
    }
    Ok(out.join("\n"))
}

async fn set_settings(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<Settings>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (allow, extra) = match (word_list(&req.allow), word_list(&req.extra)) {
        (Ok(a), Ok(e)) => (a, e),
        (Err(why), _) | (_, Err(why)) => return fail(StatusCode::BAD_REQUEST, why),
    };
    let (old_allow, old_extra, old_invites) = c.chat_settings().await;
    for (key, value) in [
        ("moderation.allow", allow.as_str()),
        ("moderation.extra", extra.as_str()),
        ("chat.discord_invites", if req.discord_invites { "1" } else { "0" }),
    ] {
        if let Err(e) = c.set_setting(key, value).await {
            return internal(e);
        }
    }
    c.forget_chat_rules();
    let (new_allow, new_extra, _) = c.chat_settings().await;
    let changes = |old: &[String], new: &[String]| {
        let added: Vec<&str> = new.iter().filter(|w| !old.contains(w)).map(String::as_str).collect();
        let removed: Vec<&str> = old.iter().filter(|w| !new.contains(w)).map(String::as_str).collect();
        format!("+[{}] -[{}]", added.join(", "), removed.join(", "))
    };
    let detail = format!(
        "allowed {}; extra {}; Discord invites {}{}",
        changes(&old_allow, &new_allow),
        changes(&old_extra, &new_extra),
        if req.discord_invites { "on" } else { "off" },
        if old_invites == req.discord_invites { "" } else { " (changed)" },
    );
    c.audit(&s.username, Some(&client), "moderation: rules changed", &detail).await;
    c.publish(Event::Moderation);
    c.moderation_overview().await.map_or_else(internal, ok)
}
