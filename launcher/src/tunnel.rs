//! Whether the game goes through the server's tunnel over HTTPS this start (0.5.2's T3): when
//! this PC's UDP to the server gets nowhere but the server answers over HTTPS and offers one, or
//! when the player chose Always go through HTTPS. Checked again at every start, so a player who
//! moves to Wi-Fi plays as usual.

use std::time::Duration;

/// How long the quick UDP check waits for the NAT helper, and the server's info over HTTPS
/// (Play waits on them).
const UDP_WAIT: Duration = Duration::from_millis(1500);
const INFO_WAIT: Duration = Duration::from_secs(3);

/// What the player is told when the tunnel is used.
pub const NOTICE: &str = "Your connection blocks the game's traffic (UDP), so the game goes through HTTPS instead. It will be slower than usual.";
/// Or, when the player chose it (Settings › Network › Always go through HTTPS).
pub const NOTICE_CHOSEN: &str = "The game goes through HTTPS, as set in Settings › Network. It will be slower than usual.";

/// The tunnel for this start, if the game should use one: `server` (the profile's host), its
/// info's ports, and whether UDP to its helper got an answer (`udp_ok`).
pub fn decide(server: &str, ports: Option<&setup::server_info::Ports>, udp_ok: bool, always: bool) -> Option<hooks_config::Tunnel> {
    let ports = ports.filter(|p| p.tunnel)?;
    if udp_ok && !always {
        return None;
    }
    Some(hooks_config::Tunnel {
        host: server.to_string(),
        login: ports.login,
        secure: ports.secure.unwrap_or(ports.login.wrapping_add(1)),
        helper: ports.nat?,
        via: None,
    })
}

/// The check at start: the server's info over HTTPS, and a quick UDP probe to its helper (only
/// when the player hasn't chosen the tunnel anyway, and the server offers one).
pub fn for_start(server: &str) -> Option<hooks_config::Tunnel> {
    let always = crate::prefs::Prefs::tunnel_always();
    let info = crate::services::rt()
        .block_on(async { tokio::time::timeout(INFO_WAIT, crate::network::server_info_tls(server, 443)).await })
        .ok()
        .flatten()?;
    let ports = info.ports?;
    if !ports.tunnel {
        return None;
    }
    let udp_ok = always
        || ports.nat.is_some_and(|port| {
            crate::services::rt()
                .block_on(async { tokio::time::timeout(UDP_WAIT, crate::network::test_nat_helper(server, port)).await })
                .is_ok_and(|r| r.is_ok())
        });
    decide(server, Some(&ports), udp_ok && !always, always)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ports(tunnel: bool) -> setup::server_info::Ports {
        setup::server_info::Ports {
            api: 80,
            login: 21126,
            nat: Some(21128),
            api_tls: Some(443),
            secure: Some(21127),
            tunnel,
        }
    }

    #[test]
    fn the_tunnel_only_when_udp_is_blocked_or_chosen_and_offered() {
        assert_eq!(decide("scbl.example.net", Some(&ports(true)), true, false), None, "UDP works");
        let t = decide("scbl.example.net", Some(&ports(true)), false, false).unwrap();
        assert_eq!((t.host.as_str(), t.login, t.secure, t.helper), ("scbl.example.net", 21126, 21127, 21128));
        assert!(decide("scbl.example.net", Some(&ports(true)), true, true).is_some(), "chosen");
        assert_eq!(decide("scbl.example.net", Some(&ports(false)), false, true), None, "not offered");
        assert_eq!(decide("scbl.example.net", None, false, true), None, "no info");
    }
}
