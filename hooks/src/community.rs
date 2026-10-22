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

use hooks_config::text::clip;
use serde::Deserialize;
use server_api::friends;
use tracing::info;
use tracing::warn;

pub use crate::api::FriendChange;

/// How often the lists refresh on their own.
const REFRESH_EVERY: Duration = Duration::from_secs(15);
/// Timeout for the server's HTTP API (port 80), to connect and for each read. Mobile
/// connections stall for a few seconds now and then: at 3 s, PlaySkill's game (eu1,
/// 2026-10-06) timed out on about 1 request in 100.
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
/// A request may take this long in all (a server dripping bytes mustn't hold the thread).
const HTTP_DEADLINE: Duration = Duration::from_secs(12);
/// Failed requests in a row before it's worth a line (one stall misses one refresh, and the
/// overlay keeps what it had).
const FAILURES_TOLD: u32 = 3;
static FAILURES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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
    /// What they're doing, e.g. "SvM Classic · Virus Vault · in a match with Tank".
    pub activity: Option<String>,
    /// They're in a party (as its host or a member) or a public match: the room a friend joins
    /// them in without an invitation. Not in a private match, which needs one.
    pub joinable: bool,
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
    /// What the server supports ("join-now": a friend's game can be joined without an
    /// invitation); empty from servers before features were listed.
    #[serde(default)]
    pub features: Vec<String>,
    /// Maintenance booked for this server or the whole network, not over yet.
    #[serde(default)]
    pub maintenance: Vec<Maintenance>,
}

/// A maintenance window (Unix seconds), as the server heard it from the coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Maintenance {
    pub start: i64,
    pub end: i64,
    #[serde(default)]
    pub note: String,
    /// The whole network (the coordinator), not this server: games carry on.
    #[serde(default)]
    pub network: bool,
    /// For installing a release: this server restarts (a whole network's updates every
    /// server).
    #[serde(default)]
    pub update: bool,
}

/// How long before a window the overlay warns.
pub const MAINTENANCE_WARNING: i64 = 600;

/// "about 1 hour", "about an hour and a half": a window's length (in words: the overlay's
/// fonts may not have "½").
fn length(secs: i64) -> String {
    let minutes = (secs + 59) / 60;
    if minutes < 60 {
        let rounded = ((minutes + 4) / 5 * 5).max(5);
        return format!("about {rounded} minutes");
    }
    let halves = (minutes + 15) / 30;
    match (halves / 2, halves % 2) {
        (1, 0) => String::from("about 1 hour"),
        (1, _) => String::from("about an hour and a half"),
        (h, 0) => format!("about {h} hours"),
        (h, _) => format!("about {h} and a half hours"),
    }
}

/// The warning the overlay shows `now` (Unix seconds), from 10 minutes before the next window
/// until it starts: a heading and a line.
pub fn maintenance_warning(windows: &[Maintenance], now: i64) -> Option<(String, String)> {
    let w = windows.iter().filter(|w| w.start > now && w.start - now <= MAINTENANCE_WARNING).min_by_key(|w| w.start)?;
    let minutes = ((w.start - now + 59) / 60).max(1);
    let when = if minutes == 1 { String::from("1 min") } else { format!("{minutes} min") };
    let note = if w.note.is_empty() { String::new() } else { format!(": {}", w.note) };
    Some(if w.update {
        // The note an update window gets when booked without one says no more.
        let note = if w.note == "Server update" { "" } else { note.as_str() };
        (
            format!("Server update in {when}"),
            format!("This server restarts with a new release, down for about two minutes{note}. Finish your match before then."),
        )
    } else if w.network {
        (
            format!("Network maintenance in {when}"),
            format!("Friends and invites pause for {}{note}. Your match carries on.", length(w.end - w.start)),
        )
    } else {
        (
            format!("Server maintenance in {when}"),
            format!("This server goes down for {}{note}. Finish your match before then.", length(w.end - w.start)),
        )
    })
}

/// A short message for the player about something they did, or something
/// that happened (a friend request).
#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// Something that just went wrong for the player, as the server explained it (a join that
/// failed, and why): the overlay shows it at once.
#[derive(Debug, Clone)]
pub struct Problem {
    pub title: String,
    pub text: String,
    pub at: Instant,
}

/// The longest problem title and text shown.
const MAX_PROBLEM_TITLE: usize = 80;
const MAX_PROBLEM_TEXT: usize = 400;

/// The longest player name shown (the server's own limit).
const MAX_NAME: usize = 32;
/// The longest server message, activity or server name shown.
const MAX_TEXT: usize = 96;

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
    /// What just went wrong, from the server's notice.
    pub problem: Option<Problem>,
    /// Our own name belongs to another player on the servers sharing
    /// friends: we should rename.
    pub my_name_conflict: bool,
    /// The network's admins asked me to choose another name.
    pub my_name_not_allowed: bool,
    /// Our identity's short form, empty if not linked.
    pub my_identity: String,
    /// Friends playing on another server sharing friends: their name there,
    /// and that server's name (with its region).
    pub elsewhere: Vec<(String, String)>,
    /// What's being played on this server (`/api/playing`, servers from 0.4.4): public
    /// matches by mode, map and players, private matches and lobbies counted.
    pub playing: Option<server_api::playing::Playing>,
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

    /// Whether this server joins a friend's game without an invitation (`JoinNow`): a server
    /// without it would answer the overlay's Join with "unimplemented".
    pub fn join_offered(&self) -> bool {
        self.server.as_ref().is_some_and(|s| s.features.iter().any(|f| f == "join-now"))
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
    Invite {
        id: String,
        name: String,
    },
    Join {
        id: String,
        name: String,
    },
    Search(String),
    Change {
        change: FriendChange,
        id: String,
        identity: String,
        name: String,
    },
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

/// Where the game is, once set.
pub fn folder() -> Option<&'static std::path::Path> {
    FOLDER.get().map(std::path::PathBuf::as_path)
}

/// The launcher's join request, next to `uplay.toml`: the friend (account id) the player
/// clicked Join on there, and when (Unix seconds). The launcher is another process, so this is
/// how it tells the game which join it asked for; read and deleted when the invitation arrives
/// (or at start, for a game started for the join).
const JOIN_REQUEST_FILE: &str = "5th-echelon-join-request.json";
/// How long a join request (ours or the launcher's) stays good: long enough for a game the
/// launcher started to boot and sign in, short enough that it can't attach to a later,
/// unrelated invitation from the same friend.
const JOIN_REQUEST_FOR: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Deserialize)]
struct LauncherJoinRequest {
    id: String,
    at: i64,
}

/// The friend we asked to join (the overlay's Join, or the launcher's), by account id, and
/// when: the one invitation we take without a click. None once taken.
static JOIN_REQUESTED: Mutex<Option<(String, Instant)>> = Mutex::new(None);

fn note_join_request(id: &str) {
    *JOIN_REQUESTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((id.to_string(), Instant::now()));
}

fn forget_join_request(id: &str) {
    let mut requested = JOIN_REQUESTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if requested.as_ref().is_some_and(|(r, _)| r == id) {
        *requested = None;
    }
}

/// Takes the launcher's join request file, if there is one, as a request of ours: deleted
/// once read, so it's good for one join only.
fn take_launcher_join_request() {
    let Some(path) = folder().map(|d| d.join(JOIN_REQUEST_FILE)) else { return };
    let Ok(bytes) = std::fs::read(&path) else { return };
    if let Err(e) = std::fs::remove_file(&path) {
        warn!("Couldn't delete the launcher's join request {}: {e}", path.display());
    }
    let Ok(request) = serde_json::from_slice::<LauncherJoinRequest>(&bytes) else {
        warn!("The launcher's join request can't be read; ignored");
        return;
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    // Dated ahead (beyond a little clock slack) it would outlive its time: not one of the
    // launcher's.
    if request.at > now + 60 || !crate::api::is_plain_id(&request.id) {
        warn!("The launcher's join request isn't a valid one; ignored");
        return;
    }
    let age = Duration::from_secs(now.saturating_sub(request.at).max(0) as u64);
    if age >= JOIN_REQUEST_FOR {
        info!("The launcher's join request is stale ({}s old); ignored", age.as_secs());
        return;
    }
    info!("The launcher asked to join {}", request.id);
    // Dated when the launcher wrote it, so its time runs from the click, not from here.
    let at = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
    let mut requested = JOIN_REQUESTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *requested = Some((request.id, at));
}

/// Whether an invitation from `sender` (account id) is the join we asked for — from the
/// overlay's Join or the launcher's — within [`JOIN_REQUEST_FOR`], once: the game joins it
/// without a click. Any other invitation, whatever the server says of it, is an ordinary one.
pub fn requested_join(sender: &str) -> bool {
    let matches = |requested: &Option<(String, Instant)>| requested.as_ref().is_some_and(|(id, at)| id == sender && at.elapsed() < JOIN_REQUEST_FOR);
    let mut requested = JOIN_REQUESTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if !matches(&requested) {
        // The launcher may have asked while the game was running.
        drop(requested);
        take_launcher_join_request();
        requested = JOIN_REQUESTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    if matches(&requested) {
        *requested = None;
        return true;
    }
    false
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

pub(crate) fn say(text: String, error: bool) {
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

/// Joins a friend's game without an invitation (the overlay's "Join"). Remembered as asked
/// for, so the invitation it brings is taken without a click (see [`requested_join`]).
pub fn join(id: String, name: String) {
    note_join_request(&id);
    send(Action::Join { id, name });
}

/// Searches players by part of their name (everyone online for "").
pub fn search(query: String) {
    send(Action::Search(query));
}

/// Changes how we stand to a player (a friend request, a block...).
pub fn change(change: FriendChange, id: String, name: String) {
    send(Action::Change {
        change,
        id,
        identity: String::new(),
        name,
    });
}

/// Changes how we stand to a chat message's sender, by their identity (chat has no account id).
pub fn change_by_identity(change: FriendChange, identity: String, name: String) {
    send(Action::Change {
        change,
        id: String::new(),
        identity,
        name,
    });
}

/// A friend event from the server's event poll: shows it and refreshes.
pub fn friend_event(event: &server_api::misc::FriendEvent) {
    let Some(from) = event.from.as_ref() else {
        return;
    };
    let text = match event.kind() {
        server_api::misc::friend_event::Kind::Request => format!(
            "{} wants to be friends. Press {} to answer.",
            clip(&from.username, MAX_NAME),
            hooks_config::get().map_or_else(|| "F5".into(), |c| hooks_config::overlay_key_hint(&c.overlay_key))
        ),
        server_api::misc::friend_event::Kind::Accepted => format!("{} accepted your friend request.", clip(&from.username, MAX_NAME)),
    };
    info!("Friend event: {text}");
    say(text, false);
    refresh();
}

/// A notice from the server's event poll: what just went wrong, and why. As any server text,
/// cut and cleaned; logged, so a report shows what the player was told.
pub fn problem(notice: &server_api::misc::Notice) {
    let title = clip(&notice.title, MAX_PROBLEM_TITLE);
    let text = clip(&notice.text, MAX_PROBLEM_TEXT);
    if title.is_empty() && text.is_empty() {
        return;
    }
    info!("Told the player: {title}: {text}");
    update(|s| {
        s.problem = Some(Problem { title, text, at: Instant::now() });
    });
}

/// Something the hooks did on the player's behalf, and why: the same card as a server's notice.
pub fn tell(title: &str, text: &str) {
    info!("Told the player: {title}: {text}");
    let (title, text) = (title.to_string(), text.to_string());
    update(|s| {
        s.problem = Some(Problem { title, text, at: Instant::now() });
    });
}

/// Answers from the network's admins the player hasn't read, as the server last said.
static SUPPORT_UNREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The server's count of the player's unread support answers, from each event poll: a
/// notice when it goes up (a new answer), none while it stays (they've been told).
pub fn support_unread(count: u32) {
    let before = SUPPORT_UNREAD.swap(count, std::sync::atomic::Ordering::Relaxed);
    if count > before {
        info!("Support: {count} answer(s) from the admins unread");
        say("The admins answered your support message. Read it on the launcher's Support page.".into(), false);
    }
}

/// The players the mode of the match the player is in is made for, as the server last said
/// (0: not in a match, or not known).
static MATCH_SIZE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The server's size for the mode of the player's match, from each event poll.
pub fn set_match_size(size: u32) {
    let before = MATCH_SIZE.swap(size, std::sync::atomic::Ordering::Relaxed);
    if size != before && size != 0 {
        info!("The match's mode is made for {size} players");
    }
}

/// The players the mode of the player's match is made for, if known.
pub fn match_size() -> Option<i32> {
    i32::try_from(MATCH_SIZE.load(std::sync::atomic::Ordering::Relaxed)).ok().filter(|n| *n > 0)
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
        // A game the launcher started for a join: its request is taken up now, so the
        // invitation finds it (and a stale one is cleared).
        take_launcher_join_request();
        refresh_now();
        loop {
            match rx.recv_timeout(REFRESH_EVERY) {
                Ok(Action::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => refresh_now(),
                Ok(Action::Invite { id, name }) => invite_now(&id, &name),
                Ok(Action::Join { id, name }) => join_now_impl(&id, &name),
                Ok(Action::Search(query)) => search_now(&query),
                Ok(Action::Change { change, id, identity, name }) => {
                    change_now(change, &id, &identity, &name);
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

/// The server's reason, when it gave one people can read: one line, cut short.
fn reason(e: &crate::api::Error) -> Option<String> {
    match e {
        crate::api::Error::GRPCStatus(s) if s.code() != tonic::Code::Internal && s.code() != tonic::Code::Unavailable => {
            Some(clip(s.message(), MAX_TEXT)).filter(|m| !m.is_empty())
        }
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

fn join_now_impl(id: &str, name: &str) {
    use server_api::friends::join_now_response::Status as JoinStatus;
    match crate::api::join_friend(id) {
        Ok(resp) => match JoinStatus::try_from(resp.status).unwrap_or(JoinStatus::Offline) {
            JoinStatus::Ok => {
                say(format!("Joining {name}..."), false);
                return;
            }
            _ if !resp.detail.is_empty() => say(resp.detail, true),
            JoinStatus::Offline => say(format!("{name} isn't in a game right now."), true),
            JoinStatus::Private => say(format!("{name} is in a private match - you need an invitation."), true),
            JoinStatus::NotFriendsWithHost => say(format!("{name} can't bring you in: the game's host doesn't have them as a friend."), true),
            _ => say(format!("Couldn't join {name}."), true),
        },
        Err(e) => {
            warn!("Join {name} failed: {e}");
            say(reason(&e).unwrap_or_else(|| format!("Couldn't join {name}; try again.")), true);
        }
    }
    // Nothing queued: an invitation from them later is an ordinary one.
    forget_join_request(id);
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

fn change_now(change: FriendChange, id: &str, identity: &str, name: &str) {
    let done = match change {
        FriendChange::Request => format!("Friend request sent to {name}"),
        FriendChange::Accept => format!("You and {name} are friends"),
        FriendChange::Decline => format!("Request from or to {name} removed"),
        FriendChange::Remove => format!("{name} is no longer your friend"),
        FriendChange::Block => format!("Blocked {name}. They can't see you or invite you."),
        FriendChange::Unblock => format!("Unblocked {name}"),
    };
    match crate::api::change_friend(change, id, identity) {
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
    let server = host.as_deref().and_then(|h| http_get_json::<ServerInfo>(h, "/api/info")).map(|i| ServerInfo {
        name: clip(&i.name, MAX_TEXT),
        version: clip(&i.version, MAX_NAME),
        revision: clip(&i.revision, MAX_NAME),
        // Only the one looked at, kept short: a server's list, like all its text.
        features: i.features.iter().filter(|f| *f == "join-now").map(|f| clip(f, MAX_NAME)).collect(),
        maintenance: i
            .maintenance
            .into_iter()
            .take(4)
            .map(|m| Maintenance {
                note: clip(&m.note, MAX_TEXT),
                ..m
            })
            .collect(),
    });
    let playing = host
        .as_deref()
        .and_then(|h| http_get_json::<server_api::playing::Playing>(h, "/api/playing"))
        .map(server_api::playing::Playing::checked);

    update(|s| {
        match lists {
            Ok(lists) => {
                if s.known.is_empty() {
                    s.known = load_known();
                }
                s.my_name_conflict = lists.my_name() == friends::NameStatus::Conflict;
                s.my_name_not_allowed = lists.my_name() == friends::NameStatus::NotAllowed;
                s.my_identity = clip(&lists.my_identity, MAX_NAME);
                s.friends = sorted(lists.friends.into_iter().map(player).collect());
                tell_game_if_changed(&s.friends);
                let friends = s.friends.clone();
                remember(&mut s.known, &friends);
                s.requests_in = lists.requests_received.into_iter().map(player).collect();
                let known = s.known.clone();
                mark_lookalikes(&mut s.requests_in, &known);
                s.requests_out = lists.requests_sent.into_iter().map(player).collect();
                s.blocked = lists.blocked.into_iter().map(player).collect();
                s.everyone_mode = lists.mode != "mutual";
                s.elsewhere = lists
                    .elsewhere
                    .iter()
                    .take(100)
                    .map(|e| {
                        let server = if e.region.is_empty() {
                            e.server.clone()
                        } else {
                            format!("{} ({})", e.server, e.region)
                        };
                        (clip(&e.username, MAX_NAME), clip(&server, MAX_TEXT))
                    })
                    .collect();
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
        s.playing = playing;
    });
}

/// Who was on the friend list at the last refresh, by id. None before the first one.
static LAST_FRIENDS: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// Has the game fetch its friend list again when someone joined or left ours since the last
/// refresh. The game reads the list itself at the online menu, so the first refresh only
/// remembers it.
fn tell_game_if_changed(friends: &[Player]) {
    let mut ids: Vec<String> = friends.iter().map(|f| f.id.clone()).collect();
    ids.sort_unstable();
    let mut last = LAST_FRIENDS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let changed = last.as_ref().is_some_and(|before| *before != ids);
    *last = Some(ids);
    drop(last);
    if changed && crate::config::get().is_none_or(|c| c.push_friend_list) {
        info!("The friend list changed; asking the game to fetch it again");
        crate::uplay_r1_loader::queue_event(crate::uplay_r1_loader::Event::FriendsListUpdated);
    }
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
        // Only ever my own: never sent for others.
        friends::NameStatus::NotAllowed => NameStatus::Linked,
    };
    Player {
        activity: if p.is_online { p.activity.as_ref().map(describe) } else { None },
        joinable: p.is_online && p.activity.as_ref().is_some_and(|a| a.joinable),
        id: p.id,
        name: clip(&p.username, MAX_NAME),
        online: p.is_online,
        relation,
        identity: clip(&p.identity, MAX_NAME),
        name_status,
        lookalike: None,
    }
}

/// "SvM Classic · Virus Vault · in a match with Tank"; "In a lobby with Tank" (a lobby is
/// for either mode). Everything in it comes from the server: names cleaned, the whole cut
/// to one line.
fn describe(a: &friends::Activity) -> String {
    let with = a.with.iter().take(8).map(|n| clip(n, MAX_NAME)).collect::<Vec<_>>().join(", ");
    let room = match (a.room.as_str(), with.is_empty()) {
        ("match", true) => String::from("in a match"),
        ("match", false) => format!("in a match with {with}"),
        (_, true) => String::from("in a lobby"),
        (_, false) => format!("in a lobby with {with}"),
    };
    let line = match server_api::playing::activity_what(a) {
        Some(what) => format!("{what} · {room}"),
        None => {
            let mut chars = room.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    };
    clip(&line, MAX_TEXT)
}

/// A GET to the server's HTTP API (port 80), decoded as JSON. Errors are
/// logged and give `None`: the overlay shows what it has.
/// A request failing is said once it has failed [`FAILURES_TOLD`] times in a row, and its
/// answering again after that.
fn http_get_json<T: for<'de> Deserialize<'de>>(host: &str, path: &str) -> Option<T> {
    use std::sync::atomic::Ordering;
    match http_get(host, path) {
        Ok(body) => {
            let failed = FAILURES.swap(0, Ordering::Relaxed);
            if failed >= FAILURES_TOLD {
                info!("{path}: the server answers again (after {failed} failed requests)");
            }
            serde_json::from_slice(&body).map_err(|e| warn!("{path}: {e}")).ok()
        }
        Err(e) => {
            if FAILURES.fetch_add(1, Ordering::Relaxed) + 1 == FAILURES_TOLD {
                warn!("{path}: {e} ({FAILURES_TOLD} times in a row)");
            }
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
    let deadline = Instant::now() + HTTP_DEADLINE;
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
    #[test]
    fn maintenance_is_warned_ten_minutes_before() {
        use super::maintenance_warning;
        use super::Maintenance;
        let server = Maintenance {
            start: 10_000,
            end: 13_600,
            note: String::from("Moving to a faster machine"),
            network: false,
            update: false,
        };
        let network = Maintenance {
            start: 20_000,
            end: 21_800,
            note: String::new(),
            network: true,
            update: false,
        };
        let windows = [network.clone(), server];
        assert_eq!(maintenance_warning(&windows, 10_000 - 601), None, "not yet");
        assert_eq!(
            maintenance_warning(&windows, 10_000 - 540),
            Some((
                "Server maintenance in 9 min".into(),
                "This server goes down for about 1 hour: Moving to a faster machine. Finish your match before then.".into()
            ))
        );
        assert_eq!(maintenance_warning(&windows, 10_000 - 20).map(|w| w.0), Some("Server maintenance in 1 min".into()));
        assert_eq!(maintenance_warning(&windows, 10_000), None, "started: the server says the rest");
        assert_eq!(
            maintenance_warning(&windows, 20_000 - 300),
            Some((
                "Network maintenance in 5 min".into(),
                "Friends and invites pause for about 30 minutes. Your match carries on.".into()
            ))
        );
        // A whole network's update window restarts this server too.
        let update = Maintenance {
            update: true,
            note: String::from("Server update"),
            ..network.clone()
        };
        assert_eq!(
            maintenance_warning(&[update], 20_000 - 300),
            Some((
                "Server update in 5 min".into(),
                "This server restarts with a new release, down for about two minutes. Finish your match before then.".into()
            ))
        );
        assert_eq!(super::length(90 * 60), "about an hour and a half");
        assert_eq!(super::length(120 * 60), "about 2 hours");
        assert_eq!(super::length(14 * 60), "about 15 minutes");
    }

    use super::*;

    fn test_player(name: &str, online: bool) -> Player {
        Player {
            id: name.into(),
            name: name.into(),
            online,
            activity: None,
            joinable: false,
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
                game_mode: 0,
                joinable: false,
            }),
        };
        p.set_relation(friends::Relation::RequestReceived);
        let got = player(p.clone());
        assert_eq!(got.relation, Relation::RequestReceived);
        assert_eq!(got.activity.as_deref(), Some("Spies vs Mercs · in a match with Tank"), "an older server");
        let activity = p.activity.as_mut().unwrap();
        (activity.game_mode, activity.map) = (8, 0x05F6_9010);
        assert_eq!(player(p.clone()).activity.as_deref(), Some("SvM Classic · Virus Vault · in a match with Tank"));
        let activity = p.activity.as_mut().unwrap();
        (activity.mode, activity.room) = ("coop".into(), "lobby".into());
        assert_eq!(player(p.clone()).activity.as_deref(), Some("In a lobby with Tank"), "a lobby is for either mode");
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
