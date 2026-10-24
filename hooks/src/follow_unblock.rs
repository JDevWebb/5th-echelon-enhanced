//! Frees a party member's UI facade when a stale follow task blocks the host-loss exit
//! (always on, next to the match lobby migration; the DX11 build only). Background, addresses and the
//! measurement in `docs/architecture/member-stuck-after-private-match.md` (§12.3 "R-B", §13).
//!
//! A second `PartyFollowLeaderTask` for a match the member is already in never gets an answer, so
//! the facade stays in state 7 / phase 2 with that task in its slot and never reads the pending
//! "host lost" error. When that state is stable for two ticks, with the error pending, the leader
//! action consumed (`party+0x414 == 0`) and the slot holding a `PartyFollowLeaderTask`, the slot
//! and the facade's state/phase are reset, so the original tick picks up the error and leaves the
//! match. The task is not freed (its pending goal may still write to it).
//!
//! Every line starts with `follow-unblock:` (or `follow-dup:`). All reads are checked ([`word`]).

use std::ffi::c_void;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Instant;

use retour::static_detour;
use tracing::info;
use tracing::warn;

use crate::addresses::Addresses;
use crate::unblock_logic;

static_detour! {
    static FacadeTickHook: unsafe extern "thiscall" fn(*mut c_void, f32) -> u32;
    static FollowTaskObserverHook: unsafe extern "thiscall" fn(*mut c_void) -> u32;
    static FollowDelegateObserverHook: unsafe extern "thiscall" fn(*mut c_void, u32) -> u32;
}

/// Global pointers: the UI facade, its task runner, `NetOnline`.
const FACADE: u32 = 0x0344_d668;
const FACADE_GATE: u32 = 0x0344_d640;
const NET_ONLINE: u32 = 0x0338_3df8;
const ONLINE_PARTY: u32 = 0x58;
const PARTY_LEADER_ACTION: u32 = 0x414;
const ONLINE_ERROR: u32 = 0x10;
/// The task runner's single task slot.
const GATE_SLOT: u32 = 0x30;

/// `UI::FacadeScreen::vf03`: thiscall(float), `ret 4`.
const FACADE_TICK: u32 = 0x018e_ab60;
const FACADE_TICK_PROLOGUE: [u8; 10] = [0x55, 0x8B, 0xEC, 0x56, 0x8B, 0xF1, 0x83, 0x7E, 0x30, 0x00];
/// `PartyFollowLeaderTask::vf04`: thiscall, no stack args.
const FOLLOW_TASK_VF04: u32 = 0x018e_ae00;
const FOLLOW_TASK_VF04_PROLOGUE: [u8; 11] = [0x55, 0x8B, 0xEC, 0x83, 0xEC, 0x08, 0xA1, 0xF8, 0x3D, 0x38, 0x03];

/// `FollowDelegate`: thiscall, 1 stack argument (the result), `ret 4`.
const FOLLOW_DELEGATE: u32 = 0x0188_6720;
const FOLLOW_DELEGATE_PROLOGUE: [u8; 8] = [0x55, 0x8B, 0xEC, 0x33, 0xC0, 0x89, 0x41, 0x34];

/// Whether `len` bytes at `at` can be read: a stale pointer costs a `?` in the log, not the game.
fn readable(at: u32, len: usize) -> bool {
    at >= 0x1_0000 && region::query_range(at as usize as *const u8, len).is_ok_and(|mut r| r.all(|r| r.is_ok_and(|r| r.is_readable())))
}

fn word(at: u32) -> Option<u32> {
    // SAFETY: checked readable just before.
    readable(at, 4).then(|| unsafe { std::ptr::read_unaligned(at as usize as *const u32) })
}

/// `[[global] + off]`-style pointer follow: the word at `word(base) + off`.
fn field(base: u32, off: u32) -> Option<u32> {
    word(word(base).filter(|p| *p != 0)? + off)
}

fn show_w(v: Option<u32>) -> String {
    v.map_or_else(|| "?".to_string(), |v| format!("{v:#x}"))
}

/// Start of the process clock for [`FOLLOW_DONE_MS`].
static CLOCK: OnceLock<Instant> = OnceLock::new();
/// Milliseconds since [`CLOCK`] (+1) at which a follow ended with result 0; 0 = none yet.
static FOLLOW_DONE_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Records that a `FollowDelegate` finished (called on the game's thread). Only result 0 (success)
/// counts.
fn note_follow_done(result: u32) {
    if result == 0 {
        FOLLOW_DONE_MS.store(now_ms() + 1, Ordering::Relaxed);
    }
}

fn follow_delegate_observer(this: *mut c_void, result: u32) -> u32 {
    note_follow_done(result);
    unsafe { FollowDelegateObserverHook.call(this, result) }
}

/// The vtable of `PartyFollowLeaderTask` (first dword of a task seen at its `vf04`); 0 = not seen.
static TASK_VTABLE: AtomicU32 = AtomicU32::new(0);
/// Consecutive ticks the stuck condition has held.
static TICKS: AtomicU32 = AtomicU32::new(0);

/// Remembers the vtable of a `PartyFollowLeaderTask` (called on the game's thread from its `vf04`).
fn note_task(task: *mut c_void) {
    if let Some(vt) = word(task as usize as u32) {
        if vt != 0 && TASK_VTABLE.swap(vt, Ordering::Relaxed) != vt {
            info!("follow-unblock: PartyFollowLeaderTask vtable {vt:#x}");
        }
    }
}

fn follow_task_observer(this: *mut c_void) -> u32 {
    note_task(this);
    unsafe { FollowTaskObserverHook.call(this) }
}

/// Writes a dword the game owns, after checking it is mapped.
unsafe fn put(at: u32, value: u32) -> bool {
    if !readable(at, 4) {
        return false;
    }
    std::ptr::write_unaligned(at as usize as *mut u32, value);
    true
}

/// H8 (docs §14): right after a follow ended, a `FollowLeader` message sets `party414` again and
/// the idle facade would start a second follow task that never ends. Reset `party414` instead.
fn drop_second_follow(facade: u32) {
    let done = FOLLOW_DONE_MS.load(Ordering::Relaxed);
    let ms = (done != 0).then(|| now_ms().saturating_sub(done - 1));
    if ms.is_none() {
        return;
    }
    let (state34, phase38) = (word(facade + 0x34), word(facade + 0x38));
    let gate = field(FACADE_GATE, GATE_SLOT);
    let Some(party) = field(NET_ONLINE, ONLINE_PARTY).filter(|p| *p != 0) else { return };
    let party414 = word(party + PARTY_LEADER_ACTION);
    if unblock_logic::should_drop_second_follow(state34, phase38, gate, party414, ms) {
        // SAFETY: the address was just read through successfully.
        let ok = unsafe { put(party + PARTY_LEADER_ACTION, 0) };
        FOLLOW_DONE_MS.store(0, Ordering::Relaxed);
        info!(
            "follow-dup: second follow dropped (party414 was {}, first follow ended {} ms ago){}",
            party414.unwrap_or(0),
            ms.unwrap_or(0),
            if ok { "" } else { " - write failed" }
        );
    }
}

/// Set once [`install`] has checked the game; [`before_tick`] does nothing before that (or on a build
/// it skipped, the DX9 one included).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The facade tick (main thread) with our own detour, when nothing else hooks the tick.
fn facade_tick(this: *mut c_void, dt: f32) -> u32 {
    before_tick(this);
    unsafe { FacadeTickHook.call(this, dt) }
}

/// What the facade tick does before the game's own code: release a stale follow task, drop a second
/// follow. The tick function (`0x18eab60`) is hooked once: by `migration_fix` (which calls this first)
/// when the crash fixes are on, else by [`facade_tick`].
pub fn before_tick(this: *mut c_void) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let facade = word(FACADE).unwrap_or(0);
    if facade != 0 && this as usize as u32 == facade {
        drop_second_follow(facade);
    }
    // Only the real facade, and only if we know what a follow task looks like.
    let task_vtable = TASK_VTABLE.load(Ordering::Relaxed);
    if facade != 0 && this as usize as u32 == facade && task_vtable != 0 {
        let state34 = word(facade + 0x34);
        let phase38 = word(facade + 0x38);
        let party = field(NET_ONLINE, ONLINE_PARTY).filter(|p| *p != 0);
        let party414 = party.and_then(|p| word(p + PARTY_LEADER_ACTION));
        let error = field(NET_ONLINE, ONLINE_ERROR).filter(|e| *e != 0);
        let error_cat = error.and_then(|e| word(e + 4));
        let gate = field(FACADE_GATE, GATE_SLOT).filter(|g| *g != 0);
        let gate_vtable = gate.and_then(word);
        let held = unblock_logic::conditions_hold(state34, phase38, party414, error_cat, gate_vtable, task_vtable);
        let ticks = unblock_logic::next_ticks(held, TICKS.load(Ordering::Relaxed));
        TICKS.store(ticks, Ordering::Relaxed);
        if unblock_logic::should_unblock(state34, phase38, party414, error_cat, gate_vtable, task_vtable, ticks) {
            TICKS.store(0, Ordering::Relaxed);
            let slot = word(FACADE_GATE).unwrap_or(0) + GATE_SLOT;
            // SAFETY: both addresses were just read through successfully (the gate object and the facade).
            let done = unsafe { put(slot, 0) && put(facade + 0x34, 0) && put(facade + 0x38, 0) };
            info!(
                "follow-unblock: stale follow task released ({}) mode30={} state34={} phase38={} party414={} error10={} err4={} gate30={} task_vtable={task_vtable:#x} (task left allocated)",
                if done { "ok" } else { "write failed" },
                show_w(word(facade + 0x30)),
                show_w(state34),
                show_w(phase38),
                show_w(party414),
                show_w(error),
                show_w(error_cat),
                show_w(gate),
            );
        }
    }
}

/// Whether the game has exactly `expected` at `at`.
unsafe fn starts_with(at: u32, expected: &[u8]) -> bool {
    readable(at, expected.len()) && std::slice::from_raw_parts(at as usize as *const u8, expected.len()) == expected
}

/// Hooks the facade tick (unless `tick_hooked_elsewhere`: `migration_fix` hooks it and calls
/// [`before_tick`]), the follow task's `vf04` (feeds [`note_task`]) and `FollowDelegate` (feeds
/// [`note_follow_done`]). Called once at startup; skips with a warning when the game isn't what
/// it was read as.
pub unsafe fn install(addr: &Addresses, tick_hooked_elsewhere: bool) {
    // The addresses below are the DX11 build's; the party functions are known for it only.
    if addr.func_party_leader_action.is_none() {
        warn!("follow-unblock: this game version's addresses aren't known (DX9?) -> skipped");
        return;
    }
    // With `tick_hooked_elsewhere` the function already starts with `migration_fix`'s detour jump, so
    // the prologue can't be compared; that hook found the function by the same address.
    if !tick_hooked_elsewhere && !starts_with(FACADE_TICK, &FACADE_TICK_PROLOGUE) {
        warn!("follow-unblock: the facade tick @ {FACADE_TICK:#x} isn't what it was read as -> skipped (unknown build?)");
        return;
    }
    if !starts_with(FOLLOW_TASK_VF04, &FOLLOW_TASK_VF04_PROLOGUE) {
        warn!("follow-unblock: PartyFollowLeaderTask::vf04 @ {FOLLOW_TASK_VF04:#x} isn't what it was read as -> skipped (unknown build?)");
        return;
    }
    crate::hooks::hook!(FollowTaskObserverHook, Some(FOLLOW_TASK_VF04 as usize), follow_task_observer);
    if starts_with(FOLLOW_DELEGATE, &FOLLOW_DELEGATE_PROLOGUE) {
        crate::hooks::hook!(FollowDelegateObserverHook, Some(FOLLOW_DELEGATE as usize), follow_delegate_observer);
    } else {
        warn!("follow-dup: FollowDelegate @ {FOLLOW_DELEGATE:#x} isn't what it was read as -> duplicate-follow guard inactive");
    }
    if !tick_hooked_elsewhere {
        crate::hooks::hook!(FacadeTickHook, Some(FACADE_TICK as usize), facade_tick);
    }
    ACTIVE.store(true, Ordering::Relaxed);
    info!(
        "follow-unblock: a stale follow task is released when the host-loss exit is blocked behind it (facade tick @ {FACADE_TICK:#x}, {})",
        if tick_hooked_elsewhere { "called from migration-fix's tick" } else { "own hook" }
    );
}

pub unsafe fn deinit() {
    ACTIVE.store(false, Ordering::Relaxed);
    let _ = FacadeTickHook.disable();
    let _ = FollowTaskObserverHook.disable();
    let _ = FollowDelegateObserverHook.disable();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_addresses_read_as_none() {
        assert_eq!(word(0), None);
        assert_eq!(field(0x10, 4), None);
        assert_eq!(show_w(None), "?");
    }
}
