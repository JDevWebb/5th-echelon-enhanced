//! Implements the `GameSessionProtocolServer` for managing game sessions.

use std::sync::Arc;

use quazal::prudp::ClientRegistry;
use quazal::rmc::basic::ToStream as _;
use quazal::rmc::types::DateTime;
use quazal::rmc::Error;
use quazal::rmc::Protocol;
use quazal::ClientInfo;
use quazal::Context;
use sc_bl_protocols::game_session_service::game_session_protocol::JoinSessionRequest;
use sc_bl_protocols::game_session_service::game_session_protocol::JoinSessionResponse;
use sc_bl_protocols::game_session_service::game_session_protocol::RemoveParticipantsRequest;
use sc_bl_protocols::game_session_service::game_session_protocol::RemoveParticipantsResponse;
use slog::Logger;

use crate::config::DebugConfig;
use crate::login_required;
use crate::protocols::game_session_service::game_session_protocol::AbandonSessionRequest;
use crate::protocols::game_session_service::game_session_protocol::AbandonSessionResponse;
use crate::protocols::game_session_service::game_session_protocol::AcceptInvitationRequest;
use crate::protocols::game_session_service::game_session_protocol::AcceptInvitationResponse;
use crate::protocols::game_session_service::game_session_protocol::AddParticipantsRequest;
use crate::protocols::game_session_service::game_session_protocol::AddParticipantsResponse;
use crate::protocols::game_session_service::game_session_protocol::CancelInvitationRequest;
use crate::protocols::game_session_service::game_session_protocol::CancelInvitationResponse;
use crate::protocols::game_session_service::game_session_protocol::CreateSessionRequest;
use crate::protocols::game_session_service::game_session_protocol::CreateSessionResponse;
use crate::protocols::game_session_service::game_session_protocol::DeclineInvitationRequest;
use crate::protocols::game_session_service::game_session_protocol::DeclineInvitationResponse;
use crate::protocols::game_session_service::game_session_protocol::DeleteSessionRequest;
use crate::protocols::game_session_service::game_session_protocol::DeleteSessionResponse;
use crate::protocols::game_session_service::game_session_protocol::GameSessionProtocolServer;
use crate::protocols::game_session_service::game_session_protocol::GameSessionProtocolServerTrait;
use crate::protocols::game_session_service::game_session_protocol::GetInvitationReceivedCountRequest;
use crate::protocols::game_session_service::game_session_protocol::GetInvitationReceivedCountResponse;
use crate::protocols::game_session_service::game_session_protocol::GetInvitationSentCountRequest;
use crate::protocols::game_session_service::game_session_protocol::GetInvitationSentCountResponse;
use crate::protocols::game_session_service::game_session_protocol::GetInvitationsReceivedRequest;
use crate::protocols::game_session_service::game_session_protocol::GetInvitationsReceivedResponse;
use crate::protocols::game_session_service::game_session_protocol::GetInvitationsSentRequest;
use crate::protocols::game_session_service::game_session_protocol::GetInvitationsSentResponse;
use crate::protocols::game_session_service::game_session_protocol::LeaveSessionRequest;
use crate::protocols::game_session_service::game_session_protocol::LeaveSessionResponse;
use crate::protocols::game_session_service::game_session_protocol::RegisterUrLsRequest;
use crate::protocols::game_session_service::game_session_protocol::RegisterUrLsResponse;
use crate::protocols::game_session_service::game_session_protocol::ReportUnsuccessfulJoinSessionsRequest;
use crate::protocols::game_session_service::game_session_protocol::ReportUnsuccessfulJoinSessionsResponse;
use crate::protocols::game_session_service::game_session_protocol::SearchSessionsRequest;
use crate::protocols::game_session_service::game_session_protocol::SearchSessionsResponse;
use crate::protocols::game_session_service::game_session_protocol::SearchSessionsWithParticipantsRequest;
use crate::protocols::game_session_service::game_session_protocol::SearchSessionsWithParticipantsResponse;
use crate::protocols::game_session_service::game_session_protocol::SendInvitationRequest;
use crate::protocols::game_session_service::game_session_protocol::SendInvitationResponse;
use crate::protocols::game_session_service::game_session_protocol::SplitSessionRequest;
use crate::protocols::game_session_service::game_session_protocol::SplitSessionResponse;
use crate::protocols::game_session_service::game_session_protocol::UpdateSessionRequest;
use crate::protocols::game_session_service::game_session_protocol::UpdateSessionResponse;
use crate::protocols::game_session_service::types::GameSessionInvitationReceived;
use crate::protocols::game_session_service::types::GameSessionInvitationSent;
use crate::protocols::game_session_service::types::GameSessionKey;
use crate::protocols::game_session_service::types::GameSessionSearchResult;
use crate::protocols::game_session_service::types::GameSessionSearchWithParticipantsResult;
use crate::storage::Storage;
use crate::storage::UrlsFor;

/// Quazal's notification event, hand-written rather than generated.
///
/// `sc_bl_protocols::protocol_foundation` does carry it, but that crate does not compile:
/// `types.rs` wants a `BufferTail` type quazal does not have, and
/// `remote_log_device_protocol.rs` is still `todo!()`. Not worth it for six fields.
///
/// Field order follows the DDL - it decides the bytes on the wire.
#[derive(Debug, quazal_macros::ToStream)]
pub struct NotificationEvent {
    pub pid_source: u32,
    pub ui_type: u32,
    pub ui_param_1: u32,
    pub ui_param_2: u32,
    pub str_param: String,
    pub ui_param_3: u32,
}

/// Quazal Rendez-Vous lists the notification protocol as 14, with `ProcessNotificationEvent`
/// as its method 1.
const NOTIFICATION_PROTOCOL_ID: u16 = 14;
const PROCESS_NOTIFICATION_EVENT: u32 = 1;

/// Implementation of the `GameSessionProtocolServerTrait` for handling game session operations.
struct GameSessionProtocolServerImpl {
    storage: Arc<Storage>,
    debug_config: Arc<DebugConfig>,
    /// The session each player last left, and when: a player splits the session it has
    /// just abandoned (see `split_session`), and is no longer in it then.
    left: std::sync::Mutex<std::collections::HashMap<u32, (u32, std::time::Instant)>>,
}

/// How long after leaving a session a player may still split it.
const SPLIT_AFTER_LEAVING: std::time::Duration = std::time::Duration::from_secs(60);

/// Property that tells the two rooms of a private match apart.
///
/// A host who opened a private match is in two sessions at once: the anteroom every client
/// opens on entering multiplayer, and the configured match room itself. Both name the same
/// host and live in the same session type - this property is the only thing distinguishing
/// them.
const PROPERTY_ROOM_KIND: u32 = 113;

/// Value of [`PROPERTY_ROOM_KIND`] for the anteroom every client opens for itself.
const ROOM_KIND_ANTEROOM: u32 = 1;

/// Picks the rooms a friend search should answer with.
///
/// The invited room is always in. Whether anything joins it depends on what kind of room it is:
///
///   * invited into a private match (`113 == 0`) - the host's current anteroom comes along, so
///     the client can fill both of its invitation slots
///   * invited into a lobby (`113 == 1`) - that room already *is* the anteroom, and the answer
///     is complete with it alone
///
/// At most one anteroom either way, because the client keeps exactly one slot for it. A host
/// can be left holding older anterooms it no longer advertises; sending those along means the
/// first one in the answer takes the slot, and the guest joins a room nobody is in. The
/// current one is the youngest, session ids being handed out in order - the same assumption
/// `find_host_sessions` already encodes in its `ORDER BY g.id DESC`.
fn rooms_for_friend_search<'a>(rooms: impl Iterator<Item = (u32, &'a str)>, invited_session_id: u32) -> Vec<u32> {
    let kinds: Vec<(u32, Option<u32>)> = rooms.map(|(id, attributes)| (id, attribute_value(attributes, PROPERTY_ROOM_KIND))).collect();

    let invited_is_anteroom = kinds.iter().any(|(id, kind)| *id == invited_session_id && *kind == Some(ROOM_KIND_ANTEROOM));

    let mut keep = vec![invited_session_id];
    if !invited_is_anteroom {
        let current_anteroom = kinds
            .iter()
            .filter(|(id, kind)| *kind == Some(ROOM_KIND_ANTEROOM) && *id != invited_session_id)
            .map(|(id, _)| *id)
            .max();
        keep.extend(current_anteroom);
    }
    keep
}

/// Reads one property out of an `"id => value;..."` attribute string.
///
/// Returns `None` if the property is absent or unparsable; callers treat both the same way.
pub(crate) fn attribute_value(attributes: &str, wanted_id: u32) -> Option<u32> {
    attributes.split(';').find_map(|part| {
        let (id, value) = part.split_once("=>")?;
        (id.trim().parse::<u32>().ok()? == wanted_id).then(|| value.trim().parse().ok())?
    })
}

/// A match room's mode: `"svm"` (Spies vs Mercs) or `"coop"`. None for a lobby
/// (room kind 1), which serves either.
///
/// Attribute 103 is set only by Spies vs Mercs matchmaking; a private Spies vs
/// Mercs match carries `103 => 0` like co-op. What tells them apart there is the
/// seats (attributes 3 and 4: co-op has two, Spies vs Mercs four or eight) and
/// attribute 105, which co-op sets to 2.
pub(crate) fn match_mode(attributes: &str) -> Option<&'static str> {
    if attribute_value(attributes, PROPERTY_ROOM_KIND) != Some(0) {
        return None;
    }
    if attribute_value(attributes, 103).unwrap_or(0) != 0 {
        return Some("svm");
    }
    let seats = attribute_value(attributes, 3).unwrap_or(0).saturating_add(attribute_value(attributes, 4).unwrap_or(0));
    Some(if seats > 2 && attribute_value(attributes, 105) != Some(2) { "svm" } else { "coop" })
}

/// Presents a private room as if its slots were public.
///
/// Attribute 3 counts public slots, attribute 4 private ones - a private match reserves all
/// eight as private and leaves 3 at zero. An invited client looking at that room sees no seat
/// it could take and backs out again, which is exactly what a public match with 3 => 4 does
/// not do. Only ever applied to the room a pending invitation points at.
pub(crate) fn advertise_private_slots_as_public(attributes: &str) -> String {
    let mut public_slots = 0u32;
    let mut private_slots = 0u32;
    for part in attributes.split(';') {
        let Some((id, value)) = part.split_once("=>") else { continue };
        let (Ok(id), Ok(value)) = (id.trim().parse::<u32>(), value.trim().parse::<u32>()) else {
            continue;
        };
        match id {
            3 => public_slots = value,
            4 => private_slots = value,
            _ => {}
        }
    }
    if public_slots > 0 || private_slots == 0 {
        return attributes.to_string();
    }
    attributes
        .split(';')
        .map(|part| match part.split_once("=>").and_then(|(id, _)| id.trim().parse::<u32>().ok()) {
            Some(3) => format!("3 => {private_slots}"),
            Some(4) => "4 => 0".to_string(),
            _ => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// An IPv4 network such as `10.8.0.0/16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Subnet {
    network: u32,
    mask: u32,
}

impl Subnet {
    fn parse(s: &str) -> Option<Self> {
        let (addr, len) = s.trim().split_once('/')?;
        let addr: std::net::Ipv4Addr = addr.parse().ok()?;
        let len: u32 = len.parse().ok().filter(|len| *len <= 32)?;
        let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
        Some(Self {
            network: u32::from(addr) & mask,
            mask,
        })
    }

    fn contains(&self, ip: std::net::IpAddr) -> bool {
        matches!(ip, std::net::IpAddr::V4(v4) if u32::from(v4) & self.mask == self.network)
    }
}

/// A station URL other players may be sent to, taken apart: its scheme and its
/// parameters in order. `None` for anything else: a scheme the game doesn't use for
/// players (`prudp`, `prudps`, `udp`), or an address that isn't an IPv4 literal, which is
/// all the game writes (a host name, or any other scheme, got past the address checks
/// below, which only knew `prudp:/address=` and `prudps:/address=`).
fn station_url_parts(url: &str) -> Option<(&str, Vec<(&str, &str)>)> {
    let (scheme, rest) = url.split_once(":/")?;
    if !["prudp", "prudps", "udp"].iter().any(|s| s.eq_ignore_ascii_case(scheme)) {
        return None;
    }
    let params: Vec<(&str, &str)> = rest.split(';').map(|p| p.split_once('=')).collect::<Option<_>>()?;
    let addresses: Vec<&str> = params.iter().filter(|(k, _)| *k == "address").map(|(_, v)| *v).collect();
    let ports: Vec<&str> = params.iter().filter(|(k, _)| *k == "port").map(|(_, v)| *v).collect();
    let valid = !addresses.is_empty() && addresses.iter().all(|a| a.parse::<std::net::Ipv4Addr>().is_ok()) && !ports.is_empty() && ports.iter().all(|p| p.parse::<u16>().is_ok());
    valid.then_some((scheme, params))
}

/// `url` with each address `rewrite` gives a new one for replaced (the rest as written), or
/// `None` if it isn't a station URL players may be sent to ([`station_url_parts`]).
fn rewrite_addresses(url: &str, rewrite: impl Fn(std::net::Ipv4Addr) -> Option<std::net::IpAddr>) -> Option<String> {
    let (scheme, params) = station_url_parts(url)?;
    let mut changed = false;
    let params: Vec<String> = params
        .into_iter()
        .map(|(k, v)| match (k, v.parse().ok().and_then(&rewrite)) {
            ("address", Some(new)) => {
                changed = true;
                format!("address={new}")
            }
            _ => format!("{k}={v}"),
        })
        .collect();
    Some(if changed { format!("{scheme}:/{}", params.join(";")) } else { url.to_string() })
}

/// Only the station URLs other players can be sent to ([`station_url_parts`]).
fn allowed_station_urls(urls: Vec<String>) -> Vec<String> {
    urls.into_iter().filter(|u| station_url_parts(u).is_some()).collect()
}

/// The station URLs other players get for a client, given the address the
/// server saw it connect from.
///
/// Matches run peer to peer, over the address the game registers here. The
/// game takes it from whichever network adapter it picked, and when that is
/// the wrong one (the home LAN, Radmin, ...), nobody can join that player.
/// With `trusted` set (the service setting `trusted_subnet`, e.g. the VPN
/// the community plays over, `10.8.0.0/16`), a client connecting from inside that network gets every
/// other address in its URLs replaced by the one it connected from: the
/// address the server itself reached it on. Without it, URLs are kept as sent
/// (upstream behaviour). URLs players can't be sent to are left out.
fn station_urls_for_peers(urls: Vec<String>, observed: std::net::IpAddr, trusted: Option<Subnet>) -> Vec<String> {
    let Some(trusted) = trusted.filter(|t| t.contains(observed)) else {
        return allowed_station_urls(urls);
    };
    urls.iter()
        .filter_map(|url| {
            rewrite_addresses(url, |ip| {
                let ip = std::net::IpAddr::V4(ip);
                (ip != observed && !trusted.contains(ip)).then_some(observed)
            })
        })
        .collect()
}

/// Limits on what one player can put into sessions.
const MAX_ATTRIBUTES: usize = 64;
const MAX_LIVE_SESSIONS: u32 = 16;
const MAX_URLS: usize = crate::storage::MAX_STATION_URLS as usize;
const MAX_URL_LEN: usize = 256;
const MAX_RECIPIENTS: usize = 16;
pub(crate) const MAX_SEARCH_RESULTS: u32 = 50;
const MAX_SEARCH_PIDS: usize = 32;

/// Whether an address could be on the internet (not a LAN, loopback, link-local
/// or carrier-grade NAT address).
fn is_public(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() || (a == 100 && (64..128).contains(&b)))
        }
        std::net::IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()),
    }
}

/// Station URLs other players may be sent to: a public address in them must be
/// the one this client connected from (or the relay's). Anything else would
/// have other players' games send traffic wherever a client said. LAN
/// addresses stay, for players on one network. URLs players can't be sent to
/// at all (another scheme, a host name: [`station_url_parts`]) are left out.
pub(crate) fn only_reachable_addresses(urls: Vec<String>, observed: std::net::IpAddr, relay: Option<std::net::Ipv4Addr>) -> Vec<String> {
    urls.iter()
        .filter_map(|url| {
            rewrite_addresses(url, |ip| {
                let foreign = is_public(std::net::IpAddr::V4(ip)) && std::net::IpAddr::V4(ip) != observed && relay != Some(ip);
                foreign.then_some(observed)
            })
        })
        .collect()
}

impl GameSessionProtocolServerImpl {
    /// Whether a session is invite-only: its host's game said so when it
    /// announced it (`UPLAY_USER_SetGameSession`). Public matches share the
    /// room kind of private ones, so the kind alone can't tell.
    fn is_private_room(&self, _type_id: u32, session_id: u32) -> bool {
        self.storage.is_invite_only_session(session_id).unwrap_or(false)
    }

    /// The player's game is online (it opened or joined a room): it should be registered with
    /// the NAT helper, and is noticed when it isn't.
    fn went_online(&self, user_id: u32) {
        if let Ok(Some(name)) = self.storage.find_username_by_user_id(user_id) {
            crate::nat_helper::game_went_online(&name);
        }
    }

    /// Notes who is in a match (for the players count of finished matches and each
    /// player's matches); a lobby is left alone.
    fn note_match_players(&self, logger: &Logger, session_id: u32, users: impl IntoIterator<Item = u32>) {
        for user in users {
            match self.storage.note_match_player(session_id, user) {
                Ok(true) => crate::metrics::match_started(),
                Ok(false) => {}
                Err(e) => warn!(logger, "Couldn't note {user} in match {session_id}: {e}"),
            }
        }
    }

    /// Whether `member` is in `host`'s party: the anteroom (room kind 1) the host opened and
    /// is still in, with `member` in it too. A party follows its host into a private match
    /// (the members add themselves, or the host adds them) without invitations of its own.
    ///
    /// Only that room: any other session the two share (someone else's lobby, a public
    /// match) makes nobody a member of the host's party.
    fn in_party_of(&self, member: u32, host: u32) -> bool {
        member != host
            && self
                .storage
                .rooms_of_host_with(host, member)
                .is_ok_and(|rooms| rooms.iter().any(|a| attribute_value(a, PROPERTY_ROOM_KIND) == Some(ROOM_KIND_ANTEROOM)))
    }

    /// [`Self::in_party_of`] since before the host made `match_id`: who follows the host into
    /// a private match. A stranger can add themselves to a host's public party and then
    /// find and join the match they made; the party that was there when it was made follows.
    fn in_party_before(&self, member: u32, host: u32, match_id: u32) -> bool {
        member != host
            && self
                .storage
                .rooms_of_host_with_before(host, member, match_id)
                .is_ok_and(|rooms| rooms.iter().any(|a| attribute_value(a, PROPERTY_ROOM_KIND) == Some(ROOM_KIND_ANTEROOM)))
    }

    /// LeaveSession and AbandonSession: the player is no longer in the
    /// session, and one nobody is left in ends. Upstream answered both
    /// without doing anything, so players stayed listed in rooms they had
    /// left (friend searches then found stale parties, #44) and empty
    /// lobbies stayed on offer.
    fn leave(&self, logger: &Logger, user_id: u32, session_id: u32, verb: &str) -> Result<(), Error> {
        let Some(ended) = rmc_err!(self.storage.leave_game_session(user_id, session_id), logger, "error leaving session")? else {
            // Usual, not suspicious: a host taking its party along or ending a match removes
            // the guests first, and their own leave arrives a moment later.
            info!(logger, "User {user_id} {verb} session {session_id}, which they aren't in; nothing changes");
            return Ok(());
        };
        let mut left = self.left.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = std::time::Instant::now();
        if left.len() >= 10_000 {
            left.retain(|_, (_, at)| now.duration_since(*at) < SPLIT_AFTER_LEAVING);
        }
        left.insert(user_id, (session_id, now));
        drop(left);
        info!(logger, "User {user_id} {verb} session {session_id}{}", if ended { "; nobody left, it ends" } else { "" });
        let how = if verb == "abandons" { "abandoned" } else { "left" };
        crate::session_events::note(
            crate::session_events::Who::Id(user_id),
            "leave",
            serde_json::json!({ "room": session_id, "how": how, "ended": ended }),
        );
        if ended {
            crate::session_events::room_ended(session_id);
        }
        Ok(())
    }
}

/// Whether `caller` may change a session with host `creator` and these
/// `participants`: its host and participants may; anyone else only when the
/// change concerns nobody but themselves (`targets`, for adding and removing
/// participants: joining and leaving).
fn may_change_session(caller: u32, creator: u32, participants: &[u32], targets: Option<&[u32]>) -> bool {
    caller == creator || participants.contains(&caller) || targets.is_some_and(|t| !t.is_empty() && t.iter().all(|id| *id == caller))
}

impl GameSessionProtocolServerImpl {
    /// Refuses a change to `session_id` by someone outside it (see
    /// [`may_change_session`]). Sessions the server doesn't know are left to
    /// the handler, as before.
    fn authorise(&self, logger: &Logger, caller: u32, session_id: u32, targets: Option<&[u32]>, verb: &str) -> Result<(), Error> {
        if !self.debug_config.session_owner_checks {
            return Ok(());
        }
        let members = rmc_err!(self.storage.session_members(session_id), logger, "error reading session members")?;
        match members {
            Some((creator, participants)) if !may_change_session(caller, creator, &participants, targets) => {
                warn!(
                    logger,
                    "User {caller} may not {verb} session {session_id} (host {creator}, participants {participants:?}); refused"
                );
                Err(Error::AccessDenied)
            }
            _ => Ok(()),
        }
    }
}

impl<CI> GameSessionProtocolServerTrait<CI> for GameSessionProtocolServerImpl {
    /// Handles the `CreateSession` request, creating a new game session.
    ///
    /// This function requires the client to be logged in. It extracts and stores
    /// game session attributes.
    fn create_session(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: CreateSessionRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<CreateSessionResponse, Error> {
        info!(logger, "Client creates session: {:?}", request);
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        if !crate::rate_limit::sessions().check(user_id) || request.game_session.attributes.0.len() > MAX_ATTRIBUTES {
            warn!(logger, "User {user_id} creates sessions too fast or too large; refused");
            return Err(Error::AccessDenied);
        }
        if rmc_err!(self.storage.count_live_sessions(user_id), logger, "error counting sessions")? >= MAX_LIVE_SESSIONS {
            warn!(logger, "User {user_id} already hosts {MAX_LIVE_SESSIONS} sessions; refused");
            return Err(Error::AccessDenied);
        }

        let attributes = request
            .game_session
            .attributes
            .0
            .into_iter()
            /*
            101 => map
            102 => game mode
            https://github.com/GitHubProUser67/MultiServer3/blob/dc189cfac27589356a52d2ad64c31c8a124c68f7/SpecializedServers/QuazalServer/RDVServices/DDL/Models/GameSessionService/GameSession.cs#L15
             */
            .map(|p| format!("{} => {}", p.id, p.value))
            .collect::<Vec<_>>()
            .join(";");
        let session_id = rmc_err!(
            self.storage.create_game_session(user_id, request.game_session.type_id, attributes),
            logger,
            "error creating game session"
        )?;
        made(ci, user_id, session_id);
        crate::session_events::note(crate::session_events::Who::Id(user_id), "room", serde_json::json!({ "room": session_id }));
        self.went_online(user_id);
        Ok(CreateSessionResponse {
            game_session_key: GameSessionKey {
                type_id: request.game_session.type_id,
                session_id,
            },
        })
    }

    /// Handles the `UpdateSession` request, updating an existing game session.
    ///
    /// This function requires the client to be logged in. It updates game session attributes.
    fn update_session(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: UpdateSessionRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<UpdateSessionResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        info!(logger, "Client updates session: {:?}", request);
        if request.game_session_update.attributes.0.len() > MAX_ATTRIBUTES {
            warn!(logger, "User {user_id} updates a session with too many attributes; refused");
            return Err(Error::AccessDenied);
        }
        self.authorise(logger, user_id, request.game_session_update.session_key.session_id, None, "update")?;
        let attributes = request
            .game_session_update
            .attributes
            .0
            .into_iter()
            /*
            101 => map
            102 => game mode
            https://github.com/GitHubProUser67/MultiServer3/blob/dc189cfac27589356a52d2ad64c31c8a124c68f7/SpecializedServers/QuazalServer/RDVServices/DDL/Models/GameSessionService/GameSession.cs#L15
             */
            .map(|p| format!("{} => {}", p.id, p.value))
            .collect::<Vec<_>>()
            .join(";");
        rmc_err!(
            self.storage.update_game_session(
                request.game_session_update.session_key.type_id,
                request.game_session_update.session_key.session_id,
                attributes,
            ),
            logger,
            "error updating game session"
        )?;
        Ok(UpdateSessionResponse)
    }

    /// Handles the `DeleteSession` request, deleting an existing game session.
    ///
    /// This function requires the client to be logged in.
    fn delete_session(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: DeleteSessionRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<DeleteSessionResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        if rmc_err!(
            self.storage
                .delete_game_session(user_id, request.game_session_key.type_id, request.game_session_key.session_id),
            logger,
            "error deleting session"
        )? != 1
        {
            warn!(logger, "Unexpected amount of sessions deleted");
        }
        Ok(DeleteSessionResponse)
    }

    /// Handles the `LeaveSession` request.
    ///
    /// This function requires the client to be logged in. It currently returns an empty response.
    fn leave_session(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: LeaveSessionRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<LeaveSessionResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        self.leave(logger, user_id, request.game_session_key.session_id, "leaves")?;
        Ok(LeaveSessionResponse)
    }

    /// Handles the `AddParticipants` request, adding participants to a game session.
    ///
    /// This function requires the client to be logged in.
    fn add_participants(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: AddParticipantsRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<AddParticipantsResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        info!(logger, "Client adds participants: {:?}", request);
        let mut targets: Vec<u32> = request.private_participant_ids.0.iter().chain(request.public_participant_ids.0.iter()).copied().collect();
        if targets.len() > MAX_RECIPIENTS {
            return Err(Error::AccessDenied);
        }
        // A player named twice (once private, once public: `[me, me]`) is one player. Every
        // check below must see `[me]` then, or joining that way slipped past them.
        targets.sort_unstable();
        targets.dedup();
        self.authorise(logger, user_id, request.game_session_key.session_id, Some(&targets), "add participants to")?;
        let session_id = request.game_session_key.session_id;
        let members = rmc_err!(self.storage.session_members(session_id), logger, "error reading session members")?;
        let is_member = members
            .as_ref()
            .is_some_and(|(creator, participants)| *creator == user_id || participants.contains(&user_id));
        // Nobody walks into a private match uninvited (the ids are small numbers anyone could
        // try); members, anyone invited, and the host's party may. A party taken into a
        // private match follows its host without an invitation of its own - but not past a
        // block.
        if targets.contains(&user_id) && !is_member && self.is_private_room(request.game_session_key.type_id, session_id) {
            let invited = rmc_err!(self.storage.is_invited(user_id, session_id), logger, "error checking invitations")?;
            let party_of_host = members
                .as_ref()
                .is_some_and(|(creator, _)| self.in_party_before(user_id, *creator, session_id) && !crate::friends_policy::blocked_blocking(&self.storage, user_id, *creator));
            if !invited && !party_of_host {
                warn!(logger, "User {user_id} tried to join private room {session_id} without an invitation; refused");
                return Err(Error::AccessDenied);
            }
        }
        // Adding someone else puts them into a session, and a private match nudges them in
        // (below). Only a host taking their own party along may (friends or not, as anyone
        // in their anteroom), or a friend - never past a block, and never a stranger, whom
        // a public room the two once shared made no member of anything.
        if let Some(other) = targets.iter().find(|&&t| {
            t != user_id
                && (crate::friends_policy::blocked_blocking(&self.storage, user_id, t)
                    || !(self.in_party_of(t, user_id) || crate::friends_policy::friends_blocking(&self.storage, user_id, t)))
        }) {
            warn!(logger, "User {user_id} may not add {other} to session {session_id}; refused");
            return Err(Error::AccessDenied);
        }
        rmc_err!(
            self.storage.add_participants(
                request.game_session_key.type_id,
                request.game_session_key.session_id,
                request.private_participant_ids.0.clone(),
                request.public_participant_ids.0.clone(),
            ),
            logger,
            "error adding participants"
        )?;
        let added = request.private_participant_ids.iter().chain(request.public_participant_ids.iter()).copied();
        joined(added.clone(), request.game_session_key.session_id);
        self.note_match_players(logger, request.game_session_key.session_id, added);

        // On the invitation route this call IS the join - the client never sends JoinSession
        // here (see split_session for the measurement). Retire the binding now, otherwise it
        // stays pending and misdirects the next split this player makes.
        let via_invitation = self
            .storage
            .consume_invite_for_session(user_id, request.game_session_key.type_id, request.game_session_key.session_id)
            .unwrap_or(false);
        if via_invitation {
            info!(
                logger,
                "User {user_id} joined session {} through an invitation (via AddParticipants)", request.game_session_key.session_id
            );
        }
        // Joins into someone else's room: the player's own (invited, following their party,
        // or a public room found), or their host taking them along.
        let host = members.as_ref().map(|(creator, _)| *creator);
        for &target in &targets {
            if host.is_some_and(|h| h != target) {
                let via = if target != user_id {
                    "party"
                } else if via_invitation {
                    "invite"
                } else if self.is_private_room(request.game_session_key.type_id, session_id) {
                    "party"
                } else {
                    "search"
                };
                crate::session_events::joined(target, session_id, via);
            }
        }

        // Nudge the player who was just added, or they wait forever.
        //
        // Joining a PUBLIC match, the guest sends `JoinSession` and gets its answer. Joining a
        // PRIVATE one it deliberately sends nothing: `StateJoin::vf08` reads `[object+0x5A8]`,
        // sees the room is private, skips the join and sets `[session+0x42B]`. From there
        // `StateJoin::vf00` waits on `[session+0x42A]`, and the only thing that ever sets that
        // gate is `NetOnlineSessionServiceRdv::vf28`, on a notification with
        // `ui_type % 1000 == 3` whose `ui_param_1` carries the session id. Without it the game
        // sits at "creating game session..." indefinitely.
        //
        // Only nudge PRIVATE matches (room kind 0). Public rooms send `JoinSession` and need
        // no nudge, and a packet the client does not accept stalls the ordered PRUDP stream -
        // nudging during party setup breaks the party join that otherwise works.
        let is_private_room = self
            .storage
            .game_session_attributes(request.game_session_key.type_id, request.game_session_key.session_id)
            .ok()
            .flatten()
            .is_some_and(|a| {
                a.split(';').any(|part| {
                    let Some((id, value)) = part.split_once("=>") else { return false };
                    id.trim() == "113" && value.trim() == "0"
                })
            });
        if self.debug_config.push_notifications && is_private_room {
            for participant in targets.iter().copied()
            // The caller is notified too, even though they added themselves.
            //
            // On the invitation route into a private match the guest is its own adder: it
            // splits out of its session and calls `AddParticipants` for itself. Skipping the
            // caller here means nobody gets notified at all on that route, and the guest's
            // state machine never moves. The party route is unaffected - there the host adds
            // the guest and is not in the list itself.
            {
                let event = NotificationEvent {
                    pid_source: user_id,
                    // 7003 = category 7, subtype 3. Both halves are required:
                    //
                    // `NotificationHandler::vf01` (0x0077DA30) returns early on
                    // `ui_type / 1000 != 7` without calling any listener at all.
                    // `NetOnlineSessionServiceRdv::vf28` (0x007BDFB0) then checks
                    // `ui_type % 1000 == 3` and, on a matching `ui_param_1`, opens the
                    // `[session+0x42A]` gate that `StateJoin::vf00` waits on in a private match.
                    //
                    // A bare 3 here dies in the first filter without a trace - no listener, no
                    // error, nothing.
                    ui_type: 7003,
                    // The id of the player this is ABOUT - not the room id.
                    //
                    // `vf28` compares it against `[[service@slot_0x18]+0x40C]+8`:
                    //   0x007BDFD3  mov esi,[eax+0x18]     service from table slot 0x18
                    //   0x007BDFD7  mov ebx,[ecx+0xc]      ui_param_1
                    //   0x007BDFFA  mov esi,[esi+0x40c]
                    //   0x007BE004  cmp ebx,[esi+8]        mismatch leaves the gate shut
                    //
                    // Slot 0x18 holds the local player's object, not the room, which matches the
                    // usual meaning of a Quazal `NotificationEvent`: `pid_source` is the sender,
                    // `ui_param_1` the player concerned. Putting the room id here makes `vf28` run
                    // through without ever writing the gate.
                    ui_param_1: participant,
                    // A session id as well, not a type_id.
                    //
                    // `NotificationHandler::vf01` compares it against `[[service+0x40C]+8]` - at
                    // 0x0077DA77: `mov ecx,[esi+0x40c]` / `call 0x22b14e0` (returns `[this+8]`) /
                    // `cmp eax,[ebp+8]`. `[+0x40C]` is the GameSession object and `+8` its session
                    // id; on a mismatch the loop finds no service object and never calls `vf28`.
                    ui_param_2: request.game_session_key.session_id,
                    str_param: String::new(),
                    ui_param_3: 0,
                };
                info!(logger, "Notifying {participant} about session {}: {event:?}", request.game_session_key.session_id);
                let payload = event.to_bytes();
                let reliable = self.debug_config.push_notifications_reliable;
                let mut notify = |target: &mut ClientInfo<CI>| {
                    if let Err(e) = quazal::rmc::call_client(
                        logger,
                        _ctx,
                        _socket,
                        target,
                        NOTIFICATION_PROTOCOL_ID,
                        PROCESS_NOTIFICATION_EVENT,
                        payload.clone(),
                        reliable,
                    ) {
                        error!(logger, "Notification to {participant} failed: {e}");
                    }
                };

                // Serve the caller through their own `ci` rather than through the registry.
                //
                // `client_by_user_id` filters with `try_borrow`, and the caller's cell is borrowed
                // for the duration of their own request - they are never findable that way.
                // Swapping in `borrow_mut` is no way out either; it panics the service thread.
                if participant == user_id {
                    // Invitation route only. Otherwise the HOST notifies itself as well when it
                    // adds itself while opening the private match, opening its own `[0x42A]` gate
                    // - a join gate on the host side is pointless at best.
                    if via_invitation {
                        notify(&mut *ci);
                    }
                } else if let Some(cell) = _client_registry.client_by_user_id(participant) {
                    notify(&mut cell.borrow_mut());
                } else {
                    info!(logger, "No connected client for {participant} - no notification sent");
                }
            }
        }

        Ok(AddParticipantsResponse)
    }

    /// Handles the `RemoveParticipants` request, removing participants from a game session.
    ///
    /// This function requires the client to be logged in.
    fn remove_participants(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: RemoveParticipantsRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<RemoveParticipantsResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        info!(logger, "Client removes participants: {:?}", request);
        self.authorise(
            logger,
            user_id,
            request.game_session_key.session_id,
            Some(&request.participant_ids.0),
            "remove participants from",
        )?;
        // Removing anyone but yourself is the host's call alone.
        if request.participant_ids.0.iter().any(|&t| t != user_id) {
            let members = rmc_err!(self.storage.session_members(request.game_session_key.session_id), logger, "error reading session members")?;
            if members.is_some_and(|(creator, _)| creator != user_id) && self.debug_config.session_owner_checks {
                warn!(logger, "User {user_id} isn't the host of {}; may not remove others", request.game_session_key.session_id);
                return Err(Error::AccessDenied);
            }
        }
        rmc_err!(
            self.storage
                .remove_participants(request.game_session_key.type_id, request.game_session_key.session_id, request.participant_ids.0.clone(),),
            logger,
            "error removing participants"
        )?;
        for &removed in request.participant_ids.0.iter().filter(|&&t| t != user_id) {
            crate::session_events::note(
                crate::session_events::Who::Id(removed),
                "leave",
                serde_json::json!({ "room": request.game_session_key.session_id, "how": "removed", "by": user_id }),
            );
        }
        Ok(RemoveParticipantsResponse)
    }

    /// Handles the `AbandonSession` request.
    ///
    /// This function requires the client to be logged in. It currently returns an empty response.
    fn abandon_session(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: AbandonSessionRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<AbandonSessionResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        self.leave(logger, user_id, request.game_session_key.session_id, "abandons")?;
        Ok(AbandonSessionResponse)
    }

    /// Handles the `RegisterUrLs` request, registering client URLs.
    ///
    /// This function requires the client to be logged in.
    fn register_urls(
        &self,
        logger: &Logger,
        ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: RegisterUrLsRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<RegisterUrLsResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        info!(logger, "Client registers urls: {:?}", request);

        let trusted = ctx.settings.get("trusted_subnet").and_then(|s| {
            let subnet = Subnet::parse(s);
            if subnet.is_none() {
                warn!(logger, "ignoring invalid trusted_subnet {s:?} (expected e.g. 10.8.0.0/16)");
            }
            subnet
        });
        let sent: Vec<String> = request.station_urls.0.into_iter().map(|su| su.to_string()).collect();
        if sent.len() > MAX_URLS || sent.iter().any(|u| u.len() > MAX_URL_LEN) {
            warn!(logger, "User {user_id} registers too many or too long station URLs; refused");
            return Err(Error::AccessDenied);
        }
        let allowed = allowed_station_urls(sent.clone());
        if allowed.len() != sent.len() {
            warn!(logger, "User {user_id} registers station URLs other players can't be sent to; left out");
        }
        let mut urls = station_urls_for_peers(allowed.clone(), ci.address().ip(), trusted);
        // Outside the trusted network: the address the NAT helper checked
        // (the player's public one, or the relay's), in place of the one
        // the game registered, local or public.
        if urls == allowed {
            let name = self.storage.find_username_by_user_id(user_id).ok().flatten();
            if let Some(advertise) = name.and_then(|name| crate::nat_helper::advertised_for(&name, ci.address().ip())) {
                urls = crate::nat_helper::urls_with_public_address(urls, advertise);
            }
        }
        let urls = only_reachable_addresses(urls, ci.address().ip(), crate::nat_helper::relay_ip());
        if urls != sent {
            info!(logger, "station urls {:?} -> {:?} (the address this client connected from)", sent, urls);
        }

        rmc_err!(self.storage.register_urls(user_id, urls), logger, "error adding participants")?;
        Ok(RegisterUrLsResponse)
    }

    /// Handles the `SearchSessionsWithParticipants` request, searching for game sessions.
    ///
    /// This function requires the client to be logged in.
    fn search_sessions_with_participants(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: SearchSessionsWithParticipantsRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<SearchSessionsWithParticipantsResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        info!(logger, "Searches for sessions with {request:?}");
        if request.participant_ids.0.len() > MAX_SEARCH_PIDS || !crate::rate_limit::game_requests().check(user_id) {
            return Err(Error::AccessDenied);
        }

        // Resolved before the search, because the answer depends on it.
        let invited = rmc_err!(
            self.storage
                .find_pending_invited_session(user_id, request.game_session_type_id, request.participant_ids.0.as_slice()),
            logger,
            "error resolving invited room"
        )?;

        // The searching client is put into the room it was invited to before we answer.
        //
        // Why: on the invitation route the game hands this search an output list at
        // manager+0xA8 and then walks it to notify its listeners (0x0085B7C0). That list stays
        // empty in our setup, so nothing is notified and the join never starts. The one thing
        // our answer does differently from a working search is the participant list - it names
        // only the host, and the client that is asking does not appear in it. Adding the caller
        // first makes the room look the way it would if the join had already been accepted.
        if let Some(room) = invited.as_ref() {
            if !room.participants.iter().any(|p| p.user_id == user_id) {
                info!(logger, "Adding {user_id} to invited room {} before answering the search", room.session_id);
                rmc_err!(
                    self.storage.add_participants(room.session_type, room.session_id, vec![user_id], vec![]),
                    logger,
                    "error adding the invited player"
                )?;
                joined([user_id], room.session_id);
                self.note_match_players(logger, room.session_id, [user_id]);
            }
        }

        let mut sessions = self
            .storage
            .search_sessions_with_participants(request.game_session_type_id, request.participant_ids.0.as_slice(), MAX_SEARCH_RESULTS)
            .map_err(|e| {
                error!(logger, "Error searching game sessions: {e}");
                Error::InternalError
            })?;

        // Any pid can be searched for, so an invite-only room is answered only to those who
        // may join it: its members (the invited player was just added), anyone invited, and
        // the host's party. Anyone else learnt its host's address from this.
        //
        // Invited into a host's match, the host's anteroom counts as well: the game needs both
        // rooms in this answer (see below), and a player invited back after leaving the match
        // is no longer in the anteroom - leaving it out stranded them in a lobby of their own
        // until the match ended. `rooms_for_friend_search` below keeps no more than those two.
        let invited_by = invited.as_ref().map(|room| room.creator_id);
        let private = rmc_err!(
            self.storage.invite_only_among(&sessions.iter().map(|s| s.session_id).collect::<Vec<_>>()),
            logger,
            "error reading invite-only sessions"
        )?;
        sessions.retain(|session| {
            !private.contains(&session.session_id)
                || session.creator_id == user_id
                || session.participants.iter().any(|p| p.user_id == user_id)
                || self.storage.is_invited(user_id, session.session_id).unwrap_or(false)
                || self.in_party_before(user_id, session.creator_id, session.session_id)
                || invited_by == Some(session.creator_id)
        });

        // Answer with the invited room, and - if that is a match room - the host's anteroom.
        //
        // The client does not pick one at random - it sorts them. For every session in this
        // answer, `nsOnlinePresence::StateAcceptInviteFindSession::vf00` reads
        // `PROPERTY_ROOM_KIND` and files the session into one of two slots of its invitation
        // object, filling each slot only while it is still empty:
        //
        //     113 == 0  ->  match slot
        //     113 == 1  ->  anteroom slot
        //     absent    ->  neither slot
        //
        // `nsOnlinePresence::StateAcceptInvite::vf08` then picks, and the match slot wins: a
        // filled match slot sends the guest straight into the private session, an anteroom
        // slot alone only joins the host's lobby, and two empty slots make the client open a
        // party of its own instead of joining anything.
        //
        // Narrowing this answer to the invited room therefore withholds exactly the session
        // the client needs to fill its second slot, and no invitation can reach a match that
        // way. Sending too much is just as wrong: a second anteroom takes the one slot there
        // is, and the guest joins a room nobody is in. `rooms_for_friend_search` picks exactly
        // what the invitation calls for.
        if let Some(invited) = invited {
            if sessions.iter().any(|session| session.session_id == invited.session_id) {
                let keep = rooms_for_friend_search(sessions.iter().map(|session| (session.session_id, session.attributes.as_str())), invited.session_id);
                sessions.retain(|session| keep.contains(&session.session_id));

                let rooms: Vec<String> = sessions
                    .iter()
                    .map(|session| format!("{} (113 => {:?})", session.session_id, attribute_value(&session.attributes, PROPERTY_ROOM_KIND)))
                    .collect();
                info!(
                    logger,
                    "Friend search of {user_id}: invited room {} plus anteroom -> {}",
                    invited.session_id,
                    rooms.join(", ")
                );

                // Slot counts stay untouched. A private match carries `3 => 0; 4 => 8`: no
                // public seats, eight private ones - and a private seat is exactly what an
                // invited player takes. Rewriting that to `3 => 8; 4 => 0`, as
                // `advertise_private_slots_as_public` does, fakes public seats instead; the
                // server already adds the invited player to the private list above.
            }
        }

        info!(logger, "Found {} sessions", sessions.len());

        Ok(SearchSessionsWithParticipantsResponse {
            search_results: sessions
                .into_iter()
                .filter_map(|session| {
                    // A session whose host has left has nobody to connect to.
                    let Some(host) = session.participants.iter().find(|p| p.user_id == session.creator_id) else {
                        warn!(logger, "skipping session {} without its host", session.session_id);
                        return None;
                    };
                    Some(GameSessionSearchWithParticipantsResult {
                        game_session_search_result: GameSessionSearchResult {
                            session_key: GameSessionKey {
                                type_id: session.session_type,
                                session_id: session.session_id,
                            },
                            host_pid: host.user_id,
                            host_urls: quazal::rmc::types::QList::parse_lossy(&host.station_urls),
                            attributes: session.attributes.as_str().parse().unwrap_or_default(),
                        },
                        participant_ids: session.participants.into_iter().map(|p| p.user_id).collect(),
                    })
                })
                .collect(),
        })
    }

    /// Handles the `SplitSession` request.
    ///
    /// This function requires the client to be logged in. It currently returns a placeholder response.
    fn split_session(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: SplitSessionRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<SplitSessionResponse, Error> {
        // Ensure the client is logged in.
        let user_id = login_required(&*ci)?;
        let key = request.game_session_key;
        // A split makes a session, copied from another: as limited as creating one, and
        // only of a session the caller is in, or has just left (the game abandons its
        // session and then splits it).
        if !crate::rate_limit::sessions().check(user_id) {
            warn!(logger, "User {user_id} splits sessions too fast; refused");
            return Err(Error::AccessDenied);
        }
        if rmc_err!(self.storage.count_live_sessions(user_id), logger, "error counting sessions")? >= MAX_LIVE_SESSIONS {
            warn!(logger, "User {user_id} already hosts {MAX_LIVE_SESSIONS} sessions; split refused");
            return Err(Error::AccessDenied);
        }
        let just_left = self
            .left
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&user_id)
            .is_some_and(|(session, at)| *session == key.session_id && at.elapsed() < SPLIT_AFTER_LEAVING);
        let creator = rmc_err!(self.storage.session_creator(key.session_id), logger, "error reading session")?;
        let member = just_left
            || creator == Some(user_id)
            || rmc_err!(self.storage.session_members(key.session_id), logger, "error reading session members")?.is_some_and(|(_, p)| p.contains(&user_id));
        if creator.is_some() && !member {
            warn!(logger, "User {user_id} splits session {} they aren't in; refused", key.session_id);
            return Err(Error::AccessDenied);
        }

        let migrated = rmc_err!(
            self.storage.split_game_session(user_id, key.type_id, key.session_id),
            logger,
            "error splitting game session"
        )?;
        if let Some(session_id) = migrated {
            made(ci, user_id, session_id);
        }
        Ok(SplitSessionResponse {
            game_session_key_migrated: match migrated {
                Some(session_id) => GameSessionKey { type_id: key.type_id, session_id },
                None => key,
            },
        })
    }

    /// Records why a client failed to join a session.
    ///
    /// This is the only place where the client tells us the reason. The default stub answered
    /// with `UnimplementedMethod` and dropped the payload, which carries an error category and
    /// an error code per room - the one piece of diagnosis available when a join silently
    /// falls apart.
    fn report_unsuccessful_join_sessions(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: ReportUnsuccessfulJoinSessionsRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<ReportUnsuccessfulJoinSessionsResponse, Error> {
        let user_id = login_required(&*ci)?;
        // A game reports the join it just failed; one call listed 1,900, and calls came by
        // the thousand, each entry a warning and a report's worth of work. A few per call, and
        // a few a minute per player.
        let listed = request.unsuccessful_join_sessions.0.len();
        let taken = failed_joins_allowed(user_id, listed.min(MAX_FAILED_JOINS_PER_CALL));
        if taken < listed {
            debug!(logger, "Join failures from {user_id}: {listed} listed, {taken} taken (too many)");
        }
        for failed in request.unsuccessful_join_sessions.0.iter().take(taken) {
            crate::metrics::failed_join();
            #[allow(clippy::cast_sign_loss)]
            crate::reports::join_failed(user_id, failed.session_key.session_id, failed.error_code as u32);
            crate::session_events::note(
                crate::session_events::Who::Id(user_id),
                "join_failed",
                serde_json::json!({ "room": failed.session_key.session_id, "code": format!("{:#010x}", failed.error_code) }),
            );
            warn!(
                logger,
                "Join failed: {user_id} did not get into session {} (type {}) - category {}, code {:#010x}",
                failed.session_key.session_id,
                failed.session_key.type_id,
                failed.error_category,
                failed.error_code
            );
        }
        Ok(ReportUnsuccessfulJoinSessionsResponse)
    }

    /// Handles the `SendInvitation` request, inviting players into a session.
    ///
    /// This is the entry point of a private lobby: the host creates a session that is not meant
    /// to be found by matchmaking and hands out invitations instead. One row is stored per
    /// recipient; the recipients pick them up with `GetInvitationsReceived`.
    ///
    /// This function requires the client to be logged in.
    fn send_invitation(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: SendInvitationRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<SendInvitationResponse, Error> {
        let user_id = login_required(&*ci)?;
        info!(logger, "Client invites into session: {:?}", request);
        let invitation = request.invitation;
        // Only from inside the session, to a few players at once, within the invite rules.
        let recipients = &invitation.recipient_pids.0;
        if recipients.len() > MAX_RECIPIENTS || !crate::rate_limit::invites().check(user_id) {
            return Err(Error::AccessDenied);
        }
        let members = rmc_err!(self.storage.session_members(invitation.session_key.session_id), logger, "error reading session members")?;
        if !members.is_some_and(|(creator, participants)| creator == user_id || participants.contains(&user_id)) {
            warn!(logger, "User {user_id} invites into session {} they aren't in; refused", invitation.session_key.session_id);
            return Err(Error::AccessDenied);
        }
        if let Some(other) = recipients.iter().find(|&&r| !crate::friends_policy::may_invite_blocking(&self.storage, user_id, r)) {
            warn!(logger, "User {user_id} may not invite {other}; refused");
            return Err(Error::AccessDenied);
        }
        rmc_err!(
            self.storage.add_game_session_invites(
                invitation.session_key.type_id,
                invitation.session_key.session_id,
                user_id,
                &invitation.recipient_pids.0,
                &invitation.message,
            ),
            logger,
            "error storing session invitations"
        )?;
        Ok(SendInvitationResponse)
    }

    /// Handles the `GetInvitationReceivedCount` request.
    ///
    /// The client polls this before fetching the list itself, so it stays cheap.
    ///
    /// This function requires the client to be logged in.
    fn get_invitation_received_count(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: GetInvitationReceivedCountRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<GetInvitationReceivedCountResponse, Error> {
        let user_id = login_required(&*ci)?;
        let count = rmc_err!(
            self.storage.count_game_session_invites_received(user_id, request.game_session_type_id),
            logger,
            "error counting received invitations"
        )?;
        Ok(GetInvitationReceivedCountResponse { count })
    }

    /// Handles the `GetInvitationSentCount` request.
    ///
    /// This function requires the client to be logged in.
    fn get_invitation_sent_count(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: GetInvitationSentCountRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<GetInvitationSentCountResponse, Error> {
        let user_id = login_required(&*ci)?;
        let count = rmc_err!(
            self.storage.count_game_session_invites_sent(user_id, request.game_session_type_id),
            logger,
            "error counting sent invitations"
        )?;
        Ok(GetInvitationSentCountResponse { count })
    }

    /// Handles the `GetInvitationsReceived` request, listing pending invitations for this player.
    ///
    /// This function requires the client to be logged in.
    fn get_invitations_received(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: GetInvitationsReceivedRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<GetInvitationsReceivedResponse, Error> {
        let user_id = login_required(&*ci)?;
        let invites = rmc_err!(
            self.storage
                .list_game_session_invites_received(user_id, request.game_session_type_id, request.result_range.offset, request.result_range.size,),
            logger,
            "error listing received invitations"
        )?;
        info!(logger, "{} invitation(s) pending for user {}", invites.len(), user_id);
        Ok(GetInvitationsReceivedResponse {
            invitations: invites
                .into_iter()
                .map(|invite| GameSessionInvitationReceived {
                    session_key: GameSessionKey {
                        type_id: invite.session_type,
                        session_id: invite.session_id,
                    },
                    sender_pid: invite.sender,
                    message: invite.message,
                    creation_time: DateTime(invite.created_at),
                })
                .collect::<Vec<_>>()
                .into(),
        })
    }

    /// Handles the `GetInvitationsSent` request, listing invitations this player handed out.
    ///
    /// This function requires the client to be logged in.
    fn get_invitations_sent(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: GetInvitationsSentRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<GetInvitationsSentResponse, Error> {
        let user_id = login_required(&*ci)?;
        let invites = rmc_err!(
            self.storage
                .list_game_session_invites_sent(user_id, request.game_session_type_id, request.result_range.offset, request.result_range.size,),
            logger,
            "error listing sent invitations"
        )?;
        Ok(GetInvitationsSentResponse {
            invitations: invites
                .into_iter()
                .map(|invite| GameSessionInvitationSent {
                    session_key: GameSessionKey {
                        type_id: invite.session_type,
                        session_id: invite.session_id,
                    },
                    recipient_pid: invite.receiver,
                    message: invite.message,
                    creation_time: DateTime(invite.created_at),
                })
                .collect::<Vec<_>>()
                .into(),
        })
    }

    /// Handles the `AcceptInvitation` request.
    ///
    /// Accepting consumes the invitation and puts the player into the session's participant
    /// list. It is deliberately added as a *private* participant: that is what an invited
    /// player is, and it keeps the distinction the client already makes in `AddParticipants`.
    ///
    /// The actual connection is then established peer to peer - just like on the matchmaking
    /// path, where `JoinSession` also does not have to do anything, because the client already
    /// holds the host's station URLs.
    ///
    /// This function requires the client to be logged in.
    fn accept_invitation(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: AcceptInvitationRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<AcceptInvitationResponse, Error> {
        let user_id = login_required(&*ci)?;
        let invitation = request.game_session_invitation;
        info!(logger, "User {} accepts invitation: {:?}", user_id, invitation);
        let exists = rmc_err!(
            self.storage.game_session_invite_exists(invitation.session_key.session_id, invitation.sender_pid, user_id),
            logger,
            "error reading invitations"
        )?;
        if !exists {
            warn!(logger, "User {user_id} accepts an invitation that doesn't exist; refused");
            return Err(Error::AccessDenied);
        }

        rmc_err!(
            self.storage
                .add_participants(invitation.session_key.type_id, invitation.session_key.session_id, vec![user_id], vec![]),
            logger,
            "error joining session after accepting invitation"
        )?;
        joined([user_id], invitation.session_key.session_id);
        rmc_err!(
            self.storage
                .delete_game_session_invite(invitation.session_key.type_id, invitation.session_key.session_id, invitation.sender_pid, user_id,),
            logger,
            "error clearing accepted invitation"
        )?;
        Ok(AcceptInvitationResponse)
    }

    /// Handles the `DeclineInvitation` request, dropping the invitation without joining.
    ///
    /// This function requires the client to be logged in.
    fn decline_invitation(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: DeclineInvitationRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<DeclineInvitationResponse, Error> {
        let user_id = login_required(&*ci)?;
        let invitation = request.game_session_invitation;
        info!(logger, "User {} declines invitation: {:?}", user_id, invitation);
        rmc_err!(
            self.storage
                .delete_game_session_invite(invitation.session_key.type_id, invitation.session_key.session_id, invitation.sender_pid, user_id,),
            logger,
            "error clearing declined invitation"
        )?;
        Ok(DeclineInvitationResponse)
    }

    /// Handles the `CancelInvitation` request, withdrawing an invitation as the sender.
    ///
    /// This function requires the client to be logged in.
    fn cancel_invitation(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: CancelInvitationRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<CancelInvitationResponse, Error> {
        let user_id = login_required(&*ci)?;
        let invitation = request.game_session_invitation;
        info!(logger, "User {} cancels invitation: {:?}", user_id, invitation);
        rmc_err!(
            self.storage
                .delete_game_session_invite(invitation.session_key.type_id, invitation.session_key.session_id, user_id, invitation.recipient_pid,),
            logger,
            "error cancelling invitation"
        )?;
        Ok(CancelInvitationResponse)
    }

    /// Handles the `SearchSessions` request, the plain session search without a participant filter.
    ///
    /// `SearchSessionsWithParticipants` above answers "which session are these players in";
    /// this one answers "which sessions of this type are open". The client's own sessions are
    /// filtered out - it has no use for finding itself.
    ///
    /// This function requires the client to be logged in.
    fn search_sessions(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: SearchSessionsRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<SearchSessionsResponse, Error> {
        let user_id = login_required(&*ci)?;
        info!(logger, "Client searches for sessions: {:?}", request);
        if !crate::rate_limit::game_requests().check(user_id) {
            return Err(Error::AccessDenied);
        }

        let sessions = rmc_err!(
            self.storage.search_sessions(request.game_session_query.type_id, user_id, Some(MAX_SEARCH_RESULTS)),
            logger,
            "error searching game sessions"
        )?;
        let sessions = rmc_err!(self.storage.with_participants(sessions, UrlsFor::Hosts), logger, "error reading participants")?;
        info!(logger, "Found {} sessions", sessions.len());

        Ok(SearchSessionsResponse {
            search_results: sessions
                .into_iter()
                .filter_map(|session| {
                    // A session whose creator is not among its own participants has not
                    // registered its host URLs yet; there is nothing a client could connect to,
                    // so it is left out rather than reported with an empty address.
                    let host = session.participants.iter().find(|p| p.user_id == session.creator_id)?;
                    Some(GameSessionSearchResult {
                        session_key: GameSessionKey {
                            type_id: session.session_type,
                            session_id: session.session_id,
                        },
                        host_pid: host.user_id,
                        host_urls: host.station_urls.clone().try_into().ok()?,
                        attributes: session.attributes.as_str().parse().ok()?,
                    })
                })
                .collect::<Vec<_>>()
                .into(),
        })
    }

    /// Handles the `JoinSession` request.
    ///
    /// This function currently returns an empty response.
    fn join_session(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: JoinSessionRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<JoinSessionResponse, Error> {
        let user_id = login_required(&*ci)?;
        // The response carries no fields - the client already has the host's addresses from
        // the search result and builds the connection itself. What this call is good for is
        // knowing that the join happened: only now may a pending invitation be retired, so
        // that a client repeating its search in the meantime still finds the room.
        let key = request.game_session_key;
        let invited = rmc_err!(
            self.storage.consume_invite_for_session(user_id, key.type_id, key.session_id),
            logger,
            "error consuming invitation"
        )?;
        if invited {
            info!(logger, "User {user_id} joined session {} through an invitation", key.session_id);
        }
        // Who's in it: the host never adds a guest who joins this way, so note them here, or
        // they show as in no game at all. Only a room matchmaking lists, or one they were
        // invited to (storage checks), and not faster than a game joins (JoinSession names any
        // id: a client looping over them would be a guest of every match).
        let mut admitted = invited;
        if crate::rate_limit::game_requests().check(user_id) {
            match self.storage.add_guest(key.session_id, user_id, invited) {
                Ok(true) => {
                    joined([user_id], key.session_id);
                    admitted = true;
                }
                Ok(false) => {}
                Err(e) => warn!(logger, "Couldn't note {user_id} in session {}: {e}", key.session_id),
            }
            // Sessions' join event too, at the same pace: each is kept, and remembered for
            // ten minutes to match the join with how it went.
            crate::session_events::joined(user_id, key.session_id, if invited { "invite" } else { "search" });
        }
        // In the match's players (their matches played) only if they're in it: JoinSession
        // names any id, and a stranger's joins of a private match counted for them and the
        // host alike.
        if !admitted {
            admitted = crate::storage::run(self.storage.is_in_session(user_id, key.session_id)).is_ok_and(|r| r.unwrap_or(false));
        }
        if admitted {
            self.note_match_players(logger, key.session_id, [user_id]);
        }
        self.went_online(user_id);
        Ok(JoinSessionResponse)
    }
}

/// The rooms each signed-in player's connections made, by connection: a game that dropped
/// and signed in again from the same address (its old connection still open, as far as we
/// know) leaves rooms nobody will host again, and they go when that old connection does
/// ([`rooms_of`]), while the user, still connected, keeps everything else.
static ROOMS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u32, Vec<(quazal::ConnectionID, u32)>>>> = std::sync::LazyLock::new(Default::default);

/// Notes that `ci`'s connection made room `session_id`.
fn made<CI>(ci: &ClientInfo<CI>, user_id: u32, session_id: u32) {
    if let Some(conn) = ci.connection_id {
        ROOMS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(user_id)
            .or_default()
            .push((conn, session_id));
    }
}

/// The rooms `conn` of `user_id` made, forgotten here: it's gone.
pub fn rooms_of(user_id: u32, conn: quazal::ConnectionID) -> Vec<u32> {
    let mut rooms = ROOMS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(made) = rooms.get_mut(&user_id) else { return Vec::new() };
    let (gone, kept): (Vec<_>, Vec<_>) = made.drain(..).partition(|(c, _)| *c == conn);
    *made = kept;
    if made.is_empty() {
        rooms.remove(&user_id);
    }
    gone.into_iter().map(|(_, room)| room).collect()
}

/// When each signed-in player's current game signed in, and the sessions they were put in
/// (most recent last, at most [`JOINS_KEPT`]): a game that dropped and signed in again
/// stays a member of the rooms it had joined, and leaves them when its old connection goes
/// ([`joined_by_earlier_games`]).
#[derive(Default)]
struct Membership {
    signed_in: Option<std::time::Instant>,
    joined: std::collections::VecDeque<(u32, std::time::Instant)>,
}

const JOINS_KEPT: usize = 256;

static MEMBERSHIP: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u32, Membership>>> = std::sync::LazyLock::new(Default::default);

fn membership() -> std::sync::MutexGuard<'static, std::collections::HashMap<u32, Membership>> {
    MEMBERSHIP.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `user_id`'s game signed in: a new game, if they had one before.
pub fn signed_in(user_id: u32) {
    membership().entry(user_id).or_default().signed_in = Some(std::time::Instant::now());
}

/// Notes that `users` were put in session `session_id`.
fn joined(users: impl IntoIterator<Item = u32>, session_id: u32) {
    let now = std::time::Instant::now();
    let mut all = membership();
    for user_id in users {
        let joined = &mut all.entry(user_id).or_default().joined;
        if joined.len() >= JOINS_KEPT {
            joined.pop_front();
        }
        joined.push_back((session_id, now));
    }
}

/// The sessions `user_id` was put in before their current game signed in, forgotten here:
/// the games that joined them are gone. Not those the current game was put in again.
pub fn joined_by_earlier_games(user_id: u32) -> Vec<u32> {
    let mut all = membership();
    let Some(m) = all.get_mut(&user_id) else { return Vec::new() };
    let Some(since) = m.signed_in else { return Vec::new() };
    let (earlier, later): (Vec<_>, Vec<_>) = m.joined.drain(..).partition(|(_, at)| *at < since);
    let mut gone: Vec<u32> = earlier
        .into_iter()
        .map(|(session, _)| session)
        .filter(|s| !later.iter().any(|(again, _)| again == s))
        .collect();
    gone.sort_unstable();
    gone.dedup();
    m.joined = later.into();
    gone
}

/// Forgets `user_id`'s rooms and memberships: they're signed out, every room of theirs has
/// ended, and their memberships are cleaned up with their session.
pub fn forget_rooms(user_id: u32) {
    ROOMS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&user_id);
    membership().remove(&user_id);
}

/// Creates a new boxed `GameSessionProtocolServer` instance.
///
/// This function is typically used to register the game session protocol
/// with the server's protocol dispatcher.
pub fn new_protocol<T: 'static>(storage: Arc<Storage>, debug_config: Arc<DebugConfig>) -> Box<dyn Protocol<T>> {
    Box::new(GameSessionProtocolServer::new(GameSessionProtocolServerImpl {
        storage,
        debug_config,
        left: std::sync::Mutex::default(),
    }))
}

/// Join failures taken from one call, and from one player a minute (see
/// `report_unsuccessful_join_sessions`).
const MAX_FAILED_JOINS_PER_CALL: usize = 8;
const MAX_FAILED_JOINS_A_MINUTE: u32 = 30;

/// How many of `wanted` join failures `user_id` may report now, counting them.
fn failed_joins_allowed(user_id: u32, wanted: usize) -> usize {
    static SEEN: std::sync::Mutex<Option<std::collections::HashMap<u32, (std::time::Instant, u32)>>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let seen = seen.get_or_insert_with(std::collections::HashMap::new);
    let now = std::time::Instant::now();
    let minute = std::time::Duration::from_secs(60);
    if seen.len() >= 10_000 {
        seen.retain(|_, (since, _)| now.duration_since(*since) < minute);
    }
    let entry = seen.entry(user_id).or_insert((now, 0));
    if now.duration_since(entry.0) >= minute {
        *entry = (now, 0);
    }
    let room = MAX_FAILED_JOINS_A_MINUTE.saturating_sub(entry.1) as usize;
    let taken = wanted.min(room);
    entry.1 += taken as u32;
    taken
}

#[cfg(test)]
mod tests {

    /// A game that signed in again leaves the rooms an earlier game of the player's joined,
    /// but not one it joined again itself.
    #[test]
    fn earlier_games_rooms_are_left_but_not_those_joined_again() {
        use super::forget_rooms;
        use super::joined;
        use super::joined_by_earlier_games;
        use super::signed_in;
        let user = 4_000_123;
        joined([user], 1);
        joined([user], 2);
        assert!(joined_by_earlier_games(user).is_empty(), "no game signed in since");
        std::thread::sleep(std::time::Duration::from_millis(2));
        signed_in(user);
        joined([user], 2);
        joined([user], 3);
        assert_eq!(joined_by_earlier_games(user), vec![1]);
        assert!(joined_by_earlier_games(user).is_empty(), "forgotten once left");
        forget_rooms(user);
    }
    use quazal::rmc::basic::ToStream as _;
    use quazal::rmc::Request;

    use super::attribute_value;
    use super::may_change_session;
    use super::only_reachable_addresses;
    use super::rooms_for_friend_search;
    use super::station_urls_for_peers;
    use super::NotificationEvent;
    use super::Subnet;
    use super::NOTIFICATION_PROTOCOL_ID;
    use super::PROCESS_NOTIFICATION_EVENT;
    use super::PROPERTY_ROOM_KIND;

    /// Builds the notification exactly as `add_participants` does and takes it apart again.
    ///
    /// This cannot tell whether 14 is the right protocol id - only the client knows that - but
    /// it catches everything that can go wrong on our side: wrong field order, broken framing,
    /// a lost call id.
    #[test]
    fn notification_event_is_well_formed() {
        let event = NotificationEvent {
            pid_source: 1006,
            ui_type: 3,
            ui_param_1: 234,
            ui_param_2: 1,
            str_param: String::new(),
            ui_param_3: 0,
        };
        let payload = event.to_bytes();

        let request = Request {
            protocol_id: NOTIFICATION_PROTOCOL_ID,
            call_id: 0x8000_0001,
            method_id: PROCESS_NOTIFICATION_EVENT,
            parameters: payload.clone(),
        };
        let bytes = request.to_bytes();
        let decoded = Request::from_bytes(&bytes).expect("request must round-trip");

        assert_eq!(decoded.protocol_id, 14, "notification protocol");
        assert_eq!(decoded.method_id, 1, "ProcessNotificationEvent");
        assert_eq!(decoded.call_id, 0x8000_0001);
        assert_eq!(decoded.parameters, payload);

        // The first three fields decide the gate on the client side. They have to sit there
        // as u32 in this order, or the game reads nonsense.
        assert_eq!(&payload[0..4], &1006u32.to_le_bytes(), "pid_source first");
        assert_eq!(&payload[4..8], &3u32.to_le_bytes(), "then ui_type");
        assert_eq!(&payload[8..12], &234u32.to_le_bytes(), "then ui_param_1");
    }

    const MATCH_ROOM_ATTRIBUTES: &str = "113 => 0;3 => 0;4 => 8;102 => 7";
    const ANTEROOM_ATTRIBUTES: &str = "113 => 1;3 => 8;4 => 0";

    /// Invited into a private match: the match room and the anteroom, so the client can fill
    /// both of its invitation slots.
    #[test]
    fn match_invitation_answers_with_match_room_and_anteroom() {
        let rooms = [(471, ANTEROOM_ATTRIBUTES), (472, MATCH_ROOM_ATTRIBUTES)];

        let keep = rooms_for_friend_search(rooms.into_iter(), 472);

        assert!(keep.contains(&472), "the invited match room");
        assert!(keep.contains(&471), "the anteroom the client needs alongside it");
        assert_eq!(keep.len(), 2);
    }

    /// Invited into a lobby: that room already is the anteroom, so it goes alone.
    ///
    /// Sending a second anteroom would take the one slot the client has for it.
    #[test]
    fn lobby_invitation_answers_with_that_room_alone() {
        let rooms = [(477, ANTEROOM_ATTRIBUTES), (478, ANTEROOM_ATTRIBUTES)];

        let keep = rooms_for_friend_search(rooms.into_iter(), 478);

        assert_eq!(keep, vec![478]);
    }

    /// Anterooms the host stopped advertising must not displace the current one.
    ///
    /// A host can be left holding older anterooms; the current one is the youngest.
    #[test]
    fn stale_anterooms_do_not_displace_the_current_one() {
        let rooms = [(477, ANTEROOM_ATTRIBUTES), (478, ANTEROOM_ATTRIBUTES), (484, MATCH_ROOM_ATTRIBUTES)];

        let keep = rooms_for_friend_search(rooms.into_iter(), 484);

        assert!(keep.contains(&484), "the invited match room");
        assert!(keep.contains(&478), "the youngest anteroom is the current one");
        assert!(!keep.contains(&477), "the stale anteroom stays out");
    }

    /// A host without an anteroom yields just the invited room, not an empty answer.
    #[test]
    fn match_invitation_without_anteroom_still_answers() {
        let rooms = [(484, MATCH_ROOM_ATTRIBUTES)];

        assert_eq!(rooms_for_friend_search(rooms.into_iter(), 484), vec![484]);
    }

    /// A missing or unparsable property reads as absent rather than as a value.
    #[test]
    fn peers_get_the_address_a_trusted_client_connected_from() {
        let trusted = Subnet::parse("10.8.0.0/16");
        let vpn: std::net::IpAddr = "10.8.1.2".parse().unwrap();
        let sent = vec![
            "prudp:/address=192.168.1.20;port=3074;sid=15;type=2".to_string(),
            "prudp:/address=26.144.25.254;port=3074;sid=15;type=3".to_string(),
        ];
        // The game picked the home LAN / Radmin adapter; peers get the VPN address.
        assert_eq!(
            station_urls_for_peers(sent.clone(), vpn, trusted),
            ["prudp:/address=10.8.1.2;port=3074;sid=15;type=2", "prudp:/address=10.8.1.2;port=3074;sid=15;type=3"]
        );
        // Already right: unchanged.
        let right = vec!["prudp:/address=10.8.1.2;port=3074;type=2".to_string()];
        assert_eq!(station_urls_for_peers(right.clone(), vpn, trusted), right);
        // Not configured, or a client from outside the trusted network: kept as sent.
        assert_eq!(station_urls_for_peers(sent.clone(), vpn, None), sent);
        assert_eq!(station_urls_for_peers(sent.clone(), "203.0.113.9".parse().unwrap(), trusted), sent);
    }

    #[test]
    fn station_urls_players_cant_be_sent_to_are_left_out() {
        let observed: std::net::IpAddr = "203.0.113.9".parse().unwrap();
        let sent: Vec<String> = [
            "prudp:/address=198.51.100.1;port=3074;type=2",
            "udp:/address=198.51.100.1;port=80",
            "prudps:/address=victim.example;port=3074",
            "http:/address=192.168.1.1;port=80",
            "prudp:/address=::1;port=3074",
            "prudp:/port=3074",
            "prudp:/address=10.0.0.5;port=13000;RVCID=5;hdrType=0",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            only_reachable_addresses(sent.clone(), observed, None),
            [
                "prudp:/address=203.0.113.9;port=3074;type=2",
                "udp:/address=203.0.113.9;port=80",
                "prudp:/address=10.0.0.5;port=13000;RVCID=5;hdrType=0"
            ]
        );
        let trusted = Subnet::parse("10.8.0.0/16");
        let vpn: std::net::IpAddr = "10.8.1.2".parse().unwrap();
        assert_eq!(
            station_urls_for_peers(sent, vpn, trusted),
            [
                "prudp:/address=10.8.1.2;port=3074;type=2",
                "udp:/address=10.8.1.2;port=80",
                "prudp:/address=10.8.1.2;port=13000;RVCID=5;hdrType=0"
            ]
        );
        // Unchanged URLs keep their exact text (the probe check compares it).
        let own = vec!["prudp:/port=13000;address=203.0.113.9;type=2;RVCID=7".to_string()];
        assert_eq!(only_reachable_addresses(own.clone(), observed, None), own);
    }

    #[test]
    fn only_members_change_a_session_but_anyone_may_join_or_leave() {
        let (host, guest, stranger) = (10, 11, 99);
        let participants = [host, guest];
        assert!(may_change_session(host, host, &participants, None));
        assert!(may_change_session(guest, host, &participants, None), "participants may update");
        assert!(!may_change_session(stranger, host, &participants, None));
        assert!(may_change_session(stranger, host, &participants, Some(&[stranger])), "joining");
        assert!(!may_change_session(stranger, host, &participants, Some(&[guest])), "removing someone else");
        assert!(!may_change_session(stranger, host, &participants, Some(&[stranger, guest])));
        assert!(!may_change_session(stranger, host, &participants, Some(&[])));
        assert!(may_change_session(host, host, &participants, Some(&[stranger])), "the host adds anyone");
    }

    #[test]
    fn subnets_parse() {
        let s = Subnet::parse("10.8.0.0/16").unwrap();
        assert!(s.contains("10.8.255.1".parse().unwrap()));
        assert!(!s.contains("10.78.0.1".parse().unwrap()));
        assert!(Subnet::parse("0.0.0.0/0").unwrap().contains("8.8.8.8".parse().unwrap()));
        assert_eq!(Subnet::parse("10.8.0.0"), None);
        assert_eq!(Subnet::parse("10.8.0.0/33"), None);
    }

    #[test]
    fn matches_tell_their_mode() {
        use super::match_mode;
        // As the game sends them (co-op on Oceania, Spies vs Mercs on na1, 2026-10-03).
        let coop_private = "113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 0;4 => 2;101 => 3578398534;102 => 3;103 => 0;105 => 2;112 => 2";
        let coop_public = "113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 2;4 => 0;101 => 3578398534;102 => 3;103 => 0;105 => 2;112 => 2";
        let svm_private = "113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 0;4 => 8;101 => 615323303;102 => 7;103 => 0;105 => 1;112 => 3";
        let svm_classic = "113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 0;4 => 4;101 => 2423434240;102 => 8;103 => 0;105 => 1;112 => 4";
        let svm_matchmaking = "113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 4;4 => 0;101 => 72621668;102 => 8;103 => 2165463540;105 => 0;112 => 2";
        assert_eq!(match_mode(coop_private), Some("coop"));
        assert_eq!(match_mode(coop_public), Some("coop"));
        assert_eq!(match_mode(svm_private), Some("svm"));
        assert_eq!(match_mode(svm_classic), Some("svm"));
        assert_eq!(match_mode(svm_matchmaking), Some("svm"));
        assert_eq!(match_mode("113 => 1;3 => 8;4 => 0"), None, "a lobby serves either mode");
    }

    #[test]
    fn attribute_value_reports_absence() {
        assert_eq!(attribute_value("113 => 0;3 => 8", PROPERTY_ROOM_KIND), Some(0));
        assert_eq!(attribute_value("3 => 8;4 => 0", PROPERTY_ROOM_KIND), None);
        assert_eq!(attribute_value("", PROPERTY_ROOM_KIND), None);
    }

    /// A few join failures per call and a few a minute per player are taken.
    #[test]
    fn join_failures_are_limited_per_player() {
        let user = 4_000_001;
        assert_eq!(super::failed_joins_allowed(user, 8), 8);
        assert_eq!(super::failed_joins_allowed(user, 8), 8);
        assert_eq!(super::failed_joins_allowed(user, 8), 8);
        assert_eq!(super::failed_joins_allowed(user, 8), 6, "30 a minute");
        assert_eq!(super::failed_joins_allowed(user, 1), 0);
        assert_eq!(super::failed_joins_allowed(user + 1, 1), 1, "another player");
    }
}
