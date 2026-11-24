//! Stops a crash in co-op with more than two players: a character's ragdoll merging its body
//! parts from a parts list that was freed and reused under it (Glacio1, 18 Nov 2026: 4 fatal
//! crashes in 27 minutes, hosting 3-4 players with the overlay's player limit raised, each 7-15 s
//! after a player joined, left or was removed; DX11 `+0x5b9b03` x3 and `+0x5bdb36`).
//!
//! A character's body (`FUN_00dbb7f0`, "SAM_Ragdoll") merges its parts into one physics body
//! (DX11 `FUN_009bd510`, DX9 `0xc56df0`; thiscall, the parts list first). The list holds
//! 0x34-byte entries (`[list+0x28]`, count `[list+0x2c]`): a body object, then 10 part slots.
//! For each slot the game adds `[slot+0x40]+0x34` to a list (`TArray<int>::AddUniqueItem`). When
//! the parts list was freed and reused, a slot is garbage (`0x3f800000`, a float: the crash at
//! `+0x5bdb36` reading `[slot+0x40]`) or its `[+0x40]` is 0, -1 or garbage (the crash in
//! AddUniqueItem at `+0x5b9b03`). The game's own bug: the same place crashed in co-op before our
//! crash fixes (0.4.6 plan R1), other players too.
//!
//! The fix: the function's first three instructions (`push ebp` / `mov ebp,esp` / `push -1`)
//! become a jump to a stub that checks the list as the loop will read it: each entry the loop
//! visits, its body's `[+0x120]` and `[[+0x120]+0x34]`, and each part slot (a game object, whose
//! first word points into the game's `.rdata`, with `[slot+0x40]+0x34` readable). A list that
//! fails is handed to the function as null, which it already handles (no parts: it sets the body
//! up without merging), so the character gets no merged ragdoll body that once, instead of the
//! game closing. Every caller copes with that (they test the result, or carry on with the body as
//! set up). A good list runs the function unchanged.
//!
//! The site is found by the function's entry with its first statement (`or [edi+0x1c],0x80004`)
//! and the crash's loop at `+0x620`, unique in both executables. The stub counts what it
//! stopped, and a thread logs the count when it changes.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::crash_fixes::ragdoll;
use hooks_addresses::crash_fixes::ragdoll::HOOK_LEN;
use hooks_addresses::crash_fixes::ragdoll::REPLACED;
use tracing::info;
use tracing::warn;

/// Stale parts lists stopped since the game started.
static STOPPED: AtomicU32 = AtomicU32::new(0);
/// Why the last one failed ([`Why`]), and the entry and slot.
static LAST_WHY: AtomicU32 = AtomicU32::new(0);
/// The game's `.rdata`, where a game object's first word (its vtable) points.
static RDATA_START: AtomicU32 = AtomicU32::new(0);
static RDATA_END: AtomicU32 = AtomicU32::new(0);

/// The most entries a parts list is believed to have (a character has a few dozen bones).
const MOST_ENTRIES: u32 = 512;

/// Whether `len` bytes at `at` can be read (the crash handler's check: committed, neither
/// no-access nor a guard page).
fn readable(at: u32, len: u32) -> bool {
    at >= 0x10000 && len > 0 && unsafe { crate::crash::readable(at as usize, len as usize) }
}

/// The word at `at` (checked readable by the caller).
fn word(at: u32) -> u32 {
    // SAFETY: the caller checked the 4 bytes are readable.
    unsafe { std::ptr::read_unaligned(at as usize as *const u32) }
}

/// Whether `obj` is a game object: readable for `len` bytes, its first word in the game's `.rdata`.
fn game_object(obj: u32, len: u32) -> bool {
    readable(obj, len) && {
        let vtable = word(obj);
        (RDATA_START.load(Ordering::Relaxed)..RDATA_END.load(Ordering::Relaxed)).contains(&vtable)
    }
}

/// Why a list failed, for the log: the kind in the low byte, the entry in the next 16 bits, the
/// slot in the top byte.
#[derive(Clone, Copy)]
#[repr(u8)]
enum Why {
    Count = 1,
    Entries = 2,
    Body = 3,
    BodyPart = 4,
    Slot = 5,
    SlotTarget = 6,
}

fn why_text(code: u32) -> String {
    let (kind, entry, slot) = (code & 0xff, (code >> 8) & 0xffff, code >> 24);
    let what = match kind {
        1 => "a count past any character's",
        2 => "its entries not readable",
        3 => "an entry's body not readable",
        4 => "an entry's body part not readable",
        5 => "a part slot that isn't a game object",
        6 => "a part slot pointing at nothing readable",
        _ => "?",
    };
    format!("{what} (entry {entry}, slot {slot})")
}

/// Checks the parts list as the merge's loop reads it. 0 when it can't be read (stale).
fn check(list: u32) -> Result<(), u32> {
    let fail = |why: Why, entry: u32, slot: u32| Err(why as u32 | (entry.min(0xffff) << 8) | (slot << 24));
    if list == 0 {
        return Ok(());
    }
    if !readable(list.wrapping_add(0x28), 8) {
        return fail(Why::Entries, 0, 0);
    }
    let (entries, count) = (word(list.wrapping_add(0x28)), word(list.wrapping_add(0x2c)));
    if count == 0 || count > 0x7fff_ffff {
        return Ok(()); // The function's own "no parts" (it tests count > 0).
    }
    if count > MOST_ENTRIES {
        return fail(Why::Count, count, 0);
    }
    if !readable(entries, count * 0x34) {
        return fail(Why::Entries, 0, 0);
    }
    for i in 0..count {
        let entry = entries + i * 0x34;
        let body = word(entry);
        if body == 0 {
            continue;
        }
        if !readable(body.wrapping_add(0x120), 4) {
            return fail(Why::Body, i, 0);
        }
        let part = word(body.wrapping_add(0x120));
        if part == 0 {
            continue;
        }
        if !readable(part.wrapping_add(0x34), 4) || !readable(word(part.wrapping_add(0x34)), 0x68) {
            return fail(Why::BodyPart, i, 0);
        }
        for k in 0..10 {
            let slot = word(entry + 0xc + k * 4);
            if slot == 0 {
                continue;
            }
            if !game_object(slot, 0x44) {
                return fail(Why::Slot, i, k);
            }
            if !readable(word(slot.wrapping_add(0x40)).wrapping_add(0x34), 4) {
                return fail(Why::SlotTarget, i, k);
            }
        }
    }
    Ok(())
}

/// The stub's check: 1 when the list is fine, 0 when it's stale (and why is noted).
extern "stdcall" fn parts_ok(list: u32) -> u32 {
    match std::panic::catch_unwind(|| check(list)) {
        Ok(Ok(())) => 1,
        Ok(Err(why)) => {
            LAST_WHY.store(why, Ordering::Relaxed);
            0
        }
        // Never unwind into the game: a check that went wrong lets the game run as it would.
        Err(_) => 1,
    }
}

/// Assembles the stub at `stub` (its final address).
#[allow(clippy::cast_possible_truncation)]
fn assemble(stub: u32, check: u32, cont: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(48);
    let rel = |code: &Vec<u8>, target: u32| target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes();
    code.extend_from_slice(&[0x50, 0x51, 0x52]); // push eax / push ecx (this) / push edx
    code.extend_from_slice(&[0xFF, 0x74, 0x24, 0x10]); // push dword [esp+0x10] (the parts list)
    code.push(0xE8); // call parts_ok (stdcall: it pops the list)
    let r = rel(&code, check);
    code.extend_from_slice(&r);
    code.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    code.extend_from_slice(&[0x5A, 0x59, 0x58]); // pop edx / pop ecx / pop eax (flags kept)
    code.extend_from_slice(&[0x75, 0x0F]); // jnz run
    code.extend_from_slice(&[0xC7, 0x44, 0x24, 0x04, 0x00, 0x00, 0x00, 0x00]); // mov dword [esp+4],0
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    // run: the replaced bytes, then back.
    code.extend_from_slice(&REPLACED);
    code.push(0xE9); // jmp cont
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    code
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_ragdoll_parts() {
    let Some(image) = crate::game_image() else {
        warn!("ragdoll-fix: the game's sections weren't found -> skipped");
        return;
    };
    let site = match ragdoll::locate(&image) {
        Ok(site) => site,
        Err(what) => {
            warn!("ragdoll-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let rdata = image.rdata_range();
    RDATA_START.store(rdata.start as u32, Ordering::Relaxed);
    RDATA_END.store(rdata.end as u32, Ordering::Relaxed);
    let (hook, cont) = (site.hook as u32, site.cont as u32);

    let Ok(alloc) = region::alloc(64, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("ragdoll-fix: couldn't allocate the stub -> skipped");
        return;
    };
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let body = assemble(stub as u32, parts_ok as usize as u32, cont, std::ptr::addr_of!(STOPPED) as u32);
    std::ptr::copy_nonoverlapping(body.as_ptr(), stub, body.len());
    // The stub lives as long as the game.
    std::mem::forget(alloc);

    let mut patch = [0x90u8; HOOK_LEN];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(hook + 5).to_le_bytes());
    if !crate::writemem(hook as *mut u8, &patch) {
        return;
    }
    crate::fix_stats::installed("ragdoll-fix", &[&STOPPED]);
    info!(
        "ragdoll-fix: the ragdoll's parts merge @ {hook:#010x} checks its parts list first (stub {:#010x})",
        stub as u32
    );

    std::thread::spawn(|| {
        let mut logged = 0;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let stopped = STOPPED.load(Ordering::Relaxed);
            if stopped != logged {
                info!(
                    "ragdoll-fix: a character's ragdoll had a freed parts list ({stopped} so far; the last: {}): the game would have crashed",
                    why_text(LAST_WHY.load(Ordering::Relaxed))
                );
                logged = stopped;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_checks_the_list_then_runs_the_replaced_bytes() {
        // DX11: the merge 0x9bd510, so continue 0x9bd515.
        let stub = 0x1000_0000;
        let code = assemble(stub, 0x1100_0000, 0x9bd515, 0x2000_0000);
        let target = |at: usize| (stub + at as u32 + 4).wrapping_add(u32::from_le_bytes(code[at..at + 4].try_into().unwrap()));
        assert_eq!(
            &code[0..7],
            &[0x50, 0x51, 0x52, 0xFF, 0x74, 0x24, 0x10],
            "saves, then pushes the list (past 3 saves and the return)"
        );
        assert_eq!(code[7], 0xE8);
        assert_eq!(target(8), 0x1100_0000, "calls the check");
        assert_eq!(&code[12..17], &[0x85, 0xC0, 0x5A, 0x59, 0x58]);
        assert_eq!(&code[17..19], &[0x75, 0x0F], "a good list skips the 15 bytes that null it");
        assert_eq!(&code[19..27], &[0xC7, 0x44, 0x24, 0x04, 0, 0, 0, 0]);
        assert_eq!(&code[27..30], &[0xF0, 0xFF, 0x05]);
        assert_eq!(u32::from_le_bytes(code[30..34].try_into().unwrap()), 0x2000_0000);
        // jnz lands at 19 + 0x0F = 34: the replaced bytes.
        assert_eq!(&code[34..39], &REPLACED);
        assert_eq!(code[39], 0xE9);
        assert_eq!(target(40), 0x9bd515, "back after the replaced bytes");
        assert_eq!(code.len(), 44);
    }

    #[test]
    fn why_is_readable() {
        let code = Why::SlotTarget as u32 | (3 << 8) | (7 << 24);
        assert_eq!(why_text(code), "a part slot pointing at nothing readable (entry 3, slot 7)");
    }
}
