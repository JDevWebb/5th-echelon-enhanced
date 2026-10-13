//! Chat in the game's overlay (docs/chat.md): the same rooms as the launcher's Chat page, through
//! the player's server. A thread waits on the server for messages (8 s at a time: the overlay's
//! calls time out at 10) and keeps what the overlay draws; messages pop up as the launcher's
//! Settings › Notifications says. Text from the server is cut and cleaned like any other.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use hooks_config::text::clip;
use server_api::chat::Room;
use tracing::info;
use tracing::warn;

/// The messages kept per room or conversation.
const KEEP: usize = 150;
/// How often whether chat is open (and the player's lobby, and match) is asked again.
const STATUS_EVERY: Duration = Duration::from_secs(30);
/// Messages arriving this close together share one pop-up ("3 new messages").
const TOAST_GROUP: Duration = Duration::from_secs(4);
/// How long a pop-up shows.
pub const TOAST_FOR: Duration = Duration::from_secs(6);
/// The longest name and server name shown.
const MAX_NAME: usize = 32;
/// The longest message (the server's limit; the moderation crate isn't in the DLL).
const MAX_MESSAGE: usize = 500;

/// A message as the overlay shows it.
#[derive(Debug, Clone)]
pub struct Msg {
    pub id: i64,
    pub room: Room,
    pub from: String,
    pub from_name: String,
    pub server_name: String,
    pub to: String,
    pub body: String,
    pub mine: bool,
    pub mentions_me: bool,
}

impl Msg {
    fn of(m: server_api::chat::Message) -> Self {
        Self {
            id: m.id,
            room: m.room(),
            from_name: clip(&m.from_name, MAX_NAME),
            server_name: clip(&m.server_name, 64),
            body: clip(&m.body, MAX_MESSAGE),
            from: m.from,
            to: m.to,
            mine: m.mine,
            mentions_me: m.mentions_me,
        }
    }

    /// Who a direct message is with, from the player's side.
    pub fn other(&self) -> &str {
        if self.mine {
            &self.to
        } else {
            &self.from
        }
    }
}

/// A pop-up: the latest message, and how many came together.
#[derive(Debug, Clone)]
pub struct Toast {
    pub title: String,
    pub text: String,
    pub count: u32,
    pub at: Instant,
}

/// What the overlay draws.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub available: bool,
    pub why_not: String,
    pub read_only: bool,
    pub may_post_global: bool,
    pub global_wait: String,
    pub lobby: u32,
    pub in_match: bool,
    pub global: Vec<Msg>,
    pub lobby_messages: Vec<Msg>,
    pub direct: BTreeMap<String, Vec<Msg>>,
    /// Names of the people direct conversations are with (and friends), by identity.
    pub names: BTreeMap<String, String>,
    /// Friends who can be messaged: identity, name, online.
    pub friends: Vec<(String, String, bool)>,
    /// Direct messages not seen, by who from; mentions not seen.
    pub unread: BTreeMap<String, u32>,
    pub unseen_mentions: u32,
    pub toast: Option<Toast>,
    /// Why the last message wasn't sent.
    pub refused: Option<String>,
    pub sending: bool,
}

impl View {
    /// What the Chat tab's badge counts.
    pub fn badge(&self) -> u32 {
        self.unseen_mentions + self.unread.values().sum::<u32>()
    }
}

static VIEW: Mutex<Option<View>> = Mutex::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);
/// What the panel shows, so messages there don't pop up or count as unseen: the Chat tab's
/// room, and the open conversation.
static SHOWING: Mutex<Option<(Room, String)>> = Mutex::new(None);

/// The latest view (empty until chat answered).
pub fn view() -> View {
    VIEW.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().unwrap_or_default()
}

/// The pop-up, if one is showing.
pub fn toast() -> Option<Toast> {
    VIEW.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|v| v.toast.clone())
        .filter(|t| t.at.elapsed() < TOAST_FOR)
}

fn update<R>(f: impl FnOnce(&mut View) -> R) -> R {
    let mut guard = VIEW.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f(guard.get_or_insert_with(View::default))
}

/// The panel shows `room` (a direct conversation: with `with`), or (None) not the Chat tab:
/// what shows there is seen.
pub fn showing(room: Option<(Room, String)>) {
    if let Some((room, with)) = &room {
        let mark = update(|v| match room {
            Room::Global | Room::Lobby => {
                v.unseen_mentions = 0;
                None
            }
            Room::Direct => {
                let had = v.unread.remove(with).is_some();
                let last = v.direct.get(with).and_then(|l| l.iter().rev().find(|m| !m.mine)).map(|m| m.id);
                last.filter(|_| had).map(|last| (with.clone(), last))
            }
        });
        if let Some((with, last)) = mark {
            std::thread::spawn(move || {
                if let Err(e) = crate::api::chat_mark_read(&with, last) {
                    warn!("Chat: couldn't mark read: {e}");
                }
            });
        }
    }
    *SHOWING.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = room;
}

/// Adds `m` to `list` by id, once, keeping the newest [`KEEP`].
fn keep(list: &mut Vec<Msg>, m: Msg) {
    match list.binary_search_by_key(&m.id, |x| x.id) {
        Ok(i) => list[i] = m,
        Err(i) => list.insert(i, m),
    }
    if list.len() > KEEP {
        let extra = list.len() - KEEP;
        list.drain(..extra);
    }
}

/// Takes messages in: deletions applied, unseen counted, a pop-up when the settings say
/// (`toasts`: not for the backlog a start brings).
fn take(messages: Vec<server_api::chat::Message>, toasts: bool) {
    let showing = SHOWING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let settings = hooks_config::get().map(|c| c.notifications).unwrap_or_default();
    update(|v| {
        for m in messages {
            if m.deletes != 0 {
                let gone = m.deletes;
                v.global.retain(|x| x.id != gone);
                v.lobby_messages.retain(|x| x.id != gone);
                for list in v.direct.values_mut() {
                    list.retain(|x| x.id != gone);
                }
                continue;
            }
            let m = Msg::of(m);
            let seen = showing
                .as_ref()
                .is_some_and(|(room, with)| *room == m.room && (m.room != Room::Direct || with == m.other()));
            let (setting, room_name) = match m.room {
                Room::Global => (settings.global, "Global"),
                Room::Lobby => (settings.lobby, "Lobby"),
                Room::Direct => (settings.direct, "Message"),
            };
            if !m.mine && !seen {
                if m.room == Room::Direct {
                    *v.unread.entry(m.from.clone()).or_default() += 1;
                } else if m.mentions_me {
                    v.unseen_mentions += 1;
                }
                if toasts && setting.shows(m.mentions_me, v.in_match) {
                    let text = format!("{}: {}", m.from_name, m.body);
                    v.toast = Some(match v.toast.take().filter(|t| t.at.elapsed() < TOAST_GROUP) {
                        Some(t) => Toast {
                            title: format!("{} new messages", t.count + 1),
                            text,
                            count: t.count + 1,
                            at: Instant::now(),
                        },
                        None => Toast {
                            title: room_name.to_string(),
                            text,
                            count: 1,
                            at: Instant::now(),
                        },
                    });
                }
            }
            match m.room {
                Room::Global => keep(&mut v.global, m),
                Room::Lobby => keep(&mut v.lobby_messages, m),
                Room::Direct => {
                    let other = m.other().to_string();
                    if !m.mine {
                        v.names.insert(other.clone(), m.from_name.clone());
                    }
                    keep(v.direct.entry(other).or_default(), m);
                }
            }
        }
    });
}

/// Asks the server whether chat is open to the player (and their lobby, whether in a match),
/// and their friends to message.
fn refresh_status() -> bool {
    match crate::api::chat_status() {
        Ok(s) => {
            let friends = if s.available { crate::api::chat_friends().unwrap_or_default() } else { Vec::new() };
            update(|v| {
                if s.lobby != v.lobby {
                    v.lobby_messages.clear();
                }
                v.available = s.available;
                v.why_not = clip(&s.why_not, 200);
                v.read_only = s.read_only;
                v.may_post_global = s.may_post_global;
                v.global_wait = clip(&s.global_wait, 200);
                v.lobby = s.lobby;
                v.in_match = s.in_match;
                v.friends = friends.into_iter().take(200).map(|f| (f.identity, clip(&f.name, MAX_NAME), f.online)).collect();
                for (identity, name, _) in v.friends.clone() {
                    v.names.entry(identity).or_insert(name);
                }
            });
            s.available
        }
        Err(e) => {
            warn!("Chat: status: {e}");
            false
        }
    }
}

/// Starts the chat thread (once).
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("overlay-chat".into()).spawn(|| {
        let mut status_at: Option<Instant> = None;
        let mut available = false;
        let mut after = 0;
        let mut primed = false;
        let mut lobby_read = 0;
        let mut failures = 0u32;
        loop {
            if status_at.is_none_or(|t| t.elapsed() >= STATUS_EVERY) {
                status_at = Some(Instant::now());
                let was = available;
                available = refresh_status();
                if available && !was {
                    info!("Chat: open");
                    if let Ok(history) = crate::api::chat_history(Room::Global, "") {
                        take(history, false);
                    }
                }
            }
            if !available {
                std::thread::sleep(STATUS_EVERY);
                continue;
            }
            // The lobby's history, once a lobby.
            let lobby = view().lobby;
            if lobby != 0 && lobby != lobby_read {
                lobby_read = lobby;
                if let Ok(history) = crate::api::chat_history(Room::Lobby, "") {
                    take(history, false);
                }
            }
            match crate::api::chat_wait(after) {
                Ok(answer) => {
                    failures = 0;
                    after = answer.last.max(after);
                    // The first answer is what the server kept: no pop-ups for it.
                    take(answer.messages, primed);
                    primed = true;
                }
                Err(e) => {
                    failures += 1;
                    if failures == 1 || failures % 20 == 0 {
                        warn!("Chat: waiting for messages failed: {e}");
                    }
                    std::thread::sleep(Duration::from_secs(u64::from(failures.min(6)) * 5));
                    status_at = None;
                }
            }
        }
    });
    if let Err(e) = spawned {
        warn!("Couldn't start the overlay's chat thread: {e}");
    }
}

/// The server's reason a call failed, as the player reads it.
fn says(e: &crate::api::Error) -> String {
    match e {
        crate::api::Error::GRPCStatus(s) if !s.message().is_empty() => clip(s.message(), 200),
        _ => String::from("Chat can't reach the server right now."),
    }
}

/// Sends a message (a direct one: to `to`); what came of it shows in the view.
pub fn send(room: Room, to: String, text: String) {
    update(|v| {
        v.sending = true;
        v.refused = None;
    });
    std::thread::spawn(move || {
        let result = crate::api::chat_send(room, &to, &text);
        match result {
            Ok(sent) => match sent.message {
                Some(m) => {
                    update(|v| v.sending = false);
                    take(vec![m], false);
                }
                None => update(|v| {
                    v.sending = false;
                    v.refused = Some(clip(if sent.says.is_empty() { "Not sent." } else { &sent.says }, 200));
                }),
            },
            Err(e) => update(|v| {
                v.sending = false;
                v.refused = Some(says(&e));
            }),
        }
    });
}

/// A conversation's history, when it's first opened.
pub fn open_conversation(with: String) {
    let known = update(|v| v.direct.get(&with).is_some_and(|l| !l.is_empty()));
    if known {
        return;
    }
    std::thread::spawn(move || match crate::api::chat_history(Room::Direct, &with) {
        Ok(history) => take(history, false),
        Err(e) => warn!("Chat: history with {with}: {e}"),
    });
}

/// Reports a message to the network's admins.
pub fn report(id: i64, name: String) {
    std::thread::spawn(move || match crate::api::chat_report(id, "") {
        Ok(()) => crate::community::say(format!("Reported {name}'s message to the admins."), false),
        Err(e) => crate::community::say(says(&e), true),
    });
}

/// Deletes one of the player's own messages (for everyone).
pub fn delete(id: i64) {
    std::thread::spawn(move || match crate::api::chat_delete(id) {
        Ok(()) => crate::community::say("Deleted.".into(), false),
        Err(e) => crate::community::say(says(&e), true),
    });
}
