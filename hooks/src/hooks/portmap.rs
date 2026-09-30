//! Asks the router to forward Storm's port (UDP 13000) to this PC while the
//! game runs: UPnP first, then NAT-PMP. With a mapping, other players reach
//! this PC directly, even through a NAT that hole punching can't get
//! through. The mapping is renewed while the game runs and removed when it
//! exits (and it expires by itself after an hour otherwise).

use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::net::UdpSocket;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use igd_next::PortMappingProtocol;
use igd_next::SearchOptions;
use tracing::info;
use tracing::warn;

const PORT: u16 = nat_proto::STORM_PORT;
const LEASE: u32 = 3600;
const RENEW: Duration = Duration::from_secs(25 * 60);
const RETRY: Duration = Duration::from_secs(5 * 60);
const DESCRIPTION: &str = "Splinter Cell Blacklist (5th Echelon)";
const PMP_PORT: u16 = 5351;

enum Mapped {
    Upnp { gateway: igd_next::Gateway, port: u16 },
    Pmp { gateway: Ipv4Addr, local: Ipv4Addr },
}

static STOP: AtomicBool = AtomicBool::new(false);
static MAPPED: Mutex<Option<Mapped>> = Mutex::new(None);

pub fn start(pinned: Option<Ipv4Addr>) {
    let _ = std::thread::Builder::new().name("fe-portmap".into()).spawn(move || {
        let mut next = Instant::now();
        let mut logged_failure = false;
        while !STOP.load(Ordering::Relaxed) {
            if Instant::now() >= next {
                match map(pinned) {
                    Some((public, how, mapped)) => {
                        let first = MAPPED.lock().map(|m| m.is_none()).unwrap_or(true);
                        if first {
                            info!("NAT: the router forwards {public} to this PC ({how})");
                        }
                        if let Ok(mut m) = MAPPED.lock() {
                            *m = Some(mapped);
                        }
                        // A mapping behind another NAT (double NAT) is no use
                        // to players outside; the helper ignores it.
                        super::nat::set_mapping(Some(public));
                        next = Instant::now() + RENEW;
                    }
                    None => {
                        if !logged_failure {
                            info!("NAT: the router didn't forward a port (UPnP and NAT-PMP unavailable or refused); hole punching and the relay still work");
                            logged_failure = true;
                        }
                        super::nat::set_mapping(None);
                        next = Instant::now() + RETRY;
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// Removes the mapping (the game is exiting).
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
    let Some(mapped) = MAPPED.lock().ok().and_then(|mut m| m.take()) else {
        return;
    };
    match mapped {
        Mapped::Upnp { gateway, port } => {
            let _ = gateway.remove_port(PortMappingProtocol::UDP, port);
        }
        Mapped::Pmp { gateway, local } => {
            let _ = pmp_map(gateway, local, 0);
        }
    }
}

fn map(pinned: Option<Ipv4Addr>) -> Option<(SocketAddrV4, &'static str, Mapped)> {
    match upnp(pinned) {
        Ok(r) => return Some(r),
        Err(e) => info!("NAT: UPnP: {e}"),
    }
    match pmp(pinned) {
        Ok(r) => Some(r),
        Err(e) => {
            info!("NAT: NAT-PMP: {e}");
            None
        }
    }
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

fn upnp(pinned: Option<Ipv4Addr>) -> Result<(SocketAddrV4, &'static str, Mapped), String> {
    let gateway = igd_next::search_gateway(SearchOptions {
        timeout: Some(Duration::from_secs(3)),
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    let gw_ip = match gateway.addr {
        SocketAddr::V4(a) => *a.ip(),
        SocketAddr::V6(_) => return Err("IPv6 gateway".into()),
    };
    let local = pinned.or_else(|| local_ip_towards(gw_ip)).ok_or("no local address towards the router")?;
    let external = match gateway.get_external_ip().map_err(|e| e.to_string())? {
        std::net::IpAddr::V4(ip) => ip,
        std::net::IpAddr::V6(_) => return Err("IPv6 external address".into()),
    };
    let internal = SocketAddr::V4(SocketAddrV4::new(local, PORT));
    let port = match gateway.add_port(PortMappingProtocol::UDP, PORT, internal, LEASE, DESCRIPTION) {
        Ok(()) => PORT,
        // Taken, e.g. by another PC on this network playing too.
        Err(e) => {
            warn!("NAT: UPnP: port {PORT} not mapped ({e}); asking for any port");
            gateway.add_any_port(PortMappingProtocol::UDP, internal, LEASE, DESCRIPTION).map_err(|e| e.to_string())?
        }
    };
    Ok((SocketAddrV4::new(external, port), "UPnP", Mapped::Upnp { gateway, port }))
}

/// The default gateway: the next hop towards the internet.
fn default_gateway() -> Option<Ipv4Addr> {
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

fn pmp_map(gateway: Ipv4Addr, local: Ipv4Addr, lifetime: u32) -> Result<u16, String> {
    let mut req = vec![0u8, 1, 0, 0];
    req.extend_from_slice(&PORT.to_be_bytes());
    req.extend_from_slice(&PORT.to_be_bytes());
    req.extend_from_slice(&lifetime.to_be_bytes());
    let reply = pmp_request(gateway, local, &req, 16)?;
    Ok(u16::from_be_bytes([reply[10], reply[11]]))
}

fn pmp(pinned: Option<Ipv4Addr>) -> Result<(SocketAddrV4, &'static str, Mapped), String> {
    let gateway = default_gateway().ok_or("no default gateway")?;
    let local = pinned.or_else(|| local_ip_towards(gateway)).ok_or("no local address towards the router")?;
    let reply = pmp_request(gateway, local, &[0, 0], 12)?;
    let external = Ipv4Addr::new(reply[8], reply[9], reply[10], reply[11]);
    let port = pmp_map(gateway, local, LEASE)?;
    Ok((SocketAddrV4::new(external, port), "NAT-PMP", Mapped::Pmp { gateway, local }))
}
