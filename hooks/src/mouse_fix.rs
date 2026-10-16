//! Removes Splinter Cell: Blacklist's negative mouse acceleration without tipping the camera —
//! for both the DX9 and DX11 executables, and robust across game builds.
//!
//! The look routine clamps the per-frame look delta to `[dt*-500, dt*+500]` (`dt` = frame time).
//! That cap *is* the negative acceleration: fast flicks get clipped, so effective sensitivity
//! drops at speed. `ThirteenAG`'s Fusion Fix NOPs the four `movaps` that write the bound into the
//! result register, which drops the cap entirely. That is harmless on Windows, but under Wine/
//! Proton an input spike then slips through and the camera snaps to the pitch limit (the clamp was
//! the only guard).
//!
//! So instead of removing the clamp, we only *widen* it: the two `mulsd` instructions that compute
//! `dt*-500` / `dt*+500` get new constant operands (`±100000`). Normal (even fast) movement is
//! never capped — acceleration gone — while a spike stays bounded, so the camera can't flip. The
//! change is purely algorithmic (it rewrites the game's own clamp), so it works the same on
//! Windows and under Proton.
//!
//! Rather than fixed bytes (which differ per build), the site is matched by its structure and
//! verified by the constant *values* (`-500.0` / `+500.0`):
//!   `F2 0F 59 15 <disp32 a>  0F 5A C0  F2 0F 59 05 <disp32 b>`
//! where `*a == -500.0` and `*b == +500.0` (`f64`). This matches DX9 (VA `0x1023d93`) and DX11
//! (VA `0xd8e293`) uniquely and stays robust against patch levels.

use tracing::info;
use tracing::warn;

/// Wide replacement bounds: `dt*±100000` (~±1666/frame at 60 fps) — unreachable for real movement,
/// yet still catches a garbage/spike value. `f64`, because `mulsd` reads a QWORD.
const WIDE_LOWER: f64 = -100_000.0;
const WIDE_UPPER: f64 = 100_000.0;

/// Reads an `f64` at absolute address `addr`, but only if it lies fully inside the loaded module
/// (guards against faulting on a coincidental structural match whose disp32 is garbage).
unsafe fn read_module_f64(addr: u32, base: usize, size: usize) -> Option<f64> {
    let a = addr as usize;
    if a < base || a.checked_add(8)? > base + size {
        return None;
    }
    Some(std::ptr::read_unaligned(a as *const f64))
}

/// Boxes `v` and leaks it, returning the raw address as a disp32. Leaked on purpose: the operand
/// must stay valid for the process's lifetime. (`usize as u32` is lossless on the forced i686 target.)
#[allow(clippy::cast_possible_truncation)]
fn leak_f64_addr(v: f64) -> u32 {
    Box::into_raw(Box::new(v)) as usize as u32
}

/// Removes the negative mouse acceleration by widening the look-delta clamp bounds.
pub unsafe fn disable_negative_mouse_acceleration() {
    // Fusion Fix's own fix NOPs the clamp, so its bounds (ours) are never used.
    if crate::community::folder().is_some_and(hooks_config::fusion_fix::removes_mouse_accel) {
        info!(
            "mouse-accel-fix: Fusion Fix's DisableNegativeMouseAcceleration is on and removes the clamp itself -> skipped (set it to 0 in SplinterCellBlacklist.FusionFix.ini to use this fix)"
        );
        return;
    }
    let Some((base, img_size)) = crate::module_bounds() else {
        warn!("mouse-accel-fix: module info unavailable -> skipped");
        return;
    };
    let Some(image) = crate::game_code() else {
        warn!("mouse-accel-fix: .text section not found -> skipped");
        return;
    };
    info!("mouse-accel-fix: scanning .text ({} bytes)", image.len());

    // Match the clamp site by structure, then confirm it by the constant values (exact bit pattern,
    // so no float comparison): mulsd xmm2,[a] / cvtps2pd xmm0,xmm0 / mulsd xmm0,[b].
    let lower_bits = (-500.0_f64).to_bits();
    let upper_bits = (500.0_f64).to_bits();
    let structural = |w: &[u8]| w[0..4] == [0xF2, 0x0F, 0x59, 0x15] && w[8..11] == [0x0F, 0x5A, 0xC0] && w[11..15] == [0xF2, 0x0F, 0x59, 0x05];
    let hits: Vec<usize> = image
        .windows(19)
        .enumerate()
        .filter(|(_, w)| structural(w))
        .filter_map(|(i, w)| {
            let a = u32::from_le_bytes(w[4..8].try_into().unwrap());
            let b = u32::from_le_bytes(w[15..19].try_into().unwrap());
            let ca = read_module_f64(a, base, img_size)?;
            let cb = read_module_f64(b, base, img_size)?;
            (ca.to_bits() == lower_bits && cb.to_bits() == upper_bits).then_some(i)
        })
        .collect();

    let [off] = hits[..] else {
        warn!("mouse-accel-fix: {} verified clamp site(s), not exactly one -> skipped (unknown build?)", hits.len());
        return;
    };

    let lower_ptr = leak_f64_addr(WIDE_LOWER);
    let upper_ptr = leak_f64_addr(WIDE_UPPER);
    info!("mouse-accel-fix: clamp verified @ .text+{off:#x}; lower->{lower_ptr:#010x} upper->{upper_ptr:#010x}");
    // disp32 of the lower (-500) mulsd sits at +4, of the upper (+500) mulsd at +15.
    let text = image.as_ptr();
    crate::writemem(text.add(off + 4).cast_mut(), &lower_ptr.to_le_bytes());
    crate::writemem(text.add(off + 15).cast_mut(), &upper_ptr.to_le_bytes());
    info!("mouse-accel-fix: bounds widened - negative acceleration OFF, camera clamp still active");
}
