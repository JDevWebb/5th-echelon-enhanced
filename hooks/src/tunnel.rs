//! The tunnel (0.5.2's T2): when the launcher found this PC's UDP blocked (a carrier that lets
//! HTTPS through and nothing else), the game's datagrams to the server go in one WebSocket over
//! HTTPS instead (`wss://<server>/tunnel`, dedicated_server's tunnel.rs).
//!
//! The game keeps its real sockets. For each of the server's UDP services it uses (the sign-in,
//! the secure service, the NAT helper) a forwarder socket waits on 127.0.0.1, and the `sendto`
//! hook (nat.rs) sends what the game sends to that service to its forwarder instead
//! ([`redirect`]); the forwarder puts it on the WebSocket. What comes back is sent from the
//! forwarder to the game's socket, and the `recvfrom` hook shows it as from the server
//! ([`from_forwarder`]). Everything else (the relay's wrapping, Storm) is as for a relayed
//! player: the helper relays a tunnelled player.
//!
//! The first message proves the name (a signed probe with the NAT ticket, [`crate::hooks::nat::tunnel_proof`]).
//! A dropped WebSocket is opened again, a little later each time.

use std::net::SocketAddrV4;
use std::net::ToSocketAddrs;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::time::Duration;

use futures_util::SinkExt;
use futures_util::StreamExt;
use nat_proto::tunnel;
use tokio_tungstenite::tungstenite::Message as Ws;
use tracing::info;
use tracing::warn;

/// How long the server keeps a dropped tunnel's place (dedicated_server's tunnel.rs).
const RESUME_WINDOW: Duration = Duration::from_secs(30);

/// The server's services and their forwarders: (the server's address, the forwarder's).
static MAP: RwLock<Vec<(u8, SocketAddrV4, SocketAddrV4)>> = RwLock::new(Vec::new());

/// Where a datagram the game sends to `dest` goes instead: its service's forwarder, when the
/// tunnel is on and `dest` is one of the server's services.
pub fn redirect(dest: SocketAddrV4) -> Option<SocketAddrV4> {
    MAP.read().ok()?.iter().find(|(_, server, _)| *server == dest).map(|(_, _, local)| *local)
}

/// The server's address a datagram from forwarder `src` stands for, if `src` is one: its port,
/// from one of this PC's own addresses. A forwarder listens on every address, and its replies
/// to a game socket bound to the PC's LAN address come from that address, not 127.0.0.1
/// (Linux drops a datagram from 127.0.0.1 to another address as a martian; the gamebox,
/// 2026-11-15).
pub fn from_forwarder(src: SocketAddrV4) -> Option<SocketAddrV4> {
    if !own(*src.ip()) {
        return None;
    }
    MAP.read().ok()?.iter().find(|(_, _, local)| local.port() == src.port()).map(|(_, server, _)| *server)
}

/// The addresses the game's sockets were bound to (the PC's own), besides loopback.
static OWN: RwLock<Vec<std::net::Ipv4Addr>> = RwLock::new(Vec::new());

/// The game bound a socket to `ip`: one of this PC's addresses.
pub fn own_address(ip: std::net::Ipv4Addr) {
    if ip.is_unspecified() || ip.is_loopback() {
        return;
    }
    if let Ok(mut own) = OWN.write() {
        if !own.contains(&ip) {
            own.push(ip);
        }
    }
}

/// Whether `ip` is this PC's: loopback, or an address the game bound a socket to.
fn own(ip: std::net::Ipv4Addr) -> bool {
    ip.is_loopback() || OWN.read().is_ok_and(|o| o.contains(&ip))
}

/// The tunnel's last round trip, in ms (a WebSocket ping to where it ends: the server, or the
/// relay it opens at), with when it was measured. A tunnelled player's UDP probes to the NAT
/// helper are what the tunnel is there for: they never come back, and the relay's ping went
/// unshown (0.5.5's T).
static RTT: Mutex<Option<(u16, std::time::Instant)>> = Mutex::new(None);

/// How often the tunnel is pinged while open.
const PING_EVERY: Duration = Duration::from_secs(15);

/// A tunnel that closes sooner than this after opening is a short one (0.5.6's TL:
/// CoolPeaceMaker's lived 18-20 s each, 531 times in two hours, closed by something on their
/// path, and the game signed in again after most of them).
const SHORT_LIFE: Duration = Duration::from_secs(60);
/// After this many short tunnels in a row, the player is told and reopening slows down.
const SHORT_RUNS: u32 = 3;

/// How long to wait before reopening after `short` short tunnels in a row: None while there
/// have been fewer than [`SHORT_RUNS`] (the usual quick reopen), then 30 s, then a minute, so a
/// link that keeps being cut doesn't have the game signing in every 20 s.
fn slowed_reopen(short: u32) -> Option<Duration> {
    match short {
        s if s < SHORT_RUNS => None,
        s if s == SHORT_RUNS => Some(Duration::from_secs(30)),
        _ => Some(Duration::from_secs(60)),
    }
}

/// Whether the player has been told their tunnel keeps being cut, this run.
static TOLD_SHORT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The round trip to `server` (a NAT helper's address) if it goes through the tunnel: Some(the
/// tunnel's ping in ms, measured in the last minute, if any); None when it doesn't go through.
pub fn round_trip_for(server: SocketAddrV4) -> Option<Option<u16>> {
    redirect(server)?;
    Some(RTT.lock().ok().and_then(|r| *r).filter(|(_, at)| at.elapsed() < Duration::from_secs(60)).map(|(ms, _)| ms))
}

/// Notes a pong whose payload is `data`, against the ping sent with it at `sent`.
fn note_pong(sent: &mut Option<([u8; 8], std::time::Instant)>, data: &[u8]) {
    if let Some((_, at)) = sent.take_if(|(nonce, _)| nonce[..] == *data) {
        let ms = u16::try_from(at.elapsed().as_millis()).unwrap_or(u16::MAX).max(1);
        if let Ok(mut rtt) = RTT.lock() {
            *rtt = Some((ms, std::time::Instant::now()));
        }
    }
}

/// Whether the game's traffic goes through the tunnel this run.
pub fn on() -> bool {
    MAP.read().is_ok_and(|m| !m.is_empty())
}

fn resolve_v4(host: &str) -> Option<std::net::Ipv4Addr> {
    (host, 443).to_socket_addrs().ok().and_then(|mut a| {
        a.find_map(|a| match a {
            std::net::SocketAddr::V4(v4) => Some(*v4.ip()),
            std::net::SocketAddr::V6(_) => None,
        })
    })
}

/// Starts the tunnel to `target` (the launcher's `networking.tunnel`). Through a relay
/// (`target.via`, 0.5.5's SR), the tunnel ends there: the game's sign-in and secure service still
/// go to the server's addresses (the relay passes them on), and its helper is the relay's.
pub fn start(target: hooks_config::Tunnel) {
    let Some(server) = resolve_v4(&target.host) else {
        warn!("Tunnel: {} didn't resolve; the game's traffic goes as usual", target.host);
        return;
    };
    let (end, helper_ip) = match target.via.as_deref() {
        Some(via) => match resolve_v4(via) {
            Some(ip) => (via.to_string(), ip),
            None => {
                warn!("Tunnel: the relay {via} didn't resolve; the game's traffic goes as usual");
                return;
            }
        },
        None => (target.host.clone(), server),
    };
    let services = [
        (tunnel::LOGIN, SocketAddrV4::new(server, target.login)),
        (tunnel::SECURE, SocketAddrV4::new(server, target.secure)),
        (tunnel::HELPER, SocketAddrV4::new(helper_ip, target.helper)),
    ];
    if start_with(&end, &services) {
        match &target.via {
            Some(via) => info!("Tunnel: the game signs in to {} through the relay {via} (https://{via}{})", target.host, tunnel::PATH),
            None => info!("Tunnel: the game's UDP to {server} goes through https://{}{}", target.host, tunnel::PATH),
        }
    }
}

/// Starts a tunnel for the NAT helper's traffic only (its probes and the relayed game traffic)
/// to `host`, whose helper is `helper`: the add-on's fallback when the helper never hears this
/// PC over UDP although the game signed in (nat.rs's `TUNNEL_AFTER`). The sign-in and the secure
/// service, which work, keep their UDP. False when it couldn't start.
pub fn start_helper(host: &str, helper: SocketAddrV4) -> bool {
    let started = start_with(host, &[(tunnel::HELPER, helper)]);
    if started {
        info!(
            "Tunnel: the relay's traffic (the NAT helper {helper}) goes through https://{host}{} from now on",
            tunnel::PATH
        );
    }
    started
}

/// Starts a tunnel to `host` carrying `services` (each with the server's address the game sends
/// it to): a forwarder for each, the redirects, and the WebSocket. False when no forwarder
/// could be made.
fn start_with(host: &str, services: &[(u8, SocketAddrV4)]) -> bool {
    let mut forwarders = Vec::new();
    for &(service, server) in services {
        // On every address, so it can answer a game socket bound to the LAN address from that
        // address ([`from_forwarder`]); only datagrams from this PC's own addresses are taken.
        let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
            warn!("Tunnel: no local socket; the game's traffic goes as usual");
            return false;
        };
        let Ok(bound) = socket.local_addr() else { return false };
        let local = SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, bound.port());
        forwarders.push((service, server, local, Arc::new(socket)));
    }
    if let Ok(mut map) = MAP.write() {
        *map = forwarders.iter().map(|(service, server, local, _)| (*service, *server, *local)).collect();
    }
    let url = format!("wss://{host}{}", tunnel::PATH);
    let _ = std::thread::Builder::new().name("fe-tunnel".into()).spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() else {
            warn!("Tunnel: couldn't start");
            return;
        };
        rt.block_on(run(url, forwarders));
    });
    true
}

type Forwarder = (u8, SocketAddrV4, SocketAddrV4, Arc<UdpSocket>);

/// The game's socket each service last heard from: where its replies go.
static GAME: Mutex<[Option<std::net::SocketAddr>; 4]> = Mutex::new([None; 4]);

async fn run(url: String, forwarders: Vec<Forwarder>) {
    let (to_ws, mut from_game) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    // Each forwarder's datagrams from the game, onto the tunnel.
    for (service, _, _, socket) in &forwarders {
        let (service, socket, to_ws) = (*service, Arc::clone(socket), to_ws.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while let Ok((n, from)) = socket.recv_from(&mut buf) {
                // Only the game on this PC: never another machine that found the port.
                if !matches!(from, std::net::SocketAddr::V4(v4) if own(*v4.ip())) {
                    continue;
                }
                if let Ok(mut g) = GAME.lock() {
                    g[usize::from(service)] = Some(from);
                }
                let _ = to_ws.blocking_send(tunnel::frame(service, &buf[..n]));
            }
        });
    }
    let mut wait = Duration::from_secs(1);
    // When an open tunnel last dropped: within the server's 30 s to resume it, every second.
    let mut dropped_at: Option<std::time::Instant> = None;
    // Short tunnels in a row ([`SHORT_LIFE`]).
    let mut short = 0u32;
    loop {
        // The proof needs the NAT ticket, from signing in.
        let proof = loop {
            if let Some(p) = crate::hooks::nat::tunnel_proof() {
                break p;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        let Some(tls) = connector() else {
            warn!("Tunnel: no TLS set-up; the game's traffic can't go through it");
            return;
        };
        match tokio_tungstenite::connect_async_tls_with_config(url.as_str(), None, false, Some(tls)).await {
            Ok((ws, _)) => {
                info!("Tunnel: open");
                wait = Duration::from_secs(1);
                let opened = std::time::Instant::now();
                let (mut bytes_up, mut bytes_down) = (0u64, 0u64);
                let why: String;
                let (mut down, mut up) = ws.split();
                if down.send(Ws::Binary(tunnel::frame(tunnel::AUTH, &proof).into())).await.is_err() {
                    continue;
                }
                let mut ping = tokio::time::interval(PING_EVERY);
                let mut sent: Option<([u8; 8], std::time::Instant)> = None;
                loop {
                    tokio::select! {
                        _ = ping.tick() => {
                            let nonce = rand::random::<[u8; 8]>();
                            sent = Some((nonce, std::time::Instant::now()));
                            if let Err(e) = down.send(Ws::Ping(nonce.to_vec().into())).await {
                                why = format!("an error sending a ping: {e}");
                                break;
                            }
                        }
                        out = from_game.recv() => {
                            let Some(out) = out else { return };
                            bytes_up += out.len() as u64;
                            if let Err(e) = down.send(Ws::Binary(out.into())).await {
                                why = format!("an error sending: {e}");
                                break;
                            }
                        }
                        message = up.next() => match message {
                            Some(Ok(Ws::Binary(data))) => {
                                bytes_down += data.len() as u64;
                                let Some((service, datagram)) = tunnel::parse(&data) else { continue };
                                let to = GAME.lock().ok().and_then(|g| g.get(usize::from(service)).copied().flatten());
                                if let (Some(to), Some((.., socket))) = (to, forwarders.iter().find(|f| f.0 == service)) {
                                    let _ = socket.send_to(datagram, to);
                                }
                            }
                            Some(Ok(Ws::Pong(data))) => note_pong(&mut sent, &data),
                            Some(Ok(Ws::Close(frame))) => {
                                why = match frame {
                                    Some(f) => format!("the server's side closed it ({})", u16::from(f.code)),
                                    None => "the server's side closed it".into(),
                                };
                                break;
                            }
                            None => {
                                why = "the connection ended without a close".into();
                                break;
                            }
                            Some(Err(e)) => {
                                why = format!("an error: {e}");
                                break;
                            }
                            Some(Ok(_)) => {}
                        }
                    }
                }
                let lived = opened.elapsed();
                warn!(
                    "Tunnel: closed after {} s ({bytes_up} bytes up, {bytes_down} down; {why}); opening it again",
                    lived.as_secs()
                );
                dropped_at = Some(std::time::Instant::now());
                short = if lived < SHORT_LIFE { short + 1 } else { 0 };
                if short >= SHORT_RUNS && !TOLD_SHORT.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    crate::community::tell(
                        "Your connection keeps being cut",
                        "Your connection to the server keeps being cut, most likely by your network or internet provider. A VPN or another network may help.",
                    );
                }
                if let Some(slow) = slowed_reopen(short) {
                    warn!(
                        "Tunnel: {short} closed within {} s of opening in a row; waiting {} s before opening it again",
                        SHORT_LIFE.as_secs(),
                        slow.as_secs()
                    );
                    tokio::time::sleep(slow).await;
                    continue;
                }
            }
            Err(e) => warn!("Tunnel: couldn't open ({e}); trying again in {} s", wait.as_secs()),
        }
        // The game gives up on a silent player after about 10 s (the gamebox, 2026-11-15: an 8 s
        // drop with the doubling wait reopened after 15 s, and the match was lost). While the
        // server keeps the player's place, try every second; then wait longer each time.
        if dropped_at.is_some_and(|t| t.elapsed() < RESUME_WINDOW) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(30));
    }
}

/// TLS with the usual web roots (the server's certificate is a public one, Caddy's), on ring as
/// the rest of the add-on's TLS is.
fn connector() -> Option<tokio_tungstenite::Connector> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .ok()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Some(tokio_tungstenite::Connector::Rustls(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TL: a tunnel that keeps being cut reopens as usual twice, then after 30 s, then a minute.
    #[test]
    fn a_tunnel_that_keeps_being_cut_reopens_more_slowly() {
        assert_eq!(slowed_reopen(0), None);
        assert_eq!(slowed_reopen(SHORT_RUNS - 1), None, "the usual quick reopen, for the server's resume window");
        assert_eq!(slowed_reopen(SHORT_RUNS), Some(Duration::from_secs(30)));
        assert_eq!(slowed_reopen(SHORT_RUNS + 5), Some(Duration::from_secs(60)));
    }

    /// T: a pong for the ping in flight gives the round trip; another is ignored.
    #[test]
    fn a_pong_for_the_ping_sent_is_the_round_trip() {
        let mut sent = Some(([1u8; 8], std::time::Instant::now() - Duration::from_millis(40)));
        note_pong(&mut sent, &[2u8; 8]);
        assert!(sent.is_some(), "not ours: still waiting");
        note_pong(&mut sent, &[1u8; 8]);
        assert!(sent.is_none());
        let (ms, _) = RTT.lock().unwrap().unwrap();
        assert!((40..1000).contains(&ms), "{ms}");
    }

    #[test]
    fn a_forwarders_reply_is_known_by_its_port_from_this_pcs_own_addresses() {
        let server = SocketAddrV4::new(std::net::Ipv4Addr::new(45, 77, 50, 224), 21128);
        *MAP.write().unwrap() = vec![(tunnel::HELPER, server, SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 50000))];
        let lan = std::net::Ipv4Addr::new(192, 168, 1, 20);
        assert_eq!(redirect(server), Some(SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 50000)));
        assert_eq!(from_forwarder(SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 50000)), Some(server));
        assert_eq!(from_forwarder(SocketAddrV4::new(lan, 50000)), None, "not one of this PC's addresses yet");
        own_address(lan);
        assert_eq!(from_forwarder(SocketAddrV4::new(lan, 50000)), Some(server), "the game bound a socket to it");
        assert_eq!(from_forwarder(SocketAddrV4::new(lan, 50001)), None, "another port");
        assert_eq!(from_forwarder(SocketAddrV4::new(std::net::Ipv4Addr::new(192, 168, 1, 30), 50000)), None, "another machine");
        own_address(std::net::Ipv4Addr::UNSPECIFIED);
        assert!(!own(std::net::Ipv4Addr::UNSPECIFIED));
        MAP.write().unwrap().clear();
    }
}
