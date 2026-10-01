//! Asks the router to forward Storm's port (UDP 13000) to this PC while the
//! game runs (the `portmap` crate: UPnP first, then NAT-PMP). With a mapping,
//! other players reach this PC directly, even through a NAT that hole
//! punching can't get through. The mapping is renewed while the game runs and
//! removed when it exits (and it expires by itself after an hour otherwise).

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use tracing::info;

const PORT: u16 = nat_proto::STORM_PORT;
const LEASE: u32 = 3600;
const RENEW: Duration = Duration::from_secs(25 * 60);
const RETRY: Duration = Duration::from_secs(5 * 60);
const DESCRIPTION: &str = "Splinter Cell Blacklist (5th Echelon)";

static STOP: AtomicBool = AtomicBool::new(false);
static MAPPED: Mutex<Option<portmap::Mapping>> = Mutex::new(None);

pub fn start(pinned: Option<Ipv4Addr>) {
    let _ = std::thread::Builder::new().name("fe-portmap".into()).spawn(move || {
        let mut next = Instant::now();
        let mut logged_failure = false;
        while !STOP.load(Ordering::Relaxed) {
            if Instant::now() >= next {
                match portmap::map(PORT, pinned, LEASE, DESCRIPTION) {
                    Ok(mapping) => {
                        let first = MAPPED.lock().map(|m| m.is_none()).unwrap_or(true);
                        if first {
                            info!("NAT: the router forwards {} to this PC ({})", mapping.public, mapping.how);
                        }
                        // A mapping behind another NAT (double NAT) is refused by the crate:
                        // it would be no use to players outside.
                        super::nat::set_mapping(Some(mapping.public));
                        if let Ok(mut m) = MAPPED.lock() {
                            *m = Some(mapping);
                        }
                        next = Instant::now() + RENEW;
                    }
                    Err(e) => {
                        if !logged_failure {
                            info!("NAT: the router didn't forward a port ({e}); hole punching and the relay still work");
                            logged_failure = true;
                        }
                        super::nat::set_mapping(None);
                        next = Instant::now() + RETRY;
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// Stops renewing the mapping. Safe while the DLL unloads (no network).
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// Removes the mapping from the router: from `UPLAY_Quit`, while the game
/// exits normally. Never from the DLL's unload, which holds the loader lock
/// (network calls there can hang the exit).
pub fn remove_mapping() {
    STOP.store(true, Ordering::Relaxed);
    if let Some(mapping) = MAPPED.lock().ok().and_then(|mut m| m.take()) {
        mapping.remove();
    }
}
