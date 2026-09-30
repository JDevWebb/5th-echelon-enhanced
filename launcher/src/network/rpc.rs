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
    let Ok(mut client) = UsersClient::connect(api_url).await else {
        return Err(Error::ConnectionFailed);
    };

    let resp = match client
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
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
    let Ok(mut client) = UsersClient::connect(api_url).await else {
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
        })
        .await
    {
        Ok(resp) => resp,
        Err(status) => {
            // Handle different gRPC status codes.
            // The server's reason (a name it won't take, say) is worth showing.
            if matches!(status.code(), tonic::Code::AlreadyExists) {
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
    let Ok(mut client) = UsersClient::connect(api_url).await else {
        return Err(Error::ConnectionFailed);
    };
    let resp = client
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        })
        .await?
        .into_inner();
    Ok(resp.user.map(|u| u.id).filter(|id| !id.is_empty()).unwrap_or_else(|| username.to_string()))
}

/// Renames the account (signed with `identity` for `host`, the server as the player reached it, when
/// it's linked). Answers the new name.
pub async fn rename(api_url: String, username: &str, password: &str, new_name: &str, identity: Option<(&identity::Identity, &str)>) -> Result<String, Error> {
    let host = identity.map(|(_, host)| identity::host_key(host)).unwrap_or_default();
    let channel = tonic::transport::Endpoint::from_shared(api_url).map_err(|_| Error::ConnectionFailed)?.connect().await.map_err(|_| Error::ConnectionFailed)?;
    let token = UsersClient::new(channel.clone())
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
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

/// Signs in to `username` with the player's identity key (signed for `host`,
/// the server as the player reached it), setting `new_password`.
pub async fn key_login(api_url: String, identity: &identity::Identity, host: &str, username: &str, new_password: &str) -> Result<(), Error> {
    let Ok(mut client) = UsersClient::connect(api_url).await else {
        return Err(Error::ConnectionFailed);
    };
    let time = identity::now();
    let request = server_api::users::KeyLoginRequest {
        username: username.to_string(),
        global_id: identity.global_id(),
        time,
        signature: identity.sign_login(host, username, time, new_password),
        new_password: new_password.to_string(),
        host: identity::host_key(host),
    };
    match client.key_login(request).await {
        Ok(_) => Ok(()),
        Err(status) if matches!(status.code(), tonic::Code::Unauthenticated | tonic::Code::NotFound) => Err(Error::UserNotFound),
        Err(status) => Err(Error::Rpc(status)),
    }
}

/// Signs in as `username` and links the account to the player's identity,
/// so friends follow them to other servers sharing a coordinator.
pub async fn link_identity(api_url: String, identity: &identity::Identity, host: &str, username: &str, password: &str) -> Result<(), Error> {
    let Ok(channel) = tonic::transport::Endpoint::from_shared(api_url).map_err(|_| Error::ConnectionFailed)?.connect().await else {
        return Err(Error::ConnectionFailed);
    };
    let token = UsersClient::new(channel.clone())
        .login(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
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
