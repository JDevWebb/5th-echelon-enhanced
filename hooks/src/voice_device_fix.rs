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

use hooks_addresses::crash_fixes::voice_device;
use tracing::info;
use tracing::warn;

type Delete = unsafe extern "thiscall" fn(*mut u8, u32) -> *mut u8;
type Deinit = unsafe extern "thiscall" fn(*mut u8, *mut u32) -> *mut u32;
type Callback = unsafe extern "system" fn(*mut u8, *const u16, u32) -> i32;

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

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_voice_devices() {
    let Some(image) = crate::game_image() else {
        warn!("voice-device-fix: the game's sections weren't found -> skipped");
        return;
    };
    let voice_device::Sites {
        playback_vtable,
        playback_delete: playback_delete_orig,
        capture_vtable,
        capture_delete: capture_delete_orig,
        playback_teardown,
        capture_teardown,
        callback,
        callback_slot,
    } = match voice_device::locate(&image) {
        Ok(sites) => sites,
        Err(what) => {
            warn!("voice-device-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };

    PLAYBACK_VTABLE.store(playback_vtable, Ordering::Relaxed);
    CAPTURE_VTABLE.store(capture_vtable, Ordering::Relaxed);
    PLAYBACK_DELETE.store(playback_delete_orig, Ordering::Relaxed);
    CAPTURE_DELETE.store(capture_delete_orig, Ordering::Relaxed);
    PLAYBACK_TEARDOWN.store(playback_teardown, Ordering::Relaxed);
    CAPTURE_TEARDOWN.store(capture_teardown, Ordering::Relaxed);
    CALLBACK_ORIGINAL.store(callback, Ordering::Relaxed);
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
    fn knows_a_live_object_by_its_vtable() {
        let object = Box::new([0x02ce_9130usize, 0, 0]);
        let at = object.as_ptr() as usize;
        assert!(is_object(at, 0x02ce_9130));
        assert!(!is_object(at, 0x02ce_90cc), "a destroyed one has its base class's vtable");
        assert!(!is_object(at + 1, 0x02ce_9130), "misaligned");
        assert!(!is_object(0x10, 0x02ce_9130), "unmapped");
    }
}
