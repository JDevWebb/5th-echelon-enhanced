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
/// Other ports a player's relayed data may come from (see [`Peer::aliases`]); the oldest goes.
const MAX_ALIASES: usize = 4;
/// Packets for a player go back to where their data last came from, for this long after.
const DATA_FROM_FRESH: Duration = Duration::from_secs(30);
/// Players registered at once, and from one address (a LAN party shares one).
const MAX_PEERS: usize = 20_000;
const MAX_PEERS_PER_IP: usize = 64;
/// Relayed packets one player may send a second (a match is ~30 per peer).
const MAX_PACKETS_PER_SECOND: u32 = 600;

/// The helper's secrets: the ticket key (kept in [`KEY_FILE`], so tickets
/// survive a restart) and the cookie key (new at every start).
struct Keys {
    ticket: [u8; 32],
    cookie: [u8; 32],
}

/// Next to the database.
pub const KEY_FILE: &str = "nat.key";

static KEYS: OnceLock<Keys> = OnceLock::new();

fn mac(key: &[u8; 32], parts: &[&[u8]]) -> [u8; 32] {
    use hmac::Mac as _;
    let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("any key length");
    for p in parts {
        m.update(&(p.len() as u32).to_be_bytes());
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

/// The ticket that lets `name`'s game register with the helper: handed out at
/// sign-in (`Users.Login`), good for [`TICKET_DAYS`] days. None when the
/// helper isn't running.
pub fn ticket_for(name: &str) -> Option<nat_proto::Ticket> {
    Some(ticket_on(KEYS.get()?, name, today()))
}

/// Days a ticket works: the day it was issued and the next six. The game
/// gets a new one each time it signs in to the API (at every start), so
/// only a game left running for a week loses its relay; a ticket someone
/// copies stops working within the week.
const TICKET_DAYS: u64 = 7;

fn today() -> u64 {
    minute() / (24 * 60)
}

/// The ticket for `name` issued on `day` (days since 1970): the day's low 16
/// bits, then a MAC of the name and the day.
fn ticket_on(keys: &Keys, name: &str, day: u64) -> nat_proto::Ticket {
    let full = mac(&keys.ticket, &[b"fes-nat-ticket-v2", identity::name_key(name).as_bytes(), &day.to_be_bytes()]);
    let mut ticket = [0; 16];
    ticket[..2].copy_from_slice(&(day as u16).to_be_bytes());
    ticket[2..].copy_from_slice(&full[..14]);
    ticket
}

/// Whether `ticket` is `name`'s and still good on `today`.
fn ticket_valid(keys: &Keys, name: &str, ticket: &nat_proto::Ticket, today: u64) -> bool {
    let issued = u16::from_be_bytes([ticket[0], ticket[1]]);
    let age = u64::from((today as u16).wrapping_sub(issued));
    age < TICKET_DAYS && same(&ticket_on(keys, name, today - age), ticket)
}

/// The cookie for a probe from `src` for `name`, in time bucket `bucket`
/// (minutes).
fn cookie_for(keys: &Keys, src: SocketAddrV4, name: &str, bucket: u64) -> nat_proto::Cookie {
    let full = mac(
        &keys.cookie,
        &[&src.ip().octets(), &src.port().to_be_bytes(), identity::name_key(name).as_bytes(), &bucket.to_be_bytes()],
    );
    full[..16].try_into().expect("16 of 32 bytes")
}

fn minute() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 60)
}

/// Compares secrets in time that doesn't depend on where they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

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
    window_packets: u32,
    /// The registration's secret: relayed data must carry it, both ways.
    tag: nat_proto::Tag,
    /// Since when `advertise` has been what it is.
    settled_since: Instant,
    /// Packets relayed from this player and to them since [`Table::flows`] last looked.
    sent: u64,
    received: u64,
    /// The game's last probe (relayed traffic doesn't count: a registration lives on that).
    probed_at: Instant,
    /// The game's round trip to this server, as it last measured it.
    rtt_ms: Option<u16>,
    /// Other ports on `real`'s address this player's relayed data came from, with its tag:
    /// the game's second Storm socket, or a NAT that gives each socket (or destination) its
    /// own port. Relayed data from them is theirs.
    aliases: Vec<SocketAddrV4>,
    /// Where this player's relayed data last came from, and when: packets for them go back
    /// there (the socket that sent), not to `real` (the one that probes).
    data_from: Option<(SocketAddrV4, Instant)>,
}

/// How long a direct player's address must stand before the address echo gives it out: the
/// hook checks a second port first, and a NAT that gives every destination its own port
/// then moves to the relay (a fraction of a second later).
const SETTLE: Duration = Duration::from_secs(2);

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
    /// [`Peer::aliases`], indexed.
    by_alias: HashMap<SocketAddrV4, Id>,
    /// Who registered from each address (at most [`MAX_PEERS_PER_IP`] each): what an unknown
    /// port's tag is checked against, without a scan of everyone.
    by_ip: HashMap<Ipv4Addr, Vec<Id>>,
    /// Relay ports given back by a Bye, and when: not handed out again for [`EXPIRY`], as
    /// the departed player's peers may still be sending to them.
    retired: HashMap<u16, Instant>,
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
            by_alias: HashMap::new(),
            by_ip: HashMap::new(),
            retired: HashMap::new(),
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
                //
                // Without one, nobody can start a connection to them: the game's own NAT
                // probes don't get a joiner through in practice (the first community night,
                // 2026-10-02: every host without a mapping - a router out of free ports, no
                // UPnP, carrier-grade NAT - could invite no one, and everyone could join the
                // hosts with one). So every player without a mapping goes through the relay,
                // whatever their NAT; players with one stay direct.
                //
                // A LAN party with the server on the same network relays nobody: the players
                // reach each other there as they are.
                // Asking for it, or a NAT that gives every destination its own port, still
                // always gets it.
                let lan_server = nat_proto::is_private(*observed.ip()) && nat_proto::is_private(self.relay_ip);
                flags & (probe_flags::WANT_RELAY | probe_flags::SYMMETRIC) != 0 || (!public_mapping && !lan_server)
            }
        }
    }

    fn free_vport(&self) -> Option<u16> {
        let (first, last) = self.cfg.relay_ports;
        (first..=last).find(|p| !self.by_vport.contains_key(p) && self.retired.get(p).is_none_or(|at| at.elapsed() > EXPIRY))
    }

    /// Takes a player out of the table, indexes and all.
    fn take(&mut self, id: Id) -> Option<Peer> {
        let p = self.peers.remove(&id)?;
        if let Some(ids) = self.by_ip.get_mut(p.real.ip()) {
            ids.retain(|i| *i != id);
            if ids.is_empty() {
                self.by_ip.remove(p.real.ip());
            }
        }
        self.by_name.remove(&p.name);
        if self.by_real.get(&p.real) == Some(&id) {
            self.by_real.remove(&p.real);
        }
        for a in &p.aliases {
            if self.by_alias.get(a) == Some(&id) {
                self.by_alias.remove(a);
            }
        }
        if self.by_advertise.get(&p.advertise) == Some(&id) {
            self.by_advertise.remove(&p.advertise);
        }
        if let Some(v) = p.vport {
            self.by_vport.remove(&v);
        }
        Some(p)
    }

    /// Whether a probe from `src` for `name` may register: the table and
    /// each address have a limit (a refresh of an existing one always may).
    pub fn admits(&self, src: SocketAddrV4, name: &str) -> bool {
        if self.by_name.contains_key(&name.to_lowercase()) {
            return true;
        }
        let here = self.peers.values().filter(|p| p.real.ip() == src.ip()).count();
        self.peers.len() < MAX_PEERS && here < MAX_PEERS_PER_IP
    }

    /// Registers (or refreshes) the player behind `src` and answers its probe.
    pub fn probe(&mut self, src: SocketAddrV4, flags: u8, nonce: u32, mapping: Option<SocketAddrV4>, name: &str, now: Instant) -> Message {
        // The router port mapping the client reports is only its word: a mapping on another
        // address than the one this probe came from would have this player advertised (and
        // relayed packets routed) at someone else's address.
        let mapping = mapping.filter(|m| m.ip() == src.ip());
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
        let vport = if relayed {
            old.as_ref().and_then(|(_, o)| o.vport).or_else(|| self.free_vport())
        } else {
            None
        };
        let relayed = relayed && vport.is_some();
        let id = old.as_ref().map_or_else(
            || {
                self.next_id += 1;
                self.next_id
            },
            |(id, _)| *id,
        );
        // Another player's advertised address is theirs: relayed packets for it go to them.
        let taken = |table: &Self, a: &SocketAddrV4| table.by_advertise.get(a).is_some_and(|other| *other != id);
        let advertise = match (vport, mapping, &old) {
            (Some(v), _, _) => SocketAddrV4::new(self.relay_ip, v),
            (None, _, Some((_, o))) if !o.relayed && o.real == src => o.advertise,
            (None, Some(m), _) if !nat_proto::is_private(*m.ip()) => m,
            _ => src,
        };
        let advertise = if vport.is_none() && taken(self, &advertise) { src } else { advertise };
        let peer = Peer {
            name: key.clone(),
            real: src,
            advertise,
            relayed,
            vport,
            last_seen: now,
            window_start: old.as_ref().map_or(now, |(_, o)| o.window_start),
            window_bytes: old.as_ref().map_or(0, |(_, o)| o.window_bytes),
            window_packets: old.as_ref().map_or(0, |(_, o)| o.window_packets),
            // Kept while the registration lives: the game already relays with it.
            tag: old.as_ref().map_or_else(rand::random, |(_, o)| o.tag),
            settled_since: old.as_ref().filter(|(_, o)| o.advertise == advertise).map_or(now, |(_, o)| o.settled_since),
            sent: old.as_ref().map_or(0, |(_, o)| o.sent),
            received: old.as_ref().map_or(0, |(_, o)| o.received),
            probed_at: now,
            rtt_ms: old.as_ref().and_then(|(_, o)| o.rtt_ms),
            // The other ports stay theirs while the address does.
            aliases: old
                .as_ref()
                .map(|(_, o)| o.aliases.iter().copied().filter(|a| a.ip() == src.ip() && *a != src).collect())
                .unwrap_or_default(),
            data_from: old.as_ref().and_then(|(_, o)| o.data_from).filter(|(a, _)| a.ip() == src.ip()),
        };
        let tag = peer.tag;
        for a in &peer.aliases {
            self.by_alias.insert(*a, id);
        }
        self.by_name.insert(key, id);
        self.by_real.insert(src, id);
        self.by_ip.entry(*src.ip()).or_default().push(id);
        if !taken(self, &advertise) {
            self.by_advertise.insert(advertise, id);
        }
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
            cookie: [0; 16],
            tag,
        }
    }

    /// [`Self::route`], for a packet that must carry its sender's `tag`.
    ///
    /// From a port the player didn't register on, their tag proves it's theirs as long as it
    /// comes from their address: the game's other Storm socket, or a NAT that maps each socket
    /// (or destination) to its own port, as PlaySkill's mobile carrier did (eu1, 2026-10-06:
    /// registered from 47868, the match's traffic from 47861, dropped until the game happened
    /// to register again). That port is theirs from then on. Someone else at the address
    /// (carrier-grade NAT) doesn't have the tag.
    pub fn route_tagged(&mut self, src: SocketAddrV4, tag: nat_proto::Tag, to: SocketAddrV4, len: usize, now: Instant) -> Option<(SocketAddrV4, SocketAddrV4, nat_proto::Tag)> {
        let sender_id = match self.by_real.get(&src).or_else(|| self.by_alias.get(&src)) {
            Some(id) => *id,
            None => self.adopt_port(src, &tag)?,
        };
        if !same(&self.peers.get(&sender_id)?.tag, &tag) {
            return None;
        }
        let (target, from, target_id) = self.route_from(sender_id, to, len, now)?;
        if let Some(sender) = self.peers.get_mut(&sender_id) {
            sender.data_from = Some((src, now));
        }
        let target_tag = self.peers.get(&target_id)?.tag;
        Some((target, from, target_tag))
    }

    /// The player registered from `src`'s address with `tag`, taking `src` as theirs too.
    fn adopt_port(&mut self, src: SocketAddrV4, tag: &nat_proto::Tag) -> Option<Id> {
        let id = *self.by_ip.get(src.ip())?.iter().find(|id| self.peers.get(id).is_some_and(|p| same(&p.tag, tag)))?;
        let peer = self.peers.get_mut(&id)?;
        if peer.aliases.len() >= MAX_ALIASES {
            let old = peer.aliases.remove(0);
            self.by_alias.remove(&old);
        }
        peer.aliases.push(src);
        self.by_alias.insert(src, id);
        Some(id)
    }

    /// Where a relayed packet from `src` to `to` goes: the receiver's real
    /// address, and the sender address its game should see. `None` drops it
    /// (unknown sender or receiver, or the sender is over its rate).
    pub fn route(&mut self, src: SocketAddrV4, to: SocketAddrV4, len: usize, now: Instant) -> Option<(SocketAddrV4, SocketAddrV4)> {
        let sender_id = *self.by_real.get(&src)?;
        self.route_from(sender_id, to, len, now).map(|(target, from, _)| (target, from))
    }

    /// [`Self::route`] for the player `sender_id`; also the receiver's id.
    fn route_from(&mut self, sender_id: Id, to: SocketAddrV4, len: usize, now: Instant) -> Option<(SocketAddrV4, SocketAddrV4, Id)> {
        let (first, last) = self.cfg.relay_ports;
        // A relay port, or else a player's own address (one on the relay's address whose port
        // happens to be in the relay's range: a player on the server's own machine).
        let relayed = (*to.ip() == self.relay_ip && (first..=last).contains(&to.port()))
            .then(|| self.by_vport.get(&to.port()))
            .flatten();
        let target_id = *relayed.or_else(|| self.by_advertise.get(&to)).or_else(|| self.by_real.get(&to))?;
        if target_id == sender_id {
            return None;
        }
        // Back to the socket their data last came from, while it's recent; else the one that
        // probes (it keeps its own mapping alive).
        let target = self
            .peers
            .get(&target_id)
            .map(|p| p.data_from.filter(|(_, at)| now.duration_since(*at) < DATA_FROM_FRESH).map_or(p.real, |(a, _)| a))?;
        let limit = self.cfg.relay_kbps_per_player as usize * 1024;
        let sender = self.peers.get_mut(&sender_id)?;
        sender.last_seen = now;
        if now.duration_since(sender.window_start) >= Duration::from_secs(1) {
            sender.window_start = now;
            sender.window_bytes = 0;
            sender.window_packets = 0;
        }
        // Headers count too, and packets: empty payloads must not slip past the byte limit.
        sender.window_bytes += len + nat_proto::DATA_OVERHEAD;
        sender.window_packets += 1;
        if sender.window_bytes > limit || sender.window_packets > MAX_PACKETS_PER_SECOND {
            return None;
        }
        sender.sent += 1;
        let from = sender.advertise;
        if let Some(receiver) = self.peers.get_mut(&target_id) {
            receiver.received += 1;
        }
        Some((target, from, target_id))
    }

    /// Notes `name`'s round trip to this server, as its game measured it.
    pub fn note_rtt(&mut self, name: &str, rtt_ms: Option<u16>) {
        let Some(ms) = rtt_ms else { return };
        if let Some(p) = self.by_name.get(&name.to_lowercase()).and_then(|id| self.peers.get_mut(id)) {
            p.rtt_ms = Some(ms);
        }
    }

    /// How `guest`'s game and `host`'s reach each other, when the guest is registered.
    pub fn path(&self, guest: &str, host: &str) -> Option<Path> {
        let peer = |name: &str| self.by_name.get(&name.to_lowercase()).and_then(|id| self.peers.get(id));
        let g = peer(guest)?;
        let h = peer(host);
        Some(Path {
            ping_ms: g.rtt_ms,
            host_ping_ms: h.and_then(|h| h.rtt_ms),
            relayed: g.relayed || h.is_some_and(|h| h.relayed),
            ips: h.map(|h| (*g.real.ip(), *h.real.ip())),
        })
    }

    /// Each player's relayed packets (from them, to them) since the last call.
    pub fn flows(&mut self) -> Vec<(String, u64, u64)> {
        self.peers
            .values_mut()
            .filter(|p| p.sent + p.received > 0 || p.relayed)
            .map(|p| (p.name.clone(), std::mem::take(&mut p.sent), std::mem::take(&mut p.received)))
            .collect()
    }

    /// Whether `name`'s game probed at or after `since`.
    pub fn probed_since(&self, name: &str, since: Instant) -> bool {
        self.by_name
            .get(&name.to_lowercase())
            .and_then(|id| self.peers.get(id))
            .is_some_and(|p| p.probed_at >= since)
    }

    /// Who an address the relay saw is: a registered player's game, an unregistered port on
    /// the address a player registered from (a game sending from a port it didn't register),
    /// or nobody known.
    fn who_sent(&self, src: SocketAddrV4) -> String {
        if let Some(p) = self.by_real.get(&src).or_else(|| self.by_alias.get(&src)).and_then(|id| self.peers.get(id)) {
            return p.name.clone();
        }
        let same_ip: Vec<String> = self
            .peers
            .values()
            .filter(|p| p.real.ip() == src.ip())
            .map(|p| format!("{} (registered from port {})", p.name, p.real.port()))
            .collect();
        if same_ip.is_empty() {
            "nobody registered".into()
        } else {
            format!("an unregistered port of {}", same_ip.join(", "))
        }
    }

    /// Whose relay port (or advertised address) `to` is.
    fn who_has(&self, to: SocketAddrV4) -> String {
        self.by_advertise
            .get(&to)
            .and_then(|id| self.peers.get(id))
            .map_or_else(|| "nobody's".into(), |p| format!("{}'s", p.name))
    }

    /// Forgets the player registered from `src` with `tag`, whose game says it's going
    /// offline (it closed its Storm socket); their name, if that was one. Anyone else's
    /// tag, or another address, changes nothing.
    pub fn bye(&mut self, src: SocketAddrV4, tag: nat_proto::Tag) -> Option<String> {
        let id = *self.by_real.get(&src)?;
        if !same(&self.peers.get(&id)?.tag, &tag) {
            return None;
        }
        let p = self.take(id)?;
        if let Some(v) = p.vport {
            if self.retired.len() > 4096 {
                self.retired.retain(|_, at| at.elapsed() <= EXPIRY);
            }
            self.retired.insert(v, Instant::now());
        }
        Some(p.name)
    }

    /// The players [`Self::expire`] would forget now, with their game's last probe.
    fn expiring(&self, now: Instant) -> Vec<(String, Instant)> {
        self.peers
            .values()
            .filter(|p| now.duration_since(p.last_seen) > EXPIRY)
            .map(|p| (p.name.clone(), p.probed_at))
            .collect()
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

    /// The address the one player registered from `ip` should be reached on, if exactly one
    /// is (several behind one router can't be told apart by address alone).
    ///
    /// Only once that address is final: a relayed one at once, a direct one when it has stood
    /// for [`SETTLE`] (until then the player may still move to the relay).
    pub fn advertised_for_ip(&self, ip: Ipv4Addr, now: Instant) -> Option<SocketAddrV4> {
        let mut found = self.peers.values().filter(|p| *p.real.ip() == ip);
        let first = found.next()?;
        let settled = first.relayed || now.duration_since(first.settled_since) >= SETTLE;
        (found.next().is_none() && settled).then_some(first.advertise)
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn relayed(&self) -> usize {
        self.peers.values().filter(|p| p.relayed).count()
    }
}

/// How a guest's game and their host's reach each other ([`Table::path`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// Each one's round trip to this server, as their game measured it.
    pub ping_ms: Option<u16>,
    pub host_ping_ms: Option<u16>,
    /// Whether their traffic goes through the relay (either one relayed is enough).
    pub relayed: bool,
    /// Their public addresses (guest, host), to place them; never sent.
    ips: Option<(Ipv4Addr, Ipv4Addr)>,
}

/// Light in fibre covers about 200 km a millisecond: a round trip takes at least this long
/// per km between two players, whatever the route.
const KM_PER_RTT_MS: f64 = 100.0;

impl Path {
    /// As event detail: `ping_ms`, `host_ping_ms`, `relayed`, and for a relayed pair
    /// `relay_ms` (their round trip through this server: one's to it, plus the other's) and
    /// `direct_ms` (the least a direct one could take, over the distance between where they
    /// are, from `km`).
    pub fn detail(&self, km: impl Fn(Ipv4Addr, Ipv4Addr) -> Option<f64>) -> serde_json::Value {
        let mut d = serde_json::json!({ "relayed": self.relayed });
        if let Some(ms) = self.ping_ms {
            d["ping_ms"] = ms.into();
        }
        if let Some(ms) = self.host_ping_ms {
            d["host_ping_ms"] = ms.into();
        }
        if self.relayed {
            if let (Some(a), Some(b)) = (self.ping_ms, self.host_ping_ms) {
                d["relay_ms"] = (u32::from(a) + u32::from(b)).into();
            }
            if let Some(km) = self.ips.and_then(|(a, b)| if a == b { Some(0.0) } else { km(a, b) }) {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let ms = (km / KM_PER_RTT_MS).round() as u32;
                d["direct_ms"] = ms.into();
            }
        }
        d
    }
}

/// How `guest` and the `host` they joined reach each other, as event detail
/// ([`Path::detail`]), when the helper runs and the guest's game registered with it.
pub fn path_detail(guest: &str, host: &str) -> Option<serde_json::Value> {
    let path = TABLE.get()?.lock().ok()?.path(guest, host)?;
    // Looked up with the table let go: relaying waits on it.
    Some(path.detail(|a, b| {
        let (a, b) = (crate::metrics::place(IpAddr::V4(a))?, crate::metrics::place(IpAddr::V4(b))?);
        (a.located() && b.located()).then(|| a.km_to(&b))
    }))
}

static TABLE: OnceLock<Arc<Mutex<Table>>> = OnceLock::new();

/// The address relayed players are given (this server's), once the helper runs.
pub fn relay_ip() -> Option<Ipv4Addr> {
    TABLE.get().map(|t| t.lock().unwrap_or_else(std::sync::PoisonError::into_inner).relay_ip)
}

/// `name`'s round trip to this server, as their game last measured it.
pub fn ping_of(name: &str) -> Option<u16> {
    let t = TABLE.get()?.lock().ok()?;
    t.peers.get(t.by_name.get(&name.to_lowercase())?)?.rtt_ms
}

/// The address `name` should advertise, when the NAT helper runs and that
/// player probed it from `ip`.
pub fn advertised_for(name: &str, ip: IpAddr) -> Option<SocketAddrV4> {
    TABLE.get()?.lock().ok()?.advertised_for(name, ip)
}

/// The address the NAT helper checked for the one player at `ip` (their public one, or the
/// relay's), when it runs and exactly one player there is registered.
pub fn advertised_for_ip(ip: IpAddr) -> Option<SocketAddrV4> {
    let IpAddr::V4(ip) = ip else { return None };
    TABLE.get()?.lock().ok()?.advertised_for_ip(ip, Instant::now())
}

/// Station URLs with `advertise` in place of the address the game registered
/// for itself.
///
/// The game registers `prudp:/address=A;port=13000;RVCID=..;hdrType=0;type=2`
/// (plus, when it knows one, a second address without `type`). Other players
/// read the URL with `type` bit 2 as the address to reach it on, and the
/// other as its local one. A becomes `advertise` when they differ: when A is
/// private (it is then kept as the local URL, without `type`, so players on
/// the same network still use it), and when A is public too. The game may
/// have taken a public address from the server's view of its connection;
/// behind a NAT that gives every destination its own port (a VPN, a mobile
/// network) nobody else reaches it there, and `advertise` is the address this
/// helper checked: the player's own, or the relay's.
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
            let port = parts.iter().find_map(|p| p.strip_prefix("port=")).and_then(|p| p.parse::<u16>().ok());
            if typ & 2 == 0 || local.is_some() || (addr == *advertise.ip() && port == Some(advertise.port())) {
                return url;
            }
            let private = nat_proto::is_private(addr);
            local = Some(if private {
                parts.iter().filter(|p| !p.starts_with("type=")).copied().collect::<Vec<_>>().join(";")
            } else {
                String::new()
            });
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
    if let (Some(local), false) = (local.filter(|l| !l.is_empty()), has_local) {
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
    let ticket = crate::keys::load_or_create(std::path::Path::new(KEY_FILE)).map_err(|e| std::io::Error::other(e.to_string()))?;
    let _ = KEYS.set(Keys {
        ticket: ticket.0,
        cookie: rand::random(),
    });
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
        let (len, src) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            // An ICMP error for an earlier send, or the read timeout: nothing wrong here.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                continue
            }
            // Anything else may come again at once: not round and round at full speed.
            Err(_) => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let (Some(src), Some(Message::Probe { nonce, .. })) = (v4(src), Message::decode(&buf[..len])) else {
            continue;
        };
        if let Err(e) = socket.send_to(&address_only(nonce, src, [0; 16]).encode(), src) {
            debug!(logger, "NAT detect reply to {src} failed: {e}");
        }
    }
}

/// A reply that only tells the sender its address (and a cookie to come back
/// with), registering nothing: no bigger than the probe, so it amplifies
/// nothing.
fn address_only(nonce: u32, src: SocketAddrV4, cookie: nat_proto::Cookie) -> Message {
    Message::ProbeReply {
        nonce,
        observed: src,
        advertise: src,
        flags: 0,
        relay_ip: Ipv4Addr::UNSPECIFIED,
        relay_ports: (0, 0),
        cookie,
        tag: [0; 8],
    }
}

/// A game that went online (opened or joined a room) should probe the helper within this (it
/// does in a second or two): one that hasn't can't be reached by anyone, relayed or direct.
/// Not from signing in: a game in the menus has no Storm socket, so nothing to register.
const PROBE_WITHIN: Duration = Duration::from_secs(90);

/// Games online and not yet probed since: when they went online, by name.
fn awaiting() -> std::sync::MutexGuard<'static, HashMap<String, Instant>> {
    static AWAITING: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    AWAITING.get_or_init(Mutex::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Games signed in to the server now, by name, with when they signed in: a registration
/// that lapses while its game is still signed in leaves it unreachable.
fn signed_in() -> std::sync::MutexGuard<'static, HashMap<String, Instant>> {
    static SIGNED_IN: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    SIGNED_IN.get_or_init(Mutex::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `name`'s game signed in.
pub fn game_signed_in(name: &str) {
    if TABLE.get().is_some() {
        signed_in().insert(name.to_lowercase(), Instant::now());
    }
}

/// `name`'s game went online (opened or joined a room): it should be registered with the
/// helper soon (see [`PROBE_WITHIN`]).
pub fn game_went_online(name: &str) {
    let key = name.to_lowercase();
    if TABLE.get().is_some() && signed_in().contains_key(&key) {
        awaiting().entry(key).or_insert_with(Instant::now);
    }
}

/// `name`'s game signed out (or its connection timed out): nothing to wait for.
pub fn game_signed_out(name: &str) {
    awaiting().remove(&name.to_lowercase());
    signed_in().remove(&name.to_lowercase());
}

/// Of the players whose registration just lapsed (with their last probe), those whose game
/// is still signed in, with how long since it last probed. Their game stopped probing (the
/// hook every 20 s) though it still talks to the server: nobody can reach it, relayed or
/// direct, until it registers again (a restart of the game did, on NA1 on 2026-10-04).
///
/// Not when the game signed in again since its last probe: that's the game before a restart,
/// whose registration lapsing says nothing about the new one.
fn lapsed_while_signed_in(gone: &[(String, Instant)], now: Instant) -> Vec<(String, u64)> {
    let signed_in = signed_in();
    gone.iter()
        .filter(|(name, probed)| signed_in.get(&name.to_lowercase()).is_some_and(|since| since <= probed))
        .map(|(name, probed)| (name.clone(), now.duration_since(*probed).as_secs()))
        .collect()
}

/// Games that signed in [`PROBE_WITHIN`] ago or more and haven't probed since (each said
/// once), with how long ago they signed in; those that probed are let go.
fn unregistered(table: &Table, now: Instant) -> Vec<(String, u64)> {
    let mut late = Vec::new();
    awaiting().retain(|name, at| {
        if table.probed_since(name, *at) {
            return false;
        }
        let waited = now.duration_since(*at);
        if waited >= PROBE_WITHIN {
            late.push((name.clone(), waited.as_secs()));
            return false;
        }
        true
    });
    late
}

/// Dropped relayed packets whose senders are told, at most this many a report (the busiest).
const DROPS_TOLD: usize = 3;

/// A player's relayed traffic in one direction counts as having fallen when it was at least
/// this many packets a second (a match is 30 or more) and is now under a quarter of that.
const DROP_FROM_PPS: f64 = 20.0;
const DROP_TO_SHARE: f64 = 0.25;

/// Watches each player's relayed traffic from one look to the next (every ten seconds), for
/// a match whose traffic stops while the players are still connected: the relay can't tell
/// why, but the admin UI can show which side went quiet.
#[derive(Default)]
struct RelayWatch {
    last: HashMap<String, (f64, f64)>,
}

impl RelayWatch {
    /// Takes the packets each player sent and received over `secs`; returns who fell, which
    /// way (`sending`: from them, `receiving`: to them), and from what to what (a second).
    fn tick(&mut self, flows: &[(String, u64, u64)], secs: f64) -> Vec<(String, &'static str, f64, f64)> {
        let mut fell = Vec::new();
        let mut now = HashMap::with_capacity(flows.len());
        #[allow(clippy::cast_precision_loss)]
        for (name, sent, received) in flows {
            let rates = (*sent as f64 / secs.max(1.0), *received as f64 / secs.max(1.0));
            if let Some(&(sent_before, received_before)) = self.last.get(name) {
                for (direction, before, after) in [("sending", sent_before, rates.0), ("receiving", received_before, rates.1)] {
                    if before >= DROP_FROM_PPS && after < before * DROP_TO_SHARE {
                        fell.push((name.clone(), direction, before, after));
                    }
                }
            }
            now.insert(name.clone(), rates);
        }
        self.last = now;
        fell
    }
}

fn main_loop(logger: &Logger, socket: &UdpSocket, table: &Mutex<Table>) {
    let mut buf = vec![0u8; 2048];
    let mut out = Vec::with_capacity(2048);
    let mut last_expiry = Instant::now();
    let mut watch = RelayWatch::default();
    // Relayed packets since the last report: forwarded, and dropped.
    let (mut forwarded, mut dropped, mut failed) = (0u64, 0u64, 0u64);
    // Where the dropped ones came from and went to (a few hundred at most a report).
    let mut drops: HashMap<(SocketAddrV4, SocketAddrV4), u64> = HashMap::new();
    loop {
        let now = Instant::now();
        if now.duration_since(last_expiry) > Duration::from_secs(10) {
            let secs = now.duration_since(last_expiry).as_secs_f64();
            last_expiry = now;
            let (gone, players, relayed, flows, late, dropped_from, lapsed) = table
                .lock()
                .map(|mut t| {
                    // Who sent the packets the relay dropped, and whose port they were for: a
                    // game sending from a port it never registered shows here.
                    let mut busiest: Vec<_> = drops.drain().collect();
                    busiest.sort_by(|a, b| b.1.cmp(&a.1));
                    let told: Vec<String> = if dropped >= 3 {
                        busiest
                            .iter()
                            .take(DROPS_TOLD)
                            .map(|((src, to), n)| format!("{n} from {src} ({}) to {to} ({} relay port)", t.who_sent(*src), t.who_has(*to)))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let lapsed = lapsed_while_signed_in(&t.expiring(now), now);
                    (t.expire(now), t.len(), t.relayed(), t.flows(), unregistered(&t, now), told, lapsed)
                })
                .unwrap_or_default();
            if !dropped_from.is_empty() {
                info!(logger, "NAT relay: dropped {}", dropped_from.join("; "));
            }
            for (name, secs) in late {
                warn!(
                    logger,
                    "NAT helper: {name}'s game went online {secs} s ago (opened or joined a room) and hasn't registered with the helper: nobody can reach it until it does"
                );
                crate::session_events::note(crate::session_events::Who::Name(name), "nat_missing", serde_json::json!({ "after_secs": secs }));
            }
            for (name, direction, before, after) in watch.tick(&flows, secs) {
                info!(logger, "NAT relay: {name}'s traffic {direction} fell from {before:.0} to {after:.0} packets/s");
                crate::session_events::note(
                    crate::session_events::Who::Name(name),
                    "relay_drop",
                    serde_json::json!({ "direction": direction, "before": before.round(), "after": after.round() }),
                );
            }
            for name in gone.iter().filter(|n| !lapsed.iter().any(|(l, _)| l == *n)) {
                info!(logger, "NAT helper: {name} is gone");
            }
            for (name, secs) in lapsed {
                warn!(
                    logger,
                    "NAT helper: {name}'s registration lapsed (last probe {secs} s ago) while the game is still signed in: nobody can reach it until it registers again"
                );
                crate::session_events::note(crate::session_events::Who::Name(name), "nat_lost", serde_json::json!({ "probe_secs": secs }));
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

        if let Some((tag, to, offset)) = nat_proto::data_to(data) {
            let route = table.lock().ok().and_then(|mut t| t.route_tagged(src, tag, to, len - offset, now));
            if let Some((target, from, target_tag)) = route {
                nat_proto::encode_data_from(&mut out, target_tag, from, &data[offset..]);
                match socket.send_to(&out, target) {
                    Ok(n) => {
                        forwarded += 1;
                        crate::metrics::relayed(n);
                    }
                    Err(e) => {
                        failed += 1;
                        debug!(logger, "NAT relay to {target} failed: {e}");
                    }
                }
            } else {
                dropped += 1;
                if drops.len() < 256 {
                    *drops.entry((src, to)).or_default() += 1;
                }
            }
            continue;
        }

        if let Some(Message::Bye { tag }) = Message::decode(data) {
            // The game went offline (or quit): nobody can join it now, and that's no fault.
            if let Some(name) = table.lock().ok().and_then(|mut t| t.bye(src, tag)) {
                info!(logger, "NAT helper: {name}'s game went offline");
            }
            continue;
        }

        if let Some(Message::Probe {
            flags,
            nonce,
            mapping,
            name,
            ticket,
            cookie,
            rtt_ms,
        }) = Message::decode(data)
        {
            if flags & probe_flags::SECOND_PORT != 0 {
                continue;
            }
            let Some(keys) = KEYS.get() else { continue };
            // No ticket for this name: it only learns its address (the launcher's test).
            if name.is_empty() || !ticket_valid(keys, &name, &ticket, today()) {
                let _ = socket.send_to(&address_only(nonce, src, [0; 16]).encode(), src);
                continue;
            }
            // Not yet proved it receives at this address: a cookie to come back with.
            let bucket = minute();
            let proved = [bucket, bucket.saturating_sub(1)].iter().any(|b| same(&cookie_for(keys, src, &name, *b), &cookie));
            if !proved {
                let _ = socket.send_to(&address_only(nonce, src, cookie_for(keys, src, &name, bucket)).encode(), src);
                continue;
            }
            let (reply, count, relayed, new) = {
                let Ok(mut t) = table.lock() else { continue };
                if !t.admits(src, &name) {
                    debug!(logger, "NAT helper: table full (or too many from {}); {name} not registered", src.ip());
                    continue;
                }
                let new = t.advertised_for(&name, IpAddr::V4(*src.ip())).is_none();
                let mut reply = t.probe(src, flags, nonce, mapping, &name, now);
                t.note_rtt(&name, rtt_ms);
                if let Message::ProbeReply { cookie: c, .. } = &mut reply {
                    *c = cookie_for(keys, src, &name, bucket);
                }
                (reply, t.len(), t.relayed(), new)
            };
            if new {
                if let Message::ProbeReply { advertise, flags: rf, .. } = &reply {
                    crate::reports::nat_registered(&name, rf & reply_flags::RELAYED != 0);
                    crate::session_events::note(
                        crate::session_events::Who::Name(name.clone()),
                        "nat",
                        serde_json::json!({ "relayed": rf & reply_flags::RELAYED != 0 }),
                    );
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

    /// A game that signed in and never probed is told once, after a while; one that probed
    /// since, or is still within its time, isn't.
    #[test]
    fn a_game_that_signs_in_and_never_registers_is_noticed() {
        let mut t = table(RelayMode::Auto);
        let now = Instant::now();
        let ago = |s| now.checked_sub(Duration::from_secs(s)).unwrap();
        awaiting().insert("nat-test-silent".into(), ago(100));
        awaiting().insert("nat-test-registered".into(), ago(100));
        awaiting().insert("nat-test-starting".into(), ago(10));
        t.probe(a("198.51.100.20:13000"), 0, 1, None, "NAT-Test-Registered", ago(5));
        let late = unregistered(&t, now);
        assert_eq!(late.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["nat-test-silent"]);
        assert!(late[0].1 >= 100);
        assert!(unregistered(&t, now).is_empty(), "told once");
        assert!(awaiting().contains_key("nat-test-starting"), "still has time");
        // A probe from before the sign-in (the game before a restart) doesn't count.
        awaiting().insert("nat-test-registered".into(), ago(1));
        assert!(!t.probed_since("nat-test-registered", ago(1)));
        game_signed_out("NAT-Test-Starting");
        assert!(!awaiting().contains_key("nat-test-starting"));
    }

    /// A registration that lapses while its game is still signed in is told; one whose game
    /// signed out (or never signed in here) isn't.
    #[test]
    fn a_registration_lapsing_while_the_game_is_signed_in_is_noticed() {
        let mut t = table(RelayMode::All);
        let now = Instant::now();
        let ago = |s| now.checked_sub(Duration::from_secs(s)).unwrap();
        t.probe(a("198.51.100.30:13000"), 0, 1, None, "NAT-Lapse-On", ago(120));
        t.probe(a("198.51.100.31:13000"), 0, 1, None, "NAT-Lapse-Off", ago(120));
        signed_in().insert("nat-lapse-on".into(), ago(600));
        let lapsed = lapsed_while_signed_in(&t.expiring(now), now);
        assert_eq!(lapsed.len(), 1, "{lapsed:?}");
        assert_eq!(lapsed[0].0, "nat-lapse-on");
        assert!(lapsed[0].1 >= 120);
        assert_eq!(t.expire(now).len(), 2);
        // The game restarted (signed in again) after its last probe: the old game's lapse.
        signed_in().insert("nat-lapse-on".into(), ago(60));
        assert!(lapsed_while_signed_in(&[("nat-lapse-on".into(), ago(100))], now).is_empty());
        game_signed_out("NAT-Lapse-On");
        assert!(lapsed_while_signed_in(&[("nat-lapse-on".into(), ago(100))], now).is_empty());
    }

    /// The relay's dropped packets are put down to who sent them: a registered game, an
    /// unregistered port of a player's address, or nobody.
    #[test]
    fn dropped_packets_say_who_sent_them() {
        let mut t = table(RelayMode::All);
        let now = Instant::now();
        let (sam, relayed) = advertise(&t.probe(a("198.51.100.7:13000"), 0, 1, None, "Sam", now));
        assert!(relayed);
        assert_eq!(t.who_sent(a("198.51.100.7:13000")), "sam");
        assert_eq!(t.who_sent(a("198.51.100.7:3074")), "an unregistered port of sam (registered from port 13000)");
        assert_eq!(t.who_sent(a("203.0.113.9:13000")), "nobody registered");
        assert_eq!(t.who_has(sam), "sam's");
        assert_eq!(t.who_has(a("192.0.2.1:49999")), "nobody's");
    }

    #[test]
    fn a_player_advertises_the_address_the_helper_sees() {
        let mut t = table(RelayMode::Off);
        let now = Instant::now();
        let reply = t.probe(a("198.51.100.7:61000"), 0, 42, None, "Sam", now);
        assert_eq!(advertise(&reply), (a("198.51.100.7:61000"), false));
        assert_eq!(t.advertised_for("sam", "198.51.100.7".parse().unwrap()), Some(a("198.51.100.7:61000")));
        // Only for the address the player probed from.
        assert_eq!(t.advertised_for("sam", "203.0.113.1".parse().unwrap()), None);
    }

    /// A join's network detail: each one's ping as their game reported it (kept when a later
    /// probe has none), whether either is relayed, and for a relayed pair the round trip
    /// through the relay next to the least a direct one could take.
    #[test]
    fn a_guest_and_host_path_says_what_the_relay_costs() {
        let mut t = table(RelayMode::Auto);
        let now = Instant::now();
        let (guest, host) = (a("203.0.113.4:61000"), a("198.51.100.7:13000"));
        t.probe(guest, 0, 1, None, "Tui", now);
        t.note_rtt("Tui", Some(38));
        t.probe(host, probe_flags::HAS_MAPPING, 1, Some(host), "Kiwi", now);
        t.note_rtt("kiwi", Some(41));
        // A probe without one (a hook from before) keeps what was measured.
        t.probe(guest, 0, 2, None, "Tui", now);
        t.note_rtt("Tui", None);

        let path = t.path("tui", "Kiwi").unwrap();
        assert_eq!((path.ping_ms, path.host_ping_ms, path.relayed), (Some(38), Some(41), true));
        // Auckland to Wellington, about 490 km: a direct round trip takes 5 ms at least.
        let km = |a: Ipv4Addr, b: Ipv4Addr| {
            assert_eq!((a, b), (*guest.ip(), *host.ip()));
            Some(490.0)
        };
        assert_eq!(
            path.detail(km),
            serde_json::json!({ "relayed": true, "ping_ms": 38, "host_ping_ms": 41, "relay_ms": 79, "direct_ms": 5 })
        );
        // Not placed: no direct estimate. Under one roof: none needed to say it's 0.
        assert!(path.detail(|_, _| None).get("direct_ms").is_none());
        t.probe(a("203.0.113.4:61001"), 0, 1, None, "Flatmate", now);
        assert_eq!(t.path("Flatmate", "Tui").unwrap().detail(|_, _| None)["direct_ms"], 0);

        // Both direct: only the pings.
        let mut d = table(RelayMode::Off);
        d.probe(guest, 0, 1, None, "Tui", now);
        d.note_rtt("Tui", Some(38));
        d.probe(host, 0, 1, None, "Kiwi", now);
        assert_eq!(d.path("Tui", "Kiwi").unwrap().detail(km), serde_json::json!({ "relayed": false, "ping_ms": 38 }));
        // A guest the helper doesn't know: nothing to say.
        assert!(d.path("Nobody", "Kiwi").is_none());
    }

    #[test]
    fn a_router_port_mapping_is_advertised_as_it_is() {
        let mut t = table(RelayMode::Off);
        let reply = t.probe(a("198.51.100.7:61000"), probe_flags::HAS_MAPPING, 1, Some(a("198.51.100.7:13000")), "sam", Instant::now());
        assert_eq!(advertise(&reply), (a("198.51.100.7:13000"), false));
        // A mapping that is itself private (double NAT) is useless.
        let reply = t.probe(a("198.51.100.8:61000"), probe_flags::HAS_MAPPING, 1, Some(a("10.0.0.2:13000")), "fisher", Instant::now());
        assert_eq!(advertise(&reply), (a("198.51.100.8:61000"), false));
    }

    #[test]
    fn a_mapping_counts_only_on_the_probes_own_address() {
        let mut t = table(RelayMode::Off);
        let now = Instant::now();
        let (victim, _) = advertise(&t.probe(a("198.51.100.7:61000"), 0, 1, None, "victim", now));
        // A mapping on someone else's address is ignored.
        let reply = t.probe(a("203.0.113.4:5000"), probe_flags::HAS_MAPPING, 1, Some(victim), "thief", now);
        assert_eq!(advertise(&reply), (a("203.0.113.4:5000"), false));
        // Behind the same router, another player's advertised address stays theirs.
        let reply = t.probe(a("198.51.100.7:5001"), probe_flags::HAS_MAPPING, 1, Some(victim), "flatmate", now);
        assert_eq!(advertise(&reply), (a("198.51.100.7:5001"), false));
        // Packets for the victim's address still reach the victim.
        assert_eq!(t.route(a("203.0.113.4:5000"), victim, 10, now).map(|(to, _)| to), Some(a("198.51.100.7:61000")));
    }

    /// A player whose relayed traffic one way falls from a match's rate to almost nothing is
    /// noticed, once; a quiet player or a slow fall isn't.
    #[test]
    fn falling_relay_traffic_is_noticed() {
        let mut watch = super::RelayWatch::default();
        let flows = |a: (u64, u64), b: (u64, u64)| vec![("a".to_string(), a.0, a.1), ("b".to_string(), b.0, b.1)];
        assert!(watch.tick(&flows((650, 640), (5, 5)), 10.0).is_empty(), "nothing to compare with yet");
        assert!(watch.tick(&flows((600, 400), (3, 0)), 10.0).is_empty(), "lower, not fallen");
        let fell = watch.tick(&flows((610, 40), (0, 0)), 10.0);
        assert_eq!(fell.len(), 1, "{fell:?}");
        assert_eq!((fell[0].0.as_str(), fell[0].1), ("a", "receiving"));
        assert!(watch.tick(&flows((610, 40), (0, 0)), 10.0).is_empty(), "already low");
    }

    #[test]
    fn who_gets_relayed() {
        let now = Instant::now();
        let mut t = table(RelayMode::Auto);
        assert!(advertise(&t.probe(a("198.51.100.7:1"), probe_flags::SYMMETRIC, 1, None, "sym", now)).1);
        assert!(advertise(&t.probe(a("198.51.100.8:1"), probe_flags::WANT_RELAY, 1, None, "asked", now)).1);
        // On the server's own network, with no port mapping.
        assert!(advertise(&t.probe(a("192.168.1.20:13000"), 0, 1, None, "lan", now)).1);
        // No port mapping at all: nobody could start a connection to them.
        assert!(advertise(&t.probe(a("198.51.100.9:1"), 0, 1, None, "cone", now)).1);
        // A router port mapping: reachable as it is, so direct.
        assert!(!advertise(&t.probe(a("198.51.100.10:13000"), probe_flags::HAS_MAPPING, 1, Some(a("198.51.100.10:13000")), "mapped", now)).1);

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
        // Direct (a router mapping), then the router maps another port: the advertised
        // address stays.
        let (direct, _) = advertise(&t.probe(a("203.0.113.4:61000"), probe_flags::HAS_MAPPING, 1, Some(a("203.0.113.4:61000")), "d", now));
        let later = t.probe(a("203.0.113.4:61000"), probe_flags::HAS_MAPPING, 2, Some(a("203.0.113.4:13000")), "d", now);
        assert_eq!(advertise(&later), (direct, false));
        // Its NAT gave it a new mapping: that's where it's reachable now.
        let moved = t.probe(a("203.0.113.4:61555"), probe_flags::HAS_MAPPING, 3, Some(a("203.0.113.4:61555")), "d", now);
        assert_eq!(advertise(&moved), (a("203.0.113.4:61555"), false));
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

    fn tag(m: &Message) -> nat_proto::Tag {
        match m {
            Message::ProbeReply { tag, .. } => *tag,
            _ => panic!("{m:?}"),
        }
    }

    #[test]
    fn relayed_data_needs_the_senders_tag() {
        let now = Instant::now();
        let mut t = table(RelayMode::All);
        let a_reply = t.probe(a("198.51.100.7:1"), 0, 1, None, "a", now);
        let b_reply = t.probe(a("198.51.100.8:1"), 0, 1, None, "b", now);
        let (b_adv, _) = advertise(&b_reply);
        assert_eq!(t.route_tagged(a("198.51.100.7:1"), [0; 8], b_adv, 10, now), None, "no tag");
        assert_eq!(t.route_tagged(a("198.51.100.7:1"), tag(&b_reply), b_adv, 10, now), None, "someone else's tag");
        let (target, _, target_tag) = t.route_tagged(a("198.51.100.7:1"), tag(&a_reply), b_adv, 10, now).unwrap();
        assert_eq!((target, target_tag), (a("198.51.100.8:1"), tag(&b_reply)), "delivered with the receiver's tag");
        // A refresh keeps the tag (the game relays with it).
        assert_eq!(tag(&t.probe(a("198.51.100.7:1"), 0, 2, None, "a", now)), tag(&a_reply));
    }

    /// PlaySkill's case (eu1, 2026-10-06): registered from one port, the match's traffic from
    /// another (the game's other Storm socket, or a NAT giving each its own port).
    #[test]
    fn relayed_data_from_another_port_of_the_player_is_theirs() {
        let now = Instant::now();
        let mut t = table(RelayMode::All);
        let ps = t.probe(a("176.0.198.61:47868"), 0, 1, None, "PlaySkill", now);
        let th = t.probe(a("177.23.180.245:42401"), 0, 1, None, "Theusma01", now);
        let ((ps_adv, _), (th_adv, _)) = (advertise(&ps), advertise(&th));
        // Someone else behind the same carrier address, without the tag: nothing.
        assert_eq!(t.route_tagged(a("176.0.198.61:5555"), [0; 8], th_adv, 10, now), None);
        assert_eq!(t.route_tagged(a("176.0.198.61:5555"), tag(&th), th_adv, 10, now), None, "another player's tag");
        // Their tag from another port of their address: theirs, and delivered as from them.
        let (target, from, _) = t.route_tagged(a("176.0.198.61:47861"), tag(&ps), th_adv, 10, now).unwrap();
        assert_eq!((target, from), (a("177.23.180.245:42401"), ps_adv));
        assert_eq!(t.who_sent(a("176.0.198.61:47861")), "playskill");
        // The answer goes back to the socket that sent, not the one that probes.
        let (back, _, _) = t.route_tagged(a("177.23.180.245:42401"), tag(&th), ps_adv, 10, now).unwrap();
        assert_eq!(back, a("176.0.198.61:47861"));
        // A keepalive from the probing socket keeps that port theirs.
        t.probe(a("176.0.198.61:47868"), 0, 2, None, "PlaySkill", now);
        assert!(t.route_tagged(a("176.0.198.61:47861"), tag(&ps), th_adv, 10, now).is_some());
        // Quiet a while: back to the probing socket.
        let later = now + DATA_FROM_FRESH + Duration::from_secs(1);
        let (back, _, _) = t.route_tagged(a("177.23.180.245:42401"), tag(&th), ps_adv, 10, later).unwrap();
        assert_eq!(back, a("176.0.198.61:47868"));
        // At most a few ports each; the oldest goes.
        for port in 1..=MAX_ALIASES as u16 {
            assert!(t.route_tagged(a(&format!("176.0.198.61:{port}")), tag(&ps), th_adv, 10, later).is_some());
        }
        assert_eq!(t.who_sent(a("176.0.198.61:47861")), "an unregistered port of playskill (registered from port 47868)");
    }

    #[test]
    fn registrations_are_capped_per_address() {
        let now = Instant::now();
        let mut t = table(RelayMode::Off);
        for i in 0..MAX_PEERS_PER_IP {
            let port = u16::try_from(i + 1).unwrap();
            assert!(t.admits(SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 7), port), &format!("p{i}")));
            t.probe(SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 7), port), 0, 1, None, &format!("p{i}"), now);
        }
        assert!(!t.admits(a("198.51.100.7:9999"), "one-more"), "the address is full");
        assert!(t.admits(a("198.51.100.7:9999"), "p0"), "a refresh always may");
        assert!(t.admits(a("198.51.100.8:1"), "elsewhere"));
    }

    #[test]
    fn packets_count_against_the_rate_even_when_empty() {
        let now = Instant::now();
        let mut t = table(RelayMode::All);
        t.probe(a("198.51.100.7:1"), 0, 1, None, "a", now);
        let (b_adv, _) = advertise(&t.probe(a("198.51.100.8:1"), 0, 1, None, "b", now));
        let sent = (0..2000).filter(|_| t.route(a("198.51.100.7:1"), b_adv, 0, now).is_some()).count();
        assert!(sent <= MAX_PACKETS_PER_SECOND as usize, "{sent} empty packets in a second");
    }

    #[test]
    fn tickets_name_one_account_and_cookies_one_address() {
        let _ = KEYS.set(Keys { ticket: [1; 32], cookie: [2; 32] });
        assert_eq!(ticket_for("Kiwi"), ticket_for("kiwi"), "any case");
        assert_ne!(ticket_for("Kiwi"), ticket_for("Tank"));
        let keys = KEYS.get().unwrap();
        // Good for a week from the day it's issued, then not.
        let day = 20_000;
        let t = ticket_on(keys, "Kiwi", day);
        assert!(ticket_valid(keys, "kiwi", &t, day) && ticket_valid(keys, "Kiwi", &t, day + TICKET_DAYS - 1));
        assert!(!ticket_valid(keys, "Kiwi", &t, day + TICKET_DAYS), "expired");
        assert!(!ticket_valid(keys, "Kiwi", &t, day - 1), "not issued yet");
        assert!(!ticket_valid(keys, "Tank", &t, day), "another name");
        let mut later = t;
        later[..2].copy_from_slice(&((day + 3) as u16).to_be_bytes());
        assert!(!ticket_valid(keys, "Kiwi", &later, day + 3), "the day can't be changed");
        // Across the 16-bit wrap of the day number.
        let wrap = 65_535 + 65_536;
        assert!(ticket_valid(keys, "Kiwi", &ticket_on(keys, "Kiwi", wrap), wrap + 2));
        let c = cookie_for(keys, a("198.51.100.7:1"), "kiwi", 5);
        assert_ne!(c, cookie_for(keys, a("198.51.100.7:2"), "kiwi", 5), "another address");
        assert_ne!(c, cookie_for(keys, a("198.51.100.7:1"), "tank", 5), "another name");
        assert_ne!(c, cookie_for(keys, a("198.51.100.7:1"), "kiwi", 6), "another minute");
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
    fn a_game_going_offline_is_forgotten_at_once() {
        let now = Instant::now();
        let mut t = table(RelayMode::All);
        let Message::ProbeReply { tag, .. } = t.probe(a("198.51.100.7:1"), 0, 1, None, "x", now) else {
            panic!()
        };
        assert_ne!(tag, [0; 8]);
        assert_eq!(t.bye(a("198.51.100.7:1"), [9; 8]), None, "someone else's tag");
        assert_eq!(t.bye(a("198.51.100.7:2"), tag), None, "another address");
        assert_eq!(t.len(), 1);
        let (gone, _) = advertise(&t.probe(a("198.51.100.8:1"), 0, 1, None, "y", now));
        assert_eq!(t.bye(a("198.51.100.7:1"), tag).as_deref(), Some("x"));
        assert_eq!(t.len(), 1);
        assert!(t.expiring(now + EXPIRY + Duration::from_secs(1)).iter().all(|(n, _)| n != "x"), "so it never lapses");
        // Its relay port isn't handed to the next player at once: its peers may still send.
        let Message::ProbeReply { tag: y_tag, .. } = t.probe(a("198.51.100.8:1"), 0, 2, None, "y", now) else {
            panic!()
        };
        let x_port = {
            let mut t2 = table(RelayMode::All);
            advertise(&t2.probe(a("198.51.100.7:1"), 0, 1, None, "x", now)).0
        };
        let (next, _) = advertise(&t.probe(a("198.51.100.9:1"), 0, 1, None, "z", now));
        assert_ne!(next, x_port, "the departed player's relay port waits");
        assert_ne!(next, gone);
        let _ = y_tag;
    }

    #[test]
    fn the_one_player_at_an_address_is_found_by_it() {
        let mut t = table(RelayMode::Auto);
        let now = Instant::now();
        let ip = Ipv4Addr::new(198, 51, 100, 7);
        assert_eq!(t.advertised_for_ip(ip, now), None);
        // First a direct registration (a router mapping): not final until it has stood a while.
        let (direct, _) = advertise(&t.probe(a("198.51.100.7:25676"), probe_flags::HAS_MAPPING, 1, Some(a("198.51.100.7:25676")), "Solo", now));
        assert_eq!(t.advertised_for_ip(ip, now), None, "the player may still move to the relay");
        assert_eq!(t.advertised_for_ip(ip, now + SETTLE), Some(direct));
        // Then its hook asks for the relay: final at once.
        let (relay, relayed) = advertise(&t.probe(a("198.51.100.7:25676"), nat_proto::probe_flags::WANT_RELAY, 2, None, "Solo", now));
        assert!(relayed);
        assert_eq!(t.advertised_for_ip(ip, now), Some(relay), "a relayed player is reached on the relay");
        t.probe(a("198.51.100.7:51000"), 0, 3, None, "Flatmate", now);
        assert_eq!(t.advertised_for_ip(ip, now + SETTLE), None, "two players behind one address can't be told apart");
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
        // The game already advertises that address, or also sent a local one.
        let ok = vec!["prudp:/address=198.51.100.7;port=61000;RVCID=5;hdrType=0;type=2".to_string()];
        assert_eq!(urls_with_public_address(ok.clone(), public), ok);
        // A public address the helper didn't check (a port only the server can reach, behind
        // a VPN's NAT) gives way to the checked one - here the relay's - with no local URL
        // made of it.
        let relay = a("139.99.171.113:40000");
        let vpn = vec![
            "prudp:/address=187.15.120.80;port=29462;RVCID=5;hdrType=0;type=2".to_string(),
            "prudp:/address=10.5.0.2;port=13000;RVCID=5;hdrType=0".to_string(),
        ];
        assert_eq!(
            urls_with_public_address(vpn, relay),
            [
                "prudp:/address=139.99.171.113;port=40000;RVCID=5;hdrType=0;type=2",
                "prudp:/address=10.5.0.2;port=13000;RVCID=5;hdrType=0"
            ]
        );
        let alone = vec!["prudp:/address=187.15.120.80;port=29462;RVCID=5;hdrType=0;type=2".to_string()];
        assert_eq!(
            urls_with_public_address(alone, relay),
            ["prudp:/address=139.99.171.113;port=40000;RVCID=5;hdrType=0;type=2"]
        );
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
