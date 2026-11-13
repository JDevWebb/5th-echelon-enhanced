//! Asks the router to forward Storm's port (UDP 13000) to this PC while the
//! game runs (the `portmap` crate: UPnP first, then NAT-PMP). With a mapping,
//! other players reach this PC directly, even through a NAT that hole
//! punching can't get through. The mapping is renewed while the game runs and
//! removed when it exits (and it expires by itself after an hour otherwise).
//!
//! It waits for the NAT helper's first answer and asks for the router-side port the helper
//! saw: another PC behind the same router may already hold 13000 there, and a forward of 13000
//! made first would have this PC's traffic leave on that same port, colliding with the other
//! PC's: the helper's answers then go to the other PC (0.5.2's J7, found on the cutover night).
//! Without the forward first, the router gives this PC's packets a port of their own, and that's
//! the one asked for. Until the forward is made the helper relays this PC (it said no mapping),
//! which works, and it moves to direct with the next probe. With no helper answering (a LAN
//! server), the forward is made with 13000 after [`WAIT_FOR_HELPER`], as before.

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
/// How long the first mapping waits for the helper to say which port the router gives us
/// (the game starts Storm's socket, and so its probes, only once it goes online).
const WAIT_FOR_HELPER: Duration = Duration::from_secs(180);

static STOP: AtomicBool = AtomicBool::new(false);
static MAPPED: Mutex<Option<portmap::Mapping>> = Mutex::new(None);

pub fn start(pinned: Option<Ipv4Addr>) {
    let _ = std::thread::Builder::new().name("fe-portmap".into()).spawn(move || {
        let started = Instant::now();
        let mut next = Instant::now();
        let mut logged_failure = false;
        // The router-side port asked for: chosen once, then kept for the renewals.
        let mut external: Option<u16> = None;
        while !STOP.load(Ordering::Relaxed) {
            if external.is_none() {
                let seen = super::nat::observed_port();
                if seen.is_none() && started.elapsed() < WAIT_FOR_HELPER {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                let port = portmap::port_to_ask(PORT, seen);
                if port != PORT {
                    info!("NAT: the router gives this PC's {PORT} as {port} (another device here holds {PORT}): asking to forward {port}");
                }
                external = Some(port);
            }
            if Instant::now() >= next {
                match portmap::map_as(PORT, external.unwrap_or(PORT), pinned, LEASE, DESCRIPTION) {
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
