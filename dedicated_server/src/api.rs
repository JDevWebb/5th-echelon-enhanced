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

/// How long a sign-in token is good for. The game and the overlay sign in
/// again on their own when one runs out.
const TOKEN_LIFETIME_SECS: i64 = 30 * 24 * 60 * 60;

/// The caller's user id, put there by [`check_token`].
fn caller<T>(request: &Request<T>) -> Result<u32, Status> {
    request
        .metadata()
        .get("user_id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| Status::unauthenticated("Not signed in"))
}

fn internal(e: impl std::fmt::Display) -> Status {
    Status::internal(e.to_string())
}

/// A new sign-in token for `user_id`: the id and the time, sealed with the
/// server's key.
fn issue_token(key: &Key, user_id: u32) -> String {
    let plain = format!("{user_id}:{}", identity::now());
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
    if !name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return Err("names can have letters, digits, _, - and . only");
    }
    Ok(())
}

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
        person.map_err(internal)?.filter(|p| !p.ubi_id.is_empty()).ok_or_else(|| friend_error(FriendError::NotFound))
    }

    fn player(&self, person: Person, relation: Relation, sessions: Option<&[crate::storage::LiveSession]>) -> friends::Player {
        let activity = sessions
            .filter(|_| person.is_online)
            .and_then(|s| crate::community_api::activity_of(&person.username, s))
            .map(|a| friends::Activity {
                mode: a.mode.into(),
                room: a.room.into(),
                with: a.with,
                map: a.map.unwrap_or_default(),
            });
        let mut p = friends::Player {
            id: person.ubi_id,
            username: person.username,
            is_online: person.is_online || self.debug_config.mark_all_as_online,
            relation: 0,
            activity,
        };
        p.set_relation(relation_proto(relation));
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

        let Some(receiver_id) = self
            .storage
            .find_user_id_by_ubi_id_async(&receiver)
            .await
            .map_err(|e| Status::internal(format!("Couldn't add invite: {e:?}")))?
        else {
            return Err(Status::not_found("User not found"));
        };

        match self.storage.relation(sender, receiver_id).await.map_err(internal)? {
            Relation::Blocked | Relation::BlockedBy => return Err(Status::permission_denied("You can't invite this player")),
            Relation::Friend => {}
            _ if self.mode == FriendsMode::Mutual => return Err(Status::permission_denied("Only friends can invite each other on this server")),
            _ => {}
        }

        // Bind the invitation to the room the sender is actually in. The event the game
        // receives carries only the sender, so without this the invited client has no way to
        // learn which session it is supposed to join. If the host has no joinable session at
        // all, the invitation stays unbound and behaves as it did before room tracking.
        let host_sessions = self
            .storage
            .find_host_sessions_async(sender)
            .await
            .map_err(|e| Status::internal(format!("Couldn't resolve invite room: {e:?}")))?;
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
        let people = match self.mode {
            FriendsMode::Everyone => {
                let mut people = self.storage.everyone_for(me).await.map_err(internal)?;
                // The list always had the player themselves in it; kept, as the game got it.
                people.extend(self.storage.find_person(me).await.map_err(internal)?.filter(|p| !p.ubi_id.is_empty()));
                people
            }
            FriendsMode::Mutual => self.storage.friends_of(me).await.map_err(internal)?,
        };
        // Which session everybody is in. The game looks for a friend's session right here in
        // the friend list - it never asks separately - so an accepted invitation is dead
        // without it.
        let sessions = self.storage.list_advertised_sessions_async().await.map_err(internal)?;
        let friends = people
            .into_iter()
            .map(|u| {
                let (session_id, invite_only, session_data) = sessions.get(&u.id).cloned().unwrap_or_default();
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

        self.storage
            .set_advertised_session_async(user_id, (request.session_id != 0).then_some(request.session_id), request.invite_only, &request.session_data)
            .await
            .map_err(|e| Status::internal(format!("Couldn't store session: {e:?}")))?;

        Ok(Response::new(friends::SetSessionResponse {}))
    }

    async fn relationships(&self, request: Request<friends::RelationshipsRequest>) -> Result<Response<friends::RelationshipsResponse>, Status> {
        let me = caller(&request)?;
        federation::pull_now_and_then(me);
        let sessions = self.storage.presence_async().await.map_err(internal)?.1;
        let list = |people: Vec<Person>, relation: Relation| -> Vec<friends::Player> {
            people.into_iter().map(|p| self.player(p, relation, Some(&sessions))).collect()
        };
        Ok(Response::new(friends::RelationshipsResponse {
            friends: list(self.storage.friends_of(me).await.map_err(internal)?, Relation::Friend),
            requests_received: list(self.storage.friend_requests(me, true).await.map_err(internal)?, Relation::RequestReceived),
            requests_sent: list(self.storage.friend_requests(me, false).await.map_err(internal)?, Relation::RequestSent),
            blocked: list(self.storage.blocked_by(me).await.map_err(internal)?, Relation::Blocked),
            mode: match self.mode {
                FriendsMode::Everyone => "everyone",
                FriendsMode::Mutual => "mutual",
            }
            .into(),
        }))
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
        let found = self.storage.search_players(me, query.trim(), 25).await.map_err(internal)?;
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
        let me = caller(&request)?;
        let request = request.into_inner();
        let person = self.storage.find_person(me).await.map_err(internal)?.ok_or_else(|| Status::unauthenticated("Unknown user"))?;
        if !identity::is_global_id(&request.global_id) {
            return Err(Status::invalid_argument("Not an identity"));
        }
        if !identity::fresh(request.time, identity::now()) {
            return Err(Status::invalid_argument("The signature's time is off; check this PC's clock"));
        }
        let message = identity::link_message(federation::server_id(), &person.username, request.time);
        if !identity::verify(&request.global_id, &message, &request.signature) {
            return Err(Status::permission_denied("The signature doesn't match"));
        }
        if person.global_id.as_deref() == Some(request.global_id.as_str()) {
            return Ok(Response::new(friends::LinkIdentityResponse {}));
        }
        if person.global_id.is_some() {
            return Err(Status::already_exists("This account is linked to another identity"));
        }
        self.storage.link_global_id(me, &request.global_id).await.map_err(|e| Status::already_exists(e.to_string()))?;
        info!(self.logger, "{} ({me}) linked to identity {}", person.username, identity::short(&request.global_id));
        let link = federation::Change::Link {
            global_id: request.global_id,
            username: person.username,
            time: request.time,
            signature: request.signature,
        };
        federation::linked(&self.logger, &self.storage, me, link).await;
        Ok(Response::new(friends::LinkIdentityResponse {}))
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
    // "<user id>:<issued>"; tokens from before they had a time are refused, so their
    // holders sign in again (the game and the overlay do that on their own).
    let (user_id, issued) = plain.split_once(':').ok_or(Status::unauthenticated("Token expired"))?;
    let issued: i64 = issued.parse().map_err(|_| Status::unauthenticated("Invalid token"))?;
    if identity::now() - issued > TOKEN_LIFETIME_SECS {
        return Err(Status::unauthenticated("Token expired"));
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
        let peer = client_addr(&request);
        if crate::rate_limit::logins().blocked(peer) {
            return Err(Status::resource_exhausted("Too many failed logins; try again later"));
        }
        let request = request.into_inner();
        let username = request.username;
        let password = request.password;

        let maybe_user = self
            .storage
            .login_user_async(&username, &password)
            .await
            .map_err(|e| Status::internal(format!("Login error: {e:?}")))?;

        let user_id = maybe_user.map_err(|err| {
            crate::rate_limit::logins().record(peer);
            match err {
                LoginError::InvalidPassword => Status::unauthenticated("Invalid login"),
                LoginError::NotFound => Status::not_found("Unknown user"),
            }
        })?;

        info!(self.logger, "Login successful for {username}");
        self.signed_in(user_id).await
    }

    /// Handles user registration requests.
    ///
    /// Registers a new user in the storage, handling potential conflicts like duplicate usernames or Ubisoft IDs.
    async fn register(&self, request: Request<users::RegisterRequest>) -> Result<Response<users::RegisterResponse>, Status> {
        if !crate::rate_limit::registrations().check(client_addr(&request)) {
            return Err(Status::resource_exhausted("Too many new accounts from this address; try again later"));
        }
        let request = request.into_inner();
        let username = request.username.trim().to_string();
        let password = request.password;
        check_username(&username).map_err(Status::invalid_argument)?;
        if password.len() < 8 || password.len() > 128 {
            return Err(Status::invalid_argument("Passwords are 8 to 128 characters"));
        }
        // The account id is the name, set here: a client can't claim someone else's.
        let ubi_id = username.clone();

        let error = if let Err(err) = self.storage.register_user_async(&username, &password, Some(&ubi_id)).await {
            match err.downcast::<sqlx::Error>() {
                Ok(sqlx::Error::Database(db_err)) => {
                    if db_err.is_unique_violation() {
                        return Err(Status::already_exists(String::from("Username already taken or Ubisoft ID already registered")));
                    }
                    return Err(Status::internal(db_err.to_string()));
                }
                Ok(err) => return Err(Status::internal(err.to_string())),
                Err(err) => err.to_string(),
            }
        } else {
            String::new()
        };
        info!(self.logger, "New user {username} registered");
        Ok(Response::new(users::RegisterResponse {
            error,
            user: Some(User {
                id: ubi_id,
                username,
                ips: vec![],
            }),
        }))
    }

    /// Signs in with the identity key the account is linked to (see `identity`).
    async fn key_login(&self, request: Request<users::KeyLoginRequest>) -> Result<Response<users::LoginResponse>, Status> {
        let peer = client_addr(&request);
        if crate::rate_limit::logins().blocked(peer) {
            return Err(Status::resource_exhausted("Too many failed logins; try again later"));
        }
        let request = request.into_inner();
        let refused = |why: &str| {
            crate::rate_limit::logins().record(peer);
            Status::unauthenticated(why.to_string())
        };
        let person = self
            .storage
            .find_person_by_name(&request.username)
            .await
            .map_err(internal)?
            .ok_or_else(|| refused("Unknown user"))?;
        if person.global_id.as_deref() != Some(request.global_id.as_str()) {
            return Err(refused("This account isn't linked to that identity"));
        }
        if !identity::fresh(request.time, identity::now()) {
            return Err(Status::invalid_argument("The signature's time is off; check this PC's clock"));
        }
        let message = identity::login_message(federation::server_id(), &person.username, request.time);
        if !identity::verify(&request.global_id, &message, &request.signature) {
            return Err(refused("The signature doesn't match"));
        }
        if !self.storage.use_key_login(person.id, request.time).await.map_err(internal)? {
            return Err(refused("That signature was already used"));
        }
        if !request.new_password.is_empty() {
            if request.new_password.len() < 8 || request.new_password.len() > 128 {
                return Err(Status::invalid_argument("Passwords are 8 to 128 characters"));
            }
            self.storage.set_password(person.id, &request.new_password).await.map_err(internal)?;
            info!(self.logger, "{} set a new password with their identity key", person.username);
        }
        info!(self.logger, "Key login successful for {}", person.username);
        self.signed_in(person.id).await
    }
}

impl MyUsers {
    /// The answer to a successful sign-in: a token, and who they are (the
    /// account id the game should use).
    async fn signed_in(&self, user_id: u32) -> Result<Response<users::LoginResponse>, Status> {
        let person = self.storage.find_person(user_id).await.map_err(internal)?;
        Ok(Response::new(users::LoginResponse {
            error: String::new(),
            token: issue_token(&self.key, user_id),
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

        let Some(invite) = self.storage.take_invite_async(user_id).await.map_err(|e| {
            error!(self.logger, "Error getting latest invite for user: {e}");
            Status::internal(format!("{e:?}"))
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
            Status::internal(format!("{e:?}"))
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
    async fn test_p2p(&self, request: Request<misc::TestP2pRequest>) -> Result<Response<misc::TestP2pResponse>, Status> {
        let mut client_addr = request.remote_addr().ok_or(Status::failed_precondition("no client address"))?;
        client_addr.set_port(13_000);
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
                        creator: s.participants.iter().find(|p| p.user_id == s.creator_id).unwrap().name.clone(),
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
            let this = check_token(&logger, &key, &storage, req).await;
            if let Err(ref e) = this {
                error!(logger, "Auth failure: {e}");
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
            }),
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
        // The launcher reads it from stdout; an operator from the file.
        println!("Admin Key: {preshared}");
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
