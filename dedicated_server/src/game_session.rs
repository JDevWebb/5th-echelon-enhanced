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
    /// The session each player last left, when, and whether by AbandonSession: a player
    /// splits the session it has just abandoned (see `split_session`), and is no longer in it
    /// then.
    left: std::sync::Mutex<std::collections::HashMap<u32, (u32, std::time::Instant, bool)>>,
    /// Sessions a host migration moved on (see `split_session`), and when: the old session,
    /// the new one.
    migrated: std::sync::Mutex<std::collections::HashMap<u32, (u32, std::time::Instant)>>,
}

/// How long after leaving a session a player may still split it.
const SPLIT_AFTER_LEAVING: std::time::Duration = std::time::Duration::from_secs(60);

/// A member's split of a session whose host was still in it, and signed in (see
/// `split_session`): the new session, the member who took it, and when. Nothing told it from a
/// member splitting off on its own, so only that member moved; the others follow once the host
/// leaves the old session or signs out, within [`SPLIT_AFTER_LEAVING`].
struct Takeover {
    to: u32,
    by: u32,
    at: std::time::Instant,
}

/// The [`Takeover`]s waiting for their host, by the session taken over.
fn takeovers() -> std::sync::MutexGuard<'static, std::collections::HashMap<u32, Takeover>> {
    static TAKEOVERS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u32, Takeover>>> = std::sync::OnceLock::new();
    TAKEOVERS.get_or_init(std::sync::Mutex::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The session a member's split of `session_id` made while its host was still in it, lately.
fn taken_over_by(session_id: u32) -> Option<(u32, u32)> {
    takeovers().get(&session_id).filter(|t| t.at.elapsed() < SPLIT_AFTER_LEAVING).map(|t| (t.to, t.by))
}

/// Notes the members of `from` that a host migration moved to `to`: they joined it, and
/// played in it if it's a match.
fn moved_along(logger: &Logger, storage: &Storage, from: u32, to: u32, moved: &[u32]) {
    joined(moved.iter().copied(), to);
    for &along in moved {
        match storage.note_match_player(to, along) {
            Ok(true) => crate::metrics::match_started(),
            Ok(false) => {}
            Err(e) => warn!(logger, "Couldn't note {along} in match {to}: {e}"),
        }
        crate::session_events::note(
            crate::session_events::Who::Id(along),
            "leave",
            serde_json::json!({ "room": from, "how": "split", "ended": false }),
        );
        crate::session_events::joined(along, to, "migration");
    }
}

/// The sessions a deferred takeover (`host_gone`) moved on, and when: the old session, the new
/// one. `host_gone` has no server object to note it in, so `migrated_to` looks here too.
fn host_migrated() -> std::sync::MutexGuard<'static, std::collections::HashMap<u32, (u32, std::time::Instant)>> {
    static MIGRATED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u32, (u32, std::time::Instant)>>> = std::sync::OnceLock::new();
    MIGRATED.get_or_init(std::sync::Mutex::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn note_host_migrated(from: u32, to: u32) {
    let mut migrated = host_migrated();
    let now = std::time::Instant::now();
    if migrated.len() >= 10_000 {
        migrated.retain(|_, (_, at)| now.duration_since(*at) < SPLIT_AFTER_LEAVING);
    }
    migrated.insert(from, (to, now));
}

fn host_migrated_to(from: u32) -> Option<u32> {
    host_migrated().get(&from).filter(|(_, at)| at.elapsed() < SPLIT_AFTER_LEAVING).map(|(to, _)| *to)
}

/// The old host of `from` is gone and `by` hosts it as `to`: invitations and join targets
/// that named `from` name `to` now.
fn carried_over(logger: &Logger, storage: &Storage, from: u32, to: u32, by: u32) {
    crate::join_targets::moved(from, to);
    match storage.rebind_invites(from, to, by) {
        Ok(0) => {}
        Ok(n) => info!(logger, "{n} pending invitations to session {from} now name {to}"),
        Err(e) => warn!(logger, "Couldn't move the invitations to session {from} on to {to}: {e}"),
    }
}

/// `host` is out of its sessions (it left `only`, or every one when it signed out): the
/// members a takeover of one of them left behind follow the member who took it over.
/// Migration notifications to send once there's a socket to send them on: the members a
/// deferred takeover (`host_gone`) moves along are told by [`deliver_migration_notes`] on the
/// server's next sweep.
static MIGRATION_NOTES: std::sync::Mutex<Vec<(u32, NotificationEvent, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());

/// How long a migration notification waits for its member to be connected (a reconnect) before
/// it's dropped.
const MIGRATION_NOTE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether an undelivered notification of the given age is tried again on the next sweep.
fn note_still_due(age: std::time::Duration) -> bool {
    age < MIGRATION_NOTE_TTL
}

fn migration_event(new_host: u32, old_session: u32, new_session: u32) -> NotificationEvent {
    NotificationEvent {
        pid_source: new_host,
        ui_type: 7001,
        ui_param_1: new_session,
        ui_param_2: old_session,
        str_param: String::new(),
        ui_param_3: 0,
    }
}

/// Sends the migration notifications `host_gone` queued.
/// A host that has said nothing for this long is gone: a takeover waiting for it goes through.
/// Its pings come every ten seconds, so a host that's merely idle stays well under it.
const HOST_SILENT_AFTER: std::time::Duration = std::time::Duration::from_secs(12);

/// Lets the takeovers whose host has gone quiet (or has no connection left) go through now,
/// instead of when its connection finally expires.
pub fn resolve_silent_hosts<CI>(logger: &Logger, storage: &Storage, registry: &ClientRegistry<CI>) {
    let waiting: Vec<u32> = {
        let mut waiting = takeovers();
        waiting.retain(|_, t| t.at.elapsed() < SPLIT_AFTER_LEAVING);
        waiting.keys().copied().collect()
    };
    for from in waiting {
        let Some(host) = storage.session_creator(from).ok().flatten() else { continue };
        if registry.user_silence(host).is_none_or(|quiet| quiet >= HOST_SILENT_AFTER) {
            info!(logger, "Host {host} of session {from} has been silent; the takeover of it goes through");
            host_gone(logger, storage, host, Some(from));
        }
    }
}

/// Whether a host silent for `silence` (`None`: no connection to tell, which is no proof) has
/// gone.
fn host_silent(silence: Option<std::time::Duration>) -> bool {
    silence.is_some_and(|quiet| quiet >= HOST_SILENT_AFTER)
}

/// Whether the lobby `session_id` of `creator` isn't worth offering a searching player: a host
/// migration moved it on (its host is gone), or its host has said nothing for
/// [`HOST_SILENT_AFTER`] (cut off, or about to be): a join would only fail, or land in a room
/// that migrates a moment later. A host without a connection to look at (nobody's) is offered.
pub fn lobby_is_dead<CI>(registry: &ClientRegistry<CI>, session_id: u32, creator: u32) -> bool {
    host_migrated_to(session_id).is_some() || host_silent(registry.user_silence(creator))
}

pub fn deliver_migration_notes<CI>(logger: &Logger, ctx: &Context, socket: &std::net::UdpSocket, registry: &ClientRegistry<CI>) {
    let notes: Vec<(u32, NotificationEvent, std::time::Instant)> = std::mem::take(&mut *MIGRATION_NOTES.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    for (member, event, queued) in notes {
        if registry.client_by_user_id(member).is_none() {
            if note_still_due(queued.elapsed()) {
                // Reconnecting: keep the note for the next sweep.
                MIGRATION_NOTES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((member, event, queued));
            } else {
                info!(logger, "No connected client for {member} within {MIGRATION_NOTE_TTL:?} - migration notification dropped");
            }
            continue;
        }
        info!(
            logger,
            "Notifying {member} that session {} moved to {} under {}: {event:?}", event.ui_param_2, event.ui_param_1, event.pid_source
        );
        if let Some(cell) = registry.client_by_user_id(member) {
            if let Err(e) = quazal::rmc::call_client(
                logger,
                ctx,
                socket,
                &mut cell.borrow_mut(),
                NOTIFICATION_PROTOCOL_ID,
                PROCESS_NOTIFICATION_EVENT,
                event.to_bytes(),
                true,
            ) {
                error!(logger, "Migration notification to {member} failed: {e}");
            }
        }
    }
}

fn host_gone(logger: &Logger, storage: &Storage, host: u32, only: Option<u32>) {
    let due: Vec<(u32, Takeover)> = {
        let mut waiting = takeovers();
        waiting.retain(|_, t| t.at.elapsed() < SPLIT_AFTER_LEAVING);
        let sessions: Vec<u32> = waiting
            .keys()
            .copied()
            .filter(|s| only.is_none_or(|o| o == *s) && storage.session_creator(*s).ok().flatten() == Some(host))
            .collect();
        sessions.into_iter().filter_map(|s| waiting.remove(&s).map(|t| (s, t))).collect()
    };
    for (from, t) in due {
        match storage.move_members(from, t.to, t.by, None) {
            Ok((moved, ended)) => {
                info!(
                    logger,
                    "Host {host} is out of session {from}: user {} takes it over as {} (host migration); along: {moved:?}", t.by, t.to
                );
                note_host_migrated(from, t.to);
                carried_over(logger, storage, from, t.to, t.by);
                moved_along(logger, storage, from, t.to, &moved);
                let mut notes = MIGRATION_NOTES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                notes.extend(
                    moved
                        .iter()
                        .filter(|m| **m != t.by)
                        .map(|&m| (m, migration_event(t.by, from, t.to), std::time::Instant::now())),
                );
                drop(notes);
                if ended {
                    crate::session_events::room_ended(from);
                }
            }
            Err(e) => error!(logger, "Couldn't move the members of session {from} on to {}: {e}", t.to),
        }
    }
}

/// `user_id` signed out (its game connection closed or expired): a match or party it hosted
/// that a member took over carries on with everyone in it. Call before its sessions end.
pub fn host_signed_out(logger: &Logger, storage: &Storage, user_id: u32) {
    host_gone(logger, storage, user_id, None);
}

/// `user_id`'s game dropped and signed in again from the same address, and its old connection
/// is gone: a match or party that connection made, which a member took over, carries on with
/// everyone in it instead of waiting for the takeover to lapse. Call before the rooms end.
pub fn host_left_rooms(logger: &Logger, storage: &Storage, user_id: u32, rooms: &[u32]) {
    for &room in rooms {
        host_gone(logger, storage, user_id, Some(room));
    }
}

/// `leader` opened the party `party` (a room of kind 1): if it boarded a private match whose
/// members then came after it, and that match is over (dissolved, or the leader out of it), the
/// members who came are asked into the party, each with an invitation from the leader that their
/// game takes up without a click (as `JoinNow` makes it). The game does not take them back: it
/// has the leader leave its party for the match and host a new one afterwards, and the members
/// split the old one among themselves. Once per trip (`escorts`). Call from a thread that may
/// block.
pub fn bring_party_back(logger: &Logger, storage: &std::sync::Arc<Storage>, leader: u32, type_id: u32, party: u32) {
    let members = party_back_members(logger, storage, leader, party);
    if members.is_empty() {
        return;
    }
    // The member's game is still leaving the match when the match ends: an invitation that
    // reaches it then is accepted and lost (it never sends the join). So wait a moment, and ask
    // again if the member hasn't come into the party by then. Not on the caller's thread: that is
    // the leader's CreateSession.
    let (logger, storage) = (logger.clone(), std::sync::Arc::clone(storage));
    let spawned = std::thread::Builder::new().name("party-back".into()).spawn({
        let logger = logger.clone();
        move || ask_back(&logger, &storage, leader, type_id, party, &members, PARTY_BACK_DELAY, PARTY_BACK_RETRY, PARTY_BACK_TRIES)
    });
    if let Err(e) = spawned {
        warn!(logger, "Couldn't start the thread that asks {leader}'s party back into {party}: {e}");
    }
}

/// The members `leader`'s party takes back into the party `party`: those who came along into a
/// match that is over now, and may be asked (online, not blocked, not too fast).
fn party_back_members(logger: &Logger, storage: &Storage, leader: u32, party: u32) -> Vec<u32> {
    let over = |room: u32| match storage.session_members(room) {
        Ok(None) => true,
        Ok(Some((creator, participants))) => creator != leader && !participants.contains(&leader) && !storage.is_guest(room, leader).unwrap_or(false),
        Err(_) => false,
    };
    let mut members = vec![];
    for member in crate::escorts::back(leader, over) {
        if !storage.is_online(member).unwrap_or(false) || crate::friends_policy::blocked_blocking(storage, leader, member) {
            info!(
                logger,
                "The match of {leader}'s party is over: {member} isn't asked back into party {party} (offline or blocked)"
            );
            continue;
        }
        if !crate::rate_limit::invites().check(leader) {
            warn!(logger, "{leader} brings its party back too fast; {member} isn't asked into party {party}");
            continue;
        }
        members.push(member);
    }
    members
}

/// The live private match (`113 == 0`) among `rooms` (id, host, attributes) that isn't the
/// leader's own: a member in it is still playing and must not be called out of it.
fn match_still_running(rooms: &[(u32, u32, String)], leader: u32) -> Option<u32> {
    rooms
        .iter()
        .find(|(_, creator, attributes)| *creator != leader && attribute_value(attributes, PROPERTY_ROOM_KIND) == Some(0))
        .map(|(id, _, _)| *id)
}

/// Asks `members` into the party `party` with a requested invitation from `leader`, after `delay`,
/// and again every `retry` (up to `tries` times in all) while one hasn't come.
fn ask_back(logger: &Logger, storage: &Storage, leader: u32, type_id: u32, party: u32, members: &[u32], delay: std::time::Duration, retry: std::time::Duration, tries: u32) {
    std::thread::sleep(delay);
    for attempt in 1..=tries {
        let mut missing = false;
        for &member in members {
            let in_party = storage
                .session_members(party)
                .ok()
                .flatten()
                .is_some_and(|(creator, participants)| creator == member || participants.contains(&member));
            if in_party {
                continue;
            }
            missing = true;
            // A member that is still in somebody else's running match (the leader was cut off
            // without a leave, and the member plays on) isn't pulled out of it.
            if let Some(room) = storage.live_rooms_of(member).ok().and_then(|rooms| match_still_running(&rooms, leader)) {
                info!(
                    logger,
                    "The match of {leader}'s party is over: asking {member} back into party {party} skipped: {member} is still in match session {room} (try {attempt})"
                );
                continue;
            }
            match storage.add_return_invite(leader, member, type_id, party) {
                Ok(invite) => {
                    crate::escorts::mark_return_invite(invite);
                    info!(
                        logger,
                        "The match of {leader}'s party is over: {member} is asked back into party {party} (invitation {invite}, requested, try {attempt})"
                    );
                }
                Err(e) => warn!(logger, "Couldn't ask {member} back into party {party}: {e}"),
            }
        }
        if !missing || attempt == tries {
            break;
        }
        std::thread::sleep(retry);
    }
}

/// How long the leader's party waits for a member's game to be done with the match before the
/// member is asked back into the party, and how long after an invitation before asking again.
/// The retry is long on purpose: a member's game that is still busy with the first invitation (a
/// join after a migration took 12 s, live) takes a second one as a new accept and leaves the party
/// it just joined.
const PARTY_BACK_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
const PARTY_BACK_RETRY: std::time::Duration = std::time::Duration::from_secs(15);
const PARTY_BACK_TRIES: u32 = 3;

/// Property that tells the two rooms of a private match apart.
///
/// A host who opened a private match is in two sessions at once: the anteroom every client
/// opens on entering multiplayer, and the configured match room itself. Both name the same
/// host and live in the same session type - this property is the only thing distinguishing
/// them.
const PROPERTY_ROOM_KIND: u32 = 113;
/// The game's data version (`server_api::data_versions`): games whose versions differ can't
/// join each other.
pub(crate) const PROPERTY_DATA_VERSION: u32 = 107;

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

/// Whether the room an invitee is about to be put into is full: its members and seats
/// (`Storage::room_seats`) when they've taken every seat, so the invitee has none. Not for one
/// who already has a seat (a guest of it), nor a room without seats (nothing to compare
/// with) or one that's gone. Seats are whatever the host last sent, never a size per mode.
fn full_for_invitee(has_seat: bool, room: Option<(u32, u32)>) -> Option<(u32, u32)> {
    room.filter(|&(members, seats)| !has_seat && seats > 0 && members >= seats)
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

/// The players a match's mode is made for, from the game's own playlists (`maxPlayers`): by
/// its playlist (attribute 103, set by matchmaking) when it's one of them, else by its game
/// mode (102). None for a lobby, or a mode not known here.
///
/// A host can raise a match past it (the overlay's "Most players"); above it some players'
/// games can crash or freeze, so the overlay warns.
pub(crate) fn mode_size(attributes: &str) -> Option<u32> {
    if attribute_value(attributes, PROPERTY_ROOM_KIND) != Some(0) {
        return None;
    }
    let by_playlist = match attribute_value(attributes, 103) {
        Some(2_165_463_540) => Some(4),                                             // Classic
        Some(4_008_726_265 | 709_003_443 | 655_282_787 | 1_365_420_058) => Some(8), // Blacklist, Team Deathmatch, Capture, Training Grounds
        Some(4_078_930_099 | 1_487_234_637) => Some(6),                             // Frontline, Training Grounds
        _ => None,
    };
    by_playlist.or(match attribute_value(attributes, 102)? {
        3..=5 => Some(2),      // co-op
        6 | 7 | 10 => Some(8), // Team Deathmatch, Blacklist, Capture
        8 => Some(4),          // Classic
        9 => Some(6),          // Frontline
        _ => None,
    })
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

    /// `leader` accepted an invitation into private match `room` of `host`: notes the members
    /// of the party it hosts, who may then come into the match after it (`escorts`): the leader's
    /// game tells them to over Storm (hook `party_follow`, always on), and the host or they themselves
    /// add them. Without the note they are strangers to the host and refused. Not for a public
    /// match, which the game doesn't take a party into either.
    fn board_party(&self, logger: &Logger, leader: u32, host: u32, room: u32) {
        if !self.is_private_room(1, room) {
            return;
        }
        let companions: Vec<u32> = match self.storage.hosted_parties(leader) {
            Ok(parties) => parties.into_iter().flat_map(|(_, _, members)| members).filter(|m| *m != host).collect(),
            Err(e) => {
                warn!(logger, "Couldn't read {leader}'s party: {e}");
                return;
            }
        };
        if !companions.is_empty() {
            info!(logger, "{leader} accepts an invitation into private match {room} of {host}; its party: {companions:?}");
        }
        crate::escorts::board(leader, room, &companions);
    }

    /// Notes that a host migration moved session `from` on to `to`.
    fn note_migrated(&self, from: u32, to: u32) {
        let mut migrated = self.migrated.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = std::time::Instant::now();
        if migrated.len() >= 10_000 {
            migrated.retain(|_, (_, at)| now.duration_since(*at) < SPLIT_AFTER_LEAVING);
        }
        migrated.insert(from, (to, now));
        drop(migrated);
        note_host_migrated(from, to);
    }

    /// Tells the members a host migration took along that their session moved: notification
    /// 7001 (`NetOnlineSessionServiceRdv::vf28` at 0x007bdfb0) carries the new host as its
    /// source and the new session id as its first parameter, and the second parameter names the
    /// session the member is in now (`NotificationHandler::vf01` at 0x0077da30 finds the
    /// member's service object by it). A member's game waits for exactly this in step 5 of its
    /// migration (0x0089e650) until it knows the new host's session; without it the lobby
    /// stays on "Host wurde geändert. Match wird migriert ..." for good. The new host
    /// itself makes the server calls and isn't told.
    #[allow(clippy::too_many_arguments)]
    fn notify_new_host<CI>(
        &self,
        logger: &Logger,
        ctx: &Context,
        socket: &std::net::UdpSocket,
        client_registry: &ClientRegistry<CI>,
        new_host: u32,
        old_session: u32,
        new_session: u32,
        members: &[u32],
    ) {
        for &member in members.iter().filter(|m| **m != new_host) {
            let event = NotificationEvent {
                pid_source: new_host,
                ui_type: 7001,
                ui_param_1: new_session,
                ui_param_2: old_session,
                str_param: String::new(),
                ui_param_3: 0,
            };
            info!(logger, "Notifying {member} that session {old_session} moved to {new_session} under {new_host}: {event:?}");
            let payload = event.to_bytes();
            let reliable = self.debug_config.push_notifications_reliable;
            if let Some(cell) = client_registry.client_by_user_id(member) {
                if let Err(e) = quazal::rmc::call_client(
                    logger,
                    ctx,
                    socket,
                    &mut cell.borrow_mut(),
                    NOTIFICATION_PROTOCOL_ID,
                    PROCESS_NOTIFICATION_EVENT,
                    payload,
                    reliable,
                ) {
                    error!(logger, "Migration notification to {member} failed: {e}");
                }
            } else {
                info!(logger, "No connected client for {member} - no migration notification sent");
            }
        }
    }

    /// The session a host migration moved `session_id` on to, lately ([`SPLIT_AFTER_LEAVING`]).
    fn migrated_to(&self, session_id: u32) -> Option<u32> {
        let migrated = self.migrated.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        migrated
            .get(&session_id)
            .filter(|(_, at)| at.elapsed() < SPLIT_AFTER_LEAVING)
            .map(|(to, _)| *to)
            .or_else(|| host_migrated_to(session_id))
    }

    /// LeaveSession and AbandonSession: the player is no longer in the
    /// session, and one nobody is left in ends. Upstream answered both
    /// without doing anything, so players stayed listed in rooms they had
    /// left (friend searches then found stale parties, #44) and empty
    /// lobbies stayed on offer.
    fn leave(&self, logger: &Logger, user_id: u32, session_id: u32, verb: &str) -> Result<(), Error> {
        crate::escorts::left(user_id, session_id);
        let Some(ended) = rmc_err!(self.storage.leave_game_session(user_id, session_id), logger, "error leaving session")? else {
            // Usual, not suspicious: a host taking its party along or ending a match removes
            // the guests first, and their own leave arrives a moment later.
            info!(logger, "User {user_id} {verb} session {session_id}, which they aren't in; nothing changes");
            return Ok(());
        };
        let mut left = self.left.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = std::time::Instant::now();
        if left.len() >= 10_000 {
            left.retain(|_, (_, at, _)| now.duration_since(*at) < SPLIT_AFTER_LEAVING);
        }
        left.insert(user_id, (session_id, now, verb == "abandons"));
        drop(left);
        let host_left = !ended && self.storage.session_creator(session_id).ok().flatten() == Some(user_id);
        if host_left {
            host_gone(logger, &self.storage, user_id, Some(session_id));
        }
        info!(
            logger,
            "User {user_id} {verb} session {session_id}{}",
            if ended {
                "; nobody left, it ends"
            } else if host_left {
                "; the others in it carry on"
            } else {
                ""
            }
        );
        let how = match verb {
            "abandons" => "abandoned",
            "splits off from" => "split",
            _ => "left",
        };
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

        let (type_id, is_party) = (request.game_session.type_id, request.game_session.attributes.0.iter().any(|p| p.id == 113 && p.value == 1));
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
        // A party opened after a private match the leader took its party into: the members who
        // went along are asked into it (`escorts`).
        if is_party {
            bring_party_back(logger, &self.storage, user_id, type_id, session_id);
        }
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
        let session_id = request.game_session_key.session_id;
        if rmc_err!(
            self.storage.delete_game_session(user_id, request.game_session_key.type_id, session_id),
            logger,
            "error deleting session"
        )? != 1
        {
            warn!(logger, "Unexpected amount of sessions deleted");
            return Ok(DeleteSessionResponse);
        }
        // The host ended it (a match's end): Sessions has their stay end here, not when their
        // game next times out, and the match as ended for whoever was still in it.
        info!(logger, "User {user_id} ends session {session_id}");
        crate::session_events::note(
            crate::session_events::Who::Id(user_id),
            "leave",
            serde_json::json!({ "room": session_id, "how": "left", "ended": true }),
        );
        crate::session_events::room_ended(session_id);
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
        // block. So does a companion of a leader invited into it (`escorts`): the leader's
        // game sent it after the leader.
        let escort_self = targets.contains(&user_id) && !is_member && crate::escorts::allowed(session_id, user_id);
        if targets.contains(&user_id) && !is_member && self.is_private_room(request.game_session_key.type_id, session_id) {
            let invited = rmc_err!(self.storage.is_invited(user_id, session_id), logger, "error checking invitations")?;
            let party_of_host = members
                .as_ref()
                .is_some_and(|(creator, _)| self.in_party_before(user_id, *creator, session_id) && !crate::friends_policy::blocked_blocking(&self.storage, user_id, *creator));
            let escorted = escort_self
                && members
                    .as_ref()
                    .is_some_and(|(creator, _)| !crate::friends_policy::blocked_blocking(&self.storage, user_id, *creator));
            if !invited && !party_of_host && !escorted {
                warn!(logger, "User {user_id} tried to join private room {session_id} without an invitation; refused");
                return Err(Error::AccessDenied);
            }
        }
        // Adding someone else puts them into a session, and a private match nudges them in
        // (below). Only a host taking their own party along may (friends or not, as anyone
        // in their anteroom), or a friend, or a companion of a leader invited into this
        // match (`escorts`) - never past a block, and never a stranger, whom a public room
        // the two once shared made no member of anything.
        if let Some(other) = targets.iter().find(|&&t| {
            t != user_id
                && (crate::friends_policy::blocked_blocking(&self.storage, user_id, t)
                    || !(self.in_party_of(t, user_id) || crate::friends_policy::friends_blocking(&self.storage, user_id, t) || crate::escorts::allowed(session_id, t)))
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
        // A member that split its party's session while the leader was still in it and now
        // adds itself to another session went somewhere else (it accepted an invitation, its
        // game having left the party): it has taken nothing over, so the party's others must
        // not follow it should the leader leave later (see `split_session`). Its own
        // session, the one it split off, is where its game adds itself after a real takeover.
        if targets.contains(&user_id) {
            takeovers().retain(|_, t| t.by != user_id || t.to == session_id);
        }

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
        for &target in &targets {
            let there = members.as_ref().is_some_and(|(creator, participants)| *creator == target || participants.contains(&target));
            if let Some(leader) = crate::escorts::boarded_by(session_id, target).filter(|_| !there && self.is_private_room(request.game_session_key.type_id, session_id)) {
                info!(logger, "{target} follows its party leader {leader} into private match {session_id}");
                crate::escorts::came(session_id, target);
            }
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
                    // Invitation route (or a companion following its leader) only. Otherwise the
                    // HOST notifies itself as well when it adds itself while opening the private
                    // match, opening its own `[0x42A]` gate - a join gate on the host side is
                    // pointless at best.
                    if via_invitation || escort_self {
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
        let mut members = None;
        if request.participant_ids.0.iter().any(|&t| t != user_id) {
            members = rmc_err!(self.storage.session_members(request.game_session_key.session_id), logger, "error reading session members")?;
            if members.as_ref().is_some_and(|(creator, _)| *creator != user_id) && self.debug_config.session_owner_checks {
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
        // Only those still in it: the host's game also removes a guest who just left, which
        // isn't the host removing them (their leave is noted already).
        let still_in = |id: u32| members.as_ref().is_none_or(|(_, participants)| participants.contains(&id));
        for &removed in request.participant_ids.0.iter().filter(|&&t| t != user_id && still_in(t)) {
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
        //
        // Not into a room full at its current size (its seats, attributes 3 and 4, as its
        // host last sent them): a host can invite more players than it has seats, and the
        // first to accept takes the last one. Added anyway, the next was counted in a match
        // whose host then refused them (FANTOMAS, eu1 2026-10-08: Daramos's 2-seat room
        // after Saulet). Answered as it is, their game tries and is told the room is full.
        if let Some(room) = invited.as_ref().filter(|room| !room.participants.iter().any(|p| p.user_id == user_id)) {
            // A guest of it already has their seat.
            let has_seat = rmc_err!(self.storage.is_guest(room.session_id, user_id), logger, "error reading session guests")?;
            let seats = rmc_err!(self.storage.room_seats(room.session_id), logger, "error reading the invited room's seats")?;
            if let Some((members, seats)) = full_for_invitee(has_seat, seats) {
                info!(
                    logger,
                    "Not adding {user_id} to invited room {} before answering the search: it's full ({members} in {seats} seats)", room.session_id
                );
            } else {
                info!(logger, "Adding {user_id} to invited room {} before answering the search", room.session_id);
                rmc_err!(
                    self.storage.add_participants(room.session_type, room.session_id, vec![user_id], vec![]),
                    logger,
                    "error adding the invited player"
                )?;
                joined([user_id], room.session_id);
                self.note_match_players(logger, room.session_id, [user_id]);
                self.board_party(logger, user_id, room.creator_id, room.session_id);
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
                crate::join_targets::invited(user_id, invited.session_id);

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
        // A session that migrated already, split again by one of the players it took along
        // (their game running the migration too): the same new session, not another one.
        // So for one a member took over while its host was still in it: this player's game
        // follows the new host, so it moves along now.
        if let Some((to, by)) = taken_over_by(key.session_id) {
            if by != user_id {
                let (moved, ended) = rmc_err!(self.storage.move_members(key.session_id, to, by, Some(user_id)), logger, "error moving a member along")?;
                let along = !moved.is_empty()
                    || rmc_err!(self.storage.session_members(to), logger, "error reading session members")?.is_some_and(|(_, p)| p.contains(&user_id))
                    || rmc_err!(self.storage.is_guest(to, user_id), logger, "error reading session guests")?;
                if along {
                    info!(logger, "User {user_id} splits session {}, which {by} took over as {to}; answered with {to}", key.session_id);
                    moved_along(logger, &self.storage, key.session_id, to, &moved);
                    if ended {
                        crate::session_events::room_ended(key.session_id);
                    }
                    return Ok(SplitSessionResponse {
                        game_session_key_migrated: GameSessionKey {
                            type_id: key.type_id,
                            session_id: to,
                        },
                    });
                }
            }
        }
        if let Some(to) = self.migrated_to(key.session_id) {
            let along = rmc_err!(self.storage.session_members(to), logger, "error reading session members")?.is_some_and(|(_, p)| p.contains(&user_id))
                || rmc_err!(self.storage.is_guest(to, user_id), logger, "error reading session guests")?;
            if along {
                info!(logger, "User {user_id} splits session {}, which migrated to {to}; answered with {to}", key.session_id);
                return Ok(SplitSessionResponse {
                    game_session_key_migrated: GameSessionKey {
                        type_id: key.type_id,
                        session_id: to,
                    },
                });
            }
        }
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
        let (just_left, abandoned_it) = self
            .left
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&user_id)
            .filter(|(session, at, _)| *session == key.session_id && at.elapsed() < SPLIT_AFTER_LEAVING)
            .map_or((false, false), |(_, _, abandoned)| (true, *abandoned));
        let creator = rmc_err!(self.storage.session_creator(key.session_id), logger, "error reading session")?;
        // A guest (who joined a public match from a search) is in it as much as a participant:
        // their game splits the match to carry on when its host leaves (eu1: 4 such splits
        // refused in a week, 3 in one match). So is anyone who was in it before: the game takes
        // players off a party's participants while they're still in its party, and splits it
        // later (eu1 2026-10-12: five members of one party refused within a second).
        let live_members = rmc_err!(self.storage.session_members(key.session_id), logger, "error reading session members")?;
        let guest = rmc_err!(self.storage.is_guest(key.session_id, user_id), logger, "error reading session guests")?;
        // In it now, and it's live (`session_members` knows only live sessions).
        let in_it = live_members.as_ref().is_some_and(|(_, p)| guest || p.contains(&user_id));
        let member = just_left || creator == Some(user_id) || in_it || guest || rmc_err!(self.storage.was_member(key.session_id, user_id), logger, "error reading past members")?;
        if creator.is_some() && !member {
            warn!(logger, "User {user_id} splits session {} they aren't in; refused", key.session_id);
            return Err(Error::AccessDenied);
        }

        // Someone in a live session that isn't theirs, splitting it: its host is gone (or
        // going), and Storm made them the new one. Their game's migration (`StateMigrateHost`,
        // then `StateMigrateHostAddParticipant`, which adds nobody but itself) is all the server
        // hears; the others follow the new host over Storm without a call of their own, so they
        // move along here. The game abandons the session first when its online service still
        // holds a session of its own (`StateAbandon` before `StateMigrateHost`): a split right
        // after an AbandonSession is a migration too. Not one after LeaveSession or
        // RemoveParticipants (a player leaving to split the session for themselves).
        //
        // Only once its host is out of it, or has no game signed in: a host still there is
        // either one whose leave hasn't arrived yet, or one a member is splitting off from on
        // its own. Then only the member moves, and the others follow when the host leaves or
        // signs out (`host_gone`), or when their own game splits it too (above). A member who
        // abandoned it while its host is still there splits off alone.
        let not_theirs = creator.is_some_and(|c| c != user_id);
        let host_here = match creator {
            Some(c) if not_theirs && (in_it || abandoned_it) => {
                let host_in =
                    live_members.as_ref().is_some_and(|(_, p)| p.contains(&c)) || rmc_err!(self.storage.is_guest(key.session_id, c), logger, "error reading session guests")?;
                host_in && rmc_err!(self.storage.is_online(c), logger, "error reading who is online")?
            }
            _ => false,
        };
        let takes_over = not_theirs && (in_it || abandoned_it) && !host_here;
        let waits_for_host = not_theirs && in_it && host_here;
        let migrated = if takes_over {
            rmc_err!(
                self.storage.migrate_game_session(user_id, key.type_id, key.session_id),
                logger,
                "error migrating game session"
            )?
        } else {
            rmc_err!(
                self.storage.split_game_session(user_id, key.type_id, key.session_id),
                logger,
                "error splitting game session"
            )?
            .map(|session_id| (session_id, Vec::new()))
        };
        if let Some((session_id, moved)) = &migrated {
            let session_id = *session_id;
            made(ci, user_id, session_id);
            if takes_over {
                info!(
                    logger,
                    "User {user_id} takes over session {} as {session_id} (host migration); along: {moved:?}", key.session_id
                );
                self.note_migrated(key.session_id, session_id);
                carried_over(logger, &self.storage, key.session_id, session_id, user_id);
                moved_along(logger, &self.storage, key.session_id, session_id, moved);
                self.notify_new_host(logger, _ctx, _socket, _client_registry, user_id, key.session_id, session_id, moved);
            } else if waits_for_host {
                info!(
                    logger,
                    "User {user_id} splits session {} as {session_id} while its host is still in it; the others follow when the host leaves", key.session_id
                );
                let mut waiting = takeovers();
                let now = std::time::Instant::now();
                if waiting.len() >= 10_000 {
                    waiting.retain(|_, t| now.duration_since(t.at) < SPLIT_AFTER_LEAVING);
                }
                waiting.insert(
                    key.session_id,
                    Takeover {
                        to: session_id,
                        by: user_id,
                        at: now,
                    },
                );
            }
            // The game moves into the copy and never leaves the session it split: it stayed
            // live, its departed host's, until that host signed out, and the launcher's and
            // overlay's lists showed it beside the new one (eu1 2026-10-13: 15 such, one for
            // 4 h 20 min).
            self.leave(logger, user_id, key.session_id, "splits off from")?;
        }
        Ok(SplitSessionResponse {
            game_session_key_migrated: match migrated {
                Some((session_id, _)) => GameSessionKey { type_id: key.type_id, session_id },
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
            #[allow(clippy::cast_sign_loss)]
            let code = failed.error_code as u32;
            let reported = failed.session_key.session_id;
            let join = match explained_just_before(user_id, code) {
                Some(earlier) => FailedJoin::follow_up(&earlier, reported),
                None => failed_join(&self.storage, user_id, reported, code),
            };
            if join.told() {
                crate::metrics::failed_join();
                // Its host's game turned the joiner away as under way: Join stops leading
                // there for a while (match_started.rs). Only for the room the server just gave
                // this player (an invitation, a search, a probe), someone else's and live: the
                // game names any session id it likes, and a made-up report mustn't close a
                // match to everyone's Join.
                if code == crate::reports::LOCKED_SESSION
                    && crate::join_targets::target(user_id, |host| self.storage.public_room_for(host, user_id).ok().flatten()) == Some(join.room)
                {
                    // Named, so a player making reports up can be found.
                    if crate::match_started::refused(join.room, user_id) {
                        info!(logger, "Match {} counts as started: {user_id} was turned away as locked", join.room);
                    } else {
                        warn!(logger, "{user_id} reports too many matches locked; match {} not marked started", join.room);
                    }
                }
            }
            let noted = join
                .counts()
                .then(|| crate::reports::join_failed(user_id, join.room, code, join.outcome, join.mismatch.clone()));
            // The player hears why at once, in the game's overlay (not their game's own churn,
            // nor what they were just told); one that says all there is needn't be asked about
            // after the game.
            if join.told() {
                let problem = join.outcome.problem();
                if problem.explained() {
                    note_explained(user_id, &join);
                }
                let m = join.mismatch.clone().unwrap_or_default();
                let (title, text) = server_api::notices::join_notice(
                    problem,
                    m.host.as_deref().unwrap_or_default(),
                    &crate::reports::data_shown(m.mine),
                    &crate::reports::data_shown(m.theirs),
                );
                crate::reports::tell(user_id, title, text, noted.filter(|_| problem.explained()));
            }
            crate::session_events::note(crate::session_events::Who::Id(user_id), "join_failed", join.detail(code));
            warn!(
                logger,
                "Join failed: {user_id} did not get into session {} (type {}) - category {}, code {code:#010x}: {}{}{}",
                join.room,
                failed.session_key.type_id,
                failed.error_category,
                join.outcome.as_str(),
                if join.own {
                    ", the game's own room or one that had ended"
                } else if join.follows {
                    ", part of the failure just before"
                } else {
                    ""
                },
                join.reported_room.map(|r| format!(" (the game said session {r})")).unwrap_or_default()
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
        migrated: std::sync::Mutex::default(),
    }))
}

/// What the server makes of a join the game reported failing (`report_unsuccessful_join_sessions`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct FailedJoin {
    /// The room the join was for.
    room: u32,
    /// What the game said, when that was another room: session 0, or the party it had just
    /// split off for itself. `room` is then the one the server had just given the player
    /// (join_targets.rs).
    reported_room: Option<u32>,
    outcome: crate::reports::JoinOutcome,
    /// The room is the player's own, or had already ended: the game's own churn as it
    /// leaves one room for another, not a join that failed.
    own: bool,
    /// For a data mismatch: both sides' data.
    mismatch: Option<crate::reports::Mismatch>,
    /// The game's TICKET_PENDING straight after a failure it was told about (the room closing
    /// as they joined): part of that one, on its room, with its outcome.
    follows: bool,
}

impl FailedJoin {
    /// Whether it counts as the player's failed join, for their report prompt.
    fn counts(&self) -> bool {
        !self.own && !self.follows && self.outcome.counts()
    }

    /// Whether the player is told about it: not the game's own churn, nor a failure they
    /// were just told about.
    fn told(&self) -> bool {
        !self.own && !self.follows
    }

    /// The game's TICKET_PENDING (against `reported`) following `earlier`, the failure the
    /// player was just told about: part of it, on its room whatever room the game names.
    fn follow_up(earlier: &Explained, reported: u32) -> Self {
        FailedJoin {
            room: earlier.room,
            reported_room: (reported != earlier.room).then_some(reported),
            outcome: earlier.outcome,
            own: false,
            mismatch: None,
            follows: true,
        }
    }

    /// The `join_failed` session event's detail.
    fn detail(&self, code: u32) -> serde_json::Value {
        let mut detail = serde_json::json!({ "room": self.room, "code": format!("{code:#010x}"), "outcome": self.outcome.as_str() });
        if self.own {
            detail["own"] = true.into();
        }
        if self.follows {
            detail["follows"] = true.into();
        }
        if let Some(reported) = self.reported_room {
            detail["reported_room"] = reported.into();
        }
        if let Some(m) = &self.mismatch {
            let mut data = serde_json::Map::new();
            for (side, version) in [("mine", m.mine), ("theirs", m.theirs)] {
                if let Some(v) = version {
                    data.insert(side.into(), format!("{v:#010x}").into());
                }
            }
            detail["data"] = data.into();
            detail["mine_name"] = m.mine.and_then(server_api::data_versions::name).into();
            detail["theirs_name"] = m.theirs.and_then(server_api::data_versions::name).into();
        }
        detail
    }
}

/// Works out which room `user`'s failed join (reported against `reported`, with `code`) was
/// for, and how it went.
fn failed_join(storage: &Storage, user: u32, reported: u32, code: u32) -> FailedJoin {
    use crate::reports::JoinOutcome;
    let outcome = JoinOutcome::of(code);
    let room_of = |room: u32| storage.room_of_failed_join(room).ok().flatten();
    let reported_facts = room_of(reported);
    // The game names session 0, or the party it just split off for itself, for a join to
    // someone else's room (PlaySkill's refused join to Theusma01's room came in against
    // his own party). A party of its own locked or closed, though, is the game reopening a
    // party it had just left (Renegade's, 3 s after it closed): its own churn.
    let own_party = reported_facts.as_ref().is_some_and(|(creator, _, _)| *creator == user);
    let elsewhere = reported == 0 || (own_party && !matches!(outcome, JoinOutcome::Locked | JoinOutcome::Closed));
    let given = || crate::join_targets::target(user, |host| storage.public_room_for(host, user).ok().flatten());
    let target = elsewhere
        .then(&given)
        .flatten()
        .filter(|&room| room != reported)
        .and_then(|room| room_of(room).filter(|(creator, _, _)| *creator != user).map(|facts| (room, facts)));
    let moved = target.is_some();
    let (room, reported_room, facts) = match target {
        Some((room, facts)) => (room, Some(reported), Some(facts)),
        None => (reported, None, reported_facts),
    };
    let (creator, ended, attributes) = facts.map_or((None, false, String::new()), |(c, e, a)| (Some(c), e, a));
    let own = creator == Some(user) || ended;
    let sides = || {
        // Someone else's room: theirs is the data its host's game has. Only for the room the
        // server just gave this player (an invitation, a search, a probe): the game names any
        // session id it likes, and this goes back to the player in their summary, so any other
        // room would tell them who hosts it (a private match, say) and with what mods.
        let host = creator.filter(|&c| c != user && (moved || given() == Some(room)));
        crate::reports::Mismatch {
            host: host.and_then(|h| storage.find_username_by_user_id(h).ok().flatten()),
            mine: storage.newest_room_attributes(user).ok().flatten().and_then(|a| attribute_value(&a, PROPERTY_DATA_VERSION)),
            theirs: host.and_then(|_| attribute_value(&attributes, PROPERTY_DATA_VERSION)),
        }
    };
    let (outcome, mismatch) = match outcome {
        JoinOutcome::DataMismatch => (outcome, Some(sides())),
        // A join that just failed between games with different data is the mod's doing,
        // whatever the game named it: a PEC.ini mod joining a stock host got CONNECTION_FAILED.
        JoinOutcome::Failed if !own => match sides() {
            m @ crate::reports::Mismatch {
                mine: Some(a), theirs: Some(b), ..
            } if a != b => (JoinOutcome::DataMismatch, Some(m)),
            _ => (outcome, None),
        },
        _ => (outcome, None),
    };
    FailedJoin {
        room,
        reported_room,
        outcome,
        own,
        mismatch,
        follows: false,
    }
}

/// A failed join the player was just told about, for the game's TICKET_PENDING after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Explained {
    room: u32,
    outcome: crate::reports::JoinOutcome,
}

/// How soon after a failure the player was told about the game's TICKET_PENDING is part of
/// it: it comes in the same second (the room closing as they joined: LEAVING, then
/// TICKET_PENDING; 3 of 7 join-failure logs).
const FOLLOWS_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);

/// Each player's last failed join they were told about, and when.
type ExplainedLately = std::collections::HashMap<u32, (std::time::Instant, Explained)>;

fn explained_lately() -> std::sync::MutexGuard<'static, ExplainedLately> {
    static LATELY: std::sync::OnceLock<std::sync::Mutex<ExplainedLately>> = std::sync::OnceLock::new();
    LATELY.get_or_init(Default::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `user` was just told about `join`'s failure (one that says all there is).
fn note_explained(user: u32, join: &FailedJoin) {
    let mut lately = explained_lately();
    if lately.len() >= 10_000 {
        lately.retain(|_, (at, _)| at.elapsed() < FOLLOWS_WITHIN);
    }
    lately.insert(
        user,
        (
            std::time::Instant::now(),
            Explained {
                room: join.room,
                outcome: join.outcome,
            },
        ),
    );
}

/// The failure `user` was just told about that their game's `code` is part of: a
/// TICKET_PENDING within [`FOLLOWS_WITHIN`] of it. A TICKET_PENDING on its own is a failure.
fn explained_just_before(user: u32, code: u32) -> Option<Explained> {
    if code != crate::reports::TICKET_PENDING {
        return None;
    }
    explained_lately().get(&user).filter(|(at, _)| at.elapsed() < FOLLOWS_WITHIN).map(|(_, e)| *e)
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

    /// A player whose game splits a match its host left moves into the copy and out of the
    /// match: it ends once they were the last in it, and only the copy is live. It used to
    /// keep them and stay listed until its host signed out (eu1 2026-10-13: hours).
    #[test]
    fn a_migration_note_waits_for_a_reconnect_until_its_ttl() {
        assert!(super::note_still_due(std::time::Duration::from_secs(5)));
        assert!(super::note_still_due(std::time::Duration::from_secs(59)));
        assert!(!super::note_still_due(std::time::Duration::from_secs(60)));
    }

    #[test]
    fn a_deferred_takeover_is_noted_as_migrated_for_a_later_split() {
        let (storage, dir) = crate::storage::tests::temp_storage("host-gone-migrated");
        let storage = std::sync::Arc::new(storage);
        let server = super::GameSessionProtocolServerImpl {
            storage: std::sync::Arc::clone(&storage),
            debug_config: std::sync::Arc::default(),
            left: std::sync::Mutex::default(),
            migrated: std::sync::Mutex::default(),
        };
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        let mut ids = vec![];
        for name in ["Host", "First", "Second"] {
            storage.register_user(name, "pw", Some(name)).unwrap();
            ids.push(storage.find_user_id_by_name(name).unwrap().unwrap());
        }
        let [host, first, second] = ids[..] else { unreachable!() };
        // Session ids high enough not to meet other tests' takeovers (a global).
        for _ in 0..200 {
            storage.create_game_session(host, 1, "113 => 0".into()).unwrap();
        }
        let game = storage.create_game_session(host, 1, "113 => 0;3 => 4;4 => 0".into()).unwrap();
        storage.add_participants(1, game, vec![], vec![host]).unwrap();
        storage.add_guest(game, first, false).unwrap();
        storage.add_guest(game, second, false).unwrap();
        // `first` splits while the host is still in it; the takeover waits.
        let copy = storage.split_game_session(first, 1, game).unwrap().unwrap();
        super::takeovers().insert(
            game,
            super::Takeover {
                to: copy,
                by: first,
                at: std::time::Instant::now(),
            },
        );
        super::host_left_rooms(&logger, &storage, host, &[game]);
        assert_eq!(server.migrated_to(game), Some(copy), "a later split of the old session is answered with the new one");
        assert!(storage.session_members(copy).unwrap().is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_silent_host_is_not_offered() {
        use std::time::Duration;
        assert!(!super::host_silent(None), "no connection to look at proves nothing");
        assert!(!super::host_silent(Some(Duration::from_secs(11))));
        assert!(super::host_silent(Some(Duration::from_secs(12))));
        // The pings come every ten seconds: an idle host stays under the limit.
        assert!(!super::host_silent(Some(Duration::from_secs(10))));
    }

    #[test]
    fn splitting_leaves_the_session_split() {
        let (storage, dir) = crate::storage::tests::temp_storage("split-leaves");
        let storage = std::sync::Arc::new(storage);
        let server = super::GameSessionProtocolServerImpl {
            storage: std::sync::Arc::clone(&storage),
            debug_config: std::sync::Arc::default(),
            left: std::sync::Mutex::default(),
            migrated: std::sync::Mutex::default(),
        };
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        let mut ids = vec![];
        for name in ["Host", "First", "Second"] {
            storage.register_user(name, "pw", Some(name)).unwrap();
            ids.push(storage.find_user_id_by_name(name).unwrap().unwrap());
        }
        let [host, first, second] = ids[..] else { unreachable!() };
        let live = |id: u32| run_live(&storage).contains(&id);

        // The host leaves; their only guest carries on in a copy.
        let game = storage.create_game_session(host, 1, "113 => 0;3 => 4;4 => 0".into()).unwrap();
        storage.add_participants(1, game, vec![], vec![host]).unwrap();
        storage.add_guest(game, first, false).unwrap();
        server.leave(&logger, host, game, "abandons").unwrap();
        assert!(live(game), "its guest is still in it");
        let copy = storage.split_game_session(first, 1, game).unwrap().unwrap();
        server.leave(&logger, first, game, "splits off from").unwrap();
        assert!(!live(game), "nobody left in it: ended");
        assert!(live(copy));

        // With two guests, the first to split leaves the other in it until they split too.
        let game = storage.create_game_session(host, 1, "113 => 0;3 => 4;4 => 0".into()).unwrap();
        storage.add_participants(1, game, vec![], vec![host]).unwrap();
        storage.add_guest(game, first, false).unwrap();
        storage.add_guest(game, second, false).unwrap();
        server.leave(&logger, host, game, "abandons").unwrap();
        storage.split_game_session(first, 1, game).unwrap().unwrap();
        server.leave(&logger, first, game, "splits off from").unwrap();
        assert!(live(game), "the second guest is still in it");
        storage.split_game_session(second, 1, game).unwrap().unwrap();
        server.leave(&logger, second, game, "splits off from").unwrap();
        assert!(!live(game));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A party's leader invited into someone else's private match notes its party as escorts: the
    /// members may come into that match after it. Not for a match that isn't private, nor for
    /// anyone who isn't in the leader's party.
    #[test]
    fn the_party_of_a_leader_invited_into_a_private_match_may_follow_it() {
        let (storage, dir) = crate::storage::tests::temp_storage("party-escorts");
        let storage = std::sync::Arc::new(storage);
        let server = super::GameSessionProtocolServerImpl {
            storage: std::sync::Arc::clone(&storage),
            debug_config: std::sync::Arc::default(),
            left: std::sync::Mutex::default(),
            migrated: std::sync::Mutex::default(),
        };
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        let mut ids = vec![];
        for name in ["EHost", "ELeader", "EMember", "EStray"] {
            storage.register_user(name, "pw", Some(name)).unwrap();
            let id = storage.find_user_id_by_name(name).unwrap().unwrap();
            storage.set_online(id).unwrap();
            ids.push(id);
        }
        let [host, leader, member, stray] = ids[..] else { unreachable!() };

        let party = storage.create_game_session(leader, 1, "113 => 1;3 => 8;4 => 0".into()).unwrap();
        storage.add_participants(1, party, vec![], vec![leader, member]).unwrap();
        let game = storage.create_game_session(host, 1, "113 => 0;3 => 0;4 => 8".into()).unwrap();
        storage.add_participants(1, game, vec![host], vec![]).unwrap();
        let public = storage.create_game_session(host, 1, "113 => 0;3 => 8;4 => 0".into()).unwrap();
        storage.add_participants(1, public, vec![], vec![host]).unwrap();

        server.board_party(&logger, leader, host, public);
        assert!(!crate::escorts::allowed(public, member), "a public match takes no party along");
        server.board_party(&logger, leader, host, game);
        assert!(crate::escorts::allowed(game, member), "the leader's party follows it into the private match");
        assert_eq!(crate::escorts::boarded_by(game, member), Some(leader));
        assert!(!crate::escorts::allowed(game, stray), "a stranger doesn't");
        assert!(!crate::escorts::allowed(game, leader), "nor does the leader need to");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The members who followed their leader into a private match are asked into the leader's
    /// next party once the match is over, by a requested invitation from the leader: not while the
    /// match goes on, not twice, not a member who didn't come, not one the leader blocked.
    #[test]
    fn the_members_who_followed_are_asked_into_the_leaders_new_party() {
        let (storage, dir) = crate::storage::tests::temp_storage("party-return");
        let storage = std::sync::Arc::new(storage);
        let server = super::GameSessionProtocolServerImpl {
            storage: std::sync::Arc::clone(&storage),
            debug_config: std::sync::Arc::default(),
            left: std::sync::Mutex::default(),
            migrated: std::sync::Mutex::default(),
        };
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        let mut ids = vec![];
        for name in ["RHost", "RLeader", "RMember", "RStay", "RBlocked"] {
            storage.register_user(name, "pw", Some(name)).unwrap();
            let id = storage.find_user_id_by_name(name).unwrap().unwrap();
            storage.set_online(id).unwrap();
            ids.push(id);
        }
        let [host, leader, member, stayed, blocked] = ids[..] else { unreachable!() };
        let waiting = |user: u32| crate::storage::run(storage.take_invite_async(user)).unwrap().unwrap();

        let party = storage.create_game_session(leader, 1, "113 => 1;3 => 8;4 => 0".into()).unwrap();
        storage.add_participants(1, party, vec![], vec![leader, member, stayed, blocked]).unwrap();
        let game = storage.create_game_session(host, 1, "113 => 0;3 => 0;4 => 8".into()).unwrap();
        storage.add_participants(1, game, vec![host], vec![]).unwrap();
        crate::storage::run(storage.block(leader, blocked)).unwrap().unwrap().unwrap();
        server.board_party(&logger, leader, host, game);
        storage.add_participants(1, game, vec![leader], vec![]).unwrap();
        for who in [member, blocked] {
            storage.add_participants(1, game, vec![who], vec![]).unwrap();
            crate::escorts::came(game, who);
        }

        // The match goes on (the leader is in it): a new party asks nobody.
        let next = storage.create_game_session(leader, 1, "113 => 1;3 => 8;4 => 0".into()).unwrap();
        {
            let m = super::party_back_members(&logger, &storage, leader, next);
            super::ask_back(&logger, &storage, leader, 1, next, &m, std::time::Duration::ZERO, std::time::Duration::ZERO, 1);
        }
        assert!(waiting(member).is_none(), "the match is still going");

        // The leader is out of it, the host's match goes on: over for the party.
        server.leave(&logger, leader, game, "leaves").unwrap();
        storage.leave_game_session(member, game).unwrap();
        {
            let m = super::party_back_members(&logger, &storage, leader, next);
            super::ask_back(&logger, &storage, leader, 1, next, &m, std::time::Duration::ZERO, std::time::Duration::ZERO, 1);
        }
        let invite = waiting(member).expect("the member is asked back");
        assert_eq!((invite.sender, invite.session_id), (leader, Some(next)), "from the leader, to its new party");
        assert!(crate::escorts::is_return_invite(invite.id), "delivered as a join the member requested");
        assert!(!invite.join_request, "an ordinary invitation from the host, so it holds for an invite-only party");
        assert!(waiting(stayed).is_none() && waiting(blocked).is_none(), "nobody else is asked");
        {
            let m = super::party_back_members(&logger, &storage, leader, next);
            super::ask_back(&logger, &storage, leader, 1, next, &m, std::time::Duration::ZERO, std::time::Duration::ZERO, 1);
        }
        assert!(waiting(member).is_none(), "and only once");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The decision alone: only a live private match of somebody else's keeps a member.
    #[test]
    fn a_member_in_somebody_elses_match_is_not_called_back() {
        use super::match_still_running;
        let leader = 10;
        let host = 20;
        let match_attrs = "113 => 0;3 => 0;4 => 8".to_string();
        let party_attrs = "113 => 1;3 => 8;4 => 0".to_string();
        assert_eq!(match_still_running(&[(5, host, match_attrs.clone())], leader), Some(5));
        assert_eq!(match_still_running(&[(4, host, party_attrs.clone()), (5, host, match_attrs.clone())], leader), Some(5));
        assert_eq!(match_still_running(&[(5, leader, match_attrs)], leader), None, "the leader's own match");
        assert_eq!(match_still_running(&[(6, host, party_attrs)], leader), None, "a party is no match");
        assert_eq!(match_still_running(&[], leader), None, "in none");
    }

    /// With real storage: a member still in the host's live match isn't asked, one whose match
    /// was destroyed is, and one who leaves the match between try 1 and try 2 is asked at try 2.
    #[test]
    fn asking_back_waits_while_the_member_is_still_in_the_match() {
        let (storage, dir) = crate::storage::tests::temp_storage("party-return-in-match");
        let storage = std::sync::Arc::new(storage);
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        let mut ids = vec![];
        for name in ["MHost", "MLeader", "MMember", "MOther"] {
            storage.register_user(name, "pw", Some(name)).unwrap();
            let id = storage.find_user_id_by_name(name).unwrap().unwrap();
            storage.set_online(id).unwrap();
            ids.push(id);
        }
        let [host, leader, member, other] = ids[..] else { unreachable!() };
        let waiting = |user: u32| crate::storage::run(storage.take_invite_async(user)).unwrap().unwrap();
        let game = storage.create_game_session(host, 1, "113 => 0;3 => 0;4 => 8".into()).unwrap();
        storage.add_participants(1, game, vec![host, member, other], vec![]).unwrap();
        let next = storage.create_game_session(leader, 1, "113 => 1;3 => 8;4 => 0".into()).unwrap();
        let zero = std::time::Duration::ZERO;

        // Still in the host's live match: not asked, however often it tries.
        super::ask_back(&logger, &storage, leader, 1, next, &[member], zero, zero, 3);
        assert!(waiting(member).is_none(), "no call in the middle of the match");

        // Leaves between try 1 and try 2 (the retry waits): asked at try 2.
        let leaver = {
            let storage = std::sync::Arc::clone(&storage);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                storage.leave_game_session(member, game).unwrap();
            })
        };
        super::ask_back(&logger, &storage, leader, 1, next, &[member], zero, std::time::Duration::from_millis(100), 5);
        leaver.join().unwrap();
        assert!(waiting(member).is_some(), "asked once out of the match");

        // The match is gone altogether: asked.
        storage.leave_game_session(host, game).unwrap();
        storage.leave_game_session(other, game).unwrap();
        super::ask_back(&logger, &storage, leader, 1, next, &[other], zero, zero, 1);
        assert!(waiting(other).is_some(), "a destroyed match holds nobody");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn run_live(storage: &crate::storage::Storage) -> Vec<u32> {
        storage.presence().unwrap().1.into_iter().map(|s| s.id).collect()
    }

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
    fn a_match_knows_the_size_its_mode_is_made_for() {
        use super::mode_size;
        let coop_private = "113 => 0;3 => 0;4 => 2;102 => 3;103 => 0;105 => 2";
        let coop_raised = "113 => 0;3 => 0;4 => 6;102 => 3;103 => 0;105 => 2";
        let svm_private = "113 => 0;3 => 0;4 => 8;102 => 7;103 => 0;105 => 1";
        let svm_raised = "113 => 0;3 => 10;4 => 0;102 => 7;103 => 0;105 => 1";
        let classic = "113 => 0;3 => 4;4 => 0;102 => 8;103 => 2165463540;105 => 0";
        let training = "113 => 0;3 => 6;4 => 0;102 => 7;103 => 1487234637;105 => 0";
        assert_eq!(mode_size(coop_private), Some(2));
        assert_eq!(mode_size(coop_raised), Some(2), "the seats a host set aren't the mode's size");
        assert_eq!(mode_size(svm_private), Some(8));
        assert_eq!(mode_size(svm_raised), Some(8));
        assert_eq!(mode_size(classic), Some(4));
        assert_eq!(mode_size(training), Some(6), "a playlist's own size wins over its game mode's");
        assert_eq!(mode_size("113 => 0;102 => 9"), Some(6));
        assert_eq!(mode_size("113 => 1;3 => 8;4 => 0;102 => 7"), None, "a lobby");
        assert_eq!(mode_size("113 => 0;102 => 42"), None, "a mode not known here");
    }

    /// An invitee isn't put into a room whose members have taken every seat it has now;
    /// a host who raised the size has room for them.
    #[test]
    fn invitees_are_added_only_while_the_room_has_a_seat() {
        use super::full_for_invitee;
        assert_eq!(full_for_invitee(false, Some((2, 2))), Some((2, 2)), "a 2-seat co-op room with its host and one guest");
        assert_eq!(full_for_invitee(false, Some((2, 4))), None, "raised to 4");
        assert_eq!(full_for_invitee(false, Some((5, 6))), None, "raised to 6, one seat left");
        assert_eq!(full_for_invitee(false, Some((6, 6))), Some((6, 6)));
        assert_eq!(full_for_invitee(false, Some((3, 0))), None, "no seats sent: as before");
        assert_eq!(full_for_invitee(true, Some((2, 2))), None, "a guest of it already has a seat");
        assert_eq!(full_for_invitee(false, None), None, "the room is gone");
    }

    #[test]
    fn attribute_value_reports_absence() {
        assert_eq!(attribute_value("113 => 0;3 => 8", PROPERTY_ROOM_KIND), Some(0));
        assert_eq!(attribute_value("3 => 8;4 => 0", PROPERTY_ROOM_KIND), None);
        assert_eq!(attribute_value("", PROPERTY_ROOM_KIND), None);
    }

    /// A failed join is put on the room the player was trying to reach, says how it went,
    /// and tells the game's own churn apart (the 8–9 October reports).
    #[test]
    fn failed_joins_are_put_on_the_right_room() {
        use super::failed_join;
        use crate::reports::JoinOutcome;
        let (storage, dir) = crate::storage::tests::temp_storage("failed-joins");
        let mut ids = vec![];
        for name in ["Theusma01", "PlaySkill", "Renegade", "D15hb", "Tacit"] {
            storage.register_user(name, "pw", Some(name)).unwrap();
            ids.push(storage.find_user_id_by_name(name).unwrap().unwrap());
        }
        let [theusma, playskill, renegade, d15, tacit] = ids[..] else { unreachable!() };
        let stock = "113 => 0;107 => 3909881133;3 => 0;4 => 2";
        let balaclava = "113 => 1;107 => 3713379539;3 => 2;4 => 0";

        // PlaySkill's game, refused by Theusma01's for its balaclava mod, names its own split
        // party: the failure goes on Theusma01's room, the one the invitation gave.
        let invited = storage.create_game_session(theusma, 1, stock.into()).unwrap();
        let split = storage.create_game_session(playskill, 1, balaclava.into()).unwrap();
        crate::join_targets::invited(playskill, invited);
        let join = failed_join(&storage, playskill, split, crate::reports::VERSION_MISMATCH);
        assert_eq!(
            (join.room, join.reported_room, join.outcome, join.own),
            (invited, Some(split), JoinOutcome::DataMismatch, false)
        );
        assert!(join.counts());
        let mismatch = join.mismatch.clone().unwrap();
        assert_eq!(mismatch.host.as_deref(), Some("Theusma01"));
        assert_eq!((mismatch.mine, mismatch.theirs), (Some(0xdd55_acd3), Some(0xe90c_0d2d)));
        let detail = join.detail(crate::reports::VERSION_MISMATCH);
        assert_eq!(
            detail,
            serde_json::json!({
                "room": invited, "code": "0xeea440ee", "outcome": "data_mismatch", "reported_room": split,
                "data": { "mine": "0xdd55acd3", "theirs": "0xe90c0d2d" },
                "mine_name": "balaclava mod", "theirs_name": "unmodified",
            })
        );

        // A mismatch the game reports against a room it wasn't given names nobody: the game
        // can name any session id, and the answer reaches the player in their summary.
        let private = storage.create_game_session(tacit, 1, stock.into()).unwrap();
        let join = failed_join(&storage, d15, private, crate::reports::VERSION_MISMATCH);
        let mismatch = join.mismatch.unwrap();
        assert_eq!((mismatch.host, mismatch.theirs), (None, None));
        // Given it (an invitation), it does.
        crate::join_targets::invited(d15, private);
        let mismatch = failed_join(&storage, d15, private, crate::reports::VERSION_MISMATCH).mismatch.unwrap();
        assert_eq!((mismatch.host.as_deref(), mismatch.theirs), (Some("Tacit"), Some(0xe90c_0d2d)));

        // Renegade's game reports its own party, closed 3 s earlier, as locked, while an
        // invitation is pending: its own churn, not a failed join.
        let party = storage.create_game_session(renegade, 1, balaclava.into()).unwrap();
        storage.delete_game_session(renegade, 1, party).unwrap();
        crate::join_targets::invited(renegade, invited);
        let join = failed_join(&storage, renegade, party, crate::reports::LOCKED_SESSION);
        assert_eq!((join.room, join.reported_room, join.own), (party, None, true));
        assert!(!join.counts());
        assert_eq!(join.detail(crate::reports::LOCKED_SESSION)["own"], true);
        // With nothing given lately, their own room is theirs.
        let own = storage.create_game_session(tacit, 1, stock.into()).unwrap();
        let join = failed_join(&storage, tacit, own, 0xb08a_1a05);
        assert_eq!((join.room, join.own, join.outcome), (own, true, JoinOutcome::Failed));
        // Someone else's room that had ended.
        let join = failed_join(&storage, d15, party, crate::reports::ROOM_CLOSED);
        assert!(join.own && !join.counts());

        // d15_HB's quick match found one full room, and the game said session 0.
        let full = storage.create_game_session(tacit, 1, "113 => 0;107 => 3909881133;3 => 2;4 => 0".into()).unwrap();
        crate::join_targets::searched(d15, [(full, tacit)]);
        let join = failed_join(&storage, d15, 0, crate::reports::ROOM_FULL);
        assert_eq!((join.room, join.reported_room, join.outcome, join.own), (full, Some(0), JoinOutcome::Full, false));
        assert!(!join.counts(), "a full lobby isn't a failed join");
        assert_eq!(join.mismatch, None);
        // Nothing given: session 0 stays, and isn't the player's own.
        let join = failed_join(&storage, theusma, 0, 0xb08a_1a05);
        assert_eq!((join.room, join.reported_room, join.own), (0, None, false));
        assert!(join.counts());
        assert_eq!(join.detail(0xb08a_1a05), serde_json::json!({ "room": 0, "code": "0xb08a1a05", "outcome": "failed" }));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A join that failed between games with different data is the mod's doing, whatever the
    /// game called it (a PEC.ini mod joining a stock host got CONNECTION_FAILED); between
    /// games with the same data, or when either side's isn't known, it stays the join's own.
    #[test]
    fn a_failed_join_between_different_data_is_put_down_to_the_mod() {
        use super::failed_join;
        use crate::reports::JoinOutcome;
        const CONNECTION_FAILED: u32 = 0xb08a_1a05;
        let (storage, dir) = crate::storage::tests::temp_storage("failed-joins-data");
        // Other ids than the test above's players: what the server gave each is shared.
        for i in 0..5 {
            storage.register_user(&format!("Pad{i}"), "pw", None).unwrap();
        }
        let mut ids = vec![];
        for name in ["StockHost", "PecMod", "Stock", "NoRoom"] {
            storage.register_user(name, "pw", Some(name)).unwrap();
            ids.push(storage.find_user_id_by_name(name).unwrap().unwrap());
        }
        let [host, modded, stock, no_room] = ids[..] else { unreachable!() };
        let lobby = storage.create_game_session(host, 1, "113 => 0;107 => 3909881133;3 => 8;4 => 0".into()).unwrap();
        storage.create_game_session(modded, 1, "113 => 1;107 => 4181535495;3 => 2;4 => 0".into()).unwrap();
        storage.create_game_session(stock, 1, "113 => 1;107 => 3909881133;3 => 2;4 => 0".into()).unwrap();

        // Not a room the server gave them: theirs isn't known, so nothing changes.
        let join = failed_join(&storage, modded, lobby, CONNECTION_FAILED);
        assert_eq!((join.outcome, join.mismatch), (JoinOutcome::Failed, None));

        crate::join_targets::searched(modded, [(lobby, host)]);
        let join = failed_join(&storage, modded, lobby, CONNECTION_FAILED);
        assert_eq!((join.room, join.outcome, join.own), (lobby, JoinOutcome::DataMismatch, false));
        let m = join.mismatch.clone().unwrap();
        assert_eq!((m.host.as_deref(), m.mine, m.theirs), (Some("StockHost"), Some(0xf93d_2b07), Some(0xe90c_0d2d)));
        assert!(join.counts() && join.outcome.problem().explained());
        assert_eq!(join.detail(CONNECTION_FAILED)["outcome"], "data_mismatch");
        let (title, text) = server_api::notices::join_notice(
            join.outcome.problem(),
            m.host.as_deref().unwrap_or_default(),
            &crate::reports::data_shown(m.mine),
            &crate::reports::data_shown(m.theirs),
        );
        assert_eq!(title, "Couldn't join: different game data");
        assert!(text.contains("StockHost") && text.contains("Everyone in a match needs the same mods"), "{text}");
        // Counted as a mismatch, as a refused one is.
        let user = modded + 7_000_000;
        crate::reports::join_failed(user, join.room, CONNECTION_FAILED, join.outcome, join.mismatch.clone());
        let summary = crate::reports::summary(&storage, user).unwrap();
        assert_eq!((summary.failed_joins, summary.version_mismatches), (1, 1));
        assert_eq!(summary.mismatch, Some(m));

        // The same data: the join's own failure, told as before.
        crate::join_targets::searched(stock, [(lobby, host)]);
        let join = failed_join(&storage, stock, lobby, CONNECTION_FAILED);
        assert_eq!((join.outcome, join.mismatch), (JoinOutcome::Failed, None));
        // Their own data not known (no room of theirs): the same.
        crate::join_targets::searched(no_room, [(lobby, host)]);
        let join = failed_join(&storage, no_room, lobby, CONNECTION_FAILED);
        assert_eq!((join.outcome, join.mismatch), (JoinOutcome::Failed, None));
        // A full lobby says all there is, whatever the data.
        let join = failed_join(&storage, modded, lobby, crate::reports::ROOM_FULL);
        assert_eq!((join.outcome, join.mismatch), (JoinOutcome::Full, None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The game's TICKET_PENDING straight after a failure the player was told about (LEAVING,
    /// the room closing as they joined) is part of that one: not counted again, nor told,
    /// and on its room whatever room the game names. On its own it's a failure.
    #[test]
    fn a_ticket_pending_after_an_explained_failure_is_part_of_it() {
        use super::explained_just_before;
        use super::failed_join;
        use super::note_explained;
        use super::FailedJoin;
        use crate::reports::JoinOutcome;
        use crate::reports::TICKET_PENDING;
        const LEAVING: u32 = 0x33ae_7adc;
        let (storage, dir) = crate::storage::tests::temp_storage("failed-joins-follow");
        // Other ids than the two tests above (what the server gave each is shared).
        for i in 0..9 {
            storage.register_user(&format!("Pad{i}"), "pw", None).unwrap();
        }
        let mut ids = vec![];
        for name in ["Leaver", "Joiner", "Alone"] {
            storage.register_user(name, "pw", Some(name)).unwrap();
            ids.push(storage.find_user_id_by_name(name).unwrap().unwrap());
        }
        let [host, joiner, alone] = ids[..] else { unreachable!() };
        let room = storage.create_game_session(host, 1, "113 => 0;107 => 3909881133;3 => 8;4 => 0".into()).unwrap();
        let party = storage.create_game_session(joiner, 1, "113 => 1;107 => 3909881133;3 => 2;4 => 0".into()).unwrap();
        crate::join_targets::searched(joiner, [(room, host)]);

        let leaving = failed_join(&storage, joiner, room, LEAVING);
        assert_eq!((leaving.room, leaving.outcome), (room, JoinOutcome::Closed));
        assert!(leaving.told() && leaving.counts() && leaving.outcome.problem().explained());
        assert_eq!(explained_just_before(joiner, TICKET_PENDING), None, "nothing told yet");
        note_explained(joiner, &leaving);
        // In the same second, against the joiner's own party (na1: 909, put on 907).
        let earlier = explained_just_before(joiner, TICKET_PENDING).unwrap();
        assert_eq!((earlier.room, earlier.outcome), (room, JoinOutcome::Closed));
        let pending = FailedJoin::follow_up(&earlier, party);
        assert_eq!((pending.room, pending.reported_room, pending.outcome), (room, Some(party), JoinOutcome::Closed));
        assert!(!pending.counts() && !pending.told(), "counted once, told once");
        assert_eq!(pending.detail(TICKET_PENDING)["follows"], true);
        // Only a TICKET_PENDING is part of it: another failure is one of its own.
        assert_eq!(explained_just_before(joiner, 0xac2d_801a), None);
        // Later than that, it's a failure of its own again.
        super::explained_lately().get_mut(&joiner).unwrap().0 -= super::FOLLOWS_WITHIN;
        assert_eq!(explained_just_before(joiner, TICKET_PENDING), None);

        // A TICKET_PENDING on its own: the join's own failure, counted and not explained.
        crate::join_targets::searched(alone, [(room, host)]);
        let join = failed_join(&storage, alone, room, TICKET_PENDING);
        assert_eq!(explained_just_before(alone, TICKET_PENDING), None);
        assert_eq!((join.room, join.outcome, join.follows), (room, JoinOutcome::Failed, false));
        assert!(join.counts() && join.told() && !join.outcome.problem().explained());
        std::fs::remove_dir_all(dir).unwrap();
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
