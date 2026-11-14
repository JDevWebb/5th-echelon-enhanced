//! The TCP tunnel (0.5.2's T): for a player whose network blocks UDP (KazakhBro's mobile data,
//! 4 Nov), the game's datagrams come in one WebSocket over HTTPS. Caddy takes it on 443
//! (`/tunnel`) and proxies it here, on the machine only; each tunnel gets a UDP socket on
//! 127.0.0.1 and its datagrams go from there to the same services the game would reach over UDP
//! (the sign-in, the secure service, the NAT helper), and what comes back goes down the tunnel.
//!
//! - The first message proves the name ([`crate::nat_helper::tunnel_auth`]: a signed probe with
//!   its ticket), the same proof a relayed player gives: a tunnel is no wider a door than the relay.
//! - One tunnel per name (a new one replaces it), [`MAX_PER_IP`] per address and [`MAX_TUNNELS`]
//!   in all, each at most [`BYTES_PER_SEC`] (a relayed player's traffic and some).
//! - A tunnelled player is always relayed: the helper sees them at a loopback address, which no
//!   other player can reach, and relays them (no mapping).
//! - Their real address (from Caddy's `X-Forwarded-For`, believed only from 127.0.0.1) is kept by
//!   loopback port ([`real_ip`]) for the places that key on an address. Until those read it, the
//!   per-address limits count every tunnelled player as 127.0.0.1: hence the low caps.

use std::collections::HashMap;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use futures_util::SinkExt;
use futures_util::StreamExt;
use nat_proto::tunnel;
use slog::info;
use slog::warn;
use slog::Logger;
use tokio_tungstenite::tungstenite::Message as Ws;

/// The most tunnels at once, and from one address.
pub const MAX_TUNNELS: usize = 24;
pub const MAX_PER_IP: usize = 3;
/// Each tunnel's bytes a second, both ways together, averaged over a few seconds.
pub const BYTES_PER_SEC: f64 = 256.0 * 1024.0;
/// Keepalive: carriers drop idle TCP too.
const PING_EVERY: Duration = Duration::from_secs(20);
/// The first message must come within this.
const AUTH_WITHIN: Duration = Duration::from_secs(10);
/// A closed tunnel's socket is kept this long for the name's next tunnel: a carrier dropping the
/// WebSocket for a moment leaves the game's connection and relay registration as they were
/// (the services know the player by that socket's port).
const RESUME_WITHIN: Duration = Duration::from_secs(30);

/// Closed tunnels' sockets, by name, kept for [`RESUME_WITHIN`].
fn parked() -> std::sync::MutexGuard<'static, HashMap<String, (Instant, std::sync::Arc<tokio::net::UdpSocket>)>> {
    static PARKED: std::sync::LazyLock<Mutex<HashMap<String, (Instant, std::sync::Arc<tokio::net::UdpSocket>)>>> = std::sync::LazyLock::new(Mutex::default);
    PARKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Where each tunnel's datagrams go on this machine.
#[derive(Debug, Clone, Copy)]
pub struct Services {
    pub login: u16,
    pub secure: u16,
    pub helper: u16,
}

impl Services {
    fn port(self, service: u8) -> Option<u16> {
        match service {
            tunnel::LOGIN => Some(self.login),
            tunnel::SECURE => Some(self.secure),
            tunnel::HELPER => Some(self.helper),
            _ => None,
        }
    }

    fn service(self, port: u16) -> Option<u8> {
        [(self.login, tunnel::LOGIN), (self.secure, tunnel::SECURE), (self.helper, tunnel::HELPER)]
            .into_iter()
            .find(|(p, _)| *p == port)
            .map(|(_, s)| s)
    }
}

#[derive(Default)]
struct Open {
    /// By name: the tunnel's id (a newer one replaces it).
    by_name: HashMap<String, u64>,
    /// By id: the player's real address.
    by_id: HashMap<u64, IpAddr>,
    /// By loopback port: the player's real address.
    by_port: HashMap<u16, IpAddr>,
    next: u64,
}

static OPEN: std::sync::LazyLock<Mutex<Open>> = std::sync::LazyLock::new(Mutex::default);

fn open() -> std::sync::MutexGuard<'static, Open> {
    OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The real address of the tunnelled player behind loopback port `port`, if it's a tunnel's.
pub fn real_ip(port: u16) -> Option<IpAddr> {
    open().by_port.get(&port).copied()
}

/// The address a datagram from `src` stands for: a tunnel's player's real address when `src` is
/// a tunnel's loopback socket, else `src`'s own.
pub fn source_ip(src: SocketAddr) -> IpAddr {
    if src.ip().is_loopback() {
        if let Some(ip) = real_ip(src.port()) {
            return ip;
        }
    }
    src.ip()
}

/// Whether a new tunnel from `ip` may open now.
fn admits(o: &Open, ip: IpAddr) -> Result<(), &'static str> {
    if o.by_id.len() >= MAX_TUNNELS {
        return Err("the server's tunnels are all in use");
    }
    if o.by_id.values().filter(|i| **i == ip).count() >= MAX_PER_IP {
        return Err("too many tunnels from this address");
    }
    Ok(())
}

/// The player's address: Caddy's `X-Forwarded-For` (its last entry, the one Caddy added) when
/// the connection is from this machine, else the connection's own.
fn real_address(peer: SocketAddr, forwarded: Option<&str>) -> IpAddr {
    if peer.ip().is_loopback() {
        if let Some(ip) = forwarded.and_then(|f| f.rsplit(',').next()).and_then(|ip| ip.trim().parse().ok()) {
            return ip;
        }
    }
    peer.ip()
}

/// A token bucket of bytes.
struct Rate {
    at: Instant,
    allowance: f64,
}

impl Rate {
    fn new() -> Self {
        Self {
            at: Instant::now(),
            allowance: BYTES_PER_SEC * 2.0,
        }
    }

    fn take(&mut self, bytes: usize) -> bool {
        let now = Instant::now();
        self.allowance = (self.allowance + now.duration_since(self.at).as_secs_f64() * BYTES_PER_SEC).min(BYTES_PER_SEC * 4.0);
        self.at = now;
        #[allow(clippy::cast_precision_loss)]
        let bytes = bytes as f64;
        if self.allowance < bytes {
            return false;
        }
        self.allowance -= bytes;
        true
    }
}

/// Takes tunnels on `listen` (127.0.0.1: Caddy proxies `/tunnel` to it).
pub async fn serve(logger: Logger, listen: SocketAddr, services: Services) {
    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) => {
            warn!(logger, "Tunnel: can't listen on {listen} ({e}); players whose UDP is blocked can't play here");
            return;
        }
    };
    info!(logger, "Tunnel for players whose UDP is blocked on {listen} (behind {})", tunnel::PATH);
    loop {
        let Ok((stream, peer)) = listener.accept().await else { continue };
        let logger = logger.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(&logger, stream, peer, services).await {
                info!(logger, "Tunnel from {peer}: {e}");
            }
        });
    }
}

async fn handle(logger: &Logger, stream: tokio::net::TcpStream, peer: SocketAddr, services: Services) -> Result<(), String> {
    let mut forwarded: Option<String> = None;
    let ws = tokio_tungstenite::accept_hdr_async(stream, |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
        forwarded = req.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok()).map(str::to_string);
        Ok(resp)
    })
    .await
    .map_err(|e| format!("not a WebSocket ({e})"))?;
    let ip = real_address(peer, forwarded.as_deref());
    let (mut down, mut up) = ws.split();
    // The proof of the name, first.
    let first = tokio::time::timeout(AUTH_WITHIN, up.next()).await.map_err(|_| "no proof in time".to_string())?;
    let Some(Ok(Ws::Binary(first))) = first else { return Err("no proof".into()) };
    let name = match tunnel::parse(&first) {
        Some((tunnel::AUTH, probe)) => crate::nat_helper::tunnel_auth(probe).ok_or("the proof didn't hold")?,
        _ => return Err("no proof".into()),
    };
    // The name's socket from a tunnel that closed a moment ago, else a new one.
    let resumed = {
        let mut p = parked();
        p.retain(|_, (at, _)| at.elapsed() < RESUME_WITHIN);
        p.remove(&identity::name_key(&name)).map(|(_, s)| s)
    };
    let was_resumed = resumed.is_some();
    let socket = match resumed {
        Some(s) => s,
        None => std::sync::Arc::new(tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.map_err(|e| e.to_string())?),
    };
    let port = socket.local_addr().map_err(|e| e.to_string())?.port();
    let id = {
        let mut o = open();
        // A tunnel for this name already: the new one replaces it (a reconnect).
        if let Some(old) = o.by_name.get(&identity::name_key(&name)).copied() {
            o.by_id.remove(&old);
        }
        admits(&o, ip).map_err(str::to_string)?;
        o.next += 1;
        let id = o.next;
        o.by_name.insert(identity::name_key(&name), id);
        o.by_id.insert(id, ip);
        o.by_port.insert(port, ip);
        id
    };
    info!(
        logger,
        "Tunnel: {name} from {ip} (their UDP is blocked); on 127.0.0.1:{port}{}",
        if was_resumed { ", resumed" } else { "" }
    );
    crate::session_events::note(crate::session_events::Who::Name(name.clone()), "tunnel", serde_json::json!({ "open": true }));
    let mut rate = Rate::new();
    let mut ping = tokio::time::interval(PING_EVERY);
    let mut buf = vec![0u8; 2048];
    let result: Result<(), String> = loop {
        // Replaced by a newer tunnel for the name.
        if !open().by_id.contains_key(&id) {
            break Ok(());
        }
        tokio::select! {
            message = up.next() => match message {
                Some(Ok(Ws::Binary(data))) => {
                    let Some((service, datagram)) = tunnel::parse(&data) else { continue };
                    let Some(to) = services.port(service) else { continue };
                    if rate.take(datagram.len()) {
                        let _ = socket.send_to(datagram, ("127.0.0.1", to)).await;
                    }
                }
                Some(Ok(Ws::Close(_))) | None => break Ok(()),
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e.to_string()),
            },
            got = socket.recv_from(&mut buf) => {
                let Ok((n, from)) = got else { continue };
                let Some(service) = from.ip().is_loopback().then(|| services.service(from.port())).flatten() else { continue };
                if rate.take(n) {
                    if let Err(e) = down.send(Ws::Binary(tunnel::frame(service, &buf[..n]).into())).await {
                        break Err(e.to_string());
                    }
                }
            }
            _ = ping.tick() => {
                if let Err(e) = down.send(Ws::Ping(Vec::new().into())).await {
                    break Err(e.to_string());
                }
            }
        }
    };
    {
        let mut o = open();
        let replaced = !o.by_id.contains_key(&id);
        o.by_port.remove(&port);
        if o.by_id.remove(&id).is_some() && o.by_name.get(&identity::name_key(&name)) == Some(&id) {
            o.by_name.remove(&identity::name_key(&name));
        }
        // Kept a while for the name's next tunnel, unless a newer one already took over.
        if !replaced {
            let mut p = parked();
            if p.len() < MAX_TUNNELS * 4 {
                p.insert(identity::name_key(&name), (Instant::now(), std::sync::Arc::clone(&socket)));
            }
        }
    }
    info!(logger, "Tunnel: {name}'s closed");
    crate::session_events::note(crate::session_events::Who::Name(name), "tunnel", serde_json::json!({ "open": false }));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_address_is_caddys_only_from_this_machine() {
        let local: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let outside: SocketAddr = "198.51.100.4:5000".parse().unwrap();
        assert_eq!(real_address(local, Some("203.0.113.9")), "203.0.113.9".parse::<IpAddr>().unwrap());
        assert_eq!(
            real_address(local, Some("10.0.0.1, 203.0.113.9")),
            "203.0.113.9".parse::<IpAddr>().unwrap(),
            "Caddy's, the last"
        );
        assert_eq!(real_address(outside, Some("203.0.113.9")), outside.ip(), "never believed from outside");
        assert_eq!(real_address(local, None), local.ip());
    }

    #[test]
    fn tunnels_are_capped_per_address_and_in_all() {
        let mut o = Open::default();
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        for id in 0..MAX_PER_IP as u64 {
            assert!(admits(&o, ip).is_ok());
            o.by_id.insert(id, ip);
        }
        assert!(admits(&o, ip).is_err());
        for id in 100..(100 + MAX_TUNNELS as u64) {
            o.by_id.insert(id, IpAddr::from([198, 51, 100, (id % 250) as u8]));
        }
        assert!(admits(&o, "192.0.2.1".parse().unwrap()).is_err());
    }

    #[test]
    fn each_service_has_its_port_and_back() {
        let s = Services {
            login: 21126,
            secure: 21127,
            helper: 21128,
        };
        assert_eq!(s.port(tunnel::SECURE), Some(21127));
        assert_eq!(s.port(tunnel::AUTH), None);
        assert_eq!(s.service(21128), Some(tunnel::HELPER));
        assert_eq!(s.service(9999), None);
    }

    #[test]
    fn a_tunnels_rate_is_capped() {
        let mut r = Rate::new();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let burst = (BYTES_PER_SEC * 2.0) as usize;
        assert!(r.take(burst));
        assert!(!r.take(10_000), "the burst is spent");
    }

    /// A tunnel end to end: the proof is refused without a valid ticket, and the socket closes.
    #[tokio::test]
    async fn a_tunnel_without_proof_is_closed() {
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = listener.local_addr().unwrap();
        let services = Services { login: 1, secure: 2, helper: 3 };
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let r = handle(&logger, stream, peer, services).await;
            assert!(r.is_err());
        });
        let (mut ws, _) = tokio_tungstenite::client_async(format!("ws://{at}/tunnel"), tokio::net::TcpStream::connect(at).await.unwrap())
            .await
            .unwrap();
        ws.send(Ws::Binary(tunnel::frame(tunnel::AUTH, b"not a probe").into())).await.unwrap();
        // The server ends it.
        let next = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap();
        assert!(!matches!(next, Some(Ok(Ws::Binary(_)))));
    }

    /// T5: a tunnel end to end. Proven with a signed probe and its ticket, a datagram for the
    /// sign-in goes to the service's port from the tunnel's own socket, and the service's answer
    /// comes back down the tunnel marked as the sign-in's; the tunnel is registered by its loopback
    /// port with the player's real address (Caddy's), and gone when it closes.
    #[tokio::test]
    async fn a_proven_tunnel_carries_datagrams_both_ways() {
        crate::nat_helper::test_keys();
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        // The sign-in service: answers each datagram with "re:" and it.
        let service = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let login = service.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = service.recv_from(&mut buf).await {
                let mut answer = b"re:".to_vec();
                answer.extend_from_slice(&buf[..n]);
                let _ = service.send_to(&answer, from).await;
            }
        });
        let services = Services { login, secure: 1, helper: 2 };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = listener.local_addr().unwrap();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let _ = done_tx.send(handle(&logger, stream, peer, services).await);
        });
        // The proof: a probe for the name with its ticket for this session key, signed with it.
        let key = identity::Identity::generate();
        let name = "TunnelTester";
        let ticket = crate::nat_helper::ticket_for(name, Some(&key.public_key())).unwrap();
        let (nonce, cookie) = (7u32, [0u8; 16]);
        let mut probe = nat_proto::Message::Probe {
            flags: 0,
            nonce,
            mapping: None,
            name: name.into(),
            ticket,
            cookie,
            rtt_ms: None,
            reach: None,
        }
        .encode();
        nat_proto::add_key(&mut probe, &key.public_key(), &key.sign_bytes(&nat_proto::probe_message(nonce, &cookie, name)));
        let request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!("ws://{at}/tunnel")).unwrap();
        let mut request = request;
        request.headers_mut().insert("x-forwarded-for", "203.0.113.77".parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::client_async(request, tokio::net::TcpStream::connect(at).await.unwrap()).await.unwrap();
        ws.send(Ws::Binary(tunnel::frame(tunnel::AUTH, &probe).into())).await.unwrap();
        ws.send(Ws::Binary(tunnel::frame(tunnel::LOGIN, b"hello").into())).await.unwrap();
        let answer = loop {
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap() {
                Some(Ok(Ws::Binary(data))) => break data,
                Some(Ok(_)) => {}
                other => panic!("no answer: {other:?}"),
            }
        };
        assert_eq!(tunnel::parse(&answer), Some((tunnel::LOGIN, &b"re:hello"[..])));
        let real: IpAddr = "203.0.113.77".parse().unwrap();
        let port_of = || open().by_port.iter().find(|(_, ip)| **ip == real).map(|(p, _)| *p);
        let first_port = port_of().expect("kept by its loopback port, with the real address");
        ws.close(None).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), done_rx).await.unwrap().unwrap();
        assert!(ended.is_ok(), "{ended:?}");
        assert_eq!(port_of(), None, "gone when it closed");

        // Back within 30 s (a carrier dropped it): the same socket, so the same port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = listener.local_addr().unwrap();
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let _ = handle(&logger, stream, peer, services).await;
        });
        let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!("ws://{at}/tunnel")).unwrap();
        request.headers_mut().insert("x-forwarded-for", "203.0.113.77".parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::client_async(request, tokio::net::TcpStream::connect(at).await.unwrap()).await.unwrap();
        ws.send(Ws::Binary(tunnel::frame(tunnel::AUTH, &probe).into())).await.unwrap();
        ws.send(Ws::Binary(tunnel::frame(tunnel::LOGIN, b"again").into())).await.unwrap();
        let answer = loop {
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap() {
                Some(Ok(Ws::Binary(data))) => break data,
                Some(Ok(_)) => {}
                other => panic!("no answer: {other:?}"),
            }
        };
        assert_eq!(tunnel::parse(&answer), Some((tunnel::LOGIN, &b"re:again"[..])));
        assert_eq!(port_of(), Some(first_port), "resumed on the same port");
        ws.close(None).await.unwrap();
    }
}
