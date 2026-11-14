//! The game's Bink videos (bink2w32.dll, by its exports, the same in both builds): which are
//! open, and pausing the title's background when the quick start leaves the title.
//!
//! The title's "music" is the soundtrack of its background video, PRESS_START_BG.bik (a 30 s
//! loop). In a normal start the travel to the hub after Start ends it; the quick start holds
//! that travel back, so the video played on behind the online menus and its soundtrack looped
//! through the lobby and on (Cloud, 13 Nov; recorded on the gamebox, 14 Nov, DX9 and DX11).

use std::ffi::c_void;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use retour::static_detour;
use tracing::info;
use tracing::warn;
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::System::LibraryLoader::GetProcAddress;

static_detour! {
    static BinkOpenHook: unsafe extern "system" fn(*const c_void, u32) -> usize;
    static BinkCloseHook: unsafe extern "system" fn(usize);
}

type Pause = unsafe extern "system" fn(usize, i32) -> i32;

/// `BINKFROMMEMORY`: the name is the video in memory, not a file name.
const FROM_MEMORY: u32 = 0x0400_0000;

/// The videos open now: (handle, name, or "" for one opened from memory).
static OPEN: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
static PAUSE: std::sync::OnceLock<Pause> = std::sync::OnceLock::new();
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Hooks BinkOpen and BinkClose, if bink2w32.dll is loaded (the game imports it).
pub fn init() {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    // SAFETY: looking up a module the game imports, and its exports.
    unsafe {
        let Ok(dll) = GetModuleHandleA(windows::core::s!("bink2w32.dll")) else {
            info!("Videos: bink2w32.dll isn't loaded: the title's background isn't paused");
            return;
        };
        let (Some(open), Some(close), Some(pause)) = (
            GetProcAddress(dll, windows::core::s!("_BinkOpen@8")),
            GetProcAddress(dll, windows::core::s!("_BinkClose@4")),
            GetProcAddress(dll, windows::core::s!("_BinkPause@8")),
        ) else {
            warn!("Videos: Bink's exports weren't found: the title's background isn't paused");
            return;
        };
        let _ = PAUSE.set(std::mem::transmute::<_, Pause>(pause));
        let hooked = BinkOpenHook
            .initialize(std::mem::transmute(open), bink_open)
            .and_then(|h| h.enable())
            .and_then(|()| BinkCloseHook.initialize(std::mem::transmute(close), bink_close))
            .and_then(|h| h.enable());
        match hooked {
            Ok(()) => info!("Videos: BinkOpen and BinkClose hooked"),
            Err(e) => warn!("Videos: couldn't hook Bink ({e:?}): the title's background isn't paused"),
        }
    }
}

fn bink_open(name: *const c_void, flags: u32) -> usize {
    // SAFETY: the original, unchanged.
    let handle = unsafe { BinkOpenHook.call(name, flags) };
    let text = if flags & FROM_MEMORY != 0 || name.is_null() {
        String::new()
    } else {
        crate::dataversion::read_string(name as usize, 260).unwrap_or_default()
    };
    info!(
        "Videos: opened {:?} (flags {flags:#x}) -> {handle:#x}",
        if text.is_empty() { "(from memory)" } else { text.as_str() }
    );
    if handle != 0 {
        if let Ok(mut open) = OPEN.lock() {
            if open.len() < 64 {
                open.push((handle, text));
            }
        }
    }
    handle
}

fn bink_close(handle: usize) {
    if let Ok(mut open) = OPEN.lock() {
        open.retain(|(h, _)| *h != handle);
    }
    // SAFETY: the original, unchanged.
    unsafe { BinkCloseHook.call(handle) }
}

/// Pauses the title's background (PRESS_START_BG.bik), if it's open. What was paused, for the log.
pub fn pause_title_background() -> Vec<String> {
    let Some(pause) = PAUSE.get() else { return Vec::new() };
    let Ok(open) = OPEN.lock() else { return Vec::new() };
    let mut paused = Vec::new();
    for (handle, name) in open.iter() {
        if name.to_ascii_uppercase().contains("PRESS_START_BG") {
            // SAFETY: a handle BinkOpen gave and BinkClose hasn't taken back.
            unsafe { pause(*handle, 1) };
            paused.push(name.clone());
        }
    }
    if paused.is_empty() {
        info!(
            "Videos: the title's background isn't open; open: {:?}",
            open.iter().map(|(h, n)| format!("{h:#x} {n}")).collect::<Vec<_>>()
        );
    }
    paused
}
