//! Asks the router to forward a UDP port to this PC: UPnP first, then
//! NAT-PMP. The game client keeps Storm's port (13000) mapped while the game
//! runs, so other players can reach it directly; the launcher's connection
//! test maps it briefly to check the router allows it.
//!
//! Only the router this PC routes through is asked, and an answer from any
//! other host is ignored, so another machine on the network can't pose as
//! the router. A mapping whose "external" address isn't a public one (a
//! router behind another NAT) is refused: it would be no use to players
//! outside.

use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::net::UdpSocket;
use std::time::Duration;

use igd_next::PortMappingProtocol;
use igd_next::SearchOptions;

const PMP_PORT: u16 = 5351;

/// A port the router forwards to this PC. Dropping it leaves the mapping in
/// place until its lease ends; [`Mapping::remove`] takes it away now.
pub struct Mapping {
    /// The address and port players outside reach this PC on.
    pub public: SocketAddrV4,
    /// "UPnP" or "NAT-PMP".
    pub how: &'static str,
    handle: Handle,
}

enum Handle {
    Upnp { gateway: igd_next::Gateway, port: u16 },
    Pmp { gateway: Ipv4Addr, local: Ipv4Addr, port: u16 },
}

impl Mapping {
    /// Asks the router to stop forwarding the port.
    pub fn remove(self) {
        match self.handle {
            Handle::Upnp { gateway, port } => {
                let _ = gateway.remove_port(PortMappingProtocol::UDP, port);
            }
            Handle::Pmp { gateway, local, port } => {
                let _ = pmp_map(gateway, local, port, 0);
            }
        }
    }
}

/// Asks the router to forward UDP `port` to this PC (at `pinned`, or the
/// address this PC reaches the router from) for `lease` seconds. On failure,
/// says why for each way tried.
pub fn map(port: u16, pinned: Option<Ipv4Addr>, lease: u32, description: &str) -> Result<Mapping, String> {
    let upnp_err = match upnp(port, pinned, lease, description) {
        Ok(m) => return Ok(m),
        Err(e) => e,
    };
    pmp(port, pinned, lease).map_err(|pmp_err| format!("UPnP: {upnp_err}; NAT-PMP: {pmp_err}"))
}

/// This PC's address on the network towards `to` (no packet is sent).
fn local_ip_towards(to: Ipv4Addr) -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect((to, 9)).ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(a) => Some(*a.ip()),
        SocketAddr::V6(_) => None,
    }
}

fn upnp(port: u16, pinned: Option<Ipv4Addr>, lease: u32, description: &str) -> Result<Mapping, String> {
    // The search goes to the router alone, not the whole network.
    let router = default_gateway().ok_or("no default gateway")?;
    let gateway = igd_next::search_gateway(SearchOptions {
        timeout: Some(Duration::from_secs(3)),
        broadcast_address: SocketAddr::V4(SocketAddrV4::new(router, 1900)),
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    let gw_ip = match gateway.addr {
        SocketAddr::V4(a) => *a.ip(),
        SocketAddr::V6(_) => return Err("IPv6 gateway".into()),
    };
    if gw_ip != router {
        return Err(format!("{gw_ip} answered, but the router is {router}; ignored"));
    }
    let local = pinned.or_else(|| local_ip_towards(gw_ip)).ok_or("no local address towards the router")?;
    let external = match gateway.get_external_ip().map_err(|e| e.to_string())? {
        std::net::IpAddr::V4(ip) => ip,
        std::net::IpAddr::V6(_) => return Err("IPv6 external address".into()),
    };
    usable_external(external)?;
    let internal = SocketAddr::V4(SocketAddrV4::new(local, port));
    let mapped = match gateway.add_port(PortMappingProtocol::UDP, port, internal, lease, description) {
        Ok(()) => port,
        // Taken, e.g. by another PC on this network playing too.
        Err(_) => gateway.add_any_port(PortMappingProtocol::UDP, internal, lease, description).map_err(|e| e.to_string())?,
    };
    Ok(Mapping {
        public: SocketAddrV4::new(external, mapped),
        how: "UPnP",
        handle: Handle::Upnp { gateway, port: mapped },
    })
}

/// Whether the router's "external" address could be one the internet reaches.
/// A private or shared (carrier-grade NAT) one means the mapping is no use,
/// and a made-up one would send other players somewhere else.
pub fn usable_external(ip: Ipv4Addr) -> Result<(), String> {
    let [a, b, ..] = ip.octets();
    let shared = a == 100 && (64..128).contains(&b);
    if ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() || shared || a == 0 || a >= 240 {
        return Err(format!("the router's external address {ip} isn't a public one (another router or the ISP's NAT is in front)"));
    }
    Ok(())
}

/// The default gateway: the next hop towards the internet.
#[cfg(target_os = "windows")]
pub fn default_gateway() -> Option<Ipv4Addr> {
    use windows::Win32::NetworkManagement::IpHelper::GetBestRoute;
    use windows::Win32::NetworkManagement::IpHelper::MIB_IPFORWARDROW;
    let mut row = MIB_IPFORWARDROW::default();
    let dest = u32::from_ne_bytes([8, 8, 8, 8]);
    if unsafe { GetBestRoute(dest, 0, &mut row) } != 0 {
        return None;
    }
    let hop = Ipv4Addr::from(row.dwForwardNextHop.to_ne_bytes());
    (!hop.is_unspecified()).then_some(hop)
}

/// The default gateway: the next hop towards the internet.
#[cfg(target_os = "linux")]
pub fn default_gateway() -> Option<Ipv4Addr> {
    gateway_from_route_table(&std::fs::read_to_string("/proc/net/route").ok()?)
}

/// The default gateway.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn default_gateway() -> Option<Ipv4Addr> {
    None
}

/// The default route's gateway in `/proc/net/route` (hex, in host byte order).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn gateway_from_route_table(table: &str) -> Option<Ipv4Addr> {
    const RTF_GATEWAY: u32 = 0x2;
    table.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (dest, gateway, flags) = (f.get(1)?, f.get(2)?, u32::from_str_radix(f.get(3)?, 16).ok()?);
        if *dest != "00000000" || flags & RTF_GATEWAY == 0 {
            return None;
        }
        let hop = Ipv4Addr::from(u32::from_str_radix(gateway, 16).ok()?.to_le_bytes());
        (!hop.is_unspecified()).then_some(hop)
    })
}

fn pmp_request(gateway: Ipv4Addr, local: Ipv4Addr, request: &[u8], reply_len: usize) -> Result<Vec<u8>, String> {
    let socket = UdpSocket::bind((local, 0)).map_err(|e| e.to_string())?;
    socket.connect((gateway, PMP_PORT)).map_err(|e| e.to_string())?;
    let mut wait = Duration::from_millis(250);
    let mut buf = [0u8; 16];
    // RFC 6886: 250 ms, doubling; stop at 2 s here.
    while wait <= Duration::from_secs(2) {
        socket.send(request).map_err(|e| e.to_string())?;
        socket.set_read_timeout(Some(wait)).map_err(|e| e.to_string())?;
        if let Ok(n) = socket.recv(&mut buf) {
            if n >= reply_len && buf[0] == 0 && buf[1] == request[1] + 128 {
                let result = u16::from_be_bytes([buf[2], buf[3]]);
                if result != 0 {
                    return Err(format!("the router refused (result {result})"));
                }
                return Ok(buf[..n].to_vec());
            }
        }
        wait *= 2;
    }
    Err("no answer".into())
}

fn pmp_map(gateway: Ipv4Addr, local: Ipv4Addr, port: u16, lifetime: u32) -> Result<u16, String> {
    let mut req = vec![0u8, 1, 0, 0];
    req.extend_from_slice(&port.to_be_bytes());
    req.extend_from_slice(&port.to_be_bytes());
    req.extend_from_slice(&lifetime.to_be_bytes());
    let reply = pmp_request(gateway, local, &req, 16)?;
    Ok(u16::from_be_bytes([reply[10], reply[11]]))
}

fn pmp(port: u16, pinned: Option<Ipv4Addr>, lease: u32) -> Result<Mapping, String> {
    let gateway = default_gateway().ok_or("no default gateway")?;
    let local = pinned.or_else(|| local_ip_towards(gateway)).ok_or("no local address towards the router")?;
    let reply = pmp_request(gateway, local, &[0, 0], 12)?;
    let external = Ipv4Addr::new(reply[8], reply[9], reply[10], reply[11]);
    usable_external(external)?;
    let mapped = pmp_map(gateway, local, port, lease)?;
    Ok(Mapping {
        public: SocketAddrV4::new(external, mapped),
        how: "NAT-PMP",
        handle: Handle::Pmp { gateway, local, port },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_default_route() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
                     eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n";
        assert_eq!(gateway_from_route_table(table), Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(gateway_from_route_table("Iface\tDestination\n"), None);
    }

    #[test]
    fn only_public_external_addresses() {
        assert!(usable_external(Ipv4Addr::new(203, 0, 114, 7)).is_ok());
        for ip in [[192, 168, 1, 2], [10, 0, 0, 1], [100, 64, 0, 1], [0, 0, 0, 0]] {
            assert!(usable_external(Ipv4Addr::from(ip)).is_err(), "{ip:?}");
        }
    }
}
