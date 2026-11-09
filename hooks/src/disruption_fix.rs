//! Stops a crash when a player's navigation is disrupted (a jammer, an EMP) while they hold no
//! item: `Blacklist_DX11_game.exe+0xa72c73`, reading `0x310` (Ghost_Leader 25 Oct and 1 Nov,
//! BlueBlueS 30 Oct, all 0.4.5-0.4.7).
//!
//! `UGeoNavDisruptionState`'s tick (DX11 `0x150c230`) takes the player's current item
//! (`[[player+0x93c]+0x21c]`, which is 0 when there's none), asks the item lookup (DX11
//! `0xe72c70`, thiscall) about it, and uses the answer. The lookup reads `[this+0x310]` first, so
//! with no item the game reads address `0x310` and crashes. Other callers reach the lookup with
//! no item too (BlueBlueS's came through a different one, which checks the answer).
//!
//! The fix, two guards:
//! - the lookup's first 9 bytes (`push esi` / `mov esi,ecx` / `mov ecx,[esi+0x310]`) jump to a
//!   stub that answers 0 ("not found", which its callers test for) when it's asked about no item,
//!   and else runs them and goes on;
//! - the tick's 9 bytes (`mov edi,eax` / `mov ecx,edi` / `call <lookup>`) jump to a stub that, with
//!   no item, goes straight to the tick's end (it would use the lookup's answer unchecked), and
//!   else runs them and goes on.
//!
//! Both sites are matched by signatures unique in both executables (DX11 `0xe72c70` and
//! `0x150c256`, DX9 `0x1108250` and `0x179e3e6`); the tick's call is checked to be the lookup, and
//! its end; the build is left alone otherwise. The stubs count what they caught, and a thread logs
//! the counts when they change.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::crash_fixes::disruption;
use hooks_addresses::crash_fixes::disruption::LOOKUP_LEN;
use hooks_addresses::crash_fixes::disruption::TICK_LEN;
use tracing::info;
use tracing::warn;

/// Lookups asked about no item, and ticks without one, since the game started.
static LOOKUPS: AtomicU32 = AtomicU32::new(0);
static TICKS: AtomicU32 = AtomicU32::new(0);

fn rel(stub: u32, code: &[u8], target: u32) -> [u8; 4] {
    #[allow(clippy::cast_possible_truncation)]
    target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes()
}

/// The lookup's stub at `stub`: no item answers 0; else the replaced bytes and back to `cont`.
fn assemble_lookup(stub: u32, cont: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(32);
    code.extend_from_slice(&[0x85, 0xC9]); // test ecx,ecx
    code.extend_from_slice(&[0x74, 0x0E]); // jz none
    code.push(0x56); // push esi
    code.extend_from_slice(&[0x8B, 0xF1]); // mov esi,ecx
    code.extend_from_slice(&[0x8B, 0x8E, 0x10, 0x03, 0x00, 0x00]); // mov ecx,[esi+0x310]
    code.push(0xE9); // jmp cont
    let r = rel(stub, &code, cont);
    code.extend_from_slice(&r);
    // none:
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    code.extend_from_slice(&[0x33, 0xC0]); // xor eax,eax
    code.push(0xC3); // ret
    code
}

/// The tick's stub at `stub`: no item goes to `end`; else the replaced bytes (the call to
/// `lookup`) and back to `cont`.
fn assemble_tick(stub: u32, lookup: u32, cont: u32, end: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(40);
    code.extend_from_slice(&[0x8B, 0xF8]); // mov edi,eax
    code.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    code.extend_from_slice(&[0x74, 0x0C]); // jz none
    code.extend_from_slice(&[0x8B, 0xCF]); // mov ecx,edi
    code.push(0xE8); // call lookup
    let r = rel(stub, &code, lookup);
    code.extend_from_slice(&r);
    code.push(0xE9); // jmp cont
    let r = rel(stub, &code, cont);
    code.extend_from_slice(&r);
    // none:
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    code.push(0xE9); // jmp end
    let r = rel(stub, &code, end);
    code.extend_from_slice(&r);
    code
}

/// Writes `body` to newly allocated code memory kept for the game's life, and the jump to it over
/// `len` bytes at `site`. Whether the jump went in.
unsafe fn install_stub(site: u32, len: usize, body: impl FnOnce(u32) -> Vec<u8>) -> Option<u32> {
    let alloc = region::alloc(64, region::Protection::READ_WRITE_EXECUTE).ok()?;
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let code = body(stub as u32);
    std::ptr::copy_nonoverlapping(code.as_ptr(), stub, code.len());
    std::mem::forget(alloc);
    let mut patch = vec![0x90u8; len];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(site + 5).to_le_bytes());
    crate::writemem(site as *mut u8, &patch).then_some(stub as u32)
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_disruption() {
    let Some(image) = crate::game_image() else {
        warn!("disruption-fix: the game's sections weren't found -> skipped");
        return;
    };
    let s = match disruption::locate(&image) {
        Ok(s) => s,
        Err(what) => {
            warn!("disruption-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let (lookup, lookup_cont, tick, tick_cont, tick_end) = (s.lookup as u32, s.lookup_cont as u32, s.tick as u32, s.tick_cont as u32, s.tick_end as u32);
    // The tick's guard first: it calls the lookup at its own address, which stays the same.
    let Some(tick_stub) = install_stub(tick, TICK_LEN, |stub| assemble_tick(stub, lookup, tick_cont, tick_end, std::ptr::addr_of!(TICKS) as u32)) else {
        warn!("disruption-fix: the nav-disruption tick's guard didn't go in -> skipped");
        return;
    };
    let Some(lookup_stub) = install_stub(lookup, LOOKUP_LEN, |stub| assemble_lookup(stub, lookup_cont, std::ptr::addr_of!(LOOKUPS) as u32)) else {
        warn!("disruption-fix: the item lookup's guard didn't go in (the tick's is in)");
        return;
    };
    crate::fix_stats::installed("disruption-fix", &[&LOOKUPS, &TICKS]);
    info!("disruption-fix: the item lookup @ {lookup:#010x} and the nav-disruption tick @ {tick:#010x} cope with no item (stubs {lookup_stub:#010x}, {tick_stub:#010x})");

    std::thread::spawn(|| {
        let mut logged = (0, 0);
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let now = (LOOKUPS.load(Ordering::Relaxed), TICKS.load(Ordering::Relaxed));
            if now != logged {
                info!(
                    "disruption-fix: asked about no item {} times, a nav disruption without an item {} times: the game would have crashed",
                    now.0, now.1
                );
                logged = now;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(code: &[u8], stub: u32, at: usize) -> u32 {
        (stub + at as u32 + 4).wrapping_add(u32::from_le_bytes(code[at..at + 4].try_into().unwrap()))
    }

    #[test]
    fn the_lookup_answers_nothing_for_no_item() {
        let stub = 0x1000_0000;
        let code = assemble_lookup(stub, 0xe72c79, 0x2000_0000);
        assert_eq!(&code[0..4], &[0x85, 0xC9, 0x74, 0x0E]);
        assert_eq!(&code[4..13], &[0x56, 0x8B, 0xF1, 0x8B, 0x8E, 0x10, 0x03, 0x00, 0x00], "the game's own bytes");
        assert_eq!(code[13], 0xE9);
        assert_eq!(target(&code, stub, 14), 0xe72c79, "an item: on as before");
        // jz lands right after the jmp.
        assert_eq!(4 + 0x0E, 18);
        assert_eq!(&code[18..21], &[0xF0, 0xFF, 0x05]);
        assert_eq!(u32::from_le_bytes(code[21..25].try_into().unwrap()), 0x2000_0000);
        assert_eq!(&code[25..28], &[0x33, 0xC0, 0xC3], "none: answers 0");
        assert_eq!(code.len(), 28);
    }

    #[test]
    fn the_tick_ends_for_no_item() {
        let stub = 0x1000_0000;
        let code = assemble_tick(stub, 0xe72c70, 0x150c25f, 0x150c27e, 0x2000_0000);
        assert_eq!(&code[0..8], &[0x8B, 0xF8, 0x85, 0xC0, 0x74, 0x0C, 0x8B, 0xCF]);
        assert_eq!(code[8], 0xE8);
        assert_eq!(target(&code, stub, 9), 0xe72c70, "the lookup, as the game calls it");
        assert_eq!(code[13], 0xE9);
        assert_eq!(target(&code, stub, 14), 0x150c25f, "an item: on as before");
        assert_eq!(6 + 0x0C, 18, "jz lands right after the jmp");
        assert_eq!(&code[18..21], &[0xF0, 0xFF, 0x05]);
        assert_eq!(code[25], 0xE9);
        assert_eq!(target(&code, stub, 26), 0x150c27e, "none: the tick's end");
        assert_eq!(code.len(), 30);
    }
}
