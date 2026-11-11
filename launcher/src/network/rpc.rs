//! Implements client-side logic for interacting with the server's gRPC API
//! for user authentication.
//!
//! This module provides functions for testing user logins and registering new users.

use server_api::users::users_client::UsersClient;
use server_api::users::LoginRequest;
use server_api::users::RegisterRequest;

use super::Error;

/// Signs in as `username` (the one way every call here does), unless a past refusal says not to
/// yet (sign_in_gate.rs: an outdated launcher never again, a wrong password three times then 30
/// minutes off, a server out of reach after a growing wait); notes how it went. The channel and
/// the server's answer.
pub(super) async fn login(api_url: &str, username: &str, password: &str) -> Result<(tonic::transport::Channel, server_api::users::LoginResponse), Error> {
    super::sign_in_gate::gated(api_url, username, password, async {
        let channel = super::endpoint(api_url)?.connect().await.map_err(|_| Error::ConnectionFailed)?;
        let resp = UsersClient::new(channel.clone())
            .login(LoginRequest {
                username: username.to_string(),
                password: password.to_string(),
                client: super::CLIENT.into(),
                build: super::build(),
                hardware: super::hardware(api_url),
                vm: super::vm(),
                nat_key: vec![],
                graphics: String::new(),
                files: Vec::new(),
                other_files: Vec::new(),
                relay_pings: Vec::new(),
                nat_off: false,
            })
            .await
            .map_err(|status| match status.code() {
                tonic::Code::Unauthenticated => Error::InvalidPassword,
                tonic::Code::NotFound => Error::UserNotFound,
                _ => Error::Rpc(status),
            })?
            .into_inner();
        if resp.error.is_empty() {
            Ok((channel, resp))
        } else {
            Err(Error::ServerFailure(resp.error))
        }
    })
    .await
}

/// Tests a user login against the API server.
///
/// # Arguments
///
/// * `api_url` - The URL of the API server.
/// * `username` - The username to test.
/// * `password` - The password to test.
///
/// # Returns
///
/// An `Ok(())` if the login is successful, or an `Error` otherwise.
pub async fn test_login(api_url: String, username: &str, password: &str) -> Result<(), Error> {
    login(&api_url, username, password).await.map(|_| ())
}

/// Registers a new user with the API server.
///
/// # Arguments
///
/// * `api_url` - The URL of the API server.
/// * `username` - The desired username.
/// * `password` - The desired password.
/// * `ubi_id` - The user's Ubisoft ID.
///
/// # Returns
///
/// An `Ok(())` if the registration is successful, or an `Error` otherwise.
/// With `identity` (the player's, and the host they reached the server by),
/// the account is linked at once and its name reserved across servers
/// sharing friends.
pub async fn register(api_url: String, username: &str, password: &str, identity: Option<(&identity::Identity, &str)>) -> Result<(), Error> {
    let Ok(mut client) = super::endpoint(&api_url)?.connect().await.map(UsersClient::new) else {
        return Err(Error::ConnectionFailed);
    };
    let time = identity::now();
    let (global_id, signature) = identity.map_or_else(Default::default, |(id, host)| (id.global_id(), id.sign_link(host, username, time)));
    let host = identity.map(|(_, host)| identity::host_key(host)).unwrap_or_default();

    let resp = match client
        .register(RegisterRequest {
            username: username.to_string(),
            password: password.to_string(),
            ubi_id: String::new(),
            global_id,
            time,
            signature,
            host,
            client: super::CLIENT.into(),
            build: super::build(),
        })
        .await
    {
        Ok(resp) => resp,
        Err(status) => {
            // Handle different gRPC status codes.
            // The server's reason (a name it won't take, say) is worth showing.
            if status.code() == tonic::Code::AlreadyExists && !status.message().contains("identity") {
                return Err(Error::UsernameAlreadyTaken);
            }
            return Err(Error::Rpc(status));
        }
    };

    let resp = resp.into_inner();
    if resp.error.is_empty() {
        Ok(())
    } else {
        Err(Error::ServerFailure(resp.error))
    }
}

/// The account id the server gave `username` (the game's "Ubisoft id").
pub async fn account_id(api_url: String, username: &str, password: &str) -> Result<String, Error> {
    let (_, resp) = login(&api_url, username, password).await?;
    Ok(resp.user.map(|u| u.id).filter(|id| !id.is_empty()).unwrap_or_else(|| username.to_string()))
}

/// Renames the account (signed with `identity` for `host`, the server as the player reached it, when
/// it's linked). Answers the new name.
pub async fn rename(api_url: String, username: &str, password: &str, new_name: &str, identity: Option<(&identity::Identity, &str)>) -> Result<String, Error> {
    let host = identity.map(|(_, host)| identity::host_key(host)).unwrap_or_default();
    let (channel, resp) = login(&api_url, username, password).await?;
    let token = resp.token;
    let token: tonic::metadata::MetadataValue<_> = token.parse().map_err(|_| Error::ServerFailure("bad token".into()))?;
    let mut client = server_api::friends::friends_client::FriendsClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
        req.metadata_mut().insert("authorization", token.clone());
        Ok(req)
    });
    let time = identity::now();
    let signature = identity.map(|(id, host)| id.sign_link(host, new_name, time)).unwrap_or_default();
    let resp = client
        .rename(server_api::friends::RenameRequest {
            new_name: new_name.to_string(),
            time,
            signature,
            host,
        })
        .await?
        .into_inner();
    Ok(resp.username)
}

/// Signs in with the player's identity key (signed for `host`, the server as
/// the player reached it) to whichever account there is linked to it,
/// setting `new_password`: its name, or None when the identity has no
/// account there.
pub async fn identity_login(api_url: String, identity: &identity::Identity, host: &str, new_password: &str) -> Result<Option<String>, Error> {
    let global_id = identity.global_id();
    super::sign_in_gate::gated(&api_url, &global_id, "", identity_login_now(&api_url, identity, host, new_password)).await
}

async fn identity_login_now(api_url: &str, identity: &identity::Identity, host: &str, new_password: &str) -> Result<Option<String>, Error> {
    let Ok(mut client) = super::endpoint(api_url)?.connect().await.map(UsersClient::new) else {
        return Err(Error::ConnectionFailed);
    };
    let time = identity::now();
    let request = server_api::users::KeyLoginRequest {
        username: String::new(),
        global_id: identity.global_id(),
        time,
        signature: identity.sign_login(host, "", time, new_password),
        new_password: new_password.to_string(),
        host: identity::host_key(host),
        client: super::CLIENT.into(),
        build: super::build(),
        hardware: super::hardware(api_url),
        vm: super::vm(),
    };
    match client.key_login(request).await {
        Ok(resp) => Ok(resp.into_inner().user.map(|u| u.username).filter(|u| !u.is_empty())),
        Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
        Err(status) => Err(Error::Rpc(status)),
    }
}

/// Signs in as `username` and links the account to the player's identity,
/// so friends follow them to other servers sharing a coordinator.
pub async fn link_identity(api_url: String, identity: &identity::Identity, host: &str, username: &str, password: &str) -> Result<(), Error> {
    let (channel, resp) = login(&api_url, username, password).await?;
    let token = resp.token;
    let token: tonic::metadata::MetadataValue<_> = token.parse().map_err(|_| Error::ServerFailure("bad token".into()))?;
    let mut client = server_api::friends::friends_client::FriendsClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
        req.metadata_mut().insert("authorization", token.clone());
        Ok(req)
    });
    let time = identity::now();
    client
        .link_identity(server_api::friends::LinkIdentityRequest {
            global_id: identity.global_id(),
            time,
            signature: identity.sign_link(host, username, time),
            host: identity::host_key(host),
        })
        .await?;
    Ok(())
}

/// Signs in and answers the API token (the launcher keeps it, so it needn't
/// sign in for every call).
pub async fn sign_in(api_url: String, username: &str, password: &str) -> Result<String, Error> {
    login(&api_url, username, password).await.map(|(_, resp)| resp.token)
}

/// The signed-in player's friends, requests and friends on other servers
/// sharing friends. A token the server no longer takes is `InvalidPassword`.
pub async fn relationships(api_url: String, token: &str) -> Result<server_api::friends::RelationshipsResponse, Error> {
    let channel = super::endpoint(&api_url)?.connect().await.map_err(|_| Error::ConnectionFailed)?;
    let mut client = server_api::friends::friends_client::FriendsClient::new(channel);
    let mut request = tonic::Request::new(server_api::friends::RelationshipsRequest {});
    let token = tonic::metadata::MetadataValue::try_from(token).map_err(|_| Error::InvalidPassword)?;
    request.metadata_mut().insert("authorization", token);
    match client.relationships(request).await {
        Ok(resp) => Ok(resp.into_inner()),
        Err(status) if status.code() == tonic::Code::Unauthenticated => Err(Error::InvalidPassword),
        Err(status) => Err(status.into()),
    }
}

/// The friends API with the session token, on `api_url`.
async fn friends_client(
    api_url: &str,
    token: &str,
) -> Result<
    (
        server_api::friends::friends_client::FriendsClient<tonic::transport::Channel>,
        tonic::metadata::MetadataValue<tonic::metadata::Ascii>,
    ),
    Error,
> {
    let channel = super::endpoint(api_url)?.connect().await.map_err(|_| Error::ConnectionFailed)?;
    let token = tonic::metadata::MetadataValue::try_from(token).map_err(|_| Error::InvalidPassword)?;
    Ok((server_api::friends::friends_client::FriendsClient::new(channel), token))
}

fn authed<T>(message: T, token: &tonic::metadata::MetadataValue<tonic::metadata::Ascii>) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert("authorization", token.clone());
    request
}

/// Who's online on the server, as the overlay's Find players lists them before anything is
/// typed (an empty search): everyone in the "everyone" friends mode, only friends in
/// "mutual"; never anyone blocked either way, nor the caller.
pub async fn online_players(api_url: String, token: &str) -> Result<Vec<server_api::friends::Player>, Error> {
    search_players(api_url, token, "").await
}

/// Players whose names hold `query` (the overlay's Find players; at most 32 characters, as the
/// server takes), never anyone blocked either way, nor the caller.
pub async fn search_players(api_url: String, token: &str, query: &str) -> Result<Vec<server_api::friends::Player>, Error> {
    let (mut client, token) = friends_client(&api_url, token).await?;
    match client.search(authed(server_api::friends::SearchRequest { query: query.to_string() }, &token)).await {
        Ok(resp) => Ok(resp.into_inner().players),
        Err(status) if status.code() == tonic::Code::Unauthenticated => Err(Error::InvalidPassword),
        Err(status) => Err(status.into()),
    }
}

/// Something the player does to another from the launcher's list of who's online.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerAction {
    /// Asks them to be friends (accepts, if they'd asked).
    Request,
    /// Invites them to the game session the player is in (needs the game running).
    Invite,
    Block,
    /// Takes their request.
    Accept,
    /// Turns their request down.
    Decline,
    /// Takes the player's own request back (the server's decline, worded apart).
    Cancel,
    /// Ends the friendship.
    Remove,
    Unblock,
}

/// Does `action` to the player with account id `id` (none known, from chat: by their identity,
/// never by a name, which may be another account's here).
pub async fn player_action(api_url: String, token: &str, action: PlayerAction, id: &str, identity: &str) -> Result<(), Error> {
    let (mut client, token) = friends_client(&api_url, token).await?;
    let target = || server_api::friends::TargetRequest {
        id: id.to_string(),
        username: String::new(),
        identity: if id.is_empty() { identity.to_string() } else { String::new() },
    };
    let result = match action {
        PlayerAction::Request => client.request(authed(target(), &token)).await.map(|_| ()),
        PlayerAction::Block => client.block(authed(target(), &token)).await.map(|_| ()),
        PlayerAction::Invite => client.invite(authed(server_api::friends::InviteRequest { id: id.to_string() }, &token)).await.map(|_| ()),
        PlayerAction::Accept => client.accept(authed(target(), &token)).await.map(|_| ()),
        PlayerAction::Decline | PlayerAction::Cancel => client.decline(authed(target(), &token)).await.map(|_| ()),
        PlayerAction::Remove => client.remove(authed(target(), &token)).await.map(|_| ()),
        PlayerAction::Unblock => client.unblock(authed(target(), &token)).await.map(|_| ()),
    };
    match result {
        Ok(()) => Ok(()),
        Err(status) if status.code() == tonic::Code::Unauthenticated => Err(Error::InvalidPassword),
        Err(status) => Err(status.into()),
    }
}

/// Asks the server to join a friend's game (`JoinNow`): it queues an invitation for the caller's
/// game, marked as a join they asked for. `friend_id` is the friend's account id.
pub async fn join_now(api_url: String, token: &str, friend_id: String) -> Result<server_api::friends::JoinNowResponse, Error> {
    let (mut client, token) = friends_client(&api_url, token).await?;
    match client.join_now(authed(server_api::friends::JoinNowRequest { id: friend_id }, &token)).await {
        Ok(resp) => Ok(resp.into_inner()),
        Err(status) if status.code() == tonic::Code::Unauthenticated => Err(Error::InvalidPassword),
        // The server's own words for a refusal (already in that game, too many requests),
        // cut short like anything it sends; not the RPC's dump.
        Err(status)
            if matches!(
                status.code(),
                tonic::Code::InvalidArgument | tonic::Code::ResourceExhausted | tonic::Code::FailedPrecondition
            ) && !status.message().is_empty() =>
        {
            Err(Error::ServerFailure(hooks_config::text::clip(status.message(), 200)))
        }
        Err(status) => Err(status.into()),
    }
}

/// The server's own words for a refusal (already in that game, too many requests), cut short
/// like anything it sends; not the RPC's dump.
fn refusal(status: tonic::Status) -> Error {
    match status.code() {
        tonic::Code::Unauthenticated => Error::InvalidPassword,
        tonic::Code::InvalidArgument | tonic::Code::ResourceExhausted | tonic::Code::FailedPrecondition if !status.message().is_empty() => {
            Error::ServerFailure(hooks_config::text::clip(status.message(), 200))
        }
        _ => status.into(),
    }
}

/// Joins a public match picked from the lobby browser by its room (`JoinRoom`): as
/// [`join_now`], an invitation from its host marked as a join the caller asked for; the answer
/// names the host (`host_id`) for the game's join request.
pub async fn join_room(api_url: String, token: &str, room: u32) -> Result<server_api::friends::JoinNowResponse, Error> {
    let (mut client, token) = friends_client(&api_url, token).await?;
    client
        .join_room(authed(server_api::friends::JoinRoomRequest { room }, &token))
        .await
        .map(tonic::Response::into_inner)
        .map_err(refusal)
}

/// In line for a match under way or full, kept there, or out of it (`on` false): `QueueRoom`.
pub async fn queue_room(api_url: String, token: &str, room: u32, on: bool) -> Result<server_api::friends::QueueRoomResponse, Error> {
    let (mut client, token) = friends_client(&api_url, token).await?;
    client
        .queue_room(authed(server_api::friends::QueueRoomRequest { room, on }, &token))
        .await
        .map(tonic::Response::into_inner)
        .map_err(refusal)
}

/// The public matches in full, for the Matches tab (`Lobbies`).
pub async fn lobbies(api_url: String, token: &str) -> Result<server_api::friends::LobbiesResponse, Error> {
    let (mut client, token) = friends_client(&api_url, token).await?;
    client
        .lobbies(authed(server_api::friends::LobbiesRequest {}, &token))
        .await
        .map(tonic::Response::into_inner)
        .map_err(refusal)
}

/// Asks for a new match (`NewMatch`): the player's searches find nothing until they host one,
/// so the game starts a fresh public match. `on` false takes it back. How long it holds.
pub async fn new_match(api_url: String, token: &str, on: bool) -> Result<std::time::Duration, Error> {
    let (mut client, token) = friends_client(&api_url, token).await?;
    let resp = client.new_match(authed(server_api::friends::NewMatchRequest { on }, &token)).await.map_err(refusal)?;
    Ok(std::time::Duration::from_secs(resp.into_inner().seconds.into()))
}

/// A session token for the account, and a channel to its server.
async fn signed_in(api_url: &str, username: &str, password: &str) -> Result<(tonic::transport::Channel, tonic::metadata::MetadataValue<tonic::metadata::Ascii>), Error> {
    let (channel, resp) = login(api_url, username, password).await?;
    let token = resp.token.parse().map_err(|_| Error::ServerFailure("bad token".into()))?;
    Ok((channel, token))
}

/// What the server saw of the account's last game session (feedback.rs).
pub async fn session_summary(api_url: String, username: &str, password: &str) -> Result<server_api::misc::SessionSummaryResponse, Error> {
    let (channel, token) = signed_in(&api_url, username, password).await?;
    let mut client = server_api::misc::misc_client::MiscClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
        req.metadata_mut().insert("authorization", token.clone());
        Ok(req)
    });
    Ok(client.session_summary(server_api::misc::SessionSummaryRequest {}).await?.into_inner())
}

/// Sends the player's report to the server; answers its id. `on_sent` hears how many of
/// the request's bytes have gone (see [`report_size`] for all of them).
pub async fn send_report(
    api_url: String,
    username: &str,
    password: &str,
    report: server_api::misc::ReportRequest,
    on_sent: impl Fn(u64) + Send + Sync + 'static,
) -> Result<String, Error> {
    let (channel, token) = signed_in(&api_url, username, password).await?;
    let counted = upload::Counted::new(channel, on_sent);
    let mut client = server_api::misc::misc_client::MiscClient::with_interceptor(counted, move |mut req: tonic::Request<()>| {
        req.metadata_mut().insert("authorization", token.clone());
        Ok(req)
    });
    Ok(client.report(report).await?.into_inner().id)
}

/// Whether `name` is free for a new account on the server (`NameAvailable`): its answer, and why.
pub async fn name_available(api_url: String, name: &str) -> Result<(server_api::users::name_available_response::Answer, String), Error> {
    let Ok(mut client) = super::endpoint(&api_url)?.connect().await.map(UsersClient::new) else {
        return Err(Error::ConnectionFailed);
    };
    let answer = client.name_available(server_api::users::NameRequest { name: name.to_string() }).await?.into_inner();
    Ok((answer.answer(), answer.reason))
}

/// The bytes a report takes on the wire: the message and gRPC's 5-byte frame header.
pub fn report_size(report: &server_api::misc::ReportRequest) -> u64 {
    use prost::Message as _;
    report.encoded_len() as u64 + 5
}

/// Counting a request's bytes as they're sent, for the activity bar's upload progress.
mod upload {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::Context;
    use std::task::Poll;

    use bytes::Bytes;
    use http_body::Frame;
    use tonic::body::Body;
    use tonic::transport::Channel;
    use tower_service::Service;

    /// The request goes in pieces this size, so the count follows what the connection takes
    /// (it asks for the next piece once it has room for it).
    const PIECE: usize = 32 * 1024;

    /// A channel whose requests report their bytes sent.
    #[derive(Clone)]
    pub struct Counted {
        inner: Channel,
        on_sent: Arc<dyn Fn(u64) + Send + Sync>,
    }

    impl Counted {
        pub fn new(inner: Channel, on_sent: impl Fn(u64) + Send + Sync + 'static) -> Self {
            Self {
                inner,
                on_sent: Arc::new(on_sent),
            }
        }
    }

    impl Service<http::Request<Body>> for Counted {
        type Response = <Channel as Service<http::Request<Body>>>::Response;
        type Error = <Channel as Service<http::Request<Body>>>::Error;
        type Future = <Channel as Service<http::Request<Body>>>::Future;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Service::<http::Request<Body>>::poll_ready(&mut self.inner, cx)
        }

        fn call(&mut self, request: http::Request<Body>) -> Self::Future {
            let on_sent = Arc::clone(&self.on_sent);
            let request = request.map(|inner| {
                Body::new(Counting {
                    inner,
                    rest: Bytes::new(),
                    sent: 0,
                    on_sent,
                })
            });
            self.inner.call(request)
        }
    }

    /// A body handing on its data in pieces, counting them.
    pub struct Counting {
        inner: Body,
        rest: Bytes,
        sent: u64,
        on_sent: Arc<dyn Fn(u64) + Send + Sync>,
    }

    impl http_body::Body for Counting {
        type Data = Bytes;
        type Error = tonic::Status;

        fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, tonic::Status>>> {
            let this = &mut *self;
            if this.rest.is_empty() {
                match Pin::new(&mut this.inner).poll_frame(cx) {
                    Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                        Ok(data) => this.rest = data,
                        Err(frame) => return Poll::Ready(Some(Ok(frame))),
                    },
                    other => return other,
                }
            }
            let piece = this.rest.split_to(this.rest.len().min(PIECE));
            this.sent += piece.len() as u64;
            (this.on_sent)(this.sent);
            Poll::Ready(Some(Ok(Frame::data(piece))))
        }

        fn is_end_stream(&self) -> bool {
            self.rest.is_empty() && self.inner.is_end_stream()
        }

        fn size_hint(&self) -> http_body::SizeHint {
            let inner = self.inner.size_hint();
            let rest = self.rest.len() as u64;
            let mut hint = http_body::SizeHint::new();
            hint.set_lower(inner.lower() + rest);
            if let Some(upper) = inner.upper() {
                hint.set_upper(upper + rest);
            }
            hint
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::AtomicU64;
        use std::sync::atomic::Ordering;

        use http_body::Body as _;

        use super::*;

        #[test]
        fn bodies_go_in_counted_pieces() {
            let seen = Arc::new(AtomicU64::new(0));
            let s = Arc::clone(&seen);
            let mut body = Counting {
                inner: Body::new(http_body_util::Full::new(Bytes::from(vec![7u8; PIECE * 2 + 10]))),
                rest: Bytes::new(),
                sent: 0,
                on_sent: Arc::new(move |n| s.store(n, Ordering::SeqCst)),
            };
            let waker = std::task::Waker::noop();
            let mut cx = Context::from_waker(waker);
            let mut sizes = vec![];
            while let Poll::Ready(Some(Ok(frame))) = Pin::new(&mut body).poll_frame(&mut cx) {
                sizes.push(frame.into_data().map(|d| d.len()).unwrap_or(0));
            }
            assert_eq!(sizes, vec![PIECE, PIECE, 10]);
            assert_eq!(seen.load(Ordering::SeqCst), (PIECE * 2 + 10) as u64);
            assert!(body.is_end_stream());
        }
    }
}

/// Chat on the player's server (docs/chat.md), signed in with a session token.
pub struct ChatApi {
    client: server_api::chat::chat_client::ChatClient<tonic::transport::Channel>,
    token: tonic::metadata::MetadataValue<tonic::metadata::Ascii>,
}

/// What a chat call's failure says: a token the server no longer takes is `InvalidPassword`.
fn chat_error(status: tonic::Status) -> Error {
    if status.code() == tonic::Code::Unauthenticated {
        Error::InvalidPassword
    } else {
        Error::ServerFailure(status.message().chars().take(300).collect())
    }
}

impl ChatApi {
    pub async fn connect(api_url: &str, token: &str) -> Result<Self, Error> {
        let channel = super::endpoint(api_url)?.connect().await.map_err(|_| Error::ConnectionFailed)?;
        let token = tonic::metadata::MetadataValue::try_from(token).map_err(|_| Error::InvalidPassword)?;
        Ok(Self {
            client: server_api::chat::chat_client::ChatClient::new(channel),
            token,
        })
    }

    pub async fn status(&mut self) -> Result<server_api::chat::StatusResponse, Error> {
        let r = authed(server_api::chat::StatusRequest {}, &self.token);
        self.client.status(r).await.map(tonic::Response::into_inner).map_err(chat_error)
    }

    pub async fn send(&mut self, room: server_api::chat::Room, to: String, text: String) -> Result<server_api::chat::SendResponse, Error> {
        let r = authed(server_api::chat::SendRequest { room: room.into(), to, text }, &self.token);
        self.client.send(r).await.map(tonic::Response::into_inner).map_err(chat_error)
    }

    /// The messages after `after`: at once, or when one comes, or after about 25 s.
    pub async fn wait(&mut self, after: i64) -> Result<server_api::chat::WaitResponse, Error> {
        let r = authed(server_api::chat::WaitRequest { after, wait_seconds: 0 }, &self.token);
        self.client.wait(r).await.map(tonic::Response::into_inner).map_err(chat_error)
    }

    pub async fn history(&mut self, room: server_api::chat::Room, with: String, before: i64) -> Result<Vec<server_api::chat::Message>, Error> {
        let r = authed(
            server_api::chat::HistoryRequest {
                room: room.into(),
                with,
                before,
                limit: 100,
            },
            &self.token,
        );
        self.client.history(r).await.map(|r| r.into_inner().messages).map_err(chat_error)
    }

    pub async fn conversations(&mut self) -> Result<Vec<server_api::chat::Conversation>, Error> {
        let r = authed(server_api::chat::ConversationsRequest {}, &self.token);
        self.client.conversations(r).await.map(|r| r.into_inner().conversations).map_err(chat_error)
    }

    pub async fn mark_read(&mut self, with: String, up_to: i64) -> Result<(), Error> {
        let r = authed(
            server_api::chat::MarkReadRequest {
                room: server_api::chat::Room::Direct.into(),
                with,
                up_to,
            },
            &self.token,
        );
        self.client.mark_read(r).await.map(|_| ()).map_err(chat_error)
    }

    pub async fn delete(&mut self, id: i64) -> Result<(), Error> {
        let r = authed(server_api::chat::MessageRequest { id, why: String::new() }, &self.token);
        self.client.delete(r).await.map(|_| ()).map_err(chat_error)
    }

    /// Friends who can be messaged, with their identities.
    pub async fn friends(&mut self) -> Result<Vec<server_api::chat::ChatFriend>, Error> {
        let r = authed(server_api::chat::Empty {}, &self.token);
        self.client.friends(r).await.map(|r| r.into_inner().friends).map_err(chat_error)
    }

    pub async fn report(&mut self, id: i64, why: String) -> Result<(), Error> {
        let r = authed(server_api::chat::MessageRequest { id, why }, &self.token);
        self.client.report(r).await.map(|_| ()).map_err(chat_error)
    }
}
