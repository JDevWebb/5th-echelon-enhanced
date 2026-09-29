//! What the overlay shows about the server and the people on it.
//!
//! Everything here runs on a background thread: the API calls block, and the
//! render loop must never wait on the network. The render loop only reads the
//! latest [`Snapshot`] and queues actions (refresh, invite).

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
use tracing::info;
use tracing::warn;

/// How often the player list refreshes on its own.
const REFRESH_EVERY: Duration = Duration::from_secs(15);
/// Timeout for the server's HTTP API (port 80).
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

/// Someone on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    /// The API's user id, needed to invite them.
    pub id: String,
    pub name: String,
    pub online: bool,
    /// What they're doing, e.g. "Spies vs Mercs · in a match with Tank".
    pub activity: Option<String>,
}

/// What the server says about itself (`/api/info`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub revision: String,
}

/// A short message for the player about something they did.
#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// The latest state for the overlay.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Everyone but ourselves, online first, then by name.
    pub players: Vec<Player>,
    pub server: Option<ServerInfo>,
    /// Round trip of the last successful player refresh.
    pub response_time: Option<Duration>,
    /// Set once the player list has loaded at least once.
    pub loaded: bool,
    pub notice: Option<Notice>,
}

impl Snapshot {
    pub fn online_count(&self) -> usize {
        self.players.iter().filter(|p| p.online).count()
    }

    /// The activity of the named player, if they're online and doing something.
    pub fn activity_of(&self, name: &str) -> Option<&str> {
        self.players.iter().find(|p| p.name == name).and_then(|p| p.activity.as_deref())
    }
}

enum Action {
    Refresh,
    Invite { id: String, name: String },
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

/// Asks for a refresh now (e.g. when the overlay opens).
pub fn refresh() {
    send(Action::Refresh);
}

/// Invites a player into our current session.
pub fn invite(id: String, name: String) {
    send(Action::Invite { id, name });
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
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    if let Err(e) = spawned {
        warn!("Couldn't start the overlay data thread: {e}");
    }
}

fn invite_now(id: &str, name: &str) {
    let result = crate::api::invite_friend(id);
    let notice = match &result {
        Ok(()) => Notice {
            text: format!("Invite sent to {name}"),
            error: false,
            at: Instant::now(),
        },
        Err(e) => {
            warn!("Invite to {name} failed: {e}");
            Notice {
                text: format!("Couldn't invite {name}. Host or join a match first, then try again."),
                error: true,
                at: Instant::now(),
            }
        }
    };
    info!("Invite to {name}: {:?}", result.is_ok());
    update(|s| s.notice = Some(notice));
}

fn refresh_now() {
    let host = crate::config::get().and_then(|c| c.api_server.host_str().map(String::from));
    let started = Instant::now();
    let friends = crate::api::list_friends();
    let response_time = started.elapsed();
    let presence = host.as_deref().and_then(|h| http_get_json::<Presence>(h, "/api/presence"));
    let server = host.as_deref().and_then(|h| http_get_json::<ServerInfo>(h, "/api/info"));
    let me = crate::api::username();

    update(|s| {
        match friends {
            Ok(friends) => {
                s.players = merge(friends, presence.as_ref(), me.as_deref());
                s.response_time = Some(response_time);
                s.loaded = true;
            }
            Err(e) => {
                warn!("Couldn't load the player list: {e}");
                s.response_time = None;
            }
        }
        if server.is_some() {
            s.server = server;
        }
    });
}

/// `/api/presence` (when the server has it on): every registered player and what they're doing.
#[derive(Debug, Deserialize)]
struct Presence {
    players: Vec<PresencePlayer>,
}

#[derive(Debug, Deserialize)]
struct PresencePlayer {
    name: String,
    #[serde(default)]
    activity: Option<Activity>,
}

#[derive(Debug, Deserialize)]
struct Activity {
    mode: String,
    room: String,
    #[serde(default)]
    with: Vec<String>,
}

impl Activity {
    fn describe(&self) -> String {
        let mode = match self.mode.as_str() {
            "svm" => "Spies vs Mercs",
            "coop" => "Co-op",
            other => other,
        };
        let room = match (self.room.as_str(), self.with.as_slice()) {
            ("match", []) => String::from("in a match"),
            ("match", with) => format!("in a match with {}", with.join(", ")),
            (_, []) => String::from("in a lobby"),
            (_, with) => format!("in a lobby with {}", with.join(", ")),
        };
        format!("{mode} · {room}")
    }
}

/// Combines the API's friend list (ids, online state) with presence
/// (activities), leaving ourselves out.
fn merge(friends: Vec<crate::api::Friend>, presence: Option<&Presence>, me: Option<&str>) -> Vec<Player> {
    let mut players: Vec<Player> = friends
        .into_iter()
        .filter(|f| Some(f.username.as_str()) != me)
        .map(|f| {
            let activity = presence
                .and_then(|p| p.players.iter().find(|p| p.name == f.username))
                .and_then(|p| p.activity.as_ref())
                .map(Activity::describe);
            Player {
                id: f.id,
                activity: if f.is_online { activity } else { None },
                online: f.is_online,
                name: f.username,
            }
        })
        .collect();
    players.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    players
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
    // The server answers HTTP/1.0 with a Content-Length; read up to it.
    loop {
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

    fn friend(id: &str, name: &str, online: bool) -> crate::api::Friend {
        crate::api::Friend {
            id: id.into(),
            username: name.into(),
            is_online: online,
            session_id: 0,
            invite_only: false,
            session_data: Vec::new(),
            pid: 0,
        }
    }

    #[test]
    fn merge_sorts_online_first_and_adds_activities() {
        let presence: Presence = serde_json::from_str(
            r#"{"players":[
                {"name":"Wingduck","online":true,"activity":{"mode":"svm","room":"match","map":7,"with":["Tank"]}},
                {"name":"Mjewbear","online":true,"activity":{"mode":"coop","room":"lobby","map":2,"with":[]}},
                {"name":"NexusX","online":true}
            ]}"#,
        )
        .unwrap();
        let friends = vec![
            friend("3", "zed", false),
            friend("1", "Wingduck", true),
            friend("9", "NexusX", true),
            friend("2", "Mjewbear", true),
        ];
        let players = merge(friends, Some(&presence), Some("NexusX"));
        let names: Vec<_> = players.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Mjewbear", "Wingduck", "zed"], "online first, ourselves left out");
        assert_eq!(players[0].activity.as_deref(), Some("Co-op · in a lobby"));
        assert_eq!(players[1].activity.as_deref(), Some("Spies vs Mercs · in a match with Tank"));
        assert_eq!(players[2].activity, None);
    }

    #[test]
    fn http_response_parts() {
        let raw = b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        let (head, body) = split_response(raw).unwrap();
        assert_eq!(content_length(head), Some(2));
        assert_eq!(body, b"{}");
    }
}
