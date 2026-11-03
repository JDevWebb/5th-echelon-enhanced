//! The relays this server works with (global matchmaking, G1 item 4): its own NAT helper's,
//! and, as the network's matchmaking server (`[matchmaking] network_relays`), every relay the
//! coordinator lists (its heartbeat answer's `relays`). An address on a relay is a relayed
//! player's, not someone else's network: RegisterURLs and the probe leave it alone rather than
//! rewriting it to the address the player was seen from. `/api/info` offers the relays to
//! launchers, which pick the nearest.

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use serde::Serialize;

/// The most relays taken from the coordinator.
const MAX_RELAYS: usize = 64;

/// A relay on the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relay {
    pub id: String,
    pub name: String,
    pub region: String,
    /// Its host name, when it has one (what launchers tell the game).
    pub host: String,
    pub ip: Ipv4Addr,
    pub port: u16,
    pub ports: (u16, u16),
}

static NETWORK_RELAYS: AtomicBool = AtomicBool::new(false);
static NETWORK: Mutex<Vec<Relay>> = Mutex::new(Vec::new());

/// Whether this server works with the network's relays (`[matchmaking] network_relays`).
/// Called once at start.
pub fn init(network_relays: bool) {
    NETWORK_RELAYS.store(network_relays, Ordering::Relaxed);
}

fn network() -> std::sync::MutexGuard<'static, Vec<Relay>> {
    NETWORK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The network's relays from the coordinator's heartbeat answer (coordinators from 0.4.8 send
/// them); an answer without them leaves the last list. Only public addresses with a sane range.
pub fn from_heartbeat(answer: &serde_json::Value) {
    links_from_heartbeat(answer);
    let Some(list) = answer["relays"].as_array() else { return };
    let text = |v: &serde_json::Value, key: &str| v[key].as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(64).collect::<String>();
    let relays: Vec<Relay> = list
        .iter()
        .filter_map(|v| {
            let ip: Ipv4Addr = v["ip"].as_str()?.parse().ok()?;
            let port = u16::try_from(v["port"].as_u64()?).ok().filter(|p| *p > 0)?;
            let ports = v["ports"].as_array()?;
            let ports = (u16::try_from(ports.first()?.as_u64()?).ok()?, u16::try_from(ports.get(1)?.as_u64()?).ok()?);
            (crate::game_session::is_public(std::net::IpAddr::V4(ip)) && ports.0 > 0 && ports.0 <= ports.1).then(|| Relay {
                id: text(v, "id"),
                name: text(v, "name"),
                region: text(v, "region"),
                host: text(v, "host"),
                ip,
                port,
                ports,
            })
        })
        .take(MAX_RELAYS)
        .collect();
    *network() = relays;
}

/// The relays this server works with, as address and port range: its own helper's, and the
/// network's when it's the matchmaking server. Only an address in one of these ranges is a
/// relayed player's: another port on a relay's address is no player's.
pub fn ranges() -> Vec<(Ipv4Addr, (u16, u16))> {
    let mut ranges: Vec<(Ipv4Addr, (u16, u16))> = crate::nat_helper::relay_range().into_iter().collect();
    if NETWORK_RELAYS.load(Ordering::Relaxed) {
        ranges.extend(network().iter().map(|r| (r.ip, r.ports)));
    }
    ranges
}

/// The relays to offer launchers (`/api/info`): the network's, when this is the matchmaking
/// server; none otherwise (players then use this server's own helper).
pub fn offered() -> Vec<Relay> {
    if NETWORK_RELAYS.load(Ordering::Relaxed) {
        network().clone()
    } else {
        Vec::new()
    }
}

/// Another member's helper this server has a link to (link.rs), from the coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: String,
    pub ip: Ipv4Addr,
    pub port: u16,
    /// The range its relayed players' addresses take their ports from.
    pub ports: (u16, u16),
    pub key: [u8; 32],
    /// It's the network's matchmaking server: relays report to it and ask it about pairs.
    pub matchmaking: bool,
}

static LINKS: Mutex<Vec<Link>> = Mutex::new(Vec::new());
static OWN_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn links() -> std::sync::MutexGuard<'static, Vec<Link>> {
    LINKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// This server's id, which its link messages are sent as. Called once at start.
pub fn set_own_id(id: &str) {
    let _ = OWN_ID.set(id.to_string());
}

/// This server's links from the coordinator's heartbeat answer (`links`); an answer without
/// them leaves the last ones.
fn links_from_heartbeat(answer: &serde_json::Value) {
    let Some(list) = answer["links"].as_array() else { return };
    *links() = list
        .iter()
        .filter_map(|v| {
            Some(Link {
                id: v["id"].as_str()?.chars().take(crate::link::MAX_TEXT).collect(),
                ip: v["ip"].as_str()?.parse().ok()?,
                port: u16::try_from(v["port"].as_u64()?).ok()?,
                ports: {
                    let p = v["ports"].as_array()?;
                    (u16::try_from(p.first()?.as_u64()?).ok()?, u16::try_from(p.get(1)?.as_u64()?).ok()?)
                },
                key: identity::base32_decode(v["key"].as_str()?).and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok())?,
                matchmaking: v["matchmaking"].as_bool().unwrap_or(false),
            })
        })
        .take(MAX_RELAYS)
        .collect();
}

/// A link message that came to the helper's port from `src`: opened only with the key of the
/// link its sender names, from that link's address. On the matchmaking server: a relay's
/// report is kept, and a relay's question about a pair answered.
/// Returns a game packet another relay forwarded, for the helper to deliver ([`Forwarded`]).
pub fn on_link(data: &[u8], src: std::net::SocketAddrV4, socket: &std::net::UdpSocket) -> Option<Forwarded> {
    // The matchmaking server tells relays of pairs from the helper's socket, as it answers them.
    if NETWORK_RELAYS.load(Ordering::Relaxed) && MM_SOCKET.get().is_none() {
        if let Ok(s) = socket.try_clone() {
            let _ = MM_SOCKET.set(s);
        }
    }
    let sender = crate::link::sender(data)?;
    let link = links().iter().find(|l| l.id == sender && l.ip == *src.ip()).cloned()?;
    let (_, body) = crate::link::open(data, &link.key, identity::now())?;
    match body {
        // Another relay's player's packet for one of this helper's: the address it came from is
        // that relay's from now on (a reply to it goes back there).
        crate::link::Body::Forward { from_name, from, to, payload } => {
            learn(from, &link.id);
            return Some(Forwarded { from_name, from, to, payload });
        }
        // Only the network's matchmaking server keeps relays' reports.
        crate::link::Body::Report(regs) if NETWORK_RELAYS.load(Ordering::Relaxed) => remote_report(&link, regs),
        crate::link::Body::Report(_) => {}
        // Looked up beside the packets: a slow database never holds up the relay's traffic.
        crate::link::Body::Ask { nonce, a, b } => {
            if let (Some(queue), Ok(socket)) = (ASKS.get(), socket.try_clone()) {
                let _ = queue.try_send(Question { link, nonce, a, b, src, socket });
            }
        }
        crate::link::Body::Answer { nonce, yes } => answered(nonce, yes, &link.id),
        // Only from the matchmaking server: it put two players together.
        crate::link::Body::Introduce { a, b } if link.matchmaking => crate::nat_helper::introduced(&a, &b),
        crate::link::Body::Introduce { .. } => {}
        // A relay's event about its player, kept here with the player's sessions; only the
        // relay's own kinds, so a relay can't write anything else into them.
        // Only about a player that relay reports (its events follow its reports), and only
        // the fields each kind has.
        crate::link::Body::Event { name, kind, detail } if NETWORK_RELAYS.load(Ordering::Relaxed) => {
            let (kind, fields) = RELAY_EVENTS.iter().copied().find(|(k, _)| *k == kind)?;
            if remote_entry(&name).is_none_or(|(relay, ..)| relay != link.id) {
                return None;
            }
            let sent: serde_json::Value = serde_json::from_str(&detail).unwrap_or_default();
            let mut detail = serde_json::json!({ "relay": link.id });
            for field in fields {
                if let Some(v) = sent.get(*field).filter(|v| v.is_number() || v.is_boolean() || v.as_str().is_some_and(|s| s.len() <= 16)) {
                    detail[*field] = v.clone();
                }
            }
            crate::session_events::note(crate::session_events::Who::Name(name), kind, detail);
        }
        crate::link::Body::Event { .. } => {}
    }
    None
}

/// The helper's socket, on the network's matchmaking server (taken with the first link message).
static MM_SOCKET: std::sync::OnceLock<std::net::UdpSocket> = std::sync::OnceLock::new();

/// Whether this server works with the network's relays (it's the matchmaking server).
pub fn network_relays() -> bool {
    NETWORK_RELAYS.load(Ordering::Relaxed)
}

/// On the network's matchmaking server: `a` and `b` were put together (a search's results, a
/// probe, a join), so the relays they're registered at are told ([`crate::link::Body::Introduce`]),
/// each once; a player at none (this server's own helper) needs nothing.
pub fn introduce_remote(a: &str, b: &str) {
    if !NETWORK_RELAYS.load(Ordering::Relaxed) {
        return;
    }
    let (Some(socket), Some(own)) = (MM_SOCKET.get(), OWN_ID.get()) else { return };
    let mut relays: Vec<String> = [a, b].iter().filter_map(|name| remote_relay_of(name).map(|(relay, ..)| relay)).collect();
    relays.dedup();
    if relays.is_empty() {
        return;
    }
    let body = crate::link::Body::Introduce { a: a.into(), b: b.into() };
    for link in links().iter().filter(|l| relays.contains(&l.id)) {
        let message = crate::link::seal(&link.key, own, identity::now(), &body);
        let _ = socket.send_to(&message, std::net::SocketAddrV4::new(link.ip, link.port));
    }
}

/// A game packet another relay forwarded: from its player `from_name`, whose game is seen at
/// `from`, for `to` (an address on this helper).
pub struct Forwarded {
    pub from_name: String,
    pub from: std::net::SocketAddrV4,
    pub to: std::net::SocketAddrV4,
    pub payload: Vec<u8>,
}

/// How long an address stays with the relay its packets were forwarded from.
const LEARNED_FOR: std::time::Duration = std::time::Duration::from_secs(120);
const MAX_LEARNED: usize = 50_000;
type Learned = std::collections::HashMap<std::net::SocketAddrV4, (String, std::time::Instant)>;
static LEARNED: std::sync::LazyLock<Mutex<Learned>> = std::sync::LazyLock::new(Mutex::default);

fn learn(from: std::net::SocketAddrV4, link: &str) {
    let now = std::time::Instant::now();
    let mut learned = LEARNED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if learned.len() >= MAX_LEARNED && !learned.contains_key(&from) {
        learned.retain(|_, (_, at)| now.duration_since(*at) < LEARNED_FOR);
        if learned.len() >= MAX_LEARNED {
            return;
        }
    }
    learned.insert(from, (link.to_string(), now));
}

/// The relay to forward a packet for `to` to: the one whose range `to` is in (a player relayed
/// there), else the one packets from `to` lately came through (a player there reached directly).
pub fn forward_link(to: std::net::SocketAddrV4) -> Option<Link> {
    let links = links();
    if let Some(l) = links.iter().find(|l| l.ip == *to.ip() && (l.ports.0..=l.ports.1).contains(&to.port())) {
        return Some(l.clone());
    }
    let learned = LEARNED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (id, at) = learned.get(&to)?;
    if at.elapsed() >= LEARNED_FOR {
        return None;
    }
    links.iter().find(|l| &l.id == id).cloned()
}

/// Forwards a packet of this helper's player `from_name` (seen by the others at `from`) for `to`
/// to the relay of `link`, over the helper's own socket (the address the link lists).
pub fn send_forward(socket: &std::net::UdpSocket, link: &Link, from_name: &str, from: std::net::SocketAddrV4, to: std::net::SocketAddrV4, payload: &[u8]) -> bool {
    let Some(own) = OWN_ID.get() else { return false };
    let body = crate::link::Body::Forward {
        from_name: from_name.into(),
        from,
        to,
        payload: payload.to_vec(),
    };
    socket
        .send_to(&crate::link::seal(&link.key, own, identity::now(), &body), std::net::SocketAddrV4::new(link.ip, link.port))
        .is_ok()
}

// The matchmaking server's side.

/// A relay's question about a pair, waiting for the database.
struct Question {
    link: Link,
    nonce: u32,
    a: String,
    b: String,
    src: std::net::SocketAddrV4,
    socket: std::net::UdpSocket,
}

/// The most questions waiting; more are dropped (the relay asks again).
const QUESTIONS_QUEUED: usize = 256;
static ASKS: std::sync::OnceLock<std::sync::mpsc::SyncSender<Question>> = std::sync::OnceLock::new();

/// Answers relays' questions about pairs from `storage`, on a thread of its own (the network's
/// matchmaking server).
pub fn serve(storage: std::sync::Arc<crate::storage::Storage>) -> std::io::Result<std::thread::JoinHandle<()>> {
    let (queue, questions) = std::sync::mpsc::sync_channel::<Question>(QUESTIONS_QUEUED);
    let _ = ASKS.set(queue);
    std::thread::Builder::new().name("relay-pairs".into()).spawn(move || {
        while let Ok(q) = questions.recv() {
            // The way the packet goes: sender (a) to receiver (b).
            let yes = storage.may_relay(&q.a, &q.b).unwrap_or(false);
            let Some(own) = OWN_ID.get() else { continue };
            let answer = crate::link::seal(&q.link.key, own, identity::now(), &crate::link::Body::Answer { nonce: q.nonce, yes });
            let _ = q.socket.send_to(&answer, q.src);
        }
    })
}

/// A relay's registration, as the matchmaking server keeps it.
#[derive(Debug, Clone)]
struct Remote {
    relay: String,
    real: std::net::SocketAddrV4,
    advertise: std::net::SocketAddrV4,
    relayed: bool,
    rtt_ms: Option<u16>,
    at: std::time::Instant,
}

/// How long a reported registration counts without being reported again (relays report every
/// [`REPORT_EVERY`]).
const REMOTE_FRESH: std::time::Duration = std::time::Duration::from_secs(10);
/// The most registrations kept from relays.
const MAX_REMOTE: usize = 50_000;

type Remotes = std::collections::HashMap<String, Remote>;
static REMOTE: std::sync::LazyLock<Mutex<Remotes>> = std::sync::LazyLock::new(Mutex::default);

/// Whether a relay's report of `reg` can be so: a relayed player's address is on that relay, in
/// its range; a direct one's is where the relay saw them (their public address, or a port
/// their router mapped on it). Anything else would point other players' games elsewhere.
fn plausible(link: &Link, reg: &crate::link::Reg) -> bool {
    if reg.relayed {
        *reg.advertise.ip() == link.ip && (link.ports.0..=link.ports.1).contains(&reg.advertise.port())
    } else {
        reg.advertise.ip() == reg.real.ip()
    }
}

fn remote_report(link: &Link, regs: Vec<crate::link::Reg>) {
    let now = std::time::Instant::now();
    let mut remote = REMOTE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if remote.len() >= MAX_REMOTE {
        remote.retain(|_, r| now.duration_since(r.at) < REMOTE_FRESH);
    }
    for reg in regs.into_iter().filter(|r| plausible(link, r)) {
        if remote.len() >= MAX_REMOTE {
            break;
        }
        let key = identity::name_key(&reg.name);
        // Another relay's fresh entry stands until it lapses: one relay can't take over a
        // player another reports.
        if remote.get(&key).is_some_and(|r| r.relay != link.id && now.duration_since(r.at) < REMOTE_FRESH) {
            continue;
        }
        remote.insert(
            key,
            Remote {
                relay: link.id.clone(),
                real: reg.real,
                advertise: reg.advertise,
                relayed: reg.relayed,
                rtt_ms: reg.rtt_ms,
                at: now,
            },
        );
    }
}

/// The address a relay elsewhere checked for `name` (its public one, or the relay's), when a
/// relay reported them lately from where this server sees them connect from (`ip`): the same
/// address, or one in the same /24 (carrier-grade NAT can give a player different public
/// addresses towards two servers).
pub fn remote_advertised_for(name: &str, ip: std::net::IpAddr) -> Option<std::net::SocketAddrV4> {
    let std::net::IpAddr::V4(ip) = ip else { return None };
    let remote = REMOTE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let r = remote.get(&identity::name_key(name)).filter(|r| r.at.elapsed() < REMOTE_FRESH)?;
    let same_24 = r.real.ip().octets()[..3] == ip.octets()[..3];
    if !same_24 {
        // Carrier-grade NAT can show a player at two addresses: said once in a while, so the
        // admins see why the relay's address wasn't taken (no addresses in it).
        let relay = r.relay.clone();
        drop(remote);
        if mismatch_told(name) {
            crate::session_events::note(
                crate::session_events::Who::Name(name.to_string()),
                "relay_address_mismatch",
                serde_json::json!({ "relay": relay }),
            );
        }
        return None;
    }
    Some(r.advertise)
}

/// Whether a mismatch for `name` is to be said now (once in ten minutes each).
fn mismatch_told(name: &str) -> bool {
    type Told = std::collections::HashMap<String, std::time::Instant>;
    static TOLD: std::sync::LazyLock<Mutex<Told>> = std::sync::LazyLock::new(Mutex::default);
    let mut told = TOLD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = std::time::Instant::now();
    if told.len() >= 10_000 {
        told.retain(|_, at| now.duration_since(*at) < std::time::Duration::from_secs(600));
    }
    match told.get(&identity::name_key(name)) {
        Some(at) if now.duration_since(*at) < std::time::Duration::from_secs(600) => false,
        _ => {
            told.insert(identity::name_key(name), now);
            true
        }
    }
}

/// The relay elsewhere that reported `name` lately: its id, whether it relays them, and their
/// ping to it (the admins' views of who's online).
pub fn remote_relay_of(name: &str) -> Option<(String, bool, Option<u16>)> {
    let remote = REMOTE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    remote
        .get(&identity::name_key(name))
        .filter(|r| r.at.elapsed() < REMOTE_FRESH)
        .map(|r| (r.relay.clone(), r.relayed, r.rtt_ms))
}

/// The session events a relay sends the matchmaking server, and the fields each keeps.
const RELAY_EVENTS: [(&str, &[&str]); 5] = [
    ("nat", &["relayed"]),
    ("relay_drop", &["direction", "before", "after", "player_quiet"]),
    ("relay_gaps", &["gaps", "bursts", "longest_ms"]),
    ("nat_missing", &["after_secs"]),
    ("nat_lost", &["probe_secs"]),
];

/// A player's registration with a relay elsewhere, as last reported: the relay's id, where it
/// sees them, whether it relays them, and their ping to it.
pub fn remote_entry(name: &str) -> Option<(String, std::net::SocketAddrV4, bool, Option<u16>)> {
    let remote = REMOTE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    remote
        .get(&identity::name_key(name))
        .filter(|r| r.at.elapsed() < REMOTE_FRESH)
        .map(|r| (r.relay.clone(), r.real, r.relayed, r.rtt_ms))
}

// A relay's side.

static RELAY_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// This server is a relay alone: its session events go to the matchmaking server
/// ([`forward_event`]). Called once at start.
pub fn set_relay_only() {
    RELAY_ONLY.store(true, Ordering::Relaxed);
}

/// Sends a session event about `name` to the network's matchmaking server, when this is a
/// relay alone (which keeps none itself); false when it isn't (the caller keeps it).
pub fn forward_event(name: &str, kind: &str, detail: &serde_json::Value) -> bool {
    if !RELAY_ONLY.load(Ordering::Relaxed) {
        return false;
    }
    // Sent after the next report, which names the player: the matchmaking server takes an
    // event only about a player the relay reports.
    let mut queued = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if queued.len() < MAX_EVENTS_QUEUED {
        queued.push(crate::link::Body::Event {
            name: name.into(),
            kind: kind.into(),
            detail: detail.to_string(),
        });
    }
    true
}

/// The most events waiting for the next report; more are dropped.
const MAX_EVENTS_QUEUED: usize = 500;
static EVENTS: Mutex<Vec<crate::link::Body>> = Mutex::new(Vec::new());

/// How often a relay reports its registrations to the matchmaking server.
pub const REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
/// How long a relay waits for the matchmaking server's answer about a pair.
const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

static LINK_SOCKET: std::sync::OnceLock<std::net::UdpSocket> = std::sync::OnceLock::new();
/// Questions waiting, by nonce: the link asked, and its answer once it comes.
type Waiting = std::collections::HashMap<u32, (String, Option<bool>)>;
static WAITING: std::sync::LazyLock<(Mutex<Waiting>, std::sync::Condvar)> = std::sync::LazyLock::new(Default::default);

/// The answer to the question with `nonce`, taken only from the link it went to.
fn answered(nonce: u32, yes: bool, from: &str) {
    let (waiting, ready) = &*WAITING;
    let mut w = waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((asked, slot)) = w.get_mut(&nonce) {
        if asked == from {
            *slot = Some(yes);
            ready.notify_all();
        }
    }
}

/// Starts a relay's link: its registrations to the network's matchmaking server every
/// [`REPORT_EVERY`], and the answers to its questions about pairs ([`ask_matchmaking`]).
pub fn start_link(logger: slog::Logger) -> std::io::Result<std::thread::JoinHandle<()>> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(std::time::Duration::from_millis(250)))?;
    let _ = LINK_SOCKET.set(socket.try_clone()?);
    std::thread::Builder::new().name("relay-link".into()).spawn(move || {
        let mut buf = [0u8; 1500];
        let mut last_report: Option<std::time::Instant> = None;
        let mut told_none = false;
        loop {
            if last_report.is_none_or(|t| t.elapsed() >= REPORT_EVERY) {
                last_report = Some(std::time::Instant::now());
                let targets: Vec<Link> = links().iter().filter(|l| l.matchmaking).cloned().collect();
                if targets.is_empty() && !told_none {
                    slog::warn!(logger, "Relay link: the coordinator lists no matchmaking server yet; nothing reported");
                    told_none = true;
                }
                if let Some(own) = OWN_ID.get() {
                    let regs = crate::nat_helper::registrations();
                    for link in &targets {
                        told_none = false;
                        let to = std::net::SocketAddrV4::new(link.ip, link.port);
                        for chunk in regs.chunks(crate::link::MAX_REGS).chain(regs.is_empty().then_some(&[][..])) {
                            let report = crate::link::seal(&link.key, own, identity::now(), &crate::link::Body::Report(chunk.to_vec()));
                            let _ = socket.send_to(&report, to);
                        }
                        // Its players' events, now that the report naming them went.
                        let events = std::mem::take(&mut *EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
                        for event in &events {
                            let _ = socket.send_to(&crate::link::seal(&link.key, own, identity::now(), event), to);
                        }
                    }
                }
            }
            if let Ok((len, std::net::SocketAddr::V4(src))) = socket.recv_from(&mut buf) {
                // Forwards come to the helper's port, never here.
                let _ = on_link(&buf[..len], src, &socket);
            }
        }
    })
}

/// Asks the network's matchmaking server whether it put `a` and `b` together, waiting up to
/// [`ASK_TIMEOUT`] (a relay's pair check runs beside its packets, never in their way).
pub fn ask_matchmaking(a: &str, b: &str) -> Result<(), String> {
    let link = links()
        .iter()
        .find(|l| l.matchmaking)
        .cloned()
        .ok_or("no matchmaking server to ask (the coordinator lists none)")?;
    let (Some(socket), Some(own)) = (LINK_SOCKET.get(), OWN_ID.get()) else {
        return Err("the relay's link isn't running".into());
    };
    let nonce = rand::random::<u32>() | 1;
    let (waiting, ready) = &*WAITING;
    waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(nonce, (link.id.clone(), None));
    let ask = crate::link::seal(&link.key, own, identity::now(), &crate::link::Body::Ask { nonce, a: a.into(), b: b.into() });
    let _ = socket.send_to(&ask, std::net::SocketAddrV4::new(link.ip, link.port));
    let deadline = std::time::Instant::now() + ASK_TIMEOUT;
    let mut w = waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let answer = loop {
        if let Some((_, Some(yes))) = w.get(&nonce) {
            break Some(*yes);
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break None;
        }
        w = ready.wait_timeout(w, left).unwrap_or_else(std::sync::PoisonError::into_inner).0;
    };
    w.remove(&nonce);
    match answer {
        Some(true) => Ok(()),
        Some(false) => Err("the matchmaking server didn't put them together".into()),
        None => Err("the matchmaking server didn't answer in time".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relays_report_is_kept_only_where_it_can_be_so() {
        let link = |id: &str, ip: [u8; 4]| Link {
            id: id.into(),
            ip: ip.into(),
            port: 21128,
            ports: (40000, 40999),
            key: [0; 32],
            matchmaking: false,
        };
        let (syd, eu) = (link("syd", [5, 6, 7, 8]), link("eu", [9, 9, 9, 9]));
        let reg = |name: &str, advertise: &str, relayed: bool| crate::link::Reg {
            name: name.into(),
            real: "203.0.113.9:13000".parse().unwrap(),
            advertise: advertise.parse().unwrap(),
            relayed,
            rtt_ms: None,
        };
        assert!(plausible(&syd, &reg("a", "5.6.7.8:40012", true)));
        assert!(!plausible(&syd, &reg("a", "5.6.7.8:22", true)), "outside its range");
        assert!(!plausible(&syd, &reg("a", "9.9.9.9:40012", true)), "another relay's address");
        assert!(plausible(&syd, &reg("a", "203.0.113.9:3074", false)), "a direct player's mapped port");
        assert!(!plausible(&syd, &reg("a", "198.51.100.1:13000", false)), "somewhere the player isn't");
        let seen: std::net::IpAddr = "203.0.113.9".parse().unwrap();
        remote_report(&syd, vec![reg("Taken", "5.6.7.8:40012", true)]);
        remote_report(&eu, vec![reg("Taken", "9.9.9.9:40001", true)]);
        assert_eq!(remote_advertised_for("Taken", seen), Some("5.6.7.8:40012".parse().unwrap()), "the first relay's stands");
        remote_report(&syd, vec![reg("Elsewhere", "198.51.100.1:13000", false)]);
        assert_eq!(remote_advertised_for("Elsewhere", seen), None, "an implausible report isn't kept");
    }

    #[test]
    fn the_networks_relays_come_from_the_heartbeat() {
        from_heartbeat(&serde_json::json!({ "relays": [
            { "id": "syd", "name": "Sydney", "region": "Oceania", "host": "syd.example.net", "role": "relay", "ip": "5.6.7.8", "port": 21128, "ports": [40000, 40999] },
            { "id": "lan", "ip": "192.168.1.20", "port": 21128, "ports": [40000, 40999] },
            { "id": "upside-down", "ip": "9.9.9.9", "port": 21128, "ports": [40999, 40000] },
        ] }));
        let kept: Vec<String> = network().iter().map(|r| r.id.clone()).collect();
        assert_eq!(kept, ["syd"]);
        // An answer without relays keeps the last list.
        from_heartbeat(&serde_json::json!({}));
        assert_eq!(network().len(), 1);
        // Offered, and their addresses worked with, only on the matchmaking server.
        init(false);
        let syd = (Ipv4Addr::new(5, 6, 7, 8), (40000, 40999));
        assert!(offered().is_empty() && !ranges().contains(&syd));
        init(true);
        assert_eq!(offered().len(), 1);
        assert!(ranges().contains(&syd));
        init(false);
    }
}
