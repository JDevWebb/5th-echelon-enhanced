//! A client-side PRUDP connection: what the game's network layer does, enough
//! to make RMC calls to the auth and secure servers and receive the server's
//! own requests (notifications).

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;
use std::time::Instant;

use eyre::bail;
use eyre::eyre;
use eyre::Result;
use quazal::prudp::packet::PacketFlag;
use quazal::prudp::packet::PacketType;
use quazal::prudp::packet::QPacket;
use quazal::prudp::packet::StreamType;
use quazal::prudp::packet::VPort;
use quazal::rmc::Packet;
use quazal::rmc::Request;
use quazal::rmc::Response;
use quazal::Context;
use tokio::net::UdpSocket;

/// How long a call waits for its response.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);

const LOCAL: VPort = VPort { port: 15, stream_type: StreamType::RVSec };
const REMOTE: VPort = VPort { port: 1, stream_type: StreamType::RVSec };

/// An RMC error answer (the server's error code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RmcError(pub u32);

impl std::fmt::Display for RmcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match quazal::rmc::Error::from_error_code(self.0) {
            Ok(e) => write!(f, "RMC error {e:?} ({:#x})", self.0),
            Err(_) => write!(f, "RMC error {:#x}", self.0),
        }
    }
}

impl std::error::Error for RmcError {}

pub struct Conn {
    ctx: Context,
    socket: UdpSocket,
    session_id: u8,
    /// The server's connection signature (sent in every packet of ours).
    server_signature: u32,
    sequence: u16,
    next_call_id: u32,
    /// Fragments of a message in progress, in arrival order.
    fragments: Vec<u8>,
    responses: HashMap<u32, Response>,
    requests: VecDeque<Request>,
    /// Packets the server sent us (for asserting on duplicates, acks ...).
    pub received: usize,
    /// Simulated loss: this many of the server's own requests (pushes) are
    /// dropped without an acknowledgement, as if they never arrived.
    pub drop_requests: u32,
}

/// Behave like a game far from the server: its first resend comes before the
/// server's answer, so the SYN and CONNECT go twice, and it takes the
/// signature from the last SYN answer to arrive.
pub static SLOW_HANDSHAKE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Conn {
    /// Opens a connection (SYN, CONNECT). `connect_payload` is the ticket for
    /// the secure server, empty for the auth server. Returns the payload of
    /// the CONNECT acknowledgement.
    pub async fn connect(server: SocketAddr, connect_payload: Vec<u8>) -> Result<(Conn, Vec<u8>)> {
        Self::connect_from("0.0.0.0:0".parse()?, server, connect_payload).await
    }

    /// [`Self::connect`] from a socket bound to `bind`.
    pub async fn connect_from(bind: SocketAddr, server: SocketAddr, connect_payload: Vec<u8>) -> Result<(Conn, Vec<u8>)> {
        let socket = UdpSocket::bind(bind).await?;
        socket.connect(server).await?;
        let mut c = Conn {
            ctx: Context::splinter_cell_blacklist(),
            socket,
            session_id: rand::random(),
            server_signature: 0,
            sequence: 0,
            next_call_id: 1,
            fragments: vec![],
            responses: HashMap::new(),
            requests: VecDeque::new(),
            received: 0,
            drop_requests: 0,
        };
        let slow = SLOW_HANDSHAKE.load(std::sync::atomic::Ordering::Relaxed);
        let is_syn_ack = |p: &QPacket| p.packet_type == PacketType::Syn && p.flags.contains(PacketFlag::Ack);
        c.send_maybe_twice(
            QPacket {
                packet_type: PacketType::Syn,
                flags: PacketFlag::NeedAck.into(),
                conn_signature: Some(0),
                ..Default::default()
            },
            slow,
        )
        .await?;
        let syn_ack = c.expect(is_syn_ack).await?;
        c.server_signature = syn_ack.conn_signature.ok_or_else(|| eyre!("SYN ack without a connection signature"))?;

        c.send_maybe_twice(
            QPacket {
                packet_type: PacketType::Connect,
                flags: PacketFlag::NeedAck.into(),
                conn_signature: Some(rand::random()),
                payload: connect_payload,
                ..Default::default()
            },
            slow,
        )
        .await?;
        let mut connect_ack = None;
        let mut second_syn_ack = !slow;
        while connect_ack.is_none() || !second_syn_ack {
            let p = c.expect(|p| is_syn_ack(p) || (p.packet_type == PacketType::Connect && p.flags.contains(PacketFlag::Ack))).await?;
            if is_syn_ack(&p) {
                // The answer to the repeated SYN: like the game, use its signature from now on.
                c.server_signature = p.conn_signature.ok_or_else(|| eyre!("SYN ack without a connection signature"))?;
                second_syn_ack = true;
            } else if connect_ack.is_none() {
                connect_ack = Some(p);
            }
        }
        Ok((c, connect_ack.map(|p| p.payload).unwrap_or_default()))
    }

    /// Sends a handshake packet, twice (the same packet, as a resend) if `twice`.
    async fn send_maybe_twice(&mut self, mut p: QPacket, twice: bool) -> Result<()> {
        p.source = LOCAL;
        p.destination = REMOTE;
        p.session_id = self.session_id;
        p.signature = self.server_signature;
        p.sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        let bytes = p.to_bytes(&self.ctx);
        self.socket.send(&bytes).await?;
        if twice {
            self.socket.send(&bytes).await?;
        }
        Ok(())
    }

    async fn send(&mut self, mut p: QPacket) -> Result<()> {
        p.source = LOCAL;
        p.destination = REMOTE;
        p.session_id = self.session_id;
        p.signature = self.server_signature;
        p.sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        self.socket.send(&p.to_bytes(&self.ctx)).await?;
        Ok(())
    }

    /// Receives one packet, or `None` after `wait`.
    async fn recv(&mut self, wait: Duration) -> Result<Option<QPacket>> {
        let mut buf = vec![0u8; 4096];
        match tokio::time::timeout(wait, self.socket.recv(&mut buf)).await {
            Err(_) => Ok(None),
            Ok(Err(e)) => Err(e.into()),
            Ok(Ok(n)) => {
                self.received += 1;
                let (p, _) = QPacket::from_bytes(&self.ctx, &buf[..n]).map_err(|e| eyre!("bad packet from the server: {e}"))?;
                Ok(Some(p))
            }
        }
    }

    /// Waits for a handshake packet matching `want`.
    async fn expect(&mut self, want: impl Fn(&QPacket) -> bool) -> Result<QPacket> {
        let deadline = Instant::now() + CALL_TIMEOUT;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Some(p) = self.recv(left).await? {
                if want(&p) {
                    return Ok(p);
                }
            }
        }
        bail!("the server didn't answer the handshake")
    }

    /// Handles incoming packets for up to `wait`: acknowledges reliable data,
    /// reassembles fragments, and files RMC responses and requests.
    async fn pump(&mut self, wait: Duration) -> Result<()> {
        let Some(p) = self.recv(wait).await? else { return Ok(()) };
        if p.packet_type != PacketType::Data || p.flags.contains(PacketFlag::Ack) {
            return Ok(());
        }
        // Simulated loss of a server push: no acknowledgement, not delivered.
        if self.drop_requests > 0 && self.fragments.is_empty() && p.fragment_id.unwrap_or(0) == 0 {
            if let Ok(Packet::Request(_)) = Packet::from_bytes(&p.payload) {
                self.drop_requests -= 1;
                return Ok(());
            }
        }
        if p.flags.contains(PacketFlag::NeedAck) {
            let ack = QPacket {
                source: LOCAL,
                destination: REMOTE,
                packet_type: PacketType::Data,
                flags: PacketFlag::Ack | PacketFlag::HasSize,
                session_id: self.session_id,
                signature: self.server_signature,
                sequence: p.sequence,
                fragment_id: p.fragment_id,
                ..Default::default()
            };
            self.socket.send(&ack.to_bytes(&self.ctx)).await?;
        }
        self.fragments.extend_from_slice(&p.payload);
        if p.fragment_id.unwrap_or(0) != 0 {
            return Ok(()); // more to come
        }
        let payload = std::mem::take(&mut self.fragments);
        match Packet::from_bytes(&payload).map_err(|e| eyre!("bad RMC message: {e:?}"))? {
            Packet::Response(r) => {
                let call_id = match &r.result {
                    Ok(d) => d.call_id,
                    Err(e) => e.call_id,
                };
                self.responses.insert(call_id, r);
            }
            Packet::Request(r) => self.requests.push_back(r),
        }
        Ok(())
    }

    /// Makes an RMC call and returns the response's data, or the server's
    /// error code as an `RmcError`.
    pub async fn call(&mut self, protocol_id: u16, method_id: u32, parameters: Vec<u8>) -> Result<Vec<u8>> {
        self.call_opts(protocol_id, method_id, parameters, false).await
    }

    /// Like `call`, but sends the request packet twice, as a client does when
    /// it retransmits after a lost acknowledgement (same sequence number).
    pub async fn call_twice(&mut self, protocol_id: u16, method_id: u32, parameters: Vec<u8>) -> Result<Vec<u8>> {
        self.call_opts(protocol_id, method_id, parameters, true).await
    }

    async fn call_opts(&mut self, protocol_id: u16, method_id: u32, parameters: Vec<u8>, twice: bool) -> Result<Vec<u8>> {
        let call_id = self.next_call_id;
        self.next_call_id += 1;
        let request = Packet::Request(Request { protocol_id, call_id, method_id, parameters });
        let packet = QPacket {
            packet_type: PacketType::Data,
            flags: PacketFlag::NeedAck | PacketFlag::Reliable | PacketFlag::HasSize,
            fragment_id: Some(0),
            payload: request.to_bytes(),
            ..Default::default()
        };
        self.send(packet.clone()).await?;
        if twice {
            self.sequence = self.sequence.wrapping_sub(1); // the same packet again
            self.send(packet).await?;
        }
        let deadline = Instant::now() + CALL_TIMEOUT;
        loop {
            if let Some(r) = self.responses.remove(&call_id) {
                return match r.result {
                    Ok(d) => Ok(d.data),
                    Err(e) => Err(RmcError(e.error_code).into()),
                };
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                bail!("no answer to call {protocol_id}.{method_id} within {CALL_TIMEOUT:?}");
            };
            self.pump(left.min(Duration::from_millis(100))).await?;
        }
    }

    /// Waits up to `wait` for a request the server sends us (e.g. a
    /// notification), matching `protocol_id`/`method_id`.
    pub async fn wait_request(&mut self, protocol_id: u16, method_id: u32, wait: Duration) -> Result<Option<Request>> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(i) = self.requests.iter().position(|r| r.protocol_id == protocol_id && r.method_id == method_id) {
                return Ok(self.requests.remove(i));
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { return Ok(None) };
            self.pump(left.min(Duration::from_millis(100))).await?;
        }
    }

    /// Whether the server closes this connection (sends a Disconnect) within `wait`.
    pub async fn closed_by_server(&mut self, wait: Duration) -> Result<bool> {
        let deadline = Instant::now() + wait;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Some(p) = self.recv(left).await? {
                if p.packet_type == PacketType::Disconnect && !p.flags.contains(PacketFlag::Ack) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Says goodbye (the server then cleans up this connection's state).
    pub async fn disconnect(mut self) -> Result<()> {
        self.send(QPacket {
            packet_type: PacketType::Disconnect,
            flags: PacketFlag::NeedAck.into(),
            ..Default::default()
        })
        .await?;
        let _ = self.expect(|p| p.packet_type == PacketType::Disconnect).await;
        Ok(())
    }
}
