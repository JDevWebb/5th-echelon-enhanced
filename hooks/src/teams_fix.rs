//! Stops a crash at a round's end in a room set below its mode's size (RS: Tea_bag, 21-22 Nov
//! 2026, three in Spies vs Mercs with the seats turned down to 2, each at `RoundState_Ended` as
//! the game saved; DX11 `+0x41a00c`, `+0x3d731d`, `+0x3d733c`, all from `+0x3d7646` ←
//! `+0x38d1dc`).
//!
//! The team service (`NetServiceTeams`, its message handler, vtable slot 35) copies a list of its
//! players' teams into its own (`0x7d72f0`, DX9 `0xa74600`): it resizes its array to the list's
//! count (`[list+0x64]`), then reads each 12-byte entry. The list holds 8 entries in place; at the
//! crash the count was garbage (the loop was some 406,000 entries in, `edi=0xdf0cec`, reading off
//! the end of a mapping; the first crash was the resize to it, writing through a null-ish array).
//! So the list it was handed wasn't one: freed, or never filled, as the room's seats changed.
//!
//! The fix: the function's first instructions become a jump to a stub that checks the count; past
//! any match's size ([`MOST`], 32) the copy is skipped (the function returns, as for an empty
//! list), so the service keeps its last teams that once instead of the game closing. A real list
//! (8 at most) runs the function unchanged. Counted, and logged when it changes.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::crash_fixes::teams;
use hooks_addresses::crash_fixes::teams::HOOK_LEN;
use hooks_addresses::crash_fixes::teams::REPLACED;
use tracing::info;
use tracing::warn;

static STOPPED: AtomicU32 = AtomicU32::new(0);
/// The most players a list can be for.
const MOST: u32 = 32;

#[allow(clippy::cast_possible_truncation)]
fn assemble(stub: u32, cont: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(40);
    let rel = |code: &Vec<u8>, target: u32| target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes();
    code.extend_from_slice(&[0x8B, 0x44, 0x24, 0x04]); // mov eax,[esp+4] (the list)
    code.extend_from_slice(&[0x81, 0x78, teams::COUNT_AT]); // cmp dword [eax+0x64], MOST
    code.extend_from_slice(&MOST.to_le_bytes());
    code.extend_from_slice(&[0x77, 0x0A]); // ja skip (past the replaced bytes and the jump)
    code.extend_from_slice(&REPLACED); // the replaced bytes
    code.push(0xE9); // jmp cont
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    // skip: count it, and return as the function does.
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    code.push(0xC2); // ret 0x10
    code.extend_from_slice(&teams::ARGS.to_le_bytes());
    code
}

#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_team_copies() {
    let Some(image) = crate::game_image() else {
        warn!("teams-fix: the game's sections weren't found -> skipped");
        return;
    };
    let site = match teams::locate(&image) {
        Ok(site) => site,
        Err(what) => {
            warn!("teams-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let (hook, cont) = (site.hook as u32, site.cont as u32);
    let Ok(alloc) = region::alloc(64, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("teams-fix: couldn't allocate the stub -> skipped");
        return;
    };
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let body = assemble(stub as u32, cont, std::ptr::addr_of!(STOPPED) as u32);
    std::ptr::copy_nonoverlapping(body.as_ptr(), stub, body.len());
    std::mem::forget(alloc);
    let mut patch = [0x90u8; HOOK_LEN];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(hook + 5).to_le_bytes());
    if !crate::writemem(hook as *mut u8, &patch) {
        return;
    }
    crate::fix_stats::installed("teams-fix", &[&STOPPED]);
    info!("teams-fix: the team service's copy @ {hook:#010x} checks its list's count first (stub {:#010x})", stub as u32);
    std::thread::spawn(|| {
        let mut logged = 0;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let stopped = STOPPED.load(Ordering::Relaxed);
            if stopped != logged {
                info!("teams-fix: a team list with a count past any match's ({stopped} so far): the game would have crashed");
                logged = stopped;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_checks_the_count_then_runs_the_replaced_bytes_or_returns() {
        let (stub, cont, counter) = (0x1000_0000u32, 0x007d_72f5u32, 0x2000_0000u32);
        let code = assemble(stub, cont, counter);
        assert_eq!(&code[..4], &[0x8B, 0x44, 0x24, 0x04]);
        assert_eq!(&code[4..11], &[0x81, 0x78, 0x64, 32, 0, 0, 0]);
        assert_eq!(&code[11..13], &[0x77, 0x0A], "past the replaced bytes and the jump");
        assert_eq!(13 + 0x0A, 23, "to the skip");
        assert_eq!(&code[13..18], &REPLACED);
        assert_eq!(code[18], 0xE9);
        let rel = u32::from_le_bytes(code[19..23].try_into().unwrap());
        assert_eq!(stub + 23 + rel, cont, "back after the replaced bytes");
        assert_eq!(&code[23..26], &[0xF0, 0xFF, 0x05]);
        assert_eq!(&code[30..], &[0xC2, 0x10, 0x00], "ret 0x10, as the function");
    }
}
