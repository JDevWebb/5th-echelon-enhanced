//! The boot videos (the disclaimer and the logos) skipped on every start, not only the quick
//! start's: the same patch (`quickstart.rs` step 1). The video gate in the Bink player reads the
//! command line; its own skip branch empties the video list and clears the "video playing"
//! flag, so the renderer doesn't wait for the end. Its `jne` after the `menu` check is made a
//! `jmp` into that branch.
//!
//! Why on every start: players' games stood still 11 s or more while the boot videos played as
//! the game loaded (`bink2w32.dll` on top of the stack, "not online yet", 25-42 s after start;
//! 4 freeze reports from 2 players, 22-23 Nov), with the quick start off. The patch has run in
//! every quick start since 0.5.0, with Fusion Fix too: it isn't one of the places Fusion Fix
//! scans (its SkipIntro hooks the title scene's tick, which this leaves alone).
//!
//! Runtime only; no file is changed. Found by signature (`hooks_addresses::quick_start`), or
//! nothing is changed.
//!
//! **After the quick start:** its signatures include the gate's own bytes, so a gate already
//! patched here wasn't found there and the whole quick start stood down ("VideoGate found 0
//! times"; the gamebox, 25 Nov). The quick start installs first and says when it has skipped the
//! videos itself ([`skipped_by_quick_start`]); then this does nothing.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use tracing::info;
use tracing::warn;

/// The quick start's own patch skipped them.
static SKIPPED: AtomicBool = AtomicBool::new(false);

/// The quick start's patch took: the videos are skipped already.
pub fn skipped_by_quick_start() {
    SKIPPED.store(true, Ordering::Relaxed);
}

/// Skips the boot videos (from `hooks::init`, in `DllMain`, before the game's code runs), after
/// the quick start has installed (or stood down).
pub unsafe fn skip() {
    if SKIPPED.load(Ordering::Relaxed) {
        return;
    }
    let Some(image) = crate::game_image() else {
        warn!("boot-videos: the game's sections weren't found -> the videos play");
        return;
    };
    let (jne, skip_branch) = match hooks_addresses::quick_start::locate_video_gate(&image) {
        Ok(sites) => sites,
        Err(why) => {
            warn!("boot-videos: {why} -> the videos play (unknown build?)");
            return;
        }
    };
    let rel = (skip_branch as u32).wrapping_sub(jne as u32 + 5);
    let mut patch = [0xE9, 0, 0, 0, 0, 0x90];
    patch[1..5].copy_from_slice(&rel.to_le_bytes());
    if !crate::writemem(jne as *mut u8, &patch) {
        warn!("boot-videos: the video gate patch didn't take -> the videos play");
        return;
    }
    info!("boot-videos: skipped (video gate @ {jne:#x} -> {skip_branch:#x})");
}
