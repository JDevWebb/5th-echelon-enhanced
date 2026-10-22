//! Stops a queued particle fill from writing into a vertex stream that a map load has already
//! freed (a crash after leaving a match: `Blacklist_DX11_game.exe+0x127ce09`, text read where the
//! stream's buffer pointer belongs).
//!
//! The particle thread queues `FunctionCall3<void(*)(l3d::VertexStream*, l3d::Particle*, int)>`
//! commands that copy the *value* of `streams[idx]` (two double-buffered globals) when they're
//! queued, and the render runs them a frame or more later. `l3d`'s stream release frees both
//! streams and zeroes the globals without draining those commands; it runs first thing in a map
//! load, in the old level's unload and in a renderer release, while the drain only comes at the
//! end of the load. A frame rendered in between (the game keeps presenting during a load, even
//! minimised) runs the stale fill, which locks a freed stream: a crash, or a silent write into
//! whatever reused the memory.
//!
//! The fill is only ever reached through one `push <fill>` (the command's function pointer). Its
//! immediate is pointed at [`guarded_fill`] instead, which calls the game's fill only when the
//! stream is still one of the two live ones; after a release the globals are 0, so a stale fill is
//! skipped and at most one frame of particle vertices isn't written.
//!
//! The site is matched by a signature unique in both executables (DX11 VA `0x161c169`, DX9 VA
//! `0x18a9b09`), and the fill it pushes must start with the fill's own prologue; a build where
//! either isn't found exactly once is left alone.

use std::ffi::c_void;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use hooks_addresses::crash_fixes::particle;
use tracing::info;
use tracing::warn;

type Fill = unsafe extern "C" fn(*mut c_void, *mut c_void, i32);

/// The game's fill and `&streams[0]`, set once before the immediate is rewritten.
static FILL: AtomicUsize = AtomicUsize::new(0);
static STREAMS: AtomicUsize = AtomicUsize::new(0);
/// Stale fills skipped since the game started.
static SKIPPED: AtomicU32 = AtomicU32::new(0);

/// Runs in place of the game's fill, with its arguments (cdecl, so the caller pops them).
unsafe extern "C" fn guarded_fill(stream: *mut c_void, particles: *mut c_void, count: i32) {
    let streams = STREAMS.load(Ordering::Relaxed) as *const *mut c_void;
    let live = |i| std::ptr::read_volatile(streams.add(i));
    if stream.is_null() || (stream != live(0) && stream != live(1)) {
        let n = SKIPPED.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n.is_power_of_two() {
            info!(
                "particle-fix: skipped a particle fill for a released vertex stream ({stream:p}; live {:p}, {:p}; {n} so far)",
                live(0),
                live(1)
            );
        }
        return;
    }
    let fill: Fill = std::mem::transmute(FILL.load(Ordering::Relaxed));
    fill(stream, particles, count);
}

/// Installs the guard. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_particle_fills() {
    let Some(image) = crate::game_image() else {
        warn!("particle-fix: the game's sections weren't found -> skipped");
        return;
    };
    let particle::Site { fill_slot, fill, streams } = match particle::locate(&image) {
        Ok(site) => site,
        Err(what) => {
            warn!("particle-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    FILL.store(fill, Ordering::Relaxed);
    STREAMS.store(streams, Ordering::Relaxed);
    let guard = guarded_fill as Fill as usize as u32;
    crate::writemem(fill_slot as *mut u8, &guard.to_le_bytes());
    info!("particle-fix: particle fills ({fill:#010x}) now check their vertex stream (streams {streams:#010x})");
}
