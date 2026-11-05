//! Speed-hack check (the 0.5.0 plan's F7, report only): a speed hack makes the game's clocks
//! run fast by hooking its timing calls (QueryPerformanceCounter, GetTickCount), and the
//! add-on's own clock goes through the same calls. Windows also writes the time since boot
//! into a page every process shares (KUSER_SHARED_DATA's InterruptTime), read here straight
//! from memory: no call to hook. Every [`WINDOW`] the two are compared; a clock running more
//! than [`OFF_BY`] fast or slow for [`WINDOWS`] in a row is told the server
//! (`Misc.FairPlayCheck`, a session event for the admins), once a [`TOLD_EVERY`] at most.

use std::time::Duration;
use std::time::Instant;

use tracing::info;
use tracing::warn;

/// How long each comparison runs.
const WINDOW: Duration = Duration::from_secs(10);
/// Off by more than this (as a share of real time) counts.
const OFF_BY: f64 = 0.1;
/// Windows in a row before it's told.
const WINDOWS: u32 = 3;
const TOLD_EVERY: Duration = Duration::from_secs(300);

/// KUSER_SHARED_DATA, at the same address in every process.
const KUSER_INTERRUPT_TIME: usize = 0x7FFE_0008;

#[link(name = "kernel32")]
extern "system" {
    fn GetTickCount64() -> u64;
}

/// The time since boot as Windows keeps it in the shared page (100 ns units): read until its
/// two high halves agree, as Windows' own readers do.
fn interrupt_time() -> u64 {
    let base = KUSER_INTERRUPT_TIME as *const u32;
    loop {
        // SAFETY: the page is mapped read-only in every Windows process (and by Wine).
        let (high1, low, high2) = unsafe { (base.add(1).read_volatile(), base.read_volatile(), base.add(2).read_volatile()) };
        if high1 == high2 {
            return (u64::from(high1) << 32) | u64::from(low);
        }
        std::hint::spin_loop();
    }
}

/// How far a clock ran against real time: its elapsed over real elapsed.
fn speed(clock: f64, real: f64) -> Option<f64> {
    (real > 0.5).then(|| clock / real)
}

/// Whether a speed is off by more than [`OFF_BY`].
fn off(speed: f64) -> bool {
    (speed - 1.0).abs() > OFF_BY
}

pub fn start() {
    let wine = hooks_config::running_under_wine();
    let spawned = std::thread::Builder::new().name("fe-speed-check".into()).spawn(move || {
        let mut runs = 0u32;
        let mut told: Option<Instant> = None;
        loop {
            let (qpc, tick, real) = (Instant::now(), unsafe { GetTickCount64() }, interrupt_time());
            std::thread::sleep(WINDOW);
            #[allow(clippy::cast_precision_loss)]
            let real_secs = interrupt_time().saturating_sub(real) as f64 / 10_000_000.0;
            #[allow(clippy::cast_precision_loss)]
            let tick_secs = unsafe { GetTickCount64() }.saturating_sub(tick) as f64 / 1000.0;
            let (Some(by_qpc), Some(by_tick)) = (speed(qpc.elapsed().as_secs_f64(), real_secs), speed(tick_secs, real_secs)) else {
                // The shared page didn't move (a Wine that doesn't keep it): nothing to compare.
                continue;
            };
            if off(by_qpc) || off(by_tick) {
                runs += 1;
            } else {
                runs = 0;
            }
            if runs >= WINDOWS && told.is_none_or(|at| at.elapsed() >= TOLD_EVERY) {
                told = Some(Instant::now());
                let worst = if (by_qpc - 1.0).abs() >= (by_tick - 1.0).abs() { by_qpc } else { by_tick };
                let detail = format!(
                    "the game's clocks ran {worst:.2}x real time for {} s (QueryPerformanceCounter {by_qpc:.2}x, GetTickCount {by_tick:.2}x)",
                    WINDOW.as_secs() * u64::from(runs)
                );
                warn!("Speed check: {detail}");
                crate::api::fair_play_check("speed", &detail, worst, wine);
            } else if runs == 1 {
                info!(
                    "Speed check: the game's clocks ran {by_qpc:.2}x (QueryPerformanceCounter) and {by_tick:.2}x (GetTickCount) real time for {} s",
                    WINDOW.as_secs()
                );
            }
        }
    });
    if let Err(e) = spawned {
        warn!("Speed check: couldn't start ({e})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_off_by_more_than_a_tenth_counts() {
        assert!(!off(speed(10.0, 10.0).unwrap()));
        assert!(!off(speed(10.5, 10.0).unwrap()), "a laptop's clock drifting a little");
        assert!(off(speed(15.0, 10.0).unwrap()), "a 1.5x speed hack");
        assert!(off(speed(5.0, 10.0).unwrap()), "slowed down");
        assert_eq!(speed(1.0, 0.0), None, "the real clock didn't move");
    }
}
