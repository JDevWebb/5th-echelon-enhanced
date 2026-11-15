//! Automatic sets itself (0.5.2's J4): on Internet play's Automatic, the launcher knows for each
//! network whether other players can reach this PC directly, and the game starts on the right
//! path (the relay at once, when they can't) instead of finding out in its first seconds.
//!
//! What's known comes from the launcher's own quiet check (at start, when a network is new or
//! its result is over a week old, or from Settings › Network), and from the game: the add-on
//! writes what it found on this network ([`hooks_config::ReachFound`]), which beats the
//! launcher's guess (Windows Firewall can let the launcher in and keep the game out). Kept in the
//! launcher's settings, by the gateway's hardware address; never sent anywhere. It never
//! overrides a choice the player made (Always through the server, LAN or VPN only).

use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

/// How long a network's result is trusted before it's checked again.
pub const FRESH: i64 = 7 * 24 * 3600;
/// The most networks kept.
const MAX_KNOWN: usize = 16;

/// What's known about reaching this PC on one network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Known {
    /// [`portmap::network_id`].
    pub network: String,
    /// Other players reach it through the relay only.
    pub relayed: bool,
    /// Why, in a few words.
    pub why: String,
    /// Unix seconds.
    pub at: i64,
    /// Who found it: "launcher" or "game".
    pub by: String,
}

/// Adds or replaces `k` in `list` (by network), the oldest going past [`MAX_KNOWN`].
pub fn remember(list: &mut Vec<Known>, k: Known) {
    list.retain(|x| x.network != k.network);
    list.push(k);
    list.sort_by_key(|x| x.at);
    while list.len() > MAX_KNOWN {
        list.remove(0);
    }
}

/// What's known about `network`, while it's fresh.
pub fn find<'a>(list: &'a [Known], network: &str, now: i64) -> Option<&'a Known> {
    list.iter().find(|k| k.network == network && now - k.at < FRESH)
}

/// The network this PC is on now.
pub fn here() -> Option<String> {
    portmap::network_id()
}

/// What's known about the network this PC is on now (the game's own finding taken first).
pub fn known_here(game_dir: &std::path::Path) -> Option<Known> {
    let network = here()?;
    take_game_finding(game_dir, &network);
    find(&crate::prefs::Prefs::known_networks(), &network, identity::now()).cloned()
}

/// The reason the game should ask for the relay at once, on Automatic: this network is known
/// not to be reachable directly.
pub fn start_relayed(game_dir: &std::path::Path) -> Option<String> {
    known_here(game_dir).filter(|k| k.relayed).map(|k| k.why)
}

/// The game's finding (the add-on's [`hooks_config::REACH_FILE`]) for `network`, when it's newer
/// than what's known: the game ran on the network the PC is on now (it rarely changes between
/// the game closing and the launcher reading this).
fn take_game_finding(game_dir: &std::path::Path, network: &str) {
    let Ok(text) = std::fs::read_to_string(game_dir.join(hooks_config::REACH_FILE)) else {
        return;
    };
    let Ok(found) = serde_json::from_str::<hooks_config::ReachFound>(&text) else { return };
    let list = crate::prefs::Prefs::known_networks();
    if list.iter().any(|k| k.network == network && k.at >= found.at) {
        return;
    }
    tracing::info!("This network, as the game found it: {}", found.why);
    crate::prefs::Prefs::remember_network(Known {
        network: network.into(),
        relayed: found.relayed,
        why: found.why,
        at: found.at,
        by: "game".into(),
    });
}

/// Whether the network this PC is on needs checking: new, or its result old.
pub fn due() -> bool {
    match here() {
        Some(network) => find(&crate::prefs::Prefs::known_networks(), &network, identity::now()).is_none(),
        None => false,
    }
}

/// Checks the network this PC is on now and remembers the result, quietly (at start, or from
/// Settings › Network). None when it can't be checked: no network to name, the game running (it
/// holds the port the check needs), or the server not answering.
pub fn check(profile: &setup::config::Profile, game_dir: &std::path::Path) -> Option<Known> {
    let network = here()?;
    if setup::game::game_running() {
        return None;
    }
    let (relayed, why) = verdict(profile, game_dir)?;
    let k = Known {
        network,
        relayed,
        why: why.into(),
        at: identity::now(),
        by: "launcher".into(),
    };
    tracing::info!("This network, as the launcher checked it: {}", k.why);
    crate::prefs::Prefs::remember_network(k.clone());
    Some(k)
}

/// The check itself: the game's own firewall rules, the router's kind of NAT, and whether the
/// server's packet gets in (on its own, then through the router's forward, as the game asks
/// for one).
fn verdict(profile: &setup::config::Profile, game_dir: &std::path::Path) -> Option<(bool, &'static str)> {
    use crate::network;
    let rt = crate::services::rt();
    let t = Duration::from_secs(8);
    // A rule keeping the game out (Windows writes one when its prompt is dismissed): the
    // launcher's own packet would get in, the game's wouldn't.
    if cfg!(target_os = "windows") && !hooks_config::running_under_wine() {
        let (_, says) = crate::settings::firewall_now(game_dir);
        if says.is_some_and(|s| s.starts_with("a rule blocks")) {
            return Some((true, "Windows Firewall keeps the game out"));
        }
    }
    let (nat_host, nat_port) = match crate::relays::pick_for(&profile.server) {
        Some(r) => (r.host, r.port),
        None => (profile.server.clone(), profile.nat_port.unwrap_or(nat_proto::DEFAULT_PORT)),
    };
    let nat = rt
        .block_on(async { tokio::time::timeout(t, network::test_nat_helper(&nat_host, nat_port)).await })
        .ok()?
        .ok()?;
    if nat.symmetric == Some(true) {
        return Some((true, "the router gives each destination its own port (strict NAT)"));
    }
    let api = profile.api_server_url().to_string();
    let secret = profile.user.secret().unwrap_or_default();
    let user = profile.user.username.clone();
    let reached = || {
        rt.block_on(async { tokio::time::timeout(t, network::test_p2p(api.clone(), &user, &secret)).await })
            .is_ok_and(|r| r.is_ok())
    };
    if reached() {
        return Some((false, "reached directly"));
    }
    // As the game does: the router asked to forward its port while it runs.
    let pinned = profile
        .adapter
        .as_deref()
        .and_then(|name| setup::net::adapter_ip(name, &setup::net::adapters()))
        .and_then(|ip| match ip {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(_) => None,
        });
    match portmap::map(nat_proto::STORM_PORT, pinned, 120, "5th Echelon network check") {
        Ok(_mapping) if reached() => Some((false, "reached through the router's forward")),
        Ok(_) => Some((true, "the router's forward lets nothing in (a firewall, or a second router)")),
        Err(_) => Some((true, "the router doesn't forward the game's port (UPnP and NAT-PMP off, or none)")),
    }
}

/// How the network is described in Settings › Network.
pub fn describe(k: &Known, clock: &setup::clock::Clock) -> String {
    let when = clock.time(k.at, identity::now());
    let by = if k.by == "game" { "the game found" } else { "checked" };
    if k.relayed {
        format!(
            "On this network, other players reach you through the relay: {} ({by} {when}). Your games ask for it at once.",
            k.why
        )
    } else {
        format!("On this network, other players reach you directly: {} ({by} {when}).", k.why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(network: &str, relayed: bool, at: i64) -> Known {
        Known {
            network: network.into(),
            relayed,
            why: "why".into(),
            at,
            by: "launcher".into(),
        }
    }

    #[test]
    fn each_network_is_remembered_for_a_week() {
        let now = 1_800_000_000;
        let mut list = Vec::new();
        remember(&mut list, known("mac aa", false, now - 100));
        remember(&mut list, known("mac bb", true, now - 50));
        assert!(find(&list, "mac bb", now).is_some_and(|k| k.relayed));
        remember(&mut list, known("mac bb", false, now));
        assert_eq!(list.len(), 2, "replaced, not added");
        assert!(find(&list, "mac bb", now).is_some_and(|k| !k.relayed));
        assert!(find(&list, "mac aa", now + FRESH).is_none(), "over a week: checked again");
        for i in 0..40 {
            remember(&mut list, known(&format!("mac {i}"), false, now + i));
        }
        assert_eq!(list.len(), MAX_KNOWN);
        assert!(list.iter().any(|k| k.network == "mac 39") && !list.iter().any(|k| k.network == "mac aa"), "the oldest go");
    }
}
