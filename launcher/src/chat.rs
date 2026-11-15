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

use server_api::chat::ChatFriend;
use server_api::chat::Conversation;
use server_api::chat::LobbyEvent;
use server_api::chat::Message;
use server_api::chat::Room;
use setup::config::Config;

use crate::flow;
use crate::network::ChatApi;
use crate::prefs::Prefs;

/// The messages kept per room or conversation.
pub const KEEP: usize = 500;
/// How often the status (and who's online) is asked again: while the screen shows, and not.
pub const STATUS_WHILE_OPEN: Duration = Duration::from_secs(30);
pub const STATUS_EVERY: Duration = Duration::from_secs(5 * 60);
/// What players read under the box.
pub const RULES_LINE: &str = "Messages are kept for 7 days and the admins can read them. No links, keep it friendly.";

/// Which part of chat shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Global,
    Lobby,
    Messages,
}

/// What the feed's thread hands over.
pub enum FeedEvent {
    /// Messages, and whether they're what the server kept before the feed began (the first
    /// answer): shown, but not counted as unseen.
    Messages(Vec<Message>, bool),
    /// Who joined or left the player's lobby (from a server that says).
    Lobby(Vec<LobbyEvent>),
    /// The server can't be reached, or said no: why, for the screen.
    Trouble(String),
    Back,
}

/// The server and account chat is for, to tell when they change (the password too: the feed
/// signs in with the one it started with, as `flow::api_session` keys its token).
pub fn feed_key(cfg: &Config) -> Option<String> {
    let p = cfg.current_profile().filter(|p| !p.server.is_empty() && p.has_account())?;
    let secret = p.user.secret().unwrap_or_default();
    Some(format!("{}\n{}\n{}", p.api_server_url(), p.user.username, crate::network::secret_fingerprint(&secret)))
}

/// A call to chat as the current account; signed in again once if the server stopped taking
/// the kept token.
pub fn call<T>(cfg: &Config, f: impl Fn(String, String) -> Result<T, crate::network::Error>) -> Result<T, String> {
    let (url, token) = flow::api_session(cfg, false)?;
    match f(url, token) {
        Err(crate::network::Error::InvalidPassword) => {
            let (url, token) = flow::api_session(cfg, true)?;
            f(url, token).map_err(|e| says(&e))
        }
        other => other.map_err(|e| says(&e)),
    }
}

pub fn says(e: &crate::network::Error) -> String {
    match e {
        crate::network::Error::ServerFailure(s) if !s.is_empty() => s.clone(),
        crate::network::Error::ConnectionFailed => "Can't reach your server right now.".into(),
        other => other.to_string(),
    }
}

/// Runs a chat call on the shared runtime.
pub fn with_api<T>(cfg: &Config, f: impl Fn(&mut ChatApi) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, crate::network::Error>> + '_>>) -> Result<T, String> {
    call(cfg, |url, token| {
        crate::services::rt().block_on(async {
            let mut api = ChatApi::connect(&url, &token).await?;
            f(&mut api).await
        })
    })
}

/// The feed's thread: waits on the server for messages after the newest it has, again and
/// again, until stopped.
pub fn run_feed(cfg: Config, stop: Arc<AtomicBool>, tx: mpsc::Sender<FeedEvent>, notify: impl Fn()) {
    let mut after = 0;
    let mut after_event = 0;
    let mut primed = false;
    let mut failures = 0u32;
    while !stop.load(Ordering::Relaxed) {
        match with_api(&cfg, |api| Box::pin(api.wait(after, after_event))) {
            Ok(answer) => {
                if failures > 0 {
                    let _ = tx.send(FeedEvent::Back);
                }
                failures = 0;
                after = answer.last.max(after);
                after_event = answer.last_event.max(after_event);
                let kept = !std::mem::replace(&mut primed, true);
                if !answer.events.is_empty() {
                    if tx.send(FeedEvent::Lobby(answer.events)).is_err() {
                        return;
                    }
                    notify();
                }
                if !answer.messages.is_empty() {
                    if tx.send(FeedEvent::Messages(answer.messages, kept)).is_err() {
                        return;
                    }
                    notify();
                }
            }
            Err(e) => {
                failures += 1;
                if tx.send(FeedEvent::Trouble(e)).is_err() {
                    return;
                }
                notify();
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

/// Whether chat is offered: on the community network.
pub fn available() -> bool {
    Prefs::directory().is_some_and(|d| Prefs::is_community(&d))
}

/// The messages kept, by room and conversation, and what's unseen: shared by both interfaces.
pub struct Rooms {
    pub global: Vec<Message>,
    pub lobby: Vec<Message>,
    pub direct: BTreeMap<String, Vec<Message>>,
    /// Names of the people direct conversations are with.
    pub names: BTreeMap<String, String>,
    /// Who joined or left the player's lobby, oldest first (cleared with the lobby's messages).
    pub lobby_events: Vec<LobbyEvent>,
    /// Messages naming the player, and direct messages, not seen yet (the rail's count).
    pub unseen_mentions: u32,
    pub unseen_direct: BTreeMap<String, u32>,
    /// The conversations list is to be read again.
    pub conversations_stale: bool,
}

impl Default for Rooms {
    fn default() -> Self {
        Self {
            global: Vec::new(),
            lobby: Vec::new(),
            direct: BTreeMap::new(),
            names: BTreeMap::new(),
            lobby_events: Vec::new(),
            unseen_mentions: 0,
            unseen_direct: BTreeMap::new(),
            conversations_stale: true,
        }
    }
}

impl Rooms {
    /// Unseen direct messages and mentions: the rail's count.
    pub fn unread(&self) -> u32 {
        self.unseen_mentions + self.unseen_direct.values().sum::<u32>()
    }

    /// The player blocked `identity`: what they wrote goes from every room now, as the server
    /// already leaves it out of what it sends from now on.
    pub fn blocked(&mut self, identity: &str) {
        self.global.retain(|m| m.from != identity);
        self.lobby.retain(|m| m.from != identity);
        self.direct.remove(identity);
        self.unseen_direct.remove(identity);
        self.conversations_stale = true;
    }

    /// The player's lobby changed: its messages and who joined or left it go.
    pub fn lobby_changed(&mut self) {
        self.lobby.clear();
        self.lobby_events.clear();
    }

    /// Takes who joined or left the lobby (by id, once, cleaned), keeping the newest [`KEEP`].
    pub fn take_events(&mut self, events: Vec<LobbyEvent>) {
        use hooks_config::text::clip;
        for mut e in events {
            e.name = clip(&e.name, MAX_NAME);
            e.identity = clip(&e.identity, MAX_IDENTITY);
            match self.lobby_events.binary_search_by_key(&e.id, |x| x.id) {
                Ok(i) => self.lobby_events[i] = e,
                Err(i) => self.lobby_events.insert(i, e),
            }
        }
        if self.lobby_events.len() > KEEP {
            let extra = self.lobby_events.len() - KEEP;
            self.lobby_events.drain(..extra);
        }
    }

    /// Who a direct message is with, from the player's side.
    pub fn other(m: &Message) -> String {
        if m.mine {
            m.to.clone()
        } else {
            m.from.clone()
        }
    }

    /// Takes messages into their rooms, deletions applied. `kept`: what the server had before
    /// the feed began, not counted as unseen (the conversations' unread counts come from the
    /// server); `showing(tab)`: that tab shows now; `with`: the conversation open. Says whether
    /// the player was named or sent a message they haven't seen.
    pub fn take(&mut self, messages: Vec<Message>, kept: bool, showing: impl Fn(Tab) -> bool, with: Option<&str>) -> bool {
        let mut attention = false;
        for m in messages {
            let m = clean(m);
            if m.deletes != 0 {
                let gone = m.deletes;
                self.global.retain(|x| x.id != gone);
                self.lobby.retain(|x| x.id != gone);
                for list in self.direct.values_mut() {
                    list.retain(|x| x.id != gone);
                }
                continue;
            }
            let seen_here = |tab: Tab| kept || showing(tab);
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
                    let other = Rooms::other(&m);
                    if !m.mine {
                        self.names.insert(other.clone(), m.from_name.clone());
                        let open = kept || (seen_here(Tab::Messages) && with == Some(other.as_str()));
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
        self.prune(with);
        attention
    }

    /// Notes the conversations' names (and unread counts, but for the one open, `with`) from
    /// the server's list, cleaned ([`clean_conversations`]); the list to keep.
    pub fn take_conversations(&mut self, list: Vec<Conversation>, with: Option<&str>) -> Vec<Conversation> {
        let list = clean_conversations(list);
        for c in &list {
            self.names.insert(c.with.clone(), c.with_name.clone());
            if c.unread > 0 && with != Some(c.with.as_str()) {
                self.unseen_direct.insert(c.with.clone(), c.unread);
            }
        }
        self.prune(with);
        list
    }

    /// Keeps at most [`MAX_CONVERSATIONS`] direct conversations (the open one, `with`, always),
    /// dropping those whose newest message is oldest, and names and unread counts for no more
    /// than twice that: a server sending messages from ever more identities can't grow them
    /// without end.
    pub fn prune(&mut self, with: Option<&str>) {
        while self.direct.len() > MAX_CONVERSATIONS {
            let oldest = self
                .direct
                .iter()
                .filter(|(other, _)| with != Some(other.as_str()))
                .min_by_key(|(_, list)| list.last().map_or(i64::MIN, |m| m.id))
                .map(|(other, _)| other.clone());
            let Some(oldest) = oldest else { break };
            self.direct.remove(&oldest);
            self.unseen_direct.remove(&oldest);
        }
        if self.names.len() > 2 * MAX_CONVERSATIONS || self.unseen_direct.len() > 2 * MAX_CONVERSATIONS {
            let direct = &self.direct;
            self.names.retain(|other, _| direct.contains_key(other) || with == Some(other.as_str()));
            self.unseen_direct.retain(|other, _| direct.contains_key(other));
        }
    }
}

/// The longest name kept from the server (a player's, a server's), as the overlay keeps them.
pub const MAX_NAME: usize = 32;
pub const MAX_SERVER_NAME: usize = 64;
/// The longest identity kept (they're far shorter: one longer isn't one).
pub const MAX_IDENTITY: usize = 128;
/// The most direct conversations kept; past it the one with the oldest message goes.
pub const MAX_CONVERSATIONS: usize = 200;

/// `m` as the server sent it, made safe to keep and show: names and the body without control
/// or invisible characters, each no longer than the server lets players write them (a server
/// may be anyone's).
pub fn clean(mut m: Message) -> Message {
    use hooks_config::text::clip;
    m.from = clip(&m.from, MAX_IDENTITY);
    m.to = clip(&m.to, MAX_IDENTITY);
    m.from_name = clip(&m.from_name, MAX_NAME);
    m.server_name = clip(&m.server_name, MAX_SERVER_NAME);
    m.body = clip(&m.body, moderation::MAX_MESSAGE);
    m
}

/// The server's conversations list, cleaned as [`clean`] does messages, at most
/// [`MAX_CONVERSATIONS`] (the most recent, in the server's order).
pub fn clean_conversations(mut list: Vec<Conversation>) -> Vec<Conversation> {
    use hooks_config::text::clip;
    if list.len() > MAX_CONVERSATIONS {
        let mut newest: Vec<i64> = list.iter().map(|c| c.last).collect();
        newest.sort_unstable_by(|a, b| b.cmp(a));
        let oldest_kept = newest[MAX_CONVERSATIONS - 1];
        let mut room = MAX_CONVERSATIONS;
        list.retain(|c| {
            let kept = room > 0 && c.last >= oldest_kept;
            room -= usize::from(kept);
            kept
        });
    }
    for c in &mut list {
        c.with = clip(&c.with, MAX_IDENTITY);
        c.with_name = clip(&c.with_name, MAX_NAME);
    }
    list
}

/// The server's friends list for chat, cleaned as [`clean`] does messages.
pub fn clean_friends(mut list: Vec<ChatFriend>) -> Vec<ChatFriend> {
    use hooks_config::text::clip;
    list.truncate(2 * MAX_CONVERSATIONS);
    for f in &mut list {
        f.identity = clip(&f.identity, MAX_IDENTITY);
        f.name = clip(&f.name, MAX_NAME);
    }
    list
}

/// Adds `m` to `list` (by id, once, cleaned), keeping the newest [`KEEP`].
pub fn keep(list: &mut Vec<Message>, m: Message) {
    let m = clean(m);
    match list.binary_search_by_key(&m.id, |x| x.id) {
        Ok(i) => list[i] = m,
        Err(i) => list.insert(i, m),
    }
    if list.len() > KEEP {
        let extra = list.len() - KEEP;
        list.drain(..extra);
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
    fn blocking_someone_takes_what_they_wrote_away() {
        let mut rooms = Rooms::default();
        let from = |id: i64, room: Room, who: &str| Message {
            from: who.into(),
            ..message(id, room)
        };
        rooms.global = vec![from(1, Room::Global, "B"), from(2, Room::Global, "K")];
        rooms.lobby = vec![from(3, Room::Lobby, "B")];
        rooms.direct.insert("B".into(), vec![from(4, Room::Direct, "B")]);
        rooms.unseen_direct.insert("B".into(), 1);
        rooms.conversations_stale = false;
        rooms.blocked("B");
        assert_eq!(rooms.global.iter().map(|m| m.id).collect::<Vec<_>>(), [2]);
        assert!(rooms.lobby.is_empty());
        assert!(!rooms.direct.contains_key("B"));
        assert_eq!(rooms.unread(), 0);
        assert!(rooms.conversations_stale);
    }

    #[test]
    fn what_the_server_sends_is_cleaned_and_clipped() {
        let mut rooms = Rooms::default();
        let m = Message {
            from: "F".into(),
            from_name: format!("Raven\u{202e}\n{}", "x".repeat(100)),
            server_name: "s".repeat(500),
            body: format!("hi\r\nthere{}", "y".repeat(5000)),
            ..message(1, Room::Direct)
        };
        rooms.take(vec![m], false, |_| false, None);
        let kept = &rooms.direct["F"][0];
        assert!(kept.from_name.starts_with("Ravenx") && kept.from_name.chars().count() == MAX_NAME, "{}", kept.from_name);
        assert_eq!(rooms.names["F"], kept.from_name);
        assert_eq!(kept.server_name.len(), MAX_SERVER_NAME);
        assert!(kept.body.starts_with("hithere") && kept.body.chars().count() == moderation::MAX_MESSAGE);
        // History goes through keep, cleaned the same.
        let mut list = Vec::new();
        keep(
            &mut list,
            Message {
                body: "a\u{0}b".into(),
                ..message(2, Room::Global)
            },
        );
        assert_eq!(list[0].body, "ab");
    }

    #[test]
    fn conversations_kept_are_bounded() {
        let mut rooms = Rooms::default();
        let many: Vec<Message> = (0..MAX_CONVERSATIONS as i64 + 50)
            .map(|i| Message {
                from: format!("p{i}"),
                ..message(i + 1, Room::Direct)
            })
            .collect();
        rooms.take(many, false, |_| false, Some("p0"));
        assert_eq!(rooms.direct.len(), MAX_CONVERSATIONS);
        assert!(rooms.direct.contains_key("p0"), "the open one stays");
        assert!(!rooms.direct.contains_key("p1"), "the oldest goes");
        assert!(rooms.direct.contains_key(&format!("p{}", MAX_CONVERSATIONS + 49)));
        assert!(rooms.unseen_direct.len() <= 2 * MAX_CONVERSATIONS && rooms.names.len() <= 2 * MAX_CONVERSATIONS + 50);

        let list: Vec<Conversation> = (0..MAX_CONVERSATIONS as i64 + 10)
            .map(|i| Conversation {
                with: format!("c{i}"),
                with_name: "N\u{7}ame".into(),
                last: i,
                ..Conversation::default()
            })
            .collect();
        let kept = Rooms::default().take_conversations(list, None);
        assert_eq!(kept.len(), MAX_CONVERSATIONS);
        assert_eq!((kept[0].with.as_str(), kept[0].with_name.as_str()), ("c10", "Name"), "the newest, in the server's order");
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

        let mut rooms = Rooms::default();
        let elsewhere = |_| false;
        rooms.take(vec![message(5, Room::Global), message(6, Room::Global)], false, elsewhere, None);
        let deletion = Message {
            deletes: 5,
            ..message(7, Room::Global)
        };
        rooms.take(vec![deletion], false, elsewhere, None);
        assert_eq!(rooms.global.iter().map(|m| m.id).collect::<Vec<_>>(), [6], "deleted, and the deletion isn't shown");

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
        rooms.take(vec![direct.clone(), mention.clone()], false, elsewhere, None);
        assert_eq!((rooms.unread(), rooms.names.get("FRIEND").map(String::as_str)), (2, Some("Kiwi")));

        // The same, as what the server kept before the launcher started: shown, not counted
        // (the conversations' own unread counts follow from the server).
        let mut rooms = Rooms::default();
        rooms.take(vec![direct, mention], true, elsewhere, None);
        assert_eq!((rooms.unread(), rooms.global.len(), rooms.conversations_stale), (0, 1, true));
    }
}
