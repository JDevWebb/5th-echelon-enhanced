//! Customize Gear in the online menu shows the Characters chooser (Campaign, Spy, Merc) after a
//! quick start too, not the campaign's loadout A (players launching Spies vs Mercs from the
//! launcher, Nov 2026: "it shows the campaign loadout instead"; three times out of three on the
//! gamebox, DX11).
//!
//! When the gear screen (`UI::SMILoadout`) opens, its activation (DX11 slot 6 `0x1a28010`, DX9
//! `0x587820`) asks a function (DX11 `0x1a24b20`, DX9 `0x593d30`) what to open with, left at the
//! screen's `+0x3a4`. In the online menu (menu state 0x56, or 0x57 for its Customize Gear) with
//! no match yet, that's the chooser (0) when the local player's game type is 1, which the hub
//! sets; otherwise 5, the campaign's loadout A, with nothing to switch to Spy or Merc. A quick
//! start goes from the title to the online menu without the hub, so the type isn't 1 and every
//! Customize Gear there opened loadout A. In a match's lobby the game chooses by the match, and
//! was right.
//!
//! The fix: the compare right after that call (`cmp dword [esi+0x3a4],5`, the same 7 bytes in
//! both executables) becomes a call to a stub that saves the registers, has [`chosen`] look at
//! the screen (ESI), and runs the compare before going back, so the game's own branch on it is
//! unchanged. [`chosen`] turns a 5 into 0 only when the quick start took this game online from
//! the title, in the online menu's two states: the choice a start through the hub makes there.
//! Installed with the quick start, and only when it's in use.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use hooks_addresses::loadout_chooser;
use tracing::info;
use tracing::warn;

/// The online menu, and its Customize Gear.
const MENU_STATES: [u32; 2] = [0x56, 0x57];
/// The campaign's loadout A, and the chooser.
const LOADOUT_A: u32 = 5;
const CHOOSER: u32 = 0;

/// Times the chooser was opened instead.
static CHANGED: AtomicU32 = AtomicU32::new(0);

/// Assembles the stub at `stub` (its final address), calling `chosen` (stdcall, one argument).
#[allow(clippy::cast_possible_truncation)]
fn assemble(stub: u32, chosen: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(24);
    code.push(0x60); // pushad
    code.push(0x56); // push esi (the screen)
    code.push(0xE8); // call chosen
    let rel = chosen.wrapping_sub(stub + code.len() as u32 + 4);
    code.extend_from_slice(&rel.to_le_bytes());
    code.push(0x61); // popad
    code.extend_from_slice(&loadout_chooser::CHECK); // cmp dword [esi+0x3a4],5: the game's own
    code.push(0xC3); // ret (the flags the compare set go back with it)
    code
}

/// What the gear screen at `screen` chose, looked at before the game acts on it.
extern "stdcall" fn chosen(screen: u32) {
    let at = (screen as usize).wrapping_add(loadout_chooser::MODE);
    if screen == 0 || !crate::quickstart::online_from_title() {
        return;
    }
    let Some(state) = crate::quickstart::menu_state() else { return };
    if !MENU_STATES.contains(&state) || crate::quickstart::read_u32(at) != Some(LOADOUT_A) {
        return;
    }
    // SAFETY: the screen's own field, just written by the game on this (the main) thread, and
    // read above.
    unsafe { std::ptr::write_volatile(at as *mut u32, CHOOSER) };
    let n = CHANGED.fetch_add(1, Ordering::Relaxed) + 1;
    info!("loadout-chooser: Customize Gear in the online menu (state {state:#x}) opens the Characters chooser, not the campaign's loadout A ({n} so far)");
}

/// Installs the fix (from the quick start's `install`, once it's in use). Leaves an unrecognised
/// build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn install(image: &hooks_addresses::crash_fixes::Image) {
    let site = match loadout_chooser::locate(image) {
        Ok(site) => site,
        Err(what) => {
            warn!("loadout-chooser: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let Ok(alloc) = region::alloc(32, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("loadout-chooser: couldn't allocate the stub -> skipped");
        return;
    };
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let body = assemble(stub as u32, chosen as usize as u32);
    std::ptr::copy_nonoverlapping(body.as_ptr(), stub, body.len());
    // The stub lives as long as the game.
    std::mem::forget(alloc);

    let check = site.check as u32;
    let mut patch = [0x90u8; loadout_chooser::CHECK.len()];
    patch[0] = 0xE8;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(check + 5).to_le_bytes());
    if !crate::writemem(check as *mut u8, &patch) {
        return;
    }
    crate::fix_stats::installed("loadout-chooser", &[&CHANGED]);
    info!(
        "loadout-chooser: the gear screen's choice ({:#010x}) is looked at @ {check:#010x} (stub {:#010x})",
        site.choice, stub as u32
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_saves_everything_and_runs_the_compare() {
        let stub = 0x1000_0000;
        let code = assemble(stub, 0x1000_1000);
        assert_eq!(&code[0..3], &[0x60, 0x56, 0xE8]);
        let rel = u32::from_le_bytes(code[3..7].try_into().unwrap());
        assert_eq!((stub + 7).wrapping_add(rel), 0x1000_1000, "calls chosen");
        assert_eq!(code[7], 0x61, "popad after (stdcall: the argument is gone)");
        assert_eq!(&code[8..15], &loadout_chooser::CHECK, "the game's compare");
        assert_eq!(code[15], 0xC3);
        assert_eq!(code.len(), 16);
    }
}
