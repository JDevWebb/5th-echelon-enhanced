//! Beta (`[Beta] MouseTurnPerSecond`, the launcher's Settings › Beta features): mouse turning
//! the same at any frame rate. Above 60 fps the game turns less for the same mouse movement,
//! so players cap their frame rate.
//!
//! What's known (notes: blacklist-re `notes/mouse-turn-fps.md`): every camera mode turns by
//! `input × speed × dt`, so it takes the look input as a rate per second, and the game caps the
//! mouse's at `500 × dt`. That cap only makes sense for a distance per frame, and nothing in
//! the look routine divides it by `dt`. So the mouse's turn per distance shrinks with the frame
//! time: 2.4 times less at 144 fps than at 60. Not traced: where the value is made. If that
//! divided by `dt` already, this would turn faster above 60 fps instead, which testers would
//! see at once; hence a beta.
//!
//! The fix: in the look routine's mouse branch, right after the game's clamp
//! (`hooks_addresses::mouse_turn`), the mouse's X (XMM1, already stored at `[esi+0x470]`) and
//! Y (XMM0, about to be stored at `[edi]`) are multiplied by `(1/60) / dt` (`dt` the float at
//! `[esi]`), held to [0.25, 8] (15 to 480 fps), and X is stored again. A `dt` of zero or less (or
//! not a number) leaves them as they were. Then the 10 bytes replaced are replayed: the x87
//! load last, as the code after it pops it. XMM2–XMM4 are free there. Controller input takes
//! the routine's other branch and never comes here.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::mouse_turn;
use tracing::info;
use tracing::warn;

/// Frames whose mouse turn was scaled since the game started (the stub counts them).
static SCALED: AtomicU32 = AtomicU32::new(0);

/// The constants the stub reads, in its own memory after the code.
const DATA_AT: usize = 0x60;
const ONE_SIXTIETH: f32 = 1.0 / 60.0;
const LEAST: f32 = 0.25;
const MOST: f32 = 8.0;

/// Assembles the stub at `stub` (its final address); `data` holds 1/60, the least and the most
/// factor, in that order, and `counter` the frame count.
#[allow(clippy::cast_possible_truncation)]
fn assemble(stub: u32, resume: u32, data: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(80);
    code.extend_from_slice(&[0xF3, 0x0F, 0x10, 0x16]); // movss xmm2,[esi]        dt
    code.extend_from_slice(&[0x0F, 0x57, 0xDB]); // xorps xmm3,xmm3
    code.extend_from_slice(&[0x0F, 0x2F, 0xD3]); // comiss xmm2,xmm3
    code.extend_from_slice(&[0x76, 0x32]); // jbe replay (dt <= 0, or NaN)
    let scale = code.len();
    code.extend_from_slice(&[0xF3, 0x0F, 0x10, 0x25]); // movss xmm4,[1/60]
    code.extend_from_slice(&data.to_le_bytes());
    code.extend_from_slice(&[0xF3, 0x0F, 0x5E, 0xE2]); // divss xmm4,xmm2
    code.extend_from_slice(&[0xF3, 0x0F, 0x5F, 0x25]); // maxss xmm4,[least]
    code.extend_from_slice(&(data + 4).to_le_bytes());
    code.extend_from_slice(&[0xF3, 0x0F, 0x5D, 0x25]); // minss xmm4,[most]
    code.extend_from_slice(&(data + 8).to_le_bytes());
    code.extend_from_slice(&[0xF3, 0x0F, 0x59, 0xCC]); // mulss xmm1,xmm4         X
    code.extend_from_slice(&[0xF3, 0x0F, 0x59, 0xC4]); // mulss xmm0,xmm4         Y
    code.extend_from_slice(&[0xF3, 0x0F, 0x11, 0x8E, 0x70, 0x04, 0x00, 0x00]); // movss [esi+0x470],xmm1
    code.extend_from_slice(&[0xFF, 0x05]); // inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    debug_assert_eq!(code.len() - scale, 0x32, "jbe's distance");
    // replay:
    code.extend_from_slice(&mouse_turn::ORIGINAL); // fld dword [esi+0x4b4] / movss [edi],xmm0
    code.push(0xE9); // jmp resume
    let rel = resume.wrapping_sub(stub + code.len() as u32 + 4);
    code.extend_from_slice(&rel.to_le_bytes());
    code
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn turn_per_second() {
    let Some(image) = crate::game_image() else {
        warn!("mouse-turn-fix (beta): the game's sections weren't found -> skipped");
        return;
    };
    let site = match mouse_turn::locate(&image) {
        Ok(site) => site,
        Err(what) => {
            warn!("mouse-turn-fix (beta): {what} -> skipped (unknown build?)");
            return;
        }
    };
    let Ok(alloc) = region::alloc(DATA_AT + 16, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("mouse-turn-fix (beta): couldn't allocate the stub -> skipped");
        return;
    };
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let data = stub.add(DATA_AT);
    for (i, value) in [ONE_SIXTIETH, LEAST, MOST].into_iter().enumerate() {
        std::ptr::write_unaligned(data.add(i * 4).cast::<f32>(), value);
    }
    let body = assemble(stub as u32, site.resume as u32, data as u32, std::ptr::addr_of!(SCALED) as u32);
    std::ptr::copy_nonoverlapping(body.as_ptr(), stub, body.len());
    // The stub lives as long as the game.
    std::mem::forget(alloc);

    let mut patch = [0x90u8; mouse_turn::ORIGINAL.len()];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(site.patch as u32 + 5).to_le_bytes());
    crate::writemem(site.patch as *mut u8, &patch);
    info!(
        "mouse-turn-fix (beta): the mouse turns as at 60 fps at any frame rate (look routine @ {:#010x}, stub {:#010x})",
        site.patch, stub as u32
    );

    // Once, when the mouse first turns: that the stub runs.
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(10));
        let scaled = SCALED.load(Ordering::Relaxed);
        if scaled > 0 {
            info!("mouse-turn-fix (beta): scaling the mouse's turn ({scaled} frames so far)");
            return;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_scales_then_replays_and_returns() {
        // DX11: back to 0xd8e31f.
        let (stub, data, counter) = (0x1000_0000, 0x1000_0060, 0x2000_0000);
        let code = assemble(stub, 0xd8e31f, data, counter);
        let word = |at: usize| u32::from_le_bytes(code[at..at + 4].try_into().unwrap());
        assert_eq!(&code[..12], &[0xF3, 0x0F, 0x10, 0x16, 0x0F, 0x57, 0xDB, 0x0F, 0x2F, 0xD3, 0x76, 0x32]);
        assert_eq!(word(16), data, "1/60");
        assert_eq!(word(28), data + 4, "least");
        assert_eq!(word(36), data + 8, "most");
        assert_eq!(word(58), counter);
        // jbe lands on the replay, which is the original 10 bytes.
        assert_eq!(&code[12 + 0x32..12 + 0x32 + 10], &mouse_turn::ORIGINAL);
        let jmp = 12 + 0x32 + 10;
        assert_eq!(code[jmp], 0xE9);
        assert_eq!((stub + jmp as u32 + 5).wrapping_add(word(jmp + 1)), 0xd8e31f, "back after the bytes replaced");
        assert!(code.len() <= DATA_AT, "the code stays clear of its data");
    }

    /// The stub itself, run on a stand-in for the look routine's object (32-bit only: the
    /// game's, and CI's hooks tests): it's called with ESI and EDI and the mouse's X and Y in
    /// XMM1 and XMM0 as the game has them, and jumps on to a landing that pops the x87 load it
    /// replays (as the game's next instruction does) and returns.
    #[cfg(target_arch = "x86")]
    #[test]
    fn the_stub_scales_the_mouse_by_the_frame_time() {
        unsafe fn run(dt: f32, x: f32, y: f32) -> (f32, f32, f32, f32, u32) {
            let alloc = region::alloc(0x100, region::Protection::READ_WRITE_EXECUTE).unwrap();
            let base = alloc.as_ptr::<u8>().cast_mut();
            let (stub, data, landing, counter) = (base, base.add(DATA_AT), base.add(0x80), base.add(0x90));
            for (i, value) in [ONE_SIXTIETH, LEAST, MOST].into_iter().enumerate() {
                std::ptr::write_unaligned(data.add(i * 4).cast::<f32>(), value);
            }
            // fstp st(0); ret
            std::ptr::copy_nonoverlapping([0xDD, 0xD8, 0xC3].as_ptr(), landing, 3);
            std::ptr::write_unaligned(counter.cast::<u32>(), 0);
            let code = assemble(stub as u32, landing as u32, data as u32, counter as u32);
            std::ptr::copy_nonoverlapping(code.as_ptr(), stub, code.len());

            let mut object = vec![0u8; 0x500];
            let this = object.as_mut_ptr();
            std::ptr::write_unaligned(this.cast::<f32>(), dt);
            std::ptr::write_unaligned(this.add(0x470).cast::<f32>(), x);
            std::ptr::write_unaligned(this.add(0x4b4).cast::<f32>(), 1.0);
            let (mut x_out, mut y_out) = (x, y);
            std::arch::asm!(
                "push esi",
                "mov esi, {this}",
                "call {stub}",
                "pop esi",
                this = in(reg) this as usize,
                stub = in(reg) stub as usize,
                in("edi") this as usize + 0x474,
                inout("xmm0") y_out,
                inout("xmm1") x_out,
                out("xmm2") _,
                out("xmm3") _,
                out("xmm4") _,
            );
            let stored_x = std::ptr::read_unaligned(this.add(0x470).cast::<f32>());
            let stored_y = std::ptr::read_unaligned(this.add(0x474).cast::<f32>());
            let frames = std::ptr::read_unaligned(counter.cast::<u32>());
            (x_out, y_out, stored_x, stored_y, frames)
        }
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        unsafe {
            // 120 fps: each frame's mouse distance is half 60's, so it's doubled: (1/60) / (1/120).
            let (x, y, sx, sy, n) = run(1.0 / 120.0, 3.0, -1.5);
            assert!(close(x, 6.0) && close(y, -3.0) && close(sx, 6.0) && close(sy, -3.0) && n == 1, "{x} {y} {sx} {sy} {n}");
            // 60 fps: as it was.
            let (x, y, sx, sy, _) = run(1.0 / 60.0, 3.0, -1.5);
            assert!(close(x, 3.0) && close(y, -1.5) && close(sx, 3.0) && close(sy, -1.5), "{x} {y} {sx} {sy}");
            // Held to 8 above 480 fps, and to 0.25 below 15.
            let (x, ..) = run(1.0 / 1000.0, 1.0, 0.0);
            assert!(close(x, 8.0), "{x}");
            let (x, ..) = run(1.0, 1.0, 0.0);
            assert!(close(x, 0.25), "{x}");
            // No frame time: left alone, not counted.
            let (x, y, sx, sy, n) = run(0.0, 3.0, -1.5);
            assert!(close(x, 3.0) && close(y, -1.5) && close(sx, 3.0) && close(sy, -1.5) && n == 0, "{x} {y} {sx} {sy} {n}");
            let (x, _, _, _, n) = run(f32::NAN, 3.0, 0.0);
            assert!(close(x, 3.0) && n == 0, "NaN frame time: {x} {n}");
        }
    }
}
