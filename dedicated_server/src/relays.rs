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

/// The network's relays from the coordinator's heartbeat answer (coordinators from 0.5.0 send
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

/// The header a relay's Caddy adds when it passes a player's API call on to the matchmaking
/// server (0.5.5's SR): the player's address, as the relay saw it.
pub const RELAYED_FOR: &str = "x-5e-relayed-for";

/// The client behind an API call that came through one of the network's relays (SR): the
/// address in [`RELAYED_FOR`] when the call came from a relay of the network (`from`, the client
/// as the proxies in front of this server name it), else `from`. On the matchmaking server only.
pub fn relayed_client(from: Option<std::net::IpAddr>, relayed_for: Option<&str>) -> Option<std::net::IpAddr> {
    if !NETWORK_RELAYS.load(Ordering::Relaxed) {
        return from;
    }
    // The relays the directory offers (public addresses only), and the members this server holds
    // link keys for (the relays it already takes reports and questions from, by address), which
    // a network on private addresses has only.
    let mut relays: Vec<std::net::IpAddr> = network().iter().map(|r| std::net::IpAddr::V4(r.ip)).collect();
    relays.extend(links().iter().filter(|l| !l.matchmaking).map(|l| std::net::IpAddr::V4(l.ip)));
    via_relay(from, relayed_for, &relays)
}

fn via_relay(from: Option<std::net::IpAddr>, relayed_for: Option<&str>, relays: &[std::net::IpAddr]) -> Option<std::net::IpAddr> {
    let from = from?;
    if !relays.contains(&from.to_canonical()) {
        return Some(from);
    }
    // The relay's Caddy sets the header (replacing any the player sent): one address.
    Some(relayed_for.and_then(|v| v.trim().parse::<std::net::IpAddr>().ok()).unwrap_or(from))
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
    // Anything from the matchmaking server, signed: it's up and reaches this relay.
    if link.matchmaking {
        MM_SEEN.store(identity::now(), Ordering::Relaxed);
    }
    match body {
        // Another relay's player's packet for one of this helper's: the address it came from is
        // that relay's from now on (a reply to it goes back there).
        crate::link::Body::Forward { from_name, from, to, payload } => {
            // Only for its own players: one relayed there is on its address in its range, one
            // direct isn't on another relay's address; and the matchmaking server, which has
            // every relay's reports, knows which relay each player is at and their address.
            let others: Vec<Ipv4Addr> = links().iter().filter(|l| l.id != link.id).map(|l| l.ip).collect();
            let reported = network_relays().then(|| reported_for(&from_name));
            if !forward_plausible(&link, from, &others, reported.as_ref()) {
                return None;
            }
            learn(from, &link.id);
            return Some(Forwarded { from_name, from, to, payload });
        }
        // Only the network's matchmaking server keeps relays' reports; it answers each (nonce 0,
        // no question's), so the relay knows it's heard (`matchmaking_seen`).
        crate::link::Body::Report(regs) if NETWORK_RELAYS.load(Ordering::Relaxed) => {
            remote_report(&link, regs);
            if let Some(own) = OWN_ID.get() {
                let heard = crate::link::seal(&link.key, own, identity::now(), &crate::link::Body::Answer { nonce: 0, yes: true });
                let _ = socket.send_to(&heard, src);
            }
        }
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
        // Only from the matchmaking server: a player of ours whose joins kept failing (J2).
        crate::link::Body::Relay { name } if link.matchmaking => crate::nat_helper::force_relay(&name, "failed_joins"),
        crate::link::Body::Relay { .. } => {}
        // Another member's reach check for its player (J1), sent from here: rate-limited, so a
        // member can't use it to send traffic anywhere at will.
        // Only from the matchmaking server: a direct player at another member, put together with
        // one of ours: their address is at that member (forward_link), not "nobody's".
        crate::link::Body::Locate { name: _, at, relay } if link.matchmaking => {
            let others: Vec<Ipv4Addr> = links().iter().map(|l| l.ip).collect();
            let known = links().iter().any(|l| l.id == relay);
            if known && OWN_ID.get().is_some_and(|own| *own != relay) && !others.contains(at.ip()) {
                learn(at, &relay);
            }
        }
        crate::link::Body::Locate { .. } => {}
        // A relay's tunnelled player signing in through it (SR): only on the network's
        // matchmaking server, only from a relay of the network, and only for that relay's own
        // address (where the datagrams come from).
        crate::link::Body::Tunnelled { name, port, ip, open } if NETWORK_RELAYS.load(Ordering::Relaxed) && !link.matchmaking => {
            forwarded_note(std::net::SocketAddr::new(link.ip.into(), port), ip, &name, open, std::time::Instant::now());
            // The player's Sessions row says they signed in through this relay (0.5.5's SR). Noted
            // here from the relay's own word: an event it sent would come before it reports the
            // player (the tunnel opens before the game registers), and be dropped.
            crate::session_events::note(
                crate::session_events::Who::Name(name),
                "tunnel",
                serde_json::json!({ "open": open, "relay": true, "at": link.id }),
            );
        }
        crate::link::Body::Tunnelled { .. } => {}
        crate::link::Body::Reach { to, nonce } => {
            if reach_allowed(to) {
                crate::nat_helper::send_reach_for_another(socket, to, nonce);
            }
        }
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

/// When this relay last had a signed message from the network's matchmaking server (Unix
/// seconds; 0: never).
static MM_SEEN: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// How long ago this relay last heard from the matchmaking server, in seconds (None: never, or
/// it's no relay of one).
pub fn matchmaking_seen_ago() -> Option<u64> {
    let at = MM_SEEN.load(Ordering::Relaxed);
    (at > 0).then(|| u64::try_from(identity::now() - at).unwrap_or(0))
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
    // Where a direct one of them is, for the other's member (Body::Locate).
    let place = |name: &str| -> Option<Place> {
        let remote = REMOTE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(r) = remote.get(&identity::name_key(name)).filter(|r| r.at.elapsed() < REMOTE_FRESH) {
            return Some(Place {
                relay: r.relay.clone(),
                advertise: r.advertise,
                relayed: r.relayed,
            });
        }
        drop(remote);
        crate::nat_helper::local_place(name).map(|(advertise, relayed)| Place {
            relay: own.clone(),
            advertise,
            relayed,
        })
    };
    for locate in locates((a, place(a)), (b, place(b))) {
        if locate.tell == *own {
            learn(locate.at, &locate.relay);
        } else if let Some(link) = links().iter().find(|l| l.id == locate.tell) {
            let body = crate::link::Body::Locate {
                name: locate.name,
                at: locate.at,
                relay: locate.relay,
            };
            let _ = socket.send_to(&crate::link::seal(&link.key, own, identity::now(), &body), std::net::SocketAddrV4::new(link.ip, link.port));
        }
    }
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

/// A relay's tunnelled players signing in through it (SR), on the matchmaking server: by the
/// relay's address and port their datagrams come from, their real address, their name and when
/// the relay last said so.
type SignIns = std::collections::HashMap<std::net::SocketAddr, (std::net::IpAddr, String, std::time::Instant)>;
static FORWARDED: std::sync::LazyLock<Mutex<SignIns>> = std::sync::LazyLock::new(Mutex::default);
/// How long a relay's word on a tunnel holds without being said again (every 20 s).
const FORWARDED_FOR: std::time::Duration = std::time::Duration::from_secs(70);
const MAX_FORWARDED: usize = 50_000;

fn forwarded_note(from: std::net::SocketAddr, ip: std::net::IpAddr, name: &str, open: bool, now: std::time::Instant) {
    let mut f = FORWARDED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if !open {
        f.remove(&from);
        return;
    }
    if f.len() >= MAX_FORWARDED && !f.contains_key(&from) {
        f.retain(|_, (.., at)| now.saturating_duration_since(*at) < FORWARDED_FOR);
        if f.len() >= MAX_FORWARDED {
            return;
        }
    }
    f.insert(from, (ip, name.to_string(), now));
}

/// The real address of the player whose sign-in a relay forwards from `src` (SR), if one does.
pub fn forwarded_ip(src: std::net::SocketAddr) -> Option<std::net::IpAddr> {
    forwarded_ip_at(src, std::time::Instant::now())
}

fn forwarded_ip_at(src: std::net::SocketAddr, now: std::time::Instant) -> Option<std::net::IpAddr> {
    let f = FORWARDED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f.get(&src).filter(|(.., at)| now.saturating_duration_since(*at) < FORWARDED_FOR).map(|(ip, ..)| *ip)
}

/// The relay whose tunnel a game's connection from `src` came through (SR), by its name when
/// the directory gives one (else its address): where that game reaches the matchmaking
/// server's content and uploads, which the relay's Caddy passes on.
pub fn relay_name_for(src: std::net::SocketAddr) -> Option<String> {
    forwarded_ip(src)?;
    let std::net::IpAddr::V4(ip) = src.ip() else { return Some(src.ip().to_string()) };
    let named = network().iter().find(|r| r.ip == ip && !r.host.is_empty()).map(|r| r.host.clone());
    Some(named.unwrap_or_else(|| ip.to_string()))
}

/// On a relay: the network's matchmaking server's address, where its tunnelled players' sign-in
/// goes (SR).
pub fn matchmaking_ip() -> Option<Ipv4Addr> {
    links().iter().find(|l| l.matchmaking).map(|l| l.ip)
}

/// On a relay: tells the network's matchmaking server that its tunnelled player `name` (at `ip`)
/// signs in through this relay's port `port`, or no longer does.
pub fn tunnelled(name: &str, port: u16, ip: std::net::IpAddr, open: bool) {
    let (Some(socket), Some(own)) = (LINK_SOCKET.get(), OWN_ID.get()) else { return };
    let body = crate::link::Body::Tunnelled {
        name: name.into(),
        port,
        ip,
        open,
    };
    for link in links().iter().filter(|l| l.matchmaking) {
        let _ = socket.send_to(&crate::link::seal(&link.key, own, identity::now(), &body), std::net::SocketAddrV4::new(link.ip, link.port));
    }
}

/// Where a player is registered: at which member, what their game advertises, whether relayed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    relay: String,
    advertise: std::net::SocketAddrV4,
    relayed: bool,
}

/// A [`crate::link::Body::Locate`] to send: tell member `tell` that `name` is at `at`, at member
/// `relay`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Locate {
    tell: String,
    name: String,
    at: std::net::SocketAddrV4,
    relay: String,
}

/// For two players put together: each direct one's address, for the other's member when that's
/// another member. A relayed player's address is on their relay's range, which every member
/// forwards to already; two at one member need nothing.
fn locates(a: (&str, Option<Place>), b: (&str, Option<Place>)) -> Vec<Locate> {
    let mut out = Vec::new();
    for ((name, place), (_, other)) in [(&a, &b), (&b, &a)] {
        let (Some(p), Some(o)) = (place, other) else { continue };
        if !p.relayed && p.relay != o.relay {
            out.push(Locate {
                tell: o.relay.clone(),
                name: (*name).to_string(),
                at: p.advertise,
                relay: p.relay.clone(),
            });
        }
    }
    out
}

/// On the network's matchmaking server: `name` is to be relayed from now on (J2); the relay
/// they're registered at is told ([`crate::link::Body::Relay`]). False when they're at none.
pub fn relay_remote(name: &str) -> bool {
    if !NETWORK_RELAYS.load(Ordering::Relaxed) {
        return false;
    }
    let (Some(socket), Some(own)) = (MM_SOCKET.get(), OWN_ID.get()) else { return false };
    let Some((relay, ..)) = remote_relay_of(name) else { return false };
    let body = crate::link::Body::Relay { name: name.into() };
    for link in links().iter().filter(|l| l.id == relay) {
        let message = crate::link::seal(&link.key, own, identity::now(), &body);
        let _ = socket.send_to(&message, std::net::SocketAddrV4::new(link.ip, link.port));
    }
    true
}

/// Asks another member of the network to send a reach check to `to` (J1): false when there's
/// none to ask (a lone server).
pub fn reach_via_another(to: std::net::SocketAddrV4, nonce: u32) -> bool {
    let Some(own) = OWN_ID.get() else { return false };
    let Some(socket) = MM_SOCKET.get().or_else(|| LINK_SOCKET.get()) else { return false };
    let all = links();
    let Some(link) = all.iter().find(|l| l.id != *own) else { return false };
    let message = crate::link::seal(&link.key, own, identity::now(), &crate::link::Body::Reach { to, nonce });
    socket.send_to(&message, std::net::SocketAddrV4::new(link.ip, link.port)).is_ok()
}

/// Reach checks sent for other members: one a second to any address at most, and a few
/// hundred a minute in all.
fn reach_allowed(to: std::net::SocketAddrV4) -> bool {
    static SENT: std::sync::LazyLock<Mutex<(std::time::Instant, u32, std::collections::HashMap<std::net::SocketAddrV4, std::time::Instant>)>> =
        std::sync::LazyLock::new(|| Mutex::new((std::time::Instant::now(), 0, std::collections::HashMap::new())));
    let now = std::time::Instant::now();
    let mut s = SENT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if now.duration_since(s.0) >= std::time::Duration::from_secs(60) {
        *s = (now, 0, std::collections::HashMap::new());
    }
    if s.1 >= 300 || s.2.get(&to).is_some_and(|at| now.duration_since(*at) < std::time::Duration::from_secs(1)) {
        return false;
    }
    s.1 += 1;
    s.2.insert(to, now);
    true
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

/// Whether link `link` may forward a packet of its player seen at `from`: relayed there, an
/// address on it in its range; direct, not on another relay's address (`others`). On the
/// matchmaking server, `reported`: what the relays last reported of the player
/// ([`reported_for`]), which must be this relay's report at `from`.
fn forward_plausible(link: &Link, from: std::net::SocketAddrV4, others: &[Ipv4Addr], reported: Option<&Option<(String, std::net::SocketAddrV4)>>) -> bool {
    let on_it = if *from.ip() == link.ip {
        (link.ports.0..=link.ports.1).contains(&from.port())
    } else {
        !others.contains(from.ip())
    };
    on_it && reported.is_none_or(|r| r.as_ref().is_some_and(|(relay, advertise)| *relay == link.id && *advertise == from))
}

/// The relay that reported `name` lately (within [`LEARNED_FOR`]: a report or two late still
/// counts), and the address it gave them.
fn reported_for(name: &str) -> Option<(String, std::net::SocketAddrV4)> {
    let remote = REMOTE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    remote
        .get(&identity::name_key(name))
        .filter(|r| r.at.elapsed() < LEARNED_FOR)
        .map(|r| (r.relay.clone(), r.advertise))
}

/// Notes that packets from `from` come through `link`, unless another relay's did lately (an
/// address stays with the relay that first forwarded from it while that goes on).
fn learn(from: std::net::SocketAddrV4, link: &str) {
    let now = std::time::Instant::now();
    let mut learned = LEARNED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if learned.get(&from).is_some_and(|(other, at)| other != link && now.duration_since(*at) < LEARNED_FOR) {
        return;
    }
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
            let yes = storage.may_relay(&q.a, &q.b).unwrap_or(false) || storage.follows_into_private(&q.a, &q.b).unwrap_or(false);
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
    let mut moved = Vec::new();
    // Who it reports, for moves that last (JC2).
    let mut reported: Vec<(String, std::net::Ipv4Addr, bool)> = Vec::new();
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
        // Moved by its relay after signing in here (J1, J2): the URLs it registered here follow.
        if remote
            .get(&key)
            .is_some_and(|r| r.relay == link.id && r.advertise != reg.advertise && now.duration_since(r.at) < REMOTE_FRESH)
        {
            moved.push((reg.name.clone(), reg.advertise, std::net::IpAddr::V4(*reg.real.ip())));
        }
        reported.push((reg.name.clone(), *reg.real.ip(), reg.relayed));
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
    drop(remote);
    // Direct there, though moved to the relay lately on that network (JC2): their relay is told.
    for name in crate::relay_moves::remote_direct(reported.iter().map(|(name, ip, relayed)| (name.as_str(), *ip, *relayed))) {
        relay_remote(&name);
    }
    if !moved.is_empty() {
        let _ = std::thread::Builder::new().name("relay-readdress".into()).spawn(move || {
            for (name, advertise, real) in moved {
                crate::nat_helper::readdress_quietly(&name, advertise, real);
            }
        });
    }
}

/// The player (name key) another relay reported lately as advertising `address` (JC4).
pub fn remote_advertising(address: std::net::SocketAddrV4) -> Option<String> {
    let remote = REMOTE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    remote
        .iter()
        .find(|(_, r)| r.advertise == address && r.at.elapsed() < REMOTE_FRESH)
        .map(|(key, _)| key.clone())
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
    ("nat", &["relayed", "why"]),
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
    /// A game signed in through a relay's tunnel gets its content and uploads at that relay.
    #[test]
    fn a_relayed_sign_in_is_given_its_relay() {
        let src: std::net::SocketAddr = "192.0.2.77:40511".parse().unwrap();
        assert_eq!(super::relay_name_for(src), None, "not through a relay");
        super::forwarded_note(src, "198.51.100.9".parse().unwrap(), "Faraway", true, std::time::Instant::now());
        assert_eq!(super::relay_name_for(src).as_deref(), Some("192.0.2.77"), "no name in the directory: its address");
        assert_eq!(super::relay_name_for("192.0.2.77:40512".parse().unwrap()), None, "another of its sockets: someone else's");
    }

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
    fn a_relay_forwards_only_for_its_own_players() {
        let syd = Link {
            id: "syd".into(),
            ip: [5, 6, 7, 8].into(),
            port: 21128,
            ports: (40000, 40999),
            key: [0; 32],
            matchmaking: false,
        };
        let others: Vec<Ipv4Addr> = vec![[9, 9, 9, 9].into()];
        let at = |s: &str| -> std::net::SocketAddrV4 { s.parse().unwrap() };
        assert!(forward_plausible(&syd, at("5.6.7.8:40012"), &others, None), "relayed there");
        assert!(!forward_plausible(&syd, at("5.6.7.8:22"), &others, None), "its address, outside its range");
        assert!(forward_plausible(&syd, at("203.0.113.9:3074"), &others, None), "a direct player there");
        assert!(!forward_plausible(&syd, at("9.9.9.9:40012"), &others, None), "another relay's player");
        // The matchmaking server knows where each player is.
        let theirs = Some(("syd".to_string(), at("5.6.7.8:40012")));
        assert!(forward_plausible(&syd, at("5.6.7.8:40012"), &others, Some(&theirs)));
        assert!(!forward_plausible(&syd, at("5.6.7.8:40013"), &others, Some(&theirs)), "not where it reported them");
        assert!(
            !forward_plausible(&syd, at("5.6.7.8:40012"), &others, Some(&Some(("eu".to_string(), at("5.6.7.8:40012"))))),
            "another relay's player"
        );
        assert!(!forward_plausible(&syd, at("5.6.7.8:40012"), &others, Some(&None)), "reported by nobody");
    }

    #[test]
    fn an_address_stays_with_the_relay_that_forwarded_from_it() {
        let from: std::net::SocketAddrV4 = "198.51.100.77:3074".parse().unwrap();
        learn(from, "syd");
        learn(from, "eu");
        assert_eq!(LEARNED.lock().unwrap().get(&from).map(|(l, _)| l.clone()).as_deref(), Some("syd"));
        learn(from, "syd");
        assert_eq!(LEARNED.lock().unwrap().get(&from).map(|(l, _)| l.clone()).as_deref(), Some("syd"), "its own again");
    }

    #[test]
    fn an_api_call_through_a_relay_counts_for_the_player_behind_it() {
        let relay: std::net::IpAddr = "198.51.100.33".parse().unwrap();
        let player: std::net::IpAddr = "203.0.113.40".parse().unwrap();
        let other: std::net::IpAddr = "192.0.2.9".parse().unwrap();
        assert_eq!(via_relay(Some(relay), Some("203.0.113.40"), &[relay]), Some(player));
        assert_eq!(via_relay(Some(other), Some("203.0.113.40"), &[relay]), Some(other), "only a relay's word counts");
        assert_eq!(via_relay(Some(relay), None, &[relay]), Some(relay), "the relay's own call");
        assert_eq!(via_relay(Some(relay), Some("not an address"), &[relay]), Some(relay));
        assert_eq!(via_relay(None, Some("203.0.113.40"), &[relay]), None);
    }

    #[test]
    fn a_relays_tunnelled_players_are_known_by_its_port_for_a_while() {
        let relay: std::net::SocketAddr = "198.51.100.33:52001".parse().unwrap();
        let player: std::net::IpAddr = "203.0.113.40".parse().unwrap();
        let t = std::time::Instant::now();
        assert_eq!(forwarded_ip_at(relay, t), None);
        forwarded_note(relay, player, "Ghost", true, t);
        assert_eq!(forwarded_ip_at(relay, t + std::time::Duration::from_secs(30)), Some(player));
        assert_eq!(forwarded_ip_at("198.51.100.33:52002".parse().unwrap(), t), None, "only that port");
        assert_eq!(forwarded_ip_at(relay, t + FORWARDED_FOR), None, "not said again: lapsed");
        forwarded_note(relay, player, "Ghost", true, t);
        forwarded_note(relay, player, "Ghost", false, t);
        assert_eq!(forwarded_ip_at(relay, t), None, "closed");
    }

    #[test]
    fn a_direct_player_is_located_for_the_other_players_member() {
        let place = |relay: &str, at: &str, relayed: bool| {
            Some(Place {
                relay: relay.into(),
                advertise: at.parse().unwrap(),
                relayed,
            })
        };
        // Tea_bag relayed at de1 joining John_Fisher direct at us1 (ca1, 16 Nov): de1 is told
        // where John_Fisher is; us1 needs nothing (Tea_bag's address is de1's).
        let host = place("us1", "203.0.113.51:60063", false);
        let joiner = place("de1", "198.51.100.33:40000", true);
        assert_eq!(
            locates(("Tea_bag", joiner.clone()), ("John_Fisher", host.clone())),
            vec![Locate {
                tell: "de1".into(),
                name: "John_Fisher".into(),
                at: "203.0.113.51:60063".parse().unwrap(),
                relay: "us1".into(),
            }]
        );
        // Both direct at two members: each is told of the other.
        let direct_joiner = place("de1", "192.0.2.7:13000", false);
        assert_eq!(locates(("a", direct_joiner), ("b", host.clone())).len(), 2);
        // At one member, or relayed, or not registered: nothing.
        assert!(locates(("a", place("us1", "192.0.2.7:13000", false)), ("b", host.clone())).is_empty());
        assert!(locates(("a", joiner.clone()), ("b", place("us1", "198.51.100.9:40001", true))).is_empty());
        assert!(locates(("a", None), ("b", host)).is_empty());
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
