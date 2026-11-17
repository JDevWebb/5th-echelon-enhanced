//! Names the players Storm couldn't reach, for the server (0.5.5's JC4).
//!
//! A Storm session is a full mesh: every member connects to a newcomer, so a join fails when any
//! one member can't reach the joiner, and the game reports it against the host's room
//! (`0xb08a1a05`, `CONNECTION_FAILED`). On ca1, 10-17 Nov 2026, 148 joins failed that way, and the
//! server could only blame the host. Storm itself knows which peer: the places that give up on one
//! ("Unreachable") all call one report, `Storm::session`'s peer failure, with the peer's
//! `PeerDescriptor` and `CONNECTION_FAILED`'s error pair (hooks-addresses `unreachable`).
//!
//! A detour on that report reads the peer's addresses when the code is `CONNECTION_FAILED` (its
//! other callers pass other codes: a peer leaving, kicked) and calls the game's report as it was.
//! The peers seen are sent to the server a couple of seconds later, a few at a time, from another
//! thread (`api::unreachable_peers`); the server finds who they are and moves a player two
//! others' games couldn't reach to the relay.
//!
//! A `PeerDescriptor` (5ee-research transport.md §5): address A (public) at `+0x10` and address B
//! (local) at `+0x80`, each an object `{vtable, u32 ip, u16 port}`, and the RVCID at `+0x98`. The
//! detour reads them only from committed, readable memory, and only when both address objects
//! have the same vtable, in the game's `.rdata`. The bytes are sent as the game holds them: the
//! server tries both byte orders.

use std::ffi::c_void;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

use hooks_addresses::unreachable;
use retour::static_detour;
use tracing::info;
use tracing::warn;

static_detour! {
    /// `thiscall(session, result*, context, peer*, code*, message*, a6, a7) -> result*`, `ret 0x1c`.
    static ReportHook: unsafe extern "thiscall" fn(*mut c_void, *mut c_void, usize, *const u8, *const u8, usize, usize, usize) -> *mut c_void;
}

/// `CONNECTION_FAILED`'s error pair: the reports with it are the unreachable peers.
static CONNECTION_FAILED: AtomicUsize = AtomicUsize::new(0);
/// The game's `.rdata`, where an address object's vtable is.
static RDATA: AtomicUsize = AtomicUsize::new(0);
static RDATA_END: AtomicUsize = AtomicUsize::new(0);
/// Peers found unreachable since the game started.
static SEEN: AtomicU32 = AtomicU32::new(0);

/// The peers waiting to go to the server, and whether a send is on its way.
static PENDING: Mutex<Vec<server_api::misc::UnreachablePeer>> = Mutex::new(Vec::new());
static SENDING: AtomicBool = AtomicBool::new(false);

/// How long the peers of one failure are gathered before they're sent.
const GATHER: Duration = Duration::from_secs(2);
/// The least time between two sends.
const BETWEEN_SENDS: Duration = Duration::from_secs(10);
/// The most peers held for the next send (the server takes 8).
const MAX_PENDING: usize = 8;

/// Where the parts of a `PeerDescriptor` are.
const ADDRESS_A: usize = 0x10;
const ADDRESS_B: usize = 0x80;
const RVCID: usize = 0x98;
const DESCRIPTOR_SIZE: usize = 0x9c;

/// The peer's addresses and RVCID, if `peer` looks like a live `PeerDescriptor`.
fn read_peer(peer: *const u8, rdata: &std::ops::Range<usize>) -> Option<server_api::misc::UnreachablePeer> {
    let at = peer as usize;
    if !(0x10000..0xffff_0000 - DESCRIPTOR_SIZE).contains(&at) || at % 4 != 0 {
        return None;
    }
    let readable = region::query(peer).is_ok_and(|r| r.is_committed() && !r.is_guarded() && r.is_readable() && r.as_range().contains(&(at + DESCRIPTOR_SIZE - 1)));
    if !readable {
        return None;
    }
    unsafe {
        let u32_at = |off: usize| std::ptr::read_unaligned(peer.add(off).cast::<u32>());
        let u16_at = |off: usize| std::ptr::read_unaligned(peer.add(off).cast::<u16>());
        let (vtable_a, vtable_b) = (u32_at(ADDRESS_A) as usize, u32_at(ADDRESS_B) as usize);
        if vtable_a != vtable_b || !rdata.contains(&vtable_a) {
            return None;
        }
        Some(server_api::misc::UnreachablePeer {
            ip: u32_at(ADDRESS_A + 4),
            port: u32::from(u16_at(ADDRESS_A + 8)),
            local_ip: u32_at(ADDRESS_B + 4),
            local_port: u32::from(u16_at(ADDRESS_B + 8)),
            rvcid: u32_at(RVCID),
        })
    }
}

/// Holds `peer` for the next send, and starts one if none is on its way.
fn note(peer: server_api::misc::UnreachablePeer) {
    {
        let mut pending = PENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.iter().any(|p| p.ip == peer.ip && p.port == peer.port) || pending.len() >= MAX_PENDING {
            return;
        }
        pending.push(peer);
    }
    if SENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let started = std::thread::Builder::new().name("fe-unreachable".into()).spawn(|| {
        std::thread::sleep(GATHER);
        let peers = std::mem::take(&mut *PENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        if !peers.is_empty() {
            crate::api::unreachable_peers(peers);
        }
        std::thread::sleep(BETWEEN_SENDS);
        SENDING.store(false, Ordering::Release);
    });
    if started.is_err() {
        SENDING.store(false, Ordering::Release);
    }
}

#[allow(clippy::too_many_arguments)]
fn report(session: *mut c_void, result: *mut c_void, context: usize, peer: *const u8, code: *const u8, message: usize, a6: usize, a7: usize) -> *mut c_void {
    if code as usize == CONNECTION_FAILED.load(Ordering::Relaxed) {
        let rdata = RDATA.load(Ordering::Relaxed)..RDATA_END.load(Ordering::Relaxed);
        if let Some(p) = read_peer(peer, &rdata) {
            let n = SEEN.fetch_add(1, Ordering::Relaxed) + 1;
            // The first few, then at each power of two.
            if n <= 16 || n.is_power_of_two() {
                info!(
                    "Storm couldn't reach a peer: address A {:#010x}:{}, B {:#010x}:{}, RVCID {} ({n} so far); telling the server",
                    p.ip, p.port, p.local_ip, p.local_port, p.rvcid
                );
            }
            note(p);
        }
    }
    unsafe { ReportHook.call(session, result, context, peer, code, message, a6, a7) }
}

/// Installs the detour. Safe to call once at startup; leaves an unrecognised build untouched.
pub unsafe fn name_unreachable_peers() {
    let Some(image) = crate::game_image() else {
        warn!("unreachable-peers: the game's sections weren't found -> off");
        return;
    };
    let sites = match unreachable::locate(&image) {
        Ok(sites) => sites,
        Err(what) => {
            warn!("unreachable-peers: {what} -> off (unknown build?)");
            return;
        }
    };
    RDATA.store(image.rdata_va, Ordering::Relaxed);
    RDATA_END.store(image.rdata_va + image.rdata.len(), Ordering::Relaxed);
    CONNECTION_FAILED.store(sites.connection_failed, Ordering::Relaxed);
    match ReportHook.initialize(std::mem::transmute(sites.report), report).and_then(|h| h.enable()) {
        Ok(()) => info!(
            "unreachable-peers: Storm's unreachable peers are named to the server (report @ {:#010x}, CONNECTION_FAILED {:#010x})",
            sites.report, sites.connection_failed
        ),
        Err(err) => warn!("unreachable-peers: the report's detour failed: {err:?}"),
    }
}
