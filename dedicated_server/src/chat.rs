//! Chat for this server's players (docs/chat.md): the network's coordinator runs it and keeps
//! the messages; this server passes its players' messages on, waits on the coordinator for
//! what's for them, and hands each player what they may see.
//!
//! On only when the coordinator says it runs chat (in its pulse answers, with the network's
//! rules); a server on its own or on another network answers `Unimplemented`.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use reqwest::Method;
use serde_json::json;
use serde_json::Value;
use server_api::chat::chat_server::Chat;
use server_api::chat::ChatFriend;
use server_api::chat::Conversation;
use server_api::chat::ConversationsRequest;
use server_api::chat::ConversationsResponse;
use server_api::chat::Empty;
use server_api::chat::FriendsResponse;
use server_api::chat::HistoryRequest;
use server_api::chat::HistoryResponse;
use server_api::chat::MarkReadRequest;
use server_api::chat::Message;
use server_api::chat::MessageRequest;
use server_api::chat::Room;
use server_api::chat::SendRequest;
use server_api::chat::SendResponse;
use server_api::chat::StatusRequest;
use server_api::chat::StatusResponse;
use server_api::chat::WaitRequest;
use server_api::chat::WaitResponse;
use slog::warn;
use slog::Logger;
use tonic::Request;
use tonic::Response;
use tonic::Status;

use crate::storage::Storage;

/// How long a player's wait for messages is held open.
const WAIT: Duration = Duration::from_secs(25);
/// The messages kept here for players' waits (each is the coordinator's too).
const KEPT: usize = 2000;
/// The most messages in one answer to a player.
const MAX_ANSWER: usize = 200;
/// A new account posts in Global after this long signed in here, or after a match.
const GLOBAL_AFTER: i64 = 30 * 60;
/// The coordinator unheard for this long: rooms read only.
const AWAY_AFTER: u64 = 90;

/// The coordinator runs chat (its last pulse answer said so).
static ON: AtomicBool = AtomicBool::new(false);
/// The newest message id taken from the coordinator.
static LAST: AtomicI64 = AtomicI64::new(-1);
static RELAY_STARTED: AtomicBool = AtomicBool::new(false);

fn kept() -> &'static Mutex<VecDeque<Kept>> {
    static KEPT_MESSAGES: std::sync::OnceLock<Mutex<VecDeque<Kept>>> = std::sync::OnceLock::new();
    KEPT_MESSAGES.get_or_init(Mutex::default)
}

fn arrived() -> &'static tokio::sync::Notify {
    static NOTIFY: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    NOTIFY.get_or_init(tokio::sync::Notify::new)
}

/// A message as the coordinator sends it.
#[derive(Debug, Clone, PartialEq)]
struct Kept {
    id: i64,
    room: Room,
    conv: String,
    at: i64,
    from: String,
    from_name: String,
    server_name: String,
    to: String,
    body: String,
    /// For a deletion: the message it deletes (0: none).
    deletes: i64,
}

impl Kept {
    fn from_json(v: &Value) -> Option<Self> {
        let text = |k: &str, max: usize| hooks_text(v[k].as_str().unwrap_or_default(), max);
        let room = match v["room"].as_str()? {
            "global" => Room::Global,
            "direct" => Room::Direct,
            "lobby" => Room::Lobby,
            _ => return None,
        };
        Some(Self {
            id: v["id"].as_i64()?,
            room,
            conv: v["conv"].as_str()?.to_string(),
            at: v["at"].as_i64().unwrap_or_default(),
            from: v["from"].as_str()?.to_string(),
            from_name: text("from_name", 32),
            server_name: text("server_name", 64),
            to: v["to"].as_str().unwrap_or_default().to_string(),
            body: v["body"].as_str().unwrap_or_default().chars().take(moderation::MAX_MESSAGE).collect(),
            deletes: v["deletes"].as_i64().unwrap_or_default(),
        })
    }

    fn to_message(&self, me: &str, my_name: &str) -> Message {
        let mention = format!("@{}", my_name.to_lowercase());
        Message {
            id: self.id,
            room: self.room.into(),
            at: self.at,
            from: self.from.clone(),
            from_name: self.from_name.clone(),
            server_name: self.server_name.clone(),
            to: self.to.clone(),
            body: self.body.clone(),
            mine: self.from == me,
            mentions_me: !my_name.is_empty() && self.from != me && self.body.to_lowercase().contains(&mention),
            deletes: self.deletes,
        }
    }
}

/// Text from elsewhere, for one line: without control, zero-width or bidi characters, cut short.
fn hooks_text(s: &str, max: usize) -> String {
    moderation::clean(s, max)
}

/// Takes a pulse's answer: the network's rules, which then apply to names (and messages) here
/// too, and whether its coordinator runs chat.
pub fn from_pulse(answer: &Value, logger: &Logger) {
    let Some(chat) = answer["rules"].as_object() else {
        ON.store(false, Ordering::Relaxed);
        return;
    };
    let list = |k: &str| -> Vec<String> {
        chat.get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(2000)
            .map(str::to_string)
            .collect()
    };
    let (allow, extra) = (list("allow"), list("extra"));
    let invites = chat.get("discord_invites").and_then(Value::as_bool).unwrap_or(true);
    crate::api::set_rules(moderation::Rules::new(allow.iter().map(String::as_str), extra.iter().map(String::as_str), invites));
    let on = answer["chat"].as_bool().unwrap_or_default();
    if ON.swap(on, Ordering::Relaxed) != on {
        slog::info!(logger, "Chat: {} (the network's coordinator runs it)", if on { "on" } else { "off" });
    }
}

/// Starts waiting on the coordinator for messages (once): from the newest it has on.
pub fn start_relay(logger: Logger) {
    if RELAY_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        let mut failures = 0u32;
        loop {
            if !ON.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
            let after = LAST.load(Ordering::Relaxed);
            match crate::federation::chat_call(Method::GET, &format!("/v1/chat/wait?after={after}"), None).await {
                Ok(answer) => {
                    failures = 0;
                    take(&answer);
                }
                Err(e) => {
                    failures += 1;
                    if failures == 1 || failures % 30 == 0 {
                        warn!(logger, "Chat: waiting on the coordinator failed (trying again): {e}");
                    }
                    tokio::time::sleep(Duration::from_secs(u64::from(failures.min(6)) * 5)).await;
                }
            }
        }
    });
}

/// Keeps a wait's answer's messages and wakes the players waiting.
fn take(answer: &Value) {
    let messages: Vec<Kept> = answer["messages"].as_array().into_iter().flatten().filter_map(Kept::from_json).collect();
    if let Some(last) = answer["last"].as_i64() {
        LAST.fetch_max(last, Ordering::Relaxed);
    }
    if messages.is_empty() {
        return;
    }
    let mut all = kept().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for m in messages {
        // A deletion: the message goes, and the deletion is kept for players who have it.
        if m.deletes != 0 {
            all.retain(|k| k.id != m.deletes);
        }
        if all.back().is_none_or(|b| b.id < m.id) {
            all.push_back(m);
        }
    }
    while all.len() > KEPT {
        all.pop_front();
    }
    drop(all);
    arrived().notify_waiters();
}

/// What a player is, as chat sees them.
struct Caller {
    user: u32,
    me: String,
    name: String,
    lobby: Option<u32>,
}

pub struct MyChat {
    pub logger: Logger,
    pub storage: Arc<Storage>,
}

fn internal(e: impl std::fmt::Display) -> Status {
    Status::internal(e.to_string())
}

impl MyChat {
    /// The caller, if chat is open to them (on here, their account linked to an identity).
    async fn caller<T>(&self, request: &Request<T>) -> Result<Caller, Status> {
        if !ON.load(Ordering::Relaxed) {
            return Err(Status::unimplemented("no chat on this server's network"));
        }
        let user = request
            .metadata()
            .get("user_id")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| Status::unauthenticated("Not signed in"))?;
        let me = self
            .storage
            .global_id_of(user)
            .await
            .map_err(internal)?
            .ok_or_else(|| Status::failed_precondition("Chat needs your account linked to your identity: set it up again in the launcher."))?;
        // The storage's synchronous calls run their own runtime: not on this one's threads.
        let storage = Arc::clone(&self.storage);
        let name = tokio::task::spawn_blocking(move || storage.find_username_by_user_id(user).ok().flatten().unwrap_or_default())
            .await
            .map_err(internal)?;
        let lobby = self.storage.current_room(user).await.map_err(internal)?;
        Ok(Caller { user, me, name, lobby })
    }

    /// Whether `c` may post in Global: 30 minutes signed in here, or a match played. Not when
    /// their record can't be read (`None` is only for a record that says so).
    async fn global_wait(&self, c: &Caller) -> Option<String> {
        let (storage, user) = (Arc::clone(&self.storage), c.user);
        let record = match tokio::task::spawn_blocking(move || storage.player_records(Some(&[user]))).await {
            Ok(Ok(mut records)) => records.pop(),
            Ok(Err(e)) => {
                warn!(self.logger, "Chat: player {user}'s record couldn't be read: {e}");
                None
            }
            Err(e) => {
                warn!(self.logger, "Chat: player {user}'s record couldn't be read: {e}");
                None
            }
        };
        let Some(record) = record else {
            return Some("Global can't check your account right now; try again in a minute.".into());
        };
        if record.play_seconds >= GLOBAL_AFTER || record.matches > 0 {
            return None;
        }
        let left = (GLOBAL_AFTER - record.play_seconds + 59) / 60;
        Some(format!("New accounts can post in Global after 30 minutes signed in, or a match: {left} more minutes."))
    }

    /// The conversation a room means for `c`.
    fn conv(c: &Caller, room: Room, with: &str) -> Result<String, Status> {
        match room {
            Room::Global => Ok("global".into()),
            Room::Direct if identity::is_global_id(with) && with != c.me => Ok(direct_conv(&c.me, with)),
            Room::Direct => Err(Status::invalid_argument("a direct conversation needs a friend's identity")),
            Room::Lobby => c
                .lobby
                .map(|session| format!("lobby:{}:{session}", crate::federation::server_id()))
                .ok_or_else(|| Status::failed_precondition("You're not in a lobby or match.")),
        }
    }
}

/// The direct conversation between two identities, as the coordinator names it.
fn direct_conv(a: &str, b: &str) -> String {
    if a < b {
        format!("direct:{a}|{b}")
    } else {
        format!("direct:{b}|{a}")
    }
}

/// Whether `c` may see `m`, given who they've blocked or been blocked by.
fn visible(c: &Caller, m: &Kept, blocked: &[String]) -> bool {
    if blocked.contains(&m.from) {
        return false;
    }
    match m.room {
        Room::Global => true,
        Room::Direct => m.from == c.me || m.to == c.me,
        Room::Lobby => c.lobby.is_some_and(|s| m.conv == format!("lobby:{}:{s}", crate::federation::server_id())),
    }
}

#[tonic::async_trait]
impl Chat for MyChat {
    async fn status(&self, request: Request<StatusRequest>) -> Result<Response<StatusResponse>, Status> {
        if !ON.load(Ordering::Relaxed) {
            return Ok(Response::new(StatusResponse {
                available: false,
                why_not: "This server's network has no chat.".into(),
                ..StatusResponse::default()
            }));
        }
        let c = match self.caller(&request).await {
            Ok(c) => c,
            Err(e) if e.code() == tonic::Code::FailedPrecondition => {
                return Ok(Response::new(StatusResponse {
                    available: false,
                    why_not: e.message().to_string(),
                    ..StatusResponse::default()
                }))
            }
            Err(e) => return Err(e),
        };
        let global_wait = self.global_wait(&c).await;
        let sessions = self.storage.live_sessions_async().await.unwrap_or_default();
        let in_match = crate::community_api::activity_of(&c.name, &sessions).is_some_and(|a| a.room == "match");
        let away = crate::federation::coordinator_seen_ago().is_none_or(|s| s > AWAY_AFTER);
        Ok(Response::new(StatusResponse {
            available: true,
            why_not: String::new(),
            read_only: away,
            may_post_global: global_wait.is_none(),
            global_wait: global_wait.unwrap_or_default(),
            lobby: c.lobby.unwrap_or(0),
            me: c.me,
            in_match,
        }))
    }

    async fn send(&self, request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        let c = self.caller(&request).await?;
        if !crate::rate_limit::game_requests().check(c.user) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        let r = request.into_inner();
        let room = Room::try_from(r.room).map_err(|_| Status::invalid_argument("not a room"))?;
        let refused = |code: &str, says: String| {
            Ok(Response::new(SendResponse {
                message: None,
                refused: code.into(),
                says,
            }))
        };
        if room == Room::Global {
            if let Some(wait) = self.global_wait(&c).await {
                return refused("global_wait", wait);
            }
        }
        // Here first, for a quick answer; the coordinator decides.
        if let Err(why) = crate::api::rules().check_message(&r.text) {
            return refused("rules", why.says().into());
        }
        let mut body = json!({ "from": c.me, "text": r.text });
        match room {
            Room::Global => body["room"] = "global".into(),
            Room::Direct => {
                if !identity::is_global_id(&r.to) {
                    return Err(Status::invalid_argument("a direct message needs a friend's identity"));
                }
                body["room"] = "direct".into();
                body["to"] = r.to.into();
            }
            Room::Lobby => {
                let Some(session) = c.lobby else {
                    return refused("no_lobby", "You're not in a lobby or match.".into());
                };
                body["room"] = "lobby".into();
                body["session"] = session.into();
            }
        }
        let answer = crate::federation::chat_call(Method::POST, "/v1/chat/send", Some(&body)).await.map_err(|e| {
            warn!(self.logger, "Chat: a message from {} couldn't reach the coordinator: {e}", c.user);
            Status::unavailable("Chat can't reach the network right now; try again in a minute.")
        })?;
        if let Some(code) = answer["refused"].as_str() {
            return refused(code, hooks_text(answer["says"].as_str().unwrap_or_default(), 200));
        }
        let message = Kept::from_json(&answer["message"]).ok_or_else(|| Status::internal("no message in the answer"))?;
        Ok(Response::new(SendResponse {
            message: Some(message.to_message(&c.me, &c.name)),
            refused: String::new(),
            says: String::new(),
        }))
    }

    async fn wait(&self, request: Request<WaitRequest>) -> Result<Response<WaitResponse>, Status> {
        let c = self.caller(&request).await?;
        if !crate::rate_limit::polls().check(c.user) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        let r = request.into_inner();
        let after = r.after;
        let hold = match r.wait_seconds {
            0 => WAIT,
            s => Duration::from_secs(u64::from(s)).min(WAIT),
        };
        let blocked = self.storage.blocked_global_ids(c.user).await.map_err(internal)?;
        let deadline = tokio::time::Instant::now() + hold;
        loop {
            let notified = arrived().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let (messages, newest) = {
                let all = kept().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let newest = all.back().map_or(after, |m| m.id.max(after));
                let mine: Vec<Message> = all
                    .iter()
                    .filter(|m| m.id > after && visible(&c, m, &blocked))
                    .take(MAX_ANSWER)
                    .map(|m| m.to_message(&c.me, &c.name))
                    .collect();
                (mine, newest)
            };
            if !messages.is_empty() {
                let last = messages.last().map_or(newest, |m| m.id);
                return Ok(Response::new(WaitResponse { messages, last }));
            }
            // Nothing for this player in what came: past it all next time.
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(Response::new(WaitResponse { messages: vec![], last: newest }));
            }
        }
    }

    async fn history(&self, request: Request<HistoryRequest>) -> Result<Response<HistoryResponse>, Status> {
        let c = self.caller(&request).await?;
        if !crate::rate_limit::polls().check(c.user) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        let r = request.into_inner();
        let room = Room::try_from(r.room).map_err(|_| Status::invalid_argument("not a room"))?;
        let conv = Self::conv(&c, room, &r.with)?;
        let path = format!(
            "/v1/chat/history?for={}&conv={}&before={}&limit={}",
            crate::federation::query_value(&c.me),
            crate::federation::query_value(&conv),
            r.before.max(0),
            r.limit.clamp(1, 100)
        );
        let answer = crate::federation::chat_call(Method::GET, &path, None)
            .await
            .map_err(|e| Status::unavailable(e.to_string()))?;
        let blocked = self.storage.blocked_global_ids(c.user).await.map_err(internal)?;
        let messages = answer["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Kept::from_json)
            .filter(|m| !blocked.contains(&m.from))
            .map(|m| m.to_message(&c.me, &c.name))
            .collect();
        Ok(Response::new(HistoryResponse { messages }))
    }

    async fn conversations(&self, request: Request<ConversationsRequest>) -> Result<Response<ConversationsResponse>, Status> {
        let c = self.caller(&request).await?;
        if !crate::rate_limit::polls().check(c.user) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        let path = format!("/v1/chat/conversations?for={}", crate::federation::query_value(&c.me));
        let answer = crate::federation::chat_call(Method::GET, &path, None)
            .await
            .map_err(|e| Status::unavailable(e.to_string()))?;
        let conversations = answer["conversations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                Some(Conversation {
                    with: v["with"].as_str()?.to_string(),
                    with_name: hooks_text(v["with_name"].as_str().unwrap_or_default(), 32),
                    last: v["last"].as_i64().unwrap_or_default(),
                    at: v["at"].as_i64().unwrap_or_default(),
                    unread: u32::try_from(v["unread"].as_i64().unwrap_or_default().clamp(0, 9999)).unwrap_or_default(),
                })
            })
            .collect();
        Ok(Response::new(ConversationsResponse { conversations }))
    }

    async fn mark_read(&self, request: Request<MarkReadRequest>) -> Result<Response<Empty>, Status> {
        let c = self.caller(&request).await?;
        let r = request.into_inner();
        let room = Room::try_from(r.room).map_err(|_| Status::invalid_argument("not a room"))?;
        let conv = Self::conv(&c, room, &r.with)?;
        crate::federation::chat_call(Method::POST, "/v1/chat/read", Some(&json!({ "for": c.me, "conv": conv, "up_to": r.up_to })))
            .await
            .map_err(|e| Status::unavailable(e.to_string()))?;
        Ok(Response::new(Empty {}))
    }

    async fn delete(&self, request: Request<MessageRequest>) -> Result<Response<Empty>, Status> {
        let c = self.caller(&request).await?;
        let r = request.into_inner();
        crate::federation::chat_call(Method::POST, "/v1/chat/delete", Some(&json!({ "for": c.me, "id": r.id })))
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;
        // Gone here too, for waits to come.
        kept().lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|m| m.id != r.id);
        Ok(Response::new(Empty {}))
    }

    async fn friends(&self, request: Request<Empty>) -> Result<Response<FriendsResponse>, Status> {
        let c = self.caller(&request).await?;
        let friends = self
            .storage
            .friends_of(c.user)
            .await
            .map_err(internal)?
            .into_iter()
            .filter_map(|p| {
                Some(ChatFriend {
                    identity: p.global_id?,
                    name: p.username,
                    online: p.is_online,
                })
            })
            .collect();
        Ok(Response::new(FriendsResponse { friends }))
    }

    async fn report(&self, request: Request<MessageRequest>) -> Result<Response<Empty>, Status> {
        let c = self.caller(&request).await?;
        if !crate::rate_limit::game_requests().check(c.user) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        let r = request.into_inner();
        crate::federation::chat_call(Method::POST, "/v1/chat/report", Some(&json!({ "for": c.me, "id": r.id, "why": hooks_text(&r.why, 300) })))
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;
        Ok(Response::new(Empty {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept_message(id: i64, room: &str, conv: &str, from: &str, to: &str) -> Value {
        json!({ "id": id, "room": room, "conv": conv, "at": 1, "from": from, "from_name": "Kiwi\u{202e}", "server_name": "EU", "to": to, "body": "hi @Tank" })
    }

    #[test]
    fn a_player_sees_only_what_is_theirs() {
        let me = Caller {
            user: 1,
            me: "me".into(),
            name: "Tank".into(),
            lobby: Some(7),
        };
        let lobby = format!("lobby:{}:7", crate::federation::server_id());
        let other_lobby = format!("lobby:{}:8", crate::federation::server_id());
        let see = |v: Value, blocked: &[String]| visible(&me, &Kept::from_json(&v).unwrap(), blocked);
        assert!(see(kept_message(1, "global", "global", "kiwi", ""), &[]));
        assert!(!see(kept_message(1, "global", "global", "kiwi", ""), &["kiwi".into()]), "blocked");
        assert!(see(kept_message(2, "direct", "direct:kiwi|me", "kiwi", "me"), &[]));
        assert!(!see(kept_message(3, "direct", "direct:a|b", "a", "b"), &[]), "someone else's");
        assert!(see(kept_message(4, "lobby", &lobby, "kiwi", ""), &[]));
        assert!(!see(kept_message(5, "lobby", &other_lobby, "kiwi", ""), &[]), "another lobby");
        let m = Kept::from_json(&kept_message(6, "global", "global", "kiwi", "")).unwrap();
        assert_eq!(m.from_name, "Kiwi", "no control characters");
        let shown = m.to_message("me", "Tank");
        assert!(shown.mentions_me && !shown.mine);
        assert!(Kept::from_json(&json!({ "id": 1, "room": "elsewhere", "conv": "x", "from": "y" })).is_none());
    }
}
