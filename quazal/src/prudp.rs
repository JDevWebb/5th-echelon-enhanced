/// This module handles the PRUDP protocol, which is a custom reliable UDP protocol.
pub mod packet;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::net::UdpSocket;
use std::net::{self};
use std::sync::atomic::AtomicU32;
use std::time::Duration;
use std::time::Instant;

use slog::debug;
use slog::error;
use slog::o;
use slog::Logger;

use self::packet::crypt_key;
use self::packet::PacketFlag;
use self::packet::PacketType;
use self::packet::QPacket;
use self::packet::StreamHandler;
use self::packet::StreamHandlerRegistry;
use self::packet::VPort;
use crate::kerberos::KerberosTicketInternal;
use crate::rmc::basic::ReadStream;
use crate::rmc::basic::ToStream;
use crate::ClientInfo;
use crate::ConnectionID;
use crate::Context;
use crate::Signature;

const MAX_PAYLOAD_SIZE: usize = 1000;
const SESSION_TIMEOUT: Duration = Duration::from_secs(60);
/// How often unacknowledged packets are checked for resending.
const RESEND_TICK: Duration = Duration::from_millis(200);
/// First resend after this; each further one waits twice as long.
const RESEND_AFTER: Duration = Duration::from_millis(500);
/// Resends before giving up on a packet (0.5 + 1 + 2 + 4 s).
const RESEND_TRIES: u32 = 4;
/// How many handled sequence numbers are remembered per client.
const HANDLED_MEMORY: usize = 64;
/// Most unacknowledged packets kept per client.
const UNACKED_MAX: usize = 256;
/// Fragments of one message a client may send, and their total size.
const MAX_FRAGMENTS: usize = 32;
const MAX_REASSEMBLED: usize = 64 * 1024;
/// Connections in all (per address: [`max_connections_per_ip`]).
const MAX_CONNECTIONS: usize = 16384;
/// A connection that has neither signed in nor sent anything since its
/// CONNECT is dropped after this long without a packet (others after
/// [`SESSION_TIMEOUT`]): a game always says something right away.
const SILENT_TIMEOUT: Duration = Duration::from_secs(10);
/// How many signatures one address and session can be offered (see
/// [`Server::handle_syn`]); they are told apart by two bits of the signature.
const COOKIE_GENERATIONS: u32 = 4;
/// The address echo (user packets): its largest payload, how often per address,
/// and how many addresses are tracked at once.
const MAX_ECHO_PAYLOAD: usize = 64;
const ECHOES_PER_SECOND: u32 = 5;
const MAX_ECHO_SOURCES: usize = 10_000;
/// How long after a connection's last packet a repeated SYN or CONNECT for it
/// is still answered as a repeat.
const REPEAT_WINDOW: Duration = Duration::from_secs(5);

/// Connections one address may hold: 256 (a LAN party behind one router), or
/// `FE_MAX_CONNECTIONS_PER_IP` (the load test's players all share one).
fn max_connections_per_ip() -> usize {
    static MAX: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| std::env::var("FE_MAX_CONNECTIONS_PER_IP").ok().and_then(|v| v.parse().ok()).unwrap_or(256))
}

/// An address as 16 bytes (IPv4 as IPv4-mapped IPv6), the same for both ways of
/// writing an IPv4 address.
pub(crate) fn address_bytes(ip: std::net::IpAddr) -> [u8; 16] {
    match ip.to_canonical() {
        std::net::IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
        std::net::IpAddr::V6(v6) => v6.octets(),
    }
}

/// Records that a client's packet with this sequence number is being handled.
fn remember_handled<T>(ci: &mut ClientInfo<T>, sequence: u16) {
    if ci.handled.len() >= HANDLED_MEMORY {
        ci.handled.pop_front();
    }
    ci.handled.push_back((sequence, vec![]));
}

/// The secret behind handshake signatures (SYN cookies), new at every start.
///
/// A SYN keeps no state: its answer's signature is a MAC of the client's address and
/// session, so only a CONNECT from that address can show it, and only then is a connection
/// made. A table of half-open handshakes, one per SYN, could be filled by anyone forging
/// source addresses, and then nobody could sign in.
struct Cookies {
    key: [u8; 32],
}

impl Cookies {
    fn new() -> Self {
        Self { key: rand::random() }
    }

    /// The signature for `client`'s handshake with this session: the session in the low
    /// byte, the generation in the next two bits, and 22 bits of MAC above them. The
    /// session travels in the signature itself, so checking it needs nothing from the
    /// CONNECT but the signature it carries back.
    fn signature(&self, client: SocketAddr, session: u8, generation: u32) -> u32 {
        use hmac::Mac as _;
        let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(&self.key).expect("any key length");
        m.update(&address_bytes(client.ip()));
        m.update(&client.port().to_be_bytes());
        m.update(&[session, generation as u8]);
        let mac = m.finalize().into_bytes();
        let mac = u32::from_be_bytes([mac[0], mac[1], mac[2], mac[3]]);
        (mac & !0x3ff) | ((generation & 0x3) << 8) | u32::from(session)
    }

    /// Whether `signature` is one this server gave `client` in answer to a SYN.
    fn valid(&self, client: SocketAddr, signature: u32) -> bool {
        #[allow(clippy::cast_possible_truncation)]
        let session = signature as u8;
        let generation = (signature >> 8) & 0x3;
        self.signature(client, session, generation) == signature
    }
}

/// At most one log line a second for what anyone can cause (junk, forged or
/// stray packets); the rest are counted and reported with the next one.
#[derive(Default)]
struct Throttle {
    last: Option<Instant>,
    skipped: u64,
}

impl Throttle {
    /// `Some(lines skipped since the last one)` when a line may be logged now.
    fn allow(&mut self) -> Option<u64> {
        let now = Instant::now();
        if self.last.is_some_and(|last| now.duration_since(last) < Duration::from_secs(1)) {
            self.skipped += 1;
            return None;
        }
        self.last = Some(now);
        Some(std::mem::take(&mut self.skipped))
    }
}

/// A registry for clients.
#[derive(Default)]
pub struct ClientRegistry<T> {
    clients: HashMap<u32, RefCell<ClientInfo<T>>>,
    connection_id_session_ids: HashMap<ConnectionID, Signature>,
    /// Connections per address, kept with `clients` (counting them took a scan of every
    /// connection for each SYN).
    per_ip: HashMap<std::net::IpAddr, usize>,
}

impl<T> ClientRegistry<T> {
    /// Adds a new connection under its signature.
    fn insert(&mut self, signature: u32, ci: ClientInfo<T>) {
        *self.per_ip.entry(ci.address().ip()).or_default() += 1;
        self.clients.insert(signature, RefCell::new(ci));
    }

    /// Connections from `ip`.
    fn connections_from(&self, ip: std::net::IpAddr) -> usize {
        self.per_ip.get(&ip).copied().unwrap_or(0)
    }

    /// Forgets a client that is gone (already taken out of `clients`). Returns whether its
    /// user's per-user state may be cleaned up: not if the same user has another live
    /// connection (they reconnected before the old one expired), whose station URLs and
    /// lobby would otherwise be wiped.
    fn forget(&mut self, ci: &ClientInfo<T>) -> bool {
        if let Some(conn_id) = ci.connection_id {
            self.connection_id_session_ids.remove(&conn_id);
        }
        if let std::collections::hash_map::Entry::Occupied(mut n) = self.per_ip.entry(ci.address().ip()) {
            *n.get_mut() -= 1;
            if *n.get() == 0 {
                n.remove();
            }
        }
        match ci.user_id {
            Some(uid) => !self.clients.values().any(|c| c.try_borrow().map_or(true, |c| c.user_id == Some(uid))),
            None => true,
        }
    }

    /// Returns a client by its connection ID.
    #[must_use]
    pub fn client_by_connection_id(&self, conn_id: ConnectionID) -> Option<&RefCell<ClientInfo<T>>> {
        self.connection_id_session_ids.get(&conn_id).and_then(|sig| self.clients.get(&sig.0))
    }

    /// Finds a logged-in client by its player id.
    ///
    /// Needed to send something to a *different* client on our own initiative: whoever adds a
    /// participant has to be able to notify the one being added, and that is not the caller.
    #[must_use]
    pub fn client_by_user_id(&self, user_id: u32) -> Option<&RefCell<ClientInfo<T>>> {
        // `try_borrow`, not `borrow`: the caller's cell is mutably borrowed for the duration
        // of their own request, and the search unavoidably passes over it. `borrow()` panics
        // the service thread. An already borrowed cell is always the caller itself, and the
        // caller is never the one to notify anyway.
        //
        // Take the MOST RECENTLY SEEN stream, not just any. A player holds several PRUDP
        // streams to the same service, each with its own session id, sequence counter and
        // `ClientInfo`. Picking an arbitrary HashMap hit lands on the wrong one about half the
        // time: the client acknowledges the packet (the address matches) and then drops it,
        // because session and sequence do not fit - it never reaches the RMC dispatcher. The
        // RMC stream is the busy one, hence the most recently seen.
        self.clients
            .values()
            .filter(|c| c.try_borrow().is_ok_and(|ci| ci.user_id == Some(user_id)))
            .max_by_key(|c| c.borrow().last_seen)
    }
}

/// A PRUDP server.
pub struct Server<'a, ECH, DH, T = ()>
where
    ECH: FnMut(ClientInfo<T>),
    DH: FnMut(ClientInfo<T>),
{
    logger: slog::Logger,
    registry: StreamHandlerRegistry<T>,
    socket: Option<net::UdpSocket>,
    ctx: &'a Context,
    cookies: Cookies,
    client_registry: ClientRegistry<T>,
    /// A handler for user-defined packets.
    pub user_handler: Option<fn(logger: &Logger, packet: QPacket, client: SocketAddr, sock: &net::UdpSocket)>,
    /// A handler for expired clients.
    pub expired_client_handler: Option<ECH>,
    /// A handler for disconnected clients.
    pub disconnect_handler: Option<DH>,
    /// Called with the user id once a connection has proved its ticket: the
    /// user is signed in here (until their last connection closes or expires).
    /// Also given the address the connection came from.
    pub login_handler: Option<Box<dyn FnMut(u32, SocketAddr) + 'a>>,
    /// Whether a user's newest sign-in closes their connections from elsewhere (see
    /// [`Self::sign_out_elsewhere`]): every user by default. Shared accounts (the server's
    /// own, whose passwords are public) must not.
    pub newest_sign_in_wins: fn(u32) -> bool,
    /// Users to sign out now, from every address (an admin's kick or ban); read once a
    /// second.
    pub sign_outs: Option<std::sync::mpsc::Receiver<u32>>,
    next_conn_id: AtomicU32,
    /// Address echoes answered per source this second, and when they were last swept.
    echoes: HashMap<std::net::IpAddr, (Instant, u32)>,
    echo_sweep: Instant,
    /// Log lines for what anyone can send.
    noise: Throttle,
}

impl<ECH, DH, T> Server<'_, ECH, DH, T>
where
    T: Default,
    ECH: FnMut(ClientInfo<T>),
    DH: FnMut(ClientInfo<T>),
{
    /// Creates a new PRUDP server.
    #[must_use]
    pub fn new(logger: slog::Logger, ctx: &Context, registry: StreamHandlerRegistry<T>) -> Server<'_, ECH, DH, T> {
        Server {
            logger,
            registry,
            socket: None,
            ctx,
            cookies: Cookies::new(),
            echoes: HashMap::default(),
            echo_sweep: Instant::now(),
            noise: Throttle::default(),
            client_registry: ClientRegistry::default(),
            user_handler: None,
            expired_client_handler: None,
            disconnect_handler: None,
            login_handler: None,
            newest_sign_in_wins: |_| true,
            sign_outs: None,
            next_conn_id: AtomicU32::new(0x3AAA_AAAA),
        }
    }

    /// Registers a stream handler for a virtual port.
    pub fn register(&mut self, vport: VPort, protocol: Box<dyn StreamHandler<T>>) {
        self.registry.register(vport, protocol);
    }

    /// Binds the server to a socket address.
    pub fn bind<A: net::ToSocketAddrs>(&mut self, addrs: A) -> io::Result<()> {
        self.socket = Some(net::UdpSocket::bind(addrs)?);
        info!(self.logger, "Listening on {}", self.socket.as_ref().unwrap().local_addr().unwrap());
        Ok(())
    }

    /// Starts the server's main loop.
    pub fn serve(mut self) {
        let socket = self.socket.as_ref().expect("UDP socket required").try_clone().expect("Couldn't clone socket");
        socket.set_read_timeout(Some(RESEND_TICK)).expect("error setting read timeout");
        let mut buf = vec![0u8; 1024];
        let mut last_sweep = Instant::now();
        let mut last_resend = Instant::now();
        'outer: loop {
            if last_resend.elapsed() >= RESEND_TICK {
                self.resend_unacked();
                last_resend = Instant::now();
            }
            // Expire dead clients at least once a second. Upstream only did it
            // after a second without any packet, so on a busy server they
            // never expired.
            if last_sweep.elapsed() >= Duration::from_secs(1) {
                self.clear_clients();
                let wanted: Vec<u32> = self.sign_outs.as_ref().map(|r| r.try_iter().collect()).unwrap_or_default();
                for user_id in wanted {
                    let logger = self.logger.clone();
                    self.sign_out(&logger, user_id, None);
                }
                last_sweep = Instant::now();
            }
            let (nread, client) = match socket.recv_from(&mut buf) {
                Ok(x) => x,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock {
                        // Nothing to do: the sweep above runs on the next pass.
                    } else {
                        error!(self.logger, "recv_from failed: {}", e);
                    }
                    continue;
                }
            };
            let logger = self.logger.new(o!("client" => client));
            let mut data = &buf[..nread];

            while !data.is_empty() {
                let (packet, nparsed) = match QPacket::from_bytes(self.ctx, data) {
                    Ok(p) => p,
                    Err(e) => {
                        if let Some(skipped) = self.noise.allow() {
                            warn!(logger, "Invalid packet received"; "error" => %e, "similar_skipped" => skipped);
                        }
                        continue 'outer;
                    }
                };
                #[allow(clippy::cast_possible_truncation)]
                let (packet_data, next_data) = data.split_at(nparsed as usize);
                data = next_data;
                trace!(logger, "-> {:02x?}", packet_data);

                if let Err(e) = packet.validate(self.ctx, packet_data) {
                    if let Some(skipped) = self.noise.allow() {
                        warn!(logger, "Invalid packet received: {:?}", packet; "error" => %e, "similar_skipped" => skipped);
                    }
                    continue;
                }

                let logger = logger.new(o!("seq" => packet.sequence, "session" => packet.session_id));
                // A bug reachable from one client's packet must not take down
                // the service for everyone (the panic hook has logged it).
                let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle_packet(&logger, packet, client)));
                if handled.is_err() {
                    error!(logger, "packet handler panicked; packet dropped");
                }
            }
        }
    }

    /// Handles a received packet.
    fn handle_packet(&mut self, logger: &Logger, packet: QPacket, client: SocketAddr) {
        debug!(logger, "packet: {:?}", packet);
        // An established connection only answers its own address: the signature alone is
        // no proof of who sent a packet (UDP sources can be forged), and replies go to the
        // source.
        if !matches!(packet.packet_type, PacketType::Syn | PacketType::Connect) {
            if let Some(ci) = self.client_registry.clients.get(&packet.signature) {
                if ci.try_borrow().is_ok_and(|ci| *ci.address() != client) {
                    debug!(logger, "Packet for connection {:x} from another address; dropped", packet.signature);
                    return;
                }
            }
        }
        if packet.flags.contains(PacketFlag::Ack) {
            debug!(logger, "Received ACK"; "sequence" => packet.sequence);
            // The client has our reliable packet: stop resending it.
            if packet.packet_type == PacketType::Data {
                if let Some(ci) = self.client_registry.clients.get(&packet.signature) {
                    if let Ok(mut ci) = ci.try_borrow_mut() {
                        ci.unacked.remove(&packet.sequence);
                    }
                }
            }
            return;
        }
        match packet.packet_type {
            PacketType::Syn => self.handle_syn(logger, packet, client),
            PacketType::Connect => self.handle_connect(logger, packet, client),
            PacketType::Data => self.handle_data(logger, packet, client),
            PacketType::Disconnect => {
                let Some(ci) = self.client_registry.clients.remove(&packet.signature) else {
                    return;
                };
                info!(logger, "Client disconnected"; "signature" => packet.signature, "session" => packet.session_id);
                if self.send_ack(logger, &client, &packet, &ci.borrow(), false).is_err() {
                    // ignore
                }
                let ci = ci.into_inner();
                if !self.client_registry.forget(&ci) {
                    info!(logger, "User {:?} is still connected; keeping their state", ci.user_id);
                } else if let Some(handler) = self.disconnect_handler.as_mut() {
                    (handler)(ci);
                }
            }
            PacketType::Ping => {
                let Some(ci) = self.client_registry.clients.get(&packet.signature) else {
                    return;
                };
                ci.borrow_mut().seen();
                if self.send_ack(logger, &client, &packet, &ci.borrow(), false).is_err() {
                    // ignore
                }
            }
            PacketType::User => {
                // The address echo answers whatever source a packet claims, so it's kept
                // small and slow: short payloads only, a few a second per address.
                if packet.payload.len() > MAX_ECHO_PAYLOAD || !self.echo_allowed(client.ip()) {
                    debug!(logger, "User packet dropped (too large or too many)");
                    return;
                }
                if self.user_handler.is_none() {
                    if let Some(skipped) = self.noise.allow() {
                        warn!(logger, "unsupported user packet"; "similar_skipped" => skipped);
                    }
                } else {
                    (self.user_handler.as_ref().unwrap())(logger, packet, client, self.socket.as_ref().unwrap());
                }
            }
            PacketType::Route | PacketType::Raw => {
                if let Some(skipped) = self.noise.allow() {
                    warn!(logger, "unsupported packet type {:?}", packet.packet_type; "similar_skipped" => skipped);
                }
            }
        }
    }

    /// Whether `ip` may have another address echo now (at most
    /// [`ECHOES_PER_SECOND`]).
    ///
    /// An address already counted is checked on its own entry. Only a new one may need
    /// room, and old entries are swept at most once a second: sweeping on every packet cost
    /// a pass over all [`MAX_ECHO_SOURCES`] for each forged source. While the table is full,
    /// new addresses wait for the next sweep.
    fn echo_allowed(&mut self, ip: std::net::IpAddr) -> bool {
        let now = Instant::now();
        if let Some((since, count)) = self.echoes.get_mut(&ip) {
            if now.duration_since(*since) >= Duration::from_secs(1) {
                *since = now;
                *count = 0;
            }
            *count += 1;
            return *count <= ECHOES_PER_SECOND;
        }
        if self.echoes.len() >= MAX_ECHO_SOURCES && now.duration_since(self.echo_sweep) >= Duration::from_secs(1) {
            self.echoes.retain(|_, (since, _)| now.duration_since(*since) < Duration::from_secs(1));
            self.echo_sweep = now;
        }
        if self.echoes.len() >= MAX_ECHO_SOURCES {
            return false;
        }
        self.echoes.insert(ip, (now, 1));
        true
    }

    /// Handles a data packet.
    fn handle_data(&mut self, logger: &Logger, packet: QPacket, client: SocketAddr) {
        #![allow(clippy::cast_possible_truncation)]

        debug!(logger, "Handling data packet");
        let Some(ci) = self.client_registry.clients.get(&packet.signature) else {
            if let Some(skipped) = self.noise.allow() {
                warn!(logger, "client is unknown!"; "similar_skipped" => skipped);
            }
            return;
        };
        let logger = logger.new(o!("pid" => ci.borrow().user_id));
        let ci = &mut ci.borrow_mut();
        ci.seen();
        if let Err(e) = self.send_ack(&logger, &client, &packet, &*ci, false) {
            error!(logger, "Error sending ack"; "error" => %e);
        } else {
            debug!(logger, "Send ack");
        }
        // A retransmission (our acknowledgement was lost): answer it again, but
        // don't handle it twice (a repeated CreateSession made a second lobby).
        if let Some((_, replies)) = ci.handled.iter().find(|(seq, _)| *seq == packet.sequence) {
            info!(logger, "Duplicate packet {}; resending {} replies", packet.sequence, replies.len());
            for data in replies.clone() {
                if let Err(e) = self.socket.as_ref().unwrap().send_to(&data, client) {
                    error!(logger, "Error resending reply"; "error" => %e);
                }
            }
            return;
        }
        remember_handled(ci, packet.sequence);
        let payload = if let Some(fid) = packet.fragment_id {
            if fid != 0 {
                // Bounded per connection: fragments are kept until the last one comes.
                let cached: usize = ci.packet_fragments.values().map(Vec::len).sum();
                if ci.packet_fragments.len() >= MAX_FRAGMENTS || cached + packet.payload.len() > MAX_REASSEMBLED {
                    warn!(logger, "Too many or too large fragments; dropping them");
                    ci.packet_fragments.clear();
                    return;
                }
                debug!(logger, "Caching fragment {}", fid);
                ci.packet_fragments.insert(fid, packet.payload);
                return;
            }
            let mut payload = vec![];
            if !ci.packet_fragments.is_empty() {
                for fid in 1..=ci.packet_fragments.len() as u8 {
                    let f = match ci.packet_fragments.get(&fid) {
                        None => {
                            error!(logger, "missing fragment {}", fid);
                            ci.packet_fragments.clear();
                            return;
                        }
                        Some(f) => f,
                    };
                    payload.extend(f.iter());
                }
                info!(logger, "Reassembled {} fragments", ci.packet_fragments.len() + 1);
                ci.packet_fragments.clear();
            }
            payload.extend(packet.payload);
            payload
        } else {
            packet.payload
        };
        ci.replying = Some(vec![]);
        let resp = self
            .registry
            .handle_packet(&logger, self.ctx, ci, &packet.destination, &payload, &self.client_registry, self.socket.as_ref().unwrap());
        ci.last_vports = Some((packet.source, packet.destination));
        match resp {
            Some(Ok(payload)) => {
                let chunks = payload.chunks(MAX_PAYLOAD_SIZE);
                for (fid, chunk) in (0..chunks.len()).rev().zip(chunks) {
                    let resp = QPacket {
                        source: packet.destination,
                        destination: packet.source,
                        packet_type: PacketType::Data,
                        payload: chunk.to_vec(),
                        fragment_id: Some(fid as u8),
                        ..Default::default()
                    };
                    if let Err(e) = self.send_response(&logger, &client, resp, ci) {
                        error!(logger, "Error sending response"; "error" => %e);
                    } else {
                        trace!(logger, "Send response");
                    }
                }
            }
            None => {
                error!(logger, "No handler found");
            }
            Some(Err(_)) => {
                error!(logger, "Handler failed");
            }
        }
        // Keep the replies for this sequence number, for a retransmission.
        let replies = ci.replying.take().unwrap_or_default();
        if let Some(entry) = ci.handled.iter_mut().find(|(seq, _)| *seq == packet.sequence) {
            entry.1 = replies;
        }
    }

    /// Resends reliable packets clients haven't acknowledged, with backoff,
    /// and gives up after a few tries (a lost "come in" push used to leave a
    /// player waiting for good).
    fn resend_unacked(&mut self) {
        let now = Instant::now();
        let socket = self.socket.as_ref().unwrap();
        for ci in self.client_registry.clients.values() {
            let Ok(mut ci) = ci.try_borrow_mut() else { continue };
            let addr = *ci.address();
            let mut give_up = vec![];
            for (seq, p) in &mut ci.unacked {
                if now.duration_since(p.sent) < RESEND_AFTER * 2u32.pow(p.tries) {
                    continue;
                }
                if p.tries >= RESEND_TRIES {
                    give_up.push(*seq);
                    continue;
                }
                p.tries += 1;
                p.sent = now;
                debug!(self.logger, "Resending unacknowledged packet {seq} to {addr} (try {})", p.tries);
                let _ = socket.send_to(&p.data, addr);
            }
            for seq in give_up {
                warn!(self.logger, "Client {addr} never acknowledged packet {seq}; giving up");
                ci.unacked.remove(&seq);
            }
        }
    }

    /// Handles a SYN packet.
    ///
    /// Nothing is kept for a SYN: its answer carries a signature made from the client's
    /// address and session ([`Cookies`]), and the connection is only made when a CONNECT
    /// from that address brings it back. Forged SYNs therefore cost an answer to the forged
    /// address and nothing else.
    fn handle_syn(&mut self, logger: &Logger, mut packet: QPacket, client: SocketAddr) {
        debug!(logger, "Handling syn packet");
        // The same SYN again: the game sends it again when our answer takes longer than
        // its first resend (a server ~300 ms away), and takes the signature from the last
        // answer it gets. A new signature would leave it talking on a handshake that never
        // finished ("client is unknown"), so it gets the one it already has: the same
        // signature while the handshake is open (the cookie is the same every time), and the
        // connection's own once it is made.
        let signatures: Vec<u32> = (0..COOKIE_GENERATIONS).map(|g| self.cookies.signature(client, packet.session_id, g)).collect();
        if let Some(ci) = signatures
            .iter()
            .find_map(|sig| self.client_registry.clients.get(sig).filter(|ci| self.is_repeat(ci, client, packet.session_id)))
        {
            debug!(logger, "SYN repeated; answering with the same signature");
            let ci = ci.borrow();
            packet.conn_signature = Some(ci.server_signature);
            if let Err(e) = self.send_ack(logger, &client, &packet, &ci, false) {
                error!(logger, "Error sending syn ack packet"; "error" => %e);
            }
            return;
        }
        // A connection from an earlier game on this address can linger for a minute with the
        // signature a new game's handshake would get (a game started again, by chance, with
        // the same session number): the new one gets the next generation's.
        let Some(sig) = signatures.into_iter().find(|sig| !self.client_registry.clients.contains_key(sig)) else {
            if let Some(skipped) = self.noise.allow() {
                warn!(logger, "Refusing a handshake from {client}: its signatures are all in use"; "similar_skipped" => skipped);
            }
            return;
        };
        // Bounded connections, per address and in all (checked again at CONNECT, where
        // they are made; here it saves answering).
        if self.client_registry.connections_from(client.ip()) >= max_connections_per_ip() || self.client_registry.clients.len() >= MAX_CONNECTIONS {
            if let Some(skipped) = self.noise.allow() {
                warn!(logger, "Refusing a handshake from {client}: too many connections"; "similar_skipped" => skipped);
            }
            return;
        }
        packet.conn_signature = Some(sig);
        // Not connected yet: no signature of the client's, and session 0 (what an unconnected
        // `ClientInfo` answered with before).
        if let Err(e) = self.send_ack(logger, &client, &packet, &ClientInfo::<T>::new(client), false) {
            error!(logger, "Error sending syn ack packet"; "error" => %e);
        }
    }

    /// Whether `ci` is the connection a repeated SYN or CONNECT from `client` with this
    /// session belongs to.
    ///
    /// Only while it's fresh (seen in the last few seconds): a game started again could, by
    /// chance, reuse the session number of its last connection, which lingers for a minute,
    /// and needs a handshake of its own.
    fn is_repeat(&self, ci: &RefCell<ClientInfo<T>>, client: SocketAddr, session: u8) -> bool {
        ci.try_borrow()
            .is_ok_and(|ci| *ci.address() == client && ci.client_session == session && ci.last_seen.elapsed() < REPEAT_WINDOW)
    }

    /// Handles a CONNECT packet.
    fn handle_connect(&mut self, logger: &Logger, mut packet: QPacket, client: SocketAddr) {
        debug!(logger, "Handling connect packet");
        let Some(signature) = packet.conn_signature else {
            debug!(logger, "Client {:x} did not provide a connection signature. This should not happen", packet.signature);
            return;
        };

        // The same CONNECT again (our answer was slow, or lost): answer it again.
        if let Some(ci) = self.client_registry.clients.get(&packet.signature) {
            if self.is_repeat(ci, client, packet.session_id) {
                let ci = ci.borrow();
                packet.payload = ci.connect_answer.clone().unwrap_or_default();
                packet.conn_signature = Some(0);
                if let Err(e) = self.send_ack(logger, &client, &packet, &ci, !packet.payload.is_empty()) {
                    error!(logger, "Error sending connect ack"; "error" => %e);
                }
                debug!(logger, "CONNECT repeated; answered again");
            } else if let Some(skipped) = self.noise.allow() {
                warn!(logger, "CONNECT from {client} for connection {:x}, which isn't its own; ignored", packet.signature; "similar_skipped" => skipped);
            }
            return;
        }
        // The signature must be the one this address got for its SYN (the game sends back
        // what our SYN answer gave it).
        if !self.cookies.valid(client, packet.signature) {
            if let Some(skipped) = self.noise.allow() {
                warn!(logger, "Unknown client {:x} tried to connect. Ignoring the attempt", packet.signature; "similar_skipped" => skipped);
            }
            return;
        }
        if self.client_registry.connections_from(client.ip()) >= max_connections_per_ip() || self.client_registry.clients.len() >= MAX_CONNECTIONS {
            if let Some(skipped) = self.noise.allow() {
                warn!(logger, "Refusing a connection from {client}: too many connections"; "similar_skipped" => skipped);
            }
            return;
        }
        let mut ci: ClientInfo<T> = ClientInfo::new(client);
        ci.server_signature = packet.signature;
        ci.client_signature = Some(signature);
        ci.server_session = rand::random();
        ci.client_session = packet.session_id;
        self.client_registry.insert(packet.signature, ci);
        let ci = self.client_registry.clients.get(&packet.signature).expect("just inserted");
        if !packet.payload.is_empty() {
            let data = std::mem::take(&mut packet.payload);
            let mut s = ReadStream::from_bytes(&data);
            let ticket_key = &self.ctx.ticket_key;
            let cids = &mut self.client_registry.connection_id_session_ids;
            let next_conn_id = &mut self.next_conn_id;
            let res = move || -> Result<_, crate::rmc::basic::FromStreamError> {
                let ticket: Vec<u8> = s.read()?;
                let request_data: Vec<u8> = s.read()?;

                let ti = KerberosTicketInternal::open(&ticket, ticket_key)?;

                if ti.valid_until
                    < std::time::SystemTime::now()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .as_ref()
                        .map(std::time::Duration::as_secs)
                        .unwrap_or_default()
                {
                    return Ok(vec![]);
                }
                // Only from the address that asked for it. Someone who saw the ticket and the
                // CONNECT on the way can't use them from anywhere else (behind a NAT, the game
                // reaches the auth and secure services from the same public address).
                if !ti.is_for(client.ip()) {
                    warn!(
                        logger,
                        "Ticket of user {} used from {client}, not the address it was issued to; not signed in", ti.principle_id
                    );
                    return Ok(vec![]);
                }
                // The ticket alone proves nothing: it travels readably, and anyone who saw one
                // could replay it. The client proves it has the session key inside by
                // encrypting its own pid with it; only then is it that user.
                let data = crypt_key(ti.session_key.as_ref(), &request_data);

                #[allow(clippy::items_after_statements)]
                #[derive(FromStream, Debug)]
                struct ConnectData {
                    user_pid: u32,
                    _connection_id: u32,
                    challenge: u32,
                }

                let cd: ConnectData = ReadStream::from_bytes(&data).read()?;
                if cd.user_pid != ti.principle_id {
                    // Not signed in: the connection stays anonymous and can do nothing.
                    return Ok(vec![]);
                }
                let id = next_conn_id.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                ci.borrow_mut().user_id.replace(ti.principle_id);
                ci.borrow_mut().connection_id.replace(ConnectionID(id));
                cids.insert(ci.borrow().connection_id.unwrap(), Signature(packet.signature));

                let resp = cd.challenge.wrapping_add(1);
                let resp = resp.to_bytes();

                Ok(resp.to_bytes())
            }();

            match res {
                Ok(resp) => packet.payload = resp,
                Err(e) => error!(logger, "Error parsing ticket"; "error" => %e),
            }
        }

        packet.conn_signature = Some(0);
        ci.borrow_mut().connect_answer = Some(packet.payload.clone());

        if let Err(e) = self.send_ack(logger, &client, &packet, &ci.borrow(), !packet.payload.is_empty()) {
            error!(logger, "Error sending syn ack packet"; "error" => %e);
        }
        info!(logger, "New client connected"; "signature" => packet.signature, "session" => packet.session_id);
        let signed_in = ci.borrow().user_id;
        if let Some(user_id) = signed_in.filter(|id| (self.newest_sign_in_wins)(*id)) {
            self.sign_out_elsewhere(logger, user_id, client);
        }
        if let (Some(user_id), Some(handler)) = (signed_in, self.login_handler.as_mut()) {
            handler(user_id, client);
        }
    }

    /// The newest sign-in wins: `user_id` just signed in from `from`, so their connections
    /// from any other address (another PC, or this one before its address changed) are
    /// closed, and what that game left behind (station URLs, rooms) is cleaned up before the
    /// new one registers its own.
    ///
    /// Only other addresses: one game holds several connections to a service at once (three
    /// to the secure server), all from the one socket, so `from` keeps every one of them.
    /// Two games on one account used to stay connected together, each overwriting the other's
    /// station URLs, with invitations and notifications reaching either.
    ///
    /// Not for the accounts [`Self::newest_sign_in_wins`] leaves out: every game signs in to
    /// the telemetry account, with the game's public password, and each would close the
    /// others' connections and clean up after them.
    fn sign_out_elsewhere(&mut self, logger: &Logger, user_id: u32, from: SocketAddr) {
        self.sign_out(logger, user_id, Some(from));
    }

    /// Closes `user_id`'s connections, except those from `keep`, and cleans up after them.
    fn sign_out(&mut self, logger: &Logger, user_id: u32, keep: Option<SocketAddr>) {
        let stale: Vec<ClientInfo<T>> = self
            .client_registry
            .clients
            .extract_if(|_, c| c.try_borrow().is_ok_and(|c| c.user_id == Some(user_id) && Some(*c.address()) != keep))
            .map(|(_, c)| c.into_inner())
            .collect();
        let Some(first) = stale.first() else { return };
        match keep {
            Some(from) => info!(
                logger,
                "User {user_id} signed in from {from}; closing their {} connection(s) from {}",
                stale.len(),
                first.address()
            ),
            None => info!(logger, "Signing out user {user_id}: closing their {} connection(s)", stale.len()),
        }
        for ci in &stale {
            if let Err(e) = self.send_disconnect(logger, ci) {
                debug!(logger, "Couldn't tell {} it was disconnected: {e}", ci.address());
            }
            self.client_registry.forget(ci);
        }
        if let Some(handler) = self.disconnect_handler.as_mut() {
            stale.into_iter().take(1).for_each(|ci| handler(ci));
        }
    }

    /// Tells a client its connection is closed (it may not be listening any more).
    fn send_disconnect(&self, logger: &Logger, ci: &ClientInfo<T>) -> Result<usize, Box<dyn std::error::Error>> {
        let Some((client_vport, server_vport)) = ci.last_vports else {
            return Err("the client hasn't sent anything yet".into());
        };
        let mut packet = QPacket {
            source: server_vport,
            destination: client_vport,
            packet_type: PacketType::Disconnect,
            sequence: ci.server_sequence_id,
            signature: ci.client_signature.unwrap_or_default(),
            session_id: ci.server_session,
            ..Default::default()
        };
        packet.flags.insert(PacketFlag::HasSize);
        self.send_packet(logger, ci.address(), packet)
    }

    /// Sends a response to a client.
    fn send_response(&self, logger: &Logger, src: &SocketAddr, resp: QPacket, ci: &mut ClientInfo<T>) -> Result<usize, Box<dyn std::error::Error>> {
        send_response(logger, self.ctx, src, self.socket.as_ref().unwrap(), resp, ci)
    }

    /// Sends a packet to a client.
    fn send_packet(&self, logger: &Logger, src: &SocketAddr, resp: QPacket) -> Result<usize, Box<dyn std::error::Error>> {
        send_packet(logger, self.ctx, src, self.socket.as_ref().unwrap(), resp)
    }

    /// Sends an ACK packet to a client.
    fn send_ack(&self, logger: &Logger, src: &SocketAddr, packet: &QPacket, ci: &ClientInfo<T>, keep_payload: bool) -> Result<usize, Box<dyn std::error::Error>> {
        let mut resp = packet.clone();
        resp.source = packet.destination;
        resp.destination = packet.source;
        resp.flags = PacketFlag::Ack | PacketFlag::HasSize;
        resp.signature = ci.client_signature.unwrap_or_default();
        resp.session_id = ci.server_session;
        if !keep_payload {
            resp.payload.clear();
        }
        resp.sequence = packet.sequence;
        self.send_packet(logger, src, resp)
    }

    /// Clears expired clients from the client registry.
    ///
    /// Expiry only counts as a goodbye when the same user has no live connection left. A
    /// player who restarts their client keeps the previous entry in the registry until
    /// `SESSION_TIMEOUT` elapses; if they log back in and open a match within that minute, the
    /// expiring stale entry would take the fresh rooms down with it, because cleanup keys on
    /// the creator rather than on the connection.
    fn clear_clients(&mut self) {
        let now = Instant::now();
        // A connection that never signed in nor sent anything goes sooner: a game always does
        // straight away, so it's a handshake someone left open.
        let expired = |ci: &ClientInfo<T>| {
            let quiet = now - ci.last_seen;
            quiet > SESSION_TIMEOUT || (quiet > SILENT_TIMEOUT && ci.user_id.is_none() && ci.handled.is_empty())
        };
        let expired: Vec<_> = self
            .client_registry
            .clients
            .extract_if(|_k, v| v.try_borrow().is_ok_and(|ci| expired(&ci)))
            .map(|(_, ci)| ci.into_inner())
            .collect();
        for ci in expired {
            if self.client_registry.forget(&ci) {
                if let Some(handler) = self.expired_client_handler.as_mut() {
                    (handler)(ci);
                }
            }
        }
    }
}

/// Sends a response to a client.
pub fn send_response<T>(
    logger: &Logger,
    ctx: &Context,
    src: &SocketAddr,
    socket: &UdpSocket,
    mut resp: QPacket,
    ci: &mut ClientInfo<T>,
) -> Result<usize, Box<dyn std::error::Error>> {
    resp.sequence = ci.server_sequence_id;
    ci.server_sequence_id += 1;
    resp.flags.insert(PacketFlag::HasSize);
    resp.flags.insert(PacketFlag::NeedAck);
    resp.flags.insert(PacketFlag::Reliable);
    resp.signature = ci.client_signature.unwrap_or_default();
    resp.session_id = ci.server_session;
    let sequence = resp.sequence;
    let data = encode_packet(logger, ctx, resp);
    let sz = socket.send_to(&data, src)?;
    // Reliable: resent until acknowledged, and kept as a reply to the packet
    // being handled (for answering a retransmission of it).
    if ci.unacked.len() >= UNACKED_MAX {
        let oldest = *ci.unacked.keys().next().expect("not empty");
        ci.unacked.remove(&oldest);
    }
    ci.unacked.insert(
        sequence,
        crate::Unacked {
            data: data.clone(),
            sent: Instant::now(),
            tries: 0,
        },
    );
    if let Some(replies) = ci.replying.as_mut() {
        replies.push(data);
    }
    Ok(sz)
}

/// Sends a request to a client.
pub fn send_request<T>(
    logger: &Logger,
    ctx: &Context,
    src: &SocketAddr,
    socket: &UdpSocket,
    mut req: QPacket,
    ci: &mut ClientInfo<T>,
) -> Result<usize, Box<dyn std::error::Error>> {
    req.sequence = ci.client_sequence_id;
    ci.client_sequence_id += 1;
    req.flags.insert(PacketFlag::HasSize);
    req.flags.insert(PacketFlag::NeedAck);
    req.flags.insert(PacketFlag::Reliable);
    req.signature = ci.server_signature;
    req.session_id = ci.client_session;
    send_packet(logger, ctx, src, socket, req)
}

/// Sends a packet to a client.
pub fn send_packet(logger: &Logger, ctx: &Context, src: &SocketAddr, socket: &UdpSocket, resp: QPacket) -> Result<usize, Box<dyn std::error::Error>> {
    let data = encode_packet(logger, ctx, resp);
    let sz = socket.send_to(&data, src)?;
    assert_eq!(sz, data.len());
    Ok(sz)
}

/// Encodes a packet the server sends.
fn encode_packet(logger: &Logger, ctx: &Context, mut resp: QPacket) -> Vec<u8> {
    if matches!(resp.packet_type, PacketType::Data) {
        resp.use_compression = true;
        if resp.fragment_id.is_none() {
            resp.fragment_id = Some(0);
        }
    }
    resp.flags.insert(PacketFlag::HasSize);
    trace!(logger, "<- {:?}", resp);
    let data = resp.to_bytes(ctx);
    trace!(logger, "<- {:02x?}", data);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(user_id: Option<u32>, port: u16) -> ClientInfo<()> {
        let mut ci = ClientInfo::new(SocketAddr::from(([10, 77, 0, 2], port)));
        ci.user_id = user_id;
        ci
    }

    /// A SYN sent twice (its answer was slow) gets the same signature both
    /// times, before and after the connection is made; another session gets
    /// its own.
    #[test]
    fn a_repeated_syn_keeps_its_signature() {
        let ctx = Context::splinter_cell_blacklist();
        let logger = Logger::root(slog::Discard, o!());
        let mut server: Server<'_, fn(ClientInfo<()>), fn(ClientInfo<()>), ()> = Server::new(logger.clone(), &ctx, StreamHandlerRegistry::new(logger.clone()));
        server.bind("127.0.0.1:0").unwrap();
        let game = UdpSocket::bind("127.0.0.1:0").unwrap();
        game.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let from = game.local_addr().unwrap();
        let syn = |session: u8| QPacket {
            packet_type: PacketType::Syn,
            flags: PacketFlag::NeedAck.into(),
            conn_signature: Some(0),
            session_id: session,
            ..Default::default()
        };
        let answer = || {
            let mut buf = [0u8; 1024];
            let n = game.recv(&mut buf).unwrap();
            QPacket::from_bytes(&ctx, &buf[..n]).unwrap().0.conn_signature.unwrap()
        };

        server.handle_syn(&logger, syn(7), from);
        let first = answer();
        server.handle_syn(&logger, syn(7), from);
        assert_eq!(answer(), first, "a repeated SYN must get the same signature");

        let connect = QPacket {
            packet_type: PacketType::Connect,
            flags: PacketFlag::NeedAck.into(),
            signature: first,
            conn_signature: Some(0x1234),
            session_id: 7,
            ..Default::default()
        };
        server.handle_connect(&logger, connect.clone(), from);
        let mut buf = [0u8; 1024];
        let _ = game.recv(&mut buf).unwrap();
        server.handle_connect(&logger, connect, from);
        let n = game.recv(&mut buf).expect("a repeated CONNECT must be answered again");
        let again = QPacket::from_bytes(&ctx, &buf[..n]).unwrap().0;
        assert!(again.flags.contains(PacketFlag::Ack) && again.packet_type == PacketType::Connect);
        server.handle_syn(&logger, syn(7), from);
        assert_eq!(answer(), first, "a SYN repeated after connecting must too");
        assert!(server.client_registry.clients.contains_key(&first));

        server.handle_syn(&logger, syn(8), from);
        assert_ne!(answer(), first, "another session is another handshake");
    }

    #[test]
    fn cleanup_waits_for_the_users_last_connection() {
        let mut registry = ClientRegistry::<()>::default();
        let old = client(Some(1001), 3074);
        registry.insert(1, client(Some(1001), 3075));
        registry.insert(2, client(Some(1002), 3076));
        assert!(!registry.forget(&old), "the user reconnected: their state must stay");

        let gone = registry.clients.remove(&1).unwrap().into_inner();
        assert!(registry.forget(&gone), "no connection left: clean up");
        assert!(registry.forget(&client(None, 3077)), "never logged in: nothing to keep");
    }

    fn test_server(ctx: &Context) -> Server<'_, fn(ClientInfo<()>), fn(ClientInfo<()>), ()> {
        let logger = Logger::root(slog::Discard, o!());
        let mut server = Server::new(logger.clone(), ctx, StreamHandlerRegistry::new(logger));
        server.bind("127.0.0.1:0").unwrap();
        server
    }

    fn handshake(packet_type: PacketType, signature: u32, session: u8) -> QPacket {
        QPacket {
            packet_type,
            flags: PacketFlag::NeedAck.into(),
            signature,
            conn_signature: Some(if packet_type == PacketType::Syn { 0 } else { 0x1234 }),
            session_id: session,
            ..Default::default()
        }
    }

    /// SYNs, forged or not, leave nothing behind; a CONNECT makes a connection only with the
    /// signature its own address was given.
    #[test]
    fn handshakes_keep_no_state_until_a_connect_proves_its_address() {
        let ctx = Context::splinter_cell_blacklist();
        let logger = Logger::root(slog::Discard, o!());
        let mut server = test_server(&ctx);
        for port in 40000..41000u16 {
            server.handle_syn(&logger, handshake(PacketType::Syn, 0, 1), SocketAddr::from(([127, 0, 0, 1], port)));
        }
        assert!(server.client_registry.clients.is_empty(), "a SYN keeps no state");

        let me = SocketAddr::from(([127, 0, 0, 1], 50000));
        let other = SocketAddr::from(([127, 0, 0, 1], 50001));
        let mine = server.cookies.signature(me, 3, 0);
        server.handle_connect(&logger, handshake(PacketType::Connect, server.cookies.signature(other, 3, 0), 3), me);
        server.handle_connect(&logger, handshake(PacketType::Connect, mine ^ 0x400, 3), me);
        assert!(server.client_registry.clients.is_empty(), "another address's signature, or a guess, makes nothing");
        server.handle_connect(&logger, handshake(PacketType::Connect, mine, 3), me);
        assert!(server.client_registry.clients.contains_key(&mine));
        assert_eq!(server.client_registry.connections_from(me.ip()), 1);
        assert!(server.cookies.valid(me, mine) && !server.cookies.valid(other, mine));
    }

    /// A connection an earlier game left lingering keeps its signature; the new game's
    /// handshake on the same address and session gets another one.
    #[test]
    fn a_lingering_connection_gets_a_new_handshake_its_own_signature() {
        let ctx = Context::splinter_cell_blacklist();
        let logger = Logger::root(slog::Discard, o!());
        let mut server = test_server(&ctx);
        let game = UdpSocket::bind("127.0.0.1:0").unwrap();
        game.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let from = game.local_addr().unwrap();
        let answer = || {
            let mut buf = [0u8; 1024];
            let n = game.recv(&mut buf).unwrap();
            QPacket::from_bytes(&ctx, &buf[..n]).unwrap().0
        };
        server.handle_syn(&logger, handshake(PacketType::Syn, 0, 9), from);
        let old = answer().conn_signature.unwrap();
        server.handle_connect(&logger, handshake(PacketType::Connect, old, 9), from);
        let _ = answer();
        server.client_registry.clients[&old].borrow_mut().last_seen = Instant::now() - Duration::from_secs(30);

        server.handle_syn(&logger, handshake(PacketType::Syn, 0, 9), from);
        let new = answer().conn_signature.unwrap();
        assert_ne!(new, old);
        server.handle_connect(&logger, handshake(PacketType::Connect, new, 9), from);
        assert_eq!(answer().packet_type, PacketType::Connect);
        assert_eq!(server.client_registry.clients.len(), 2);
        // Repeated now, the SYN gets the new connection's signature.
        server.handle_syn(&logger, handshake(PacketType::Syn, 0, 9), from);
        assert_eq!(answer().conn_signature, Some(new));
    }

    /// A connection that never says anything after its CONNECT goes after a few seconds.
    #[test]
    fn silent_connections_expire_early() {
        let ctx = Context::splinter_cell_blacklist();
        let logger = Logger::root(slog::Discard, o!());
        let mut server = test_server(&ctx);
        let from = SocketAddr::from(([127, 0, 0, 1], 50002));
        let sig = server.cookies.signature(from, 1, 0);
        server.handle_connect(&logger, handshake(PacketType::Connect, sig, 1), from);
        server.client_registry.clients[&sig].borrow_mut().last_seen = Instant::now() - SILENT_TIMEOUT - Duration::from_secs(1);
        server.clear_clients();
        assert!(server.client_registry.clients.is_empty());
        assert_eq!(server.client_registry.connections_from(from.ip()), 0);
    }

    #[test]
    fn echoes_are_limited_per_address_and_in_all() {
        let ctx = Context::splinter_cell_blacklist();
        let mut server = test_server(&ctx);
        let ip = |n: u32| std::net::IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + n));
        assert_eq!((0..10).filter(|_| server.echo_allowed(ip(0))).count(), ECHOES_PER_SECOND as usize);
        for n in 1..MAX_ECHO_SOURCES as u32 {
            server.echo_allowed(ip(n));
        }
        assert!(!server.echo_allowed(ip(u32::MAX >> 8)), "a full table takes no new address before the next sweep");
        server.echo_sweep = Instant::now() - Duration::from_secs(2);
        for (since, _) in server.echoes.values_mut() {
            *since -= Duration::from_secs(2);
        }
        assert!(server.echo_allowed(ip(u32::MAX >> 8)), "swept, there is room again");
    }
}
