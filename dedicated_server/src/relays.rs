//! The relays this server works with (global matchmaking, G1 item 4): its own NAT helper's,
//! and, as the network's matchmaking server (`[matchmaking] network_relays`), every relay the
//! coordinator lists (its heartbeat answer's `relays`). An address on a relay is a relayed
//! player's, not someone else's network: RegisterURLs and the probe leave it alone rather than
//! rewriting it to the address the player was seen from. `/api/info` offers the relays to
//! launchers, which pick the nearest.

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use serde::Serialize;

/// The most relays taken from the coordinator.
const MAX_RELAYS: usize = 64;

/// A relay on the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relay {
    pub id: String,
    pub name: String,
    pub region: String,
    /// Its host name, when it has one (what launchers tell the game).
    pub host: String,
    pub ip: Ipv4Addr,
    pub port: u16,
    pub ports: (u16, u16),
}

static NETWORK_RELAYS: AtomicBool = AtomicBool::new(false);
static NETWORK: Mutex<Vec<Relay>> = Mutex::new(Vec::new());

/// Whether this server works with the network's relays (`[matchmaking] network_relays`).
/// Called once at start.
pub fn init(network_relays: bool) {
    NETWORK_RELAYS.store(network_relays, Ordering::Relaxed);
}

fn network() -> std::sync::MutexGuard<'static, Vec<Relay>> {
    NETWORK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The network's relays from the coordinator's heartbeat answer (coordinators from 0.4.8 send
/// them); an answer without them leaves the last list. Only public addresses with a sane range.
pub fn from_heartbeat(answer: &serde_json::Value) {
    let Some(list) = answer["relays"].as_array() else { return };
    let text = |v: &serde_json::Value, key: &str| v[key].as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(64).collect::<String>();
    let relays: Vec<Relay> = list
        .iter()
        .filter_map(|v| {
            let ip: Ipv4Addr = v["ip"].as_str()?.parse().ok()?;
            let port = u16::try_from(v["port"].as_u64()?).ok().filter(|p| *p > 0)?;
            let ports = v["ports"].as_array()?;
            let ports = (u16::try_from(ports.first()?.as_u64()?).ok()?, u16::try_from(ports.get(1)?.as_u64()?).ok()?);
            (crate::game_session::is_public(std::net::IpAddr::V4(ip)) && ports.0 > 0 && ports.0 <= ports.1).then(|| Relay {
                id: text(v, "id"),
                name: text(v, "name"),
                region: text(v, "region"),
                host: text(v, "host"),
                ip,
                port,
                ports,
            })
        })
        .take(MAX_RELAYS)
        .collect();
    *network() = relays;
}

/// The relays this server works with, as address and port range: its own helper's, and the
/// network's when it's the matchmaking server. Only an address in one of these ranges is a
/// relayed player's: another port on a relay's address is no player's.
pub fn ranges() -> Vec<(Ipv4Addr, (u16, u16))> {
    let mut ranges: Vec<(Ipv4Addr, (u16, u16))> = crate::nat_helper::relay_range().into_iter().collect();
    if NETWORK_RELAYS.load(Ordering::Relaxed) {
        ranges.extend(network().iter().map(|r| (r.ip, r.ports)));
    }
    ranges
}

/// The relays to offer launchers (`/api/info`): the network's, when this is the matchmaking
/// server; none otherwise (players then use this server's own helper).
pub fn offered() -> Vec<Relay> {
    if NETWORK_RELAYS.load(Ordering::Relaxed) {
        network().clone()
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_networks_relays_come_from_the_heartbeat() {
        from_heartbeat(&serde_json::json!({ "relays": [
            { "id": "syd", "name": "Sydney", "region": "Oceania", "host": "syd.example.net", "role": "relay", "ip": "5.6.7.8", "port": 21128, "ports": [40000, 40999] },
            { "id": "lan", "ip": "192.168.1.20", "port": 21128, "ports": [40000, 40999] },
            { "id": "upside-down", "ip": "9.9.9.9", "port": 21128, "ports": [40999, 40000] },
        ] }));
        let kept: Vec<String> = network().iter().map(|r| r.id.clone()).collect();
        assert_eq!(kept, ["syd"]);
        // An answer without relays keeps the last list.
        from_heartbeat(&serde_json::json!({}));
        assert_eq!(network().len(), 1);
        // Offered, and their addresses worked with, only on the matchmaking server.
        init(false);
        let syd = (Ipv4Addr::new(5, 6, 7, 8), (40000, 40999));
        assert!(offered().is_empty() && !ranges().contains(&syd));
        init(true);
        assert_eq!(offered().len(), 1);
        assert!(ranges().contains(&syd));
        init(false);
    }
}
