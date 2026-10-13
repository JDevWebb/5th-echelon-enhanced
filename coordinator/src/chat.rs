//! Chat on the community network (docs/chat.md). The coordinator keeps every room's messages;
//! a member server passes its players' messages on (`POST /v1/chat/send`), and fetches what's
//! for its players by waiting for it (`GET /v1/chat/wait`, answered at once when there's
//! something newer, else after [`WAIT`]: plain HTTP, through any proxy).
//!
//! - **Global:** everyone on the network. **Direct:** two friends, whatever their servers.
//!   **Lobby:** the players in one game session, on one server (which says who's in it).
//! - Only the community network's coordinator (`--chat`), only member servers, and only those
//!   running the signed release: a server whose program doesn't match it can't pass messages.
//! - A server speaks only for players linked to it, by identity; names come from those links.
//! - The rules (moderation) are checked here again, whatever the server did: links refused (a
//!   Discord invite excepted), slurs and hate masked, mutes, and how fast a player may post.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::Duration;

use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

use crate::fail;
use crate::internal;
use crate::ok;
use crate::parse;
use crate::Answer;
use crate::Coordinator;
use crate::Shared;

/// How long a server's wait for new messages is held open.
pub const WAIT: Duration = Duration::from_secs(25);
/// The most messages in one answer.
const MAX_ANSWER: i64 = 200;
/// What's kept (seconds, and the newest so many).
const GLOBAL_KEEP: i64 = 7 * 86_400;
const GLOBAL_LAST: i64 = 500;
const DIRECT_KEEP: i64 = 30 * 86_400;
const DIRECT_LAST: i64 = 200;
const LOBBY_KEEP: i64 = 2 * 3600;
const REFUSED_KEEP: i64 = 7 * 86_400;
const REPORTS_KEEP: i64 = 90 * 86_400;
const MASKED_KEEP: i64 = 30 * 86_400;
/// How fast a player may post: so many in each span, the same message not again within
/// [`SAME_AGAIN`], and in Global one Discord invite each [`INVITE_EVERY`].
const PER_10S: usize = 5;
const PER_MINUTE: usize = 30;
const SAME_AGAIN: i64 = 30;
const INVITE_EVERY: i64 = 600;
/// The longest report reason.
const MAX_WHY: usize = 300;
/// How long the rules from the settings are kept before they're read again.
const RULES_FOR: i64 = 60;

/// Chat's state beside the database: who waits for messages, and players' recent posts.
#[derive(Default)]
pub struct ChatState {
    notify: tokio::sync::Notify,
    recent: std::sync::Mutex<HashMap<String, Recent>>,
    rules: std::sync::Mutex<Option<(i64, moderation::Rules)>>,
}

#[derive(Default)]
struct Recent {
    /// When, and the message as compared (lower case, trimmed).
    sent: VecDeque<(i64, String)>,
    invite_at: Option<i64>,
}

/// The direct conversation between two identities.
pub fn direct_conv(a: &str, b: &str) -> String {
    if a < b {
        format!("direct:{a}|{b}")
    } else {
        format!("direct:{b}|{a}")
    }
}

/// A refusal the server passes on to its player, by code and as the player reads it.
fn refused(code: &str, says: &str) -> Answer {
    ok(json!({ "refused": code, "says": says }))
}

impl Coordinator {
    /// Whether this coordinator runs chat (the community network's: `--chat`).
    pub fn has_chat(&self) -> bool {
        self.chat_on.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Runs chat here.
    pub fn enable_chat(&self) {
        self.chat_on.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Lets member servers whose program isn't checked against the signed release pass chat (a
    /// test network built from a branch).
    pub fn chat_from_unverified(&self) {
        self.chat_unverified.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether `server` may pass chat: chat runs here, and its program is the signed release.
    ///
    /// The release check is the one the server reports about itself (its updater's, as root):
    /// it keeps an honest server running something else out of chat, but a server changed on
    /// purpose could say anything. What keeps strangers out is membership: only servers joined
    /// with the network's token, which its admins hand out, are heard at all.
    async fn chat_gate(&self, server: &str) -> Result<(), Answer> {
        if !self.has_chat() {
            return Err(fail(StatusCode::NOT_FOUND, "no chat on this network"));
        }
        if self.chat_unverified.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let status: Option<String> = sqlx::query_scalar("SELECT update_status FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
            .flatten();
        let state = status
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["verify"]["state"].as_str().map(str::to_string));
        if state.as_deref() == Some("verified") {
            Ok(())
        } else {
            Err(fail(StatusCode::FORBIDDEN, "chat is only for servers running the signed release"))
        }
    }

    /// The network's chat rules: its settings' allow list, extra words and whether Discord
    /// invites may be sent, read again every [`RULES_FOR`].
    pub async fn chat_rules(&self) -> moderation::Rules {
        let now = identity::now();
        if let Some((at, rules)) = self.chat.rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            if now - at < RULES_FOR {
                return rules.clone();
            }
        }
        let (allow, extra, invites) = self.chat_settings().await;
        let rules = moderation::Rules::new(allow.iter().map(String::as_str), extra.iter().map(String::as_str), invites);
        *self.chat.rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((now, rules.clone()));
        rules
    }

    /// The rules' settings changed: read again at once.
    pub(crate) fn forget_chat_rules(&self) {
        *self.chat.rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// The settings the rules are made of: allowed words and names, extra words to mask, and
    /// whether Discord invites may be sent (yes unless set off).
    pub async fn chat_settings(&self) -> (Vec<String>, Vec<String>, bool) {
        let list = |v: Option<String>| -> Vec<String> { v.unwrap_or_default().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).take(2000).collect() };
        let allow = list(self.setting("moderation.allow").await.ok().flatten());
        let extra = list(self.setting("moderation.extra").await.ok().flatten());
        let invites = self.setting("chat.discord_invites").await.ok().flatten().as_deref() != Some("0");
        (allow, extra, invites)
    }

    /// Whether two identities are friends, with no block either way.
    async fn chat_friends(&self, a: &str, b: &str) -> sqlx::Result<bool> {
        let (x, y) = if a < b { (a, b) } else { (b, a) };
        let friends: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM friendships WHERE a = ? AND b = ? AND friends = 1")
            .bind(x)
            .bind(y)
            .fetch_one(&self.pool)
            .await?;
        let blocked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blocks WHERE blocked = 1 AND ((from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?))")
            .bind(a)
            .bind(b)
            .bind(b)
            .bind(a)
            .fetch_one(&self.pool)
            .await?;
        Ok(friends > 0 && blocked == 0)
    }

    /// The mute on `who` for `room` (or everywhere) now: until when (None: for good).
    async fn chat_muted(&self, who: &str, room: &str, now: i64) -> sqlx::Result<Option<Option<i64>>> {
        let rows: Vec<(Option<i64>,)> = sqlx::query_as("SELECT until FROM chat_mutes WHERE global_id = ? AND room IN (?, '*') AND (until IS NULL OR until > ?)")
            .bind(who)
            .bind(room)
            .bind(now)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(u,)| u).reduce(|a, b| match (a, b) {
            (None, _) | (_, None) => None,
            (Some(x), Some(y)) => Some(x.max(y)),
        }))
    }

    /// Notes a post by `who` and says how long to wait, if it's too soon or a repeat.
    fn chat_pace(&self, who: &str, text: &str, now: i64) -> Result<(), (&'static str, String)> {
        let same: String = text.trim().to_lowercase();
        let mut all = self.chat.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if all.len() > 20_000 {
            all.retain(|_, r| r.sent.back().is_some_and(|(at, _)| now - at < 120));
        }
        let r = all.entry(who.to_string()).or_default();
        while r.sent.front().is_some_and(|(at, _)| now - at >= 60) {
            r.sent.pop_front();
        }
        if r.sent.iter().any(|(at, t)| now - at < SAME_AGAIN && *t == same) {
            return Err(("duplicate", "You just sent that.".into()));
        }
        let last_10s: Vec<i64> = r.sent.iter().filter(|(at, _)| now - at < 10).map(|(at, _)| *at).collect();
        if last_10s.len() >= PER_10S {
            let wait = 10 - (now - last_10s[0]);
            return Err(("slow", format!("Slow down: try again in {} s.", wait.max(1))));
        }
        if r.sent.len() >= PER_MINUTE {
            let wait = 60 - (now - r.sent.front().map_or(now, |(at, _)| *at));
            return Err(("slow", format!("Slow down: try again in {} s.", wait.max(1))));
        }
        r.sent.push_back((now, same));
        Ok(())
    }

    /// Notes a Discord invite in Global by `who`, if it's been [`INVITE_EVERY`] since their last.
    fn chat_invite_due(&self, who: &str, now: i64) -> bool {
        let mut all = self.chat.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let r = all.entry(who.to_string()).or_default();
        if r.invite_at.is_some_and(|at| now - at < INVITE_EVERY) {
            return false;
        }
        r.invite_at = Some(now);
        true
    }

    /// A message as servers get it.
    async fn chat_message_json(&self, row: &Row) -> Value {
        let server_name: Option<String> = sqlx::query_scalar("SELECT json_extract(listing, '$.name') FROM servers WHERE id = ?")
            .bind(&row.server)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .flatten();
        json!({
            "id": row.id,
            "room": row.room,
            "conv": row.conv,
            "at": row.at,
            "from": row.sender,
            "from_name": row.sender_name,
            "server": row.server,
            "server_name": server_name.unwrap_or_default(),
            "to": row.recipient,
            "body": row.body,
            "deletes": row.deletes,
        })
    }

    /// Messages after `after` for `server`'s players: Global, its own lobbies, and direct
    /// messages to or from players linked to it.
    async fn chat_for_server(&self, server: &str, after: i64) -> sqlx::Result<Vec<Row>> {
        sqlx::query_as(
            "SELECT id, room, conv, at, sender, sender_name, server, recipient, body, deletes FROM chat_messages m
             WHERE m.id > ?1 AND m.deleted_at IS NULL AND (
                 m.room = 'global'
                 OR (m.room = 'lobby' AND substr(m.conv, 1, length(?2) + 7) = 'lobby:' || ?2 || ':')
                 OR (m.room = 'direct' AND (EXISTS (SELECT 1 FROM links l WHERE l.server_id = ?2 AND l.global_id = m.sender)
                                         OR EXISTS (SELECT 1 FROM links l WHERE l.server_id = ?2 AND l.global_id = m.recipient))))
             ORDER BY m.id LIMIT ?3",
        )
        .bind(after)
        .bind(server)
        .bind(MAX_ANSWER)
        .fetch_all(&self.pool)
        .await
    }

    /// Deletes message `id` (only if `sender`'s, when given), by `by` (the sender, or an admin
    /// as `admin:<name>`): hidden, and a deletion sent after it so servers and players drop it.
    pub(crate) async fn chat_delete(&self, id: i64, sender: Option<&str>, by: &str) -> sqlx::Result<bool> {
        let now = identity::now();
        let mut tx = self.pool.begin().await?;
        let done = sqlx::query("UPDATE chat_messages SET deleted_at = ?, deleted_by = ? WHERE id = ? AND deleted_at IS NULL AND deletes IS NULL AND (?4 IS NULL OR sender = ?4)")
            .bind(now)
            .bind(by)
            .bind(id)
            .bind(sender)
            .execute(&mut *tx)
            .await?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO chat_messages (room, conv, at, sender, sender_name, server, recipient, body, deletes)
             SELECT room, conv, ?, sender, sender_name, server, recipient, '', id FROM chat_messages WHERE id = ?",
        )
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.chat.notify.notify_waiters();
        Ok(true)
    }

    /// Whether `who`, a player of `server`, may read conversation `conv`.
    fn chat_may_read(server: &str, who: &str, conv: &str) -> bool {
        if conv == "global" {
            return true;
        }
        if let Some(pair) = conv.strip_prefix("direct:") {
            return pair.split('|').any(|p| p == who);
        }
        conv.strip_prefix("lobby:").and_then(|rest| rest.split_once(':')).is_some_and(|(s, _)| s == server)
    }

    /// Prunes chat past what's kept (the hourly rollup), and an identity's chat once it has no
    /// links left (its accounts unlinked or deleted everywhere): its messages, the direct
    /// conversations with it, what it read, its masked counts.
    pub(crate) async fn prune_chat(&self) -> sqlx::Result<()> {
        let now = identity::now();
        // A message reported and not dealt with yet stays: neither its sender unlinking everywhere
        // nor its age takes the evidence away from the admins. For [`REPORTS_KEEP`] at most, the
        // time a report is kept: a report anyone can make mustn't keep a message for ever.
        let held_since = now - REPORTS_KEEP;
        let reported = format!("id NOT IN (SELECT message_id FROM chat_reports WHERE status = 'open' AND at >= {held_since})");
        sqlx::query("DELETE FROM chat_reports WHERE status = 'open' AND at < ?")
            .bind(held_since)
            .execute(&self.pool)
            .await?;
        let gone = "NOT IN (SELECT global_id FROM links)";
        for sql in [
            format!("DELETE FROM chat_messages WHERE (sender {gone} OR (recipient IS NOT NULL AND recipient {gone})) AND {reported}"),
            format!("DELETE FROM chat_reads WHERE global_id {gone}"),
            format!("DELETE FROM chat_masked WHERE global_id {gone}"),
        ] {
            sqlx::query(&sql).execute(&self.pool).await?;
        }
        for (sql, keep) in [
            (format!("DELETE FROM chat_messages WHERE room = 'global' AND at < ? AND {reported}"), GLOBAL_KEEP),
            (format!("DELETE FROM chat_messages WHERE room = 'direct' AND at < ? AND {reported}"), DIRECT_KEEP),
            (format!("DELETE FROM chat_messages WHERE room = 'lobby' AND at < ? AND {reported}"), LOBBY_KEEP),
            ("DELETE FROM chat_refused WHERE at < ?".to_string(), REFUSED_KEEP),
            ("DELETE FROM chat_reports WHERE status != 'open' AND resolved_at < ?".to_string(), REPORTS_KEEP),
            ("DELETE FROM chat_masked WHERE day * 86400 < ?".to_string(), MASKED_KEEP),
            ("DELETE FROM chat_mutes WHERE until IS NOT NULL AND until < ?".to_string(), 0),
        ] {
            sqlx::query(&sql).bind(now - keep).execute(&self.pool).await?;
        }
        sqlx::query(&format!(
            "DELETE FROM chat_messages WHERE room = 'global' AND {reported} AND id NOT IN (SELECT id FROM chat_messages WHERE room = 'global' ORDER BY id DESC LIMIT ?)"
        ))
        .bind(GLOBAL_LAST)
        .execute(&self.pool)
        .await?;
        sqlx::query(&format!(
            "DELETE FROM chat_messages WHERE room = 'direct' AND {reported} AND id IN (
                 SELECT id FROM (SELECT id, ROW_NUMBER() OVER (PARTITION BY conv ORDER BY id DESC) AS n FROM chat_messages WHERE room = 'direct') WHERE n > ?)"
        ))
        .bind(DIRECT_LAST)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    room: String,
    conv: String,
    at: i64,
    sender: String,
    sender_name: String,
    server: String,
    recipient: Option<String>,
    body: String,
    /// For a deletion: the message it deletes.
    deletes: Option<i64>,
}

#[derive(Deserialize)]
struct SendRequest {
    room: String,
    /// The player's identity.
    from: String,
    /// For a direct message: the friend's identity.
    #[serde(default)]
    to: Option<String>,
    /// For a lobby: the game session's id on this server.
    #[serde(default)]
    session: Option<u32>,
    text: String,
}

/// `POST /v1/chat/send` (a member server, for one of its players): the message as sent, or why
/// it wasn't.
pub(crate) async fn send(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    if let Err(e) = c.chat_gate(&server).await {
        return e;
    }
    let r: SendRequest = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !identity::is_global_id(&r.from) {
        return fail(StatusCode::BAD_REQUEST, "not an identity");
    }
    // Its name as linked here: a server can't post as someone else, nor under another name.
    let name: Option<String> = match sqlx::query_scalar("SELECT username FROM links WHERE server_id = ? AND global_id = ?")
        .bind(&server)
        .bind(&r.from)
        .fetch_optional(&c.pool)
        .await
    {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    let Some(name) = name else {
        return fail(StatusCode::FORBIDDEN, "not one of this server's players");
    };
    // Only under a name that's theirs across the network: a link may carry a name another
    // player holds (the server then asks its player to rename), and chat shows senders by name.
    match c.owner(&identity::name_key(&name)).await {
        Ok(owner) if owner.as_deref() == Some(r.from.as_str()) => {}
        Ok(_) => return refused("name_conflict", "Your name is another player's on the network: rename to chat."),
        Err(e) => return internal(e),
    }
    let (room, conv, recipient) = match r.room.as_str() {
        "global" => ("global", "global".to_string(), None),
        "direct" => {
            let Some(to) = r.to.filter(|t| identity::is_global_id(t) && *t != r.from) else {
                return fail(StatusCode::BAD_REQUEST, "a direct message needs another player's identity");
            };
            match c.chat_friends(&r.from, &to).await {
                Ok(true) => {}
                Ok(false) => return refused("not_friends", "You can only message friends."),
                Err(e) => return internal(e),
            }
            ("direct", direct_conv(&r.from, &to), Some(to))
        }
        "lobby" => {
            let Some(session) = r.session else {
                return fail(StatusCode::BAD_REQUEST, "a lobby message needs its session");
            };
            ("lobby", format!("lobby:{server}:{session}"), None)
        }
        _ => return fail(StatusCode::BAD_REQUEST, "room is global, direct or lobby"),
    };
    let now = identity::now();
    match c.chat_muted(&r.from, room, now).await {
        Ok(Some(until)) => {
            let says = until.map_or_else(
                || "You've been muted.".to_string(),
                |u| format!("You've been muted for {} more minutes.", (u - now + 59) / 60),
            );
            return refused("muted", &says);
        }
        Ok(None) => {}
        Err(e) => return internal(e),
    }
    let rules = c.chat_rules().await;
    let checked = match rules.check_message(&r.text) {
        Ok(checked) => checked,
        Err(why) => {
            if matches!(why, moderation::Refused::Link | moderation::Refused::Invite) {
                let _ = sqlx::query("INSERT INTO chat_refused (at, sender, sender_name, server, room, reason, body) VALUES (?, ?, ?, ?, ?, ?, ?)")
                    .bind(now)
                    .bind(&r.from)
                    .bind(&name)
                    .bind(&server)
                    .bind(room)
                    .bind(format!("{why:?}").to_lowercase())
                    .bind(r.text.chars().take(moderation::MAX_MESSAGE).collect::<String>())
                    .execute(&c.pool)
                    .await;
            }
            let code = match why {
                moderation::Refused::Empty => "empty",
                moderation::Refused::TooLong => "too_long",
                moderation::Refused::Link => "link",
                moderation::Refused::Invite => "invite",
            };
            return refused(code, why.says());
        }
    };
    if let Err((code, says)) = c.chat_pace(&r.from, &checked.text, now) {
        return refused(code, &says);
    }
    if checked.invite.is_some() && room == "global" && !c.chat_invite_due(&r.from, now) {
        return refused("invite_wait", "One Discord invite every 10 minutes in Global.");
    }
    let id: i64 = match sqlx::query_scalar(
        "INSERT INTO chat_messages (room, conv, at, sender, sender_name, server, recipient, body, masked) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(room)
    .bind(&conv)
    .bind(now)
    .bind(&r.from)
    .bind(&name)
    .bind(&server)
    .bind(&recipient)
    .bind(&checked.text)
    .bind(i64::from(checked.masked))
    .fetch_one(&c.pool)
    .await
    {
        Ok(id) => id,
        Err(e) => return internal(e),
    };
    if checked.masked > 0 {
        let _ = sqlx::query("INSERT INTO chat_masked (global_id, day, count) VALUES (?, ?, ?) ON CONFLICT (global_id, day) DO UPDATE SET count = count + excluded.count")
            .bind(&r.from)
            .bind(now / 86_400)
            .bind(i64::from(checked.masked))
            .execute(&c.pool)
            .await;
    }
    c.chat.notify.notify_waiters();
    let row = Row {
        id,
        room: room.into(),
        conv,
        at: now,
        sender: r.from,
        sender_name: name,
        server,
        recipient,
        body: checked.text,
        deletes: None,
    };
    ok(json!({ "message": c.chat_message_json(&row).await }))
}

#[derive(Deserialize)]
pub(crate) struct WaitQuery {
    /// The last message the server has (0: none, from the first); -1 asks where to start (the
    /// answer's `last`, at once).
    after: i64,
}

/// `GET /v1/chat/wait?after=ID` (a member server): the messages for its players after `after`,
/// at once if there are any, else when one comes or after [`WAIT`].
pub(crate) async fn wait(State(c): State<Shared>, headers: HeaderMap, Query(q): Query<WaitQuery>) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    if let Err(e) = c.chat_gate(&server).await {
        return e;
    }
    // A server starting out: from now on (history is asked for separately).
    if q.after < 0 {
        let last: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM chat_messages").fetch_one(&c.pool).await.unwrap_or(0);
        return ok(json!({ "messages": [], "last": last }));
    }
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let notified = c.chat.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let rows = match c.chat_for_server(&server, q.after).await {
            Ok(rows) => rows,
            Err(e) => return internal(e),
        };
        if !rows.is_empty() {
            let last = rows.last().map_or(q.after, |r| r.id);
            let mut messages = Vec::with_capacity(rows.len());
            for row in &rows {
                messages.push(c.chat_message_json(row).await);
            }
            return ok(json!({ "messages": messages, "last": last }));
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return ok(json!({ "messages": [], "last": q.after }));
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct HistoryQuery {
    /// The player asking (an identity linked to the server).
    #[serde(rename = "for")]
    who: String,
    conv: String,
    /// Older than this message (0: the newest).
    #[serde(default)]
    before: i64,
    #[serde(default)]
    limit: Option<i64>,
}

/// Checks a request on behalf of a player: the server's, the player linked to it.
async fn for_player(c: &Coordinator, headers: &HeaderMap, who: &str) -> Result<String, Answer> {
    let server = c.server(headers).await?;
    c.chat_gate(&server).await?;
    if !identity::is_global_id(who) || !c.linked(&server, who).await.map_err(internal)? {
        return Err(fail(StatusCode::FORBIDDEN, "not one of this server's players"));
    }
    Ok(server)
}

/// `GET /v1/chat/history?for=ID&conv=..&before=..&limit=..`: a conversation's messages the
/// player may read, newest last.
pub(crate) async fn history(State(c): State<Shared>, headers: HeaderMap, Query(q): Query<HistoryQuery>) -> Answer {
    let server = match for_player(&c, &headers, &q.who).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    if !Coordinator::chat_may_read(&server, &q.who, &q.conv) {
        return fail(StatusCode::FORBIDDEN, "not a conversation of this player's");
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let before = if q.before > 0 { q.before } else { i64::MAX };
    let rows: Vec<Row> = match sqlx::query_as(
        "SELECT id, room, conv, at, sender, sender_name, server, recipient, body, deletes FROM chat_messages
         WHERE conv = ? AND id < ? AND deleted_at IS NULL AND deletes IS NULL ORDER BY id DESC LIMIT ?",
    )
    .bind(&q.conv)
    .bind(before)
    .bind(limit)
    .fetch_all(&c.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => return internal(e),
    };
    let mut messages = Vec::with_capacity(rows.len());
    for row in rows.iter().rev() {
        messages.push(c.chat_message_json(row).await);
    }
    ok(json!({ "messages": messages }))
}

#[derive(Deserialize)]
pub(crate) struct ForQuery {
    #[serde(rename = "for")]
    who: String,
}

/// `GET /v1/chat/conversations?for=ID`: the player's direct conversations, newest first, with
/// the friend's identity and name, the last message, and how many they haven't read.
pub(crate) async fn conversations(State(c): State<Shared>, headers: HeaderMap, Query(q): Query<ForQuery>) -> Answer {
    if let Err(e) = for_player(&c, &headers, &q.who).await {
        return e;
    }
    let rows: Vec<(String, i64, i64, String, String, i64)> = match sqlx::query_as(
        "SELECT m.conv, MAX(m.id), MAX(m.at),
                CASE WHEN m.sender = ?1 THEN m.recipient ELSE m.sender END AS other,
                COALESCE((SELECT sender_name FROM chat_messages x WHERE x.conv = m.conv AND x.sender != ?1 ORDER BY x.id DESC LIMIT 1), ''),
                (SELECT COUNT(*) FROM chat_messages u WHERE u.conv = m.conv AND u.sender != ?1 AND u.deleted_at IS NULL AND u.deletes IS NULL
                    AND u.id > COALESCE((SELECT up_to FROM chat_reads r WHERE r.global_id = ?1 AND r.conv = m.conv), 0))
         FROM chat_messages m
         WHERE m.room = 'direct' AND (m.sender = ?1 OR m.recipient = ?1) AND m.deleted_at IS NULL AND m.deletes IS NULL
         GROUP BY m.conv ORDER BY MAX(m.id) DESC LIMIT 100",
    )
    .bind(&q.who)
    .fetch_all(&c.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => return internal(e),
    };
    let list: Vec<Value> = rows
        .into_iter()
        .map(|(conv, last, at, other, name, unread)| json!({ "conv": conv, "last": last, "at": at, "with": other, "with_name": name, "unread": unread }))
        .collect();
    ok(json!({ "conversations": list }))
}

#[derive(Deserialize)]
struct ReadRequest {
    #[serde(rename = "for")]
    who: String,
    conv: String,
    up_to: i64,
}

/// `POST /v1/chat/read`: the player read `conv` up to a message.
pub(crate) async fn read(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let r: ReadRequest = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let server = match for_player(&c, &headers, &r.who).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    if !Coordinator::chat_may_read(&server, &r.who, &r.conv) {
        return fail(StatusCode::FORBIDDEN, "not a conversation of this player's");
    }
    match sqlx::query("INSERT INTO chat_reads (global_id, conv, up_to) VALUES (?, ?, ?) ON CONFLICT (global_id, conv) DO UPDATE SET up_to = MAX(up_to, excluded.up_to)")
        .bind(&r.who)
        .bind(&r.conv)
        .bind(r.up_to)
        .execute(&c.pool)
        .await
    {
        Ok(_) => ok(json!({})),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct MessageRequest {
    #[serde(rename = "for")]
    who: String,
    id: i64,
    #[serde(default)]
    why: String,
}

/// `POST /v1/chat/delete`: the player deletes a message of their own.
pub(crate) async fn delete(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let r: MessageRequest = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = for_player(&c, &headers, &r.who).await {
        return e;
    }
    match c.chat_delete(r.id, Some(&r.who), &r.who).await {
        Ok(true) => ok(json!({})),
        Ok(false) => fail(StatusCode::NOT_FOUND, "no such message of this player's"),
        Err(e) => internal(e),
    }
}

/// `POST /v1/chat/report`: the player reports a message they can read, and why.
pub(crate) async fn report(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let r: MessageRequest = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let server = match for_player(&c, &headers, &r.who).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let conv: Option<String> = match sqlx::query_scalar("SELECT conv FROM chat_messages WHERE id = ? AND deletes IS NULL")
        .bind(r.id)
        .fetch_optional(&c.pool)
        .await
    {
        Ok(conv) => conv,
        Err(e) => return internal(e),
    };
    let Some(conv) = conv.filter(|conv| Coordinator::chat_may_read(&server, &r.who, conv)) else {
        return fail(StatusCode::NOT_FOUND, "no such message for this player");
    };
    let why: String = r.why.chars().filter(|ch| !ch.is_control()).take(MAX_WHY).collect();
    match sqlx::query("INSERT OR IGNORE INTO chat_reports (message_id, reporter, reporter_server, why, at) VALUES (?, ?, ?, ?, ?)")
        .bind(r.id)
        .bind(&r.who)
        .bind(&server)
        .bind(why.trim())
        .bind(identity::now())
        .execute(&c.pool)
        .await
    {
        Ok(_) => {
            tracing::info!("Chat: message {} in {conv} reported", r.id);
            c.publish(crate::admin::live::Event::Moderation);
            ok(json!({}))
        }
        Err(e) => internal(e),
    }
}
