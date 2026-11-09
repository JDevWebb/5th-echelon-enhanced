//! Stops a crash while the game loads part of a map: `Blacklist_DX11_game.exe+0x504e8`, reading 0
//! (Gex 28 Oct, who had also written of "crashing after matches"; mpw and Sokolev 1 Nov;
//! 0.4.6-0.4.7). The same stack each time, from the package loader's reads.
//!
//! While a package loads with the game's load-order checking on (a global flag, DX11
//! `0x330c4d4`), each read is checked against a table of the reads expected, which the archive
//! walks entry by entry (DX11 `0x4504b0`, thiscall, `ret 0x10`). When the walk passes the table's
//! last entry, the code that moves it along (`0x450730`) sets the current entry and table to 0,
//! and the next read's check uses them unchecked: the game reads address 0 and crashes.
//!
//! Since 0.5.0 umd_read_fix (PR #12, mpw) is the fix for the cause: a read the sequence can't
//! place (Play SvM skips the hub, so `PEC-Main.umd`'s reads were never recorded) gets its bytes
//! from the UMD files, and the sync never runs off the table. This fix stays as the backstop for
//! what umd_read_fix leaves to the game (a cursor it can't follow, a read no UMD has, or that fix
//! off or skipped): no crash, though the read past the table may then get the wrong bytes.
//!
//! The fix: the check's two reads of the current entry (DX11 `0x4504e2`, and `0x450657` on its
//! other path; 6 bytes each, two `mov`s) jump to stubs that, with no entry left, end the check
//! as "fine" through the function's own end (`mov al,1` ... `ret 0x10`): there is nothing left
//! to check the read against. Otherwise they run the two `mov`s and go on.
//!
//! The sites are matched by signatures unique in both executables (DX11 `0x4504e2` and
//! `0x450657`, DX9 `0x6ffdb2` and `0x6fff27`), checked to be one function apart, with its end
//! found from the function's own jump to it; the build is left alone otherwise. The stubs count
//! what they caught, and a thread logs the count when it changes.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::crash_fixes::load_order;
use hooks_addresses::crash_fixes::load_order::HOOK_LEN;
use tracing::info;
use tracing::warn;

/// Checks that ran past the table's end since the game started.
static PAST_END: AtomicU32 = AtomicU32::new(0);

/// The stub at `stub` for one site: `moves` (the replaced bytes, whose first loads the pointer
/// into ECX), then ECX null goes to `done`, else back to `cont`.
fn assemble(stub: u32, moves: [u8; HOOK_LEN], cont: u32, done: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(32);
    let rel = |code: &Vec<u8>, target: u32| {
        #[allow(clippy::cast_possible_truncation)]
        target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes()
    };
    code.extend_from_slice(&moves);
    code.extend_from_slice(&[0x85, 0xC9]); // test ecx,ecx
    code.extend_from_slice(&[0x74, 0x05]); // jz none
    code.push(0xE9); // jmp cont
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    // none:
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    code.push(0xE9); // jmp done
    let r = rel(&code, done);
    code.extend_from_slice(&r);
    code
}

/// `mov ecx,[esi+0x3c]` / `mov edx,[esi+0x34]`, and `mov ecx,[esi+0x38]` / `mov edx,[ebp+8]`.
const FIRST_MOVES: [u8; HOOK_LEN] = [0x8B, 0x4E, 0x3C, 0x8B, 0x56, 0x34];
const SECOND_MOVES: [u8; HOOK_LEN] = [0x8B, 0x4E, 0x38, 0x8B, 0x55, 0x08];

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_load_order() {
    let Some(image) = crate::game_image() else {
        warn!("load-order-fix: the game's sections weren't found -> skipped");
        return;
    };
    let s = match load_order::locate(&image) {
        Ok(s) => s,
        Err(what) => {
            warn!("load-order-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let done = s.done as u32;
    let mut stubs = vec![];
    for (site, moves) in [(s.first as u32, FIRST_MOVES), (s.second as u32, SECOND_MOVES)] {
        if std::slice::from_raw_parts(site as *const u8, HOOK_LEN) != moves {
            warn!("load-order-fix: the bytes at {site:#010x} aren't as found -> stopped");
            return;
        }
        let Ok(alloc) = region::alloc(48, region::Protection::READ_WRITE_EXECUTE) else {
            warn!("load-order-fix: couldn't allocate a stub -> stopped");
            return;
        };
        let stub = alloc.as_ptr::<u8>().cast_mut();
        let code = assemble(stub as u32, moves, site + HOOK_LEN as u32, done, std::ptr::addr_of!(PAST_END) as u32);
        std::ptr::copy_nonoverlapping(code.as_ptr(), stub, code.len());
        std::mem::forget(alloc);
        let mut patch = [0x90u8; HOOK_LEN];
        patch[0] = 0xE9;
        patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(site + 5).to_le_bytes());
        if !crate::writemem(site as *mut u8, &patch) {
            return;
        }
        stubs.push(stub as u32);
    }
    crate::fix_stats::installed("load-order-fix", &[&PAST_END]);
    info!(
        "load-order-fix: a package's load-order check past its table's end is fine, not a crash (@ {:#010x}, {:#010x}; stubs {stubs:#010x?})",
        s.first, s.second
    );

    std::thread::spawn(|| {
        let mut logged = 0;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let n = PAST_END.load(Ordering::Relaxed);
            if n != logged {
                info!("load-order-fix: a load-order check ran past its table ({n} so far): the game would have crashed");
                logged = n;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_runs_the_moves_then_ends_the_check_without_an_entry() {
        let stub = 0x1000_0000;
        let code = assemble(stub, FIRST_MOVES, 0x4504e8, 0x45071a, 0x2000_0000);
        let target = |at: usize| (stub + at as u32 + 4).wrapping_add(u32::from_le_bytes(code[at..at + 4].try_into().unwrap()));
        assert_eq!(&code[0..6], &FIRST_MOVES, "the game's own bytes");
        assert_eq!(&code[6..10], &[0x85, 0xC9, 0x74, 0x05]);
        assert_eq!(code[10], 0xE9);
        assert_eq!(target(11), 0x4504e8, "an entry: on as before");
        assert_eq!(&code[15..18], &[0xF0, 0xFF, 0x05]);
        assert_eq!(code[22], 0xE9);
        assert_eq!(target(23), 0x45071a, "none: the function's fine end");
        assert_eq!(code.len(), 27);
        assert_eq!(SECOND_MOVES[..3], [0x8B, 0x4E, 0x38], "the second loads the table into ECX too");
    }
}
