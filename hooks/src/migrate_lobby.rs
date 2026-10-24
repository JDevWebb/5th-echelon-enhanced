//! Lets a match lobby carry on under a new host when its host leaves, as a party does
//! (always on; not configurable). Background in
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
//!    of `0xCC` padding, found at run time) that computes "kind is 1, or not private": a party and
//!    a public match may migrate, a private match gets 0, as the original gave it;
//! 2. the engine's check answers the flag for every session that isn't private
//!    (`[ecx+0x5a8] == 0`; `hooks::init` always asks for this, `running`). Without `running` it
//!    would keep its old answer, taken from the kind (`[ecx+0x5f0] == 1`), so a host leaving a
//!    running match would still end it as before. With it the engine no longer
//!    throws `LostConnectionToHost` over the lobby's `HostMigrationDisruption`, and the others fall
//!    back into the lobby, which migrates (`docs/architecture/running-match-host-loss.md`).
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
//! Both sites are found by their exact bytes in `.text` (once each, in the DX11 and the DX9
//! build alike) and patched together or not at all (also not without a cave).

use tracing::info;
use tracing::warn;

use crate::code_cave;

/// The flag's setter: `cmp dword [esi+0x5f0],1` / `mov [esi+0x5a8],al` / `mov ecx,[edi+0x298]` /
/// `mov [esi+0x5b4],ecx` / **`sete dl`** / `push ebx` / `lea ecx,[ebp-0x1e0]` /
/// `mov [esi+0x5a9],dl`.
const SETTER: [u8; 41] = [
    0x83, 0xBE, 0xF0, 0x05, 0x00, 0x00, 0x01, // cmp dword [esi+0x5f0],1
    0x88, 0x86, 0xA8, 0x05, 0x00, 0x00, // mov [esi+0x5a8],al
    0x8B, 0x8F, 0x98, 0x02, 0x00, 0x00, // mov ecx,[edi+0x298]
    0x89, 0x8E, 0xB4, 0x05, 0x00, 0x00, // mov [esi+0x5b4],ecx
    0x0F, 0x94, 0xC2, // sete dl
    0x53, // push ebx
    0x8D, 0x8D, 0x20, 0xFE, 0xFF, 0xFF, // lea ecx,[ebp-0x1e0]
    0x88, 0x96, 0xA9, 0x05, 0x00, 0x00, // mov [esi+0x5a9],dl
];
/// Where the 10 bytes `sete dl` / `push ebx` / `lea ecx,[ebp-0x1e0]` sit in [`SETTER`]. They become
/// `call cave` / `push ebx` / 4x `nop` (`code_cave::patch_call`); the cave computes the flag as
/// "kind is 1, or the session isn't private". Nothing reads `al` or the flags after the `cmp`
/// except the cave (`al` is given back; the next use of `eax` is the call at `0x0086a348`).
const SETTER_SITE_AT: usize = 25;
const SETTER_SITE_LEN: usize = 10;

/// The engine's check, the whole function: `cmp dword [ecx+0x610],0` / `je +7` /
/// `mov al,[ecx+0x5a9]` / `ret` / `xor al,al` / `ret`.
const CHECK: [u8; 19] = [
    0x83, 0xB9, 0x10, 0x06, 0x00, 0x00, 0x00, // cmp dword [ecx+0x610],0
    0x74, 0x07, // je .no
    0x8A, 0x81, 0xA9, 0x05, 0x00, 0x00, // mov al,[ecx+0x5a9]
    0xC3, // ret
    0x32, 0xC0, // .no: xor al,al
    0xC3, // ret
];
/// Its replacement, the same length: the flag, but only for a party (kind 1), as it was. The
/// `+0x610` test goes: that object is made in the session's constructor and lives as long as it.
///   `mov al,[ecx+0x5a9]` / `cmp dword [ecx+0x5f0],1` / `je .ret` / `xor al,al` / `.ret: ret` /
///   `int3`
const CHECK_PATCH: [u8; 19] = [
    0x8A, 0x81, 0xA9, 0x05, 0x00, 0x00, // mov al,[ecx+0x5a9]
    0x83, 0xB9, 0xF0, 0x05, 0x00, 0x00, 0x01, // cmp dword [ecx+0x5f0],1
    0x74, 0x02, // je .ret
    0x32, 0xC0, // xor al,al
    0xC3, // .ret: ret
    0xCC, // int3 (padding)
];

/// The replacement with `running` (always, from `hooks::init`), the same length: the flag, unless the session is
/// private (`+0x5a8`). A party's `+0x5a8` is 0 and its flag 1, so a party answers as before.
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

/// Where `pattern` occurs in `code`, when it occurs exactly once.
fn find_once(code: &[u8], pattern: &[u8]) -> Result<usize, usize> {
    let hits: Vec<usize> = code.windows(pattern.len()).enumerate().filter(|(_, w)| *w == pattern).map(|(i, _)| i).collect();
    match hits[..] {
        [one] => Ok(one),
        _ => Err(hits.len()),
    }
}

/// Patches the game so match lobbies migrate. Called once at startup; leaves a build it doesn't
/// recognise untouched.
///
/// `running`: the engine's check also lets a public match migrate while it runs.
pub unsafe fn migrate_match_lobbies(running: bool) {
    let Some(code) = crate::game_code() else {
        warn!("migrate-match-lobby: the game's .text wasn't found -> skipped");
        return;
    };
    let setter = match find_once(code, &SETTER) {
        Ok(at) => at,
        Err(n) => {
            warn!("migrate-match-lobby: the migration flag's setter found {n} times, not once -> skipped (unknown build?)");
            return;
        }
    };
    let check = match find_once(code, &CHECK) {
        Ok(at) => at,
        Err(n) => {
            warn!("migrate-match-lobby: the engine's host-loss check found {n} times, not once -> skipped (unknown build?)");
            return;
        }
    };
    let check_patch = if running { &CHECK_PATCH_PUBLIC } else { &CHECK_PATCH };
    let Some(cave) = code_cave::find_cave(code, code_cave::CAVE_LEN) else {
        warn!("migrate-match-lobby: no code cave ({} bytes of 0xCC) in .text -> skipped", code_cave::CAVE_LEN);
        return;
    };
    let text = code.as_ptr();
    let call = code_cave::patch_call(setter + SETTER_SITE_AT, cave);
    // The cave first, then the call into it, then the check.
    crate::writemem(text.add(cave).cast_mut(), &code_cave::CAVE);
    if std::slice::from_raw_parts(text.add(cave), code_cave::CAVE_LEN) != code_cave::CAVE {
        warn!("migrate-match-lobby: the code cave didn't take (write-protected?) -> match lobbies are dissolved as before");
        return;
    }
    crate::writemem(text.add(setter + SETTER_SITE_AT).cast_mut(), &call);
    crate::writemem(text.add(check).cast_mut(), check_patch);
    let (set, chk) = (
        std::slice::from_raw_parts(text.add(setter + SETTER_SITE_AT), SETTER_SITE_LEN),
        std::slice::from_raw_parts(text.add(check), check_patch.len()),
    );
    if set != call || chk != check_patch {
        // The cave is harmless unreferenced; undo the setter's call if it took, so no half patch stays.
        crate::writemem(text.add(setter + SETTER_SITE_AT).cast_mut(), &SETTER[SETTER_SITE_AT..SETTER_SITE_AT + SETTER_SITE_LEN]);
        crate::writemem(text.add(check).cast_mut(), &CHECK);
        warn!("migrate-match-lobby: the patch didn't take (write-protected?) -> match lobbies are dissolved as before");
        return;
    }
    let scope = if running {
        "answers for public matches too (a running public match migrates)"
    } else {
        "kept for parties only"
    };
    info!(
        "migrate-match-lobby: match sessions may migrate, private ones excepted (privacy-aware flag setter @ .text+{setter:#x}, cave @ .text+{cave:#x}, engine check @ .text+{check:#x} {scope}) - a match lobby carries on when its host leaves"
    );
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
        assert_eq!(CHECK.len(), CHECK_PATCH.len());
        assert_eq!(CHECK.len(), CHECK_PATCH_PUBLIC.len());
    }

    #[test]
    fn the_running_match_check_tests_the_privacy_byte() {
        assert_eq!(&CHECK_PATCH_PUBLIC[6..13], &[0x80, 0xB9, 0xA8, 0x05, 0x00, 0x00, 0x00], "cmp byte [ecx+0x5a8],0");
        assert_eq!(&CHECK_PATCH_PUBLIC[..6], &CHECK_PATCH[..6], "both start with the flag load");
    }

    #[test]
    fn a_site_counts_only_when_it_is_there_once() {
        let mut code = vec![0x90; 64];
        assert_eq!(find_once(&code, &CHECK), Err(0));
        code.extend_from_slice(&CHECK);
        assert_eq!(find_once(&code, &CHECK), Ok(64));
        code.extend_from_slice(&CHECK);
        assert_eq!(find_once(&code, &CHECK), Err(2));
    }
}
