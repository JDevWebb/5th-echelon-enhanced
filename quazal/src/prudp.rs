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
/// Messages whose last fragment may wait for the others at once, and how far behind the
/// newest packet a kept fragment may be before it's given up on.
const MAX_HELD_MESSAGES: usize = 4;
const FRAGMENT_WINDOW: u16 = 256;
/// Connections in all (per address: [`max_connections_per_ip`]).
const MAX_CONNECTIONS: usize = 16384;
/// Connections that haven't signed in (on the authentication service, everyone's while they
/// sign in): in all, from one address, and how long one lasts. A game signs in within
/// seconds; strangers holding connections open can't fill the table.
const MAX_ANONYMOUS: usize = 4096;
const MAX_ANONYMOUS_PER_IP: usize = 32;
const ANONYMOUS_LIFETIME: Duration = Duration::from_secs(120);
/// Calls from connections that haven't signed in, per address: a game signing in makes a
/// few, so a burst for a LAN party signing in together, then a steady rate. Each one is
/// handled, so they're bounded per address rather than per connection (a new connection
/// costs nothing).
const ANONYMOUS_CALLS_PER_SECOND: f32 = 20.0;
const ANONYMOUS_CALL_BURST: f32 = 200.0;
const MAX_ANONYMOUS_SOURCES: usize = 10_000;
/// What a connection that hasn't signed in may keep of a message in fragments, held last
/// fragments included (a sign-in fits one packet or a few).
const MAX_ANONYMOUS_REASSEMBLED: usize = 8 * 1024;
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
    many_per_ip().unwrap_or(256)
}

/// `FE_MAX_CONNECTIONS_PER_IP`, which lifts the per-address limits (the load test's
/// players all share one address).
pub fn many_per_ip() -> Option<usize> {
    static MAX: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| std::env::var("FE_MAX_CONNECTIONS_PER_IP").ok().and_then(|v| v.parse().ok()))
}

/// An address as 16 bytes (IPv4 as IPv4-mapped IPv6), the same for both ways of
/// writing an IPv4 address.
/// Whether the server knows `ip` to be the player `pid`'s (their API sign-in over TLS came from
/// there lately): a ticket used from there is theirs, wherever it was issued. Set once by the
/// server; without it only the ticket's own address and its neighbours are.
static TICKET_ADDRESS_PROVEN: std::sync::OnceLock<fn(u32, std::net::IpAddr) -> bool> = std::sync::OnceLock::new();

/// Whether a ticket (its user, and the time it's good until) may still connect: one from
/// before a password change or a ban, or for an account that's gone, mayn't. Set once by
/// the server; without it every unexpired ticket may.
#[allow(clippy::type_complexity)]
static TICKET_ACCEPTED: std::sync::OnceLock<Box<dyn Fn(u32, u64) -> bool + Send + Sync>> = std::sync::OnceLock::new();

/// The least time between two resends of replies to retransmitted packets, per connection:
/// a game resends after about a second, so this never holds up a real one.
const REPLAY_GAP: Duration = Duration::from_millis(250);

/// Sets [`TICKET_ACCEPTED`].
pub fn set_ticket_accepted(check: impl Fn(u32, u64) -> bool + Send + Sync + 'static) {
    let _ = TICKET_ACCEPTED.set(Box::new(check));
}

/// Sets [`TICKET_ADDRESS_PROVEN`].
pub fn set_ticket_address_proven(check: fn(u32, std::net::IpAddr) -> bool) {
    let _ = TICKET_ADDRESS_PROVEN.set(check);
}

pub(crate) fn address_bytes(ip: std::net::IpAddr) -> [u8; 16] {
    match ip.to_canonical() {
        std::net::IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
        std::net::IpAddr::V6(v6) => v6.octets(),
    }
}

/// What a fragment made of the message it belongs to.
enum Assembled {
    /// The whole message: its last packet (whose sequence number the replies go with), the
    /// payload, and how many packets it came in.
    Whole(QPacket, Vec<u8>, usize),
    /// Kept until the rest of the message is in.
    Waiting,
    /// Over the limits: everything kept was dropped.
    TooMuch,
}

/// Takes a data packet that carries a fragment id: 1, 2, ... for the leading fragments of a
/// message and 0 for its last (or only) one, with consecutive sequence numbers.
///
/// UDP may reorder them, so leading fragments are kept by sequence number, and the message
/// is put together only once every one is in. A last fragment that comes before the others
/// waits for them, and so does one whose message lost a fragment in the middle (that used to
/// be put together without it, and a stats request of a dozen fragments failed). Fragments
/// are matched by sequence number, so leftovers of an earlier message never get mixed in.
fn add_fragment<T>(ci: &mut ClientInfo<T>, packet: QPacket) -> Assembled {
    let seq = packet.sequence;
    // Leftovers far from it belong to messages that never completed (packets that came
    // early are a little ahead).
    let near = |s: &u16| {
        let d = seq.wrapping_sub(*s);
        d <= FRAGMENT_WINDOW || d >= FRAGMENT_WINDOW.wrapping_neg()
    };
    ci.packet_fragments.retain(|s, _| near(s));
    ci.held_last_fragments.retain(|s, _| near(s));
    // Before signing in, little is kept: no stranger can fill the memory with fragments.
    let (max_held, max_bytes) = if ci.user_id.is_some() {
        (MAX_HELD_MESSAGES, MAX_REASSEMBLED)
    } else {
        (1, MAX_ANONYMOUS_REASSEMBLED)
    };
    if ci.user_id.is_none() {
        let kept: usize = ci.packet_fragments.values().map(|(_, p)| p.len()).sum::<usize>() + ci.held_last_fragments.values().map(|p| p.payload.len()).sum::<usize>();
        if kept + packet.payload.len() > max_bytes {
            ci.packet_fragments.clear();
            ci.held_last_fragments.clear();
            return Assembled::TooMuch;
        }
    }
    if packet.fragment_id == Some(0) {
        // Nothing of an earlier fragment just before it: a message in one packet. (If every
        // leading fragment is still on its way, there is no telling it apart.)
        if !ci.packet_fragments.contains_key(&seq.wrapping_sub(1)) {
            let mut packet = packet;
            let payload = std::mem::take(&mut packet.payload);
            return Assembled::Whole(packet, payload, 1);
        }
        if ci.held_last_fragments.len() >= max_held {
            ci.packet_fragments.clear();
            ci.held_last_fragments.clear();
            return Assembled::TooMuch;
        }
        ci.held_last_fragments.insert(seq, packet);
    } else {
        let cached: usize = ci.packet_fragments.values().map(|(_, p)| p.len()).sum();
        if ci.packet_fragments.len() >= MAX_FRAGMENTS || cached + packet.payload.len() > max_bytes {
            ci.packet_fragments.clear();
            ci.held_last_fragments.clear();
            return Assembled::TooMuch;
        }
        ci.packet_fragments.insert(seq, (packet.fragment_id.unwrap_or_default(), packet.payload));
    }
    let complete = ci.held_last_fragments.keys().copied().find(|last| {
        let Some((n, _)) = ci.packet_fragments.get(&last.wrapping_sub(1)) else {
            return false;
        };
        (1..=*n).all(|k| ci.packet_fragments.get(&last.wrapping_sub(u16::from(*n - k + 1))).is_some_and(|(fid, _)| *fid == k))
    });
    let Some(last) = complete else {
        return Assembled::Waiting;
    };
    let mut packet = ci.held_last_fragments.remove(&last).expect("found above");
    let n = ci.packet_fragments[&last.wrapping_sub(1)].0;
    let mut payload = vec![];
    for k in 1..=n {
        let (_, part) = ci.packet_fragments.remove(&last.wrapping_sub(u16::from(n - k + 1))).expect("checked above");
        payload.extend(part);
    }
    payload.extend(std::mem::take(&mut packet.payload));
    Assembled::Whole(packet, payload, usize::from(n) + 1)
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

/// A token bucket: `burst` at once, refilled at `rate` a second.
struct Bucket {
    at: Instant,
    tokens: f32,
}

impl Bucket {
    fn full(burst: f32) -> Self {
        Self {
            at: Instant::now(),
            tokens: burst,
        }
    }

    fn take(&mut self, rate: f32, burst: f32) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f32() * rate).min(burst);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
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
    /// Connections that haven't signed in, in all and per address (see
    /// [`ClientInfo::counted_anonymous`]).
    anonymous: usize,
    anonymous_per_ip: HashMap<std::net::IpAddr, usize>,
}

impl<T> ClientRegistry<T> {
    /// Adds a new connection under its signature (not signed in yet).
    fn insert(&mut self, signature: u32, mut ci: ClientInfo<T>) {
        *self.per_ip.entry(ci.address().ip()).or_default() += 1;
        if ci.user_id.is_none() {
            ci.counted_anonymous = true;
            self.anonymous += 1;
            *self.anonymous_per_ip.entry(ci.address().ip()).or_default() += 1;
        }
        self.clients.insert(signature, RefCell::new(ci));
    }

    /// Connections from `ip`.
    fn connections_from(&self, ip: std::net::IpAddr) -> usize {
        self.per_ip.get(&ip).copied().unwrap_or(0)
    }

    /// Connections from `ip` that haven't signed in, and in all.
    fn anonymous_from(&self, ip: std::net::IpAddr) -> (usize, usize) {
        (self.anonymous_per_ip.get(&ip).copied().unwrap_or(0), self.anonymous)
    }

    /// Takes a connection that has signed in (on CONNECT with a ticket, or by a sign-in call)
    /// off the count of those that haven't.
    fn settle(&mut self, signature: u32) {
        let Some(ci) = self.clients.get_mut(&signature).map(RefCell::get_mut) else {
            return;
        };
        if ci.counted_anonymous && ci.user_id.is_some() {
            ci.counted_anonymous = false;
            let ip = ci.address().ip();
            self.uncount_anonymous(ip);
        }
    }

    fn uncount_anonymous(&mut self, ip: std::net::IpAddr) {
        self.anonymous = self.anonymous.saturating_sub(1);
        if let std::collections::hash_map::Entry::Occupied(mut n) = self.anonymous_per_ip.entry(ip) {
            *n.get_mut() -= 1;
            if *n.get() == 0 {
                n.remove();
            }
        }
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
        if ci.counted_anonymous {
            self.uncount_anonymous(ci.address().ip());
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
    /// Called with a connection that closed or expired while its user still has others (a
    /// game that dropped and signed in again from the same address): their state stays, but
    /// what that connection alone made can go.
    pub stale_connection_handler: Option<Box<dyn FnMut(ClientInfo<T>) + 'a>>,
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
    /// Whether only connections that signed in (with a ticket) are handled: the secure
    /// service's. Everyone else's data is dropped unread.
    pub sign_in_required: bool,
    /// Calls from connections that haven't signed in, per address (see
    /// [`ANONYMOUS_CALLS_PER_SECOND`]), and when the table was last swept.
    anonymous_calls: HashMap<std::net::IpAddr, Bucket>,
    anonymous_sweep: Instant,
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
            sign_in_required: false,
            anonymous_calls: HashMap::default(),
            anonymous_sweep: Instant::now(),
            client_registry: ClientRegistry::default(),
            user_handler: None,
            expired_client_handler: None,
            disconnect_handler: None,
            login_handler: None,
            stale_connection_handler: None,
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
                // Data, Disconnect and Ping only count for a connection from this address (see
                // `handle_packet`): anything else is dropped from its header, before its payload
                // is decrypted and inflated, which anyone could otherwise ask of the server with
                // a few bytes (the key is public). Header: ports (2), type (1), session (1),
                // signature (4).
                if data.len() >= 8 && matches!(data[2] & 0x7, 2..=4) {
                    let signature = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
                    let ours = self
                        .client_registry
                        .clients
                        .get(&signature)
                        .is_some_and(|ci| ci.try_borrow().is_ok_and(|ci| *ci.address() == client));
                    if !ours {
                        continue 'outer;
                    }
                }
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
                    if let Some(handler) = self.stale_connection_handler.as_mut() {
                        handler(ci);
                    }
                } else if let Some(handler) = self.disconnect_handler.as_mut() {
                    (handler)(ci);
                }
            }
            PacketType::Ping => {
                let Some(ci) = self.client_registry.clients.get(&packet.signature) else {
                    return;
                };
                // Pings keep a signed-in connection; one that never signed in goes when its
                // time is up, pinged or not (or a few addresses could hold every place).
                if ci.borrow().user_id.is_some() {
                    ci.borrow_mut().seen();
                }
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
    fn handle_data(&mut self, logger: &Logger, mut packet: QPacket, client: SocketAddr) {
        #![allow(clippy::cast_possible_truncation)]

        debug!(logger, "Handling data packet");
        let Some(ci) = self.client_registry.clients.get(&packet.signature) else {
            if let Some(skipped) = self.noise.allow() {
                warn!(logger, "client is unknown!"; "similar_skipped" => skipped);
            }
            return;
        };
        let signed_in = ci.borrow().user_id.is_some();
        // Nothing from a connection without a ticket on the secure service, nor more than
        // the rate from an address's connections that haven't signed in: each call is
        // handled and logged, and a flood of them from one stranger stopped every sign-in.
        // Not acknowledged, and it doesn't keep the connection alive.
        if !signed_in && (self.sign_in_required || !self.anonymous_call_allowed(client.ip())) {
            if let Some(skipped) = self.noise.allow() {
                debug!(logger, "Data from a connection that hasn't signed in dropped"; "similar_skipped" => skipped);
            }
            return;
        }
        let Some(ci) = self.client_registry.clients.get(&packet.signature) else {
            return;
        };
        let logger = logger.new(o!("pid" => ci.borrow().user_id));
        let mut guard = ci.borrow_mut();
        let ci = &mut *guard;
        ci.seen();
        if let Err(e) = self.send_ack(&logger, &client, &packet, &*ci, false) {
            error!(logger, "Error sending ack"; "error" => %e);
        } else {
            debug!(logger, "Send ack");
        }
        // A retransmission (our acknowledgement was lost): answer it again, but
        // don't handle it twice (a repeated CreateSession made a second lobby).
        if let Some(replies) = ci.handled.iter().find(|(seq, _)| *seq == packet.sequence).map(|(_, replies)| replies.clone()) {
            if ci.last_replay.is_some_and(|at| at.elapsed() < REPLAY_GAP) {
                return;
            }
            ci.last_replay = Some(Instant::now());
            info!(logger, "Duplicate packet {}; resending {} replies", packet.sequence, replies.len());
            for data in replies {
                if let Err(e) = self.socket.as_ref().unwrap().send_to(&data, client) {
                    error!(logger, "Error resending reply"; "error" => %e);
                }
            }
            return;
        }
        remember_handled(ci, packet.sequence);
        let (packet, payload) = if packet.fragment_id.is_some() {
            match add_fragment(ci, packet) {
                Assembled::Whole(packet, payload, fragments) => {
                    if fragments > 1 {
                        info!(logger, "Reassembled {fragments} fragments");
                    }
                    (packet, payload)
                }
                Assembled::Waiting => {
                    debug!(logger, "Fragment kept until the rest of its message is in");
                    return;
                }
                Assembled::TooMuch => {
                    warn!(logger, "Too many or too large fragments; dropping them");
                    return;
                }
            }
        } else {
            let payload = std::mem::take(&mut packet.payload);
            (packet, payload)
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
            // At most now and then: a connection that never signed in can send these at will.
            None => {
                if let Some(skipped) = self.noise.allow() {
                    error!(logger, "No handler found"; "similar_skipped" => skipped);
                }
            }
            Some(Err(_)) => {
                if let Some(skipped) = self.noise.allow() {
                    error!(logger, "Handler failed"; "similar_skipped" => skipped);
                }
            }
        }
        // Keep the replies for this sequence number, for a retransmission.
        let replies = ci.replying.take().unwrap_or_default();
        if let Some(entry) = ci.handled.iter_mut().find(|(seq, _)| *seq == packet.sequence) {
            entry.1 = replies;
        }
        drop(guard);
        if !signed_in {
            self.client_registry.settle(packet.signature);
        }
    }

    /// Whether a connection from `ip` that hasn't signed in may make another call now (see
    /// [`ANONYMOUS_CALLS_PER_SECOND`]).
    fn anonymous_call_allowed(&mut self, ip: std::net::IpAddr) -> bool {
        if many_per_ip().is_some() {
            return true;
        }
        if let Some(bucket) = self.anonymous_calls.get_mut(&ip) {
            return bucket.take(ANONYMOUS_CALLS_PER_SECOND, ANONYMOUS_CALL_BURST);
        }
        let now = Instant::now();
        if self.anonymous_calls.len() >= MAX_ANONYMOUS_SOURCES && now.duration_since(self.anonymous_sweep) >= Duration::from_secs(1) {
            // Those that would be full again by now: forgetting them changes nothing.
            let refilled = Duration::from_secs_f32(ANONYMOUS_CALL_BURST / ANONYMOUS_CALLS_PER_SECOND);
            self.anonymous_calls.retain(|_, b| now.duration_since(b.at) < refilled);
            self.anonymous_sweep = now;
        }
        // Still full: someone with thousands of addresses. Each is let through rather than
        // every newcomer refused.
        if self.anonymous_calls.len() >= MAX_ANONYMOUS_SOURCES {
            return true;
        }
        let mut bucket = Bucket::full(ANONYMOUS_CALL_BURST);
        let allowed = bucket.take(ANONYMOUS_CALLS_PER_SECOND, ANONYMOUS_CALL_BURST);
        self.anonymous_calls.insert(ip, bucket);
        allowed
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
        let (anonymous_here, anonymous) = self.client_registry.anonymous_from(client.ip());
        if self.client_registry.connections_from(client.ip()) >= max_connections_per_ip()
            || self.client_registry.clients.len() >= MAX_CONNECTIONS
            || anonymous_here >= many_per_ip().unwrap_or(MAX_ANONYMOUS_PER_IP)
            || anonymous >= MAX_ANONYMOUS
        {
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
            if !self.is_repeat(ci, client, packet.session_id) {
                if let Some(skipped) = self.noise.allow() {
                    warn!(logger, "CONNECT from {client} for connection {:x}, which isn't its own; ignored", packet.signature; "similar_skipped" => skipped);
                }
                return;
            }
            if ci.borrow().connect_request.as_deref() == Some(packet.payload.as_slice()) {
                let ci = ci.borrow();
                packet.payload = ci.connect_answer.clone().unwrap_or_default();
                packet.conn_signature = Some(0);
                if let Err(e) = self.send_ack(logger, &client, &packet, &ci, !packet.payload.is_empty()) {
                    error!(logger, "Error sending connect ack"; "error" => %e);
                }
                debug!(logger, "CONNECT repeated; answered again");
                return;
            }
            // Another ticket: not a resend but a new game on this address that, by chance, has
            // the session number of the connection before it, which went quiet seconds ago (a
            // game that lost the server and signed in again). Answering with the old
            // connection's answer would fail the new sign-in: the old connection goes instead.
            let old = self.client_registry.clients.remove(&packet.signature).expect("just found").into_inner();
            info!(logger, "A new connection from {client} has the session number of the one before it; replacing that one");
            self.expire(old);
        }
        // The signature must be the one this address got for its SYN (the game sends back
        // what our SYN answer gave it).
        if !self.cookies.valid(client, packet.signature) {
            if let Some(skipped) = self.noise.allow() {
                warn!(logger, "Unknown client {:x} tried to connect. Ignoring the attempt", packet.signature; "similar_skipped" => skipped);
            }
            return;
        }
        let (anonymous_here, anonymous) = self.client_registry.anonymous_from(client.ip());
        if self.client_registry.connections_from(client.ip()) >= max_connections_per_ip()
            || self.client_registry.clients.len() >= MAX_CONNECTIONS
            || anonymous_here >= many_per_ip().unwrap_or(MAX_ANONYMOUS_PER_IP)
            || anonymous >= MAX_ANONYMOUS
        {
            if let Some(skipped) = self.noise.allow() {
                warn!(logger, "Refusing a connection from {client}: too many connections"; "similar_skipped" => skipped);
            }
            return;
        }
        let mut ci: ClientInfo<T> = ClientInfo::new(client);
        ci.server_signature = packet.signature;
        ci.client_signature = Some(signature);
        ci.connect_request = Some(packet.payload.clone());
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
                // reaches the auth and secure services from the same public address). A VPN's or
                // carrier's neighbouring address is let in, and so is one the player proved
                // theirs over TLS (their API sign-in), as the relay takes a player's other port
                // on the strength of their tag.
                if !ti.is_for(client.ip()) {
                    let proven = TICKET_ADDRESS_PROVEN.get().is_some_and(|check| check(ti.principle_id, client.ip()));
                    if ti.is_near(client.ip()) || proven {
                        info!(
                            logger,
                            "Ticket of user {} used from {client}, {} the address it was issued to; signed in",
                            ti.principle_id,
                            if proven { "where they signed in to the API, not" } else { "next to" }
                        );
                    } else {
                        warn!(
                            logger,
                            "Ticket of user {} used from {client}, not the address it was issued to nor one they proved theirs; not signed in", ti.principle_id
                        );
                        return Ok(vec![]);
                    }
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
                // A good ticket the account no longer stands behind (a new password, a ban):
                // refused before the connection is anyone's, not signed out after.
                if TICKET_ACCEPTED.get().is_some_and(|accepted| !accepted(ti.principle_id, ti.valid_until)) {
                    warn!(
                        logger,
                        "Ticket of user {} from before a password change or a ban, or for an account that's gone; not signed in", ti.principle_id
                    );
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

        // The sign-in is taken in (the player online, their other games signed out) before
        // it's answered: once the game is signed in, everyone sees it online.
        let signed_in = ci.borrow().user_id;
        self.client_registry.settle(packet.signature);
        let answered = !packet.payload.is_empty();
        if let Some(user_id) = signed_in.filter(|id| (self.newest_sign_in_wins)(*id)) {
            self.sign_out_elsewhere(logger, user_id, client);
        }
        if let (Some(user_id), Some(handler)) = (signed_in, self.login_handler.as_mut()) {
            handler(user_id, client);
        }
        // Signing out its other games leaves this connection (another address's are closed).
        let Some(ci) = self.client_registry.clients.get(&packet.signature) else {
            return;
        };
        if let Err(e) = self.send_ack(logger, &client, &packet, &ci.borrow(), answered) {
            error!(logger, "Error sending syn ack packet"; "error" => %e);
        }
        info!(logger, "New client connected"; "signature" => packet.signature, "session" => packet.session_id);
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
        // straight away, so it's a handshake someone left open. One that hasn't signed in
        // goes after [`ANONYMOUS_LIFETIME`] whatever it sends.
        let expired = |ci: &ClientInfo<T>| {
            let quiet = now - ci.last_seen;
            quiet > SESSION_TIMEOUT || (ci.user_id.is_none() && ((quiet > SILENT_TIMEOUT && ci.handled.is_empty()) || now.duration_since(ci.connected) > ANONYMOUS_LIFETIME))
        };
        let expired: Vec<_> = self
            .client_registry
            .clients
            .extract_if(|_k, v| v.try_borrow().is_ok_and(|ci| expired(&ci)))
            .map(|(_, ci)| ci.into_inner())
            .collect();
        for ci in expired {
            self.expire(ci);
        }
    }

    /// A connection that's gone without a goodbye: its user is signed out if it was their
    /// last, else only what it alone made goes (see [`Self::stale_connection_handler`]).
    fn expire(&mut self, ci: ClientInfo<T>) {
        if self.client_registry.forget(&ci) {
            if let Some(handler) = self.expired_client_handler.as_mut() {
                (handler)(ci);
            }
        } else if let Some(handler) = self.stale_connection_handler.as_mut() {
            handler(ci);
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
    use std::net::Ipv4Addr;

    use super::*;

    fn client(user_id: Option<u32>, port: u16) -> ClientInfo<()> {
        let mut ci = ClientInfo::new(SocketAddr::from(([10, 77, 0, 2], port)));
        ci.user_id = user_id;
        ci
    }

    fn fragment(seq: u16, fid: u8, data: &[u8]) -> QPacket {
        QPacket {
            packet_type: PacketType::Data,
            sequence: seq,
            fragment_id: Some(fid),
            payload: data.to_vec(),
            ..Default::default()
        }
    }

    fn whole(a: Assembled) -> Option<(u16, Vec<u8>, usize)> {
        match a {
            Assembled::Whole(p, payload, n) => Some((p.sequence, payload, n)),
            _ => None,
        }
    }

    /// Fragments are put together in order whatever order they come in, and a message
    /// missing one waits for it instead of being handled without it.
    #[test]
    fn fragments_are_put_together_in_order_whenever_they_come() {
        let mut ci = client(Some(1000), 3074);
        // In order.
        assert!(whole(add_fragment(&mut ci, fragment(10, 1, b"a"))).is_none());
        assert!(whole(add_fragment(&mut ci, fragment(11, 2, b"b"))).is_none());
        assert_eq!(whole(add_fragment(&mut ci, fragment(12, 0, b"c"))), Some((12, b"abc".to_vec(), 3)));
        // One in the middle late: the last waits for it.
        assert!(whole(add_fragment(&mut ci, fragment(13, 1, b"d"))).is_none());
        assert!(whole(add_fragment(&mut ci, fragment(15, 3, b"f"))).is_none());
        assert!(whole(add_fragment(&mut ci, fragment(16, 0, b"g"))).is_none(), "fragment 2 is missing");
        assert_eq!(whole(add_fragment(&mut ci, fragment(14, 2, b"e"))), Some((16, b"defg".to_vec(), 4)));
        // Unfragmented messages around it.
        assert_eq!(whole(add_fragment(&mut ci, fragment(17, 0, b"h"))), Some((17, b"h".to_vec(), 1)));
        assert!(ci.packet_fragments.is_empty() && ci.held_last_fragments.is_empty());
    }

    /// A leftover of a message that never completed isn't mixed into the next one, and
    /// sequence numbers may wrap around.
    #[test]
    fn leftovers_never_get_into_another_message() {
        let mut ci = client(Some(1000), 3074);
        assert!(whole(add_fragment(&mut ci, fragment(30000, 1, b"old"))).is_none());
        assert!(whole(add_fragment(&mut ci, fragment(65534, 1, b"x"))).is_none());
        assert!(whole(add_fragment(&mut ci, fragment(65535, 2, b"y"))).is_none());
        assert_eq!(whole(add_fragment(&mut ci, fragment(0, 0, b"z"))), Some((0, b"xyz".to_vec(), 3)));
        assert!(!ci.packet_fragments.contains_key(&30000), "far behind: given up on");
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

    /// A CONNECT on a fresh connection's signature and session is a resend when it's the
    /// same, and a new game's sign-in when it carries another ticket: that one replaces the
    /// old connection instead of getting its answer.
    #[test]
    fn a_new_ticket_on_a_fresh_connections_session_replaces_it() {
        let ctx = Context::splinter_cell_blacklist();
        let logger = Logger::root(slog::Discard, o!());
        let mut server = test_server(&ctx);
        let from = SocketAddr::from(([127, 0, 0, 1], 50003));
        let sig = server.cookies.signature(from, 9, 0);
        let connect = |payload: &[u8]| QPacket {
            payload: payload.to_vec(),
            ..handshake(PacketType::Connect, sig, 9)
        };
        server.handle_connect(&logger, connect(b"first ticket"), from);
        server.handle_connect(&logger, connect(b"first ticket"), from);
        assert_eq!(server.client_registry.clients.len(), 1);
        assert_eq!(
            server.client_registry.clients[&sig].borrow().connect_request.as_deref(),
            Some(&b"first ticket"[..]),
            "a resend changes nothing"
        );

        server.handle_connect(&logger, connect(b"second ticket"), from);
        assert_eq!(server.client_registry.clients.len(), 1);
        assert_eq!(
            server.client_registry.clients[&sig].borrow().connect_request.as_deref(),
            Some(&b"second ticket"[..]),
            "the new game's connection"
        );
        assert_eq!(server.client_registry.connections_from(from.ip()), 1);
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

    /// Connections that haven't signed in: a few per address, and not for long, however
    /// busy they keep.
    #[test]
    fn anonymous_connections_are_capped_per_address_and_expire() {
        let ctx = Context::splinter_cell_blacklist();
        let logger = Logger::root(slog::Discard, o!());
        let mut server = test_server(&ctx);
        let connect = |server: &mut Server<'_, fn(ClientInfo<()>), fn(ClientInfo<()>), ()>, port: u16| {
            let from = SocketAddr::from(([10, 1, 2, 3], port));
            let sig = server.cookies.signature(from, 1, 0);
            server.handle_connect(&logger, handshake(PacketType::Connect, sig, 1), from);
            sig
        };
        let first = connect(&mut server, 40000);
        for port in 40001..40100 {
            connect(&mut server, port);
        }
        assert_eq!(server.client_registry.clients.len(), MAX_ANONYMOUS_PER_IP);
        // Busy, but past its lifetime without signing in.
        {
            let mut ci = server.client_registry.clients[&first].borrow_mut();
            ci.connected = Instant::now() - ANONYMOUS_LIFETIME - Duration::from_secs(1);
            ci.handled.push_back((1, vec![]));
        }
        server.clear_clients();
        assert_eq!(server.client_registry.clients.len(), MAX_ANONYMOUS_PER_IP - 1);
        assert_eq!(
            server.client_registry.anonymous_from(Ipv4Addr::new(10, 1, 2, 3).into()),
            (MAX_ANONYMOUS_PER_IP - 1, MAX_ANONYMOUS_PER_IP - 1)
        );
        // One signing in no longer counts.
        let other = *server.client_registry.clients.keys().next().unwrap();
        server.client_registry.clients[&other].borrow_mut().user_id = Some(1234);
        server.client_registry.settle(other);
        assert_eq!(server.client_registry.anonymous_from(Ipv4Addr::new(10, 1, 2, 3).into()).0, MAX_ANONYMOUS_PER_IP - 2);
        connect(&mut server, 41000);
        connect(&mut server, 41001);
        assert_eq!(server.client_registry.clients.len(), MAX_ANONYMOUS_PER_IP + 1);
    }

    /// Calls from connections that haven't signed in: a burst, then the rate, per address.
    #[test]
    fn anonymous_calls_are_limited_per_address() {
        let ctx = Context::splinter_cell_blacklist();
        let mut server = test_server(&ctx);
        let ip = std::net::IpAddr::from(Ipv4Addr::new(10, 9, 9, 9));
        let allowed = (0..1000).filter(|_| server.anonymous_call_allowed(ip)).count();
        assert!((ANONYMOUS_CALL_BURST as usize..ANONYMOUS_CALL_BURST as usize + 5).contains(&allowed), "{allowed}");
        assert!(server.anonymous_call_allowed(Ipv4Addr::new(10, 9, 9, 10).into()), "another address has its own");
    }

    /// The secure service drops data from a connection that came without a ticket, unread,
    /// and it doesn't keep the connection alive.
    #[test]
    fn data_without_a_sign_in_is_dropped_where_one_is_required() {
        let ctx = Context::splinter_cell_blacklist();
        let logger = Logger::root(slog::Discard, o!());
        for required in [true, false] {
            let mut server = test_server(&ctx);
            server.sign_in_required = required;
            let from = SocketAddr::from(([10, 1, 2, 4], 40000));
            let sig = server.cookies.signature(from, 1, 0);
            server.handle_connect(&logger, handshake(PacketType::Connect, sig, 1), from);
            let data = QPacket {
                packet_type: PacketType::Data,
                signature: sig,
                sequence: 1,
                fragment_id: Some(0),
                payload: vec![1, 2, 3],
                ..Default::default()
            };
            server.handle_data(&logger, data, from);
            assert_eq!(server.client_registry.clients[&sig].borrow().handled.is_empty(), required);
        }
    }

    /// Before signing in, little of a message in fragments is kept.
    #[test]
    fn anonymous_fragments_are_capped() {
        let mut anonymous = client(None, 3078);
        let mut signed_in = client(Some(7), 3079);
        let part = vec![0; 900];
        let mut too_much = false;
        for seq in 1..20u16 {
            let fid = u8::try_from(seq).unwrap();
            too_much |= matches!(add_fragment(&mut anonymous, fragment(seq, fid, &part)), Assembled::TooMuch);
            assert!(matches!(add_fragment(&mut signed_in, fragment(seq, fid, &part)), Assembled::Waiting));
        }
        assert!(too_much, "over 8 KiB before signing in");
    }
}
