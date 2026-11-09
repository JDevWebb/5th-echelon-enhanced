//! Stops a crash in co-op when the AI checks a player who, for a moment, has no character
//! (Glacio1 19 Oct and AngledCircles 4 Nov, both about 90 s after a mission's end save, the game
//! closing both times: `Blacklist_DX11_game.exe+0x6f205a`, reading `0x538`).
//!
//! The AI looks over its targets (DX11 `FUN_00d87cf0`), and for a co-op player
//! (`FUN_00e945d0`) asks whether they're in one of its zones (`FUN_00af2000`, DX9 `0xd8a170`):
//! the zone's owner (`+0x90`), its controller (`+0x938`), the controller's character (`+0x354`,
//! or another lookup when a global flag is set), then a part of the character (`+0x538`). Every
//! step is checked for null and answers "no" when missing, except the character: while a
//! player's character is gone (between levels, or respawning) the game reads `0 + 0x538`.
//!
//! The fix: the read and the `test` after it (`mov eax,[eax+0x538]` / `test eax,eax`, 8 bytes,
//! where both ways to the character meet) become a jump to a stub that sends a missing character
//! to the test's own "no" (`xor al,al` ... `ret 4`), and otherwise runs the two instructions and
//! goes back to the `jz` they fed. A player with a character takes the original path unchanged.
//!
//! The site is matched by a signature unique in both executables (DX11 VA `0xaf205a`, DX9 VA
//! `0xd8a1ca`), with the function's start and its "no" checked; the build is left alone
//! otherwise. The stub counts what it skipped, and a thread logs the count when it changes.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::crash_fixes::perception;
use hooks_addresses::crash_fixes::perception::HOOK_LEN;
use tracing::info;
use tracing::warn;

/// Zone tests of players without a character since the game started.
static SKIPPED: AtomicU32 = AtomicU32::new(0);

/// Assembles the stub at `stub` (its final address) for the given absolute targets.
#[allow(clippy::cast_possible_truncation)]
fn assemble(stub: u32, skip: u32, cont: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(40);
    let rel = |code: &Vec<u8>, target: u32| target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes();
    code.extend_from_slice(&[0x85, 0xC0]); // test eax,eax (the character)
    code.extend_from_slice(&[0x74, 0x0D]); // jz gone
    code.extend_from_slice(&[0x8B, 0x80, 0x38, 0x05, 0x00, 0x00]); // mov eax,[eax+0x538]
    code.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    code.push(0xE9); // jmp cont (the jz, flags as the game set them)
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    // gone:
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    code.push(0xE9); // jmp skip (the test's "no")
    let r = rel(&code, skip);
    code.extend_from_slice(&r);
    code
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_perception() {
    let Some(image) = crate::game_image() else {
        warn!("perception-fix: the game's sections weren't found -> skipped");
        return;
    };
    let site = match perception::locate(&image) {
        Ok(site) => site,
        Err(what) => {
            warn!("perception-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let (hook, skip, cont) = (site.hook as u32, site.skip as u32, site.cont as u32);

    let Ok(alloc) = region::alloc(64, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("perception-fix: couldn't allocate the stub -> skipped");
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
    crate::fix_stats::installed("perception-fix", &[&SKIPPED]);
    info!(
        "perception-fix: the AI's zone test @ {hook:#010x} answers no for a player without a character (stub {:#010x}, no {skip:#010x})",
        stub as u32
    );

    std::thread::spawn(|| {
        let mut logged = 0;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let skipped = SKIPPED.load(Ordering::Relaxed);
            if skipped != logged {
                info!("perception-fix: the AI checked a player without a character ({skipped} so far): the game would have crashed");
                logged = skipped;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_checks_the_character_then_runs_the_replaced_bytes() {
        // DX11: the no 0xaf214d, continue 0xaf2062.
        let stub = 0x1000_0000;
        let code = assemble(stub, 0xaf214d, 0xaf2062, 0x2000_0000);
        let target = |at: usize| (stub + at as u32 + 4).wrapping_add(u32::from_le_bytes(code[at..at + 4].try_into().unwrap()));
        assert_eq!(&code[0..4], &[0x85, 0xC0, 0x74, 0x0D]);
        assert_eq!(&code[4..12], &[0x8B, 0x80, 0x38, 0x05, 0x00, 0x00, 0x85, 0xC0], "the game's own bytes");
        assert_eq!(code[12], 0xE9);
        assert_eq!(target(13), 0xaf2062, "with a character: the original path");
        // jz lands right after the jmp: 4 + 0x0D = 17.
        assert_eq!(&code[17..20], &[0xF0, 0xFF, 0x05]);
        assert_eq!(u32::from_le_bytes(code[20..24].try_into().unwrap()), 0x2000_0000);
        assert_eq!(code[24], 0xE9);
        assert_eq!(target(25), 0xaf214d, "without one: the test's no");
        assert_eq!(code.len(), 29);
    }
}
