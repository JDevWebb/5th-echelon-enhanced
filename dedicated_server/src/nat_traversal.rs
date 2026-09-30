//! Implements the `NatTraversalProtocolServer` for handling NAT traversal requests,
//! such as initiating probes to other clients.

use quazal::prudp::packet::PacketType;
use quazal::prudp::packet::QPacket;
use quazal::prudp::packet::StreamType;
use quazal::prudp::packet::VPort;
use quazal::prudp::ClientRegistry;
use quazal::rmc::basic::ToStream;
use quazal::rmc::Error;
use quazal::rmc::Protocol;
use quazal::rmc::Request;
use quazal::ClientInfo;
use quazal::Context;
use slog::Logger;

use crate::login_required;
use crate::protocols::nat_traversal::nat_traversal_protocol::InitiateProbeRequest;
use crate::protocols::nat_traversal::nat_traversal_protocol::NatTraversalProtocolMethod;
use crate::protocols::nat_traversal::nat_traversal_protocol::NatTraversalProtocolServer;
use crate::protocols::nat_traversal::nat_traversal_protocol::NatTraversalProtocolServerTrait;
use crate::protocols::nat_traversal::nat_traversal_protocol::RequestProbeInitiationExtRequest;
use crate::protocols::nat_traversal::nat_traversal_protocol::RequestProbeInitiationExtResponse;
use crate::protocols::nat_traversal::nat_traversal_protocol::NAT_TRAVERSAL_PROTOCOL_ID;

/// Implementation of the `NatTraversalProtocolServerTrait` for NAT traversal operations.
struct NatTraversalProtocolServerImpl {
    storage: std::sync::Arc<crate::storage::Storage>,
}

/// The most players one probe request may reach.
const MAX_PROBE_TARGETS: usize = 8;

impl<T> NatTraversalProtocolServerTrait<T> for NatTraversalProtocolServerImpl {
    /// Handles the `RequestProbeInitiationExt` request.
    ///
    /// This method is responsible for initiating NAT probes to other clients.
    fn request_probe_initiation_ext(
        &self,
        logger: &Logger,
        ctx: &Context,
        ci: &mut ClientInfo<T>,
        request: RequestProbeInitiationExtRequest,
        client_registry: &ClientRegistry<T>,
        socket: &std::net::UdpSocket,
    ) -> Result<RequestProbeInitiationExtResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        info!(logger, "Probe initiation requested: {request:?}");
        // A probe makes another player's game send to an address: only to players in a
        // session with the caller, a few at a time, and only to the caller's own address.
        if request.url_target_list.len() > MAX_PROBE_TARGETS || !crate::rate_limit::game_requests().check(user_id) {
            return Err(Error::AccessDenied);
        }
        let own = std::iter::once(request.url_station_to_probe.to_string()).collect::<Vec<_>>();
        if crate::game_session::only_reachable_addresses(own.clone(), ci.address().ip(), crate::nat_helper::relay_ip()) != own {
            warn!(logger, "User {user_id} asked for probes to an address not their own; refused");
            return Err(Error::AccessDenied);
        }

        // Iterate over each target URL provided in the request.
        for url in request.url_target_list.iter() {
            // Extract the connection ID (RVCID) from the URL parameters.
            let Some(conn_id) = url.params.get("RVCID") else {
                warn!(logger, "{url} doesn't include RVCID");
                continue;
            };
            // Parse the connection ID into a u32.
            let Ok(conn_id) = conn_id.parse() else {
                warn!(logger, "{url} doesn't include valid RVCID");
                continue;
            };

            // Find the target client in the registry using the connection ID.
            let Some(target) = client_registry.client_by_connection_id(conn_id) else {
                warn!(logger, "No client found for RVCID {conn_id:?}");
                continue;
            };

            // Construct the payload for the InitiateProbe request.
            let payload = Request {
                protocol_id: NAT_TRAVERSAL_PROTOCOL_ID,
                call_id: rand::random(), // Generate a random call ID for the probe.
                method_id: NatTraversalProtocolMethod::InitiateProbe as u32,
                parameters: InitiateProbeRequest {
                    url_station_to_probe: request.url_station_to_probe.clone(),
                }
                .to_bytes(),
            }
            .to_bytes();

            // The commented-out section below shows an example of a hardcoded payload.
            // This is kept for reference but is not actively used.
            // let payload = Request {
            //     protocol_id: 14,
            //     call_id: rand::random(),
            //     method_id: 1,
            //     parameters: b"\x0c#\x1d\x00[\x1b\x00\x0015\x19\x00\xbd\xb3\x98\x02\x01\x00\x00\x01\x00\x00\x00".to_vec(),
            // }
            // .to_bytes();

            // The target may be the requesting client itself, which the caller
            // already holds mutably borrowed.
            let Ok(mut target) = target.try_borrow_mut() else {
                warn!(logger, "Not probing {url}: it is the requesting client");
                continue;
            };
            let addr = *target.address();
            let shares_session = target.user_id.is_some_and(|other| self.storage.share_session(user_id, other).unwrap_or(false));
            if !shares_session {
                warn!(logger, "Not probing {url}: not in a session with {user_id}");
                continue;
            }
            info!(logger, "Sending probe to {url} ({addr})\n{payload:x?}");

            // Create a QPacket for sending the probe.
            let qpacket = QPacket {
                source: VPort {
                    port: 1,
                    stream_type: StreamType::RVSec,
                },
                destination: VPort {
                    port: 15,
                    stream_type: StreamType::RVSec,
                },
                packet_type: PacketType::Data,
                payload,
                ..Default::default()
            };
            // Send the QPacket to the target client.
            if let Err(e) = quazal::prudp::send_request(logger, ctx, &addr, socket, qpacket, &mut target) {
                error!(logger, "Sending probe to {addr} failed: {e}");
            }
        }
        Ok(RequestProbeInitiationExtResponse)
    }
}

/// Creates a new boxed `NatTraversalProtocolServer` instance.
///
/// This function is typically used to register the NAT traversal protocol
/// with the server's protocol dispatcher.
pub fn new_protocol<T: 'static>(storage: std::sync::Arc<crate::storage::Storage>) -> Box<dyn Protocol<T>> {
    Box::new(NatTraversalProtocolServer::new(NatTraversalProtocolServerImpl { storage }))
}
