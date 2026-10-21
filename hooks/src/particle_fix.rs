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

use tracing::info;
use tracing::warn;

/// `lea eax,[eax*4+counts] / push eax / lea ecx,[ebp-4] / push ecx / lea eax,[edx*4+streams] /
/// push eax / push <fill> / push 0 / call <enqueue>`; `None` is a wildcard.
#[rustfmt::skip]
const SIGNATURE: [Option<u8>; 32] = [
    Some(0x8D), Some(0x04), Some(0x85), None, None, None, None, // lea eax,[eax*4+counts]
    Some(0x50), // push eax
    Some(0x8D), Some(0x4D), Some(0xFC), // lea ecx,[ebp-4]
    Some(0x51), // push ecx
    Some(0x8D), Some(0x04), Some(0x95), None, None, None, None, // lea eax,[edx*4+streams]
    Some(0x50), // push eax
    Some(0x68), None, None, None, None, // push <fill>
    Some(0x6A), Some(0x00), // push 0
    Some(0xE8), None, None, None, None, // call <enqueue>
];

/// Where `&streams[0]` and the fill's address sit in the signature.
const STREAMS_AT: usize = 15;
const FILL_AT: usize = 21;

/// The fill's first bytes: `push ebp / mov ebp,esp / push ecx / push ebx / push edi /
/// mov edi,[ebp+8] / xor ebx,ebx / cmp edi,ebx / jz`.
const FILL_PROLOGUE: [u8; 15] = [0x55, 0x8B, 0xEC, 0x51, 0x53, 0x57, 0x8B, 0x7D, 0x08, 0x33, 0xDB, 0x3B, 0xFB, 0x0F, 0x84];

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

/// Where the signature is, if it's found exactly once.
fn find(code: &[u8]) -> Result<usize, usize> {
    let matches = |w: &[u8]| w.iter().zip(SIGNATURE).all(|(b, want)| want.is_none_or(|v| *b == v));
    let hits: Vec<usize> = code.windows(SIGNATURE.len()).enumerate().filter(|(_, w)| matches(w)).map(|(i, _)| i).collect();
    match hits[..] {
        [at] => Ok(at),
        _ => Err(hits.len()),
    }
}

/// Installs the guard. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_particle_fills() {
    let Some(code) = crate::game_code() else {
        warn!("particle-fix: the game's .text wasn't found -> skipped");
        return;
    };
    let at = match find(code) {
        Ok(at) => at,
        Err(n) => {
            warn!("particle-fix: the particle fill's queueing was found {n} times, not once -> skipped (unknown build?)");
            return;
        }
    };
    let site = code.as_ptr().add(at);
    let streams = std::ptr::read_unaligned(site.add(STREAMS_AT).cast::<u32>()) as usize;
    let fill = std::ptr::read_unaligned(site.add(FILL_AT).cast::<u32>()) as usize;
    let text = code.as_ptr() as usize..code.as_ptr() as usize + code.len();
    if !text.contains(&fill) || !text.contains(&(fill + FILL_PROLOGUE.len() - 1)) {
        warn!("particle-fix: the queued function {fill:#010x} isn't in the game's code -> skipped");
        return;
    }
    if std::slice::from_raw_parts(fill as *const u8, FILL_PROLOGUE.len()) != FILL_PROLOGUE {
        warn!("particle-fix: the queued function {fill:#010x} isn't the particle fill -> skipped");
        return;
    }
    let Some((base, size)) = crate::module_bounds() else {
        warn!("particle-fix: module info unavailable -> skipped");
        return;
    };
    if streams < base || streams + 8 > base + size {
        warn!("particle-fix: the vertex streams {streams:#010x} aren't in the game's image -> skipped");
        return;
    }
    FILL.store(fill, Ordering::Relaxed);
    STREAMS.store(streams, Ordering::Relaxed);
    let guard = guarded_fill as Fill as usize as u32;
    crate::writemem(site.add(FILL_AT).cast_mut(), &guard.to_le_bytes());
    info!("particle-fix: particle fills ({fill:#010x}) now check their vertex stream (streams {streams:#010x})");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_site_once() {
        // DX11: the queueing in the particle thread's batcher.
        let site = [
            0x8D, 0x04, 0x85, 0xEC, 0xEE, 0x3E, 0x03, 0x50, 0x8D, 0x4D, 0xFC, 0x51, 0x8D, 0x04, 0x95, 0xC4, 0xEE, 0x3E, 0x03, 0x50, 0x68, 0xC0, 0xA9, 0x61, 0x01, 0x6A, 0x00, 0xE8,
            0x97, 0xFC, 0xFF, 0xFF,
        ];
        let mut code = vec![0xCC; 5];
        code.extend_from_slice(&site);
        assert_eq!(find(&code), Ok(5));
        assert_eq!(u32::from_le_bytes(code[5 + STREAMS_AT..5 + STREAMS_AT + 4].try_into().unwrap()), 0x033e_eec4);
        assert_eq!(u32::from_le_bytes(code[5 + FILL_AT..5 + FILL_AT + 4].try_into().unwrap()), 0x0161_a9c0);
        code.extend_from_slice(&site);
        assert_eq!(find(&code), Err(2));
    }
}
