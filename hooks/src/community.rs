//! What the overlay shows about the server and the people on it: friends,
//! friend requests, blocks and player searches.
//!
//! Everything here runs on a background thread: the API calls block, and the
//! render loop must never wait on the network. The render loop only reads the
//! latest [`Snapshot`] and queues actions (refresh, invite, search, friend
//! changes).

use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::net::ToSocketAddrs;
use std::sync::mpsc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use server_api::friends;
use tracing::info;
use tracing::warn;

pub use crate::api::FriendChange;

/// How often the lists refresh on their own.
const REFRESH_EVERY: Duration = Duration::from_secs(15);
/// Timeout for the server's HTTP API (port 80).
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

/// How a player stands to us.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Relation {
    #[default]
    None,
    Friend,
    RequestSent,
    RequestReceived,
    Blocked,
}

/// How far a player's name is known to be theirs (see the server's `NameStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NameStatus {
    /// No identity: the name is only this server's.
    #[default]
    Unlinked,
    /// An identity, on a server that doesn't share friends.
    Linked,
    /// Theirs on every server sharing friends.
    Reserved,
    /// Someone else holds this name on the servers sharing friends.
    Conflict,
}

/// Another player, as the overlay lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    /// The API's user id, needed to invite them or change anything.
    pub id: String,
    pub name: String,
    pub online: bool,
    /// What they're doing, e.g. "Spies vs Mercs · in a match with Tank".
    pub activity: Option<String>,
    pub relation: Relation,
    /// Short form of their identity ("K7QF-2M9D"), empty if not linked.
    pub identity: String,
    pub name_status: NameStatus,
    /// Set when they share a name with a friend of ours from another server
    /// but aren't that friend: the server that friend is on.
    pub lookalike: Option<String>,
}

/// What the server says about itself (`/api/info`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub revision: String,
}

/// A short message for the player about something they did, or something
/// that happened (a friend request).
#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// The latest state for the overlay.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Friends, online first, then by name.
    pub friends: Vec<Player>,
    /// Friend requests waiting for our answer.
    pub requests_in: Vec<Player>,
    /// Our requests waiting for theirs.
    pub requests_out: Vec<Player>,
    pub blocked: Vec<Player>,
    /// Every player is on the game's friend list (the server's "everyone"
    /// mode), not only friends; anyone can invite.
    pub everyone_mode: bool,
    /// The last search: its query and what it found.
    pub search: Option<(String, Vec<Player>)>,
    pub server: Option<ServerInfo>,
    /// Round trip of the last successful refresh.
    pub response_time: Option<Duration>,
    /// Set once the lists have loaded at least once.
    pub loaded: bool,
    pub notice: Option<Notice>,
    /// Our own name belongs to another player on the servers sharing
    /// friends: we should rename.
    pub my_name_conflict: bool,
    /// Our identity's short form, empty if not linked.
    pub my_identity: String,
    /// Friends we know from other servers, by name key.
    known: std::collections::HashMap<String, Vec<Known>>,
}

impl Snapshot {
    pub fn online_count(&self) -> usize {
        self.friends.iter().filter(|p| p.online).count()
    }

    /// Where a player called `name` who isn't our friend here shares a name
    /// with a friend of ours on another server: that server. For an invite,
    /// whose sender we may know nothing else about.
    pub fn lookalike_of(&self, name: &str) -> Option<String> {
        if self.friends.iter().any(|f| f.name == name) {
            return None;
        }
        let listed = self
            .search
            .iter()
            .flat_map(|(_, found)| found.iter())
            .chain(self.requests_in.iter())
            .find(|p| p.name == name);
        match listed {
            Some(p) => p.lookalike.clone(),
            None => self
                .known
                .get(&name_key(name))
                .and_then(|k| k.iter().find(|k| k.server != server_name()).map(|k| k.server.clone())),
        }
    }

    /// The activity of the named player, if they're online and doing something.
    pub fn activity_of(&self, name: &str) -> Option<&str> {
        self.friends
            .iter()
            .chain(self.search.iter().flat_map(|(_, found)| found.iter()))
            .find(|p| p.name == name)
            .and_then(|p| p.activity.as_deref())
    }
}

enum Action {
    Refresh,
    Invite { id: String, name: String },
    Search(String),
    Change { change: FriendChange, id: String, name: String },
}

/// A friend we know from some server: their identity there.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
struct Known {
    name: String,
    identity: String,
    server: String,
}

/// Friends from every server this game has played on, kept next to
/// `uplay.toml`: how the overlay tells a friend from someone who only took
/// their name on another server.
const KNOWN_FILE: &str = "5th-echelon-known-friends.json";
/// The most names remembered.
const MAX_KNOWN: usize = 2000;

static FOLDER: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Where the game (and `uplay.toml`) is; set at start.
pub fn set_folder(dir: std::path::PathBuf) {
    let _ = FOLDER.set(dir);
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The server we're on, as the known-friends file names it.
fn server_name() -> String {
    crate::config::get().and_then(|c| c.api_server.host_str().map(String::from)).unwrap_or_default()
}

fn load_known() -> std::collections::HashMap<String, Vec<Known>> {
    FOLDER
        .get()
        .and_then(|d| std::fs::read(d.join(KNOWN_FILE)).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Adds this server's friends (the linked ones) to what we know, saving it
/// when it changed.
fn remember(known: &mut std::collections::HashMap<String, Vec<Known>>, friends: &[Player]) {
    let server = server_name();
    let mut changed = false;
    for f in friends.iter().filter(|f| !f.identity.is_empty()) {
        let entry = Known {
            name: f.name.clone(),
            identity: f.identity.clone(),
            server: server.clone(),
        };
        let list = known.entry(name_key(&f.name)).or_default();
        if !list.contains(&entry) {
            list.retain(|k| k.server != server);
            list.push(entry);
            list.truncate(8);
            changed = true;
        }
    }
    // Bounded, whatever a server sends.
    while known.len() > MAX_KNOWN {
        let Some(key) = known.keys().next().cloned() else { break };
        known.remove(&key);
        changed = true;
    }
    if changed {
        if let (Some(dir), Ok(json)) = (FOLDER.get(), serde_json::to_vec_pretty(known)) {
            // Through a temporary file, so a crash never leaves half a file.
            let tmp = dir.join(format!("{KNOWN_FILE}.tmp"));
            if let Err(e) = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, dir.join(KNOWN_FILE))) {
                warn!("Couldn't save {KNOWN_FILE}: {e}");
            }
        }
    }
}

/// Marks `players` who share a name with a friend from another server but
/// have a different identity (or none).
fn mark_lookalikes(players: &mut [Player], known: &std::collections::HashMap<String, Vec<Known>>) {
    let server = server_name();
    for p in players.iter_mut().filter(|p| p.relation != Relation::Friend) {
        let Some(list) = known.get(&name_key(&p.name)) else { continue };
        let same_person = list.iter().any(|k| !p.identity.is_empty() && k.identity == p.identity);
        p.lookalike = if same_person {
            None
        } else {
            list.iter().find(|k| k.server != server).map(|k| k.server.clone())
        };
    }
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
static ACTIONS: OnceLock<Mutex<mpsc::Sender<Action>>> = OnceLock::new();

/// The latest snapshot (empty until the first refresh).
pub fn snapshot() -> Snapshot {
    SNAPSHOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().unwrap_or_default()
}

fn update(f: impl FnOnce(&mut Snapshot)) {
    let mut guard = SNAPSHOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f(guard.get_or_insert_with(Snapshot::default));
}

fn send(action: Action) {
    if let Some(tx) = ACTIONS.get() {
        let _ = tx.lock().unwrap_or_else(std::sync::PoisonError::into_inner).send(action);
    }
}

fn say(text: String, error: bool) {
    update(|s| {
        s.notice = Some(Notice { text, error, at: Instant::now() });
    });
}

/// Asks for a refresh now (e.g. when the overlay opens).
pub fn refresh() {
    send(Action::Refresh);
}

/// Invites a player into our current session.
pub fn invite(id: String, name: String) {
    send(Action::Invite { id, name });
}

/// Searches players by part of their name (everyone online for "").
pub fn search(query: String) {
    send(Action::Search(query));
}

/// Changes how we stand to a player (a friend request, a block...).
pub fn change(change: FriendChange, id: String, name: String) {
    send(Action::Change { change, id, name });
}

/// A friend event from the server's event poll: shows it and refreshes.
pub fn friend_event(event: &server_api::misc::FriendEvent) {
    let Some(from) = event.from.as_ref() else {
        return;
    };
    let text = match event.kind() {
        server_api::misc::friend_event::Kind::Request => format!("{} wants to be friends. Press F5 to answer.", from.username),
        server_api::misc::friend_event::Kind::Accepted => format!("{} accepted your friend request.", from.username),
    };
    info!("Friend event: {text}");
    say(text, false);
    refresh();
}

/// Starts the background thread. Safe to call more than once.
pub fn start() {
    if ACTIONS.get().is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    if ACTIONS.set(Mutex::new(tx)).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name(String::from("overlay-data")).spawn(move || {
        refresh_now();
        loop {
            match rx.recv_timeout(REFRESH_EVERY) {
                Ok(Action::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => refresh_now(),
                Ok(Action::Invite { id, name }) => invite_now(&id, &name),
                Ok(Action::Search(query)) => search_now(&query),
                Ok(Action::Change { change, id, name }) => {
                    change_now(change, &id, &name);
                    refresh_now();
                    // Keep the search's relations current too.
                    if let Some((query, _)) = snapshot().search {
                        search_now(&query);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    if let Err(e) = spawned {
        warn!("Couldn't start the overlay data thread: {e}");
    }
}

/// The server's reason, when it gave one people can read.
fn reason(e: &crate::api::Error) -> Option<String> {
    match e {
        crate::api::Error::GRPCStatus(s) if !s.message().is_empty() && s.code() != tonic::Code::Internal && s.code() != tonic::Code::Unavailable => Some(s.message().to_string()),
        _ => None,
    }
}

fn invite_now(id: &str, name: &str) {
    match crate::api::invite_friend(id) {
        Ok(()) => say(format!("Invite sent to {name}"), false),
        Err(e) => {
            warn!("Invite to {name} failed: {e}");
            let why = reason(&e).unwrap_or_else(|| String::from("Host or join a match first, then try again."));
            say(format!("Couldn't invite {name}. {why}"), true);
        }
    }
}

fn search_now(query: &str) {
    match crate::api::search_players(query) {
        Ok(found) => update(|s| {
            let mut found: Vec<Player> = found.into_iter().map(player).collect();
            mark_lookalikes(&mut found, &s.known);
            s.search = Some((query.to_string(), found));
        }),
        Err(e) => {
            warn!("Search for {query:?} failed: {e}");
            say(reason(&e).unwrap_or_else(|| String::from("Couldn't search right now.")), true);
        }
    }
}

fn change_now(change: FriendChange, id: &str, name: &str) {
    let done = match change {
        FriendChange::Request => format!("Friend request sent to {name}"),
        FriendChange::Accept => format!("You and {name} are friends"),
        FriendChange::Decline => format!("Request from or to {name} removed"),
        FriendChange::Remove => format!("{name} is no longer your friend"),
        FriendChange::Block => format!("Blocked {name}. They can't see you or invite you."),
        FriendChange::Unblock => format!("Unblocked {name}"),
    };
    match crate::api::change_friend(change, id) {
        Ok(()) => say(done, false),
        Err(e) => {
            warn!("{change:?} {name} failed: {e}");
            say(reason(&e).unwrap_or_else(|| format!("Couldn't change {name}; try again.")), true);
        }
    }
}

fn refresh_now() {
    let host = crate::config::get().and_then(|c| c.api_server.host_str().map(String::from));
    let started = Instant::now();
    let lists = crate::api::relationships();
    let response_time = started.elapsed();
    let server = host.as_deref().and_then(|h| http_get_json::<ServerInfo>(h, "/api/info"));

    update(|s| {
        match lists {
            Ok(lists) => {
                if s.known.is_empty() {
                    s.known = load_known();
                }
                s.my_name_conflict = lists.my_name() == friends::NameStatus::Conflict;
                s.my_identity = lists.my_identity.clone();
                s.friends = sorted(lists.friends.into_iter().map(player).collect());
                let friends = s.friends.clone();
                remember(&mut s.known, &friends);
                s.requests_in = lists.requests_received.into_iter().map(player).collect();
                let known = s.known.clone();
                mark_lookalikes(&mut s.requests_in, &known);
                s.requests_out = lists.requests_sent.into_iter().map(player).collect();
                s.blocked = lists.blocked.into_iter().map(player).collect();
                s.everyone_mode = lists.mode != "mutual";
                s.response_time = Some(response_time);
                s.loaded = true;
            }
            Err(e) => {
                warn!("Couldn't load friends: {e}");
                s.response_time = None;
            }
        }
        if server.is_some() {
            s.server = server;
        }
    });
}

/// Online first, then by name.
fn sorted(mut players: Vec<Player>) -> Vec<Player> {
    players.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    players
}

fn player(p: friends::Player) -> Player {
    let relation = match p.relation() {
        friends::Relation::Friend => Relation::Friend,
        friends::Relation::RequestSent => Relation::RequestSent,
        friends::Relation::RequestReceived => Relation::RequestReceived,
        friends::Relation::Blocked => Relation::Blocked,
        friends::Relation::None => Relation::None,
    };
    let name_status = match p.name() {
        friends::NameStatus::Unlinked => NameStatus::Unlinked,
        friends::NameStatus::Linked => NameStatus::Linked,
        friends::NameStatus::Reserved => NameStatus::Reserved,
        friends::NameStatus::Conflict => NameStatus::Conflict,
    };
    Player {
        activity: if p.is_online { p.activity.as_ref().map(describe) } else { None },
        id: p.id,
        name: p.username,
        online: p.is_online,
        relation,
        identity: p.identity,
        name_status,
        lookalike: None,
    }
}

/// "Spies vs Mercs · in a match with Tank".
fn describe(a: &friends::Activity) -> String {
    let mode = match a.mode.as_str() {
        "svm" => "Spies vs Mercs",
        "coop" => "Co-op",
        other => other,
    };
    let room = match (a.room.as_str(), a.with.as_slice()) {
        ("match", []) => String::from("in a match"),
        ("match", with) => format!("in a match with {}", with.join(", ")),
        (_, []) => String::from("in a lobby"),
        (_, with) => format!("in a lobby with {}", with.join(", ")),
    };
    format!("{mode} · {room}")
}

/// A GET to the server's HTTP API (port 80), decoded as JSON. Errors are
/// logged and give `None`: the overlay shows what it has.
fn http_get_json<T: for<'de> Deserialize<'de>>(host: &str, path: &str) -> Option<T> {
    match http_get(host, path) {
        Ok(body) => serde_json::from_slice(&body).map_err(|e| warn!("{path}: {e}")).ok(),
        Err(e) => {
            warn!("{path}: {e}");
            None
        }
    }
}

fn http_get(host: &str, path: &str) -> std::io::Result<Vec<u8>> {
    let addr = (host, 80)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no address"))?;
    let mut stream = TcpStream::connect_timeout(&addr, HTTP_TIMEOUT)?;
    stream.set_read_timeout(Some(HTTP_TIMEOUT))?;
    stream.set_write_timeout(Some(HTTP_TIMEOUT))?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // A server dripping bytes mustn't hold this thread: 3 s per read, 8 s in all.
    let deadline = Instant::now() + Duration::from_secs(8);
    // The server answers HTTP/1.0 with a Content-Length; read up to it.
    loop {
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the server took too long"));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some((head, body)) = split_response(&buf) {
            if let Some(len) = content_length(head) {
                if body.len() >= len {
                    break;
                }
            }
        }
        if buf.len() > 1 << 20 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "response too large"));
        }
    }
    let (head, body) = split_response(&buf).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no HTTP header"))?;
    let status = head.lines().next().unwrap_or_default();
    if !status.split_whitespace().nth(1).is_some_and(|code| code == "200") {
        return Err(std::io::Error::other(format!("server answered {status}")));
    }
    let body = match content_length(head) {
        Some(len) => &body[..len.min(body.len())],
        None => body,
    };
    Ok(body.to_vec())
}

fn split_response(buf: &[u8]) -> Option<(&str, &[u8])> {
    let i = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some((std::str::from_utf8(&buf[..i]).ok()?, &buf[i + 4..]))
}

fn content_length(head: &str) -> Option<usize> {
    head.lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_player(name: &str, online: bool) -> Player {
        Player {
            id: name.into(),
            name: name.into(),
            online,
            activity: None,
            relation: Relation::Friend,
            identity: String::new(),
            name_status: NameStatus::Unlinked,
            lookalike: None,
        }
    }

    #[test]
    fn a_name_twin_from_another_server_is_marked() {
        let mut known = std::collections::HashMap::new();
        known.insert(
            String::from("kiwi"),
            vec![Known {
                name: "Kiwi".into(),
                identity: "AAAA-BBBB".into(),
                server: "other.example.com".into(),
            }],
        );
        let mut stranger = test_player("KIWI", true);
        stranger.relation = Relation::None;
        let mut same = stranger.clone();
        same.identity = String::from("AAAA-BBBB");
        let mut players = vec![stranger, same, test_player("Kiwi", true)];
        mark_lookalikes(&mut players, &known);
        assert_eq!(players[0].lookalike.as_deref(), Some("other.example.com"), "another identity, or none");
        assert_eq!(players[1].lookalike, None, "the same identity is the same friend");
        assert_eq!(players[2].lookalike, None, "a friend here isn't questioned");
    }

    #[test]
    fn players_read_their_relation_and_activity() {
        let mut p = friends::Player {
            id: "1".into(),
            username: "Wingduck".into(),
            is_online: true,
            relation: 0,
            identity: String::new(),
            name: 0,
            activity: Some(friends::Activity {
                mode: "svm".into(),
                room: "match".into(),
                with: vec!["Tank".into()],
                map: 7,
            }),
        };
        p.set_relation(friends::Relation::RequestReceived);
        let got = player(p.clone());
        assert_eq!(got.relation, Relation::RequestReceived);
        assert_eq!(got.activity.as_deref(), Some("Spies vs Mercs · in a match with Tank"));
        p.is_online = false;
        assert_eq!(player(p).activity, None, "no activity while offline");
        let list = sorted(vec![test_player("zed", false), test_player("Mjewbear", true), test_player("anna", true)]);
        let names: Vec<_> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["anna", "Mjewbear", "zed"], "online first, then by name");
    }

    #[test]
    fn http_response_parts() {
        let raw = b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        let (head, body) = split_response(raw).unwrap();
        assert_eq!(content_length(head), Some(2));
        assert_eq!(body, b"{}");
    }
}
