//! Stops a crash when the DirectX 11 game loses the focus (Alt+Tab, a click on another window, a
//! notification): `Blacklist_DX11_game.exe+0x20d9685`, writing past a page, every time on a
//! Surface Pro 6 (Intel UHD 620, 2026-11-12); DirectX 9 and the overlay off made no difference to
//! DirectX 11, and DirectX 9 hasn't this code.
//!
//! The game uploads its video's planes (four textures picked by index, Bink's Y, colour and alpha;
//! the crashing copy's source was all `0x10`, video black) on a render worker (DX11 `0x16dd370`):
//! it maps the texture, then copies `min(rows, height)` rows with its `memcpy` (`0x24c24f0`), in
//! one go when the source's pitch and the mapping's are the same, else row by row, never checking
//! that the mapping holds that many. When the window loses the focus the texture is mapped smaller
//! than the video's height still asks for, at least with Intel's driver, and the copy runs off the
//! mapping's end into a page that isn't there.
//!
//! The fix: both `call memcpy`s (DX11 `0x16dd406`, `0x16dd446`) call [`guarded_copy`] instead,
//! which copies as far as the destination's memory is there and writable (`VirtualQuery`, walked
//! region by region) and no further. A row is usually inside the span found for the row before
//! it, so a copy within it, further on than the last, isn't asked again: one query per upload.
//! What's left out would have been written past the texture: the picture loses nothing it could
//! have shown. The copies cut short are counted, and a thread logs the count when it changes.
//!
//! The sites are matched by a signature unique in the DX11 executable (`hooks-addresses`'
//! `texture_upload`), both calls checked to go to the same function; the build is left alone
//! otherwise. DirectX 9 uploads its video another way and is left alone.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::crash_fixes::texture_upload;
use hooks_addresses::crash_fixes::texture_upload::CALL_LEN;
use tracing::info;
use tracing::warn;

/// Copies cut short since the game started: each would have written past a texture's mapping.
static CUT_SHORT: AtomicU32 = AtomicU32::new(0);

#[repr(C)]
#[derive(Default)]
struct MemoryInfo {
    base: usize,
    allocation_base: usize,
    allocation_protect: u32,
    size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn VirtualQuery(address: *const c_void, info: *mut MemoryInfo, length: usize) -> usize;
}

const MEM_COMMIT: u32 = 0x1000;
const PAGE_GUARD: u32 = 0x100;
const PAGE_NOACCESS: u32 = 0x01;

/// Whether a region's protection lets it be written (whatever its caching modifiers: a mapped
/// texture is often write-combined).
fn writable(protect: u32) -> bool {
    protect & PAGE_GUARD == 0 && protect & PAGE_NOACCESS == 0 && matches!(protect & 0xFF, 0x04 | 0x08 | 0x40 | 0x80)
}

/// How many bytes from `at` (up to `want`) are committed and writable: regions walked while each
/// is, as one mapping can be more than one region.
unsafe fn writable_from(at: usize, want: usize) -> usize {
    let mut have = 0usize;
    while have < want {
        let mut info = MemoryInfo::default();
        if VirtualQuery((at + have) as *const c_void, &mut info, std::mem::size_of::<MemoryInfo>()) == 0 {
            break;
        }
        if info.state != MEM_COMMIT || !writable(info.protect) {
            break;
        }
        let end = info.base.saturating_add(info.size);
        if end <= at + have {
            break;
        }
        have = end - at;
    }
    have.min(want)
}

thread_local! {
    /// The writable span found last on this thread, and where its last copy went: a copy inside
    /// it and further on (the next row of the same upload) needs no query.
    static SPAN: Cell<(usize, usize, usize)> = const { Cell::new((0, 0, 0)) };
}

/// How many of `len` bytes at `dst` can be written.
unsafe fn room(dst: usize, len: usize) -> usize {
    let (start, end, last) = SPAN.with(Cell::get);
    if dst > last && dst >= start && dst.saturating_add(len) <= end {
        SPAN.with(|s| s.set((start, end, dst)));
        return len;
    }
    let have = writable_from(dst, len);
    if have == len {
        // The span as far as it goes: ask for more than this row, so the next rows fit it.
        let far = writable_from(dst, usize::MAX - dst);
        SPAN.with(|s| s.set((dst, dst + far.max(len), dst)));
    } else {
        SPAN.with(|s| s.set((0, 0, 0)));
    }
    have
}

/// The game's `memcpy` (cdecl: the caller pops the arguments), copying no further than the
/// destination's memory goes.
unsafe extern "C" fn guarded_copy(dst: *mut u8, src: *const u8, len: usize) -> *mut u8 {
    if len == 0 || dst.is_null() || src.is_null() {
        return dst;
    }
    let n = room(dst as usize, len);
    if n < len {
        CUT_SHORT.fetch_add(1, Ordering::Relaxed);
    }
    if n > 0 {
        std::ptr::copy_nonoverlapping(src, dst, n);
    }
    dst
}

/// Whether the game running is the DirectX 11 executable (the only one with this code).
pub fn expected() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()))
        .is_some_and(|n| n.contains("dx11"))
}

/// Installs the fix. Safe to call once at startup; leaves DirectX 9, and an unrecognised build,
/// untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_texture_uploads() {
    if !expected() {
        info!("texture-upload-fix: not needed with DirectX 9 (its video upload is another code)");
        return;
    }
    let Some(image) = crate::game_image() else {
        warn!("texture-upload-fix: the game's sections weren't found -> skipped");
        return;
    };
    let s = match texture_upload::locate(&image) {
        Ok(s) => s,
        Err(what) => {
            warn!("texture-upload-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let ours = guarded_copy as usize as u32;
    for site in [s.whole, s.rows] {
        let at = site as u32;
        let now = std::slice::from_raw_parts(site as *const u8, CALL_LEN);
        let to = at.wrapping_add(CALL_LEN as u32).wrapping_add(u32::from_le_bytes([now[1], now[2], now[3], now[4]]));
        if now[0] != 0xE8 || to != s.memcpy as u32 {
            warn!("texture-upload-fix: the call at {site:#010x} isn't as found -> stopped");
            return;
        }
    }
    for site in [s.whole, s.rows] {
        let at = site as u32;
        let mut call = [0xE8u8; CALL_LEN];
        call[1..].copy_from_slice(&ours.wrapping_sub(at + CALL_LEN as u32).to_le_bytes());
        if !crate::writemem(site as *mut u8, &call) {
            return;
        }
    }
    crate::fix_stats::installed("texture-upload-fix", &[&CUT_SHORT]);
    info!(
        "texture-upload-fix: a video texture's upload stops at its mapping's end (copies @ {:#010x}, {:#010x}, memcpy {:#010x})",
        s.whole, s.rows, s.memcpy
    );

    std::thread::spawn(|| {
        let mut logged = 0;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let n = CUT_SHORT.load(Ordering::Relaxed);
            if n != logged {
                info!("texture-upload-fix: a video texture's upload was cut short at its mapping's end ({n} so far): the game would have crashed");
                logged = n;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_protections() {
        assert!(writable(0x04), "read-write");
        assert!(writable(0x04 | 0x400), "read-write, write-combined");
        assert!(writable(0x04 | 0x200), "read-write, uncached");
        assert!(writable(0x40), "execute-read-write");
        assert!(!writable(0x02), "read-only");
        assert!(!writable(0x20), "execute-read");
        assert!(!writable(0x04 | PAGE_GUARD), "guard page");
        assert!(!writable(PAGE_NOACCESS));
    }
}
