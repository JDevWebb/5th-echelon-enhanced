//! Checks the server's NAT helper (UDP 21128-21129), which lets players
//! join each other over the internet without a VPN.

use std::net::SocketAddr;
use std::time::Duration;

use nat_proto::Message;
use tokio::net::UdpSocket;

use super::Error;

/// What the NAT helper saw of this PC.
pub struct NatCheck {
    /// This PC's public address, as the server sees it.
    pub observed: SocketAddr,
    /// Whether the router gives each destination its own port (symmetric
    /// NAT): direct connections then fail, and the relay is used.
    pub symmetric: Option<bool>,
}

async fn probe(socket: &UdpSocket, to: SocketAddr, second: bool) -> Result<SocketAddr, Error> {
    let flags = if second { nat_proto::probe_flags::SECOND_PORT } else { 0 };
    let probe = Message::Probe {
        flags,
        nonce: rand::random::<u32>() | 1,
        mapping: None,
        name: String::new(),
        ticket: [0; 16],
        cookie: [0; 16],
        rtt_ms: None,
    }
    .encode();
    let mut buf = [0u8; 256];
    for _ in 0..4 {
        socket.send_to(&probe, to).await?;
        if let Ok(Ok((n, from))) = tokio::time::timeout(Duration::from_millis(700), socket.recv_from(&mut buf)).await {
            if from == to {
                if let Some(Message::ProbeReply { observed, .. }) = Message::decode(&buf[..n]) {
                    return Ok(SocketAddr::V4(observed));
                }
            }
        }
    }
    Err(Error::TimedOut)
}

/// Probes the helper on `server`, from one socket on both ports.
pub async fn test_nat_helper(server: &str, port: u16) -> Result<NatCheck, Error> {
    let main = tokio::net::lookup_host((server, port)).await?.find(SocketAddr::is_ipv4).ok_or(Error::ConnectionFailed)?;
    let mut second = main;
    second.set_port(port.wrapping_add(1));
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let observed = probe(&socket, main, false).await?;
    let symmetric = probe(&socket, second, true).await.ok().map(|o| o.port() != observed.port());
    Ok(NatCheck { observed, symmetric })
}

/// One probe to the helper on `server`: answered or not.
pub async fn probe_once(server: &str, port: u16) -> Result<SocketAddr, Error> {
    let to = tokio::net::lookup_host((server, port)).await?.find(SocketAddr::is_ipv4).ok_or(Error::ConnectionFailed)?;
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    probe(&socket, to, false).await
}
