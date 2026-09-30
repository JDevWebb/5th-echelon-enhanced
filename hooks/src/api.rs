use std::future::Future;
use std::sync::Mutex;
use std::sync::OnceLock;

use server_api::friends::friends_client::FriendsClient;
use server_api::friends::InviteRequest;
use server_api::friends::ListRequest;
use server_api::friends::Player;
use server_api::friends::RelationshipsRequest;
use server_api::friends::RelationshipsResponse;
use server_api::friends::SearchRequest;
use server_api::friends::SetSessionRequest;
use server_api::friends::TargetRequest;
use server_api::misc::misc_client::MiscClient;
use server_api::misc::EventRequest;
use server_api::misc::EventResponse;
use server_api::users::users_client::UsersClient;
use server_api::users::LoginRequest;
use tonic::metadata::Ascii;
use tonic::metadata::MetadataValue;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::instrument;

static TOKEN: Mutex<Option<MetadataValue<Ascii>>> = Mutex::new(None);
static CREDS: Mutex<Option<(String, String)>> = Mutex::new(None);
/// The account id the server gave us at sign-in (the game's "Ubisoft id").
static ACCOUNT_ID: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    IO(#[from] std::io::Error),
    #[error("Missing URL for the api server")]
    MissingUrl,
    #[error("Transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("gRPC error: {0}")]
    GRPCStatus(#[from] tonic::Status),
    #[error("Login failure")]
    LoginFailure,
    #[error("Login failure")]
    InvalidToken(#[from] tonic::metadata::errors::InvalidMetadataValue),
    #[error("Not connected")]
    NotConnected,
}

static CONNECTION: OnceLock<tonic::transport::Channel> = OnceLock::new();

async fn create_channel() -> std::result::Result<tonic::transport::Channel, Error> {
    // using get + set here instead of get_or_init/get_or_try_init to support async
    if let Some(channel) = CONNECTION.get() {
        tracing::debug!("Reusing connection {channel:?}");
        return Ok(channel.clone());
    }

    let Some(url) = crate::config::URL.get() else {
        return Err(Error::MissingUrl);
    };
    tracing::debug!("Connecting to {url}");
    let channel = tonic::transport::Channel::from_shared(url.as_str())
        .unwrap() // this should not fail (ideally) as url is a url::Url which gets converted to an Uri
        .connect_timeout(std::time::Duration::from_secs(1))
        .timeout(std::time::Duration::from_secs(10))
        .connect()
        .await?;
    tracing::debug!("Connected to {url}");
    if CONNECTION.set(channel).is_err() {
        tracing::warn!("API connection was already set before");
    }
    CONNECTION.get().cloned().ok_or(Error::NotConnected)
}

macro_rules! connect {
    ($client:ident) => {{
        let channel = create_channel().await?;
        $client::with_interceptor(channel, move |mut req: tonic::Request<_>| {
            let guard = TOKEN.lock().unwrap();
            if let Some(token) = (*guard).as_ref() {
                tracing::debug!("Adding auth token");
                req.metadata_mut().insert("authorization", token.clone());
            }
            Ok(req)
        })
    }};
}

pub struct Friend {
    pub id: String,
    pub username: String,
    pub is_online: bool,
    /// Session this friend is currently in; 0 means none.
    pub session_id: u32,
    /// Their session is private and can only be entered through an invitation.
    pub invite_only: bool,
    /// Opaque payload of that session, exactly as the host handed it to Uplay.
    pub session_data: Vec<u8>,
    /// Principal id as Quazal uses it - needed to search for the session this friend is in.
    pub pid: u32,
}

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn runtime() -> Result<&'static tokio::runtime::Runtime, Error> {
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    // Two workers are plenty for a handful of API calls. A runtime sized to
    // the CPU gave the game dozens of extra threads (31 on a 32-thread CPU,
    // as seen under Wine), next to its own five.
    let _ = RUNTIME.set(tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("fe-api").enable_all().build()?);
    Ok(RUNTIME.get().unwrap())
}

fn run<T>(func: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    runtime()?.block_on(func)
}

/// Whether the server refused a call because our token is no longer good
/// (e.g. the server's key changed, or the account was signed in elsewhere).
fn is_signed_out(e: &Error) -> bool {
    matches!(e, Error::GRPCStatus(s) if s.code() == tonic::Code::Unauthenticated)
}

/// Runs `call`; if the server says we're signed out, signs in again with the
/// saved credentials and tries once more. Players only see an error when
/// signing in again fails too (upstream showed "Relogin required" and left
/// it to them).
async fn signed_in<T, F, Fut>(call: F) -> Result<T, Error>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    match call().await {
        Err(e) if is_signed_out(&e) => {
            info!("Signed out by the server; signing in again");
            if relogin().await {
                call().await
            } else {
                Err(e)
            }
        }
        result => result,
    }
}

pub fn invite_friend(id: &str) -> Result<(), Error> {
    run(signed_in(|| async {
        let mut client = connect!(FriendsClient);

        let request = tonic::Request::new(InviteRequest { id: id.into() });

        client.invite(request).await?;
        Ok(())
    }))
}

pub fn list_friends() -> Result<Vec<Friend>, Error> {
    run(signed_in(|| async {
        let mut client = connect!(FriendsClient);

        let request = tonic::Request::new(ListRequest {});

        let response = client.list(request).await?.into_inner();
        Ok(response
            .friends
            .into_iter()
            .map(|f| Friend {
                id: f.id,
                username: f.username,
                is_online: f.is_online,
                session_id: f.session_id,
                invite_only: f.invite_only,
                session_data: f.session_data,
                pid: f.pid,
            })
            .collect())
    }))
}

/// A session announcement for [`announce_game_session`].
type Announcement = (Option<u32>, bool, Vec<u8>);

/// Queues a session announcement (see [`set_game_session`]) for a background
/// thread, in order, so the game thread that calls `UPLAY_USER_SetGameSession`
/// never waits on the network.
pub fn announce_game_session(session_id: Option<u32>, invite_only: bool, session_data: &[u8]) {
    static QUEUE: OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<Announcement>>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Announcement>();
        let spawned = std::thread::Builder::new().name("session-announcer".into()).spawn(move || {
            for (session_id, invite_only, data) in rx {
                match set_game_session(session_id, invite_only, &data) {
                    Ok(()) => tracing::info!("Session {session_id:?} announced (invite_only={invite_only})"),
                    // Not fatal: invitations into this session won't work, the rest does.
                    Err(e) => tracing::error!("Announcing session {session_id:?} failed: {e:?}"),
                }
            }
        });
        if let Err(e) = spawned {
            tracing::error!("Couldn't start the session announcer: {e}");
        }
        std::sync::Mutex::new(tx)
    });
    if let Ok(tx) = queue.lock() {
        let _ = tx.send((session_id, invite_only, session_data.to_vec()));
    }
}

/// Publishes the session this player is in, so it reaches their friends' friend lists.
///
/// `session_id` of `None` clears the advertisement. Called from
/// `UPLAY_USER_SetGameSession` / `UPLAY_USER_ClearGameSession`.
pub fn set_game_session(session_id: Option<u32>, invite_only: bool, session_data: &[u8]) -> Result<(), Error> {
    run(signed_in(|| async {
        let mut client = connect!(FriendsClient);

        let request = tonic::Request::new(SetSessionRequest {
            session_id: session_id.unwrap_or(0),
            invite_only,
            session_data: session_data.to_vec(),
        });

        client.set_session(request).await?;
        Ok(())
    }))
}

async fn login_async(username: &str, password: &str) -> Result<(), Error> {
    {
        let mut guard = CREDS.lock().unwrap();
        *guard = Some((String::from(username), String::from(password)));
    }

    let mut client = connect!(UsersClient);

    let request = tonic::Request::new(LoginRequest {
        username: String::from(username),
        password: String::from(password),
    });

    debug!("logging in");
    let response = client.login(request).await?.into_inner();
    if !response.error.is_empty() {
        error!("Login error: {}", response.error);
        return Err(Error::LoginFailure);
    } else if !response.token.is_empty() {
        info!("Login successful");
        {
            let mut guard = TOKEN.lock().unwrap();
            *guard = Some(response.token.parse()?);
        }
        if let Some(user) = response.user.filter(|u| !u.id.is_empty()) {
            *ACCOUNT_ID.lock().unwrap() = Some(user.id);
        }
    }
    Ok(())
}

/// The account id the server knows us by, once signed in. The game should
/// use this, not whatever the settings file says.
pub fn account_id() -> Option<String> {
    ACCOUNT_ID.lock().unwrap().clone()
}

/// A change to how we stand to another player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FriendChange {
    Request,
    Accept,
    Decline,
    Remove,
    Block,
    Unblock,
}

/// Our friends, friend requests both ways and blocks.
pub fn relationships() -> Result<RelationshipsResponse, Error> {
    run(signed_in(|| async {
        let mut client = connect!(FriendsClient);
        Ok(client.relationships(tonic::Request::new(RelationshipsRequest {})).await?.into_inner())
    }))
}

/// Players whose name contains `query`, or who's online for an empty one.
pub fn search_players(query: &str) -> Result<Vec<Player>, Error> {
    run(signed_in(|| async {
        let mut client = connect!(FriendsClient);
        let request = tonic::Request::new(SearchRequest { query: query.into() });
        Ok(client.search(request).await?.into_inner().players)
    }))
}

/// Changes how we stand to the player with account id `id`.
pub fn change_friend(change: FriendChange, id: &str) -> Result<(), Error> {
    run(signed_in(|| async {
        let mut client = connect!(FriendsClient);
        let request = tonic::Request::new(TargetRequest {
            id: id.into(),
            username: String::new(),
        });
        match change {
            FriendChange::Request => client.request(request).await?,
            FriendChange::Accept => client.accept(request).await?,
            FriendChange::Decline => client.decline(request).await?,
            FriendChange::Remove => client.remove(request).await?,
            FriendChange::Block => client.block(request).await?,
            FriendChange::Unblock => client.unblock(request).await?,
        };
        Ok(())
    }))
}

/// The name we signed in with, once login has been attempted.
pub fn username() -> Option<String> {
    CREDS.lock().unwrap().as_ref().map(|(username, _)| username.clone())
}

#[instrument(skip(password))]
pub fn login(username: &str, password: &str) -> Result<(), Error> {
    run(login_async(username, password))
}

#[instrument]
pub async fn event() -> Result<EventResponse, Error> {
    signed_in(|| async {
        let mut client = connect!(MiscClient);

        let request = tonic::Request::new(EventRequest {});

        Ok(client.event(request).await?.into_inner())
    })
    .await
}

#[instrument]
pub async fn relogin() -> bool {
    {
        let _ = TOKEN.lock().unwrap().take();
    }
    let (username, password) = {
        let Some((username, password)) = CREDS.lock().unwrap().clone() else {
            return false;
        };
        (username, password)
    };
    login_async(&username, &password).await.is_ok()
}
