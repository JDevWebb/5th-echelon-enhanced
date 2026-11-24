//! Stops a crash in co-op past its size as a player leaves (the gamebox, 23 Nov 2026: three
//! players in a co-op mission with the seats raised to 4; 9 s after one quit, another's game
//! crashed at DX11 `+0x914fb6` reading `[0+0x93c]`).
//!
//! A character's speed update (DX11 `0xd4c330`, DX9 `0xfe1f00`) works out the speed from its
//! owner's movement history (`[[this+0x24]+0x12a4]`, 36 positions; `0xd14fb0` reads its newest
//! index at `+0x93c`). With its player gone the history was null and the read crashed.
//!
//! The fix: the update's first instructions become a jump to a stub; with no owner, or an owner
//! with no history, the update is skipped (it returns; it has nothing to return), so the speed
//! keeps its last value that once. Otherwise the update runs unchanged. Counted, and logged when
//! it changes.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::crash_fixes::movement;
use hooks_addresses::crash_fixes::movement::HOOK_LEN;
use hooks_addresses::crash_fixes::movement::REPLACED;
use tracing::info;
use tracing::warn;

static STOPPED: AtomicU32 = AtomicU32::new(0);

#[allow(clippy::cast_possible_truncation)]
fn assemble(stub: u32, cont: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(48);
    let rel = |code: &Vec<u8>, target: u32| target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes();
    code.extend_from_slice(&[0x8B, 0x41, movement::OWNER_AT]); // mov eax,[ecx+0x24]
    code.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    code.extend_from_slice(&[0x74, 0x15]); // jz skip
    code.extend_from_slice(&[0x8B, 0x80]); // mov eax,[eax+0x12a4]
    code.extend_from_slice(&movement::HISTORY_AT.to_le_bytes());
    code.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    code.extend_from_slice(&[0x74, 0x0B]); // jz skip
    code.extend_from_slice(&REPLACED); // the replaced bytes
    code.push(0xE9); // jmp cont
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    // skip: count it, and return as the update does.
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]);
    code.extend_from_slice(&counter.to_le_bytes());
    code.push(0xC3); // ret
    code
}

#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_speed_updates() {
    let Some(image) = crate::game_image() else {
        warn!("movement-fix: the game's sections weren't found -> skipped");
        return;
    };
    let site = match movement::locate(&image) {
        Ok(site) => site,
        Err(what) => {
            warn!("movement-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let (hook, cont) = (site.hook as u32, site.cont as u32);
    let Ok(alloc) = region::alloc(64, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("movement-fix: couldn't allocate the stub -> skipped");
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
    crate::fix_stats::installed("movement-fix", &[&STOPPED]);
    info!(
        "movement-fix: a character's speed update @ {hook:#010x} checks its movement history first (stub {:#010x})",
        stub as u32
    );
    std::thread::spawn(|| {
        let mut logged = 0;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let stopped = STOPPED.load(Ordering::Relaxed);
            if stopped != logged {
                info!("movement-fix: a character whose movement history was gone ({stopped} so far): the game would have crashed");
                logged = stopped;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_checks_owner_and_history_then_runs_or_returns() {
        let (stub, cont, counter) = (0x1000_0000u32, 0x00d4_c336u32, 0x2000_0000u32);
        let code = assemble(stub, cont, counter);
        // 0 mov(3) 3 test(2) 5 jz(2) 7 mov(6) 13 test(2) 15 jz(2) 17 replaced(6) 23 jmp(5) 28 skip
        assert_eq!(7 + 0x15, 28, "the first jz to the skip");
        assert_eq!(code[6], 0x15);
        assert_eq!(17 + 0x0B, 28, "the second jz to the skip");
        assert_eq!(&code[17..23], &REPLACED);
        let back = u32::from_le_bytes(code[24..28].try_into().unwrap());
        assert_eq!(stub.wrapping_add(28).wrapping_add(back), cont);
        assert_eq!(&code[28..31], &[0xF0, 0xFF, 0x05]);
        assert_eq!(code[35], 0xC3);
        assert_eq!(code.len(), 36);
    }
}
