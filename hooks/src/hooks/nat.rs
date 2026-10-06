//! Playing over the internet without a VPN.
//!
//! Matches run peer to peer on Storm's socket (UDP 13000), and the game
//! advertises its local address, which only works on a LAN. To learn its
//! public address, the game sends a Quazal NAT echo that the server never
//! answered, so it gave up and stayed local.
//!
//! This module:
//! - probes the server's NAT helper **from Storm's socket** (so the helper
//!   sees that socket's public mapping), keeps probing as a keepalive, and
//!   compares the mapping seen on a second port to spot symmetric NATs;
//! - answers the game's NAT echo itself, by handing the game's own reply
//!   parser the address to advertise, so the game advertises it everywhere
//!   (its station URLs, its peer ids and its NAT probes);
//! - wraps game packets for relay addresses (and all of them, for a relayed
//!   player) for the helper, and unwraps relayed packets so the game sees
//!   them coming from the sender's advertised address;
//! - asks the router to forward the port (`portmap`).
//!
//! The protocol is in the `nat_proto` crate; the server side is
//! `dedicated_server/src/nat_helper.rs`.

use std::cell::RefCell;
use std::ffi::c_void;
use std::net::Ipv4Addr;
use std::net::SocketAddrV4;
use std::net::ToSocketAddrs;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use nat_proto::probe_flags;
use nat_proto::reply_flags;
use nat_proto::Message;
use retour::static_detour;
use tracing::info;
use tracing::warn;
use windows::core::s;
use windows::Win32::System::LibraryLoader::GetProcAddress;
use windows::Win32::System::LibraryLoader::LoadLibraryA;

use crate::addresses::Addresses;
use crate::config::Config;
use crate::config::Hook;
use crate::config::NatMode;

static_detour! {
    static BindHook: unsafe extern "system" fn(usize, *const u8, i32) -> i32;
    static CloseSocketHook: unsafe extern "system" fn(usize) -> i32;
    static SendToHook: unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32;
    static RecvFromHook: unsafe extern "system" fn(usize, *mut u8, i32, i32, *mut u8, *mut i32) -> i32;
    static EchoSendHook: unsafe extern "thiscall" fn(*mut c_void);
}

/// The game's NAT reply parser: `this` = the NAT engine, the sender's
/// address object, the message, its length.
type ParseFn = unsafe extern "thiscall" fn(*mut c_void, *mut c_void, *const u8, u32);

const AF_INET: u16 = 2;
const PROBE_INTERVAL: Duration = Duration::from_millis(500);
const KEEPALIVE: Duration = Duration::from_secs(20);
/// How long to wait for the second port's answer before trusting the first.
const DETECT_WAIT: Duration = Duration::from_millis(1500);

/// No socket yet.
const NO_SOCKET: usize = usize::MAX;
static STORM_SOCKET: AtomicUsize = AtomicUsize::new(NO_SOCKET);
/// Other sockets the game bound on the Storm port before the latest. The game
/// binds two there, one right after the other, and its traffic to other
/// players can leave on either: both must be wrapped for the relay and
/// unwrapped from it, or a player joining a relayed host sends to the relay's
/// address unwrapped, and nothing there answers.
static OTHER_STORM_SOCKETS: [AtomicUsize; 4] = [const { AtomicUsize::new(NO_SOCKET) }; 4];
/// Said once: game traffic seen on a Storm socket other than the latest.
static OTHER_SOCKET_USED: AtomicBool = AtomicBool::new(false);

/// Whether `s` is one of the game's sockets on the Storm port.
fn is_storm_socket(s: usize) -> bool {
    s != NO_SOCKET && (s == STORM_SOCKET.load(Ordering::Relaxed) || OTHER_STORM_SOCKETS.iter().any(|o| o.load(Ordering::Relaxed) == s))
}

/// Notes that the game used a Storm socket other than the latest (once).
fn note_other_socket(s: usize) {
    if s != STORM_SOCKET.load(Ordering::Relaxed) && !OTHER_SOCKET_USED.swap(true, Ordering::Relaxed) {
        info!("NAT: game traffic on the earlier Storm socket too; it goes through the relay as well");
    }
}
static LOG_PACKETS: AtomicBool = AtomicBool::new(false);
static PARSE: OnceLock<usize> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
struct Reply {
    observed: SocketAddrV4,
    advertise: SocketAddrV4,
    relayed: bool,
    relay_ip: Ipv4Addr,
    relay_ports: (u16, u16),
}

#[derive(Debug, Default)]
struct State {
    mode: Option<NatMode>,
    name: String,
    server: Option<SocketAddrV4>,
    second: Option<SocketAddrV4>,
    reply: Option<Reply>,
    first_reply_at: Option<Instant>,
    second_observed: Option<SocketAddrV4>,
    symmetric: bool,
    mapping: Option<SocketAddrV4>,
    last_probe: Option<Instant>,
    nonce: u32,
    told_game: Option<SocketAddrV4>,
    echo_requests: u32,
    /// The ticket for our name, from signing in (zeros: none yet).
    ticket: nat_proto::Ticket,
    /// The last cookie the server gave us, echoed in the next probe.
    cookie: nat_proto::Cookie,
    /// Our registration's tag: relayed data carries it both ways.
    tag: nat_proto::Tag,
    /// Nonces of recent probes: only replies to these are believed.
    sent: [u32; 4],
    /// The helper's last answer, and whether its silence since was said (the server's
    /// admins see these lines: a game the relay stops knowing can't be joined).
    last_answer: Option<Instant>,
    silence_said: bool,
    /// Whether sending the probes failed (said once until they go again).
    send_failing: bool,
    /// The round trip to the server (see [`round_trip`]), and when it was last measured.
    rtt_ms: Option<u16>,
    rtt_at: Option<Instant>,
    /// When the game signed in (took its ticket), and when the helper last forgot this game:
    /// registering normally takes a second, so only after [`GRACE`] from either is not being
    /// registered a problem.
    signed_in_at: Option<Instant>,
    lost_at: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    mode: None,
    name: String::new(),
    server: None,
    second: None,
    reply: None,
    first_reply_at: None,
    second_observed: None,
    symmetric: false,
    mapping: None,
    last_probe: None,
    nonce: 0,
    told_game: None,
    echo_requests: 0,
    ticket: [0; 16],
    cookie: [0; 16],
    tag: [0; 8],
    sent: [0; 4],
    last_answer: None,
    silence_said: false,
    send_failing: false,
    rtt_ms: None,
    rtt_at: None,
    signed_in_at: None,
    lost_at: None,
});

/// The helper not answering this long (three keepalives) is worth a line: it forgets a game
/// after 90 s without a probe.
const SILENCE: Duration = Duration::from_secs(60);

/// How long a signed-in game may go unregistered before the player is told nobody can reach
/// them (it registers within a second or two when the helper's packets get through).
const GRACE: Duration = Duration::from_secs(30);

/// Why other players can't reach this game: their matches can't connect to it, nor its to
/// theirs through the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreachable {
    /// The server's name doesn't resolve.
    NoServer,
    /// Windows won't send the probes.
    CantSend,
    /// The helper doesn't answer (or stopped answering): something blocks UDP to it.
    NoAnswer,
    /// The helper answers but hasn't taken this game.
    NotRegistered,
}

/// Why other players can't reach this game, while they can't; None when they can, or when
/// it isn't online (no Storm socket, not signed in) or NAT traversal is off.
pub fn unreachable() -> Option<Unreachable> {
    unreachable_at(&state(), STORM_SOCKET.load(Ordering::Relaxed) != NO_SOCKET, Instant::now())
}

fn unreachable_at(st: &State, socket_open: bool, now: Instant) -> Option<Unreachable> {
    if st.mode.is_none_or(|m| m == NatMode::Off) || !socket_open || st.ticket == [0; 16] {
        return None;
    }
    let since = |t: Option<Instant>| t.map(|t| now.saturating_duration_since(t));
    let quiet = since(st.last_answer);
    if st.tag != [0; 8] {
        // Registered: until the helper stops answering (it forgets the game 30 s later).
        return quiet.is_some_and(|q| q >= SILENCE).then_some(Unreachable::NoAnswer);
    }
    let unregistered = since(st.signed_in_at.max(st.lost_at)).unwrap_or_default();
    if unregistered < GRACE {
        return None;
    }
    Some(if st.server.is_none() {
        Unreachable::NoServer
    } else if st.send_failing {
        Unreachable::CantSend
    } else if quiet.is_none_or(|q| q >= GRACE) {
        Unreachable::NoAnswer
    } else {
        Unreachable::NotRegistered
    })
}

/// Whether NAT traversal is off (LAN or VPN only): players elsewhere can't join this PC.
pub fn is_off() -> bool {
    state().mode == Some(NatMode::Off)
}

/// The NAT helper's port, for telling the player what to let through.
static HELPER_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(nat_proto::DEFAULT_PORT);

pub fn helper_port() -> u16 {
    HELPER_PORT.load(Ordering::Relaxed)
}

/// How long since the server's NAT helper last answered this game, if it ever did.
pub fn quiet_for() -> Option<Duration> {
    state().last_answer.map(|t| t.elapsed())
}

/// Takes the ticket the server gave at sign-in (it names this account).
pub fn set_ticket(ticket: &[u8]) {
    if let Ok(ticket) = <nat_proto::Ticket>::try_from(ticket) {
        let mut st = state();
        if st.ticket != ticket {
            st.ticket = ticket;
            st.signed_in_at = Some(Instant::now());
            // Register again with it.
            st.last_probe = None;
        }
    }
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One line for the overlay: how other players reach this PC.
pub fn status() -> Option<String> {
    let st = state();
    let mode = st.mode?;
    if mode == NatMode::Off {
        return Some("Local address only (NAT traversal is off)".into());
    }
    let unreachable = unreachable_at(&st, STORM_SOCKET.load(Ordering::Relaxed) != NO_SOCKET, Instant::now());
    if let Some(why) = unreachable {
        return Some(
            match why {
                Unreachable::NoServer => "Nobody: the server's NAT helper can't be found",
                Unreachable::CantSend => "Nobody: this PC can't send to the server's NAT helper",
                Unreachable::NoAnswer => "Nobody: the server's NAT helper doesn't hear from this PC",
                Unreachable::NotRegistered => "Nobody yet: the server hasn't registered this game",
            }
            .into(),
        );
    }
    let Some(r) = st.reply else {
        return Some(
            if st.server.is_some() {
                "Asking the server for this PC's public address..."
            } else {
                "The server's NAT helper isn't known"
            }
            .into(),
        );
    };
    let ping = st.rtt_ms.map(|ms| format!(" · {ms} ms to the server")).unwrap_or_default();
    let line = if r.relayed {
        format!("Through the server's relay ({})", r.advertise)
    } else if st.mapping.is_some_and(|m| m == r.advertise) {
        format!("Direct, router port opened ({})", r.advertise)
    } else if nat_proto::is_private(*r.advertise.ip()) {
        format!("Direct, local network ({})", r.advertise)
    } else {
        // No port opened on the router: other players may not get through to this PC,
        // which only shows when someone tries to join a match it hosts.
        format!(
            "Direct, no router port open ({}){}: others may not be able to join you",
            r.advertise,
            if st.symmetric { ", strict NAT" } else { "" }
        )
    };
    Some(line + &ping)
}

/// The router's port mapping for the Storm port, from `portmap`.
pub(crate) fn set_mapping(mapping: Option<SocketAddrV4>) {
    let mut st = state();
    if st.mapping != mapping {
        st.mapping = mapping;
        // Tell the helper straight away.
        st.last_probe = None;
    }
}

fn read_addr(sa: *const u8) -> Option<SocketAddrV4> {
    if sa.is_null() {
        return None;
    }
    // SOCKADDR_IN: family (u16, native), port (big-endian), address, zero.
    let bytes = unsafe { std::slice::from_raw_parts(sa, 8) };
    if u16::from_ne_bytes([bytes[0], bytes[1]]) != AF_INET {
        return None;
    }
    Some(SocketAddrV4::new(
        Ipv4Addr::new(bytes[4], bytes[5], bytes[6], bytes[7]),
        u16::from_be_bytes([bytes[2], bytes[3]]),
    ))
}

fn sockaddr(addr: SocketAddrV4) -> [u8; 16] {
    let mut sa = [0u8; 16];
    sa[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    sa[2..4].copy_from_slice(&addr.port().to_be_bytes());
    sa[4..8].copy_from_slice(&addr.ip().octets());
    sa
}

fn send_raw(socket: usize, data: &[u8], to: SocketAddrV4) -> i32 {
    let sa = sockaddr(to);
    unsafe { SendToHook.call(socket, data.as_ptr(), data.len() as i32, 0, sa.as_ptr(), sa.len() as i32) }
}

fn is_relay_address(r: &Reply, addr: SocketAddrV4) -> bool {
    *addr.ip() == r.relay_ip && (r.relay_ports.0..=r.relay_ports.1).contains(&addr.port())
}

fn to_hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

thread_local! {
    static WRAP: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(2048));
}

fn bind(s: usize, name: *const u8, namelen: i32) -> i32 {
    let res = unsafe { BindHook.call(s, name, namelen) };
    if res == 0 {
        if let Some(addr) = read_addr(name) {
            if addr.port() == nat_proto::STORM_PORT {
                let old = STORM_SOCKET.swap(s, Ordering::SeqCst);
                if old != s && old != NO_SOCKET {
                    // Kept, newest first: the oldest of five drops out.
                    let mut carry = old;
                    for slot in &OTHER_STORM_SOCKETS {
                        carry = slot.swap(carry, Ordering::SeqCst);
                        if carry == NO_SOCKET || carry == s {
                            break;
                        }
                    }
                }
                if old != s {
                    info!("NAT: Storm socket bound on {addr}");
                    let mut st = state();
                    st.reply = None;
                    st.first_reply_at = None;
                    st.second_observed = None;
                    st.symmetric = false;
                    st.last_probe = None;
                    // Online again: registering takes a moment, not a warning.
                    st.lost_at = Some(Instant::now());
                }
            }
        }
    }
    res
}

/// The game closes its Storm socket when it goes offline (no party left: back to the main
/// menu) and when it quits, and binds a new one when it goes online again. Tell the helper
/// it's gone, so the server forgets the registration quietly (not as a game it lost), and
/// stop probing: nobody can join a game that's offline, and that's nothing to warn about.
fn closesocket(s: usize) -> i32 {
    if s != NO_SOCKET && is_storm_socket(s) {
        storm_socket_closing(s);
    }
    unsafe { CloseSocketHook.call(s) }
}

fn storm_socket_closing(s: usize) {
    for slot in &OTHER_STORM_SOCKETS {
        let _ = slot.compare_exchange(s, NO_SOCKET, Ordering::SeqCst, Ordering::SeqCst);
    }
    if STORM_SOCKET.load(Ordering::SeqCst) != s {
        return;
    }
    // While it's still open: from the address the helper knows, with the tag it gave.
    let bye = {
        let st = state();
        st.server.filter(|_| st.tag != [0; 8]).map(|server| (server, Message::Bye { tag: st.tag }.encode()))
    };
    if let Some((server, data)) = bye {
        send_raw(s, &data, server);
    }
    // The game binds two sockets; one it keeps (should it) carries on, registering anew.
    let next = OTHER_STORM_SOCKETS.iter().map(|o| o.swap(NO_SOCKET, Ordering::SeqCst)).find(|o| *o != NO_SOCKET);
    STORM_SOCKET.store(next.unwrap_or(NO_SOCKET), Ordering::SeqCst);
    let mut st = state();
    if next.is_none() && st.reply.is_some() {
        info!("NAT: the game closed its Storm socket (offline until it goes online again)");
    }
    forget_registration(&mut st);
}

/// Back to before the helper first answered: the next socket registers from scratch.
fn forget_registration(st: &mut State) {
    st.reply = None;
    st.first_reply_at = None;
    st.second_observed = None;
    st.symmetric = false;
    st.last_probe = None;
    st.tag = [0; 8];
    st.last_answer = None;
    st.silence_said = false;
    st.send_failing = false;
    st.told_game = None;
    st.echo_requests = 0;
    st.lost_at = Some(Instant::now());
}

fn sendto(s: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, tolen: i32) -> i32 {
    if is_storm_socket(s) && !buf.is_null() && len > 0 {
        let data = unsafe { std::slice::from_raw_parts(buf, len as usize) };
        let dest = read_addr(to);
        if LOG_PACKETS.load(Ordering::Relaxed) {
            info!("sendto {dest:?}: {}", to_hex(data));
        }
        if let Some(dest) = dest {
            let relay = {
                let st = state();
                match (st.reply, st.server) {
                    (Some(r), Some(server)) if dest != server && (r.relayed || is_relay_address(&r, dest)) => Some((server, st.tag)),
                    _ => None,
                }
            };
            if let Some((server, tag)) = relay {
                note_other_socket(s);
                if data.len() > nat_proto::MAX_PAYLOAD {
                    // Dropped, not sent directly: that would give away the address the relay
                    // hides.
                    warn!("NAT: a {}-byte game packet is too big for the relay; dropped", data.len());
                    return len;
                }
                let res = WRAP.with(|w| {
                    let mut w = w.borrow_mut();
                    nat_proto::encode_data_to(&mut w, tag, dest, data);
                    send_raw(s, &w, server)
                });
                return if res < 0 { res } else { len };
            }
        }
    }
    unsafe { SendToHook.call(s, buf, len, flags, to, tolen) }
}

/// The longest relayed packet: the largest game packet and the relay's header.
const WRAPPED_MAX: usize = nat_proto::MAX_PAYLOAD + nat_proto::DATA_OVERHEAD;

/// Copies a received packet into the game's buffer of `len` bytes as Winsock
/// would: one that doesn't fit is cut short, and the receive fails with
/// WSAEMSGSIZE.
fn hand_over(packet: &[u8], buf: *mut u8, len: i32) -> i32 {
    const WSAEMSGSIZE: i32 = 10040;
    let room = usize::try_from(len).unwrap_or(0);
    unsafe { std::ptr::copy_nonoverlapping(packet.as_ptr(), buf, packet.len().min(room)) };
    if packet.len() > room {
        unsafe { windows::Win32::Networking::WinSock::WSASetLastError(WSAEMSGSIZE) };
        return -1;
    }
    packet.len() as i32
}

fn recvfrom(s: usize, buf: *mut u8, len: i32, flags: i32, from: *mut u8, fromlen: *mut i32) -> i32 {
    // A peek leaves the packet queued; consuming NAT messages then would
    // loop on the same one.
    const MSG_PEEK: i32 = 2;
    if !is_storm_socket(s) || buf.is_null() || flags & MSG_PEEK != 0 {
        return unsafe { RecvFromHook.call(s, buf, len, flags, from, fromlen) };
    }
    // The sender is needed to trust NAT messages, even if the game doesn't
    // ask for it.
    let mut own_from = [0u8; 16];
    let mut own_len = own_from.len() as i32;
    let (from_ptr, len_ptr) = if from.is_null() || fromlen.is_null() {
        (own_from.as_mut_ptr(), &mut own_len as *mut i32)
    } else {
        (from, fromlen)
    };
    // A relayed packet is longer than the game packet inside it: into a buffer
    // only big enough for the game packet it wouldn't fit. Received here then,
    // and handed over as the game's own receive would.
    let small = usize::try_from(len).map_or(true, |l| l < WRAPPED_MAX);
    let mut scratch = [0u8; WRAPPED_MAX];
    let (into, into_len) = if small { (scratch.as_mut_ptr(), WRAPPED_MAX as i32) } else { (buf, len) };
    loop {
        let n = unsafe { RecvFromHook.call(s, into, into_len, flags, from_ptr, len_ptr) };
        if n <= 0 {
            return n;
        }
        let data = unsafe { std::slice::from_raw_parts_mut(into, n as usize) };
        let sender = read_addr(from_ptr);
        if !nat_proto::is_nat_message(data) {
            if LOG_PACKETS.load(Ordering::Relaxed) {
                info!("recvfrom {sender:?}: {}", to_hex(data));
            }
            return if small { hand_over(data, buf, len) } else { n };
        }
        let (server, second) = {
            let st = state();
            (st.server, st.second)
        };
        if sender.is_none() || (sender != server && sender != second) {
            // Not from our server: never let it reach the game.
            continue;
        }
        if let Some((tag, relayed_from, offset)) = nat_proto::data_from(data) {
            // Only from the server, with our registration's tag: anyone can forge a source
            // address, not the tag.
            if sender != server || state().tag != tag || tag == [0; 8] {
                continue;
            }
            let payload = data.len() - offset;
            if LOG_PACKETS.load(Ordering::Relaxed) {
                info!("recvfrom {relayed_from} (relayed): {}", to_hex(&data[offset..]));
            }
            let n = if small {
                hand_over(&data[offset..], buf, len)
            } else {
                data.copy_within(offset.., 0);
                payload as i32
            };
            if !from.is_null() && !fromlen.is_null() {
                let sa = sockaddr(relayed_from);
                unsafe {
                    std::ptr::copy_nonoverlapping(sa.as_ptr(), from, sa.len().min(*fromlen as usize));
                    *fromlen = sa.len() as i32;
                }
            }
            return n;
        }
        if let Some(Message::ProbeReply {
            nonce,
            observed,
            advertise,
            flags,
            relay_ip,
            relay_ports,
            cookie,
            tag,
        }) = Message::decode(data)
        {
            {
                let mut st = state();
                // Only answers to our own recent probes.
                if nonce == 0 || !st.sent.contains(&nonce) {
                    continue;
                }
                if st.silence_said {
                    let quiet = st.last_answer.map_or(0, |t| t.elapsed().as_secs());
                    info!("NAT: the server's NAT helper answers again (after {quiet} s)");
                    st.silence_said = false;
                }
                st.last_answer = Some(Instant::now());
                if sender != second {
                    if tag == [0; 8] {
                        if st.tag != [0; 8] {
                            info!("NAT: the server's NAT helper no longer knows this game; registering again");
                            st.tag = [0; 8];
                            st.lost_at = Some(Instant::now());
                        }
                        // Not registered yet: come back at once with the cookie.
                        if cookie != [0; 16] && st.ticket != [0; 16] {
                            st.cookie = cookie;
                            st.last_probe = None;
                        }
                        continue;
                    }
                    st.cookie = cookie;
                    st.tag = tag;
                }
            }
            on_reply(sender == second, observed, advertise, flags, relay_ip, relay_ports);
        }
    }
}

fn on_reply(from_second: bool, observed: SocketAddrV4, advertise: SocketAddrV4, flags: u8, relay_ip: Ipv4Addr, relay_ports: (u16, u16)) {
    let mut st = state();
    if from_second {
        if st.second_observed.is_none() {
            st.second_observed = Some(observed);
            if let Some(r) = st.reply {
                if r.observed.port() != observed.port() && !st.symmetric {
                    info!("NAT: the router gives each destination its own port ({} vs {observed}): asking for the relay", r.observed);
                    st.symmetric = true;
                    st.last_probe = None;
                }
            }
        }
        return;
    }
    let reply = Reply {
        observed,
        advertise,
        relayed: flags & reply_flags::RELAYED != 0,
        relay_ip,
        relay_ports,
    };
    let changed = st.reply.is_none_or(|r| r.advertise != reply.advertise || r.relayed != reply.relayed);
    if st.first_reply_at.is_none() {
        st.first_reply_at = Some(Instant::now());
    }
    if let (Some(second), false) = (st.second_observed, st.symmetric) {
        if second.port() != observed.port() {
            st.symmetric = true;
            st.last_probe = None;
        }
    }
    st.reply = Some(reply);
    if changed {
        info!(
            "NAT: this PC is {observed} to the server; advertising {advertise}{}",
            if reply.relayed { " (through the server's relay)" } else { "" }
        );
        if st.told_game.is_some_and(|t| t != advertise) {
            warn!("NAT: the game already advertises {:?}; the server corrects it where it can", st.told_game);
        }
    }
}

/// The address to give the game, once the helper answered and the check
/// for a symmetric NAT is done (or took too long).
fn ready_address(st: &State) -> Option<SocketAddrV4> {
    let r = st.reply?;
    let detect_done = st.second_observed.is_some() || st.symmetric || st.first_reply_at.is_some_and(|t| t.elapsed() > DETECT_WAIT);
    // A symmetric NAT waits for the relay address.
    if st.symmetric && !r.relayed && st.mode != Some(NatMode::Off) && st.last_probe.is_none_or(|t| t.elapsed() < Duration::from_secs(3)) {
        return None;
    }
    detect_done.then_some(r.advertise)
}

fn echo_send(engine: *mut c_void) {
    let (address, first) = {
        let mut st = state();
        st.echo_requests += 1;
        (ready_address(&st), st.echo_requests == 1)
    };
    if first {
        info!("NAT: the game asks for its public address");
    }
    let (Some(address), Some(&parse)) = (address, PARSE.get()) else {
        // Not known (yet): let the game try; it retries every 250 ms.
        unsafe { EchoSendHook.call(engine) };
        return;
    };
    // The reply the game's NAT echo expects: type 2, then the station URL.
    let mut msg = vec![2u8];
    msg.extend_from_slice(nat_proto::station_url(address).as_bytes());
    msg.push(0);
    // The sender's address object; only read for other message types.
    let mut sender = [0u8; 0x100];
    unsafe {
        let parse: ParseFn = std::mem::transmute(parse);
        parse(engine, sender.as_mut_ptr().cast(), msg.as_ptr(), msg.len() as u32);
    }
    let mut st = state();
    if st.told_game != Some(address) {
        info!("NAT: told the game to advertise {address}");
        st.told_game = Some(address);
    }
}

/// The round trip to the server's NAT helper, in ms: the best of three probes that only
/// ask for an address, from a socket of its own. Answers on the Storm socket wait until the
/// game reads them, which would add up to a frame.
fn round_trip(server: SocketAddrV4) -> Option<u16> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
    let mut buf = [0u8; 256];
    let mut best: Option<Duration> = None;
    for _ in 0..3 {
        let nonce = rand::random::<u32>() | 1;
        let probe = Message::Probe {
            flags: 0,
            nonce,
            mapping: None,
            name: String::new(),
            ticket: [0; 16],
            cookie: [0; 16],
            rtt_ms: None,
        }
        .encode();
        let started = Instant::now();
        if socket.send_to(&probe, server).is_err() {
            continue;
        }
        while let Ok((n, from)) = socket.recv_from(&mut buf) {
            if from == std::net::SocketAddr::V4(server) && matches!(Message::decode(&buf[..n]), Some(Message::ProbeReply { nonce: got, .. }) if got == nonce) {
                let took = started.elapsed();
                best = Some(best.map_or(took, |b| b.min(took)));
                break;
            }
            if started.elapsed() > Duration::from_secs(1) {
                break;
            }
        }
    }
    best.map(|b| u16::try_from(b.as_millis()).unwrap_or(u16::MAX).max(1))
}

fn resolve(host: &str, port: u16) -> Option<SocketAddrV4> {
    (host, port).to_socket_addrs().ok()?.find_map(|a| match a {
        std::net::SocketAddr::V4(v4) => Some(v4),
        std::net::SocketAddr::V6(_) => None,
    })
}

/// Probes the helper while Storm's socket is open: every 500 ms until it
/// answers, then every 20 s to keep the router's mapping (and the relay)
/// alive.
fn worker(host: String, port: u16) {
    let mut last_resolve: Option<Instant> = None;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let socket = STORM_SOCKET.load(Ordering::Relaxed);
        if socket == NO_SOCKET {
            continue;
        }
        let needs_resolve = state().server.is_none();
        if needs_resolve {
            if last_resolve.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
                continue;
            }
            last_resolve = Some(Instant::now());
            match resolve(&host, port) {
                Some(server) => {
                    info!("NAT: the server's NAT helper is {server}");
                    let mut st = state();
                    st.server = Some(server);
                    st.second = Some(SocketAddrV4::new(*server.ip(), port.wrapping_add(1)));
                }
                None => {
                    warn!("NAT: can't resolve {host}");
                    continue;
                }
            }
        }
        let probes = {
            let mut st = state();
            let interval = if st.reply.is_some() { KEEPALIVE } else { PROBE_INTERVAL };
            if st.last_probe.is_some_and(|t| t.elapsed() < interval) {
                continue;
            }
            let quiet = st.last_answer.map(|t| t.elapsed());
            if let (Some(quiet), false) = (quiet, st.silence_said) {
                if quiet >= SILENCE {
                    warn!(
                        "NAT: no answer from the server's NAT helper for {} s (probing every {}); it forgets this game after 90 s, and nobody can join it then",
                        quiet.as_secs(),
                        if interval < Duration::from_secs(1) {
                            format!("{} ms", interval.as_millis())
                        } else {
                            format!("{} s", interval.as_secs())
                        }
                    );
                    st.silence_said = true;
                }
            }
            st.last_probe = Some(Instant::now());
            // Random and never 0: whoever can send as the helper (e.g. on the LAN) can't
            // guess the next one, so can't answer for it.
            st.nonce = rand::random::<u32>() | 1;
            let nonce = st.nonce;
            st.sent.rotate_right(1);
            st.sent[0] = nonce;
            let mut flags = 0;
            if st.mode == Some(NatMode::Relay) {
                flags |= probe_flags::WANT_RELAY;
            }
            if st.symmetric {
                flags |= probe_flags::SYMMETRIC;
            }
            if st.mapping.is_some() {
                flags |= probe_flags::HAS_MAPPING;
            }
            let probe = |flags| {
                Message::Probe {
                    flags,
                    nonce: st.nonce,
                    mapping: st.mapping,
                    name: st.name.clone(),
                    ticket: st.ticket,
                    cookie: st.cookie,
                    rtt_ms: st.rtt_ms,
                }
                .encode()
            };
            let mut probes = vec![(probe(flags), st.server)];
            if st.second_observed.is_none() {
                probes.push((probe(flags | probe_flags::SECOND_PORT), st.second));
            }
            probes
        };
        // Measured once registered, as often as the keepalive: the next probe tells the
        // server.
        let measure = {
            let st = state();
            st.server.filter(|_| st.reply.is_some() && st.rtt_at.is_none_or(|t| t.elapsed() >= KEEPALIVE))
        };
        if let Some(server) = measure {
            let rtt = round_trip(server);
            let mut st = state();
            st.rtt_at = Some(Instant::now());
            if rtt.is_some() {
                st.rtt_ms = rtt;
            }
        }
        for (data, to) in probes {
            if let Some(to) = to {
                let sent = send_raw(socket, &data, to);
                // Closed meanwhile (the game went offline, or is quitting): not a failure.
                if sent < 0 && (STORM_SOCKET.load(Ordering::SeqCst) != socket || socket_gone()) {
                    break;
                }
                let mut st = state();
                if sent < 0 && !st.send_failing {
                    warn!("NAT: sending a probe to the server's NAT helper {to} failed ({sent})");
                    st.send_failing = true;
                } else if sent >= 0 && st.send_failing {
                    info!("NAT: probes to the server's NAT helper go again");
                    st.send_failing = false;
                }
            }
        }
    }
}

/// Whether the last failed send was on a socket that's no longer there (closed, or Winsock
/// shut down as the game quits) rather than one Windows wouldn't send on.
fn socket_gone() -> bool {
    use windows::Win32::Networking::WinSock::WSAGetLastError;
    use windows::Win32::Networking::WinSock::WSAENOTSOCK;
    use windows::Win32::Networking::WinSock::WSANOTINITIALISED;
    let e = unsafe { WSAGetLastError() };
    if e == WSAENOTSOCK || e == WSANOTINITIALISED {
        let mut st = state();
        if STORM_SOCKET.swap(NO_SOCKET, Ordering::SeqCst) != NO_SOCKET {
            forget_registration(&mut st);
        }
        return true;
    }
    false
}

pub unsafe fn init_hooks(config: &Config, addr: &Addresses) {
    LOG_PACKETS.store(config.enable_all_hooks || config.enable_hooks.contains(&Hook::StormPackets), Ordering::Relaxed);
    let mode = config.networking.nat;
    {
        let mut st = state();
        st.mode = Some(mode);
        st.name = config.user.username.clone();
    }

    if let Ok(lib) = LoadLibraryA(s!("ws2_32.dll")) {
        super::hook!(BindHook, GetProcAddress(lib, s!("bind")), bind);
        super::hook!(CloseSocketHook, GetProcAddress(lib, s!("closesocket")), closesocket);
        super::hook!(SendToHook, GetProcAddress(lib, s!("sendto")), sendto);
        super::hook!(RecvFromHook, GetProcAddress(lib, s!("recvfrom")), recvfrom);
    }

    if mode == NatMode::Off {
        info!("NAT: off; the game advertises its local address");
        return;
    }
    let Some(host) = config.config_server.clone() else {
        warn!("NAT: no server configured");
        return;
    };
    match (addr.func_nat_echo_send, addr.func_nat_packet_parse) {
        (Some(send), Some(parse)) => {
            let _ = PARSE.set(parse);
            super::hook!(EchoSendHook, Some(send), echo_send);
        }
        _ => warn!("NAT: this game version's NAT functions weren't found; the server corrects the address where it can"),
    }
    let port = config.networking.nat_port.unwrap_or(nat_proto::DEFAULT_PORT);
    HELPER_PORT.store(port, Ordering::Relaxed);
    let _ = std::thread::Builder::new().name("fe-nat".into()).spawn(move || worker(host, port));
    if config.networking.port_mapping {
        // A pinned address is the one to map; otherwise this PC's address towards the
        // router, which the automatic one (towards the server) may not be, behind a VPN.
        super::portmap::start(config.networking.ip_address.filter(|_| !config.networking.ip_address_automatic));
    }
}

pub unsafe fn deinit_hooks() {
    let _ = EchoSendHook.disable();
    let _ = RecvFromHook.disable();
    let _ = SendToHook.disable();
    let _ = CloseSocketHook.disable();
    let _ = BindHook.disable();
    super::portmap::stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn online(now: Instant) -> State {
        State {
            mode: Some(NatMode::Auto),
            server: Some(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), nat_proto::DEFAULT_PORT)),
            ticket: [1; 16],
            signed_in_at: Some(now),
            ..State::default()
        }
    }

    #[test]
    fn a_game_that_registers_is_reachable() {
        let t0 = Instant::now();
        let mut st = online(t0);
        // Signing in: a moment to register.
        assert_eq!(unreachable_at(&st, true, t0 + Duration::from_secs(5)), None);
        st.tag = [7; 8];
        st.last_answer = Some(t0 + Duration::from_secs(1));
        assert_eq!(unreachable_at(&st, true, t0 + Duration::from_secs(50)), None);
        // Not online, or NAT traversal off: nothing to say.
        assert_eq!(unreachable_at(&State::default(), true, t0), None);
        st.mode = Some(NatMode::Off);
        assert_eq!(unreachable_at(&st, true, t0 + Duration::from_secs(600)), None);
    }

    #[test]
    fn a_game_the_helper_never_hears_from_is_unreachable_and_says_why() {
        let t0 = Instant::now();
        let later = t0 + GRACE + Duration::from_secs(1);
        let mut st = online(t0);
        // Not signed in or no Storm socket: not online yet.
        assert_eq!(unreachable_at(&st, false, later), None);
        assert_eq!(unreachable_at(&State { ticket: [0; 16], ..online(t0) }, true, later), None);
        // Probes go out, nothing comes back (a firewall).
        assert_eq!(unreachable_at(&st, true, later), Some(Unreachable::NoAnswer));
        // The helper answers but doesn't take the game.
        st.last_answer = Some(later - Duration::from_secs(1));
        assert_eq!(unreachable_at(&st, true, later), Some(Unreachable::NotRegistered));
        st.send_failing = true;
        assert_eq!(unreachable_at(&st, true, later), Some(Unreachable::CantSend));
        st.server = None;
        assert_eq!(unreachable_at(&st, true, later), Some(Unreachable::NoServer));
    }

    #[test]
    fn a_game_that_goes_offline_and_back_is_never_unreachable_for_it() {
        let t0 = Instant::now();
        let mut st = online(t0);
        st.tag = [7; 8];
        st.last_answer = Some(t0);
        // Leaving its last party, the game closes its socket: offline, nothing to say,
        // however long it stays in the menus.
        forget_registration(&mut st);
        assert_eq!(st.tag, [0; 8]);
        assert_eq!(unreachable_at(&st, false, t0 + Duration::from_secs(600)), None);
        // Online again (a new socket): a moment to register, as at signing in.
        let back = t0 + Duration::from_secs(100);
        st.lost_at = Some(back);
        assert_eq!(unreachable_at(&st, true, back + Duration::from_secs(5)), None);
        assert_eq!(unreachable_at(&st, true, back + GRACE + Duration::from_secs(1)), Some(Unreachable::NoAnswer));
    }

    #[test]
    fn a_registration_that_lapses_is_unreachable() {
        let t0 = Instant::now();
        let mut st = online(t0);
        st.tag = [7; 8];
        st.last_answer = Some(t0);
        // The helper stops answering: three keepalives missed.
        assert_eq!(unreachable_at(&st, true, t0 + SILENCE), Some(Unreachable::NoAnswer));
        // It answers but forgot the game: registering again takes a moment.
        let lost = t0 + Duration::from_secs(100);
        st.tag = [0; 8];
        st.lost_at = Some(lost);
        st.last_answer = Some(lost);
        assert_eq!(unreachable_at(&st, true, lost + Duration::from_secs(2)), None);
        st.last_answer = Some(lost + GRACE);
        assert_eq!(unreachable_at(&st, true, lost + GRACE + Duration::from_secs(1)), Some(Unreachable::NotRegistered));
    }
}
