//! Lets a match lobby carry on under a new host when its host leaves, as a party does (part of
//! the beta "host migration", `host_migration.rs`). Background in
//! `docs/research/host-migration.md`.
//!
//! Every session object (`NetSessionBase`) has a "may migrate" flag at `+0x5a9`. The game sets it
//! when a session is set up (`0x0086a1e0` in the DX11 build) from the session's kind alone: 1 for
//! a party (`NetPartySession`, kind 1), 0 for a match (`NetGameSession`, kind 0). It decides
//! three things:
//!
//! * the host leaving: a plain Storm leave (the others elect a new host) instead of Dissolve, and
//!   AbandonSession on the server instead of DeleteSession;
//! * a member noticing the host is gone: start the migration instead of `LostConnectionToHost`;
//! * the engine's host-loss check during a running match (`0x00779580`).
//!
//! The migration itself (election, the server calls, handing the Storm objects to the new host,
//! the match lobby's own `StateMigrating`) is the same for both kinds. So:
//!
//! 1. the `sete dl` that turns "kind is 1" into the flag becomes a call into a code cave (16 bytes
//!    of memory allocated for it) that computes "kind is 1, or not private": a party and a public
//!    match may migrate, a private match gets 0, as the original gave it;
//! 2. the engine's check answers the flag for every session that isn't private
//!    (`[ecx+0x5a8] == 0`), where it took it from the kind (`[ecx+0x5f0] == 1`). So the engine no
//!    longer throws `LostConnectionToHost` over the lobby's `HostMigrationDisruption` when the host
//!    of a running public match drops, and the others fall back into the lobby, which migrates
//!    (`docs/architecture/running-match-host-loss.md`).
//!
//! A private match stays as the game made it, on purpose: when its host goes (leaves or drops) the
//! host dissolves it and every member lands alone in their own lobby or party
//! (`LostConnectionToHost`), as it ever did. Only a public lobby carries on. The setter is
//! privacy-aware for that reason: a private match session gets 0, so it is never given the flag,
//! not even through a party follow-up join (measured: with a flat `mov dl,1` the party member's
//! private match session had `+0x5a8=1` and `+0x5a9=1` and never left it after the host went).
//! The running-match check also gates on the privacy byte directly. Patching the decisions
//! themselves would migrate private matches too (tried 2026-10-22, it works), which isn't wanted.
//!
//! Both sites are found by signature (`hooks_addresses::host_migration`) and patched together or
//! not at all.

use hooks_addresses::host_migration::Sites;
use hooks_addresses::host_migration::CHECK;
use hooks_addresses::host_migration::SETTER;
use tracing::info;
use tracing::warn;

use crate::code_cave;

/// Where the 10 bytes `sete dl` / `push ebx` / `lea ecx,[ebp-0x1e0]` sit in [`SETTER`]. They become
/// `call cave` / `push ebx` / 4x `nop` (`code_cave::patch_call`); the cave computes the flag as
/// "kind is 1, or the session isn't private". Nothing reads `al` or the flags after the `cmp`
/// except the cave (`al` is given back; the next use of `eax` is the call at `0x0086a348`).
const SETTER_SITE_AT: usize = 25;
const SETTER_SITE_LEN: usize = 10;

/// The engine check's replacement, the same length: the flag, unless the session is private
/// (`+0x5a8`). A party's `+0x5a8` is 0 and its flag 1, so a party answers as before. The
/// `+0x610` test goes: that object is made in the session's constructor and lives as long as it.
///   `mov al,[ecx+0x5a9]` / `cmp byte [ecx+0x5a8],0` / `je .ret` / `xor al,al` / `.ret: ret` /
///   `int3`
const CHECK_PATCH_PUBLIC: [u8; 19] = [
    0x8A, 0x81, 0xA9, 0x05, 0x00, 0x00, // mov al,[ecx+0x5a9]
    0x80, 0xB9, 0xA8, 0x05, 0x00, 0x00, 0x00, // cmp byte [ecx+0x5a8],0
    0x74, 0x02, // je .ret
    0x32, 0xC0, // xor al,al
    0xC3, // .ret: ret
    0xCC, // int3 (padding)
];

/// The patches made, to undo them ([`undo`]) if the rest of the beta doesn't go in.
static PATCHED: std::sync::Mutex<Option<(usize, usize)>> = std::sync::Mutex::new(None);

/// Patches the game so match lobbies migrate, at `sites`. Whether it did: a patch that doesn't
/// take is undone.
pub unsafe fn install(sites: &Sites) -> bool {
    let setter_site = sites.setter + SETTER_SITE_AT;
    if std::slice::from_raw_parts(setter_site as *const u8, SETTER_SITE_LEN) != &SETTER[SETTER_SITE_AT..SETTER_SITE_AT + SETTER_SITE_LEN] {
        warn!("migrate-match-lobby: the flag's setter isn't as found -> skipped");
        return false;
    }
    // The cave in memory of its own: no padding in the game's code is borrowed for it.
    let Ok(alloc) = region::alloc(code_cave::CAVE_LEN, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("migrate-match-lobby: couldn't allocate the code cave -> skipped");
        return false;
    };
    let cave = alloc.as_ptr::<u8>().cast_mut();
    std::ptr::copy_nonoverlapping(code_cave::CAVE.as_ptr(), cave, code_cave::CAVE_LEN);
    // The cave lives as long as the game: the setter may call it at any time.
    std::mem::forget(alloc);
    let call = code_cave::patch_call(setter_site, cave as usize);
    let took = crate::writemem(setter_site as *mut u8, &call) && crate::writemem(sites.check as *mut u8, &CHECK_PATCH_PUBLIC);
    *PATCHED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((setter_site, sites.check));
    let (set, chk) = (
        std::slice::from_raw_parts(setter_site as *const u8, SETTER_SITE_LEN),
        std::slice::from_raw_parts(sites.check as *const u8, CHECK_PATCH_PUBLIC.len()),
    );
    if !took || set != call || chk != CHECK_PATCH_PUBLIC {
        undo();
        warn!("migrate-match-lobby: the patch didn't take (write-protected?) -> match lobbies are dissolved as before");
        return false;
    }
    info!(
        "migrate-match-lobby: match sessions may migrate, private ones excepted (privacy-aware flag setter @ {setter_site:#010x}, cave @ {:#010x}, engine check @ {:#010x}) - a match lobby carries on when its host leaves",
        cave as usize, sites.check
    );
    true
}

/// Puts the game's own bytes back where [`install`] patched them (the cave stays, unreferenced).
pub unsafe fn undo() {
    if let Some((setter_site, check)) = PATCHED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
        let back = crate::writemem(setter_site as *mut u8, &SETTER[SETTER_SITE_AT..SETTER_SITE_AT + SETTER_SITE_LEN]) & crate::writemem(check as *mut u8, &CHECK);
        if !back {
            warn!("migrate-match-lobby: the game's own bytes couldn't be put back");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_patches_keep_their_sites_lengths() {
        assert_eq!(&SETTER[SETTER_SITE_AT..SETTER_SITE_AT + 4], &[0x0F, 0x94, 0xC2, 0x53], "sete dl, push ebx");
        assert_eq!(
            &SETTER[SETTER_SITE_AT + 4..SETTER_SITE_AT + SETTER_SITE_LEN],
            &[0x8D, 0x8D, 0x20, 0xFE, 0xFF, 0xFF],
            "lea ecx,[ebp-0x1e0]"
        );
        assert_eq!(
            &SETTER[SETTER_SITE_AT + SETTER_SITE_LEN..SETTER_SITE_AT + SETTER_SITE_LEN + 2],
            &[0x88, 0x96],
            "mov [esi+0x5a9],dl follows"
        );
        assert_eq!(
            &code_cave::CAVE[9..15],
            &SETTER[SETTER_SITE_AT + 4..SETTER_SITE_AT + SETTER_SITE_LEN],
            "the cave redoes the lea"
        );
        assert_eq!(CHECK.len(), CHECK_PATCH_PUBLIC.len());
    }

    #[test]
    fn the_running_match_check_tests_the_privacy_byte() {
        assert_eq!(&CHECK_PATCH_PUBLIC[6..13], &[0x80, 0xB9, 0xA8, 0x05, 0x00, 0x00, 0x00], "cmp byte [ecx+0x5a8],0");
        assert_eq!(&CHECK_PATCH_PUBLIC[..6], &CHECK[9..15], "it starts with the original's flag load");
    }
}
