//! Stops a crash when a spy destroys mercs' tagging devices with EMP or the overcharge suit
//! (Ronis, 2026-11-01: "if you place 2 or 3 intel devices as merc and spy destroys it with either
//! EMP or overcharge suit, the game crashes", seen more than once).
//!
//! `AGI_Adv_TaggingDevice` (a timed explosive) resets itself from its tick (vtable slot 0x538,
//! DX11 `0xaa8810`): it clears its state, then reads where a part of it is (`[this+0x268]`,
//! virtual `+0xfc`) and moves it (`+0x118`). For a device the spy has just destroyed that part is
//! already gone and the pointer null, so the game reads address 0 and crashes
//! (`Blacklist_DX11_game.exe+0x6a88ce`, Tea_bag's game hosting, 2026-11-01).
//!
//! The fix: the 15 bytes before the read (`mov ecx,[esi+0x268]` / `lea edx,[ebp-0x18]` /
//! `fstp dword [esi+0x1a4]`) become a jump to a stub that runs them, then sends a device without
//! the part straight to the reset's end (`pop esi` ...), skipping only the two calls on the part
//! that isn't there. The state is cleared as before, and the float stack popped as before. A
//! device with the part takes the original path unchanged.
//!
//! The site is matched by a signature unique in both executables (DX11 VA `0xaa88bf`, DX9 VA
//! `0xd4128f`), with the function's start and end checked; the build is left alone otherwise.
//! The stub counts what it skipped, and a thread logs the count when it changes.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::crash_fixes::tagging_device;
use hooks_addresses::crash_fixes::tagging_device::HOOK_LEN;
use tracing::info;
use tracing::warn;

/// Resets of devices without the part since the game started.
static SKIPPED: AtomicU32 = AtomicU32::new(0);

/// Assembles the stub at `stub` (its final address) for the given absolute targets.
#[allow(clippy::cast_possible_truncation)]
fn assemble(stub: u32, skip: u32, cont: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(48);
    let rel = |code: &Vec<u8>, target: u32| target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes();
    code.extend_from_slice(&[0x8B, 0x8E, 0x68, 0x02, 0x00, 0x00]); // mov ecx,[esi+0x268]
    code.extend_from_slice(&[0x8D, 0x55, 0xE8]); // lea edx,[ebp-0x18]
    code.extend_from_slice(&[0xD9, 0x9E, 0xA4, 0x01, 0x00, 0x00]); // fstp dword [esi+0x1a4]
    code.extend_from_slice(&[0x85, 0xC9]); // test ecx,ecx
    code.extend_from_slice(&[0x74, 0x05]); // jz gone
    code.push(0xE9); // jmp cont (the original path)
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    // gone:
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    code.push(0xE9); // jmp skip (the reset's end)
    let r = rel(&code, skip);
    code.extend_from_slice(&r);
    code
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_tagging_devices() {
    let Some(image) = crate::game_image() else {
        warn!("tagging-device-fix: the game's sections weren't found -> skipped");
        return;
    };
    let site = match tagging_device::locate(&image) {
        Ok(site) => site,
        Err(what) => {
            warn!("tagging-device-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let (hook, skip, cont) = (site.hook as u32, site.skip as u32, site.cont as u32);

    let Ok(alloc) = region::alloc(64, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("tagging-device-fix: couldn't allocate the stub -> skipped");
        return;
    };
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let body = assemble(stub as u32, skip, cont, std::ptr::addr_of!(SKIPPED) as u32);
    std::ptr::copy_nonoverlapping(body.as_ptr(), stub, body.len());
    // The stub lives as long as the game.
    std::mem::forget(alloc);

    let mut patch = [0x90u8; HOOK_LEN];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(hook + 5).to_le_bytes());
    if !crate::writemem(hook as *mut u8, &patch) {
        return;
    }
    crate::fix_stats::installed("tagging-device-fix", &[&SKIPPED]);
    info!(
        "tagging-device-fix: a destroyed tagging device's reset @ {hook:#010x} no longer uses the part that's gone (stub {:#010x}, end {skip:#010x})",
        stub as u32
    );

    std::thread::spawn(|| {
        let mut logged = 0;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let skipped = SKIPPED.load(Ordering::Relaxed);
            if skipped != logged {
                info!("tagging-device-fix: a tagging device was reset without its part ({skipped} so far): the game would have crashed");
                logged = skipped;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_runs_the_replaced_bytes_then_jumps_where_it_should() {
        // DX11: the end 0xaa894d, continue 0xaa88ce.
        let stub = 0x1000_0000;
        let code = assemble(stub, 0xaa894d, 0xaa88ce, 0x2000_0000);
        let target = |at: usize| (stub + at as u32 + 4).wrapping_add(u32::from_le_bytes(code[at..at + 4].try_into().unwrap()));
        assert_eq!(
            &code[0..15],
            &[0x8B, 0x8E, 0x68, 0x02, 0x00, 0x00, 0x8D, 0x55, 0xE8, 0xD9, 0x9E, 0xA4, 0x01, 0x00, 0x00],
            "the game's own bytes"
        );
        assert_eq!(&code[15..19], &[0x85, 0xC9, 0x74, 0x05]);
        assert_eq!(code[19], 0xE9);
        assert_eq!(target(20), 0xaa88ce, "a device with its part: the original path");
        // jz lands right after the jmp.
        assert_eq!(&code[24..27], &[0xF0, 0xFF, 0x05]);
        assert_eq!(u32::from_le_bytes(code[27..31].try_into().unwrap()), 0x2000_0000);
        assert_eq!(code[31], 0xE9);
        assert_eq!(target(32), 0xaa894d, "without it: the reset's end");
        assert_eq!(code.len(), 36);
    }
}
