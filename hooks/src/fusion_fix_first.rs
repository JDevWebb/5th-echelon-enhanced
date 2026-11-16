//! Fusion Fix patches first, where it and this add-on change the same code.
//!
//! Fusion Fix finds the code it patches by its bytes, a few seconds after the game starts, and
//! doesn't check that a pattern was found: one that isn't there is a null read, and the game
//! crashes at start (FusionFix.asi+0x4ebfa, +0x55b8f). This add-on patches when it's loaded,
//! before that, and two of its patches change bytes Fusion Fix looks for:
//! - the mouse-turn beta's jump at the look routine's mouse store (DX11 0xd8e315), inside
//!   `0F 28 C3 D9 86` that `DisableNegativeMouseAcceleration` looks for at 0xd8e312 (Ghost_Leader,
//!   16 Nov 2026: three crashes in a row);
//! - the quick start's hook of the title scene's tick (DX11 0x19d42a0), the start of the pattern
//!   `SkipIntro` and `SkipPressAnyKeyScreen` look for to hook it at +0x16 (sainex, 14 Nov, and
//!   Glacio1, 15 Nov: three each). The launcher's Play Spies vs Mercs, Co-op and Join turn the
//!   quick start on for a start since 0.5.0; before, few had it on.
//!
//! So with Fusion Fix installed and one of those options on (its defaults are on), the patch
//! waits until Fusion Fix's own change is in (the bytes it patches have changed), then goes in:
//! neither patch overlaps what Fusion Fix writes, so both work. If Fusion Fix's change hasn't
//! come in [`WAIT_AT_MOST`], Fusion Fix isn't patching there (another build, its pattern not
//! found where it looks, or it started long before), and ours goes in then.

use std::time::Duration;
use std::time::Instant;

use tracing::info;
use tracing::warn;

/// How long a patch waits for Fusion Fix's (it patches within a few seconds of the start).
pub const WAIT_AT_MOST: Duration = Duration::from_secs(30);
/// How often it looks: short, so the game runs unpatched as briefly as can be.
const POLL: Duration = Duration::from_millis(10);

/// Whether an installed Fusion Fix has one of these options on (`hooks_config::fusion_fix`'s
/// keys), so that it will patch code by its bytes.
pub fn will_patch(keys: &[&str]) -> bool {
    crate::community::folder().is_some_and(|dir| hooks_config::fusion_fix::any_on(dir, keys))
}

/// The bytes at `at` now.
///
/// # Safety
/// `at..at + N` must be readable (the game's code).
pub unsafe fn bytes_at<const N: usize>(at: usize) -> [u8; N] {
    std::ptr::read_volatile(at as *const [u8; N])
}

/// How a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// Fusion Fix's change came in.
    FusionFixDone,
    /// It didn't, in time.
    TimedOut,
}

/// Looks at `changed` every `poll` until it says so or `at_most` has gone by.
pub fn wait_for(changed: impl Fn() -> bool, at_most: Duration, poll: Duration) -> Waited {
    let start = Instant::now();
    loop {
        if changed() {
            return Waited::FusionFixDone;
        }
        if start.elapsed() >= at_most {
            return Waited::TimedOut;
        }
        std::thread::sleep(poll);
    }
}

/// Runs `apply` (the patch called `what`) now, unless Fusion Fix will patch the same code
/// (`fusion_fix` says so); then on a thread of its own, once `changed` says Fusion Fix's change
/// is in, or after [`WAIT_AT_MOST`].
pub fn apply_after(what: &'static str, fusion_fix: bool, changed: impl Fn() -> bool + Send + 'static, apply: impl FnOnce() + Send + 'static) {
    if !fusion_fix || changed() {
        apply();
        return;
    }
    info!("{what}: Fusion Fix patches this code too: waiting for its change first");
    let spawned = std::thread::Builder::new().name("fe-fusion-fix-first".into()).spawn(move || {
        let start = Instant::now();
        match wait_for(changed, WAIT_AT_MOST, POLL) {
            Waited::FusionFixDone => info!("{what}: Fusion Fix's change is in ({} ms); now ours", start.elapsed().as_millis()),
            Waited::TimedOut => warn!(
                "{what}: Fusion Fix didn't change this code in {} s (another build, or it doesn't patch here); ours goes in now",
                WAIT_AT_MOST.as_secs()
            ),
        }
        apply();
    });
    if let Err(e) = spawned {
        // No thread: rather than crash Fusion Fix, this patch is left out.
        warn!("{what}: couldn't wait for Fusion Fix ({e}) -> skipped");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use super::*;

    #[test]
    fn without_fusion_fix_the_patch_goes_in_at_once() {
        let applied = Arc::new(AtomicBool::new(false));
        let a = applied.clone();
        apply_after("test", false, || false, move || a.store(true, Ordering::SeqCst));
        assert!(applied.load(Ordering::SeqCst), "at once, on this thread");
    }

    #[test]
    fn already_patched_by_fusion_fix_goes_in_at_once() {
        let applied = Arc::new(AtomicBool::new(false));
        let a = applied.clone();
        apply_after("test", true, || true, move || a.store(true, Ordering::SeqCst));
        assert!(applied.load(Ordering::SeqCst));
    }

    #[test]
    fn with_fusion_fix_the_patch_waits_for_its_change() {
        // Fusion Fix's change comes in on the third look.
        let looks = Arc::new(AtomicU32::new(0));
        let (applied, l) = (Arc::new(AtomicU32::new(0)), looks.clone());
        let a = applied.clone();
        let looks_then = looks.clone();
        apply_after(
            "test",
            true,
            move || l.fetch_add(1, Ordering::SeqCst) >= 2,
            move || a.store(looks_then.load(Ordering::SeqCst), Ordering::SeqCst),
        );
        let start = Instant::now();
        while applied.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(applied.load(Ordering::SeqCst) >= 3, "applied only after the change was seen");
    }

    #[test]
    fn a_wait_ends_when_fusion_fix_never_patches() {
        let start = Instant::now();
        assert_eq!(wait_for(|| false, Duration::from_millis(50), Duration::from_millis(5)), Waited::TimedOut);
        assert!(start.elapsed() >= Duration::from_millis(50));
        assert_eq!(wait_for(|| true, Duration::from_secs(5), Duration::from_millis(5)), Waited::FusionFixDone);
    }

    #[test]
    fn bytes_are_read_where_they_are() {
        let code: [u8; 5] = [0x0F, 0x28, 0xC3, 0xD9, 0x86];
        assert_eq!(unsafe { bytes_at::<3>(code.as_ptr() as usize) }, [0x0F, 0x28, 0xC3]);
    }
}
