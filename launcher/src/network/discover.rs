//! Implements a server discovery mechanism using UDP broadcasts.
//!
//! This module provides functionality for finding a game server on the local
//! network by sending a broadcast packet and listening for a response.

use std::net::IpAddr;
use std::time::Duration;

use quazal::prudp::packet::PacketFlag;
use quazal::prudp::packet::PacketType;
use quazal::prudp::packet::QPacket;
use quazal::prudp::packet::StreamType;
use quazal::prudp::packet::VPort;
use tokio::net::UdpSocket;
use tokio::task::JoinSet;
use tracing::info;

use super::Error;

/// How long discovery listens for answers.
const LISTEN_FOR: Duration = Duration::from_secs(2);
/// The most servers discovery reports.
const MAX_FOUND: usize = 16;

/// Finds the servers on the local network: every one that answers within
/// two seconds, so a stray or hostile answer can't silently take the place
/// of the real server. Errors when none does.
pub async fn try_locate_server(adapter: Option<(&str, IpAddr)>, adapters: &[(&str, IpAddr)]) -> Result<Vec<IpAddr>, Error> {
    let from: Vec<IpAddr> = match adapter {
        Some((name, ip)) => {
            info!("Trying to locate servers through adapter {name} ({ip})");
            vec![ip]
        }
        None => {
            info!("Trying to locate servers on all interfaces");
            adapters.iter().map(|(_, ip)| *ip).collect()
        }
    };
    let mut set = JoinSet::new();
    for ip in from {
        set.spawn(async move { locate_from_ip(ip).await });
    }
    let mut found = Vec::new();
    let mut error = Error::ConnectionFailed;
    while let Some(Ok(r)) = set.join_next().await {
        match r {
            Ok(ips) => {
                for ip in ips {
                    if !found.contains(&ip) && found.len() < MAX_FOUND {
                        found.push(ip);
                    }
                }
            }
            Err(e) => error = e,
        }
    }
    if found.is_empty() {
        return Err(error);
    }
    found.sort();
    Ok(found)
}

/// Broadcasts a discovery packet from `ip` and collects who answers.
async fn locate_from_ip(ip: IpAddr) -> Result<Vec<IpAddr>, Error> {
    let ctx = quazal::Context::splinter_cell_blacklist();
    let socket = UdpSocket::bind(format!("{}:0", ip)).await?;
    socket.set_broadcast(true)?;

    // Create a Quazal SYN packet to initiate discovery.
    let session_id = rand::random();
    let syn_pkt = QPacket {
        source: VPort {
            port: 15,
            stream_type: StreamType::RVSec,
        },
        destination: VPort {
            port: 1,
            stream_type: StreamType::RVSec,
        },
        packet_type: PacketType::Syn,
        flags: PacketFlag::NeedAck.into(),
        conn_signature: Some(0),
        session_id,
        ..Default::default()
    };
    let buf = syn_pkt.to_bytes(&ctx);

    // Broadcast the packet to the local network.
    socket.send_to(&buf, "255.255.255.255:21126").await?;
    let mut buf = vec![0u8; 4096];
    let mut found = Vec::new();
    let deadline = tokio::time::Instant::now() + LISTEN_FOR;
    while let Ok(received) = tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await {
        let Ok((n, peer)) = received else { continue };
        // Only SYN-ACKs from the game port.
        if peer.port() != setup::QUAZAL_PORT {
            continue;
        }
        let Ok((syn_ack_pkt, _size)) = QPacket::from_bytes(&ctx, &buf[..n]) else { continue };
        if syn_ack_pkt.packet_type == PacketType::Syn && syn_ack_pkt.flags.contains(PacketFlag::Ack) && !found.contains(&peer.ip()) && found.len() < MAX_FOUND {
            found.push(peer.ip());
        }
    }
    Ok(found)
}
