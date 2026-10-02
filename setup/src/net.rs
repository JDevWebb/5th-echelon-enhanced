//! Networking: which adapter reaches the server, and whether the server
//! answers.
//!
//! Matches run peer to peer over the address the game picks from one of the
//! PC's adapters. With a VPN, a LAN and maybe a second VPN all up, it often
//! picks the wrong one, and nobody can join. The fix is to pin the adapter
//! the server is reached through, which is exactly the one the operating
//! system routes the server's traffic over.

use std::net::IpAddr;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::net::ToSocketAddrs;
use std::net::UdpSocket;
use std::time::Duration;

pub use crate::sys::adapters;

/// Whether `host` is a server address: an IP address, or a host name of
/// letters, digits, `-` and `.` (no port, path or spaces). Anything else
/// could smuggle text into the requests made to it.
pub fn valid_host(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    let label = |l: &str| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    !host.is_empty() && host.len() <= 253 && host.split('.').all(label)
}

/// Whether `ip` is on the public internet: not loopback, private, shared
/// (100.64/10), link-local, multicast, broadcast or unspecified.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_multicast() || v4.is_broadcast() || v4.is_unspecified() || a == 0 || (a == 100 && (64..128).contains(&b)) || a >= 240)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            !(v6.is_loopback() || v6.is_multicast() || v6.is_unspecified() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || v6.to_ipv4_mapped().is_some_and(|v4| !is_public(IpAddr::V4(v4))))
        }
    }
}

/// Whether a server is on this PC or its own network (`host` as typed, `ip`
/// what it resolved to): such a server is played over plain HTTP without a
/// warning, as nobody on the internet is on the way.
pub fn is_local_server(host: &str, ip: IpAddr) -> bool {
    let host = host.trim();
    host == "localhost" || host.ends_with(".localhost") || !is_public(ip)
}

/// Resolves a server address typed by a player (an IP or a host name).
pub fn resolve(server: &str) -> Option<IpAddr> {
    let server = server.trim();
    if !valid_host(server) {
        return None;
    }
    if let Ok(ip) = server.parse() {
        return Some(ip);
    }
    (server, 0).to_socket_addrs().ok()?.map(|a| a.ip()).find(IpAddr::is_ipv4)
}

/// This PC's address on the route to `server`: what the operating system
/// would send the server's traffic from. Sends nothing.
pub fn local_ip_towards(server: IpAddr) -> Option<IpAddr> {
    let socket = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), 0)).ok()?;
    socket.connect((server, crate::QUAZAL_PORT)).ok()?;
    socket.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_unspecified())
}

/// The adapter to pin for `server`: the one holding the address the server
/// is routed from. None when no pin is needed (the server is on this PC) or
/// the route's address belongs to no listed adapter.
pub fn adapter_for_server(server: IpAddr, adapters: &[(String, IpAddr)]) -> Option<String> {
    if server.is_loopback() {
        return None;
    }
    let local = local_ip_towards(server)?;
    if local.is_loopback() {
        return None;
    }
    adapter_with_ip(local, adapters)
}

/// The adapter holding `ip`.
pub fn adapter_with_ip(ip: IpAddr, adapters: &[(String, IpAddr)]) -> Option<String> {
    adapters.iter().find(|(_, a)| *a == ip).map(|(name, _)| name.clone())
}

/// The current address of the adapter called `name` (as the hook matches it).
pub fn adapter_ip(name: &str, adapters: &[(String, IpAddr)]) -> Option<IpAddr> {
    adapters.iter().find(|(n, _)| hooks_config::adapter_name_matches(n, name)).map(|(_, ip)| *ip)
}

/// Whether a TCP port answers within `timeout`.
pub fn port_open(ip: IpAddr, port: u16, timeout: Duration) -> bool {
    TcpStream::connect_timeout(&SocketAddr::new(ip, port), timeout).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_adapter_the_server_is_routed_through() {
        let adapters = vec![("Loopback".to_string(), IpAddr::from([127, 0, 0, 1])), ("Game VPN".to_string(), IpAddr::from([10, 8, 1, 2]))];
        assert_eq!(adapter_with_ip(IpAddr::from([10, 8, 1, 2]), &adapters).as_deref(), Some("Game VPN"));
        assert_eq!(adapter_ip("game vpn", &adapters), Some(IpAddr::from([10, 8, 1, 2])));
        assert_eq!(adapter_for_server(IpAddr::from([127, 0, 0, 1]), &adapters), None, "no pin for a server on this PC");
        assert_eq!(local_ip_towards(IpAddr::from([127, 0, 0, 1])), Some(IpAddr::from([127, 0, 0, 1])));
    }

    #[test]
    fn checks_hosts() {
        for ok in ["10.8.0.10", "play.example.org", "localhost", "a-b.c1", "::1"] {
            assert!(valid_host(ok), "{ok}");
        }
        for bad in ["", "play.example.org:80", "a b", "host\r\nX: y", "-a.b", "a..b", "a/b", "é.com"] {
            assert!(!valid_host(bad), "{bad:?}");
        }
        assert!(is_public(IpAddr::from([203, 0, 114, 5])));
        for private in [[10, 0, 0, 1], [192, 168, 1, 1], [127, 0, 0, 1], [100, 64, 0, 1], [169, 254, 1, 1], [0, 0, 0, 0], [255, 255, 255, 255]] {
            assert!(!is_public(IpAddr::from(private)), "{private:?}");
        }
        assert!(is_local_server("10.8.0.10", IpAddr::from([10, 8, 0, 10])));
        assert!(is_local_server("game.localhost", IpAddr::from([203, 0, 114, 5])));
        assert!(is_local_server("box.lan", IpAddr::from([192, 168, 1, 4])), "a name for a LAN address");
        assert!(!is_local_server("play.example.org", IpAddr::from([203, 0, 114, 5])));
    }

    #[test]
    fn resolves_addresses() {
        assert_eq!(resolve(" 10.8.0.10 "), Some(IpAddr::from([10, 8, 0, 10])));
        assert_eq!(resolve("localhost").map(|ip| ip.is_loopback()), Some(true));
    }

    #[test]
    fn open_and_closed_ports() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_open(IpAddr::from([127, 0, 0, 1]), port, Duration::from_secs(1)));
        drop(listener);
        assert!(!port_open(IpAddr::from([127, 0, 0, 1]), port, Duration::from_millis(300)));
    }
}
