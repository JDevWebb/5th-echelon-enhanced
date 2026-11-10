//! Stops a crash when the menus show a loadout's gear and an item can't be found
//! (`Blacklist_DX11_game.exe+0xb116b5`, reading `0x88`: Jester 9 Nov 2026, twice, opening
//! Customize Gear and loading the hub, on 0.4.7).
//!
//! For each of a loadout's slots, the game (DX11 `FUN_00f11540`, DX9 `0x11a5210`) walks a list
//! of names, asks for the object with each (`FUN_00cb64a0`, which leaves its answer at 0 when
//! there's none), and adds a field of it (`+0x88`) to the list it's building without checking
//! anything was found: a name with no object reads `0 + 0x88`. In the gamebox (quick start,
//! the hub, 0.4.7 and 0.5.0, a rank 5 save) every name was found; which item is missing for a
//! player who crashes isn't known yet, so the fix also says.
//!
//! The fix: the read and the instruction before it (`mov ecx,[ebp-0x1c]` / `mov
//! edx,[ecx+0x88]`, 9 bytes) become a jump to a stub that runs them when something was found
//! and goes back (the item is added as before); with nothing found it notes the name (the
//! loop's current one, `[esi]` + `[ebp+0xc]`: its text and length) and goes to the loop's next
//! name, leaving that item out of the list. EAX, ECX and EDX are free there: the loop loads
//! them again before using them.
//!
//! The site is matched by a signature unique in both executables (DX11 VA `0xf116b2`, DX9 VA
//! `0x11a5382`), with the function's start checked; the build is left alone otherwise. The stub
//! counts what it skipped, and a thread logs each name not found (once each), so a report says
//! which item is missing.

use std::collections::HashSet;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use hooks_addresses::crash_fixes::gear_list;
use hooks_addresses::crash_fixes::gear_list::HOOK_LEN;
use tracing::info;
use tracing::warn;

/// Names with nothing found since the game started.
static SKIPPED: AtomicU32 = AtomicU32::new(0);
/// The last such name: its text's address and length (the game's own string, 8-bit).
static LAST_TEXT: AtomicU32 = AtomicU32::new(0);
static LAST_LEN: AtomicU32 = AtomicU32::new(0);

/// The most of a name logged.
const NAME_MAX: usize = 96;
/// The most distinct names logged in a game.
const NAMES_LOGGED: usize = 32;

/// Assembles the stub at `stub` (its final address) for the given absolute targets.
#[allow(clippy::cast_possible_truncation)]
fn assemble(stub: u32, cont: u32, skip: u32, counter: u32, last_text: u32, last_len: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(64);
    let rel = |code: &Vec<u8>, target: u32| target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes();
    code.extend_from_slice(&[0x8B, 0x4D, 0xE4]); // mov ecx,[ebp-0x1c] (what the lookup found)
    code.extend_from_slice(&[0x85, 0xC9]); // test ecx,ecx
    code.extend_from_slice(&[0x74, 0x0B]); // jz missing
    code.extend_from_slice(&[0x8B, 0x91, 0x88, 0x00, 0x00, 0x00]); // mov edx,[ecx+0x88]
    code.push(0xE9); // jmp cont (push edx: the item added)
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    // missing: the loop's current name, [esi] + [ebp+0xc]: its text and length.
    code.extend_from_slice(&[0x8B, 0x06]); // mov eax,[esi]
    code.extend_from_slice(&[0x03, 0x45, 0x0C]); // add eax,[ebp+0xc]
    code.extend_from_slice(&[0x8B, 0x08]); // mov ecx,[eax]
    code.extend_from_slice(&[0x89, 0x0D]); // mov [last_text],ecx
    code.extend_from_slice(&last_text.to_le_bytes());
    code.extend_from_slice(&[0x8B, 0x48, 0x04]); // mov ecx,[eax+4]
    code.extend_from_slice(&[0x89, 0x0D]); // mov [last_len],ecx
    code.extend_from_slice(&last_len.to_le_bytes());
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    code.push(0xE9); // jmp skip (the loop's next name)
    let r = rel(&code, skip);
    code.extend_from_slice(&r);
    code
}

/// The last name not found, as the game holds it (at most `NAME_MAX` bytes, read safely).
fn last_name() -> Option<String> {
    let text = LAST_TEXT.load(Ordering::Relaxed) as usize;
    let len = (LAST_LEN.load(Ordering::Relaxed) as usize).min(NAME_MAX);
    if text == 0 || len == 0 {
        return None;
    }
    crate::dataversion::read_string(text, len).filter(|n| !n.is_empty())
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_gear_list() {
    let Some(image) = crate::game_image() else {
        warn!("gear-list-fix: the game's sections weren't found -> skipped");
        return;
    };
    let site = match gear_list::locate(&image) {
        Ok(site) => site,
        Err(what) => {
            warn!("gear-list-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let (hook, cont, skip) = (site.hook as u32, site.cont as u32, site.skip as u32);

    let Ok(alloc) = region::alloc(96, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("gear-list-fix: couldn't allocate the stub -> skipped");
        return;
    };
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let body = assemble(
        stub as u32,
        cont,
        skip,
        std::ptr::addr_of!(SKIPPED) as u32,
        std::ptr::addr_of!(LAST_TEXT) as u32,
        std::ptr::addr_of!(LAST_LEN) as u32,
    );
    std::ptr::copy_nonoverlapping(body.as_ptr(), stub, body.len());
    // The stub lives as long as the game.
    std::mem::forget(alloc);

    let mut patch = [0x90u8; HOOK_LEN];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(hook + 5).to_le_bytes());
    if !crate::writemem(hook as *mut u8, &patch) {
        return;
    }
    crate::fix_stats::installed("gear-list-fix", &[&SKIPPED]);
    info!(
        "gear-list-fix: the gear list @ {hook:#010x} leaves out an item it can't find (stub {:#010x}, next name {skip:#010x})",
        stub as u32
    );

    std::thread::spawn(|| {
        let mut logged = 0;
        let mut names: HashSet<String> = HashSet::new();
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let skipped = SKIPPED.load(Ordering::Relaxed);
            if skipped == logged {
                continue;
            }
            logged = skipped;
            let name = last_name();
            let new = name.as_ref().is_some_and(|n| names.len() < NAMES_LOGGED && names.insert(n.clone()));
            if new || names.is_empty() {
                info!(
                    "gear-list-fix: no object for the gear named {:?} ({skipped} left out so far): the game would have crashed",
                    name.as_deref().unwrap_or("?")
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_runs_the_read_when_found_and_notes_the_name_when_not() {
        // DX11: continue 0xf116bb, next name 0xf116c3.
        let stub = 0x1000_0000;
        let code = assemble(stub, 0xf116bb, 0xf116c3, 0x2000_0000, 0x2000_0004, 0x2000_0008);
        let target = |at: usize| (stub + at as u32 + 4).wrapping_add(u32::from_le_bytes(code[at..at + 4].try_into().unwrap()));
        assert_eq!(&code[0..3], &[0x8B, 0x4D, 0xE4], "the game's first instruction");
        assert_eq!(&code[3..7], &[0x85, 0xC9, 0x74, 0x0B]);
        assert_eq!(&code[7..13], &[0x8B, 0x91, 0x88, 0x00, 0x00, 0x00], "the game's read");
        assert_eq!(code[13], 0xE9);
        assert_eq!(target(14), 0xf116bb, "found: on to the item's adding");
        // jz lands right after the jmp: 7 + 0x0B = 18.
        assert_eq!(&code[18..25], &[0x8B, 0x06, 0x03, 0x45, 0x0C, 0x8B, 0x08], "the loop's current name");
        assert_eq!(&code[25..27], &[0x89, 0x0D]);
        assert_eq!(u32::from_le_bytes(code[27..31].try_into().unwrap()), 0x2000_0004);
        assert_eq!(&code[31..36], &[0x8B, 0x48, 0x04, 0x89, 0x0D]);
        assert_eq!(u32::from_le_bytes(code[36..40].try_into().unwrap()), 0x2000_0008);
        assert_eq!(&code[40..43], &[0xF0, 0xFF, 0x05]);
        assert_eq!(u32::from_le_bytes(code[43..47].try_into().unwrap()), 0x2000_0000);
        assert_eq!(code[47], 0xE9);
        assert_eq!(target(48), 0xf116c3, "not found: the loop's next name");
        assert_eq!(code.len(), 52);
    }
}
