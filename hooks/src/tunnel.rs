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

/// The server's services and their forwarders: (the server's address, the forwarder's).
static MAP: RwLock<Vec<(u8, SocketAddrV4, SocketAddrV4)>> = RwLock::new(Vec::new());

/// Where a datagram the game sends to `dest` goes instead: its service's forwarder, when the
/// tunnel is on and `dest` is one of the server's services.
pub fn redirect(dest: SocketAddrV4) -> Option<SocketAddrV4> {
    MAP.read().ok()?.iter().find(|(_, server, _)| *server == dest).map(|(_, _, local)| *local)
}

/// The server's address a datagram from forwarder `src` stands for, if `src` is one.
pub fn from_forwarder(src: SocketAddrV4) -> Option<SocketAddrV4> {
    MAP.read().ok()?.iter().find(|(_, _, local)| *local == src).map(|(_, server, _)| *server)
}

/// Whether the game's traffic goes through the tunnel this run.
pub fn on() -> bool {
    MAP.read().is_ok_and(|m| !m.is_empty())
}

/// Starts the tunnel to `target` (the launcher's `networking.tunnel`).
pub fn start(target: hooks_config::Tunnel) {
    let Some(server) = (target.host.as_str(), 443).to_socket_addrs().ok().and_then(|mut a| {
        a.find_map(|a| match a {
            std::net::SocketAddr::V4(v4) => Some(*v4.ip()),
            std::net::SocketAddr::V6(_) => None,
        })
    }) else {
        warn!("Tunnel: {} didn't resolve; the game's traffic goes as usual", target.host);
        return;
    };
    let mut forwarders = Vec::new();
    for (service, port) in [(tunnel::LOGIN, target.login), (tunnel::SECURE, target.secure), (tunnel::HELPER, target.helper)] {
        let Ok(socket) = UdpSocket::bind("127.0.0.1:0") else {
            warn!("Tunnel: no local socket; the game's traffic goes as usual");
            return;
        };
        let Ok(std::net::SocketAddr::V4(local)) = socket.local_addr() else { return };
        forwarders.push((service, SocketAddrV4::new(server, port), local, Arc::new(socket)));
    }
    if let Ok(mut map) = MAP.write() {
        *map = forwarders.iter().map(|(service, server, local, _)| (*service, *server, *local)).collect();
    }
    info!("Tunnel: the game's UDP to {server} goes through https://{}{}", target.host, tunnel::PATH);
    let url = format!("wss://{}{}", target.host, tunnel::PATH);
    let _ = std::thread::Builder::new().name("fe-tunnel".into()).spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() else {
            warn!("Tunnel: couldn't start");
            return;
        };
        rt.block_on(run(url, forwarders));
    });
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
                if let Ok(mut g) = GAME.lock() {
                    g[usize::from(service)] = Some(from);
                }
                let _ = to_ws.blocking_send(tunnel::frame(service, &buf[..n]));
            }
        });
    }
    let mut wait = Duration::from_secs(1);
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
                let (mut down, mut up) = ws.split();
                if down.send(Ws::Binary(tunnel::frame(tunnel::AUTH, &proof).into())).await.is_err() {
                    continue;
                }
                loop {
                    tokio::select! {
                        out = from_game.recv() => {
                            let Some(out) = out else { return };
                            if down.send(Ws::Binary(out.into())).await.is_err() {
                                break;
                            }
                        }
                        message = up.next() => match message {
                            Some(Ok(Ws::Binary(data))) => {
                                let Some((service, datagram)) = tunnel::parse(&data) else { continue };
                                let to = GAME.lock().ok().and_then(|g| g.get(usize::from(service)).copied().flatten());
                                if let (Some(to), Some((.., socket))) = (to, forwarders.iter().find(|f| f.0 == service)) {
                                    let _ = socket.send_to(datagram, to);
                                }
                            }
                            Some(Ok(Ws::Close(_))) | None | Some(Err(_)) => break,
                            Some(Ok(_)) => {}
                        }
                    }
                }
                warn!("Tunnel: closed; opening it again");
            }
            Err(e) => warn!("Tunnel: couldn't open ({e}); trying again in {} s", wait.as_secs()),
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
