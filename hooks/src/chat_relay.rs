//! Passes a player's chat in a match on to the other players (`ChatRelay`, on by default; only
//! the host's game acts, on DX9 and DX11).
//!
//! The game sends a chat line from the host to every player, but from anyone else only to the
//! host: a replica's message goes to its object's master, and the match manager's master is the
//! host. The host shows the line and passes it on to nobody, so in a match of three or more the
//! other players never see it (the original game did the same). Seen here with three players: a
//! line from either of the others, to all or to the team, reached the host only.
//!
//! The fix hooks the match manager's chat display (found by signature,
//! `hooks_addresses::chat_relay`). On the host it shows the line as before, then sends the same
//! message on to every other player in the session but its sender, one at a time with the game's
//! own master unicast (as the host sends kill rewards). The receivers show it with their own
//! display and team filter, so team chat stays team chat, and the sender, who showed the line
//! when sending it, gets no second copy. No new message and nothing other players need.
//!
//! The message received is the one sent on: the game's own chat send hands a message on its
//! stack to the send and destroys it right after, so a send has written what it needs by the time
//! it returns, and the display only reads the message. Lines the game itself wouldn't send (empty,
//! longer than its 0x90 characters) aren't passed on, every read of the game's memory is checked
//! first, and the log names peers only by number, never a name or a line. Each sender's lines go
//! on at most 4 in 2 s (`chat_relay::RateLimit`); more still show on the host, as the game shows
//! them, but go no further, so one player can't make the host flood everyone else.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::PoisonError;
use std::time::Instant;

use hooks_addresses::chat_relay;
use hooks_addresses::chat_relay::Rate;
use hooks_addresses::chat_relay::RateLimit;
use hooks_addresses::chat_relay::Sites;
use hooks_addresses::chat_relay::Skip;
use retour::static_detour;
use tracing::debug;
use tracing::info;
use tracing::warn;

static_detour! {
    static ChatDisplayHook: unsafe extern "thiscall" fn(*mut c_void, *mut c_void);
}

static SITES: OnceLock<Sites> = OnceLock::new();
/// Each sender's recent lines passed on, so no one player can make the host flood the others.
static LIMIT: Mutex<RateLimit> = Mutex::new(RateLimit::new());
/// The clock the limit counts by.
static STARTED: OnceLock<Instant> = OnceLock::new();

/// The chat display, `thiscall(manager, message)`.
type Display = unsafe extern "thiscall" fn(*mut c_void, *mut c_void);

/// The match manager's role: 0 on the master (the host), 1 on a replica.
const MANAGER_REPLICA: usize = 0x14;
/// The match manager's handle, which every send of its messages passes.
const MANAGER_HANDLE: usize = 0x6c;
/// A received message's sender (the host is 0, the others count from 1 as they joined).
const MESSAGE_SENDER: usize = 0x04;
/// The chat message's line: a string buffer (references, length, capacity, then the characters).
const MESSAGE_TEXT: usize = 0x40;

/// Whether `len` bytes at `at` can be read.
fn readable(at: usize, len: usize) -> bool {
    at >= 0x1_0000 && region::query_range(at as *const u8, len).is_ok_and(|mut r| r.all(|r| r.is_ok_and(|r| r.is_readable())))
}

fn word(at: usize) -> Option<u32> {
    // SAFETY: checked readable just before.
    readable(at, 4).then(|| unsafe { std::ptr::read_unaligned(at as *const u32) })
}

/// A pointer in the game's memory, if it's set.
fn pointer(at: usize) -> Option<usize> {
    word(at).filter(|p| *p != 0).map(|p| p as usize)
}

/// The peers the session lists (a ring of nodes from `head`: next at `+4`, the entry at `+8`,
/// whose first word is the peer), or `None` when it can't be read or is longer than a match.
fn peers(head: usize) -> Option<Vec<u32>> {
    let mut peers = Vec::new();
    let mut node = pointer(head + 4)?;
    while node != head {
        if peers.len() == chat_relay::MAX_PEERS {
            return None;
        }
        peers.push(word(pointer(node + 8)?)?);
        node = pointer(node + 4)?;
    }
    Some(peers)
}

/// The detour: the game's display first, then the relay.
fn display(manager: *mut c_void, message: *mut c_void) {
    unsafe { ChatDisplayHook.call(manager, message) };
    relay(manager, message);
}

fn relay(manager: *mut c_void, message: *mut c_void) {
    let Some(sites) = SITES.get() else { return };
    let (at_manager, at_message) = (manager as usize, message as usize);
    // Only the master passes lines on; a replica's are on their way to it.
    if word(at_manager + MANAGER_REPLICA) != Some(0) {
        return;
    }
    let Some(sender) = word(at_message + MESSAGE_SENDER) else {
        return;
    };
    let fits = pointer(at_message + MESSAGE_TEXT)
        .and_then(|buf| Some((word(buf + 4)?, word(buf + 8)?)))
        .is_some_and(|(len, cap)| chat_relay::text_fits(len, cap));
    if !fits {
        debug!("chat-relay: a line from peer {sender} is empty or longer than the game sends -> not passed on");
        return;
    }
    let session = pointer(at_manager + chat_relay::SESSION);
    let own = session.and_then(|s| word(s + sites.own_peer));
    let listed = session.and_then(|s| peers(s + sites.peers));
    let (Some(own), Some(listed)) = (own, listed) else {
        debug!("chat-relay: the session's peers couldn't be read -> a line from peer {sender} not passed on");
        return;
    };
    match chat_relay::relay_targets(sender, own, &listed) {
        Ok(targets) if targets.is_empty() => {}
        Ok(targets) => {
            let now = STARTED.get_or_init(Instant::now).elapsed().as_millis() as u64;
            match LIMIT.lock().unwrap_or_else(PoisonError::into_inner).check(sender, now) {
                Rate::Pass => {}
                Rate::FirstDropped => {
                    debug!(
                        "chat-relay: peer {sender} sent more than {} lines in {} ms -> its lines not passed on until it slows down",
                        chat_relay::RATE_LINES,
                        chat_relay::RATE_WINDOW_MS
                    );
                    return;
                }
                Rate::Dropped => return,
            }
            let sent = unsafe { send(sites, manager, message, &targets) };
            debug!("chat-relay: a line from peer {sender} passed on to peers {targets:?} ({sent} sent)");
        }
        Err(Skip::FromHost) => debug!("chat-relay: a line from peer {sender} is the host's own -> not passed on"),
        Err(Skip::UnknownSender) => debug!("chat-relay: peer {sender} isn't in the session ({listed:?}) -> its line not passed on"),
    }
}

/// The game's master unicast: `(result, handle, peer, mode, message, 0, no peer, channel)`.
type Unicast = unsafe extern "C" fn(*mut u32, u32, u32, *const [u32; 2], *mut c_void, u32, u32, u32, u32) -> *mut u32;

/// Sends `message` from the master to each of `targets`, as the game's master unicasts do; how
/// many sends were made.
unsafe fn send(sites: &Sites, manager: *mut c_void, message: *mut c_void, targets: &[u32]) -> usize {
    let (Some(handle), Some(low), Some(high)) = (word(manager as usize + MANAGER_HANDLE), word(sites.channel), word(sites.channel + 4)) else {
        return 0;
    };
    let mode_of: unsafe extern "thiscall" fn(*mut c_void, *mut [u32; 2]) = std::mem::transmute(sites.mode);
    let unicast: Unicast = std::mem::transmute(sites.unicast);
    let release: unsafe extern "thiscall" fn(*mut u32) = std::mem::transmute(sites.release);
    let mut mode = [0u32; 2];
    mode_of(message, &raw mut mode);
    for &peer in targets {
        let mut result = 0u32;
        unicast(&raw mut result, handle, peer, &raw const mode, message, 0, chat_relay::NO_PEER, low, high);
        release(&raw mut result);
    }
    targets.len()
}

/// Installs the relay. Safe to call once at startup; leaves an unrecognised build untouched.
pub unsafe fn install() {
    let Some(image) = crate::game_image() else {
        warn!("chat-relay: the game's sections weren't found -> skipped");
        return;
    };
    let sites = match chat_relay::locate(&image) {
        Ok(sites) => sites,
        Err(what) => {
            warn!("chat-relay: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let _ = SITES.set(sites);
    if let Err(e) = ChatDisplayHook
        .initialize(std::mem::transmute::<usize, Display>(sites.display), display)
        .and_then(|h| h.enable())
    {
        warn!("chat-relay: the detour failed ({e:?}) -> skipped");
        return;
    }
    info!(
        "chat-relay: a player's chat in a match goes on to the others when this game hosts (display @ {:#010x}, unicast {:#010x})",
        sites.display, sites.unicast
    );
}
