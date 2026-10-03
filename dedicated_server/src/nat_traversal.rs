//! Implements the `NatTraversalProtocolServer` for handling NAT traversal requests,
//! such as initiating probes to other clients.

use quazal::prudp::packet::PacketType;
use quazal::prudp::packet::QPacket;
use quazal::prudp::packet::StreamType;
use quazal::prudp::packet::VPort;
use quazal::prudp::ClientRegistry;
use quazal::rmc::basic::ToStream;
use quazal::rmc::types::StationURL;
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

/// Whether other players may be told to probe `address` for a client connected from
/// `observed`. Addresses that only ever mean the target's own machine or every machine
/// around it (loopback, unspecified, broadcast, multicast, link-local) only when they are
/// where the client connected from (a game on the server's own machine or link).
///
/// LAN addresses stay allowed: players on one network (or a VPN using private ranges)
/// reach each other on them, and the server can't tell which networks two players share.
fn probe_address_allowed(address: &str, observed: std::net::IpAddr) -> bool {
    let Ok(ip) = address.parse::<std::net::Ipv4Addr>() else { return false };
    let special = ip.is_loopback() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() || ip.is_link_local();
    !special || std::net::IpAddr::V4(ip) == observed.to_canonical()
}

/// Where other players' games are told to probe the caller, from the station the game
/// offered: as offered, or, when it carries a public address that isn't the caller's, at the
/// address the caller connected from. Such an address is most often a VPN's (Radmin VPN
/// gives out 26.x.x.x), where nobody else can reach the game; their station URLs are
/// corrected the same way ([`crate::game_session::only_reachable_addresses`]). `None` when
/// no probe may go there ([`probe_address_allowed`]).
fn probe_station(offered: &StationURL, observed: std::net::IpAddr, relay: Option<std::net::Ipv4Addr>) -> Option<StationURL> {
    crate::game_session::only_reachable_addresses(vec![offered.to_string()], observed, relay)
        .pop()
        .and_then(|url| url.parse::<StationURL>().ok())
        .filter(|station| probe_address_allowed(&station.address, observed))
}

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
        let Some(station) = probe_station(&request.url_station_to_probe, ci.address().ip(), crate::nat_helper::relay_ip()) else {
            warn!(logger, "User {user_id} asked for probes to an address not their own; refused");
            return Err(Error::AccessDenied);
        };
        if station.address != request.url_station_to_probe.address {
            info!(
                logger,
                "User {user_id} offered {} for probes, another network's address (a VPN?); using {} instead", request.url_station_to_probe.address, station.address
            );
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
                    url_station_to_probe: station.clone(),
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

#[cfg(test)]
mod tests {
    use super::probe_address_allowed;
    use super::probe_station;

    #[test]
    fn probes_only_to_addresses_that_can_mean_the_caller() {
        let public = "203.0.113.9".parse().unwrap();
        assert!(probe_address_allowed("203.0.113.9", public));
        assert!(probe_address_allowed("192.168.1.20", public), "a LAN address, for players on one network");
        for special in ["127.0.0.1", "0.0.0.0", "255.255.255.255", "224.0.0.1", "169.254.1.1", "example.com"] {
            assert!(!probe_address_allowed(special, public), "{special}");
        }
        assert!(probe_address_allowed("127.0.0.1", "127.0.0.1".parse().unwrap()), "a game on the server's own machine");
    }

    #[test]
    fn probes_go_where_the_caller_can_be_reached() {
        let observed = "203.0.113.9".parse().unwrap();
        let relay = Some("198.51.100.1".parse().unwrap());
        let probe = |url: &str| probe_station(&url.parse().unwrap(), observed, relay).map(|s| s.address);
        assert_eq!(probe("udp:/address=203.0.113.9;port=13000").as_deref(), Some("203.0.113.9"));
        assert_eq!(probe("udp:/address=192.168.1.70;port=13000").as_deref(), Some("192.168.1.70"), "a LAN address stays");
        assert_eq!(probe("udp:/address=198.51.100.1;port=40000").as_deref(), Some("198.51.100.1"), "the relay");
        assert_eq!(
            probe("udp:/address=26.70.109.161;port=13000").as_deref(),
            Some("203.0.113.9"),
            "a VPN's address: the caller's"
        );
        assert_eq!(probe("udp:/address=127.0.0.1;port=13000"), None);
        assert_eq!(probe("http:/address=203.0.113.9;port=13000"), None);
    }
}
