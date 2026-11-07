//! Chat (the egui launcher's chat.rs, through the shared library): Global, the lobby and
//! conversations with friends, through the player's server. A ticker does what egui's frame does
//! (the status now and then, a room's history when it first shows, marking read); the feed waits
//! on the server for new messages on a thread of its own; the shared `Rooms` sorts them.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use launcher::chat as c;
use launcher::chat::Rooms;
use launcher::chat::Tab;
use launcher::network::PlayerAction;
use serde::Serialize;
use server_api::chat::ChatFriend;
use server_api::chat::Conversation;
use server_api::chat::Message;
use server_api::chat::Room;
use server_api::chat::StatusResponse;
use setup::config::Config;
use tauri::Emitter;
use tauri::Manager;

use crate::commands::changed;
use crate::commands::Launcher;
use crate::tauri;

#[derive(Default)]
pub struct Chat {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The server and account chat is for (`chat::feed_key`): another, and it starts again.
    key: Option<String>,
    /// The feed's thread, stopped when this goes (with the state).
    feed_stop: Option<FeedStop>,
    trouble: Option<String>,
    status: Option<StatusResponse>,
    status_error: Option<String>,
    status_at: Option<Instant>,
    friends: Vec<ChatFriend>,
    online: launcher::flow::Friends,
    rooms: Rooms,
    lobby_of: u32,
    conversations: Vec<Conversation>,
    history_asked: HashSet<String>,
    marked: std::collections::BTreeMap<String, i64>,
    /// What the page shows: the tab, the conversation open, and whether Chat shows at all.
    tab: Option<Tab>,
    with: Option<String>,
    showing: bool,
}

impl Chat {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Stops the feed's thread when dropped.
struct FeedStop(Arc<AtomicBool>);

impl Drop for FeedStop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// The game's settings now, for chat's calls (on the community network only).
fn config(app: &tauri::AppHandle) -> Option<Config> {
    if !c::available() {
        return None;
    }
    app.state::<Launcher>().lock().game.as_ref().map(|g| (*g.cfg).clone())
}

/// Starts chat's ticker, from start to the end.
pub fn start(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        tick(&app);
        std::thread::sleep(Duration::from_secs(1));
    });
}

fn tick(app: &tauri::AppHandle) {
    let chat = app.state::<Chat>();
    let cfg = config(app);
    let key = cfg.as_ref().and_then(c::feed_key);
    {
        let mut st = chat.lock();
        if key != st.key {
            let (tab, with, showing) = (st.tab, st.with.clone(), st.showing);
            *st = State {
                key: key.clone(),
                tab,
                with,
                showing,
                ..State::default()
            };
        }
    }
    let Some(cfg) = cfg.filter(|_| key.is_some()) else { return };

    // The status: whether chat is open to the player, their lobby; their friends; who's online.
    let due = {
        let st = chat.lock();
        let every = if st.showing { c::STATUS_WHILE_OPEN } else { c::STATUS_EVERY };
        st.status_at.is_none_or(|t| t.elapsed() >= every)
    };
    if due {
        chat.lock().status_at = Some(Instant::now());
        let read = launcher::task::guarded(|| -> Result<_, String> {
            let status = c::with_api(&cfg, |api| Box::pin(api.status()))?;
            let friends = if status.available {
                c::with_api(&cfg, |api| Box::pin(api.friends())).unwrap_or_default()
            } else {
                Vec::new()
            };
            Ok((status, friends, launcher::flow::friends(&cfg).unwrap_or_default()))
        });
        let mut st = chat.lock();
        match read {
            Ok((status, friends, online)) => {
                if status.lobby != st.lobby_of {
                    st.rooms.lobby.clear();
                    st.lobby_of = status.lobby;
                    st.history_asked.remove("lobby");
                }
                st.status = Some(status);
                st.friends = friends;
                st.online = online;
                st.status_error = None;
            }
            Err(e) => st.status_error = Some(e),
        }
        drop(st);
        let _ = app.emit("chat", ());
    }
    let open = chat.lock().status.as_ref().is_some_and(|s| s.available);
    if !open {
        return;
    }

    // The feed, once chat is open to the player.
    if chat.lock().feed_stop.is_none() {
        start_feed(app, &cfg);
    }

    // The conversations, when a direct message came.
    if std::mem::take(&mut chat.lock().rooms.conversations_stale) {
        match launcher::task::guarded(|| c::with_api(&cfg, |api| Box::pin(api.conversations()))) {
            Ok(list) => {
                let mut st = chat.lock();
                for conv in &list {
                    st.rooms.names.insert(conv.with.clone(), conv.with_name.clone());
                    if conv.unread > 0 && st.with.as_deref() != Some(conv.with.as_str()) {
                        st.rooms.unseen_direct.insert(conv.with.clone(), conv.unread);
                    }
                }
                st.conversations = list;
                drop(st);
                let _ = app.emit("chat", ());
            }
            Err(e) => chat.lock().trouble = Some(e),
        }
    }

    // What the screen shows is seen; a room's history, once, when it's first shown.
    let (showing, tab, with, lobby_of) = {
        let st = chat.lock();
        (st.showing, st.tab.unwrap_or(Tab::Global), st.with.clone(), st.lobby_of)
    };
    if !showing {
        return;
    }
    let mark = {
        let mut st = chat.lock();
        match tab {
            Tab::Global | Tab::Lobby => {
                st.rooms.unseen_mentions = 0;
                None
            }
            Tab::Messages => with.clone().and_then(|with| {
                st.rooms.unseen_direct.remove(&with);
                let last = st.rooms.direct.get(&with).and_then(|l| l.iter().rev().find(|m| !m.mine)).map(|m| m.id)?;
                st.marked.get(&with).is_none_or(|m| *m < last).then(|| {
                    st.marked.insert(with.clone(), last);
                    (with, last)
                })
            }),
        }
    };
    if let Some((with, last)) = mark {
        let _ = launcher::task::guarded(|| c::with_api(&cfg, |api| Box::pin(api.mark_read(with.clone(), last))));
    }
    let what = match (tab, &with) {
        (Tab::Global, _) => Some(("global".to_string(), Room::Global, String::new())),
        (Tab::Lobby, _) if lobby_of != 0 => Some(("lobby".to_string(), Room::Lobby, String::new())),
        (Tab::Messages, Some(with)) => Some((with.clone(), Room::Direct, with.clone())),
        _ => None,
    };
    let Some((what, room, with)) = what.filter(|(w, ..)| !chat.lock().history_asked.contains(w)) else {
        return;
    };
    chat.lock().history_asked.insert(what.clone());
    match launcher::task::guarded(|| c::with_api(&cfg, |api| Box::pin(api.history(room, with.clone(), 0)))) {
        Ok(messages) => {
            let mut st = chat.lock();
            let list = match what.as_str() {
                "global" => &mut st.rooms.global,
                "lobby" => &mut st.rooms.lobby,
                with => st.rooms.direct.entry(with.to_string()).or_default(),
            };
            for m in messages {
                c::keep(list, m);
            }
            drop(st);
            let _ = app.emit("chat", ());
        }
        Err(e) => {
            chat.lock().history_asked.remove(&what);
            app.state::<Launcher>().lock().notices.error(format!("Couldn't read chat: {e}"));
            changed(app);
        }
    }
}

/// The feed's thread, and one taking what it hands over into the rooms.
fn start_feed(app: &tauri::AppHandle, cfg: &Config) {
    let (tx, events) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    app.state::<Chat>().lock().feed_stop = Some(FeedStop(Arc::clone(&stop)));
    let (cfg, feed_stop) = (cfg.clone(), Arc::clone(&stop));
    std::thread::Builder::new()
        .name("chat-feed".into())
        .spawn(move || c::run_feed(cfg, feed_stop, tx, || {}))
        .ok();
    let app = app.clone();
    std::thread::spawn(move || {
        while let Ok(event) = events.recv() {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let chat = app.state::<Chat>();
            let mut st = chat.lock();
            let attention = match event {
                c::FeedEvent::Messages(messages, kept) => {
                    let (tab, with, showing) = (st.tab.unwrap_or(Tab::Global), st.with.clone(), st.showing);
                    st.rooms.take(messages, kept, |t| showing && tab == t, with.as_deref())
                }
                c::FeedEvent::Trouble(e) => {
                    st.trouble = Some(e);
                    false
                }
                c::FeedEvent::Back => {
                    st.trouble = None;
                    false
                }
            };
            drop(st);
            // Named, or a message from a friend, while the launcher is behind another window.
            if attention {
                if let Some(window) = app.get_webview_window("main") {
                    if !window.is_focused().unwrap_or(true) {
                        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
                    }
                }
            }
            let _ = app.emit("chat", ());
        }
    });
}

/// A message's text in parts: plain, and a Discord invite in its own form as a link.
#[derive(Serialize)]
pub struct Part {
    text: String,
    link: bool,
}

fn parts(text: &str) -> Vec<Part> {
    let mut out = Vec::new();
    let mut at = 0;
    for (start, end) in moderation::discord_invites(text) {
        if start > at {
            out.push(Part {
                text: text[at..start].to_string(),
                link: false,
            });
        }
        out.push(Part {
            text: text[start..end].to_string(),
            link: true,
        });
        at = end;
    }
    if at < text.len() {
        out.push(Part {
            text: text[at..].to_string(),
            link: false,
        });
    }
    out
}

#[derive(Serialize)]
pub struct MessageView {
    id: i64,
    time: String,
    from: String,
    from_name: String,
    server: String,
    parts: Vec<Part>,
    mine: bool,
    mentions_me: bool,
    /// The sender is the player's friend (to message them from Global).
    friend: bool,
}

#[derive(Serialize)]
pub struct RoomEntry {
    with: String,
    name: String,
    what: &'static str,
    unread: u32,
}

#[derive(Serialize)]
pub struct OnlineView {
    id: String,
    name: String,
    activity: Option<String>,
    friend: bool,
    asked: bool,
}

/// The Chat screen, for the page.
#[derive(Serialize)]
pub struct ChatView {
    /// "no-account", "opening", "error", "closed" or "open"; why, for the last three.
    state: &'static str,
    why: Option<String>,
    trouble: Option<String>,
    read_only: bool,
    may_post_global: bool,
    global_wait: String,
    lobby: bool,
    /// The open room's messages, oldest first.
    messages: Vec<MessageView>,
    /// The conversations, and friends to start one with.
    conversations: Vec<RoomEntry>,
    online: Vec<OnlineView>,
    /// The name of the friend the open conversation is with.
    with_name: Option<String>,
    unread: u32,
    rules: &'static str,
    max_message: usize,
}

/// The Chat screen as it is, for `tab` ("global", "lobby", "messages") and `with` (the
/// conversation open), `showing` while it shows.
pub fn chat(app: tauri::AppHandle, state: tauri::State<Chat>, tab: String, with: Option<String>, showing: bool) -> ChatView {
    let tab = match tab.as_str() {
        "lobby" => Tab::Lobby,
        "messages" => Tab::Messages,
        _ => Tab::Global,
    };
    let mut st = state.lock();
    let reopened = showing && !st.showing;
    st.tab = Some(tab);
    st.with = with.clone();
    st.showing = showing;
    if reopened {
        // Opened: the status again soon.
        st.status_at = st.status_at.filter(|t| t.elapsed() < c::STATUS_WHILE_OPEN);
    }
    let has_account = config(&app).as_ref().and_then(c::feed_key).is_some();
    let (state_word, why) = match (&st.status, &st.status_error) {
        _ if !has_account => ("no-account", None),
        (None, Some(e)) => ("error", Some(format!("Chat isn't reachable: {e}"))),
        (None, None) => ("opening", None),
        (Some(s), _) if !s.available => (
            "closed",
            Some(if s.why_not.is_empty() {
                "Chat isn't open on this server.".to_string()
            } else {
                s.why_not.clone()
            }),
        ),
        _ => ("open", None),
    };
    let clock = setup::clock::Clock::local();
    let friends: HashSet<&str> = st.friends.iter().map(|f| f.identity.as_str()).collect();
    let list: &[Message] = match (tab, &with) {
        (Tab::Global, _) => &st.rooms.global,
        (Tab::Lobby, _) => &st.rooms.lobby,
        (Tab::Messages, Some(with)) => st.rooms.direct.get(with).map(Vec::as_slice).unwrap_or_default(),
        (Tab::Messages, None) => &[],
    };
    let messages = list
        .iter()
        .map(|m| MessageView {
            id: m.id,
            time: clock.clock_time(m.at),
            from: m.from.clone(),
            from_name: m.from_name.clone(),
            server: m.server_name.clone(),
            parts: parts(&m.body),
            mine: m.mine,
            mentions_me: m.mentions_me,
            friend: friends.contains(m.from.as_str()),
        })
        .collect();
    let mut conversations: Vec<RoomEntry> = st
        .conversations
        .iter()
        .map(|conv| RoomEntry {
            with: conv.with.clone(),
            name: if conv.with_name.is_empty() {
                identity::short(&conv.with)
            } else {
                conv.with_name.clone()
            },
            what: "Direct",
            unread: st.rooms.unseen_direct.get(&conv.with).copied().unwrap_or(0),
        })
        .collect();
    conversations.extend(
        st.friends
            .iter()
            .filter(|f| !st.conversations.iter().any(|conv| conv.with == f.identity))
            .map(|f| RoomEntry {
                with: f.identity.clone(),
                name: f.name.clone(),
                what: if f.online { "Friend · online" } else { "Friend" },
                unread: 0,
            }),
    );
    let online = st
        .online
        .online
        .iter()
        .map(|f| OnlineView {
            id: f.id.clone(),
            name: f.name.clone(),
            activity: f.activity.clone(),
            friend: true,
            asked: false,
        })
        .chain(st.online.others.iter().map(|p| OnlineView {
            id: p.id.clone(),
            name: p.name.clone(),
            activity: p.activity.clone(),
            friend: false,
            asked: p.asked,
        }))
        .collect();
    // A friend not written to yet has no conversation, so no name kept for it: their name from
    // the friends list, not their id.
    let with_name = with.as_ref().map(|w| {
        st.rooms
            .names
            .get(w)
            .filter(|n| !n.is_empty())
            .cloned()
            .or_else(|| st.friends.iter().find(|f| &f.identity == w).map(|f| f.name.clone()))
            .unwrap_or_else(|| identity::short(w))
    });
    let status = st.status.clone().unwrap_or_default();
    ChatView {
        state: state_word,
        why,
        trouble: st.trouble.clone(),
        read_only: status.read_only,
        may_post_global: status.may_post_global,
        global_wait: status.global_wait,
        lobby: status.lobby != 0,
        messages,
        conversations,
        online,
        with_name,
        unread: st.rooms.unread(),
        rules: c::RULES_LINE,
        max_message: moderation::MAX_MESSAGE,
    }
}

/// Unseen mentions and direct messages (the side menu's dot), and the latest in Global (the Play
/// screen's Squad tab).
#[derive(Serialize)]
pub struct Glance {
    unread: u32,
    latest: Option<(String, String)>,
}

pub fn chat_glance(state: tauri::State<Chat>) -> Glance {
    let st = state.lock();
    Glance {
        unread: st.rooms.unread(),
        latest: st.rooms.global.last().map(|m| (m.from_name.clone(), m.body.clone())),
    }
}

/// What the rules say of `text` as it's typed: why it can't be sent, or that words will be masked.
#[derive(Serialize)]
pub struct Checked {
    why: Option<&'static str>,
    masked: bool,
}

pub fn chat_check(text: String) -> Checked {
    match moderation::Rules::new([], [], true).check_message(&text) {
        Ok(checked) => Checked {
            why: None,
            masked: checked.masked > 0,
        },
        Err(moderation::Refused::Empty) => Checked { why: None, masked: false },
        Err(why) => Checked {
            why: Some(why.says()),
            masked: false,
        },
    }
}

/// Sends `text` to the room (`to`: the friend, for a direct message). Why not, when the server
/// refused it.
pub fn chat_send(app: tauri::AppHandle, state: tauri::State<Chat>, room: String, to: String, text: String) -> Result<(), String> {
    let room = match room.as_str() {
        "lobby" => Room::Lobby,
        "messages" => Room::Direct,
        _ => Room::Global,
    };
    moderation::Rules::new([], [], true).check_message(&text).map_err(|why| why.says().to_string())?;
    let cfg = config(&app).ok_or("Chat is on the community network.")?;
    let text = text.trim().to_string();
    let sent = launcher::task::guarded(|| c::with_api(&cfg, |api| Box::pin(api.send(room, to.clone(), text.clone()))))?;
    match sent.message {
        Some(m) => {
            let mut st = state.lock();
            let (tab, with, showing) = (st.tab.unwrap_or(Tab::Global), st.with.clone(), st.showing);
            st.rooms.take(vec![m], false, |t| showing && tab == t, with.as_deref());
            drop(st);
            let _ = app.emit("chat", ());
            Ok(())
        }
        None => Err(if sent.says.is_empty() { "Not sent.".into() } else { sent.says }),
    }
}

fn said(app: &tauri::AppHandle, result: Result<String, String>) {
    let launcher = app.state::<Launcher>();
    let mut inner = launcher.lock();
    match result {
        Ok(done) => inner.notices.info(done),
        Err(e) => inner.notices.error(e),
    }
    drop(inner);
    changed(app);
}

/// Deletes one of the player's own messages.
pub fn chat_delete(app: tauri::AppHandle, id: i64) {
    let Some(cfg) = config(&app) else { return };
    let result = launcher::task::guarded(|| c::with_api(&cfg, |api| Box::pin(api.delete(id))).map(|()| "Deleted.".to_string()));
    said(&app, result);
}

/// Reports a message to the admins, with why.
pub fn chat_report(app: tauri::AppHandle, id: i64, why: String) {
    let Some(cfg) = config(&app) else { return };
    let why: String = why.trim().chars().take(300).collect();
    let result =
        launcher::task::guarded(|| c::with_api(&cfg, |api| Box::pin(api.report(id, why.clone()))).map(|()| "Reported. The admins see it with what came before it.".to_string()));
    said(&app, result);
}

/// Does something to a player: by account id (who's online here), or by identity (chat's
/// senders), never by name alone. "request", "invite" or "block".
pub fn chat_act(app: tauri::AppHandle, action: String, id: String, identity: String, name: String) {
    let action = match action.as_str() {
        "request" => PlayerAction::Request,
        "invite" => PlayerAction::Invite,
        "block" => PlayerAction::Block,
        _ => return,
    };
    let Some(cfg) = config(&app) else { return };
    let result = launcher::task::guarded(|| launcher::flow::player_action(&cfg, action, &id, &identity, &name));
    said(&app, result);
}
