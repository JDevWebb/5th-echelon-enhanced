//! Implements logic for testing Quazal authentication and P2P connectivity.
//!
//! This module provides functions to test the full Quazal login flow and to
//! verify P2P connectivity with the server.

use std::time::Duration;

use quazal::prudp::packet::QPacket;
use quazal::Context;
use server_api::misc::misc_client::MiscClient;
use server_api::misc::TestP2pRequest;
use server_api::users::users_client::UsersClient;
use server_api::users::LoginRequest;
use tokio::net::UdpSocket;

use super::Error;
use super::QUAZAL_DEFAULT_LOCAL_PORT;

/// Tests the Quazal login process against a server.
///
/// This function simulates a full Quazal login, including the SYN/ACK handshake,
/// connection setup, and RMC login call. It will time out after 5 seconds.
pub async fn test_quazal_login(server: &str, port: u16, username: &str, password: &str) -> Result<(), Error> {
    let ctx = quazal::Context::splinter_cell_blacklist();

    let Ok(res) = tokio::time::timeout(Duration::from_secs(5), async {
        let socket = quazal_setup(server, port).await?;
        let mut quazal = Quazal {
            ctx,
            socket,
            session_id: rand::random(),
            signature: 0,
            sequence: 0,
        };
        quazal.syn().await?;
        quazal.connect().await?;
        quazal.login(username, password).await
    })
    .await
    else {
        return Err(Error::TimedOut);
    };

    res
}

/// A struct to manage the state of a Quazal connection.
struct Quazal {
    ctx: Context,
    socket: UdpSocket,
    session_id: u8,
    signature: u32,
    sequence: u16,
}

/// Sets up a UDP socket for Quazal communication.
async fn quazal_setup(server: &str, port: u16) -> std::io::Result<UdpSocket> {
    let socket = tokio::net::UdpSocket::bind(format!("0.0.0.0:{QUAZAL_DEFAULT_LOCAL_PORT}")).await?;
    socket.connect(format!("{server}:{port}")).await?;
    Ok(socket)
}

impl Quazal {
    /// Sends a Quazal packet, automatically setting the session ID, signature, and sequence number.
    async fn send(&mut self, mut packet: QPacket) -> Result<(), Error> {
        packet.session_id = self.session_id;
        packet.signature = self.signature;
        packet.sequence = self.sequence;

        self.sequence += 1;

        self.socket.send(&packet.to_bytes(&self.ctx)).await?;
        Ok(())
    }

    /// Sends an ACK packet.
    async fn send_ack(&mut self, mut packet: QPacket) -> Result<(), Error> {
        packet.session_id = self.session_id;
        packet.signature = self.signature;
        packet.sequence = 0;

        self.socket.send(&packet.to_bytes(&self.ctx)).await?;
        Ok(())
    }

    /// Performs the SYN/ACK handshake to establish a connection.
    async fn syn(&mut self) -> Result<u32, Error> {
        use quazal::prudp::packet::PacketFlag;
        use quazal::prudp::packet::PacketType;
        use quazal::prudp::packet::QPacket;
        use quazal::prudp::packet::StreamType;
        use quazal::prudp::packet::VPort;

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
            ..Default::default()
        };
        self.send(syn_pkt).await?;

        let mut buf = vec![0u8; 4096];

        let n = self.socket.recv(&mut buf).await?;
        let (syn_ack_pkt, _size) = QPacket::from_bytes(&self.ctx, &buf[..n]).map_err(|e| std::io::Error::other(e.to_string()))?;

        if syn_ack_pkt.packet_type != PacketType::Syn || !syn_ack_pkt.flags.contains(PacketFlag::Ack) {
            return Err(Error::IO(std::io::Error::other("invalid syn ack")));
        }

        if syn_ack_pkt.conn_signature.is_none() {
            return Err(Error::IO(std::io::Error::other("missing connection signature")));
        }

        self.signature = syn_ack_pkt.conn_signature.unwrap();
        Ok(self.signature)
    }

    /// Sends a CONNECT packet to finalize the connection.
    async fn connect(&mut self) -> Result<(), Error> {
        use quazal::prudp::packet::PacketFlag;
        use quazal::prudp::packet::PacketType;
        use quazal::prudp::packet::QPacket;
        use quazal::prudp::packet::StreamType;
        use quazal::prudp::packet::VPort;

        let connect_pkt = QPacket {
            source: VPort {
                port: 15,
                stream_type: StreamType::RVSec,
            },
            destination: VPort {
                port: 1,
                stream_type: StreamType::RVSec,
            },
            packet_type: PacketType::Connect,
            flags: PacketFlag::NeedAck.into(),
            conn_signature: Some(rand::random()),
            ..Default::default()
        };
        self.send(connect_pkt).await?;

        let mut buf = vec![0u8; 4096];

        let n = self.socket.recv(&mut buf).await?;
        let (conn_ack_pkt, _size) = QPacket::from_bytes(&self.ctx, &buf[..n])?;

        if conn_ack_pkt.packet_type != PacketType::Connect || !conn_ack_pkt.flags.contains(PacketFlag::Ack) {
            return Err(Error::IO(std::io::Error::other("invalid connect ack")));
        }
        Ok(())
    }

    /// Performs the RMC login call.
    async fn login(&mut self, username: &str, password: &str) -> Result<(), Error> {
        use quazal::prudp::packet::PacketFlag;
        use quazal::prudp::packet::PacketType;
        use quazal::prudp::packet::QPacket;
        use quazal::prudp::packet::StreamType;
        use quazal::prudp::packet::VPort;
        use quazal::rmc::basic::ToStream as _;
        use quazal::rmc::types::Any;
        use quazal::rmc::Packet;
        use quazal::rmc::Request;
        use sc_bl_protocols::authentication_foundation::ticket_granting_protocol::LoginExRequest;
        use sc_bl_protocols::authentication_foundation::ticket_granting_protocol::TicketGrantingProtocolMethod;
        use sc_bl_protocols::authentication_foundation::ticket_granting_protocol::TICKET_GRANTING_PROTOCOL_ID;
        use sc_bl_protocols::ubi_authentication::types::UbiAuthenticationLoginCustomData;

        // Construct the RMC login request.
        let parameters = LoginExRequest {
            str_user_name: username.to_string(),
            o_extra_data: Any::new(
                "UbiAuthenticationLoginCustomData".to_string(),
                UbiAuthenticationLoginCustomData {
                    data: quazal::rmc::types::Data,
                    user_name: username.to_string(),
                    online_key: "AAAA-BBBB-CCCC".to_string(),
                    password: password.to_string(),
                }
                .to_bytes(),
            ),
        }
        .to_bytes();

        let login_rmc_pkt = Packet::Request(Request {
            protocol_id: TICKET_GRANTING_PROTOCOL_ID,
            call_id: 10,
            method_id: TicketGrantingProtocolMethod::LoginEx as u32,
            parameters,
        });

        // Wrap the RMC packet in a PRUDP data packet.
        let login_prudp_pkt = QPacket {
            source: VPort {
                port: 15,
                stream_type: StreamType::RVSec,
            },
            destination: VPort {
                port: 1,
                stream_type: StreamType::RVSec,
            },
            packet_type: PacketType::Data,
            flags: PacketFlag::NeedAck | PacketFlag::Reliable,
            fragment_id: Some(0),
            payload: login_rmc_pkt.to_bytes(),
            ..Default::default()
        };
        self.send(login_prudp_pkt).await?;

        // Wait for the ACK to the login request.
        let mut buf = vec![0u8; 4096];
        let n = self.socket.recv(&mut buf).await?;
        let (data_ack_pkt, _size) = QPacket::from_bytes(&self.ctx, &buf[..n])?;

        if data_ack_pkt.packet_type != PacketType::Data || !data_ack_pkt.flags.contains(PacketFlag::Ack) {
            return Err(Error::IO(std::io::Error::other("invalid data ack")));
        }

        // Wait for the login response.
        let mut buf = vec![0u8; 4096];
        let n = self.socket.recv(&mut buf).await?;
        let (login_resp, _size) = QPacket::from_bytes(&self.ctx, &buf[..n])?;

        if login_resp.packet_type != PacketType::Data || !login_resp.flags.contains(PacketFlag::HasSize) {
            return Err(Error::IO(std::io::Error::other("invalid response")));
        }

        // Acknowledge the login response.
        self.send_ack(QPacket {
            source: VPort {
                port: 15,
                stream_type: StreamType::RVSec,
            },
            destination: VPort {
                port: 1,
                stream_type: StreamType::RVSec,
            },
            packet_type: PacketType::Data,
            flags: PacketFlag::Ack.into(),
            fragment_id: Some(0),
            ..Default::default()
        })
        .await?;

        // Disconnect from the server.
        self.send(QPacket {
            source: VPort {
                port: 15,
                stream_type: StreamType::RVSec,
            },
            destination: VPort {
                port: 1,
                stream_type: StreamType::RVSec,
            },
            packet_type: PacketType::Disconnect,
            fragment_id: Some(0),
            ..Default::default()
        })
        .await?;

        let n = self.socket.recv(&mut buf).await?;
        let (disco_ack_pkt, _size) = QPacket::from_bytes(&self.ctx, &buf[..n])?;
        if disco_ack_pkt.packet_type != PacketType::Disconnect || !disco_ack_pkt.flags.contains(PacketFlag::Ack) {
            return Err(Error::IO(std::io::Error::other("invalid disco ack")));
        }

        // Parse the RMC response and check for errors.
        let resp = Packet::from_bytes(&login_resp.payload)?;
        if let Packet::Response(resp) = resp {
            // A code this side doesn't know is still a refusal, not a crash.
            resp.result.map_err(|e| match quazal::rmc::Error::from_error_code(e.error_code) {
                Ok(e) => Error::Rmc(e),
                Err(code) => Error::ServerFailure(format!("the server refused the login (error {code:#x})")),
            })?;
        } else {
            return Err(Error::IO(std::io::Error::other("invalid rmc response")));
        }

        Ok(())
    }
}

/// Tests P2P connectivity with the server.
///
/// This function logs in to the API server, initiates a P2P test, and then
/// listens for a UDP packet from the server containing a challenge. It then
/// sends the challenge back to the server to confirm connectivity.
pub async fn test_p2p(api_url: String, username: &str, password: &str) -> Result<(), Error> {
    // Log in to the API server to get an authentication token.
    let Ok(channel) = super::endpoint(&api_url)?.connect().await else {
        return Err(Error::ConnectionFailed);
    };
    let mut client = UsersClient::new(channel);

    let resp = match client
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            client: super::CLIENT.into(),
        })
        .await
    {
        Ok(resp) => resp,
        Err(status) => {
            if matches!(status.code(), tonic::Code::Unauthenticated) {
                return Err(Error::InvalidPassword);
            } else {
                return Err(Error::SendingRequestFailed);
            }
        }
    };

    let resp = resp.into_inner();
    if !resp.error.is_empty() {
        return Err(Error::ServerFailure(resp.error));
    }
    let token: tonic::metadata::MetadataValue<tonic::metadata::Ascii> = resp.token.parse().map_err(|_| Error::ServerFailure("the server's sign-in token isn't readable".into()))?;
    let Ok(channel) = super::endpoint(&api_url)?.connect().await else {
        return Err(Error::ConnectionFailed);
    };
    let mut client = MiscClient::with_interceptor(channel, move |mut req: tonic::Request<_>| {
        req.metadata_mut().insert("authorization", token.clone());
        Ok(req)
    });

    // Listen before asking the server to send. Both halves run in this future, not spawned
    // tasks: when the test times out, the socket is closed with it, and port 13000 is free for
    // the next try (spawned, it lingered and the retry got "address in use").
    let socket = UdpSocket::bind("0.0.0.0:13000").await?;
    let listen = async {
        let mut buf = vec![0u8; 4096];
        let (n, addr) = socket.recv_from(&mut buf).await?;
        let Some(challenge) = buf[..n].strip_prefix(b"P2P Test - ") else {
            return Err(Error::IO(std::io::Error::other("invalid challenge")));
        };
        socket.send_to(challenge, addr).await?;
        Ok::<_, Error>(challenge.to_vec())
    };
    let ask = async {
        let challenge: [u8; 32] = rand::random();
        let resp = client.test_p2p(TestP2pRequest { challenge: challenge.to_vec() }).await?;
        Ok::<_, Error>(resp.into_inner().challenge)
    };
    // The server's answer ends the test either way: an error from it (no reply in time) stops
    // the listening too.
    let (udp_challenge, rpc_challenge) = tokio::try_join!(listen, ask).map_err(|e| Error::P2P(Box::new(e)))?;
    if udp_challenge != rpc_challenge {
        return Err(Error::ChallengeMismatch);
    }

    Ok(())
}
