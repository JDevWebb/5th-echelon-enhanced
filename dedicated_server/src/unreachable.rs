//! The players a game couldn't reach (0.5.5's JC4).
//!
//! A session is a full mesh: every member connects to a newcomer, so a join fails when any one
//! member can't reach the joiner, not only its host. The game reports the failure against the
//! host's room (`0xb08a1a05`, Storm's `CONNECTION_FAILED`), which is all the server heard before.
//! The add-on now hooks Storm's "Unreachable" report (5ee-research; hooks `unreachable.rs`) and
//! names the peer: its addresses as the game holds them. The server finds who advertises that
//! address (at its own helper or another relay), notes it (the `unreachable` session event), and
//! moves a direct player two others' games couldn't reach to the relay (J2,
//! [`crate::join_fallback::Joins::unreachable`]).

use std::net::Ipv4Addr;
use std::net::SocketAddrV4;
use std::time::Instant;

use slog::info;
use slog::Logger;

use crate::storage::Storage;

/// The most peers taken from one report.
pub const MAX_PEERS: usize = 8;

/// A peer as the game reported it: address A (public), address B (local), and its connection id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub ip: u32,
    pub port: u32,
    pub local_ip: u32,
    pub local_port: u32,
    pub rvcid: u32,
}

/// The addresses `ip`/`port` may stand for: the game's memory holds them in an order not yet
/// read for certain, so both byte orders of each.
pub fn candidates(ip: u32, port: u32) -> Vec<SocketAddrV4> {
    let Ok(port) = u16::try_from(port) else { return vec![] };
    let mut out = Vec::with_capacity(4);
    for ip in [ip, ip.swap_bytes()] {
        for port in [port, port.swap_bytes()] {
            let a = SocketAddrV4::new(Ipv4Addr::from(ip), port);
            if port != 0 && !a.ip().is_unspecified() && !out.contains(&a) {
                out.push(a);
            }
        }
    }
    out
}

/// `reporter`'s game couldn't reach `peers`: each one found is noted, and moved to the relay
/// once two players' games couldn't reach it.
pub fn reported(storage: &Storage, logger: &Logger, reporter: u32, peers: &[Peer]) {
    let now = Instant::now();
    for peer in peers.iter().take(MAX_PEERS) {
        // A housemate's LAN address this server gave the reporter's game: their LAN doesn't
        // carry the game, so they meet at the relay from now on.
        let lan = candidates(peer.ip, peer.port)
            .into_iter()
            .chain(candidates(peer.local_ip, peer.local_port))
            .filter(|a| nat_proto::is_private(*a.ip()))
            .find_map(|a| crate::game_session::house_lan_unreachable(reporter, a).map(|user| (a, user)));
        if let Some((address, user)) = lan {
            let name = storage.find_username_by_user_id(user).ok().flatten().unwrap_or_default();
            info!(logger, "{reporter}'s game couldn't reach its housemate {name} on their LAN ({address}); they meet at the relay from now on");
            crate::session_events::note(
                crate::session_events::Who::Id(reporter),
                "unreachable",
                serde_json::json!({ "found": true, "peer": name, "peer_id": user, "at": address.to_string(), "house": true }),
            );
            continue;
        }
        let found = candidates(peer.ip, peer.port)
            .into_iter()
            .find_map(|a| crate::nat_helper::who_advertises(a).map(|user| (a, user)));
        let Some((address, user)) = found else {
            crate::session_events::note(
                crate::session_events::Who::Id(reporter),
                "unreachable",
                serde_json::json!({ "found": false, "ip": format!("{:#010x}", peer.ip), "port": peer.port, "rvcid": peer.rvcid }),
            );
            continue;
        };
        if user == reporter {
            continue;
        }
        // Only someone they play with or were joining: a made-up report mustn't move strangers.
        let together = storage.share_session(reporter, user).unwrap_or(false) || storage.hosts_public_session(user).unwrap_or(false);
        let name = storage.find_username_by_user_id(user).ok().flatten().unwrap_or_default();
        let relayed = crate::nat_helper::registration(&name).flatten();
        crate::session_events::note(
            crate::session_events::Who::Id(reporter),
            "unreachable",
            serde_json::json!({ "found": true, "peer": name, "peer_id": user, "at": address.to_string(), "relayed": relayed, "together": together }),
        );
        if !together {
            continue;
        }
        info!(logger, "{reporter}'s game couldn't reach {name} ({address})");
        let moves = crate::join_fallback::joins().unreachable(reporter, user, now);
        for moved in moves {
            crate::game_session::move_to_relay(storage, logger, moved, "unreachable_peer");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_tried_in_both_byte_orders() {
        // 203.0.113.4:13000, as a big-endian number and as the bytes read the other way.
        let ip = u32::from(Ipv4Addr::new(203, 0, 113, 4));
        let all = candidates(ip, 13000);
        assert_eq!(all.len(), 4);
        assert!(all.contains(&"203.0.113.4:13000".parse().unwrap()));
        assert!(all.contains(&SocketAddrV4::new(Ipv4Addr::new(4, 113, 0, 203), 13000)));
        assert!(all.contains(&SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 4), 13000u16.swap_bytes())));
        assert!(candidates(0, 13000).is_empty(), "no address");
        assert!(candidates(ip, 0).is_empty(), "no port");
        assert!(candidates(ip, 70000).is_empty(), "not a port");
    }
}
