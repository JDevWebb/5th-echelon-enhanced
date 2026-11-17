//! Signing in through the nearest relay (0.5.5's SR). Since the community network became one
//! matchmaking server (0.5.0), some players' routes to it lose the replies: the game's calls time
//! out and it can't go online, where the old regional server near them worked (Ghost_Leader on
//! eu1 through 0.4.7, on ca1 in 0.5.3). On those networks the game's sign-in, its secure service
//! and the API go through the nearest relay, which passes them on to the matchmaking server.
//!
//! The add-on notices (`hooks_config::VIA_RELAY_FILE`, written when its calls keep losing their
//! answers); from the next start this network signs in through the relay for [`FRESH`]. Or the
//! player chooses it for every network (Settings › Network). Only with a relay that says it can
//! (`relay-sign-in` in its `/api/info`).

/// How long a network keeps signing in through the relay after the game found it needed to.
pub const FRESH: i64 = 7 * 24 * 3600;
/// The most networks kept.
const MAX_KEPT: usize = 16;

/// What the player is told when the game signs in through the relay.
pub const NOTICE: &str = "Your connection to the server kept losing its answers, so the game signs in through the relay near you instead.";
pub const NOTICE_CHOSEN: &str = "The game signs in through the relay near you, as set in Settings › Network.";
pub const NOTICE_UNREACHABLE: &str = "The server doesn't answer from here, but the relay near you does, so the game signs in through it.";

/// Servers (host names) that didn't answer the launcher's ping while their relay did: until one
/// answers again, this run of the launcher signs in to it through the relay. Not kept: a
/// server that's down for everyone shouldn't send this PC through the relay for a week. Without
/// it, a player whose route to the matchmaking server fails altogether was stopped at "Server not
/// answering" and the game never ran to find it needed the relay.
static UNREACHABLE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Whether `server` answered the launcher's ping just now (its relay answering either way).
pub fn server_answered(server: &str, answered: bool) {
    let server = server.to_ascii_lowercase();
    let mut list = UNREACHABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    list.retain(|s| *s != server);
    if !answered {
        list.push(server);
    }
}

/// Whether `server` didn't answer from here while its relay did.
pub fn unreachable(server: &str) -> bool {
    let server = server.to_ascii_lowercase();
    UNREACHABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().any(|s| *s == server)
}

/// Adds or refreshes `network` in `list`, the oldest going past [`MAX_KEPT`].
pub fn remember(list: &mut Vec<(String, i64)>, network: &str, at: i64) {
    list.retain(|(n, _)| n != network);
    list.push((network.to_string(), at));
    list.sort_by_key(|(_, at)| *at);
    while list.len() > MAX_KEPT {
        list.remove(0);
    }
}

/// Whether `network` signs in through the relay at `now`.
pub fn found(list: &[(String, i64)], network: &str, now: i64) -> bool {
    list.iter().any(|(n, at)| n == network && now - at < FRESH)
}

/// Why the game should sign in through the relay this start, if it should: the player chose it,
/// the game found it needed to on the network this PC is on (its finding taken first), or
/// `server` doesn't answer from here while its relay does.
pub fn wanted(game_dir: &std::path::Path, server: &str) -> Option<&'static str> {
    if crate::prefs::Prefs::sign_in_via_relay() {
        return Some(NOTICE_CHOSEN);
    }
    let network = crate::reach::here();
    if let Some(network) = &network {
        take_game_finding(game_dir, network);
        if found(&crate::prefs::Prefs::via_relay_networks(), network, identity::now()) {
            return Some(NOTICE);
        }
    }
    unreachable(server).then_some(NOTICE_UNREACHABLE)
}

/// Whether this network signs in through the relay, from what's kept (no file read): the player
/// chose it, or the game found it needed to here.
pub fn kept_or_chosen() -> bool {
    crate::prefs::Prefs::sign_in_via_relay() || crate::reach::here().is_some_and(|network| found(&crate::prefs::Prefs::via_relay_networks(), &network, identity::now()))
}

/// Takes the add-on's finding for the network this PC is on now: the launcher's own calls (its
/// account check, friends) go through the relay from its first screen, not only once the game
/// starts. A player whose route to the matchmaking server loses its answers would otherwise be
/// stopped at "Server not answering" before the relay could help.
pub fn take_finding(game_dir: &std::path::Path) {
    if let Some(network) = crate::reach::here() {
        take_game_finding(game_dir, &network);
    }
}

/// The add-on's finding for `network` (the PC rarely changes networks between the game closing
/// and the launcher reading this), when it's newer than what's kept.
fn take_game_finding(game_dir: &std::path::Path, network: &str) {
    let Ok(text) = std::fs::read_to_string(game_dir.join(hooks_config::VIA_RELAY_FILE)) else {
        return;
    };
    let Ok(f) = serde_json::from_str::<hooks_config::ViaRelayFound>(&text) else { return };
    if crate::prefs::Prefs::via_relay_networks().iter().any(|(n, at)| n == network && *at >= f.at) {
        return;
    }
    tracing::info!("This network, as the game found it: {}; signing in through the nearest relay", f.why);
    crate::prefs::Prefs::remember_via_relay(network, f.at);
}

/// Whether the relay at `host` passes sign-in on (`relay-sign-in` in its `/api/info`).
pub fn relay_can(host: &str) -> bool {
    setup::server_info::fetch(host, std::time::Duration::from_secs(3)).is_some_and(|i| i.features.iter().any(|f| f == "relay-sign-in"))
}

/// The tunnel through relay `relay_host` (its helper on `relay_port`) to `server`'s sign-in and
/// secure service on `login` and `secure`.
pub fn tunnel(server: &str, login: u16, secure: u16, relay_host: &str, relay_port: u16) -> hooks_config::Tunnel {
    hooks_config::Tunnel {
        host: server.to_string(),
        login,
        secure,
        helper: relay_port,
        via: Some(relay_host.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_that_doesnt_answer_goes_through_the_relay_until_it_does() {
        let server = "mm-unreachable.example";
        assert!(!unreachable(server));
        server_answered(server, false);
        assert!(unreachable("MM-Unreachable.example"), "by its host name, any case");
        assert!(!unreachable("other.example"), "only that server");
        server_answered(server, true);
        assert!(!unreachable(server), "it answers again");
    }

    #[test]
    fn a_network_signs_in_through_the_relay_for_a_week() {
        let mut list = Vec::new();
        remember(&mut list, "gw-a", 1000);
        assert!(found(&list, "gw-a", 1000 + FRESH - 1));
        assert!(!found(&list, "gw-a", 1000 + FRESH), "a week on, it's tried directly again");
        assert!(!found(&list, "gw-b", 1000), "only that network");
        remember(&mut list, "gw-a", 5000);
        assert_eq!(list, vec![("gw-a".to_string(), 5000)], "refreshed, not doubled");
        for i in 0..20 {
            remember(&mut list, &format!("gw-{i}"), 6000 + i);
        }
        assert_eq!(list.len(), MAX_KEPT);
    }

    #[test]
    fn the_tunnel_ends_at_the_relay_and_signs_in_to_the_server() {
        let t = tunnel("scbl.example.net", 21126, 21127, "de1.example.net", 21128);
        assert_eq!(
            (t.host.as_str(), t.login, t.secure, t.helper, t.via.as_deref()),
            ("scbl.example.net", 21126, 21127, 21128, Some("de1.example.net"))
        );
    }
}
