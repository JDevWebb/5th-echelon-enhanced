//! Stops voice chat's sound-device callback from reading a headset output or microphone that was
//! freed without being closed (a crash when a sound device changes state, e.g. a headset plugged
//! in or out: `Blacklist_DX11_game.exe+0xfa96`, called from `MMDevAPI.dll`).
//!
//! Storm's voice chat registers an `IMMNotificationClient`; its `OnDeviceStateChanged` runs on
//! Windows' notification thread and walks the voice owner's four per-user headset outputs
//! (`owner+0x14..+0x20`) and its first microphone (`owner+0x04`), comparing endpoint ids. A
//! local chatter opens the headset output first, which registers it there. When the microphone
//! open (a DirectShow graph) or the encoder init then fails, the chatter isn't marked open, so its
//! teardown is skipped and the objects are only deleted. The output's destructor neither takes it
//! off the owner nor releases its XAudio2 engine (only its close does), so the owner keeps a
//! pointer to freed memory until the game exits, and the next sound-device state change reads it.
//! The microphone is the same when the encoder init failed after it opened.
//!
//! The fix, by replacing three vtable slots in `.rdata`:
//! - the output's and the microphone's deleting destructors first run the game's own teardown
//!   for them when they're still marked open (`[this+8]`), which closes them: the owner's slot is
//!   cleared and XAudio2 released, as on the normal path; then the game's destructor;
//! - the callback first clears any owner slot whose object isn't readable memory carrying the
//!   output's (or microphone's) vtable (a destroyed one carries its base class's), then runs the
//!   game's callback.
//!
//! Each function is matched by a signature unique in both executables (DX11 output destructor VA
//! `0x2000bf0`, microphone `0x1ffd4e0`, callback `0x20092f0`; DX9 `0x1fd9700`, `0x1fd5ff0`,
//! `0x1fe1e00`), and the vtables are read from the destructors and checked against them; the fix
//! is left out whole when anything doesn't match.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use tracing::info;
use tracing::warn;

type Delete = unsafe extern "thiscall" fn(*mut u8, u32) -> *mut u8;
type Deinit = unsafe extern "thiscall" fn(*mut u8, *mut u32) -> *mut u32;
type Callback = unsafe extern "system" fn(*mut u8, *const u16, u32) -> i32;

/// The output's destructor (it installs the output's vtable, whose immediate is at +11).
#[rustfmt::skip]
const PLAYBACK_DTOR: [Option<u8>; 42] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x51), Some(0x53), Some(0x56), Some(0x57), Some(0x8B), Some(0xF9),
    Some(0xC7), Some(0x07), None, None, None, None, // mov [edi],<vtable>
    Some(0xC7), Some(0x47), Some(0x2C), None, None, None, None, // mov [edi+0x2c],<vtable 2>
    Some(0x8B), Some(0x87), Some(0x80), Some(0x00), Some(0x00), Some(0x00), Some(0x8B), Some(0x50), Some(0x10),
    Some(0x6A), Some(0x01), Some(0x8D), Some(0x8F), Some(0x80), Some(0x00), Some(0x00), Some(0x00),
    Some(0xC7), Some(0x47), Some(0x7C),
];
const PLAYBACK_VTABLE_AT: usize = 11;

/// The microphone's destructor (its vtable's immediate is at +17).
#[rustfmt::skip]
const CAPTURE_DTOR: [Option<u8>; 21] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x51), Some(0x53), Some(0x56), Some(0x8B), Some(0xF1), Some(0x57),
    Some(0x8D), Some(0x8E), Some(0x04), Some(0x26), Some(0x00), Some(0x00), // lea ecx,[esi+0x2604]
    Some(0xC7), Some(0x06), None, None, None, None, // mov [esi],<vtable>
];
const CAPTURE_VTABLE_AT: usize = 17;

/// The output's teardown: `if (opened) { opened = 0; …; close }`, `thiscall(this, result*)`.
#[rustfmt::skip]
const PLAYBACK_DEINIT: [Option<u8>; 36] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x83), Some(0xEC), Some(0x08), Some(0x53), Some(0x8B), Some(0xD9),
    Some(0x80), Some(0x7B), Some(0x08), Some(0x00), Some(0x56), Some(0x89), Some(0x5D), Some(0xF8), // cmp byte [ebx+8],0
    Some(0x0F), Some(0x84), None, None, None, None,
    Some(0x8B), Some(0x73), Some(0x0C), Some(0x57), Some(0x8D), Some(0x7B), Some(0x0C), Some(0xC6), Some(0x43), Some(0x08), Some(0x00), // mov byte [ebx+8],0
    Some(0x3B), Some(0xF7),
];

/// The generic device teardown (the microphone's): `if (opened) { opened = 0; vtable[5] close }`.
#[rustfmt::skip]
const GENERIC_DEINIT: [Option<u8>; 34] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x51), Some(0x56), Some(0x8B), Some(0xF1),
    Some(0x80), Some(0x7E), Some(0x08), Some(0x00), Some(0x74), Some(0x4B), // cmp byte [esi+8],0 / jz
    Some(0x8B), Some(0x06), Some(0x8B), Some(0x50), Some(0x14), Some(0x8D), Some(0x4D), Some(0xFC), Some(0x51), Some(0x8B), Some(0xCE),
    Some(0xC6), Some(0x46), Some(0x08), Some(0x00), Some(0xFF), Some(0xD2), // mov byte [esi+8],0 / call [vtable+0x14]
    Some(0x8D), Some(0x4D), Some(0xFC), Some(0xE8),
];

/// `OnDeviceStateChanged(this, id, state)`: builds a string of `id`, then looks it up among the
/// owner's outputs (`lea ecx,[esi-0x2c]`) and sets the match's availability (`[eax+0x58]`).
#[rustfmt::skip]
const CALLBACK: [Option<u8>; 56] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x83), Some(0xEC), Some(0x14), Some(0x53), Some(0x56), Some(0x8B), Some(0x75), Some(0x08),
    Some(0x57), Some(0x8B), Some(0x7D), Some(0x0C), Some(0x83), Some(0xEC), Some(0x10), Some(0x8B), Some(0xCC), Some(0x33), Some(0xDB),
    Some(0x57), Some(0x89), Some(0x5D), Some(0xFC), Some(0xE8), None, None, None, None,
    Some(0x8D), Some(0x4E), Some(0xD4), Some(0xE8), None, None, None, None,
    Some(0x85), Some(0xC0), Some(0x74), Some(0x0A), Some(0x83), Some(0x7D), Some(0x10), Some(0x01),
    Some(0x0F), Some(0x94), Some(0xC1), Some(0x88), Some(0x48), Some(0x58), Some(0x8B), Some(0x76), Some(0xD8),
];

/// The callback's `this` is the owner's notification client, at `owner+0x2c`.
const CLIENT_IN_OWNER: usize = 0x2c;
/// The owner's microphone (the first) and its four outputs.
const OWNER_CAPTURE: usize = 0x04;
const OWNER_PLAYBACKS: usize = 0x14;

static PLAYBACK_VTABLE: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_VTABLE: AtomicUsize = AtomicUsize::new(0);
static PLAYBACK_DELETE: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_DELETE: AtomicUsize = AtomicUsize::new(0);
static PLAYBACK_TEARDOWN: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_TEARDOWN: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// Devices closed in their destructor, and dead owner slots cleared, since the game started.
static CLOSED: AtomicU32 = AtomicU32::new(0);
static CLEARED: AtomicU32 = AtomicU32::new(0);

/// Whether `object` is readable memory whose first word is `vtable`.
fn is_object(object: usize, vtable: usize) -> bool {
    if object % 4 != 0 {
        return false;
    }
    match region::query(object as *const u8) {
        // The whole word must lie in the readable region `query` found.
        Ok(r) if r.is_committed() && !r.is_guarded() && r.is_readable() && r.as_range().contains(&(object + 3)) => unsafe { std::ptr::read(object as *const usize) == vtable },
        _ => false,
    }
}

/// Runs the game's teardown for `this` if it's still marked open; whether it was.
unsafe fn close_if_open(this: *mut u8, teardown: &AtomicUsize, what: &str) -> bool {
    if *this.add(8) == 0 {
        return false;
    }
    let teardown: Deinit = std::mem::transmute(teardown.load(Ordering::Relaxed));
    let mut result = 0u32;
    teardown(this, &mut result);
    let n = CLOSED.fetch_add(1, Ordering::Relaxed) + 1;
    info!("voice-device-fix: closed a voice {what} that was being deleted while still open ({this:p}; result {result:#x}; {n} so far): a voice chatter's setup had failed");
    true
}

unsafe extern "thiscall" fn playback_delete(this: *mut u8, flags: u32) -> *mut u8 {
    close_if_open(this, &PLAYBACK_TEARDOWN, "headset output");
    let original: Delete = std::mem::transmute(PLAYBACK_DELETE.load(Ordering::Relaxed));
    original(this, flags)
}

unsafe extern "thiscall" fn capture_delete(this: *mut u8, flags: u32) -> *mut u8 {
    close_if_open(this, &CAPTURE_TEARDOWN, "microphone");
    let original: Delete = std::mem::transmute(CAPTURE_DELETE.load(Ordering::Relaxed));
    original(this, flags)
}

/// Clears `owner`'s slot at `offset` when it points at something that isn't a live object with
/// `vtable`.
unsafe fn clear_if_dead(owner: *mut u8, offset: usize, vtable: usize, what: &str) {
    let slot = owner.add(offset).cast::<usize>();
    let object = std::ptr::read_volatile(slot);
    if object != 0 && !is_object(object, vtable) {
        std::ptr::write_volatile(slot, 0);
        let n = CLEARED.fetch_add(1, Ordering::Relaxed) + 1;
        warn!("voice-device-fix: cleared a dead voice {what} ({object:#010x}) before a sound device change reached it ({n} so far): the game would have read freed memory");
    }
}

unsafe extern "system" fn on_state_changed(this: *mut u8, id: *const u16, state: u32) -> i32 {
    let owner = this.sub(CLIENT_IN_OWNER);
    let playback = PLAYBACK_VTABLE.load(Ordering::Relaxed);
    for i in 0..4 {
        clear_if_dead(owner, OWNER_PLAYBACKS + 4 * i, playback, "headset output");
    }
    clear_if_dead(owner, OWNER_CAPTURE, CAPTURE_VTABLE.load(Ordering::Relaxed), "microphone");
    let original: Callback = std::mem::transmute(CALLBACK_ORIGINAL.load(Ordering::Relaxed));
    original(this, id, state)
}

/// The destructor's address, if `found` is one and the vtable's slot 0 (the deleting destructor,
/// `push ebp / mov ebp,esp / push esi / mov esi,ecx / call <dtor>`) calls it: (vtable, slot 0).
unsafe fn checked_vtable(code: &[u8], dtor: usize, vtable_at: usize, rdata: &std::ops::Range<usize>) -> Option<(usize, usize)> {
    let dtor_va = code.as_ptr() as usize + dtor;
    let vtable = std::ptr::read_unaligned((dtor_va + vtable_at) as *const u32) as usize;
    if !rdata.contains(&vtable) {
        return None;
    }
    let delete = std::ptr::read(vtable as *const u32) as usize;
    let text = code.as_ptr() as usize..code.as_ptr() as usize + code.len();
    if !text.contains(&delete) || !text.contains(&(delete + 10)) {
        return None;
    }
    let head = std::slice::from_raw_parts(delete as *const u8, 11);
    if head[..7] != [0x55, 0x8B, 0xEC, 0x56, 0x8B, 0xF1, 0xE8] {
        return None;
    }
    let called = (delete + 11).wrapping_add_signed(i32::from_le_bytes(head[7..11].try_into().unwrap()) as isize);
    (called == dtor_va).then_some((vtable, delete))
}

/// The one `.rdata` word that holds `value`.
fn word_holding(rdata: &[u8], value: usize) -> Option<usize> {
    #[allow(clippy::cast_possible_truncation)]
    let bytes = (value as u32).to_le_bytes();
    let hits: Vec<usize> = (0..rdata.len().saturating_sub(3)).step_by(4).filter(|&i| rdata[i..i + 4] == bytes).collect();
    match hits[..] {
        [at] => Some(rdata.as_ptr() as usize + at),
        _ => None,
    }
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_voice_devices() {
    let (Some(code), Some(rdata)) = (crate::game_code(), crate::game_section(b".rdata")) else {
        warn!("voice-device-fix: the game's sections weren't found -> skipped");
        return;
    };
    let rdata_range = rdata.as_ptr() as usize..rdata.as_ptr() as usize + rdata.len();
    let base = code.as_ptr() as usize;
    let find = |what: &str, signature: &[Option<u8>]| match crate::find_unique(code, signature) {
        Ok(at) => Some(at),
        Err(n) => {
            warn!("voice-device-fix: {what} found {n} times, not once -> skipped (unknown build?)");
            None
        }
    };
    let (Some(playback_dtor), Some(capture_dtor), Some(playback_deinit), Some(generic_deinit), Some(callback)) = (
        find("the headset output's destructor", &PLAYBACK_DTOR),
        find("the microphone's destructor", &CAPTURE_DTOR),
        find("the headset output's teardown", &PLAYBACK_DEINIT),
        find("the device teardown", &GENERIC_DEINIT),
        find("the sound-device callback", &CALLBACK),
    ) else {
        return;
    };
    let Some((playback_vtable, playback_delete_orig)) = checked_vtable(code, playback_dtor, PLAYBACK_VTABLE_AT, &rdata_range) else {
        warn!("voice-device-fix: the headset output's vtable didn't check out -> skipped");
        return;
    };
    let Some((capture_vtable, capture_delete_orig)) = checked_vtable(code, capture_dtor, CAPTURE_VTABLE_AT, &rdata_range) else {
        warn!("voice-device-fix: the microphone's vtable didn't check out -> skipped");
        return;
    };
    let Some(callback_slot) = word_holding(rdata, base + callback) else {
        warn!("voice-device-fix: the sound-device callback's vtable slot wasn't found once -> skipped");
        return;
    };

    PLAYBACK_VTABLE.store(playback_vtable, Ordering::Relaxed);
    CAPTURE_VTABLE.store(capture_vtable, Ordering::Relaxed);
    PLAYBACK_DELETE.store(playback_delete_orig, Ordering::Relaxed);
    CAPTURE_DELETE.store(capture_delete_orig, Ordering::Relaxed);
    PLAYBACK_TEARDOWN.store(base + playback_deinit, Ordering::Relaxed);
    CAPTURE_TEARDOWN.store(base + generic_deinit, Ordering::Relaxed);
    CALLBACK_ORIGINAL.store(base + callback, Ordering::Relaxed);
    let playback_delete_fn: Delete = playback_delete;
    let capture_delete_fn: Delete = capture_delete;
    let callback_fn: Callback = on_state_changed;
    crate::writemem(playback_vtable as *mut u8, &(playback_delete_fn as usize as u32).to_le_bytes());
    crate::writemem(capture_vtable as *mut u8, &(capture_delete_fn as usize as u32).to_le_bytes());
    crate::writemem(callback_slot as *mut u8, &(callback_fn as usize as u32).to_le_bytes());
    info!("voice-device-fix: voice devices close before they're deleted, and the sound-device callback skips dead ones (vtables {playback_vtable:#010x}, {capture_vtable:#010x}; callback slot {callback_slot:#010x})");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_one_word_holding_a_value() {
        let mut rdata = vec![0u8; 64];
        rdata[16..20].copy_from_slice(&0x0200_92f0u32.to_le_bytes());
        assert_eq!(word_holding(&rdata, 0x0200_92f0), Some(rdata.as_ptr() as usize + 16));
        rdata[40..44].copy_from_slice(&0x0200_92f0u32.to_le_bytes());
        assert_eq!(word_holding(&rdata, 0x0200_92f0), None, "twice");
        assert_eq!(word_holding(&rdata, 0x1234), None);
    }

    #[test]
    fn knows_a_live_object_by_its_vtable() {
        let object = Box::new([0x02ce_9130usize, 0, 0]);
        let at = object.as_ptr() as usize;
        assert!(is_object(at, 0x02ce_9130));
        assert!(!is_object(at, 0x02ce_90cc), "a destroyed one has its base class's vtable");
        assert!(!is_object(at + 1, 0x02ce_9130), "misaligned");
        assert!(!is_object(0x10, 0x02ce_9130), "unmapped");
    }
}
