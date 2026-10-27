//! The Chat screen (docs/chat.md): Global (everyone on the community network, with who's
//! online here beside it), Lobby (the players in the player's game session) and Messages
//! (conversations with friends). Through the player's own server, which passes messages to and
//! from the network's coordinator; a feed on its own thread waits on the server for new ones,
//! whatever screen shows. The rules show while typing (links refused, a Discord invite
//! excepted); the server decides, and masks slurs and hate.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use eframe::egui;
use eframe::egui::RichText;
use server_api::chat::ChatFriend;
use server_api::chat::Conversation;
use server_api::chat::Message;
use server_api::chat::Room;
use server_api::chat::StatusResponse;
use setup::config::Config;

use crate::app::App;
use crate::app::Game;
use crate::app::Notices;
use crate::app::Prefs;
use crate::flow;
use crate::network::ChatApi;
use crate::network::PlayerAction;
use crate::task::Slot;
use crate::theme;

/// The messages kept per room or conversation.
const KEEP: usize = 500;
/// How often the status (and who's online) is asked again: while the screen shows, and not.
const STATUS_WHILE_OPEN: Duration = Duration::from_secs(30);
const STATUS_EVERY: Duration = Duration::from_secs(5 * 60);
/// What players read under the box.
const RULES_LINE: &str = "Messages are kept for 7 days and the admins can read them. No links, keep it friendly.";

/// Which part of chat shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Global,
    Lobby,
    Messages,
}

/// What the feed's thread hands over.
enum FeedEvent {
    /// Messages, and whether they're what the server kept before the feed began (the first
    /// answer): shown, but not counted as unseen.
    Messages(Vec<Message>, bool),
    /// The server can't be reached, or said no: why, for the screen.
    Trouble(String),
    Back,
}

/// The feed: a thread waiting on the server for messages, for one account on one server
/// ([`Chat::key`]); stopped when dropped.
struct Feed {
    stop: Arc<AtomicBool>,
    events: mpsc::Receiver<FeedEvent>,
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The server and account chat is for, to tell when they change (the password too: the feed
/// signs in with the one it started with, as `flow::api_session` keys its token).
fn feed_key(cfg: &Config) -> Option<String> {
    use sha2::Digest as _;
    let p = cfg.current_profile().filter(|p| !p.server.is_empty() && p.has_account())?;
    let secret = p.user.secret().unwrap_or_default();
    Some(format!("{}\n{}\n{:x}", p.api_server_url(), p.user.username, sha2::Sha256::digest(secret.as_bytes())))
}

/// A call to chat as the current account; signed in again once if the server stopped taking
/// the kept token.
fn call<T>(cfg: &Config, f: impl Fn(String, String) -> Result<T, crate::network::Error>) -> Result<T, String> {
    let (url, token) = flow::api_session(cfg, false)?;
    match f(url, token) {
        Err(crate::network::Error::InvalidPassword) => {
            let (url, token) = flow::api_session(cfg, true)?;
            f(url, token).map_err(|e| says(&e))
        }
        other => other.map_err(|e| says(&e)),
    }
}

fn says(e: &crate::network::Error) -> String {
    match e {
        crate::network::Error::ServerFailure(s) if !s.is_empty() => s.clone(),
        crate::network::Error::ConnectionFailed => "Can't reach your server right now.".into(),
        other => other.to_string(),
    }
}

/// Runs a chat call on the shared runtime.
fn with_api<T>(cfg: &Config, f: impl Fn(&mut ChatApi) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, crate::network::Error>> + '_>>) -> Result<T, String> {
    call(cfg, |url, token| {
        crate::services::rt().block_on(async {
            let mut api = ChatApi::connect(&url, &token).await?;
            f(&mut api).await
        })
    })
}

/// The feed's thread: waits on the server for messages after the newest it has, again and
/// again, until stopped.
fn run_feed(cfg: Config, stop: Arc<AtomicBool>, tx: mpsc::Sender<FeedEvent>, ctx: egui::Context) {
    let mut after = 0;
    let mut primed = false;
    let mut failures = 0u32;
    while !stop.load(Ordering::Relaxed) {
        match with_api(&cfg, |api| Box::pin(api.wait(after))) {
            Ok(answer) => {
                if failures > 0 {
                    let _ = tx.send(FeedEvent::Back);
                }
                failures = 0;
                after = answer.last.max(after);
                let kept = !std::mem::replace(&mut primed, true);
                if !answer.messages.is_empty() {
                    if tx.send(FeedEvent::Messages(answer.messages, kept)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
            Err(e) => {
                failures += 1;
                if tx.send(FeedEvent::Trouble(e)).is_err() {
                    return;
                }
                ctx.request_repaint();
                // 5 s, 10 s … a minute between tries; stopping is noticed within a second.
                let wait = Duration::from_secs(u64::from(failures.min(12)) * 5);
                let until = Instant::now() + wait;
                while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }
}

/// A report being written for a message.
struct Reporting {
    id: i64,
    name: String,
    why: String,
}

pub struct Chat {
    /// The server and account chat is for (see [`feed_key`]).
    key: Option<String>,
    feed: Option<Feed>,
    trouble: Option<String>,

    status: Option<StatusResponse>,
    status_error: Option<String>,
    status_slot: Slot<Result<(StatusResponse, Vec<ChatFriend>, flow::Friends), String>>,
    status_at: Option<Instant>,
    friends: Vec<ChatFriend>,
    online: flow::Friends,

    tab: Tab,
    /// The open direct conversation: the friend's identity.
    with: Option<String>,
    global: Vec<Message>,
    lobby: Vec<Message>,
    /// The lobby the messages above are from.
    lobby_of: u32,
    direct: BTreeMap<String, Vec<Message>>,
    /// Names of the people direct conversations are with.
    names: BTreeMap<String, String>,
    conversations: Vec<Conversation>,
    conversations_slot: Slot<Result<Vec<Conversation>, String>>,
    conversations_stale: bool,
    /// Rooms and conversations whose history was asked for: Global, Lobby, or an identity.
    history_asked: std::collections::HashSet<String>,
    history_slot: Slot<Result<Vec<Message>, String>>,
    /// What the history being read is for.
    history_for: String,

    /// Messages naming the player, and direct messages, not seen yet (the rail's count).
    unseen_mentions: u32,
    unseen_direct: BTreeMap<String, u32>,

    text: String,
    sending: Slot<Result<server_api::chat::SendResponse, String>>,
    refused: Option<String>,
    acting: Slot<Result<String, String>>,
    reporting: Option<Reporting>,
    /// A Discord invite the player clicked: opened once they confirm.
    open_link: Option<String>,
    /// Marking a conversation read, up to a message.
    marking: Slot<Result<(), String>>,
    marked: BTreeMap<String, i64>,
}

impl Default for Chat {
    fn default() -> Self {
        Self {
            key: None,
            feed: None,
            trouble: None,
            status: None,
            status_error: None,
            status_slot: Slot::default(),
            status_at: None,
            friends: Vec::new(),
            online: flow::Friends::default(),
            tab: Tab::Global,
            with: None,
            global: Vec::new(),
            lobby: Vec::new(),
            lobby_of: 0,
            direct: BTreeMap::new(),
            names: BTreeMap::new(),
            conversations: Vec::new(),
            conversations_slot: Slot::default(),
            conversations_stale: true,
            history_asked: std::collections::HashSet::new(),
            history_slot: Slot::default(),
            history_for: String::new(),
            unseen_mentions: 0,
            unseen_direct: BTreeMap::new(),
            text: String::new(),
            sending: Slot::default(),
            refused: None,
            acting: Slot::default(),
            reporting: None,
            open_link: None,
            marking: Slot::default(),
            marked: BTreeMap::new(),
        }
    }
}

/// Whether chat is offered: on the community network.
pub fn available() -> bool {
    Prefs::directory().is_some_and(|d| Prefs::is_community(&d))
}

/// Adds `m` to `list` (by id, once), keeping the newest [`KEEP`].
fn keep(list: &mut Vec<Message>, m: Message) {
    match list.binary_search_by_key(&m.id, |x| x.id) {
        Ok(i) => list[i] = m,
        Err(i) => list.insert(i, m),
    }
    if list.len() > KEEP {
        let extra = list.len() - KEEP;
        list.drain(..extra);
    }
}

impl Chat {
    /// Unseen direct messages and mentions: the rail's count.
    pub fn unread(&self) -> u32 {
        self.unseen_mentions + self.unseen_direct.values().sum::<u32>()
    }

    /// Who a direct message is with, from the player's side.
    fn other(m: &Message) -> String {
        if m.mine {
            m.to.clone()
        } else {
            m.from.clone()
        }
    }

    /// Takes messages from the feed (or a send): into their rooms, deletions applied. `kept`:
    /// what the server had before the feed began, not counted as unseen (the conversations'
    /// unread counts come from the server).
    fn take(&mut self, messages: Vec<Message>, ctx: &egui::Context, showing: bool, kept: bool) -> bool {
        let mut attention = false;
        for m in messages {
            if m.deletes != 0 {
                let gone = m.deletes;
                self.global.retain(|x| x.id != gone);
                self.lobby.retain(|x| x.id != gone);
                for list in self.direct.values_mut() {
                    list.retain(|x| x.id != gone);
                }
                continue;
            }
            let seen_here = |tab: Tab| kept || (showing && self.tab == tab);
            match m.room() {
                Room::Global => {
                    if m.mentions_me && !seen_here(Tab::Global) {
                        self.unseen_mentions += 1;
                        attention = true;
                    }
                    keep(&mut self.global, m);
                }
                Room::Lobby => {
                    if m.mentions_me && !seen_here(Tab::Lobby) {
                        self.unseen_mentions += 1;
                        attention = true;
                    }
                    keep(&mut self.lobby, m);
                }
                Room::Direct => {
                    let other = Self::other(&m);
                    if !m.mine {
                        self.names.insert(other.clone(), m.from_name.clone());
                        let open = kept || (seen_here(Tab::Messages) && self.with.as_deref() == Some(other.as_str()));
                        if !open {
                            *self.unseen_direct.entry(other.clone()).or_default() += 1;
                            attention = true;
                        }
                    }
                    self.conversations_stale = true;
                    keep(self.direct.entry(other).or_default(), m);
                }
            }
        }
        // Named, or a message from a friend, while the launcher is behind another window.
        if attention && !ctx.input(|i| i.focused) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        attention
    }

    /// Every frame, on whatever screen: the feed, the status now and then, and what came of
    /// the screen's work.
    pub fn tick(&mut self, ctx: &egui::Context, game: Option<&Game>, notices: &mut Notices, showing: bool) {
        let cfg: Option<&Config> = game.map(|g| &*g.cfg);
        let key = cfg.filter(|_| available()).and_then(feed_key);
        // Another account or server (or none): start again.
        if key != self.key {
            *self = Self {
                key: key.clone(),
                tab: self.tab,
                ..Self::default()
            };
        }
        let Some(cfg) = cfg.filter(|_| key.is_some()) else { return };
        // Cloned only for work started below.

        // The status: whether chat is open to the player, their lobby; their friends; who's online.
        match self.status_slot.poll() {
            Some(Ok((status, friends, online))) => {
                if status.lobby != self.lobby_of {
                    self.lobby.clear();
                    self.lobby_of = status.lobby;
                    self.history_asked.remove("lobby");
                }
                self.status = Some(status);
                self.friends = friends;
                self.online = online;
                self.status_error = None;
            }
            Some(Err(e)) => self.status_error = Some(e),
            None => {}
        }
        let every = if showing { STATUS_WHILE_OPEN } else { STATUS_EVERY };
        if !self.status_slot.running() && self.status_at.is_none_or(|t| t.elapsed() >= every) {
            self.status_at = Some(Instant::now());
            let cfg = cfg.clone();
            self.status_slot.start(ctx, move || {
                let status = with_api(&cfg, |api| Box::pin(api.status()))?;
                let friends = if status.available {
                    with_api(&cfg, |api| Box::pin(api.friends())).unwrap_or_default()
                } else {
                    Vec::new()
                };
                // Who's online here: a hint, like on Play.
                let online = flow::friends(&cfg).unwrap_or_default();
                Ok((status, friends, online))
            });
        }
        let open = self.status.as_ref().is_some_and(|s| s.available);

        // The feed, once chat is open to the player.
        if open && self.feed.is_none() {
            let (tx, events) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let (c, s, x) = (cfg.clone(), Arc::clone(&stop), ctx.clone());
            std::thread::Builder::new().name("chat-feed".into()).spawn(move || run_feed(c, s, tx, x)).ok();
            self.feed = Some(Feed { stop, events });
        }
        let mut arrived = Vec::new();
        let mut kept = Vec::new();
        if let Some(feed) = &self.feed {
            while let Ok(event) = feed.events.try_recv() {
                match event {
                    FeedEvent::Messages(m, true) => kept.extend(m),
                    FeedEvent::Messages(m, false) => arrived.extend(m),
                    FeedEvent::Trouble(e) => self.trouble = Some(e),
                    FeedEvent::Back => self.trouble = None,
                }
            }
        }
        if !kept.is_empty() {
            self.take(kept, ctx, showing, true);
        }
        if !arrived.is_empty() {
            self.take(arrived, ctx, showing, false);
        }

        match self.sending.poll() {
            Some(Ok(sent)) => match sent.message {
                Some(m) => {
                    self.refused = None;
                    self.text.clear();
                    self.take(vec![m], ctx, showing, false);
                }
                None => self.refused = Some(if sent.says.is_empty() { "Not sent.".into() } else { sent.says }),
            },
            Some(Err(e)) => self.refused = Some(e),
            None => {}
        }
        if let Some(loaded) = self.history_slot.poll() {
            let what = std::mem::take(&mut self.history_for);
            match loaded {
                Ok(messages) => {
                    let list = match what.as_str() {
                        "global" => &mut self.global,
                        "lobby" => &mut self.lobby,
                        with => self.direct.entry(with.to_string()).or_default(),
                    };
                    for m in messages {
                        keep(list, m);
                    }
                }
                Err(e) => {
                    self.history_asked.remove(&what);
                    notices.error(format!("Couldn't read chat: {e}"));
                }
            }
        }
        match self.conversations_slot.poll() {
            Some(Ok(list)) => {
                for c in &list {
                    self.names.insert(c.with.clone(), c.with_name.clone());
                    if c.unread > 0 && self.with.as_deref() != Some(c.with.as_str()) {
                        self.unseen_direct.insert(c.with.clone(), c.unread);
                    }
                }
                self.conversations = list;
            }
            Some(Err(e)) => self.trouble = Some(e),
            None => {}
        }
        if open && self.conversations_stale && !self.conversations_slot.running() {
            self.conversations_stale = false;
            let cfg = cfg.clone();
            self.conversations_slot.start(ctx, move || with_api(&cfg, |api| Box::pin(api.conversations())));
        }
        if let Some(result) = self.acting.poll() {
            match result {
                Ok(done) => notices.info(done),
                Err(e) => notices.error(e),
            }
        }
        let _ = self.marking.poll();

        // What the screen shows is seen.
        if showing {
            match self.tab {
                Tab::Global | Tab::Lobby => self.unseen_mentions = 0,
                Tab::Messages => {
                    if let Some(with) = self.with.clone() {
                        self.unseen_direct.remove(&with);
                        let last = self.direct.get(&with).and_then(|l| l.iter().rev().find(|m| !m.mine)).map(|m| m.id);
                        if let Some(last) = last.filter(|l| self.marked.get(&with).is_none_or(|m| m < l)) {
                            if !self.marking.running() {
                                self.marked.insert(with.clone(), last);
                                let cfg = cfg.clone();
                                self.marking.start(ctx, move || with_api(&cfg, |api| Box::pin(api.mark_read(with.clone(), last))));
                            }
                        }
                    }
                }
            }
            // A room's history, once, when it's first shown.
            let what = match (self.tab, &self.with) {
                (Tab::Global, _) => Some(("global".to_string(), Room::Global, String::new())),
                (Tab::Lobby, _) if self.lobby_of != 0 => Some(("lobby".to_string(), Room::Lobby, String::new())),
                (Tab::Messages, Some(with)) => Some((with.clone(), Room::Direct, with.clone())),
                _ => None,
            };
            if let Some((what, room, with)) = what.filter(|(w, ..)| open && !self.history_asked.contains(w)) {
                if !self.history_slot.running() {
                    self.history_asked.insert(what.clone());
                    self.history_for = what;
                    let cfg = cfg.clone();
                    self.history_slot.start(ctx, move || with_api(&cfg, |api| Box::pin(api.history(room, with.clone(), 0))));
                }
            }
        }
    }
}

/// The Chat screen.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let (chat, game, notices) = app.chat_mut();
    let cfg = game.as_ref().map(|g| (*g.cfg).clone());
    theme::page().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(theme::display("Chat", 32.0));
        ui.label(RichText::new("Talk with everyone on the community network, the players in your lobby, and your friends on any of its servers.").color(theme::SOFT));
        ui.add_space(12.0);
        let Some(cfg) = cfg.filter(|c| feed_key(c).is_some()) else {
            ui.label(theme::muted("Set up an account on a community server first (Play)."));
            return;
        };
        let Some(status) = chat.status.clone() else {
            match &chat.status_error {
                Some(e) => ui.label(theme::muted(format!("Chat isn't reachable: {e}"))),
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(theme::muted("Opening chat…"))
                    })
                    .inner
                }
            };
            return;
        };
        if !status.available {
            ui.label(theme::muted(if status.why_not.is_empty() {
                "Chat isn't open on this server.".to_string()
            } else {
                status.why_not.clone()
            }));
            return;
        }
        if let Some(t) = &chat.trouble {
            ui.label(RichText::new(format!("Chat is reconnecting: {t}")).color(theme::WARN).small());
        }
        if status.read_only {
            ui.label(RichText::new("The network can't be reached right now: you can read, not send.").color(theme::WARN).small());
        }
        let mut tab = chat.tab;
        let lobby_label = if status.lobby != 0 { "Lobby" } else { "Lobby (not in one)" };
        let direct_unread: u32 = chat.unseen_direct.values().sum();
        let messages_label = if direct_unread > 0 {
            format!("Messages ({direct_unread})")
        } else {
            "Messages".to_string()
        };
        theme::segmented(
            ui,
            &mut tab,
            &[(Tab::Global, "Global"), (Tab::Lobby, lobby_label), (Tab::Messages, messages_label.as_str())],
        );
        chat.tab = tab;
        ui.add_space(10.0);
        if let Some(link) = chat.open_link.clone() {
            confirm_link(chat, &link, ui);
        }
        let height = (ui.available_height() - 150.0).max(240.0);
        match chat.tab {
            Tab::Global => {
                ui.horizontal_top(|ui| {
                    let side = 240.0;
                    let main = (ui.available_width() - side - 16.0).max(320.0);
                    ui.vertical(|ui| {
                        ui.set_width(main);
                        let list = std::mem::take(&mut chat.global);
                        messages(chat, &cfg, &list, height, "global", ui);
                        chat.global = list;
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(side);
                        online(chat, &cfg, height, ui);
                    });
                });
                if !status.may_post_global {
                    ui.label(theme::muted(&status.global_wait));
                }
                composer(chat, &cfg, Room::Global, "", status.may_post_global, "Say something to everyone…", ui);
            }
            Tab::Lobby => {
                if status.lobby == 0 {
                    ui.label(theme::muted("Join a lobby or a match in the game: its players can talk here, and in the game's overlay."));
                } else {
                    let list = std::mem::take(&mut chat.lobby);
                    messages(chat, &cfg, &list, height, "lobby", ui);
                    chat.lobby = list;
                    composer(chat, &cfg, Room::Lobby, "", true, "Say something to your lobby…", ui);
                }
            }
            Tab::Messages => conversations(chat, &cfg, height, ui),
        }
        ui.add_space(4.0);
        ui.label(theme::muted(RULES_LINE).small());
    });
    let _ = notices;
}

/// The list of conversations (and friends to start one with), beside the open one.
fn conversations(chat: &mut Chat, cfg: &Config, height: f32, ui: &mut egui::Ui) {
    ui.horizontal_top(|ui| {
        let side = 240.0;
        ui.vertical(|ui| {
            ui.set_width(side);
            egui::ScrollArea::vertical()
                .id_salt("conversations")
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if chat.conversations.is_empty() && chat.friends.is_empty() {
                        ui.label(theme::muted("No conversations yet. Make friends in the game (or from Global) to message them."));
                    }
                    let mut pick = None;
                    for c in &chat.conversations {
                        let unread = chat.unseen_direct.get(&c.with).copied().unwrap_or(0);
                        let name = if c.with_name.is_empty() { identity::short(&c.with) } else { c.with_name.clone() };
                        let text = if unread > 0 { format!("{name}  ({unread})") } else { name };
                        let chosen = chat.with.as_deref() == Some(c.with.as_str());
                        if ui
                            .add(egui::Button::selectable(
                                chosen,
                                RichText::new(text).color(if unread > 0 { theme::ACCENT } else { theme::FG }),
                            ))
                            .clicked()
                        {
                            pick = Some(c.with.clone());
                        }
                    }
                    let new: Vec<&ChatFriend> = chat.friends.iter().filter(|f| !chat.conversations.iter().any(|c| c.with == f.identity)).collect();
                    if !new.is_empty() {
                        ui.add_space(8.0);
                        ui.label(theme::caps("Friends"));
                        for f in new {
                            let dot = if f.online { "● " } else { "" };
                            let chosen = chat.with.as_deref() == Some(f.identity.as_str());
                            if ui.add(egui::Button::selectable(chosen, format!("{dot}{}", f.name))).clicked() {
                                chat.names.insert(f.identity.clone(), f.name.clone());
                                pick = Some(f.identity.clone());
                            }
                        }
                    }
                    if let Some(p) = pick {
                        chat.with = Some(p);
                    }
                });
        });
        ui.add_space(16.0);
        ui.vertical(|ui| {
            let Some(with) = chat.with.clone() else {
                ui.label(theme::muted("Pick a conversation, or a friend to start one."));
                return;
            };
            let name = chat.names.get(&with).cloned().unwrap_or_else(|| identity::short(&with));
            ui.label(RichText::new(&name).family(theme::strong()).size(18.0));
            let list = chat.direct.remove(&with).unwrap_or_default();
            messages(chat, cfg, &list, height - 30.0, &with, ui);
            chat.direct.insert(with.clone(), list);
            composer(chat, cfg, Room::Direct, &with, true, &format!("Message {name}…"), ui);
        });
    });
}

/// Who's online on the player's server, beside Global.
fn online(chat: &mut Chat, cfg: &Config, height: f32, ui: &mut egui::Ui) {
    ui.label(theme::caps(&format!("Online here · {}", chat.online.online.len() + chat.online.others.len())));
    egui::ScrollArea::vertical()
        .id_salt("online")
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in &chat.online.online {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("●").color(theme::OK).small());
                    ui.label(RichText::new(&f.name).color(theme::FG))
                        .on_hover_text(f.activity.as_deref().unwrap_or("Online · your friend"));
                });
            }
            let mut act = None;
            for p in &chat.online.others {
                let row = ui.horizontal(|ui| {
                    ui.label(RichText::new("●").color(theme::MUTED).small());
                    ui.label(&p.name).on_hover_text(p.activity.as_deref().unwrap_or("Online"));
                });
                row.response.context_menu(|ui| {
                    if !p.asked && ui.button("Add friend").clicked() {
                        act = Some((PlayerAction::Request, p.id.clone(), p.name.clone()));
                        ui.close();
                    }
                    if ui.button("Invite to your game").clicked() {
                        act = Some((PlayerAction::Invite, p.id.clone(), p.name.clone()));
                        ui.close();
                    }
                    if ui.button(format!("Block {}", p.name)).clicked() {
                        act = Some((PlayerAction::Block, p.id.clone(), p.name.clone()));
                        ui.close();
                    }
                });
            }
            if chat.online.online.is_empty() && chat.online.others.is_empty() {
                ui.label(theme::muted("Nobody else is online here right now."));
            } else {
                ui.label(theme::muted("Right-click a name for more.").small());
            }
            if let Some((action, id, name)) = act {
                start_action(chat, cfg, action, id, String::new(), name, ui.ctx());
            }
        });
}

/// Does `action` to a player: by account id (who's online here), or by identity (chat's
/// senders), never by name alone.
fn start_action(chat: &mut Chat, cfg: &Config, action: PlayerAction, id: String, identity: String, name: String, ctx: &egui::Context) {
    if chat.acting.running() {
        return;
    }
    let cfg = cfg.clone();
    chat.acting.start(ctx, move || flow::player_action(&cfg, action, &id, &identity, &name));
}

/// A room's or a conversation's messages, newest at the bottom.
fn messages(chat: &mut Chat, cfg: &Config, list: &[Message], height: f32, salt: &str, ui: &mut egui::Ui) {
    let clock = setup::clock::Clock::local();
    egui::Frame::new()
        .fill(theme::SUNKEN)
        .stroke(egui::Stroke::new(1.0, theme::LINE))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("messages", salt))
                .max_height(height)
                .min_scrolled_height(height)
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if list.is_empty() {
                        ui.label(theme::muted("No messages yet."));
                    }
                    for m in list {
                        message(chat, cfg, m, &clock, ui);
                    }
                });
        });
}

/// One message: when, who (right-click for more), and what, with a Discord invite as a link.
fn message(chat: &mut Chat, cfg: &Config, m: &Message, clock: &setup::clock::Clock, ui: &mut egui::Ui) {
    let row = egui::Frame::new()
        .fill(if m.mentions_me {
            theme::ACCENT.linear_multiply(0.10)
        } else {
            egui::Color32::TRANSPARENT
        })
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(theme::muted(clock.clock_time(m.at)).small());
                let who = RichText::new(if m.mine { "You" } else { m.from_name.as_str() })
                    .family(theme::strong())
                    .color(if m.mine { theme::ACCENT } else { theme::FG });
                let who = ui.add(egui::Label::new(who).sense(egui::Sense::click()));
                let who = if m.server_name.is_empty() {
                    who
                } else {
                    who.on_hover_text(format!("On {}", m.server_name))
                };
                who.context_menu(|ui| message_menu(chat, cfg, m, ui));
                body(chat, &m.body, ui);
            });
        });
    row.response.context_menu(|ui| message_menu(chat, cfg, m, ui));
    if chat.reporting.as_ref().is_some_and(|r| r.id == m.id) {
        report_form(chat, cfg, ui);
    }
}

/// A message's text: plain, but a Discord invite in its own form is a link (opened after asking).
fn body(chat: &mut Chat, text: &str, ui: &mut egui::Ui) {
    let invites = moderation::discord_invites(text);
    let mut at = 0;
    for (start, end) in invites {
        if start > at {
            ui.label(RichText::new(&text[at..start]).color(theme::FG));
        }
        let link = &text[start..end];
        if ui.link(link).on_hover_text("A Discord invite: opens in your browser after you confirm").clicked() {
            chat.open_link = Some(link.to_string());
        }
        at = end;
    }
    if at < text.len() {
        ui.label(RichText::new(&text[at..]).color(theme::FG));
    }
}

/// What can be done about a message and its sender.
fn message_menu(chat: &mut Chat, cfg: &Config, m: &Message, ui: &mut egui::Ui) {
    if m.mine {
        if ui.button("Delete").clicked() {
            let (cfg, id) = (cfg.clone(), m.id);
            chat.acting
                .start(ui.ctx(), move || with_api(&cfg, |api| Box::pin(api.delete(id))).map(|()| "Deleted.".to_string()));
            ui.close();
        }
        return;
    }
    let friend = chat.friends.iter().any(|f| f.identity == m.from);
    if friend && m.room() != Room::Direct && ui.button(format!("Message {}", m.from_name)).clicked() {
        chat.names.insert(m.from.clone(), m.from_name.clone());
        chat.with = Some(m.from.clone());
        chat.tab = Tab::Messages;
        ui.close();
    }
    // By their identity: a name alone may be another account's on this server.
    if !friend && ui.button("Add friend").clicked() {
        start_action(chat, cfg, PlayerAction::Request, String::new(), m.from.clone(), m.from_name.clone(), ui.ctx());
        ui.close();
    }
    if ui.button(format!("Block {}", m.from_name)).clicked() {
        start_action(chat, cfg, PlayerAction::Block, String::new(), m.from.clone(), m.from_name.clone(), ui.ctx());
        ui.close();
    }
    if ui.button("Report…").clicked() {
        chat.reporting = Some(Reporting {
            id: m.id,
            name: m.from_name.clone(),
            why: String::new(),
        });
        ui.close();
    }
}

/// Why a message is reported, under it.
fn report_form(chat: &mut Chat, cfg: &Config, ui: &mut egui::Ui) {
    let Some(r) = chat.reporting.as_mut() else { return };
    let mut close = false;
    theme::card().show(ui, |ui| {
        ui.label(RichText::new(format!("Report {}'s message to the admins", r.name)).family(theme::strong()));
        ui.add(
            egui::TextEdit::singleline(&mut r.why)
                .hint_text("Why (optional)")
                .char_limit(300)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui.add(theme::primary("Report")).clicked() {
                let (cfg, id, why) = (cfg.clone(), r.id, r.why.trim().to_string());
                chat.acting.start(ui.ctx(), move || {
                    with_api(&cfg, |api| Box::pin(api.report(id, why.clone()))).map(|()| "Reported. The admins see it with what came before it.".to_string())
                });
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    if close {
        chat.reporting = None;
    }
}

/// Asks before opening a Discord invite someone sent: a dialog over the page (Escape or a
/// click outside cancels).
fn confirm_link(chat: &mut Chat, link: &str, ui: &mut egui::Ui) {
    let modal = egui::Modal::new(egui::Id::new("chat-open-link"))
        .frame(
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0, theme::CONTROL_LINE))
                .corner_radius(16)
                .inner_margin(egui::Margin {
                    left: 26,
                    right: 26,
                    top: 24,
                    bottom: 22,
                }),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.label(theme::display("Open a Discord invite?", 22.0));
            ui.label(RichText::new(link).family(theme::strong()));
            ui.label(RichText::new("It opens in your browser. Someone in chat sent it: only join Discord servers you trust.").color(theme::SOFT));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add(theme::primary("Open")).clicked() {
                    let url = if link.starts_with("http") { link.to_string() } else { format!("https://{link}") };
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    chat.open_link = None;
                }
                if ui.button("Cancel").clicked() {
                    chat.open_link = None;
                }
            });
        });
    if modal.should_close() {
        chat.open_link = None;
    }
}

/// The text box: the rules checked while typing, Enter sends.
fn composer(chat: &mut Chat, cfg: &Config, room: Room, to: &str, allowed: bool, hint: &str, ui: &mut egui::Ui) {
    ui.add_space(8.0);
    let rules = moderation::Rules::new([], [], true);
    let checked = rules.check_message(&chat.text);
    let sending = chat.sending.running();
    let mut send = false;
    ui.horizontal(|ui| {
        let edit = ui.add_enabled(
            allowed,
            egui::TextEdit::singleline(&mut chat.text)
                .hint_text(hint)
                .char_limit(moderation::MAX_MESSAGE)
                .desired_width((ui.available_width() - 90.0).max(200.0)),
        );
        if edit.changed() {
            chat.refused = None;
        }
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            send = true;
            edit.request_focus();
        }
        if ui.add_enabled(allowed && checked.is_ok() && !sending, theme::primary("Send")).clicked() {
            send = true;
        }
        if sending {
            ui.spinner();
        }
    });
    match (&checked, &chat.refused) {
        (_, Some(why)) => {
            ui.label(RichText::new(why.as_str()).color(theme::WARN).small());
        }
        (Err(moderation::Refused::Empty), None) => {}
        (Err(why), None) => {
            ui.label(RichText::new(why.says()).color(theme::WARN).small());
        }
        (Ok(c), None) if c.masked > 0 => {
            ui.label(theme::muted("Some words will show as ***.").small());
        }
        _ => {}
    }
    if send && allowed && checked.is_ok() && !sending {
        let (cfg, to, text) = (cfg.clone(), to.to_string(), chat.text.trim().to_string());
        chat.sending
            .start(ui.ctx(), move || with_api(&cfg, |api| Box::pin(api.send(room, to.clone(), text.clone()))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: i64, room: Room) -> Message {
        Message {
            id,
            room: room.into(),
            body: format!("m{id}"),
            ..Message::default()
        }
    }

    #[test]
    fn messages_are_kept_once_in_order_and_deletions_apply() {
        let mut list = Vec::new();
        for id in [3, 1, 2, 3] {
            keep(&mut list, message(id, Room::Global));
        }
        assert_eq!(list.iter().map(|m| m.id).collect::<Vec<_>>(), [1, 2, 3]);
        for id in 10..10 + KEEP as i64 {
            keep(&mut list, message(id, Room::Global));
        }
        assert_eq!((list.len(), list[0].id), (KEEP, 10), "the newest kept");

        let mut chat = Chat::default();
        let ctx = egui::Context::default();
        chat.take(vec![message(5, Room::Global), message(6, Room::Global)], &ctx, false, false);
        let deletion = Message {
            deletes: 5,
            ..message(7, Room::Global)
        };
        chat.take(vec![deletion], &ctx, false, false);
        assert_eq!(chat.global.iter().map(|m| m.id).collect::<Vec<_>>(), [6], "deleted, and the deletion isn't shown");

        // A friend's message, and a mention, while elsewhere: counted for the rail.
        let direct = Message {
            from: "FRIEND".into(),
            from_name: "Kiwi".into(),
            ..message(8, Room::Direct)
        };
        let mention = Message {
            mentions_me: true,
            ..message(9, Room::Global)
        };
        chat.take(vec![direct.clone(), mention.clone()], &ctx, false, false);
        assert_eq!((chat.unread(), chat.names.get("FRIEND").map(String::as_str)), (2, Some("Kiwi")));

        // The same, as what the server kept before the launcher started: shown, not counted
        // (the conversations' own unread counts follow from the server).
        let mut chat = Chat::default();
        chat.take(vec![direct, mention], &ctx, false, true);
        assert_eq!((chat.unread(), chat.global.len(), chat.conversations_stale), (0, 1, true));
    }
}
