//! Asks the router to forward a UDP port to this PC: UPnP first, then PCP
//! (RFC 6887, which some newer routers offer instead), then NAT-PMP. The game client keeps Storm's port (13000) mapped while the game
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
    Pcp { gateway: Ipv4Addr, local: Ipv4Addr, port: u16, nonce: [u8; 12] },
}

impl Mapping {
    /// Asks the router to stop forwarding the port.
    pub fn remove(self) {
        match self.handle {
            Handle::Upnp { gateway, port } => {
                let _ = gateway.remove_port(PortMappingProtocol::UDP, port);
            }
            Handle::Pmp { gateway, local, port } => {
                let _ = pmp_map(gateway, local, port, 0, 0);
            }
            Handle::Pcp { gateway, local, port, nonce } => {
                let _ = pcp_request(gateway, local, &pcp_map_request(local, nonce, port, 0, 0));
            }
        }
    }
}

/// Asks the router to forward UDP `port` to this PC (at `pinned`, or the
/// address this PC reaches the router from) for `lease` seconds. On failure,
/// says why for each way tried.
pub fn map(port: u16, pinned: Option<Ipv4Addr>, lease: u32, description: &str) -> Result<Mapping, String> {
    map_as(port, port, pinned, lease, description)
}

/// As [`map`], asking for `external` on the router's side (the port it already gives this PC's
/// `port`; see [`port_to_ask`]).
pub fn map_as(port: u16, external: u16, pinned: Option<Ipv4Addr>, lease: u32, description: &str) -> Result<Mapping, String> {
    let upnp_err = match upnp(port, external, pinned, lease, description) {
        Ok(m) => return Ok(m),
        Err(e) => e,
    };
    let pcp_err = match pcp(port, external, pinned, lease) {
        Ok(m) => return Ok(m),
        Err(e) => e,
    };
    pmp(port, external, pinned, lease).map_err(|pmp_err| format!("UPnP: {upnp_err}; PCP: {pcp_err}; NAT-PMP: {pmp_err}"))
}

/// The router-side port to ask for, for this PC's `local` port: the one the server saw this PC's
/// packets come from, when it saw them (`observed`). Most routers keep the port, so it's
/// `local`; one that gave another port did so because another device on the network holds
/// `local` there (two PCs playing behind one router, 0.5.2's J7). Asking for `local` then makes a
/// forward that collides with that device's traffic, and the server's answers go to the wrong
/// PC; asking for the port the router already gave this PC doesn't.
pub fn port_to_ask(local: u16, observed: Option<u16>) -> u16 {
    observed.filter(|p| *p != 0).unwrap_or(local)
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

/// Finds the router's UPnP service: the standard search to the whole
/// network (some routers answer nothing else), then one sent to the router
/// alone. Either way only the router's own answer is taken, so another
/// device on the network can't pose as it.
fn find_router(router: Ipv4Addr, local: Ipv4Addr) -> Result<igd_next::Gateway, String> {
    let search = |to: SocketAddrV4| {
        igd_next::search_gateway(SearchOptions {
            bind_addr: SocketAddr::V4(SocketAddrV4::new(local, 0)),
            broadcast_address: SocketAddr::V4(to),
            timeout: Some(Duration::from_secs(3)),
            ..Default::default()
        })
    };
    let is_router = |g: &igd_next::Gateway| matches!(g.addr, SocketAddr::V4(a) if *a.ip() == router);
    let multicast = search(SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900));
    if let Ok(g) = &multicast {
        if is_router(g) {
            return multicast.map_err(|e| e.to_string());
        }
    }
    match search(SocketAddrV4::new(router, 1900)) {
        Ok(g) if is_router(&g) => Ok(g),
        Ok(g) => Err(format!("{} answered, but the router is {router}; ignored", g.addr.ip())),
        Err(direct) => Err(match multicast {
            Ok(g) => format!("{} answered, but the router is {router}; ignored", g.addr.ip()),
            Err(e) => format!("no answer from the router ({e}; asked directly: {direct})"),
        }),
    }
}

fn upnp(port: u16, external_port: u16, pinned: Option<Ipv4Addr>, lease: u32, description: &str) -> Result<Mapping, String> {
    let router = default_gateway().ok_or("no default gateway")?;
    let local = pinned.or_else(|| local_ip_towards(router)).ok_or("no local address towards the router")?;
    let gateway = find_router(router, local)?;
    let external = match gateway.get_external_ip().map_err(|e| e.to_string())? {
        std::net::IpAddr::V4(ip) => ip,
        std::net::IpAddr::V6(_) => return Err("IPv6 external address".into()),
    };
    usable_external(external)?;
    let internal = SocketAddr::V4(SocketAddrV4::new(local, port));
    let mapped = match gateway.add_port(PortMappingProtocol::UDP, external_port, internal, lease, description) {
        Ok(()) => external_port,
        Err(_) => {
            // This PC's own mapping from an earlier game (one that didn't get to remove it)
            // holds the port for up to its lease; many routers then refuse any other mapping
            // to the same address and port ("no free ports"). Only mappings to this PC's
            // address and port are taken away.
            let entries = (0..MAX_ENTRIES).map_while(|i| gateway.get_generic_port_mapping_entry(i).ok());
            let stale = stale_mappings(entries.map(|e| (e.protocol, e.external_port, e.internal_client, e.internal_port)), local, port);
            for external in &stale {
                let _ = gateway.remove_port(PortMappingProtocol::UDP, *external);
            }
            match gateway.add_port(PortMappingProtocol::UDP, external_port, internal, lease, description) {
                Ok(()) => external_port,
                // Taken, e.g. by another PC on this network playing too.
                Err(_) => gateway.add_any_port(PortMappingProtocol::UDP, internal, lease, description).map_err(|e| e.to_string())?,
            }
        }
    };
    Ok(Mapping {
        public: SocketAddrV4::new(external, mapped),
        how: "UPnP",
        handle: Handle::Upnp { gateway, port: mapped },
    })
}

/// The most mapping entries read from a router's table.
const MAX_ENTRIES: u32 = 128;

/// The external ports of the router's UDP mappings to `local`:`port`, from its table's
/// (protocol, external port, internal client, internal port) entries.
fn stale_mappings(entries: impl Iterator<Item = (PortMappingProtocol, u16, String, u16)>, local: Ipv4Addr, port: u16) -> Vec<u16> {
    entries
        .filter(|(protocol, _, client, internal)| *protocol == PortMappingProtocol::UDP && *internal == port && client.trim() == local.to_string())
        .map(|(_, external, _, _)| external)
        .collect()
}

/// Whether the router's "external" address could be one the internet reaches.
/// A private or shared (carrier-grade NAT) one means the mapping is no use,
/// and a made-up one would send other players somewhere else.
pub fn usable_external(ip: Ipv4Addr) -> Result<(), String> {
    let [a, b, ..] = ip.octets();
    let shared = a == 100 && (64..128).contains(&b);
    if ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() || shared || a == 0 || a >= 240 {
        return Err(format!(
            "the router's external address {ip} isn't a public one (another router or the ISP's NAT is in front)"
        ));
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

/// Which network this PC is on (0.5.2's J4: the launcher remembers what each allows): the
/// default gateway's hardware address, else its IP address. Kept on this PC only.
pub fn network_id() -> Option<String> {
    let gateway = default_gateway()?;
    Some(match gateway_mac(gateway) {
        Some(mac) => format!("mac {}", mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")),
        None => format!("gw {gateway}"),
    })
}

/// The hardware address of `gateway` (the system's ARP table).
#[cfg(target_os = "windows")]
pub fn gateway_mac(gateway: Ipv4Addr) -> Option<[u8; 6]> {
    use windows::Win32::NetworkManagement::IpHelper::SendARP;
    let mut mac = [0u8; 8];
    let mut len = mac.len() as u32;
    let dest = u32::from_ne_bytes(gateway.octets());
    // SAFETY: an 8-byte buffer and its length, as SendARP asks.
    if unsafe { SendARP(dest, 0, mac.as_mut_ptr().cast(), &mut len) } != 0 || len < 6 {
        return None;
    }
    let mac: [u8; 6] = mac[..6].try_into().ok()?;
    (mac != [0; 6]).then_some(mac)
}

/// The hardware address of `gateway` (`/proc/net/arp`).
#[cfg(target_os = "linux")]
pub fn gateway_mac(gateway: Ipv4Addr) -> Option<[u8; 6]> {
    mac_from_arp_table(&std::fs::read_to_string("/proc/net/arp").ok()?, gateway)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn gateway_mac(_gateway: Ipv4Addr) -> Option<[u8; 6]> {
    None
}

/// `ip`'s hardware address in `/proc/net/arp` ("IP HW-type Flags HW-address Mask Device").
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mac_from_arp_table(table: &str, ip: Ipv4Addr) -> Option<[u8; 6]> {
    let line = table.lines().skip(1).find(|l| l.split_whitespace().next() == Some(&ip.to_string()))?;
    let hw = line.split_whitespace().nth(3)?;
    let bytes: Vec<u8> = hw.split(':').filter_map(|b| u8::from_str_radix(b, 16).ok()).collect();
    let mac: [u8; 6] = bytes.try_into().ok()?;
    (mac != [0; 6]).then_some(mac)
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

fn pmp_map(gateway: Ipv4Addr, local: Ipv4Addr, port: u16, external: u16, lifetime: u32) -> Result<u16, String> {
    let mut req = vec![0u8, 1, 0, 0];
    req.extend_from_slice(&port.to_be_bytes());
    req.extend_from_slice(&external.to_be_bytes());
    req.extend_from_slice(&lifetime.to_be_bytes());
    let reply = pmp_request(gateway, local, &req, 16)?;
    Ok(u16::from_be_bytes([reply[10], reply[11]]))
}

fn pmp(port: u16, external_port: u16, pinned: Option<Ipv4Addr>, lease: u32) -> Result<Mapping, String> {
    let gateway = default_gateway().ok_or("no default gateway")?;
    let local = pinned.or_else(|| local_ip_towards(gateway)).ok_or("no local address towards the router")?;
    let reply = pmp_request(gateway, local, &[0, 0], 12)?;
    let external = Ipv4Addr::new(reply[8], reply[9], reply[10], reply[11]);
    usable_external(external)?;
    let mapped = pmp_map(gateway, local, port, external_port, lease)?;
    Ok(Mapping {
        public: SocketAddrV4::new(external, mapped),
        how: "NAT-PMP",
        handle: Handle::Pmp { gateway, local, port },
    })
}

/// A PCP MAP request (RFC 6887 §11.1) for UDP `port` on `local` to `external` (0: any) for
/// `lifetime` seconds (0: remove), with the mapping's `nonce`.
fn pcp_map_request(local: Ipv4Addr, nonce: [u8; 12], port: u16, external: u16, lifetime: u32) -> Vec<u8> {
    let mut req = vec![2u8, 1, 0, 0];
    req.extend_from_slice(&lifetime.to_be_bytes());
    req.extend_from_slice(&local.to_ipv6_mapped().octets());
    req.extend_from_slice(&nonce);
    req.extend_from_slice(&[17, 0, 0, 0]);
    req.extend_from_slice(&port.to_be_bytes());
    req.extend_from_slice(&external.to_be_bytes());
    req.extend_from_slice(&[0u8; 16]);
    req
}

/// A fresh PCP mapping nonce (RFC 6887 11.1: random, so another host can't take over or remove
/// the mapping). The standard library's hasher keys come from the system's random source; the
/// time and a counter make each call's differ. (It was the process id's, its 12 bytes nearly all
/// the same: miniupnpd logged "PCP MAP 616161616161616161616161" on the 0.5.5 router rig.)
fn pcp_nonce() -> [u8; 12] {
    use std::hash::BuildHasher;
    use std::hash::Hasher;
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut out = [0u8; 12];
    for (i, chunk) in out.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(call);
        h.write_usize(i);
        h.write_u128(now);
        h.write_u32(std::process::id());
        chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
    }
    out
}

/// A PCP MAP answer for `nonce`: the assigned external address and port.
fn pcp_map_answer(reply: &[u8], nonce: &[u8; 12]) -> Result<SocketAddrV4, String> {
    if reply.len() < 60 || reply[0] != 2 || reply[1] != 0x81 {
        return Err("not a PCP MAP answer".into());
    }
    if reply[3] != 0 {
        return Err(format!("the router refused (result {})", reply[3]));
    }
    if reply[24..36] != nonce[..] {
        return Err("an answer to another request".into());
    }
    let port = u16::from_be_bytes([reply[42], reply[43]]);
    let ip: [u8; 16] = reply[44..60].try_into().map_err(|_| "short".to_string())?;
    let ip = std::net::Ipv6Addr::from(ip).to_ipv4_mapped().ok_or("an IPv6 address")?;
    Ok(SocketAddrV4::new(ip, port))
}

fn pcp_request(gateway: Ipv4Addr, local: Ipv4Addr, request: &[u8]) -> Result<Vec<u8>, String> {
    let socket = UdpSocket::bind((local, 0)).map_err(|e| e.to_string())?;
    socket.connect((gateway, PMP_PORT)).map_err(|e| e.to_string())?;
    let mut wait = Duration::from_millis(250);
    let mut buf = [0u8; 1100];
    while wait <= Duration::from_secs(2) {
        socket.send(request).map_err(|e| e.to_string())?;
        socket.set_read_timeout(Some(wait)).map_err(|e| e.to_string())?;
        if let Ok(n) = socket.recv(&mut buf) {
            return Ok(buf[..n].to_vec());
        }
        wait *= 2;
    }
    Err("no answer".into())
}

fn pcp(port: u16, external_port: u16, pinned: Option<Ipv4Addr>, lease: u32) -> Result<Mapping, String> {
    let gateway = default_gateway().ok_or("no default gateway")?;
    let local = pinned.or_else(|| local_ip_towards(gateway)).ok_or("no local address towards the router")?;
    let nonce = pcp_nonce();
    let reply = pcp_request(gateway, local, &pcp_map_request(local, nonce, port, external_port, lease))?;
    let public = pcp_map_answer(&reply, &nonce)?;
    usable_external(*public.ip())?;
    Ok(Mapping {
        public,
        how: "PCP",
        handle: Handle::Pcp { gateway, local, port, nonce },
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn pcp_nonces_are_random_and_differ() {
        let (a, b) = (super::pcp_nonce(), super::pcp_nonce());
        assert_ne!(a, b, "each mapping its own");
        assert!(a.iter().any(|x| *x != a[0]), "not one byte twelve times: {a:?}");
    }
    /// J4: the gateway's hardware address from Linux's ARP table.
    #[test]
    fn the_gateways_hardware_address_is_read() {
        let table = "IP address       HW type     Flags       HW address            Mask     Device\n192.168.1.1      0x1         0x2         a4:2b:b0:12:34:56     *        wlan0\n192.168.1.9      0x1         0x0         00:00:00:00:00:00     *        wlan0\n";
        assert_eq!(super::mac_from_arp_table(table, Ipv4Addr::new(192, 168, 1, 1)), Some([0xa4, 0x2b, 0xb0, 0x12, 0x34, 0x56]));
        assert_eq!(super::mac_from_arp_table(table, Ipv4Addr::new(192, 168, 1, 9)), None, "incomplete");
        assert_eq!(super::mac_from_arp_table(table, Ipv4Addr::new(10, 0, 0, 1)), None);
    }

    /// PCP (RFC 6887): a MAP request as the RFC lays it out, and its answer read back.
    #[test]
    fn pcp_map_requests_and_answers() {
        let local = Ipv4Addr::new(192, 168, 1, 20);
        let nonce = [9u8; 12];
        let req = super::pcp_map_request(local, nonce, 13000, 13000, 3600);
        assert_eq!(req.len(), 60);
        assert_eq!((req[0], req[1]), (2, 1), "version 2, MAP");
        assert_eq!(&req[4..8], &3600u32.to_be_bytes());
        assert_eq!(&req[8..24], &local.to_ipv6_mapped().octets());
        assert_eq!(
            (req[36], u16::from_be_bytes([req[40], req[41]]), u16::from_be_bytes([req[42], req[43]])),
            (17, 13000, 13000)
        );
        let mut answer = req.clone();
        answer[1] = 0x81;
        answer[3] = 0;
        answer[42..44].copy_from_slice(&41234u16.to_be_bytes());
        answer[44..60].copy_from_slice(&Ipv4Addr::new(203, 0, 113, 7).to_ipv6_mapped().octets());
        assert_eq!(super::pcp_map_answer(&answer, &nonce), Ok(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 7), 41234)));
        assert!(super::pcp_map_answer(&answer, &[1; 12]).is_err(), "another nonce");
        answer[3] = 2;
        assert!(super::pcp_map_answer(&answer, &nonce).unwrap_err().contains("refused"));
        assert!(super::pcp_map_answer(&[0u8; 8], &nonce).is_err());
    }

    use super::*;

    #[test]
    fn asks_for_the_port_the_router_already_gives_this_pc() {
        assert_eq!(port_to_ask(13000, None), 13000, "not seen yet");
        assert_eq!(port_to_ask(13000, Some(13000)), 13000, "kept");
        assert_eq!(port_to_ask(13000, Some(41234)), 41234, "another PC holds 13000 at the router");
        assert_eq!(port_to_ask(13000, Some(0)), 13000);
    }

    #[test]
    fn reads_the_default_route() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
                     eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n";
        assert_eq!(gateway_from_route_table(table), Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(gateway_from_route_table("Iface\tDestination\n"), None);
    }

    #[test]
    fn only_this_pcs_own_mappings_are_stale() {
        let local = Ipv4Addr::new(192, 168, 0, 2);
        let entry = |protocol, external, client: &str, internal| (protocol, external, client.to_string(), internal);
        let table = vec![
            entry(PortMappingProtocol::UDP, 13000, "192.168.0.2", 13000),
            entry(PortMappingProtocol::UDP, 41234, "192.168.0.2", 13000),
            entry(PortMappingProtocol::UDP, 13001, "192.168.0.7", 13000),
            entry(PortMappingProtocol::TCP, 13000, "192.168.0.2", 13000),
            entry(PortMappingProtocol::UDP, 3074, "192.168.0.2", 3074),
        ];
        assert_eq!(stale_mappings(table.into_iter(), local, 13000), [13000, 41234]);
    }

    #[test]
    fn only_public_external_addresses() {
        assert!(usable_external(Ipv4Addr::new(203, 0, 114, 7)).is_ok());
        for ip in [[192, 168, 1, 2], [10, 0, 0, 1], [100, 64, 0, 1], [0, 0, 0, 0]] {
            assert!(usable_external(Ipv4Addr::from(ip)).is_err(), "{ip:?}");
        }
    }
}
