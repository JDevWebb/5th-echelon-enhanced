//! Implements client-side logic for interacting with the server's gRPC API
//! for user authentication.
//!
//! This module provides functions for testing user logins and registering new users.

use server_api::users::users_client::UsersClient;
use server_api::users::LoginRequest;
use server_api::users::RegisterRequest;

use super::Error;

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
    let Ok(mut client) = super::endpoint(&api_url)?.connect().await.map(UsersClient::new) else {
        return Err(Error::ConnectionFailed);
    };

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
            // Handle different gRPC status codes.
            if matches!(status.code(), tonic::Code::Unauthenticated) {
                return Err(Error::InvalidPassword);
            }
            if matches!(status.code(), tonic::Code::NotFound) {
                return Err(Error::UserNotFound);
            } else if matches!(status.code(), tonic::Code::FailedPrecondition) {
                // An outdated launcher: the server says what to do.
                return Err(Error::Rpc(status));
            } else {
                return Err(Error::SendingRequestFailed);
            }
        }
    };

    let resp = resp.into_inner();
    if resp.error.is_empty() {
        Ok(())
    } else {
        Err(Error::ServerFailure(resp.error))
    }
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
    let Ok(mut client) = super::endpoint(&api_url)?.connect().await.map(UsersClient::new) else {
        return Err(Error::ConnectionFailed);
    };
    let resp = client
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            client: super::CLIENT.into(),
        })
        .await?
        .into_inner();
    Ok(resp.user.map(|u| u.id).filter(|id| !id.is_empty()).unwrap_or_else(|| username.to_string()))
}

/// Renames the account (signed with `identity` for `host`, the server as the player reached it, when
/// it's linked). Answers the new name.
pub async fn rename(api_url: String, username: &str, password: &str, new_name: &str, identity: Option<(&identity::Identity, &str)>) -> Result<String, Error> {
    let host = identity.map(|(_, host)| identity::host_key(host)).unwrap_or_default();
    let channel = super::endpoint(&api_url)?.connect().await.map_err(|_| Error::ConnectionFailed)?;
    let token = UsersClient::new(channel.clone())
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            client: super::CLIENT.into(),
        })
        .await?
        .into_inner()
        .token;
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
    let Ok(mut client) = super::endpoint(&api_url)?.connect().await.map(UsersClient::new) else {
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
    let Ok(channel) = super::endpoint(&api_url)?.connect().await else {
        return Err(Error::ConnectionFailed);
    };
    let token = UsersClient::new(channel.clone())
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            client: super::CLIENT.into(),
        })
        .await?
        .into_inner()
        .token;
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
    let mut client = UsersClient::new(super::endpoint(&api_url)?.connect().await.map_err(|_| Error::ConnectionFailed)?);
    let resp = match client
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            client: super::CLIENT.into(),
        })
        .await
    {
        Ok(resp) => resp.into_inner(),
        Err(status) if status.code() == tonic::Code::Unauthenticated => return Err(Error::InvalidPassword),
        Err(status) if status.code() == tonic::Code::NotFound => return Err(Error::UserNotFound),
        Err(status) => return Err(status.into()),
    };
    if !resp.error.is_empty() {
        return Err(Error::ServerFailure(resp.error));
    }
    Ok(resp.token)
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

/// A session token for the account, and a channel to its server.
async fn signed_in(api_url: &str, username: &str, password: &str) -> Result<(tonic::transport::Channel, tonic::metadata::MetadataValue<tonic::metadata::Ascii>), Error> {
    let channel = super::endpoint(api_url)?.connect().await.map_err(|_| Error::ConnectionFailed)?;
    let token = UsersClient::new(channel.clone())
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            client: super::CLIENT.into(),
        })
        .await?
        .into_inner()
        .token;
    let token = token.parse().map_err(|_| Error::ServerFailure("bad token".into()))?;
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

/// Sends the player's report to the server; answers its id.
pub async fn send_report(api_url: String, username: &str, password: &str, report: server_api::misc::ReportRequest) -> Result<String, Error> {
    let (channel, token) = signed_in(&api_url, username, password).await?;
    let mut client = server_api::misc::misc_client::MiscClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
        req.metadata_mut().insert("authorization", token.clone());
        Ok(req)
    });
    Ok(client.report(report).await?.into_inner().id)
}
