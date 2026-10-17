//! Removes the engine's automatic climb/vault ("Active Sprint"), so climbing over an edge is
//! only ever manual — for the third-person, MP-Merc and MP-Spy nav states alike.
//!
//! The nav scan `UAutoNavState::vf29` (shared by `UAutoNavState`, `UMPMercAutoNavState` and
//! `UMPSpyAutoNavState`) builds a box from three member floats — the scan volume at `this+0xC8`,
//! `this+0xCC`, `this+0xD0` (ScanWidth/Height/Length) — and overlap-tests it for a climbable edge.
//! Zeroing those inputs collapses the box, so no edge is found and climbing stays manual. That is
//! exactly the documented `actionscheme.ini` fix (`m_fScanWidth/Height/Length = 0`), but applied
//! to the packed game — `actionscheme.ini` isn't a loose file, so it can't be edited like
//! `Blacklist.ini`.
//!
//! Rather than detour a hot per-frame function, the three `movss xmmN,[esi+off]` that load the
//! volume are rewritten in place to `xorps xmmN,xmmN` (+NOP padding) once at startup — the same
//! static, structure-matched approach as `mouse_fix`. The site is found by a marker unique to
//! vf29 and left untouched on any build it isn't found exactly once in.

use tracing::info;
use tracing::warn;

/// A 20-byte sequence unique to `UAutoNavState::vf29`, right after its prologue:
/// `mov esi,ecx` / `mov eax,[esi+0x48]` / `mov eax,[eax+0x16e0]` / `mov ecx,[eax+0x40]` /
/// `mov ebx,[eax+0x34]` / `mov eax,[esi+0x20]`. The `[eax+0x16e0]` load makes it distinctive.
const VF29_MARKER: [u8; 20] = [
    0x8B, 0xF1, 0x8B, 0x46, 0x48, 0x8B, 0x80, 0xE0, 0x16, 0x00, 0x00, 0x8B, 0x48, 0x40, 0x8B, 0x58, 0x34, 0x8B, 0x46, 0x20,
];

/// How far past the marker the three scan-volume loads sit (they are within ~0xE0 bytes).
const BODY_WINDOW: usize = 0x400;

/// The scan-volume loads to neutralise: each is `movss xmmN,[esi+disp32]` (`F3 0F 10 <modrm>
/// <disp32>`, 8 bytes), rewritten to `xorps xmmN,xmmN` (`0F 57 <modrm>`, 3 bytes) + 5 NOPs.
///   modrm 0x86 = xmm0/[esi]+disp32, 0xAE = xmm5, 0x96 = xmm2; xorps reg byte C0/ED/D2.
struct Load {
    /// The full 8-byte `movss` as it appears, for `this+off`.
    movss: [u8; 8],
    /// `xorps xmmN,xmmN` modrm byte for the same register.
    xorps_modrm: u8,
}

const LOADS: [Load; 3] = [
    Load {
        movss: [0xF3, 0x0F, 0x10, 0x86, 0xC8, 0x00, 0x00, 0x00],
        xorps_modrm: 0xC0,
    }, // xmm0, this+0xC8
    Load {
        movss: [0xF3, 0x0F, 0x10, 0xAE, 0xCC, 0x00, 0x00, 0x00],
        xorps_modrm: 0xED,
    }, // xmm5, this+0xCC
    Load {
        movss: [0xF3, 0x0F, 0x10, 0x96, 0xD0, 0x00, 0x00, 0x00],
        xorps_modrm: 0xD2,
    }, // xmm2, this+0xD0
];

/// Disables the auto-climb scan by collapsing its scan volume. Safe to call once at startup;
/// leaves an unrecognised build untouched.
pub unsafe fn disable_auto_climb() {
    let Some(code) = crate::game_code() else {
        warn!("auto-climb-fix: the game's .text wasn't found -> skipped");
        return;
    };
    let markers: Vec<usize> = code.windows(VF29_MARKER.len()).enumerate().filter(|(_, w)| *w == VF29_MARKER).map(|(i, _)| i).collect();
    let [marker] = markers[..] else {
        warn!("auto-climb-fix: {} vf29 marker(s), not exactly one -> skipped (unknown build?)", markers.len());
        return;
    };
    let end = (marker + BODY_WINDOW).min(code.len());
    let window = &code[marker..end];

    // Find each of the three loads exactly once in the function body before touching anything,
    // so a half-applied patch can't happen.
    let mut sites = [0usize; 3];
    for (i, load) in LOADS.iter().enumerate() {
        let found: Vec<usize> = window.windows(load.movss.len()).enumerate().filter(|(_, w)| *w == load.movss).map(|(o, _)| o).collect();
        let [off] = found[..] else {
            warn!("auto-climb-fix: scan-volume load {i} found {} times in vf29, not once -> skipped", found.len());
            return;
        };
        sites[i] = marker + off;
    }

    let text = code.as_ptr();
    for (load, site) in LOADS.iter().zip(sites) {
        // xorps xmmN,xmmN ; nop ; nop ; nop ; nop ; nop  (3 + 5 = the 8 bytes of the movss)
        let patch = [0x0F, 0x57, load.xorps_modrm, 0x90, 0x90, 0x90, 0x90, 0x90];
        crate::writemem(text.add(site).cast_mut(), &patch);
    }
    info!("auto-climb-fix: scan volume collapsed @ .text+{marker:#x} - automatic climbing OFF");
}
