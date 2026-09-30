//! Calls to a server's gRPC API: accounts for players, and the admin API for
//! whoever runs the server. Each call has a timeout, so an unreachable server
//! is an error after a few seconds rather than a hang.

use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

use server_api::games;
use server_api::users;
use setup::account::AccountError;
use setup::account::AccountService;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tonic::transport::Endpoint;

use crate::network;

const TIMEOUT: Duration = Duration::from_secs(6);

/// The runtime the gRPC calls run on.
pub fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime"))
}

/// Runs `f` on the runtime from a plain thread, giving up after [`TIMEOUT`].
pub fn block_on<T>(f: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    rt().block_on(async { tokio::time::timeout(TIMEOUT, f).await.map_err(|_| anyhow::anyhow!("the server didn't answer in time"))? })
}

/// A server's account calls, for [`setup::account`].
pub struct Accounts {
    pub api: String,
    /// The player's identity and the server's id, when the server supports
    /// identities: for signing in with the key.
    pub identity: Option<(std::sync::Arc<setup::player_identity::Identity>, String)>,
}

impl Accounts {
    pub fn new(api: String) -> Self {
        Self { api, identity: None }
    }
}

impl AccountService for Accounts {
    fn login(&self, username: &str, password: &str) -> Result<(), AccountError> {
        let r = rt().block_on(async { tokio::time::timeout(TIMEOUT, network::test_login(self.api.clone(), username, password)).await });
        match r {
            Err(_) => Err(AccountError::Other("the server didn't answer in time".into())),
            Ok(Ok(())) => Ok(()),
            Ok(Err(network::Error::InvalidPassword)) => Err(AccountError::WrongPassword),
            Ok(Err(network::Error::UserNotFound)) => Err(AccountError::NotFound),
            Ok(Err(network::Error::ConnectionFailed)) => Err(AccountError::Other("couldn't connect to the server".into())),
            Ok(Err(e)) => Err(AccountError::Other(e.to_string())),
        }
    }

    fn register(&self, username: &str, password: &str) -> Result<(), AccountError> {
        // With the player's identity, the account is linked at once and its name reserved
        // across servers sharing friends. The server sets the account id.
        let identity = self.identity.as_ref().map(|(id, server_id)| (id.as_ref(), server_id.as_str()));
        let r = rt().block_on(async { tokio::time::timeout(TIMEOUT, network::register(self.api.clone(), username, password, identity)).await });
        match r {
            Err(_) => Err(AccountError::Other("the server didn't answer in time".into())),
            Ok(Ok(())) => Ok(()),
            Ok(Err(network::Error::UsernameAlreadyTaken)) => Err(AccountError::Taken),
            Ok(Err(network::Error::ConnectionFailed)) => Err(AccountError::Other("couldn't connect to the server".into())),
            Ok(Err(e)) => Err(AccountError::Other(e.to_string())),
        }
    }

    fn key_login(&self, username: &str, new_password: &str) -> Result<(), AccountError> {
        let Some((identity, server_id)) = self.identity.as_ref() else {
            return Err(AccountError::NotFound);
        };
        let r = rt().block_on(async { tokio::time::timeout(TIMEOUT, network::key_login(self.api.clone(), identity, server_id, username, new_password)).await });
        match r {
            Ok(Ok(())) => Ok(()),
            Ok(Err(network::Error::UserNotFound)) => Err(AccountError::NotFound),
            Err(_) => Err(AccountError::Other("the server didn't answer in time".into())),
            Ok(Err(e)) => Err(AccountError::Other(e.to_string())),
        }
    }
}

/// The admin API of a server started with `--launcher` (see
/// `dedicated_server/src/api.rs`), authorised by its admin key.
#[derive(Clone)]
pub struct Admin {
    pub api: String,
    pub key: String,
}

type Authed = InterceptedService<Channel, Box<dyn FnMut(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> + Send + Sync>>;

impl Admin {
    async fn channel(&self) -> anyhow::Result<Authed> {
        let channel = Endpoint::from_shared(self.api.clone())?.connect_timeout(Duration::from_secs(4)).connect().await?;
        let key: tonic::metadata::MetadataValue<_> = self.key.trim().parse()?;
        let auth: Box<dyn FnMut(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> + Send + Sync> = Box::new(move |mut req| {
            req.metadata_mut().insert("authorization", key.clone());
            Ok(req)
        });
        Ok(InterceptedService::new(channel, auth))
    }

    pub fn users(&self) -> anyhow::Result<Vec<users::User>> {
        block_on(async {
            let mut c = users::users_admin_client::UsersAdminClient::new(self.channel().await?);
            Ok(c.list(users::ListRequest::default()).await?.into_inner().users)
        })
    }

    pub fn delete_user(&self, id: String) -> anyhow::Result<()> {
        block_on(async {
            let mut c = users::users_admin_client::UsersAdminClient::new(self.channel().await?);
            c.delete(users::DeleteRequest { id }).await?;
            Ok(())
        })
    }

    pub fn games(&self) -> anyhow::Result<Vec<games::Game>> {
        block_on(async {
            let mut c = games::games_admin_client::GamesAdminClient::new(self.channel().await?);
            Ok(c.list(games::ListRequest::default()).await?.into_inner().games)
        })
    }

    pub fn delete_game(&self, id: u32) -> anyhow::Result<()> {
        block_on(async {
            let mut c = games::games_admin_client::GamesAdminClient::new(self.channel().await?);
            c.delete(games::DeleteRequest { id }).await?;
            Ok(())
        })
    }
}
