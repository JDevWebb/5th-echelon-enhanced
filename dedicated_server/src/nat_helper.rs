//! The NAT helper: lets players join each other over the internet without a
//! VPN (protocol in the `nat_proto` crate).
//!
//! Matches run peer to peer on the game's Storm socket (UDP 13000), and the
//! game advertises its local address, which only works on a LAN. The hook in
//! the game probes this helper from that socket, learns its public address,
//! and has the game advertise it instead; the game's own NAT probing
//! (forwarded by `nat_traversal.rs`) then punches through.
//!
//! Players whose NAT can't be punched through get a relay address on this
//! server instead: the hook wraps their packets for it, this helper forwards
//! them, and the other side's hook unwraps them, so each game sees every
//! player at the address that player advertised.
//!
//! If the game doesn't adopt the address (see `urls_with_public_address`),
//! the station URLs it registers are corrected here as a fallback.

use std::collections::HashMap;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use dedicated_server_config::NatConfig;
use dedicated_server_config::RelayMode;
use nat_proto::probe_flags;
use nat_proto::reply_flags;
use nat_proto::Message;
use slog::Logger;

/// A player is forgotten after this long without a probe or relayed packet
/// (the hook probes every 20 s).
const EXPIRY: Duration = Duration::from_secs(90);

#[derive(Debug, Clone)]
struct Peer {
    name: String,
    /// The Storm socket's public address, as this helper sees it.
    real: SocketAddrV4,
    /// What the player's game advertises to the others.
    advertise: SocketAddrV4,
    relayed: bool,
    vport: Option<u16>,
    last_seen: Instant,
    window_start: Instant,
    window_bytes: usize,
}

/// A registered player, by number (so relaying a packet allocates nothing).
type Id = u64;

/// Who is registered, and how to reach them. No sockets, so it can be tested.
#[derive(Debug)]
pub struct Table {
    cfg: NatConfig,
    relay_ip: Ipv4Addr,
    next_id: Id,
    peers: HashMap<Id, Peer>,
    by_name: HashMap<String, Id>,
    by_real: HashMap<SocketAddrV4, Id>,
    by_vport: HashMap<u16, Id>,
    by_advertise: HashMap<SocketAddrV4, Id>,
}

impl Table {
    pub fn new(cfg: NatConfig, relay_ip: Ipv4Addr) -> Self {
        Self {
            cfg,
            relay_ip,
            next_id: 0,
            peers: HashMap::new(),
            by_name: HashMap::new(),
            by_real: HashMap::new(),
            by_vport: HashMap::new(),
            by_advertise: HashMap::new(),
        }
    }

    fn wants_relay(&self, observed: SocketAddrV4, flags: u8, mapping: Option<SocketAddrV4>) -> bool {
        let public_mapping = mapping.is_some_and(|m| !nat_proto::is_private(*m.ip()));
        match self.cfg.relay {
            RelayMode::Off => false,
            RelayMode::All => true,
            RelayMode::Auto => {
                // A router port mapping makes the player reachable as it is.
                if public_mapping {
                    return false;
                }
                flags & (probe_flags::WANT_RELAY | probe_flags::SYMMETRIC) != 0
                    // On this server's own network: players from outside
                    // can't reach the address this helper sees.
                    || (nat_proto::is_private(*observed.ip()) && !nat_proto::is_private(self.relay_ip))
            }
        }
    }

    fn free_vport(&self) -> Option<u16> {
        let (first, last) = self.cfg.relay_ports;
        (first..=last).find(|p| !self.by_vport.contains_key(p))
    }

    /// Takes a player out of the table, indexes and all.
    fn take(&mut self, id: Id) -> Option<Peer> {
        let p = self.peers.remove(&id)?;
        self.by_name.remove(&p.name);
        if self.by_real.get(&p.real) == Some(&id) {
            self.by_real.remove(&p.real);
        }
        if self.by_advertise.get(&p.advertise) == Some(&id) {
            self.by_advertise.remove(&p.advertise);
        }
        if let Some(v) = p.vport {
            self.by_vport.remove(&v);
        }
        Some(p)
    }

    /// Registers (or refreshes) the player behind `src` and answers its probe.
    pub fn probe(&mut self, src: SocketAddrV4, flags: u8, nonce: u32, mapping: Option<SocketAddrV4>, name: &str, now: Instant) -> Message {
        let key = if name.is_empty() { format!("@{src}") } else { name.to_lowercase() };
        let old = self.by_name.get(&key).copied().and_then(|id| self.take(id).map(|p| (id, p)));
        // Someone else registered from this address before (a restarted game
        // with another account): that registration is stale.
        if let Some(other) = self.by_real.get(&src).copied() {
            self.take(other);
        }
        // The game already told other players the address it got, so a
        // registration keeps it: a relayed player stays relayed (on the same
        // relay port), and a direct one keeps its address unless its NAT
        // gave it a new one. A direct player can still move to the relay
        // (the hook asks before it tells the game anything).
        let relayed = old.as_ref().is_some_and(|(_, o)| o.relayed) || self.wants_relay(src, flags, mapping);
        let vport = if relayed { old.as_ref().and_then(|(_, o)| o.vport).or_else(|| self.free_vport()) } else { None };
        let relayed = relayed && vport.is_some();
        let advertise = match (vport, mapping, &old) {
            (Some(v), _, _) => SocketAddrV4::new(self.relay_ip, v),
            (None, _, Some((_, o))) if !o.relayed && o.real == src => o.advertise,
            (None, Some(m), _) if !nat_proto::is_private(*m.ip()) => m,
            _ => src,
        };
        let id = old.as_ref().map_or_else(
            || {
                self.next_id += 1;
                self.next_id
            },
            |(id, _)| *id,
        );
        let peer = Peer {
            name: key.clone(),
            real: src,
            advertise,
            relayed,
            vport,
            last_seen: now,
            window_start: old.as_ref().map_or(now, |(_, o)| o.window_start),
            window_bytes: old.as_ref().map_or(0, |(_, o)| o.window_bytes),
        };
        self.by_name.insert(key, id);
        self.by_real.insert(src, id);
        self.by_advertise.insert(advertise, id);
        if let Some(v) = vport {
            self.by_vport.insert(v, id);
        }
        self.peers.insert(id, peer);
        Message::ProbeReply {
            nonce,
            observed: src,
            advertise,
            flags: if relayed { reply_flags::RELAYED } else { 0 },
            relay_ip: self.relay_ip,
            relay_ports: self.cfg.relay_ports,
        }
    }

    /// Where a relayed packet from `src` to `to` goes: the receiver's real
    /// address, and the sender address its game should see. `None` drops it
    /// (unknown sender or receiver, or the sender is over its rate).
    pub fn route(&mut self, src: SocketAddrV4, to: SocketAddrV4, len: usize, now: Instant) -> Option<(SocketAddrV4, SocketAddrV4)> {
        let sender_id = *self.by_real.get(&src)?;
        let (first, last) = self.cfg.relay_ports;
        let target_id = *if *to.ip() == self.relay_ip && (first..=last).contains(&to.port()) {
            self.by_vport.get(&to.port())
        } else {
            self.by_advertise.get(&to).or_else(|| self.by_real.get(&to))
        }?;
        if target_id == sender_id {
            return None;
        }
        let target = self.peers.get(&target_id)?.real;
        let limit = self.cfg.relay_kbps_per_player as usize * 1024;
        let sender = self.peers.get_mut(&sender_id)?;
        sender.last_seen = now;
        if now.duration_since(sender.window_start) >= Duration::from_secs(1) {
            sender.window_start = now;
            sender.window_bytes = 0;
        }
        sender.window_bytes += len;
        if sender.window_bytes > limit {
            return None;
        }
        Some((target, sender.advertise))
    }

    /// Forgets players not heard from in [`EXPIRY`].
    pub fn expire(&mut self, now: Instant) -> Vec<String> {
        let stale: Vec<Id> = self.peers.iter().filter(|(_, p)| now.duration_since(p.last_seen) > EXPIRY).map(|(id, _)| *id).collect();
        stale.into_iter().filter_map(|id| self.take(id).map(|p| p.name)).collect()
    }

    /// The address `name`'s game should advertise, if it probed from `ip`
    /// (the address its matchmaking connection comes from).
    pub fn advertised_for(&self, name: &str, ip: IpAddr) -> Option<SocketAddrV4> {
        let p = self.peers.get(self.by_name.get(&name.to_lowercase())?)?;
        (IpAddr::V4(*p.real.ip()) == ip).then_some(p.advertise)
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn relayed(&self) -> usize {
        self.peers.values().filter(|p| p.relayed).count()
    }
}

static TABLE: OnceLock<Arc<Mutex<Table>>> = OnceLock::new();

/// The address `name` should advertise, when the NAT helper runs and that
/// player probed it from `ip`.
pub fn advertised_for(name: &str, ip: IpAddr) -> Option<SocketAddrV4> {
    TABLE.get()?.lock().ok()?.advertised_for(name, ip)
}

/// Station URLs with `advertise` in place of a private address the game
/// registered for itself.
///
/// The game registers `prudp:/address=A;port=13000;RVCID=..;hdrType=0;type=2`
/// (plus, when it knows one, a second address without `type`). Other players
/// read the URL with `type` bit 2 as the address to reach it on, and the
/// other as its local one. When A is private, it becomes `advertise`, and
/// the original is kept as the local URL (without `type`), so players on the
/// same network still use it.
pub fn urls_with_public_address(urls: Vec<String>, advertise: SocketAddrV4) -> Vec<String> {
    let mut local = None;
    let has_local = urls.iter().any(|u| !u.split(';').any(|p| p.starts_with("type=")));
    let mut out: Vec<String> = urls
        .into_iter()
        .map(|url| {
            let parts: Vec<&str> = url.split(';').collect();
            let typ = parts.iter().find_map(|p| p.strip_prefix("type=")).and_then(|t| t.parse::<u32>().ok());
            let has_hdr = parts.iter().any(|p| p.starts_with("hdrType="));
            let addr = parts.iter().find_map(|p| p.split_once("address=").map(|(_, a)| a)).and_then(|a| a.parse::<Ipv4Addr>().ok());
            let (Some(typ), true, Some(addr)) = (typ, has_hdr, addr) else {
                return url;
            };
            if typ & 2 == 0 || !nat_proto::is_private(addr) || local.is_some() {
                return url;
            }
            local = Some(parts.iter().filter(|p| !p.starts_with("type=")).copied().collect::<Vec<_>>().join(";"));
            parts
                .iter()
                .map(|p| {
                    if let Some((scheme, _)) = p.split_once("address=") {
                        format!("{scheme}address={}", advertise.ip())
                    } else if p.starts_with("port=") {
                        format!("port={}", advertise.port())
                    } else {
                        (*p).to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(";")
        })
        .collect();
    if let (Some(local), false) = (local, has_local) {
        out.push(local);
    }
    out
}

fn v4(addr: SocketAddr) -> Option<SocketAddrV4> {
    match addr {
        SocketAddr::V4(a) => Some(a),
        SocketAddr::V6(a) => a.ip().to_ipv4_mapped().map(|ip| SocketAddrV4::new(ip, a.port())),
    }
}

/// How much a relay socket may queue: game traffic comes in bursts (every
/// player sends at once, 30 times a second), and a burst that doesn't fit is
/// lost before the relay sees it.
const SOCKET_BUFFER: usize = 4 * 1024 * 1024;

/// A UDP socket with large buffers. Linux caps them at `net.core.rmem_max`
/// and `wmem_max` (about 208 KB by default; the installer raises them), so
/// what it got is logged.
fn bind_with_buffers(logger: &Logger, addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let socket = socket2::Socket::new(socket2::Domain::for_address(addr), socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    let _ = socket.set_recv_buffer_size(SOCKET_BUFFER);
    let _ = socket.set_send_buffer_size(SOCKET_BUFFER);
    socket.bind(&addr.into())?;
    let (recv, send) = (socket.recv_buffer_size().unwrap_or(0), socket.send_buffer_size().unwrap_or(0));
    if recv < SOCKET_BUFFER {
        warn!(
            logger,
            "NAT relay: the receive buffer is {} KB, not {} KB, so bursts of relayed traffic may be lost. Raise it with: sysctl -w net.core.rmem_max={SOCKET_BUFFER} net.core.wmem_max={SOCKET_BUFFER}",
            recv / 1024,
            SOCKET_BUFFER / 1024
        );
    } else {
        info!(logger, "NAT relay: buffers {} KB in, {} KB out", recv / 1024, send / 1024);
    }
    Ok(socket.into())
}

/// Starts the helper's threads: the main port (probes and relay) and the
/// port after it (probes only, to spot NATs that change ports per
/// destination).
pub fn start(logger: &Logger, cfg: NatConfig, relay_ip: Ipv4Addr) -> std::io::Result<Vec<std::thread::JoinHandle<()>>> {
    let socket = bind_with_buffers(logger, cfg.listen)?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    let table = Arc::clone(TABLE.get_or_init(|| Arc::new(Mutex::new(Table::new(cfg, relay_ip)))));
    info!(
        logger,
        "NAT helper on UDP {} (relay {:?}, relay addresses {}:{}-{})", cfg.listen, cfg.relay, relay_ip, cfg.relay_ports.0, cfg.relay_ports.1
    );
    let mut threads = Vec::new();

    let mut second = cfg.listen;
    second.set_port(cfg.listen.port().wrapping_add(1));
    match UdpSocket::bind(second) {
        Ok(detect) => {
            let logger = logger.clone();
            threads.push(std::thread::Builder::new().name("nat-detect".into()).spawn(move || detect_loop(&logger, &detect))?);
        }
        Err(e) => warn!(
            logger,
            "NAT helper: can't listen on UDP {second} ({e}); symmetric NATs won't be spotted, so those players aren't relayed automatically"
        ),
    }

    let logger = logger.clone();
    threads.push(std::thread::Builder::new().name("nat".into()).spawn(move || main_loop(&logger, &socket, &table))?);
    Ok(threads)
}

fn detect_loop(logger: &Logger, socket: &UdpSocket) {
    let mut buf = [0u8; 256];
    loop {
        let Ok((len, src)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let (Some(src), Some(Message::Probe { nonce, .. })) = (v4(src), Message::decode(&buf[..len])) else {
            continue;
        };
        let reply = Message::ProbeReply {
            nonce,
            observed: src,
            advertise: src,
            flags: 0,
            relay_ip: Ipv4Addr::UNSPECIFIED,
            relay_ports: (0, 0),
        };
        if let Err(e) = socket.send_to(&reply.encode(), src) {
            debug!(logger, "NAT detect reply to {src} failed: {e}");
        }
    }
}

fn main_loop(logger: &Logger, socket: &UdpSocket, table: &Mutex<Table>) {
    let mut buf = vec![0u8; 2048];
    let mut out = Vec::with_capacity(2048);
    let mut last_expiry = Instant::now();
    // Relayed packets since the last report: forwarded, and dropped.
    let (mut forwarded, mut dropped, mut failed) = (0u64, 0u64, 0u64);
    loop {
        let now = Instant::now();
        if now.duration_since(last_expiry) > Duration::from_secs(10) {
            let secs = now.duration_since(last_expiry).as_secs_f64();
            last_expiry = now;
            let (gone, players, relayed) = table.lock().map(|mut t| (t.expire(now), t.len(), t.relayed())).unwrap_or_default();
            for name in gone {
                info!(logger, "NAT helper: {name} is gone");
            }
            if forwarded + dropped + failed > 0 {
                info!(
                    logger,
                    "NAT relay: {:.0} packets/s forwarded, {dropped} dropped (unknown player or over the rate), {failed} failed to send; {players} players, {relayed} relayed",
                    forwarded as f64 / secs
                );
            }
            (forwarded, dropped, failed) = (0, 0, 0);
        }
        let (len, src) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            // Windows reports an ICMP "port unreachable" for an earlier
            // send as an error on the next receive.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
            Err(e) => {
                warn!(logger, "NAT helper: receive failed: {e}");
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let Some(src) = v4(src) else { continue };
        let data = &buf[..len];

        if let Some((to, offset)) = nat_proto::data_to(data) {
            let route = table.lock().ok().and_then(|mut t| t.route(src, to, len - offset, now));
            if let Some((target, from)) = route {
                nat_proto::encode_data_from(&mut out, from, &data[offset..]);
                match socket.send_to(&out, target) {
                    Ok(_) => forwarded += 1,
                    Err(e) => {
                        failed += 1;
                        debug!(logger, "NAT relay to {target} failed: {e}");
                    }
                }
            } else {
                dropped += 1;
            }
            continue;
        }

        if let Some(Message::Probe { flags, nonce, mapping, name }) = Message::decode(data) {
            if flags & probe_flags::SECOND_PORT != 0 {
                continue;
            }
            let (reply, count, relayed, new) = {
                let Ok(mut t) = table.lock() else { continue };
                let new = t.advertised_for(&name, IpAddr::V4(*src.ip())).is_none();
                let reply = t.probe(src, flags, nonce, mapping, &name, now);
                (reply, t.len(), t.relayed(), new)
            };
            if new {
                if let Message::ProbeReply { advertise, flags: rf, .. } = &reply {
                    info!(
                        logger,
                        "NAT helper: {name} at {src} advertises {advertise}{} (mapping {mapping:?}, flags {flags:#x}); {count} players, {relayed} relayed",
                        if rf & reply_flags::RELAYED != 0 { " (relayed)" } else { "" }
                    );
                }
            }
            if let Err(e) = socket.send_to(&reply.encode(), src) {
                debug!(logger, "NAT probe reply to {src} failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddrV4 {
        s.parse().unwrap()
    }

    fn table(relay: RelayMode) -> Table {
        Table::new(NatConfig { relay, ..NatConfig::default() }, Ipv4Addr::new(192, 0, 2, 1))
    }

    fn advertise(m: &Message) -> (SocketAddrV4, bool) {
        match m {
            Message::ProbeReply { advertise, flags, .. } => (*advertise, flags & reply_flags::RELAYED != 0),
            _ => panic!("{m:?}"),
        }
    }

    #[test]
    fn a_player_advertises_the_address_the_helper_sees() {
        let mut t = table(RelayMode::Auto);
        let now = Instant::now();
        let reply = t.probe(a("198.51.100.7:61000"), 0, 42, None, "Sam", now);
        assert_eq!(advertise(&reply), (a("198.51.100.7:61000"), false));
        assert_eq!(t.advertised_for("sam", "198.51.100.7".parse().unwrap()), Some(a("198.51.100.7:61000")));
        // Only for the address the player probed from.
        assert_eq!(t.advertised_for("sam", "203.0.113.1".parse().unwrap()), None);
    }

    #[test]
    fn a_router_port_mapping_is_advertised_as_it_is() {
        let mut t = table(RelayMode::Auto);
        let reply = t.probe(a("198.51.100.7:61000"), probe_flags::HAS_MAPPING, 1, Some(a("198.51.100.7:13000")), "sam", Instant::now());
        assert_eq!(advertise(&reply), (a("198.51.100.7:13000"), false));
        // A mapping that is itself private (double NAT) is useless.
        let reply = t.probe(a("198.51.100.8:61000"), probe_flags::HAS_MAPPING, 1, Some(a("10.0.0.2:13000")), "fisher", Instant::now());
        assert_eq!(advertise(&reply), (a("198.51.100.8:61000"), false));
    }

    #[test]
    fn who_gets_relayed() {
        let now = Instant::now();
        let mut t = table(RelayMode::Auto);
        assert!(advertise(&t.probe(a("198.51.100.7:1"), probe_flags::SYMMETRIC, 1, None, "sym", now)).1);
        assert!(advertise(&t.probe(a("198.51.100.8:1"), probe_flags::WANT_RELAY, 1, None, "asked", now)).1);
        // On the server's own network, with no port mapping.
        assert!(advertise(&t.probe(a("192.168.1.20:13000"), 0, 1, None, "lan", now)).1);
        assert!(!advertise(&t.probe(a("198.51.100.9:1"), 0, 1, None, "cone", now)).1);

        let mut t = table(RelayMode::All);
        assert!(advertise(&t.probe(a("198.51.100.9:1"), 0, 1, None, "cone", now)).1);
        let mut t = table(RelayMode::Off);
        assert!(!advertise(&t.probe(a("198.51.100.7:1"), probe_flags::SYMMETRIC | probe_flags::WANT_RELAY, 1, None, "sym", now)).1);

        // A LAN party with the server on the same LAN: nobody is relayed.
        let mut t = Table::new(NatConfig::default(), Ipv4Addr::new(192, 168, 1, 5));
        assert!(!advertise(&t.probe(a("192.168.1.20:13000"), 0, 1, None, "lan", now)).1);
    }

    #[test]
    fn relayed_packets_reach_the_right_player_from_the_right_address() {
        let now = Instant::now();
        let mut t = table(RelayMode::Auto);
        let (sym_adv, _) = advertise(&t.probe(a("198.51.100.7:50001"), probe_flags::SYMMETRIC, 1, None, "sym", now));
        let (cone_adv, _) = advertise(&t.probe(a("203.0.113.4:13000"), 0, 1, None, "cone", now));
        assert_eq!(sym_adv, a("192.0.2.1:40000"));

        // The relayed player sends to the direct one's advertised address...
        assert_eq!(t.route(a("198.51.100.7:50001"), cone_adv, 100, now), Some((a("203.0.113.4:13000"), sym_adv)));
        // ...and the direct one to the relay address.
        assert_eq!(t.route(a("203.0.113.4:13000"), sym_adv, 100, now), Some((a("198.51.100.7:50001"), cone_adv)));
        // Strangers, unknown receivers and oneself are dropped.
        assert_eq!(t.route(a("203.0.113.99:1"), sym_adv, 100, now), None);
        assert_eq!(t.route(a("203.0.113.4:13000"), a("192.0.2.1:40500"), 100, now), None);
        assert_eq!(t.route(a("198.51.100.7:50001"), sym_adv, 100, now), None);
    }

    #[test]
    fn keepalives_never_move_an_address_the_game_already_advertises() {
        let now = Instant::now();
        let mut t = table(RelayMode::Auto);
        // Relayed, then a keepalive without the relay flag: still relayed.
        let (relay, _) = advertise(&t.probe(a("198.51.100.7:1"), probe_flags::WANT_RELAY, 1, None, "r", now));
        assert_eq!(advertise(&t.probe(a("198.51.100.7:1"), 0, 2, None, "r", now)), (relay, true));
        // Direct, then the router maps a port: the advertised address stays.
        let (direct, _) = advertise(&t.probe(a("203.0.113.4:61000"), 0, 1, None, "d", now));
        let later = t.probe(a("203.0.113.4:61000"), probe_flags::HAS_MAPPING, 2, Some(a("203.0.113.4:13000")), "d", now);
        assert_eq!(advertise(&later), (direct, false));
        // Its NAT gave it a new mapping: that's where it's reachable now.
        assert_eq!(advertise(&t.probe(a("203.0.113.4:61555"), 0, 3, None, "d", now)), (a("203.0.113.4:61555"), false));
        // Direct can still become relayed (before the game is told).
        assert!(advertise(&t.probe(a("203.0.113.4:61555"), probe_flags::SYMMETRIC, 4, None, "d", now)).1);
        // Packets keep flowing to the relayed player through all of this.
        assert!(t.route(a("203.0.113.4:61555"), relay, 10, now).is_some());
    }

    #[test]
    fn the_relay_rate_is_limited_per_player() {
        let now = Instant::now();
        let mut t = Table::new(
            NatConfig {
                relay_kbps_per_player: 1,
                ..NatConfig::default()
            },
            Ipv4Addr::new(192, 0, 2, 1),
        );
        let (x, _) = advertise(&t.probe(a("198.51.100.7:1"), probe_flags::WANT_RELAY, 1, None, "x", now));
        t.probe(a("198.51.100.8:1"), 0, 1, None, "y", now);
        assert!(t.route(a("198.51.100.8:1"), x, 1000, now).is_some());
        assert!(t.route(a("198.51.100.8:1"), x, 1000, now).is_none(), "over 1 KB in a second");
        assert!(t.route(a("198.51.100.8:1"), x, 1000, now + Duration::from_secs(1)).is_some(), "a new second");
    }

    #[test]
    fn players_keep_their_relay_port_and_expire() {
        let now = Instant::now();
        let mut t = table(RelayMode::All);
        let (first, _) = advertise(&t.probe(a("198.51.100.7:1"), 0, 1, None, "x", now));
        // The NAT gave the socket a new mapping: same player, same address.
        let (again, _) = advertise(&t.probe(a("198.51.100.7:2"), 0, 2, None, "X", now));
        assert_eq!(first, again);
        assert_eq!(t.len(), 1);
        assert!(t.route(a("198.51.100.7:1"), again, 1, now).is_none(), "the old mapping is gone");

        assert!(t.expire(now + Duration::from_secs(30)).is_empty());
        assert_eq!(t.expire(now + EXPIRY + Duration::from_secs(1)), ["x"]);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn registered_urls_get_the_public_address() {
        let public = a("198.51.100.7:61000");
        let sent = vec!["prudp:/address=192.168.1.20;port=13000;RVCID=5;hdrType=0;type=2".to_string()];
        assert_eq!(
            urls_with_public_address(sent, public),
            [
                "prudp:/address=198.51.100.7;port=61000;RVCID=5;hdrType=0;type=2",
                "prudp:/address=192.168.1.20;port=13000;RVCID=5;hdrType=0"
            ]
        );
        // The game already advertises a public address, or also sent a local one.
        let ok = vec!["prudp:/address=198.51.100.7;port=61000;RVCID=5;hdrType=0;type=2".to_string()];
        assert_eq!(urls_with_public_address(ok.clone(), public), ok);
        let both = vec![
            "prudp:/address=192.168.1.20;port=13000;RVCID=5;hdrType=0;type=2".to_string(),
            "prudp:/address=10.0.0.3;port=13000;RVCID=5;hdrType=0".to_string(),
        ];
        assert_eq!(
            urls_with_public_address(both, public),
            [
                "prudp:/address=198.51.100.7;port=61000;RVCID=5;hdrType=0;type=2",
                "prudp:/address=10.0.0.3;port=13000;RVCID=5;hdrType=0"
            ]
        );
        // URLs the game didn't write this way are left alone.
        let other = vec!["prudp:/address=192.168.1.20;port=3074;sid=15;type=3".to_string()];
        assert_eq!(urls_with_public_address(other.clone(), public), other);
    }
}
