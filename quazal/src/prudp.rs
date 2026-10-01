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
/// Handshakes in progress and connections, per address and in all.
const MAX_PENDING_PER_IP: usize = 64;
const MAX_PENDING: usize = 4096;
const MAX_CONNECTIONS: usize = 16384;
/// The address echo (user packets): its largest payload, and how often per address.
const MAX_ECHO_PAYLOAD: usize = 64;
const ECHOES_PER_SECOND: u32 = 5;
/// How long after a connection's last packet a repeated SYN or CONNECT for it
/// is still answered as a repeat.
const REPEAT_WINDOW: Duration = Duration::from_secs(5);

/// Connections one address may hold: 256 (a LAN party behind one router), or
/// `FE_MAX_CONNECTIONS_PER_IP` (the load test's players all share one).
fn max_connections_per_ip() -> usize {
    static MAX: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| std::env::var("FE_MAX_CONNECTIONS_PER_IP").ok().and_then(|v| v.parse().ok()).unwrap_or(256))
}

/// Records that a client's packet with this sequence number is being handled.
fn remember_handled<T>(ci: &mut ClientInfo<T>, sequence: u16) {
    if ci.handled.len() >= HANDLED_MEMORY {
        ci.handled.pop_front();
    }
    ci.handled.push_back((sequence, vec![]));
}

/// A registry for clients.
#[derive(Default)]
pub struct ClientRegistry<T> {
    clients: HashMap<u32, RefCell<ClientInfo<T>>>,
    connection_id_session_ids: HashMap<ConnectionID, Signature>,
}

impl<T> ClientRegistry<T> {
    /// Forgets a client that is gone. Returns whether its user's per-user
    /// state may be cleaned up: not if the same user has another live
    /// connection (they reconnected before the old one expired), whose station
    /// URLs and lobby would otherwise be wiped.
    fn forget(&mut self, ci: &ClientInfo<T>) -> bool {
        if let Some(conn_id) = ci.connection_id {
            self.connection_id_session_ids.remove(&conn_id);
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
    new_clients: HashMap<u32, ClientInfo<T>>,
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
    next_conn_id: AtomicU32,
    /// Address echoes answered per source this second.
    echoes: HashMap<std::net::IpAddr, (Instant, u32)>,
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
        let client_registry = ClientRegistry {
            clients: HashMap::default(),
            connection_id_session_ids: HashMap::default(),
        };
        Server {
            logger,
            registry,
            socket: None,
            ctx,
            new_clients: HashMap::default(),
            echoes: HashMap::default(),
            client_registry,
            user_handler: None,
            expired_client_handler: None,
            disconnect_handler: None,
            login_handler: None,
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
                        error!(logger, "Invalid packet received"; "error" =>  %e);
                        continue 'outer;
                    }
                };
                #[allow(clippy::cast_possible_truncation)]
                let (packet_data, next_data) = data.split_at(nparsed as usize);
                data = next_data;
                trace!(logger, "-> {:02x?}", packet_data);

                if let Err(e) = packet.validate(self.ctx, packet_data) {
                    error!(logger, "Invalid packet received: {:?}", packet; "error" =>  %e);
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
                    error!(logger, "unsupported user packet");
                } else {
                    (self.user_handler.as_ref().unwrap())(logger, packet, client, self.socket.as_ref().unwrap());
                }
            }
            PacketType::Route | PacketType::Raw => {
                warn!(logger, "unsupported packet type {:?}", packet.packet_type);
            }
        }
    }

    /// Whether `ip` may have another address echo now (at most
    /// [`ECHOES_PER_SECOND`]).
    fn echo_allowed(&mut self, ip: std::net::IpAddr) -> bool {
        let now = Instant::now();
        if self.echoes.len() > 10_000 {
            self.echoes.retain(|_, (since, _)| now.duration_since(*since) < Duration::from_secs(1));
        }
        let (since, count) = self.echoes.entry(ip).or_insert((now, 0));
        if now.duration_since(*since) >= Duration::from_secs(1) {
            *since = now;
            *count = 0;
        }
        *count += 1;
        *count <= ECHOES_PER_SECOND
    }

    /// Handles a data packet.
    fn handle_data(&mut self, logger: &Logger, packet: QPacket, client: SocketAddr) {
        #![allow(clippy::cast_possible_truncation)]

        debug!(logger, "Handling data packet");
        let Some(ci) = self.client_registry.clients.get(&packet.signature) else {
            warn!(logger, "client is unknown!");
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
    fn handle_syn(&mut self, logger: &Logger, mut packet: QPacket, client: SocketAddr) {
        debug!(logger, "Handling syn packet");
        // The same SYN again: the game sends it again when our answer takes longer than
        // its first resend (a server ~300 ms away), and takes the signature from the last
        // answer it gets. A new signature would leave it talking on a handshake that never
        // finished ("client is unknown"), so it gets the one it already has.
        if let Some(sig) = self.handshake_of(client, packet.session_id) {
            debug!(logger, "SYN repeated; answering with the same signature");
            packet.conn_signature = Some(sig);
            let ack = match self.new_clients.get(&sig) {
                Some(ci) => self.send_ack(logger, &client, &packet, ci, false),
                None => match self.client_registry.clients.get(&sig) {
                    Some(ci) => self.send_ack(logger, &client, &packet, &ci.borrow(), false),
                    None => return,
                },
            };
            if let Err(e) = ack {
                error!(logger, "Error sending syn ack packet"; "error" => %e);
            }
            return;
        }
        // Bounded handshakes and connections, per address and in all.
        let pending_here = self.new_clients.values().filter(|c| c.address().ip() == client.ip()).count();
        let open_here = self
            .client_registry
            .clients
            .values()
            .filter(|c| c.try_borrow().is_ok_and(|c| c.address().ip() == client.ip()))
            .count();
        if pending_here >= MAX_PENDING_PER_IP
            || open_here >= max_connections_per_ip()
            || self.new_clients.len() >= MAX_PENDING
            || self.client_registry.clients.len() >= MAX_CONNECTIONS
        {
            warn!(logger, "Refusing a handshake from {client}: too many connections");
            return;
        }
        let mut ci: ClientInfo<T> = ClientInfo::new(client);
        ci.client_session = packet.session_id;
        let sig = ci.server_signature;
        self.new_clients.insert(sig, ci);

        packet.conn_signature = Some(sig);

        let ci = self.new_clients.get(&sig).unwrap();

        if let Err(e) = self.send_ack(logger, &client, &packet, ci, false) {
            error!(logger, "Error sending syn ack packet"; "error" => %e);
        }
    }

    /// The signature already given to `client`'s handshake with this session,
    /// in progress or connected.
    ///
    /// A connection counts only while it's fresh (seen in the last few seconds):
    /// a game started again could, by chance, reuse the session number of its
    /// last connection, which lingers for a minute, and needs a handshake of its own.
    fn handshake_of(&self, client: SocketAddr, session: u8) -> Option<u32> {
        let ours = |ci: &ClientInfo<T>| *ci.address() == client && ci.client_session == session;
        self.new_clients.iter().find(|(_, ci)| ours(ci)).map(|(sig, _)| *sig).or_else(|| {
            self.client_registry
                .clients
                .iter()
                .find(|(_, ci)| ci.try_borrow().is_ok_and(|ci| ours(&ci) && ci.last_seen.elapsed() < REPEAT_WINDOW))
                .map(|(sig, _)| *sig)
        })
    }

    /// Handles a CONNECT packet.
    fn handle_connect(&mut self, logger: &Logger, mut packet: QPacket, client: SocketAddr) {
        debug!(logger, "Handling connect packet");
        let Some(signature) = packet.conn_signature else {
            error!(logger, "Client {:x} did not provide a connection signature. This should not happen", packet.signature);
            return;
        };

        let Some(mut ci) = self.new_clients.remove(&packet.signature) else {
            // The same CONNECT again (our answer was slow, or lost): answer it again.
            if let Some(ci) = self.client_registry.clients.get(&packet.signature) {
                let ci = ci.borrow();
                if *ci.address() == client && ci.client_session == packet.session_id && ci.last_seen.elapsed() < REPEAT_WINDOW {
                    packet.payload = ci.connect_answer.clone().unwrap_or_default();
                    packet.conn_signature = Some(0);
                    if let Err(e) = self.send_ack(logger, &client, &packet, &ci, !packet.payload.is_empty()) {
                        error!(logger, "Error sending connect ack"; "error" => %e);
                    }
                    debug!(logger, "CONNECT repeated; answered again");
                    return;
                }
            }
            warn!(logger, "Unknown client {:x} tried to connect. Ignoring the attempt", packet.signature);
            return;
        };
        ci.client_signature = Some(signature);
        ci.server_session = rand::random();
        ci.client_session = packet.session_id;

        let ci = {
            self.client_registry.clients.insert(packet.signature, RefCell::new(ci));
            let ci = self.client_registry.clients.get(&packet.signature).unwrap();
            ci
        };

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
        if let (Some(user_id), Some(handler)) = (signed_in, self.login_handler.as_mut()) {
            handler(user_id, client);
        }
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
        // Handshakes that never completed (a SYN without a CONNECT): these
        // used to stay forever, one per SYN from anyone.
        self.new_clients.retain(|_, ci| now - ci.last_seen <= SESSION_TIMEOUT);
        let expired: Vec<_> = self
            .client_registry
            .clients
            .extract_if(|_k, v| v.try_borrow().map(|ci| (now - ci.last_seen) > SESSION_TIMEOUT).unwrap_or(false))
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
        let mut registry = ClientRegistry::<()> {
            clients: HashMap::new(),
            connection_id_session_ids: HashMap::new(),
        };
        let old = client(Some(1001), 3074);
        registry.clients.insert(1, RefCell::new(client(Some(1001), 3075)));
        registry.clients.insert(2, RefCell::new(client(Some(1002), 3076)));
        assert!(!registry.forget(&old), "the user reconnected: their state must stay");

        registry.clients.remove(&1);
        assert!(registry.forget(&old), "no connection left: clean up");
        assert!(registry.forget(&client(None, 3077)), "never logged in: nothing to keep");
    }
}
