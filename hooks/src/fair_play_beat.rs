//! The fair-play heartbeat and the add-on's check of itself (0.5.2's H, report only).
//!
//! - **Heartbeat:** while a match is under way (round_state.rs), every [`EVERY`] the add-on tells
//!   the server its checks run (`Misc.FairPlayCheck`, check `heartbeat`, a count). The server notes
//!   a game whose heartbeats stop while it still says its match is under way: the checks were
//!   switched off, or their thread killed (dedicated_server's fair_play_beats.rs).
//! - **Self-check:** this add-on's own code (its `.text`) is copied once the game has settled, and
//!   compared with that at each heartbeat. Its hooks patch the game's code, never its own, so a
//!   change is someone patching the add-on (`self`, told once).
//!
//! Limits, as the plan says: the add-on is open source, and a rebuilt one can lie. These make the
//! casual ways (a debugger, a memory editor) show.

use std::time::Duration;

use tracing::info;
use tracing::warn;

const EVERY: Duration = Duration::from_secs(60);
/// Time for every hook to be in before the add-on copies its own code.
const SETTLE: Duration = Duration::from_secs(120);

pub fn start() {
    let wine = hooks_config::running_under_wine();
    let spawned = std::thread::Builder::new().name("fe-fair-play-beat".into()).spawn(move || {
        std::thread::sleep(SETTLE);
        let own = crate::code_check::module_at(start as usize)
            .and_then(|(base, ..)| unsafe { crate::code_check::text_of(base) })
            .map(|((text, size, ..), ..)| (text, size));
        let copy = |(text, size): (usize, usize)| unsafe { std::slice::from_raw_parts(text as *const u8, size) }.to_vec();
        let original = own.map(copy);
        if original.is_none() {
            warn!("Fair play: the add-on's own code wasn't found; it isn't checked");
        }
        let mut count: u64 = 0;
        let mut said_self = false;
        loop {
            std::thread::sleep(EVERY);
            if let (Some(region), Some(first)) = (own, &original) {
                if !said_self && copy(region) != *first {
                    said_self = true;
                    let changed = copy(region).iter().zip(first.iter()).filter(|(a, b)| a != b).count();
                    let detail = format!("{changed} bytes of the add-on's own code changed while the game ran");
                    warn!("Fair play: {detail}");
                    #[allow(clippy::cast_precision_loss)]
                    crate::api::fair_play_check("self", &detail, changed as f64, wine);
                }
            }
            if crate::round_state::current().1 {
                count += 1;
                if count == 1 {
                    info!("Fair play: heartbeat in this match");
                }
                #[allow(clippy::cast_precision_loss)]
                crate::api::fair_play_check("heartbeat", "code, speed, self", count as f64, wine);
            }
        }
    });
    if let Err(e) = spawned {
        warn!("Fair play: the heartbeat couldn't start ({e})");
    }
}
