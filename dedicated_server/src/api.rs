//! This module defines and implements the gRPC services for the dedicated server,
//! including Friends, Users, Misc, UsersAdmin, and GamesAdmin services.

use std::collections::HashMap;
use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use quazal::rmc::types::Property;
use quazal::rmc::types::QList;
use quazal::rmc::types::StationURL;
use server_api::friends;
use server_api::friends::friends_server::Friends;
use server_api::friends::friends_server::FriendsServer;
use server_api::friends::Friend;
use server_api::games;
use server_api::games::games_admin_server::GamesAdmin;
use server_api::games::games_admin_server::GamesAdminServer;
use server_api::misc;
use server_api::misc::misc_server::Misc;
use server_api::misc::misc_server::MiscServer;
use server_api::users;
use server_api::users::users_admin_server::UsersAdmin;
use server_api::users::users_admin_server::UsersAdminServer;
use server_api::users::users_server::Users;
use server_api::users::users_server::UsersServer;
use server_api::users::User;
use slog::Logger;
use sodiumoxide::base64;
use sodiumoxide::crypto::secretbox;
use sodiumoxide::crypto::secretbox::Key;
use sodiumoxide::crypto::secretbox::Nonce;
use tonic::transport::Server;
use tonic::Request;
use tonic::Response;
use tonic::Status;

use crate::config::DebugConfig;
use crate::config::FriendsMode;
use crate::federation;
use crate::storage::FriendError;
use crate::storage::FriendEventKind;
use crate::storage::GameSession;
use crate::storage::LoginError;
use crate::storage::Person;
use crate::storage::Relation;
use crate::storage::Storage;

/// The longest password a new one may be: the game hands the password over
/// in a 64-byte buffer.
pub const MAX_PASSWORD: usize = 63;

/// The session payload the game announces and reads back (bytes).
const SESSION_DATA_SIZE: usize = 496;

/// How long a sign-in token is good for. The game and the overlay sign in
/// again on their own when one runs out.
const TOKEN_LIFETIME_SECS: i64 = 30 * 24 * 60 * 60;

/// The game's diagnostic lines (Misc.ClientLog): most taken from one request, and the most
/// of one kept.
const CLIENT_LOG_LINES: usize = 50;
const CLIENT_LOG_CHARS: usize = 300;

/// `text` without control characters, cut to `max` characters.
fn printable(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

/// The caller's user id, put there by [`check_token`].
fn caller<T>(request: &Request<T>) -> Result<u32, Status> {
    request
        .metadata()
        .get("user_id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| Status::unauthenticated("Not signed in"))
}

/// An internal error: the details go to the log, never to the client.
fn internal(e: impl std::fmt::Display) -> Status {
    eprintln!("API internal error: {e}");
    Status::internal("internal error")
}

/// A storage error: [`crate::storage::Busy`] says so (try again), anything
/// else is [`internal`].
fn storage_error(e: eyre::Report) -> Status {
    if e.is::<crate::storage::Busy>() {
        Status::resource_exhausted("The server is busy; try again in a moment")
    } else {
        internal(e)
    }
}

/// Refuses a request carrying a password or sign-in that came over plain
/// HTTP, when `[limits] require_tls_for_credentials` says so (see
/// `rate_limit::credentials_allowed`).
fn require_tls<T>(request: &Request<T>) -> Result<(), Status> {
    let proto = request.metadata().get("x-forwarded-proto").and_then(|v| v.to_str().ok());
    if crate::rate_limit::credentials_allowed(request.remote_addr().map(|a| a.ip()), proto) {
        Ok(())
    } else {
        Err(Status::failed_precondition(crate::rate_limit::TLS_REQUIRED))
    }
}

/// A new sign-in token for `user_id`: the id, the account's token epoch and
/// the time, sealed with the server's key.
fn issue_token(key: &Key, user_id: u32, epoch: i64) -> String {
    let plain = format!("{user_id}:{epoch}:{}", identity::now());
    let n = secretbox::gen_nonce();
    let c = secretbox::seal(plain.as_bytes(), &n, key);
    format!(
        "{}.{}",
        base64::encode(c, base64::Variant::UrlSafeNoPadding),
        base64::encode(n, base64::Variant::UrlSafeNoPadding)
    )
}

/// Whether a new account name is acceptable: 1 to 32 letters, digits, `_`,
/// `-` and `.` (what the launcher's one-click names are made of).
pub fn check_username(name: &str) -> Result<(), &'static str> {
    let n = name.chars().count();
    if n == 0 || n > 32 {
        return Err("names are 1 to 32 characters");
    }
    // ASCII only: other scripts have letters that look like Latin ones ("Kiwi" with a
    // Cyrillic "і"), which would let anyone pose as someone else.
    if !name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.')) {
        return Err("names can have letters A-Z, digits, _, - and . only");
    }
    let key: String = identity::name_key(name).chars().filter(char::is_ascii_alphanumeric).collect();
    if RESERVED_NAMES.contains(&key.as_str()) {
        return Err("that name is reserved");
    }
    Ok(())
}

/// Names no player may take (compared without case or punctuation): they'd
/// look like the server or its operators speaking.
const RESERVED_NAMES: &[&str] = &[
    "admin",
    "administrator",
    "server",
    "system",
    "moderator",
    "mod",
    "operator",
    "owner",
    "support",
    "staff",
    "root",
    "tracking",
    "ubisoft",
    "5thechelon",
    "fifthechelon",
    "anonymous",
    "nobody",
];

fn relation_proto(r: Relation) -> friends::Relation {
    match r {
        Relation::Friend => friends::Relation::Friend,
        Relation::RequestSent => friends::Relation::RequestSent,
        Relation::RequestReceived => friends::Relation::RequestReceived,
        Relation::Blocked => friends::Relation::Blocked,
        // Nobody learns that they were blocked.
        Relation::None | Relation::BlockedBy => friends::Relation::None,
    }
}

/// How far a player's name is known to be theirs.
fn name_status(person: &Person) -> friends::NameStatus {
    match (&person.global_id, person.name_conflict, federation::enabled()) {
        (None, _, _) => friends::NameStatus::Unlinked,
        (Some(_), true, _) => friends::NameStatus::Conflict,
        (Some(_), false, true) => friends::NameStatus::Reserved,
        (Some(_), false, false) => friends::NameStatus::Linked,
    }
}

/// Refuses a signature made for a host this server doesn't go by: what
/// stops a server that claims to be this one from bringing signatures
/// players made for it.
fn signed_host(host: &str) -> Result<(), Status> {
    if federation::is_own_host(host) {
        Ok(())
    } else {
        Err(Status::invalid_argument(format!(
            "This server doesn't go by {host:?}; its operator can add the name to [public] aliases"
        )))
    }
}

/// What a player is told when the name they want is someone else's across
/// the servers sharing friends.
const NAME_HELD_ELSEWHERE: &str = "That name belongs to another player on the servers sharing friends";

fn friend_error(e: FriendError) -> Status {
    match e {
        FriendError::NotFound => Status::not_found(e.to_string()),
        FriendError::Yourself | FriendError::NoRequest => Status::invalid_argument(e.to_string()),
        FriendError::YouBlocked => Status::failed_precondition(e.to_string()),
        FriendError::TooManyRequests => Status::resource_exhausted(e.to_string()),
    }
}

/// Property 113 tells a private room (0) from an ordinary lobby session (1).
///
/// Both live in the same session type, so the attribute is the only thing distinguishing
/// "the match I configured and want my friend in" from "the anteroom every client opens on
/// entering multiplayer".
fn is_private_room(session: &GameSession) -> bool {
    session
        .attributes
        .parse::<QList<Property>>()
        .is_ok_and(|attributes| attributes.0.iter().any(|property| property.id == 113 && property.value == 0))
}

/// Implements the `Friends` gRPC service.
pub struct MyFriends {
    logger: Logger,
    storage: Arc<Storage>,
    debug_config: Arc<DebugConfig>,
    mode: FriendsMode,
}

impl MyFriends {
    /// The player a request names, by account id or else by name.
    async fn target(&self, request: &friends::TargetRequest) -> Result<Person, Status> {
        let person = if request.id.is_empty() {
            self.storage.find_person_by_name(&request.username).await
        } else {
            self.storage.find_person_by_ubi_id(&request.id).await
        };
        person
            .map_err(internal)?
            .filter(|p| !p.ubi_id.is_empty())
            .ok_or_else(|| friend_error(FriendError::NotFound))
    }

    fn player(&self, person: Person, relation: Relation, sessions: Option<&[crate::storage::LiveSession]>) -> friends::Player {
        // What someone is doing (and with whom) is for their friends; in the "mutual" mode,
        // so is whether they're online.
        let friend = relation == Relation::Friend;
        let online_shown = friend || self.mode == FriendsMode::Everyone;
        let person = Person {
            is_online: person.is_online && online_shown,
            ..person
        };
        let activity = sessions
            .filter(|_| person.is_online && friend)
            .and_then(|s| crate::community_api::activity_of(&person.username, s))
            .map(|a| friends::Activity {
                mode: a.mode.into(),
                room: a.room.into(),
                with: a.with,
                map: a.map.unwrap_or_default(),
            });
        let status = name_status(&person);
        let mut p = friends::Player {
            identity: person.global_id.as_deref().map(identity::short).unwrap_or_default(),
            id: person.ubi_id,
            username: person.username,
            is_online: person.is_online || self.debug_config.mark_all_as_online,
            relation: 0,
            activity,
            name: 0,
        };
        p.set_relation(relation_proto(relation));
        p.set_name(status);
        p
    }

    /// Runs a friend change for the caller, with its rate limit, and answers
    /// with where the two stand afterwards (or what `f` says they seem to).
    async fn change<F, Fut>(&self, request: Request<friends::TargetRequest>, what: &str, f: F) -> Result<Response<friends::ChangeResponse>, Status>
    where
        F: FnOnce(u32, Person) -> Fut,
        Fut: Future<Output = Result<Option<Relation>, Status>>,
    {
        let me = caller(&request)?;
        if !crate::rate_limit::friend_changes().check(me) {
            return Err(Status::resource_exhausted("Too many friend changes; try again in a minute"));
        }
        let other = self.target(request.get_ref()).await?;
        if other.id == me {
            return Err(friend_error(FriendError::Yourself));
        }
        info!(self.logger, "{what}: {me} -> {} ({})", other.username, other.id);
        let other_id = other.id;
        let relation = match f(me, other).await? {
            Some(seems) => seems,
            None => self.storage.relation(me, other_id).await.map_err(internal)?,
        };
        let mut resp = friends::ChangeResponse { relation: 0 };
        resp.set_relation(relation_proto(relation));
        Ok(Response::new(resp))
    }
}

#[tonic::async_trait]
impl Friends for MyFriends {
    /// Handles friend invitation requests.
    ///
    /// Only friends may invite in the "mutual" mode, and never across a block; a player can
    /// send 20 a minute, and have five waiting (see `add_invite_async`).
    async fn invite(&self, request: Request<friends::InviteRequest>) -> Result<Response<friends::InviteResponse>, Status> {
        let sender = caller(&request)?;
        debug!(self.logger, "Invite request: {:?} from {}", request, sender);
        if !crate::rate_limit::invites().check(sender) {
            return Err(Status::resource_exhausted("Too many invitations; try again in a minute"));
        }

        let receiver = request.into_inner().id;

        let Some(receiver_id) = self.storage.find_user_id_by_ubi_id_async(&receiver).await.map_err(internal)? else {
            return Err(Status::not_found("User not found"));
        };

        match self.storage.relation(sender, receiver_id).await.map_err(internal)? {
            Relation::Blocked => return Err(Status::permission_denied("You blocked this player")),
            // Someone who blocked the sender: it looks sent and goes nowhere (nobody learns
            // they were blocked).
            Relation::BlockedBy => return Ok(Response::new(friends::InviteResponse {})),
            Relation::Friend => {}
            _ if self.mode == FriendsMode::Mutual => return Err(Status::permission_denied("Only friends can invite each other on this server")),
            _ => {}
        }

        // Bind the invitation to the room the sender is actually in. The event the game
        // receives carries only the sender, so without this the invited client has no way to
        // learn which session it is supposed to join. If the host has no joinable session at
        // all, the invitation stays unbound and behaves as it did before room tracking.
        let host_sessions = self.storage.find_host_sessions_async(sender).await.map_err(internal)?;
        // A host who opened a private match sits in two sessions at once: the anteroom
        // every client opens on entering multiplayer (attribute 113 == 1) and the configured
        // match room itself (113 == 0). Bind the invitation to the match room, so the guest
        // ends up in the match rather than in the host's lobby.
        //
        // The client sorts both rooms by that same attribute when it answers the invitation,
        // and it gives the match room precedence - see `search_sessions_with_participants`
        // in `game_session.rs` for the full picture.
        let room = host_sessions.iter().find(|session| is_private_room(session)).or_else(|| host_sessions.first());

        match room {
            Some(room) => info!(
                self.logger,
                "Binding invitation to {} {} of host {sender}",
                if is_private_room(room) { "private room" } else { "party session" },
                room.session_id
            ),
            None => warn!(self.logger, "Host {sender} has no joinable session; invitation to {receiver_id} stays unbound"),
        }

        self.storage
            .add_invite_async(sender, receiver_id, room.map(|r| r.session_type), room.map(|r| r.session_id))
            .await
            .map_err(|e| Status::resource_exhausted(format!("Couldn't add invite: {e}")))?;

        Ok(Response::new(friends::InviteResponse {}))
    }

    /// The game's friend list: every player in the "everyone" mode (as before friend lists
    /// existed), only friends in the "mutual" mode; never anyone blocked either way.
    async fn list(&self, request: Request<friends::ListRequest>) -> Result<Response<friends::ListResponse>, Status> {
        let me = caller(&request)?;
        debug!(self.logger, "Friendlist request from {me}");
        if !crate::rate_limit::polls().check(me) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        let mut people = match self.mode {
            FriendsMode::Everyone => self.storage.everyone_for(me).await.map_err(internal)?,
            FriendsMode::Mutual => self.storage.friends_of(me).await.map_err(internal)?,
        };
        // The list always had the player themselves in it; kept, as the game has always had it.
        people.extend(self.storage.find_person(me).await.map_err(internal)?.filter(|p| !p.ubi_id.is_empty()));
        // Which session everybody is in. The game looks for a friend's session right here in
        // the friend list - it never asks separately - so an accepted invitation is dead
        // without it.
        let sessions = self.storage.list_advertised_sessions_async().await.map_err(internal)?;
        let friend_ids: std::collections::HashSet<u32> = self.storage.friends_of(me).await.map_err(internal)?.into_iter().map(|p| p.id).collect();
        let inviters: std::collections::HashSet<u32> = self.storage.pending_inviters(me).await.map_err(internal)?.into_iter().collect();
        let friends = people
            .into_iter()
            .map(|u| {
                let (session_id, invite_only, session_data) = sessions.get(&u.id).cloned().unwrap_or_default();
                // A private session's details only go to friends, and to whoever this player
                // invited (the game needs them to join); public ones to anyone.
                let shared = !invite_only || u.id == me || friend_ids.contains(&u.id) || inviters.contains(&u.id);
                let (session_id, session_data) = if shared { (session_id, session_data) } else { (0, Vec::new()) };
                Friend {
                    pid: u.id,
                    id: u.ubi_id,
                    username: u.username,
                    is_online: u.is_online || self.debug_config.mark_all_as_online,
                    session_id,
                    invite_only,
                    session_data,
                }
            })
            .collect();
        Ok(Response::new(friends::ListResponse { friends }))
    }

    /// Publishes the caller's current game session so their friends can join it.
    ///
    /// A `session_id` of 0 clears the advertisement; that is what the game's
    /// `UPLAY_USER_ClearGameSession` boils down to.
    async fn set_session(&self, request: Request<friends::SetSessionRequest>) -> Result<Response<friends::SetSessionResponse>, Status> {
        let user_id = caller(&request)?;
        let request = request.into_inner();
        debug!(self.logger, "SetSession request from {}: {:?}", user_id, request);
        if !crate::rate_limit::polls().check(user_id) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        // The game's payload is 496 bytes, for a session its player is in: nothing else is
        // stored and handed to others.
        if !(request.session_data.is_empty() || request.session_data.len() == SESSION_DATA_SIZE) {
            return Err(Status::invalid_argument("Session data is 496 bytes"));
        }
        if request.session_id != 0 && !self.storage.is_in_session(user_id, request.session_id).await.map_err(internal)? {
            return Err(Status::permission_denied("Not a session you're in"));
        }

        self.storage
            .set_advertised_session_async(user_id, (request.session_id != 0).then_some(request.session_id), request.invite_only, &request.session_data)
            .await
            .map_err(internal)?;

        Ok(Response::new(friends::SetSessionResponse {}))
    }

    async fn relationships(&self, request: Request<friends::RelationshipsRequest>) -> Result<Response<friends::RelationshipsResponse>, Status> {
        let me = caller(&request)?;
        if !crate::rate_limit::polls().check(me) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        federation::pull_now_and_then(me);
        let sessions = self.storage.presence_async().await.map_err(internal)?.1;
        let list = |people: Vec<Person>, relation: Relation| -> Vec<friends::Player> { people.into_iter().map(|p| self.player(p, relation, Some(&sessions))).collect() };
        let me_person = self
            .storage
            .find_person(me)
            .await
            .map_err(internal)?
            .ok_or_else(|| Status::unauthenticated("Unknown user"))?;
        let mut resp = friends::RelationshipsResponse {
            my_name: 0,
            my_identity: me_person.global_id.as_deref().map(identity::short).unwrap_or_default(),
            friends: list(self.storage.friends_of(me).await.map_err(internal)?, Relation::Friend),
            requests_received: list(self.storage.friend_requests(me, true).await.map_err(internal)?, Relation::RequestReceived),
            requests_sent: list(self.storage.friend_requests(me, false).await.map_err(internal)?, Relation::RequestSent),
            blocked: list(self.storage.blocked_by(me).await.map_err(internal)?, Relation::Blocked),
            mode: match self.mode {
                FriendsMode::Everyone => "everyone",
                FriendsMode::Mutual => "mutual",
            }
            .into(),
            elsewhere: federation::friends_elsewhere(me)
                .into_iter()
                .map(|e| friends::FriendElsewhere {
                    username: e.username,
                    server: e.server,
                    region: e.region,
                    host: e.host,
                })
                .collect(),
        };
        resp.set_my_name(name_status(&me_person));
        Ok(Response::new(resp))
    }

    async fn search(&self, request: Request<friends::SearchRequest>) -> Result<Response<friends::SearchResponse>, Status> {
        let me = caller(&request)?;
        if !crate::rate_limit::searches().check(me) {
            return Err(Status::resource_exhausted("Too many searches; try again in a minute"));
        }
        let query = request.into_inner().query;
        if query.chars().count() > 32 {
            return Err(Status::invalid_argument("Search for at most 32 characters"));
        }
        let found = self
            .storage
            .search_players(me, query.trim(), 25, self.mode == FriendsMode::Everyone)
            .await
            .map_err(internal)?;
        let sessions = self.storage.presence_async().await.map_err(internal)?.1;
        Ok(Response::new(friends::SearchResponse {
            players: found.into_iter().map(|(p, r)| self.player(p, r, Some(&sessions))).collect(),
        }))
    }

    async fn request(&self, request: Request<friends::TargetRequest>) -> Result<Response<friends::ChangeResponse>, Status> {
        self.change(request, "Friend request", |me, other| async move {
            let before = self.storage.relation(me, other.id).await.map_err(internal)?;
            let now = self.storage.request_friend(me, other.id).await.map_err(internal)?.map_err(friend_error)?;
            if now == Relation::Friend && before != Relation::Friend {
                federation::record_friends(&self.logger, &self.storage, me, other.id, true).await;
            }
            // A request to someone who blocked the caller looks sent, like any other.
            Ok(Some(now))
        })
        .await
    }

    async fn accept(&self, request: Request<friends::TargetRequest>) -> Result<Response<friends::ChangeResponse>, Status> {
        self.change(request, "Friend accepted", |me, other| async move {
            self.storage.accept_friend(me, other.id).await.map_err(internal)?.map_err(friend_error)?;
            federation::record_friends(&self.logger, &self.storage, me, other.id, true).await;
            Ok(None)
        })
        .await
    }

    async fn decline(&self, request: Request<friends::TargetRequest>) -> Result<Response<friends::ChangeResponse>, Status> {
        self.change(request, "Friend request declined", |me, other| async move {
            match self.storage.relation(me, other.id).await.map_err(internal)? {
                Relation::RequestReceived | Relation::RequestSent => {
                    self.storage.remove_friend(me, other.id).await.map_err(internal)?;
                    Ok(None)
                }
                _ => Err(friend_error(FriendError::NoRequest)),
            }
        })
        .await
    }

    async fn remove(&self, request: Request<friends::TargetRequest>) -> Result<Response<friends::ChangeResponse>, Status> {
        self.change(request, "Friend removed", |me, other| async move {
            if self.storage.remove_friend(me, other.id).await.map_err(internal)? {
                federation::record_friends(&self.logger, &self.storage, me, other.id, false).await;
            }
            Ok(None)
        })
        .await
    }

    async fn block(&self, request: Request<friends::TargetRequest>) -> Result<Response<friends::ChangeResponse>, Status> {
        self.change(request, "Blocked", |me, other| async move {
            self.storage.block(me, other.id).await.map_err(internal)?.map_err(friend_error)?;
            federation::record_block(&self.logger, &self.storage, me, other.id, true).await;
            Ok(None)
        })
        .await
    }

    async fn unblock(&self, request: Request<friends::TargetRequest>) -> Result<Response<friends::ChangeResponse>, Status> {
        self.change(request, "Unblocked", |me, other| async move {
            if self.storage.unblock(me, other.id).await.map_err(internal)? {
                federation::record_block(&self.logger, &self.storage, me, other.id, false).await;
            }
            Ok(None)
        })
        .await
    }

    async fn link_identity(&self, request: Request<friends::LinkIdentityRequest>) -> Result<Response<friends::LinkIdentityResponse>, Status> {
        require_tls(&request)?;
        let me = caller(&request)?;
        let request = request.into_inner();
        let person = self
            .storage
            .find_person(me)
            .await
            .map_err(internal)?
            .ok_or_else(|| Status::unauthenticated("Unknown user"))?;
        if !identity::is_global_id(&request.global_id) {
            return Err(Status::invalid_argument("Not an identity"));
        }
        if !identity::fresh(request.time, identity::now()) {
            return Err(Status::invalid_argument("The signature's time is off; check this PC's clock"));
        }
        signed_host(&request.host)?;
        let message = identity::link_message(&request.host, &person.username, request.time);
        if !identity::verify(&request.global_id, &message, &request.signature) {
            return Err(Status::permission_denied("The signature doesn't match"));
        }
        if person.global_id.as_deref() == Some(request.global_id.as_str()) {
            return Ok(Response::new(friends::LinkIdentityResponse {}));
        }
        if person.global_id.is_some() {
            return Err(Status::already_exists("This account is linked to another identity"));
        }
        self.storage
            .link_global_id(me, &request.global_id)
            .await
            .map_err(|e| Status::already_exists(e.to_string()))?;
        info!(self.logger, "{} ({me}) linked to identity {}", person.username, identity::short(&request.global_id));
        let link = federation::Change::Link {
            global_id: request.global_id,
            username: person.username,
            host: identity::host_key(&request.host),
            time: request.time,
            signature: request.signature,
        };
        federation::linked(&self.logger, &self.storage, me, link).await;
        Ok(Response::new(friends::LinkIdentityResponse {}))
    }

    async fn unlink_identity(&self, request: Request<friends::UnlinkIdentityRequest>) -> Result<Response<friends::UnlinkIdentityResponse>, Status> {
        let me = caller(&request)?;
        if crate::rate_limit::identity_required() {
            return Err(Status::failed_precondition("Accounts on this server stay linked to their identity"));
        }
        let person = self
            .storage
            .find_person(me)
            .await
            .map_err(internal)?
            .ok_or_else(|| Status::unauthenticated("Unknown user"))?;
        if let Some(global_id) = person.global_id {
            if self.storage.unlink_global_id(me).await.map_err(internal)? {
                info!(self.logger, "{} ({me}) unlinked from identity {}", person.username, identity::short(&global_id));
                federation::record(&self.logger, &self.storage, federation::Change::Unlink { global_id }).await;
            }
        }
        Ok(Response::new(friends::UnlinkIdentityResponse {}))
    }

    async fn rename(&self, request: Request<friends::RenameRequest>) -> Result<Response<friends::RenameResponse>, Status> {
        require_tls(&request)?;
        let me = caller(&request)?;
        if !crate::rate_limit::friend_changes().check(me) {
            return Err(Status::resource_exhausted("Too many changes; try again in a minute"));
        }
        let request = request.into_inner();
        let new_name = request.new_name.trim().to_string();
        check_username(&new_name).map_err(Status::invalid_argument)?;
        let person = self
            .storage
            .find_person(me)
            .await
            .map_err(internal)?
            .ok_or_else(|| Status::unauthenticated("Unknown user"))?;
        if let Some(owner) = self.storage.find_person_by_name(&new_name).await.map_err(internal)? {
            if owner.id != me {
                return Err(Status::already_exists("Another player here has that name"));
            }
        }
        // Nor another account's id (renamed accounts keep theirs): one name, one player.
        if self.storage.find_person_by_ubi_id(&new_name).await.map_err(internal)?.is_some_and(|p| p.id != me) {
            return Err(Status::already_exists("Another player here has that name"));
        }
        // A linked account signs its new name; across the group it must be free or theirs.
        let link = match &person.global_id {
            Some(global_id) => {
                if !identity::fresh(request.time, identity::now()) {
                    return Err(Status::invalid_argument("The signature's time is off; check this PC's clock"));
                }
                signed_host(&request.host)?;
                if !identity::verify(global_id, &identity::link_message(&request.host, &new_name, request.time), &request.signature) {
                    return Err(Status::permission_denied("Sign the new name with the identity this account is linked to"));
                }
                if federation::claim_name(global_id, &new_name, &request.host, request.time, &request.signature).await == federation::NameCheck::Taken {
                    return Err(Status::already_exists(NAME_HELD_ELSEWHERE));
                }
                Some(federation::Change::Link {
                    global_id: global_id.clone(),
                    username: new_name.clone(),
                    host: identity::host_key(&request.host),
                    time: request.time,
                    signature: request.signature.clone(),
                })
            }
            None => {
                if federation::name_holder(&new_name).await == federation::NameCheck::Taken {
                    return Err(Status::already_exists(NAME_HELD_ELSEWHERE));
                }
                None
            }
        };
        match self.storage.rename_user(me, &new_name).await.map_err(internal)? {
            Ok(()) => {}
            Err(crate::storage::RenameError::Taken) => return Err(Status::already_exists("Another player here has that name")),
        }
        info!(self.logger, "{} ({me}) is now {new_name}", person.username);
        // The coordinator moves the link to the new name, and frees the old one.
        if let Some(link) = link {
            federation::record(&self.logger, &self.storage, link).await;
        }
        Ok(Response::new(friends::RenameResponse { username: new_name }))
    }
}

/// Authenticates a gRPC request by validating the provided authorization token.
///
/// This function extracts the token from the request metadata, decrypts it using the
/// provided key, and verifies the user ID against the storage. If successful,
/// the user ID is inserted into the request metadata for downstream services.
async fn check_token<T>(logger: &Logger, key: &Key, storage: &Arc<Storage>, mut req: Request<T>) -> Result<Request<T>, Status> {
    let token = req.metadata().get("authorization").ok_or(Status::unauthenticated("Missing authorization"))?;

    let mut parts = token.to_str().map_err(|_| Status::unauthenticated("Invalid token"))?.split('.');
    let c = parts.next().ok_or(Status::unauthenticated("Invalid token"))?;
    let n = parts.next().ok_or(Status::unauthenticated("Invalid token"))?;
    if parts.next().is_some() {
        return Err(Status::unauthenticated("Invalid token"));
    }

    let c = base64::decode(c, base64::Variant::UrlSafeNoPadding).map_err(|()| Status::unauthenticated("Invalid token"))?;
    let n = base64::decode(n, base64::Variant::UrlSafeNoPadding).map_err(|()| Status::unauthenticated("Invalid token"))?;

    let plain = secretbox::open(&c, &Nonce::from_slice(&n).ok_or(Status::unauthenticated("Invalid token"))?, key).map_err(|()| Status::unauthenticated("Invalid token"))?;

    let plain = std::str::from_utf8(&plain).map_err(|_| Status::unauthenticated("Invalid user"))?;
    // "<user id>:<epoch>:<issued>"; older tokens are refused, so their holders sign in
    // again (the game and the overlay do that on their own).
    let mut parts = plain.split(':');
    let (Some(user_id), Some(epoch), Some(issued), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(Status::unauthenticated("Token expired"));
    };
    let issued: i64 = issued.parse().map_err(|_| Status::unauthenticated("Invalid token"))?;
    let epoch: i64 = epoch.parse().map_err(|_| Status::unauthenticated("Invalid token"))?;
    if identity::now() - issued > TOKEN_LIFETIME_SECS {
        return Err(Status::unauthenticated("Token expired"));
    }
    let id: u32 = user_id.parse().map_err(|_| Status::unauthenticated("Invalid user"))?;
    // A new password (or a deleted and reused account) ends the tokens issued before; the
    // server's own accounts never have one.
    if epoch == 0 || storage.token_epoch(id).await.map_err(|_| Status::unauthenticated("Invalid token"))? != epoch {
        return Err(Status::unauthenticated("Signed out"));
    }
    if !storage.is_player_account(id).await.map_err(|_| Status::unauthenticated("Invalid token"))? {
        return Err(Status::unauthenticated("Invalid user"));
    }

    debug!(logger, "Looking for user {user_id}");

    let user = storage
        .find_username_by_user_id_async(user_id.parse().map_err(|_| Status::unauthenticated("Invalid user"))?)
        .await
        .map_err(|_| Status::unauthenticated("Invalid token"))?;

    let Some(user) = user else {
        return Err(Status::unauthenticated("Invalid user"));
    };

    debug!(logger, "Valid token for user {user_id}: {user}");

    req.metadata_mut().insert("user_id", user_id.parse().map_err(|_| Status::unauthenticated("Invalid user"))?);

    Ok(req)
}

/// Implements the `Users` gRPC service.
pub struct MyUsers {
    logger: Logger,
    key: Key,
    storage: Arc<Storage>,
}

/// The client behind a request, through a trusted reverse proxy's
/// `X-Forwarded-For` (see `rate_limit::client_ip`).
fn client_addr<T>(request: &Request<T>) -> Option<std::net::IpAddr> {
    let forwarded = request.metadata().get("x-forwarded-for").and_then(|v| v.to_str().ok());
    crate::rate_limit::client_ip(request.remote_addr().map(|a| a.ip()), forwarded)
}

#[tonic::async_trait]
impl Users for MyUsers {
    /// Handles user login requests.
    ///
    /// Authenticates the user against the storage and generates an authorization token upon successful login.
    async fn login(&self, request: Request<users::LoginRequest>) -> Result<Response<users::LoginResponse>, Status> {
        require_tls(&request)?;
        let peer = client_addr(&request);
        let request = request.into_inner();
        let username = request.username;
        let password = request.password;
        let client = request.client;
        // Nothing costly for what can't be an account.
        if username.chars().count() > 32 || password.len() > 128 {
            return Err(Status::unauthenticated("Invalid login"));
        }
        if !crate::rate_limit::begin_login(peer, &username) {
            crate::session_events::refused(crate::session_events::Who::Name(username.clone()), "too_many", "api", Some(client.as_str()));
            return Err(Status::resource_exhausted("Too many failed logins; try again later"));
        }
        // An outdated client retrying what was refused a moment ago: the same answer, without
        // the password hash again (the password was right then, as it is now).
        if let Some(why) = crate::clients::refused_lately(&username, &client, &password) {
            crate::session_events::refused(crate::session_events::Who::Name(username.clone()), "outdated", "api", Some(client.as_str()));
            crate::rate_limit::login_succeeded(peer, &username);
            return Err(Status::failed_precondition(why));
        }

        let maybe_user = self.storage.login_user_async(&username, &password).await.map_err(storage_error)?;

        let user_id = maybe_user.map_err(|err| {
            crate::rate_limit::login_failed(peer, &username);
            crate::metrics::failed_login();
            let who = || crate::session_events::Who::Name(username.clone());
            match &err {
                LoginError::InvalidPassword => crate::session_events::refused(who(), "wrong_password", "api", Some(client.as_str())),
                LoginError::Banned(_) => crate::session_events::refused(who(), "banned", "api", Some(client.as_str())),
                LoginError::NotFound => {}
            }
            match err {
                LoginError::InvalidPassword => Status::unauthenticated("Invalid login"),
                // The launcher needs to know a saved account is gone (to make a new one);
                // names can be looked up by anyone signed in anyway.
                LoginError::NotFound => Status::not_found("Unknown user"),
                LoginError::Banned(ban) => Status::permission_denied(crate::players::banned_message(&ban)),
            }
        })?;
        // The server's own accounts (Tracking's password is the game's, so public) never get
        // an API session.
        if !self.storage.is_player_account(user_id).await.map_err(internal)? {
            crate::rate_limit::login_failed(peer, &username);
            return Err(Status::unauthenticated("Invalid login"));
        }
        crate::rate_limit::login_succeeded(peer, &username);
        if let Err(refusal) = self.admit(user_id, &username, &client).await {
            if refusal.code() == tonic::Code::FailedPrecondition {
                crate::clients::refused(user_id, &username, &client, &password);
            }
            return Err(refusal);
        }

        info!(self.logger, "Login successful for {username}");
        crate::metrics::api_login();
        // Over TLS with the password: the game's ticket is good from this address too.
        if let Some(ip) = peer {
            crate::clients::note_api_address(user_id, ip);
        }
        self.signed_in(user_id).await
    }

    /// Handles user registration requests.
    ///
    /// Registers a new user in the storage, handling potential conflicts like duplicate usernames or Ubisoft IDs.
    async fn register(&self, request: Request<users::RegisterRequest>) -> Result<Response<users::RegisterResponse>, Status> {
        require_tls(&request)?;
        if !crate::rate_limit::registrations().check(client_addr(&request)) {
            return Err(Status::resource_exhausted("Too many new accounts from this address; try again later"));
        }
        let request = request.into_inner();
        crate::clients::check(&request.client).map_err(Status::failed_precondition)?;
        let username = request.username.trim().to_string();
        let password = request.password;
        check_username(&username).map_err(Status::invalid_argument)?;
        if password.len() < 8 || password.len() > MAX_PASSWORD {
            return Err(Status::invalid_argument("Passwords are 8 to 63 characters (the game's limit)"));
        }
        if !crate::rate_limit::registration_open() {
            return Err(Status::permission_denied("This server doesn't take new accounts"));
        }
        // With an identity, its signature must hold before anything is made.
        let identity_link = if request.global_id.is_empty() {
            if crate::rate_limit::identity_required() {
                return Err(Status::failed_precondition(
                    "Accounts on this server are linked to a player identity; update the 5th Echelon launcher",
                ));
            }
            if federation::name_holder(&username).await == federation::NameCheck::Taken {
                return Err(Status::already_exists(NAME_HELD_ELSEWHERE));
            }
            None
        } else {
            if !identity::is_global_id(&request.global_id) || !identity::fresh(request.time, identity::now()) {
                return Err(Status::invalid_argument("Not a valid identity signature; check this PC's clock"));
            }
            signed_host(&request.host)?;
            if !identity::verify(&request.global_id, &identity::link_message(&request.host, &username, request.time), &request.signature) {
                return Err(Status::permission_denied("The identity's signature doesn't match"));
            }
            if self.storage.find_person_by_global_id(&request.global_id).await.map_err(internal)?.is_some() {
                return Err(Status::already_exists("This identity already has an account here"));
            }
            Some((request.global_id.clone(), request.time, request.signature.clone(), identity::host_key(&request.host)))
        };
        // Taken here. If the one holding it has no identity and the name is this identity's
        // across the group, that account is told to rename.
        if let Some(holder) = self.storage.find_person_by_name(&username).await.map_err(internal)? {
            if let (None, Some((global_id, time, signature, host))) = (&holder.global_id, &identity_link) {
                if federation::claim_name(global_id, &username, host, *time, signature).await == federation::NameCheck::Ours {
                    self.storage.set_name_conflict_by_id(holder.id, true).await.map_err(internal)?;
                    info!(self.logger, "{username} here has no identity, and the name is someone else's across the group: flagged");
                }
            }
            return Err(Status::already_exists("Username already taken"));
        }
        // The account id is set here, so a client can't claim someone else's: the name, unless
        // a renamed account still has it.
        let ubi_id = self.storage.free_ubi_id(&username).await.map_err(internal)?;

        let error = if let Err(err) = self.storage.register_user_async(&username, &password, Some(&ubi_id)).await {
            if err.is::<crate::storage::Busy>() {
                return Err(storage_error(err));
            }
            match err.downcast::<sqlx::Error>() {
                Ok(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => return Err(Status::already_exists("Username already taken")),
                Ok(err) => return Err(internal(err)),
                Err(err) => return Err(internal(err)),
            }
        } else {
            String::new()
        };
        info!(self.logger, "New user {username} registered");
        crate::metrics::registration();
        if let Some((global_id, time, signature, host)) = identity_link {
            let Some(person) = self.storage.find_person_by_name(&username).await.map_err(internal)? else {
                return Err(Status::internal("internal error"));
            };
            // Claimed only now the account exists: a registration that fails leaves no claim
            // behind. Held by someone else across the group: the account goes again.
            if federation::claim_name(&global_id, &username, &host, time, &signature).await == federation::NameCheck::Taken {
                self.storage.delete_user_async(person.id).await.map_err(internal)?;
                return Err(Status::already_exists(NAME_HELD_ELSEWHERE));
            }
            self.storage.link_global_id(person.id, &global_id).await.map_err(internal)?;
            let link = federation::Change::Link {
                global_id,
                username: username.clone(),
                host,
                time,
                signature,
            };
            federation::linked(&self.logger, &self.storage, person.id, link).await;
        }
        Ok(Response::new(users::RegisterResponse {
            error,
            user: Some(User {
                id: ubi_id,
                username,
                ips: vec![],
            }),
        }))
    }

    /// Whether a name is free for a new account, without making one: checked as
    /// [`Self::register`] would, here and across the servers sharing friends.
    async fn name_available(&self, request: Request<users::NameRequest>) -> Result<Response<users::NameAvailableResponse>, Status> {
        use users::name_available_response::Answer;
        if !crate::rate_limit::name_checks().check(client_addr(&request)) {
            return Err(Status::resource_exhausted("Too many name checks from this address; try again later"));
        }
        let name = request.into_inner().name.trim().to_string();
        let answer = |answer: Answer, reason: &str| {
            Ok(Response::new(users::NameAvailableResponse {
                answer: answer.into(),
                reason: reason.to_string(),
            }))
        };
        if let Err(why) = check_username(&name) {
            return answer(Answer::NotAllowed, &format!("{}{}", why[..1].to_uppercase(), &why[1..]));
        }
        if self.storage.find_person_by_name(&name).await.map_err(internal)?.is_some() {
            return answer(Answer::Taken, "Someone here already has that name");
        }
        if federation::name_holder(&name).await == federation::NameCheck::Taken {
            return answer(Answer::Taken, NAME_HELD_ELSEWHERE);
        }
        answer(Answer::Free, "")
    }

    /// Signs in with the identity key the account is linked to (see `identity`).
    /// Without a username: to whichever account here is linked to the identity
    /// (the launcher finding a player's account), or `NotFound` when none is.
    async fn key_login(&self, request: Request<users::KeyLoginRequest>) -> Result<Response<users::LoginResponse>, Status> {
        require_tls(&request)?;
        let peer = client_addr(&request);
        let request = request.into_inner();
        // Failures count against the account, or the identity when there's no name.
        let limit_key = if request.username.is_empty() {
            request.global_id.clone()
        } else {
            request.username.clone()
        };
        if request.username.chars().count() > 32 || request.global_id.len() > 64 || !crate::rate_limit::begin_login(peer, &limit_key) {
            return Err(Status::resource_exhausted("Too many failed logins; try again later"));
        }
        // One answer for every failure: which accounts are linked to which identity isn't
        // anyone's business.
        let refused = |_why: &str| {
            crate::rate_limit::login_failed(peer, &limit_key);
            Status::unauthenticated("Signing in with this identity didn't work")
        };
        if !identity::fresh(request.time, identity::now()) {
            return Err(Status::invalid_argument("The signature's time is off; check this PC's clock"));
        }
        signed_host(&request.host)?;
        let person = if request.username.is_empty() {
            // The signature proves the identity first: only its holder learns whether it has
            // an account here.
            let message = identity::login_message(&request.host, "", request.time, &request.new_password);
            if !identity::is_global_id(&request.global_id) || !identity::verify(&request.global_id, &message, &request.signature) {
                return Err(refused("The signature doesn't match"));
            }
            match self.storage.find_person_by_global_id(&request.global_id).await.map_err(internal)? {
                Some(person) => person,
                // Not a failed sign-in: a player new here.
                None => {
                    crate::rate_limit::login_succeeded(peer, &limit_key);
                    return Err(Status::not_found("No account here is linked to this identity"));
                }
            }
        } else {
            let person = self
                .storage
                .find_person_by_name(&request.username)
                .await
                .map_err(internal)?
                .ok_or_else(|| refused("Unknown user"))?;
            if person.global_id.as_deref() != Some(request.global_id.as_str()) {
                return Err(refused("This account isn't linked to that identity"));
            }
            let message = identity::login_message(&request.host, &person.username, request.time, &request.new_password);
            if !identity::verify(&request.global_id, &message, &request.signature) {
                return Err(refused("The signature doesn't match"));
            }
            person
        };
        if !self.storage.is_player_account(person.id).await.map_err(internal)? {
            return Err(refused("Not a player's account"));
        }
        // Before the password changes: an outdated launcher changes nothing.
        self.admit(person.id, &person.username, &request.client).await?;
        if !self.storage.use_key_login(person.id, request.time).await.map_err(internal)? {
            return Err(refused("That signature was already used"));
        }
        if !request.new_password.is_empty() {
            if request.new_password.len() < 8 || request.new_password.len() > MAX_PASSWORD {
                return Err(Status::invalid_argument("Passwords are 8 to 63 characters (the game's limit)"));
            }
            self.storage.set_password(person.id, &request.new_password).await.map_err(storage_error)?;
            info!(self.logger, "{} set a new password with their identity key", person.username);
        }
        crate::rate_limit::login_succeeded(peer, &limit_key);
        info!(self.logger, "Key login successful for {}", person.username);
        crate::metrics::api_login();
        if let Some(ip) = peer {
            crate::clients::note_api_address(person.id, ip);
        }
        self.signed_in(person.id).await
    }
}

impl MyUsers {
    /// Lets `client` finish signing in to the account (whose password or key it proved), or
    /// refuses an outdated one. Either way it's noted: the game's own sign-in is let through
    /// only after a current client's (see [`crate::clients`]).
    async fn admit(&self, user_id: u32, username: &str, client: &str) -> Result<(), Status> {
        let verdict = crate::clients::check(client);
        self.storage.note_client_sign_in(user_id, client, verdict.is_ok()).await.map_err(internal)?;
        if verdict.is_ok() {
            crate::clients::admitted(user_id);
        }
        verdict.map_err(|why| {
            if crate::clients::worth_saying(username) {
                warn!(self.logger, "Refused {username}'s outdated client {client:?} (said once an hour; Sessions counts them all)");
            }
            crate::session_events::refused(crate::session_events::Who::Id(user_id), "outdated", "api", Some(client));
            Status::failed_precondition(why)
        })
    }

    /// The answer to a successful sign-in: a token, and who they are (the
    /// account id the game should use).
    async fn signed_in(&self, user_id: u32) -> Result<Response<users::LoginResponse>, Status> {
        // Every way in ends here: a password, or the identity key.
        if let Some(ban) = self.storage.active_ban(user_id).await.map_err(internal)? {
            return Err(Status::permission_denied(crate::players::banned_message(&ban)));
        }
        let person = self.storage.find_person(user_id).await.map_err(internal)?;
        let epoch = self.storage.token_epoch(user_id).await.map_err(internal)?;
        let nat_ticket = person.as_ref().and_then(|p| crate::nat_helper::ticket_for(&p.username)).map(Vec::from).unwrap_or_default();
        Ok(Response::new(users::LoginResponse {
            nat_ticket,
            error: String::new(),
            token: issue_token(&self.key, user_id, epoch),
            user: person.map(|p| User {
                id: p.ubi_id,
                username: p.username,
                ips: vec![],
            }),
        }))
    }
}

/// Implements the `Misc` gRPC service.
pub struct MyMisc {
    logger: Logger,
    storage: Arc<Storage>,
    debug_config: Arc<DebugConfig>,
}

#[tonic::async_trait]
impl Misc for MyMisc {
    /// Handles event requests, primarily for retrieving pending friend invites.
    async fn event(&self, request: Request<misc::EventRequest>) -> Result<Response<misc::EventResponse>, Status> {
        let user_id = caller(&request)?;
        if !crate::rate_limit::polls().check(user_id) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }

        let Some(invite) = self.storage.take_invite_async(user_id).await.map_err(|e| {
            error!(self.logger, "Error getting latest invite for user: {e}");
            Status::internal("internal error")
        })?
        else {
            // No invitation: a friend request or an accepted one, if any.
            let friend = self.storage.take_friend_event(user_id).await.map_err(internal)?.map(|(kind, from)| {
                let mut event = misc::FriendEvent {
                    kind: 0,
                    from: Some(User {
                        id: from.ubi_id,
                        username: from.username,
                        ips: vec![],
                    }),
                };
                event.set_kind(match kind {
                    FriendEventKind::Request => misc::friend_event::Kind::Request,
                    FriendEventKind::Accepted => misc::friend_event::Kind::Accepted,
                });
                event
            });
            return Ok(Response::new(misc::EventResponse { invite: None, friend }));
        };

        let Some(sender) = self.storage.find_user_by_id_async(invite.sender).await.map_err(|e| {
            error!(self.logger, "Error getting ubi id for user: {e}");
            Status::internal("internal error")
        })?
        else {
            return Err(Status::not_found(""));
        };

        // Worth a line of its own: if the room is missing here, the invited client will search
        // for a session the server cannot name, and the join fails for that reason alone.
        match (invite.session_type, invite.session_id) {
            (Some(session_type), Some(session_id)) => info!(
                self.logger,
                "Delivering invitation from {} to {}, bound to session {session_id} (type {session_type})", invite.sender, invite.receiver
            ),
            _ => warn!(
                self.logger,
                "Delivering UNBOUND invitation from {} to {} - the receiver will not find a room to join", invite.sender, invite.receiver
            ),
        }
        crate::session_events::note(
            crate::session_events::Who::Id(invite.receiver),
            "invite_delivered",
            serde_json::json!({ "from": invite.sender, "room": invite.session_id }),
        );

        Ok(Response::new(misc::EventResponse {
            invite: Some(misc::InviteEvent {
                id: invite.id,
                sender: Some(User {
                    id: sender.ubi_id,
                    username: sender.username,
                    ips: vec![],
                }),
                force_join: self.debug_config.force_joins,
            }),
            friend: None,
        }))
    }

    /// Handles P2P testing requests.
    ///
    /// Attempts to establish a UDP connection with the client and exchanges a challenge.
    /// What the server saw of the player's last game session, for the launcher to decide
    /// whether to ask how it went.
    async fn session_summary(&self, request: Request<misc::SessionSummaryRequest>) -> Result<Response<misc::SessionSummaryResponse>, Status> {
        let user_id = caller(&request)?;
        if !crate::rate_limit::game_requests().check(user_id) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        let storage = Arc::clone(&self.storage);
        let summary = tokio::task::spawn_blocking(move || crate::reports::summary(&storage, user_id))
            .await
            .map_err(internal)?
            .map_err(internal)?;
        Ok(Response::new(misc::SessionSummaryResponse {
            started: summary.started,
            ended: summary.ended,
            failed_joins: summary.failed_joins,
            version_mismatches: summary.version_mismatches,
            relayed: summary.relayed,
            matches: summary.matches,
            json: summary.json.to_string(),
        }))
    }

    /// The game's warnings, errors and network events (redacted on the player's PC), kept
    /// as session events (`client_log`) for the admin UI: at most [`CLIENT_LOG_LINES`] a
    /// request and the player's budget a minute (rate_limit.rs); the rest are counted.
    async fn client_log(&self, request: Request<misc::ClientLogRequest>) -> Result<Response<misc::ClientLogResponse>, Status> {
        let user_id = caller(&request)?;
        let request = request.into_inner();
        // Whether the player agreed to send the whole log when something goes wrong.
        let name = if request.full_logs {
            self.storage.find_username_by_user_id_async(user_id).await.ok().flatten()
        } else {
            None
        };
        crate::full_logs::diagnostics(user_id, name.as_deref(), request.full_logs);
        let mut kept = 0u32;
        let mut over = request.dropped;
        for line in request.lines.iter().take(CLIENT_LOG_LINES) {
            if !crate::rate_limit::client_log_lines().check(user_id) {
                over = over.saturating_add(1);
                continue;
            }
            let level = match line.level.as_str() {
                "ERROR" => "error",
                "WARN" => "warn",
                _ => "info",
            };
            crate::session_events::note(
                crate::session_events::Who::Id(user_id),
                "client_log",
                serde_json::json!({
                    "level": level,
                    "target": printable(&line.target, 60),
                    // Not the PC's time: the same line again soon is then one event, counted.
                    "message": printable(&line.message, CLIENT_LOG_CHARS),
                }),
            );
            kept += 1;
        }
        over = over.saturating_add(u32::try_from(request.lines.len().saturating_sub(CLIENT_LOG_LINES)).unwrap_or(u32::MAX));
        if over > 0 {
            crate::session_events::note(
                crate::session_events::Who::Id(user_id),
                "client_log",
                serde_json::json!({ "level": "info", "target": "", "message": "some lines were left out (too many at once)" }),
            );
        }
        let (send_log_since, send_log_problem) = crate::full_logs::ask(user_id).map_or((0, String::new()), |(since, problem)| (since, problem.to_string()));
        Ok(Response::new(misc::ClientLogResponse {
            kept,
            send_log_since,
            send_log_problem,
        }))
    }

    /// A player's feedback, with their logs if they agreed: queued for the coordinator with
    /// this server's side added (reports.rs).
    async fn report(&self, request: Request<misc::ReportRequest>) -> Result<Response<misc::ReportResponse>, Status> {
        let user_id = caller(&request)?;
        let peer = client_addr(&request);
        let r = request.into_inner();
        // What it carried, to say so if it's refused.
        let (files, bytes) = (r.files.len(), r.files.iter().map(|f| f.gzip.len()).sum::<usize>());
        // The game's log the server asked for (full_logs.rs): only then, and not counted
        // with the player's own reports.
        let auto = r.triggers.iter().any(|t| t == "auto");
        let asked = auto.then(|| crate::full_logs::answered(user_id)).flatten();
        if auto && asked.is_none() {
            return Err(Status::failed_precondition("The server didn't ask for this game's log"));
        }
        let incoming = crate::reports::Incoming {
            rating: r.rating,
            problems: r.problems,
            comment: r.comment,
            triggers: r.triggers,
            client: r.client.into_iter().collect(),
            files: r.files.into_iter().map(|f| (f.name, f.gzip, f.size)).collect(),
        };
        match crate::reports::accept(&self.storage, user_id, peer, incoming, auto).await.map_err(internal)? {
            Ok(id) => {
                info!(self.logger, "Report {id} from {user_id}, for the coordinator");
                if let Some(problem) = asked {
                    crate::session_events::note(
                        crate::session_events::Who::Id(user_id),
                        "log_sent",
                        serde_json::json!({ "report": id, "problem": problem, "files": files, "bytes": bytes }),
                    );
                }
                crate::federation::report_queued();
                Ok(Response::new(misc::ReportResponse { id }))
            }
            Err(refused) => {
                // The player sees why; the admins see it here and on the Sessions page.
                let (reason, why, status) = match refused {
                    crate::reports::Refused::TooMany => (
                        "too_many",
                        "too many reports today",
                        Status::resource_exhausted("You've sent a few reports today already; thanks, they're with the admins"),
                    ),
                    crate::reports::Refused::Invalid(why) => ("invalid", why, Status::invalid_argument(why)),
                };
                warn!(self.logger, "Refused a report from {user_id}: {why} ({files} files, {} KB)", bytes / 1024);
                crate::session_events::note(
                    crate::session_events::Who::Id(user_id),
                    "report_refused",
                    serde_json::json!({ "reason": reason, "why": why, "files": files, "bytes": bytes }),
                );
                Err(status)
            }
        }
    }

    async fn test_p2p(&self, request: Request<misc::TestP2pRequest>) -> Result<Response<misc::TestP2pResponse>, Status> {
        if !crate::rate_limit::game_requests().check(caller(&request)?) {
            return Err(Status::resource_exhausted("Too many requests; slow down"));
        }
        // The player's own address: behind a reverse proxy, the one it forwards
        // (trusted from listed proxies and this machine only), not the proxy's.
        let ip = client_addr(&request).ok_or(Status::failed_precondition("no client address"))?;
        let client_addr = std::net::SocketAddr::new(ip, 13_000);
        let request = request.into_inner();
        let mut resp_data = b"P2P Test - ".to_vec();
        resp_data.extend(request.challenge);

        let buf = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
            socket.connect(client_addr).await?;
            socket.send(&resp_data).await?;
            let mut buf = [0; 1024];
            let len = socket.recv(&mut buf).await?;
            Ok(buf[..len].to_vec())
        });
        let Ok(challenge) = buf.await else {
            return Err(Status::deadline_exceeded("client didn't response in time"));
        };
        let Ok(challenge): std::io::Result<Vec<u8>> = challenge else {
            return Err(Status::unknown(format!("P2P communication failed: {}", challenge.unwrap_err())));
        };

        Ok(Response::new(misc::TestP2pResponse { challenge }))
    }
}

/// Implements the `UsersAdmin` gRPC service for administrative user management.
pub struct MyUsersAdmin {
    logger: Logger,
    storage: Arc<Storage>,
}

#[tonic::async_trait]
impl UsersAdmin for MyUsersAdmin {
    /// Handles requests to list all users.
    ///
    /// Retrieves user information and their associated IP addresses from storage.
    async fn list(&self, request: Request<users::ListRequest>) -> Result<Response<users::ListResponse>, Status> {
        let _request = request.into_inner();
        let Ok(db_users) = self.storage.list_users_async().await else {
            return Err(Status::internal("Error listing users"));
        };

        let mut users = vec![];
        for user in db_users {
            let urls = self.storage.list_urls(user.id).await.map_err(|e| Status::internal(format!("{e:?}")))?;
            let ips = urls
                .into_iter()
                .map(|u| u.parse::<StationURL>())
                .filter_map(Result::ok)
                .map(|u| u.address)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            users.push(User {
                id: user.ubi_id,
                username: user.username,
                ips,
            });
        }

        let resp = users::ListResponse {
            #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
            total: users.len() as i32,
            users,
        };
        Ok(Response::new(resp))
    }

    /// Handles requests to retrieve a single user's details.
    ///
    /// Retrieves user information and their associated IP addresses from storage based on the provided user ID.
    async fn get(&self, request: Request<users::GetRequest>) -> Result<Response<users::GetResponse>, Status> {
        let request = request.into_inner();
        let user_id = request.id;
        let user_id: u32 = user_id.parse().map_err(|_| Status::invalid_argument("Invalid ID"))?;
        let Ok(user) = self.storage.find_user_by_id_async(user_id).await else {
            return Err(Status::internal("Error retrieving user"));
        };

        let Some(user) = user else {
            return Err(Status::not_found("User not found"));
        };

        let urls = self.storage.list_urls(user.id).await.map_err(|e| Status::internal(format!("{e:?}")))?;
        let ips = urls
            .into_iter()
            .map(|u| u.parse::<StationURL>())
            .filter_map(Result::ok)
            .map(|u| u.address)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let resp = users::GetResponse {
            user: Some(User {
                id: user.ubi_id,
                username: user.username,
                ips,
            }),
        };
        Ok(Response::new(resp))
    }

    /// Handles requests to delete a user.
    ///
    /// Deletes a user from storage based on their Ubisoft ID.
    async fn delete(&self, request: Request<users::DeleteRequest>) -> Result<Response<users::DeleteResponse>, Status> {
        let request = request.into_inner();
        let Some(user) = self
            .storage
            .find_user_by_ubi_id_async(&request.id)
            .await
            .map_err(|_| Status::invalid_argument("Invalid ID"))?
        else {
            return Err(Status::not_found("User not found"));
        };
        let global_id = self.storage.find_person(user.id).await.ok().flatten().and_then(|p| p.global_id);
        match self.storage.delete_user_async(user.id).await {
            Ok(()) => {
                warn!(self.logger, "Deleted user {user:?}");
                if let Some(global_id) = global_id {
                    federation::record(&self.logger, &self.storage, federation::Change::Unlink { global_id }).await;
                }
                Ok(Response::new(users::DeleteResponse {}))
            }
            Err(e) => Err(Status::internal(format!("{e:?}"))),
        }
    }
}

/// Implements the `GamesAdmin` gRPC service for administrative game session management.
struct MyGamesAdmin {
    logger: Logger,
    storage: Arc<Storage>,
}

#[tonic::async_trait]
impl GamesAdmin for MyGamesAdmin {
    /// Handles requests to list all active game sessions.
    ///
    /// Retrieves game session information from storage and parses game-specific attributes.
    async fn list(&self, request: Request<games::ListRequest>) -> Result<Response<games::ListResponse>, Status> {
        let _request = request.into_inner();
        let sessions = self.storage.list_game_sessions_async().await.map_err(|e| {
            error!(self.logger, "Error listing games: {e}");
            Status::internal(format!("{e:?}"))
        })?;

        Ok(Response::new(games::ListResponse {
            games: sessions
                .into_iter()
                .map(|s| {
                    let attributes: QList<Property> = s
                        .attributes
                        .parse()
                        .inspect_err(|e| {
                            error!(self.logger, "Error parsing game type: {e}");
                        })
                        .unwrap_or_default();
                    let attributes: HashMap<u32, u32> = attributes.0.into_iter().map(|p| (p.id, p.value)).collect();

                    // 113 is the room kind (0 match, 1 lobby) and 103 is non-zero
                    // for Spies vs Mercs (see game_session.rs).
                    let mode = if attributes.get(&103).is_some_and(|v| *v != 0) { "SvM" } else { "Co-op" };
                    let game_type = match attributes.get(&113) {
                        Some(&1) => format!("{mode} lobby"),
                        _ => format!("{mode} match"),
                    };

                    games::Game {
                        id: s.session_id,
                        creator: s.participants.iter().find(|p| p.user_id == s.creator_id).map(|p| p.name.clone()).unwrap_or_default(),
                        participants: s.participants.into_iter().filter(|p| p.user_id != s.creator_id).map(|p| p.name).collect(),
                        game_type,
                    }
                })
                .collect(),
        }))
    }

    /// Handles requests to delete a game session.
    ///
    /// Deletes a game session from storage based on its session ID.
    async fn delete(&self, request: Request<games::DeleteRequest>) -> Result<Response<games::DeleteResponse>, Status> {
        let request = request.into_inner();
        let session_id = request.id;
        match self.storage.delete_game_session_by_id_async(session_id).await {
            Ok(()) => {
                warn!(self.logger, "Deleted game session {session_id:?}");
                Ok(Response::new(games::DeleteResponse {}))
            }
            Err(e) => Err(Status::internal(format!("{e:?}"))),
        }
    }
}

/// Creates an authenticated gRPC service.
///
/// This function wraps a gRPC service with an interceptor that validates
/// an authorization token present in the request metadata.
fn authenticated<S>(
    service: S,
    logger: Logger,
    key: Key,
    storage: Arc<Storage>,
) -> tonic_async_interceptor::AsyncInterceptedService<S, impl tonic_async_interceptor::AsyncInterceptor<Future = impl Future<Output = Result<Request<()>, Status>> + Send> + Clone>
{
    tonic_async_interceptor::AsyncInterceptedService::new(service, move |req: Request<()>| {
        let storage = Arc::clone(&storage);
        let logger = logger.clone();
        let key = key.clone();
        async move {
            let peer = client_addr(&req);
            let this = check_token(&logger, &key, &storage, req).await;
            if let Err(ref e) = this {
                // An outdated game whose sign-in was refused keeps calling without a token
                // (857 in two days on eu1, nearly all from two outdated games): once an hour per
                // address and reason.
                if crate::clients::worth_saying(&format!("auth/{peer:?}/{}", e.message())) {
                    error!(logger, "Auth failure from {peer:?}: {e} (said once an hour)");
                }
            }
            this
        }
    })
}

/// Creates a gRPC service with preshared key authentication.
///
/// This function wraps a gRPC service with an interceptor that validates
/// a preshared key present in the "authorization" metadata of the request.
fn preshared_authentication<S>(service: S, key: String) -> tonic::service::interceptor::InterceptedService<S, impl tonic::service::Interceptor + Clone> {
    tonic::service::interceptor::InterceptedService::new(service, move |req: Request<()>| {
        let header_value = req.metadata().get("authorization").ok_or(Status::unauthenticated("Missing authorization"))?;
        let token = header_value.to_str().map_err(|_| Status::unauthenticated("Invalid token"))?;

        if constant_time_eq(token.as_bytes(), key.as_bytes()) {
            Ok(req)
        } else {
            Err(Status::permission_denied("Invalid token"))
        }
    })
}

/// The admin key as text, for pasting into the launcher's "Manage a server".
const ADMIN_KEY_TEXT_FILE: &str = "admin-key.txt";

/// Compares two secrets without the time taken depending on where they
/// differ, so the admin key can't be guessed a character at a time.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Encodes a byte slice into a Base32 string.
fn base32(data: &[u8]) -> String {
    let mut s = String::new();
    for chunk in data.chunks(5) {
        let mut value = 0u64;
        let mut i = 0;
        for c in chunk {
            value <<= 8;
            value |= u64::from(*c);
            i += 1;
        }
        value <<= 8 * (5 - i);
        if i == 8 {
            i = 0;
        } else {
            i = 8 - (i * 8 + 4) / 5;
        }
        for i in (i..8).rev() {
            let ch = match ((value >> (5 * i)) & 0b11111) as u8 {
                b @ 0..=9 => b'0' + b,
                b @ 10..=31 => b'A' + b - 10,
                b => unreachable!("{b:?}"),
            };
            s.push(ch as char);
        }
    }
    s
}

/// Starts the gRPC server, binding to the specified address and registering services.
///
/// This function initializes the server, sets up reflection services, and registers
/// the Friends, Users, and Misc gRPC services. Optionally, it enables and registers
/// administrative services (UsersAdmin and GamesAdmin) if `enable_admin_services` is true.
pub async fn start_server(
    logger: Logger,
    storage: Arc<Storage>,
    server_addr: SocketAddr,
    debug_config: Arc<DebugConfig>,
    enable_admin_services: bool,
    reflection: bool,
    friends_mode: FriendsMode,
    print_admin_key: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Kept across restarts, so logged-in launchers and games stay logged in.
    let key = crate::keys::load_or_create(std::path::Path::new(crate::keys::API_KEY_FILE))?;
    info!(logger, "Listening on {server_addr}");
    // Reflection lists every call to anyone who asks; only for debugging.
    let (reflection_v1alpha, reflection_v1) = if reflection {
        (
            Some(
                tonic_reflection::server::Builder::configure()
                    .register_encoded_file_descriptor_set(users::FILE_DESCRIPTOR_SET)
                    .register_encoded_file_descriptor_set(friends::FILE_DESCRIPTOR_SET)
                    .register_encoded_file_descriptor_set(misc::FILE_DESCRIPTOR_SET)
                    .build_v1alpha()
                    .unwrap(),
            ),
            Some(
                tonic_reflection::server::Builder::configure()
                    .register_encoded_file_descriptor_set(users::FILE_DESCRIPTOR_SET)
                    .register_encoded_file_descriptor_set(friends::FILE_DESCRIPTOR_SET)
                    .register_encoded_file_descriptor_set(misc::FILE_DESCRIPTOR_SET)
                    .build_v1()
                    .unwrap(),
            ),
        )
    } else {
        (None, None)
    };
    let builder = Server::builder()
        .add_optional_service(reflection_v1alpha)
        .add_optional_service(reflection_v1)
        .add_service(authenticated(
            FriendsServer::new(MyFriends {
                logger: logger.clone(),
                storage: Arc::clone(&storage),
                debug_config: Arc::clone(&debug_config),
                mode: friends_mode,
            }),
            logger.clone(),
            key.clone(),
            Arc::clone(&storage),
        ))
        .add_service(authenticated(
            MiscServer::new(MyMisc {
                logger: logger.clone(),
                storage: Arc::clone(&storage),
                debug_config,
            })
            // A player's report carries their logs (reports.rs keeps them under 6 MB).
            .max_decoding_message_size(8 * 1024 * 1024),
            logger.clone(),
            key.clone(),
            Arc::clone(&storage),
        ))
        .add_service(UsersServer::new(MyUsers {
            logger: logger.clone(),
            storage: Arc::clone(&storage),
            key,
        }));

    let builder = if enable_admin_services {
        warn!(logger, "Enabling admin services");
        let preshared = base32(&crate::keys::load_or_create(std::path::Path::new(crate::keys::ADMIN_KEY_FILE))?.0);
        // The launcher (which started this server) reads it from stdout; anyone else reads
        // the file, so it doesn't end up in service logs.
        if print_admin_key {
            println!("Admin Key: {preshared}");
        }
        crate::keys::write_secret_text(std::path::Path::new(ADMIN_KEY_TEXT_FILE), &preshared)?;
        info!(logger, "Admin API on; the key is in {ADMIN_KEY_TEXT_FILE}");
        builder
            .add_service(preshared_authentication(
                UsersAdminServer::new(MyUsersAdmin {
                    logger: logger.clone(),
                    storage: Arc::clone(&storage),
                }),
                preshared.clone(),
            ))
            .add_service(preshared_authentication(GamesAdminServer::new(MyGamesAdmin { logger, storage }), preshared))
    } else {
        builder
    };

    builder.serve(server_addr).await?;
    Ok(())
}
