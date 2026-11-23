//! The relay the game uses (global matchmaking, G1 item 3). A network's matchmaking server
//! offers its relays in `/api/info` (one near each region's players); the launcher pings each
//! helper as the game will reach it and has the game use the nearest (`networking.nat_host`
//! in uplay.toml). A server that offers none (every server before the matchmaking one) keeps
//! its own helper, as before.
//!
//! The server's list and the pick are kept for [`KEPT_FOR`], so a start doesn't wait on them
//! twice. Each server's list is also saved ([`CACHE_FILE`]) and used when the server doesn't
//! answer: a player whose route to the matchmaking server fails is the one who signs in through
//! a relay (0.5.5's SR), and the relays are known only from that server.

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
/// Each server's last list of relays, in the launcher's data folder.
pub const CACHE_FILE: &str = "relays-cache.json";
/// How long a saved list is used when the server doesn't answer.
const SAVED_FOR: i64 = 30 * 24 * 3600;
/// The most servers whose lists are saved.
const MAX_SAVED: usize = 16;

/// One server's relays, as saved.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct Saved {
    server: String,
    /// Unix seconds.
    at: i64,
    relays: Vec<Helper>,
}

fn read_saved(dir: &std::path::Path) -> Vec<Saved> {
    std::fs::read(dir.join(CACHE_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Keeps `relays` as `server`'s last list.
fn save(dir: &std::path::Path, server: &str, relays: &[Helper], now: i64) {
    let mut all = read_saved(dir);
    all.retain(|s| s.server != server);
    all.insert(
        0,
        Saved {
            server: server.to_string(),
            at: now,
            relays: relays.to_vec(),
        },
    );
    all.truncate(MAX_SAVED);
    if let Ok(json) = serde_json::to_vec(&all) {
        let _ = std::fs::create_dir_all(dir);
        let _ = setup::write_atomic(&dir.join(CACHE_FILE), &json);
    }
}

/// `server`'s last list, when it's no older than [`SAVED_FOR`].
fn saved(dir: &std::path::Path, server: &str, now: i64) -> Option<Vec<Helper>> {
    read_saved(dir).into_iter().find(|s| s.server == server && now - s.at < SAVED_FOR).map(|s| s.relays)
}

/// The relay picked: where the game reaches its helper, and the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    /// The relay's id (the coordinator's for it too).
    pub id: String,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub ms: u32,
    /// Every relay's address range ("IP:first-last"), for the game to reach players at them.
    pub ranges: Vec<String>,
    /// Each relay that answered, by id, and its round trip ("id:ms"), for the game's sign-in.
    pub pings: Vec<String>,
}

type Kept = HashMap<String, (Instant, Option<Picked>)>;
static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(Mutex::default);

fn kept() -> std::sync::MutexGuard<'static, Kept> {
    KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The relay nearest the player among those `server` offers, or None to keep the server's
/// own helper (it offers none, or none answered).
pub fn pick_for(server: &str) -> Option<Picked> {
    crate::services::rt().block_on(pick(server.to_string()))
}

/// [`pick_for`], from async code: the server menu shows the picked relay's ping as the
/// server's, since the game's traffic goes there, not to the matchmaking server.
pub async fn pick(server: String) -> Option<Picked> {
    if let Some((at, picked)) = kept().get(&server) {
        if at.elapsed() < KEPT_FOR {
            return picked.clone();
        }
    }
    let asked = server.clone();
    let answer = tokio::task::spawn_blocking(move || setup::server_info::fetch(&asked, INFO_TIMEOUT).map(|i| i.relays))
        .await
        .ok()
        .flatten();
    let now = identity::now();
    let relays = match (answer, setup::app_data_dir()) {
        (Some(relays), Some(dir)) => {
            if !relays.is_empty() {
                save(&dir, &server, &relays, now);
            }
            relays
        }
        (Some(relays), None) => relays,
        // No answer: the relays it offered last (a player whose route to it fails).
        (None, Some(dir)) => {
            let last = saved(&dir, &server, now).unwrap_or_default();
            if !last.is_empty() {
                tracing::info!("relay: {server} didn't answer; its {} relays as it last offered them", last.len());
            }
            last
        }
        (None, None) => Vec::new(),
    };
    let picked = if relays.is_empty() {
        None
    } else {
        let pings = ping_all(&relays).await;
        // Always the nearest: players don't choose a relay (a far one would only make their own
        // connection worse).
        nearest(&relays, &pings).map(|i| Picked {
            id: relays[i].id.clone(),
            host: setup::directory::helper_host(&relays[i]),
            port: relays[i].port,
            name: if relays[i].name.is_empty() { relays[i].id.clone() } else { relays[i].name.clone() },
            ms: pings[i].unwrap_or(0),
            ranges: relays.iter().map(|r| format!("{}:{}-{}", r.ip, r.ports.0, r.ports.1)).collect(),
            pings: relays.iter().zip(&pings).filter_map(|(r, ms)| Some(format!("{}:{}", r.id, (*ms)?))).collect(),
        })
    };
    match &picked {
        Some(p) => tracing::info!("relay: {} ({}:{}, {} ms) of {} offered by {server}", p.name, p.host, p.port, p.ms, relays.len()),
        None if !relays.is_empty() => tracing::warn!("relay: none of the {} relays {server} offers answered; its own helper", relays.len()),
        None => {}
    }
    kept().insert(server, (Instant::now(), picked.clone()));
    picked
}

/// A relay `server` offers, for Settings: its id, name and region, and its ping now.
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

    /// A server's relays are kept for when it doesn't answer, the newest list, for a while.
    #[test]
    fn a_servers_relays_are_kept_for_when_it_doesnt_answer() {
        let dir = std::env::temp_dir().join(format!("fe-relays-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved(&dir, "mm.example", 0), None);
        save(&dir, "mm.example", &[relay("syd")], 1000);
        save(&dir, "other.example", &[relay("fra")], 1000);
        save(&dir, "mm.example", &[relay("syd"), relay("fra")], 2000);
        let kept = saved(&dir, "mm.example", 2000 + 3600).unwrap();
        assert_eq!(kept.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["syd", "fra"], "the newest list");
        assert_eq!(saved(&dir, "mm.example", 2000 + SAVED_FOR), None, "too old");
        assert_eq!(saved(&dir, "other.example", 1000).unwrap()[0].id, "fra");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
