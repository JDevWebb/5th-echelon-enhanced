//! The relay the game uses (global matchmaking, G1 item 3). A network's matchmaking server
//! offers its relays in `/api/info` (one near each region's players); the launcher pings each
//! helper as the game will reach it and has the game use the nearest (`networking.nat_host`
//! in uplay.toml). A server that offers none (every server before the matchmaking one) keeps
//! its own helper, as before.
//!
//! The server's list and the pick are kept for [`KEPT_FOR`], so a start doesn't wait on them
//! twice.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use setup::directory::Helper;

/// How long a server's relays and the pick among them are kept.
const KEPT_FOR: Duration = Duration::from_secs(600);
/// How long each helper's answer is waited for.
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long the server's list is waited for at a start.
const INFO_TIMEOUT: Duration = Duration::from_secs(3);
/// Pings within this of the best count as the same: the server's order (busiest first) wins.
const TIE: u32 = 15;

/// The relay picked: where the game reaches its helper, and the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub host: String,
    pub port: u16,
    pub name: String,
    pub ms: u32,
}

type Kept = HashMap<String, (Instant, Option<Picked>)>;
static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(Mutex::default);

fn kept() -> std::sync::MutexGuard<'static, Kept> {
    KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The relay nearest the player among those `server` offers, or None to keep the server's
/// own helper (it offers none, or none answered).
pub fn pick_for(server: &str) -> Option<Picked> {
    if let Some((at, picked)) = kept().get(server) {
        if at.elapsed() < KEPT_FOR {
            return picked.clone();
        }
    }
    let relays = setup::server_info::fetch(server, INFO_TIMEOUT).map(|i| i.relays).unwrap_or_default();
    let picked = if relays.is_empty() {
        None
    } else {
        let pings = crate::services::rt().block_on(ping_all(&relays));
        nearest(&relays, &pings).map(|i| Picked {
            host: setup::directory::helper_host(&relays[i]),
            port: relays[i].port,
            name: if relays[i].name.is_empty() { relays[i].id.clone() } else { relays[i].name.clone() },
            ms: pings[i].unwrap_or(0),
        })
    };
    match &picked {
        Some(p) => tracing::info!("relay: {} ({}:{}, {} ms) of {} offered by {server}", p.name, p.host, p.port, p.ms, relays.len()),
        None if !relays.is_empty() => tracing::warn!("relay: none of the {} relays {server} offers answered; its own helper", relays.len()),
        None => {}
    }
    kept().insert(server.to_string(), (Instant::now(), picked.clone()));
    picked
}

/// Each helper's round trip, probed as the game probes it (its address, its port), side by side.
async fn ping_all(relays: &[Helper]) -> Vec<Option<u32>> {
    let probes: Vec<_> = relays
        .iter()
        .map(|r| {
            let (ip, port) = (r.ip.to_string(), r.port);
            tokio::spawn(async move {
                let started = Instant::now();
                match tokio::time::timeout(PROBE_TIMEOUT, crate::network::probe_helper(&ip, port)).await {
                    Ok(Ok(_)) => Some(u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)),
                    _ => None,
                }
            })
        })
        .collect();
    let mut pings = Vec::with_capacity(probes.len());
    for probe in probes {
        pings.push(probe.await.ok().flatten());
    }
    pings
}

/// The relay to use: the lowest ping, the first in the server's order among those within
/// [`TIE`] of it; None when none answered.
fn nearest(relays: &[Helper], pings: &[Option<u32>]) -> Option<usize> {
    let best = pings.iter().flatten().min()?;
    (0..relays.len()).find(|&i| pings[i].is_some_and(|ms| ms <= best + TIE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(id: &str) -> Helper {
        Helper {
            id: id.into(),
            name: String::new(),
            region: String::new(),
            host: String::new(),
            role: "relay".into(),
            ip: "1.2.3.4".parse().unwrap(),
            port: 21128,
            ports: (40000, 40999),
        }
    }

    #[test]
    fn the_nearest_relay_wins_and_near_ties_keep_the_servers_order() {
        let relays = [relay("na1"), relay("syd"), relay("eu1")];
        assert_eq!(nearest(&relays, &[Some(140), Some(30), Some(300)]), Some(1));
        assert_eq!(nearest(&relays, &[Some(40), Some(30), None]), Some(0), "within 15 ms: the server's order");
        assert_eq!(nearest(&relays, &[None, None, Some(250)]), Some(2), "the only one that answered");
        assert_eq!(nearest(&relays, &[None, None, None]), None, "none: the server's own helper");
    }
}
