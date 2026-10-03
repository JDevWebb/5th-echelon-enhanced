use std::sync::Arc;

use quazal::kerberos::KerberosTicket;
use quazal::kerberos::KerberosTicketInternal;
use quazal::kerberos::SESSION_KEY_SIZE;
use quazal::prudp::ClientRegistry;
use quazal::rmc::types::QResult;
use quazal::rmc::types::StationURL;
use quazal::rmc::Protocol;
use quazal::Context;
use rand::TryRngCore;

pub use crate::protocols::authentication_foundation::ticket_granting_protocol::*;
pub use crate::protocols::authentication_foundation::types::*;
use crate::protocols::ubi_authentication::types::UbiAuthenticationLoginCustomData;
use crate::storage::Storage;
use crate::SERVER_PID;

/// Implementation of the `TicketGrantingProtocolServerTrait` for handling ticket-granting requests.
struct TicketGrantingProtocolServerImpl {
    storage: Arc<Storage>,
}

impl TicketGrantingProtocolServerImpl {
    /// Generates a new session key for a user and stores it in the database.
    fn get_session_key(&self, logger: &slog::Logger, user_id: u32) -> [u8; SESSION_KEY_SIZE] {
        let mut key = [0u8; SESSION_KEY_SIZE];
        rand::rngs::OsRng.try_fill_bytes(&mut key).expect("Generating session key");

        if let Err(e) = self.storage.create_user_session(user_id, &key) {
            eprintln!("Error saving user session: {e}");
            error!(logger, "Error saving user session: {e}");
        }

        key
    }

    /// Retrieves the password for a user by their principal ID (PID).
    #[allow(unreachable_code)]
    fn get_password_by_pid(&self, logger: &slog::Logger, pid: u32) -> quazal::rmc::Result<Option<String>> {
        self.storage.find_password_for_user(pid).map_err(|e| {
            eprintln!("Error finding user password: {e}");
            error!(logger, "Error finding user password: {e}");
            quazal::rmc::Error::InternalError
        })
    }

    /// Retrieves the password for a user by their username.
    fn get_password_by_username(&self, logger: &slog::Logger, username: &str) -> quazal::rmc::Result<Option<String>> {
        Ok(self
            .get_pid_by_username(logger, username)?
            .map(|uid| self.get_password_by_pid(logger, uid))
            .transpose()?
            .flatten())
    }

    /// Retrieves the principal ID (PID) for a user by their username.
    #[allow(unreachable_code)]
    fn get_pid_by_username(&self, logger: &slog::Logger, username: &str) -> quazal::rmc::Result<Option<u32>> {
        self.storage.find_user_id_by_name(username).map_err(|e| {
            eprintln!("Error finding user password: {e}");
            error!(logger, "Error finding user password: {e}");
            quazal::rmc::Error::InternalError
        })
    }

    /// Whether the game may sign in to the account: player accounts only after a current
    /// client signed in to the API (see [`crate::clients`]). The server's own accounts
    /// (Tracking) have no client.
    fn has_current_client(&self, logger: &slog::Logger, user_id: u32) -> quazal::rmc::Result<bool> {
        if crate::clients::minimum().is_none() {
            return Ok(true);
        }
        let since = crate::clients::now() - crate::clients::VOUCHES_FOR;
        let check = async { Ok::<_, eyre::Report>(!self.storage.is_player_account(user_id).await? || self.storage.has_current_client(user_id, since).await?) };
        crate::storage::run(check).and_then(|r| r).map_err(|e| {
            error!(logger, "Error checking the client for user {user_id}: {e}");
            quazal::rmc::Error::InternalError
        })
    }

    /// Authenticates a user with their username and password.
    #[allow(unreachable_code)]
    fn login(&self, logger: &slog::Logger, username: &str, password: &str) -> quazal::rmc::Result<Option<u32>> {
        self.storage
            .login_user(username, password)
            .map_err(|e| {
                eprintln!("Error finding user password: {e}");
                error!(logger, "Error finding user password: {e}");
                quazal::rmc::Error::InternalError
            })
            .map(Result::ok)
    }
}

/// How long a ticket lets its holder connect to the secure service. The
/// server checks it only when the game connects (right after login), so it
/// never cuts off a game in progress; it just stops a copied ticket working
/// forever (upstream's never expired).
const TICKET_LIFETIME: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

fn ticket_expiry() -> u64 {
    (std::time::SystemTime::now() + TICKET_LIFETIME)
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX)
}

/// Creates the `RVConnectionData` for a client.
fn get_connection_data(ctx: &Context, pid: u32) -> RVConnectionData {
    ctx.secure_server_addr.map_or_else(
        || RVConnectionData {
            url_regular_protocols: format!("prudp:/address={};port={};CID=1;PID={};sid=2;stream=3;type=2", ctx.listen.ip(), ctx.listen.port(), pid)
                .parse()
                .unwrap(),
            lst_special_protocols: vec![],
            url_special_protocols: StationURL::default(),
        },
        |a| RVConnectionData {
            url_regular_protocols: format!(
                // CID => ConnectionID
                // PID => PrincipalID
                // RVCID => RVConnectionID
                // sid => StreamID
                // stream => StreamType
                "prudps:/address={};port={};CID=1;PID={};sid=1;stream=3;type=2",
                a.ip(),
                a.port(),
                pid
            )
            .parse()
            .unwrap(),
            lst_special_protocols: vec![],
            url_special_protocols: StationURL::default(),
        },
    )
}

impl<T> TicketGrantingProtocolServerTrait<T> for TicketGrantingProtocolServerImpl {
    fn login(
        &self,
        logger: &slog::Logger,
        ctx: &Context,
        ci: &mut quazal::ClientInfo<T>,
        request: LoginRequest,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<LoginResponse, quazal::rmc::Error> {
        // Nothing checks a password here, but each try looks a name up: the same limit on
        // tries per address as the other logins.
        if request.str_user_name.chars().count() > crate::rate_limit::MAX_NAME || !crate::rate_limit::plain_login(Some(ci.address().ip())) {
            warn!(logger, "plain login from {} refused (too many, or too long a name)", ci.address().ip());
            return Err(quazal::rmc::Error::AccessDenied);
        }
        let Some(user_id) = self.get_pid_by_username(logger, &request.str_user_name)? else {
            warn!(logger, "user {:?} not found", request.str_user_name);
            return Err(quazal::rmc::Error::AccessDenied);
        };
        // Plain Login proves nothing by itself: the ticket is sealed with the
        // user's stored plaintext password, so only its owner can use it.
        // Accounts registered through the launcher keep only a hash, and
        // sealing theirs with the well-known dummy password let anyone who
        // knew a username log in as them. They must use LoginEx.
        let Some(password) = self.get_password_by_username(logger, &request.str_user_name)? else {
            warn!(logger, "plain login refused for {:?}: no plaintext password (use LoginEx)", request.str_user_name);
            return Err(quazal::rmc::Error::AccessDenied);
        };
        if !self.has_current_client(logger, user_id)? {
            warn!(logger, "plain login refused for {:?}: no current client signed in", request.str_user_name);
            return Err(quazal::rmc::Error::AccessDenied);
        }
        let password = Some(password);
        ci.user_id = Some(user_id);
        let session_key = self.get_session_key(logger, user_id);
        let ticket = KerberosTicket {
            session_key,
            pid: SERVER_PID,
            internal: KerberosTicketInternal {
                principle_id: user_id,
                valid_until: ticket_expiry(),
                session_key,
                issued_to: [0; 16],
            }
            .for_address(ci.address().ip()),
        };
        Ok(LoginResponse {
            return_value: QResult::Ok,
            pid_principal: user_id,
            pbuf_response: ticket.as_bytes(user_id, password.as_deref(), &ctx.ticket_key),
            p_connection_data: get_connection_data(ctx, 2),
            str_return_msg: String::new(),
        })
    }

    fn login_ex(
        &self,
        logger: &slog::Logger,
        ctx: &Context,
        ci: &mut quazal::ClientInfo<T>,
        request: LoginExRequest,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<LoginExResponse, quazal::rmc::Error> {
        let username = request.str_user_name;
        let mut registry = quazal::rmc::types::ClassRegistry::default();
        registry.register_class::<UbiAuthenticationLoginCustomData>("UbiAuthenticationLoginCustomData");
        let ubi_data = request.o_extra_data.into_inner(&registry)?;
        let ubi_data: Option<&UbiAuthenticationLoginCustomData> = ubi_data.as_any().downcast_ref();
        let Some(UbiAuthenticationLoginCustomData {
            user_name: ubi_username,
            password,
            ..
        }) = ubi_data
        else {
            error!(logger, "Error parsing UbiAuthenticationLoginCustomData");
            return Err(quazal::rmc::Error::ParsingError);
        };

        // No account has a longer name: not looked up, counted or logged in full.
        if ubi_username.chars().count() > crate::rate_limit::MAX_NAME || username.chars().count() > crate::rate_limit::MAX_NAME {
            warn!(logger, "LoginEx from {} with too long a name; refused", ci.address().ip());
            return Err(quazal::rmc::Error::AccessDenied);
        }
        info!(logger, "LoginEx attempt by {:?} ({:?})", ubi_username, username);
        let peer = Some(ci.address().ip());
        if !crate::rate_limit::begin_login(peer, ubi_username) {
            warn!(logger, "too many logins from {} or failures for {ubi_username:?}; refused", ci.address().ip());
            return Err(quazal::rmc::Error::AccessDenied);
        }

        // An error (e.g. the server too busy checking passwords) isn't a failed login.
        let Some(user_id) = self.login(logger, ubi_username, password)? else {
            crate::rate_limit::login_failed(peer, ubi_username);
            crate::metrics::failed_login();
            warn!(logger, "login failed for {:?}", ubi_username);
            return Err(quazal::rmc::Error::AccessDenied);
        };
        crate::rate_limit::login_succeeded(peer, ubi_username);
        if !self.has_current_client(logger, user_id)? {
            warn!(logger, "login refused for {:?}: its game client is outdated (or didn't sign in to the API)", ubi_username);
            return Err(quazal::rmc::Error::AccessDenied);
        }
        info!(logger, "login successful for {:?}", ubi_username);

        ci.user_id = Some(user_id);
        let session_key = self.get_session_key(logger, user_id);
        let ticket = KerberosTicket {
            session_key,
            pid: SERVER_PID,
            internal: KerberosTicketInternal {
                principle_id: user_id,
                valid_until: ticket_expiry(),
                session_key,
                issued_to: [0; 16],
            }
            .for_address(ci.address().ip()),
        };
        Ok(LoginExResponse {
            return_value: QResult::Ok,
            pid_principal: user_id,
            pbuf_response: ticket.as_bytes(user_id, None, &ctx.ticket_key),
            p_connection_data: get_connection_data(ctx, SERVER_PID),
            str_return_msg: String::new(),
        })
    }

    fn request_ticket(
        &self,
        logger: &slog::Logger,
        ctx: &Context,
        ci: &mut quazal::ClientInfo<T>,
        request: RequestTicketRequest,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> Result<RequestTicketResponse, quazal::rmc::Error> {
        let user_id = request.id_source;
        let server_id = request.id_target;
        if !matches!(ci.user_id, Some(uid) if uid == user_id) {
            warn!(logger, "Ticket request for {} to {} denied (user: {:?})", user_id, server_id, ci.user_id);
            return Err(quazal::rmc::Error::AccessDenied);
        }
        let session_key = self.get_session_key(logger, user_id);
        let ticket = KerberosTicket {
            session_key,
            pid: server_id,
            internal: KerberosTicketInternal {
                principle_id: user_id,
                valid_until: ticket_expiry(),
                session_key,
                issued_to: [0; 16],
            }
            .for_address(ci.address().ip()),
        };
        Ok(RequestTicketResponse {
            return_value: QResult::Ok,
            buf_response: ticket.as_bytes(user_id, self.get_password_by_pid(logger, user_id)?.as_deref(), &ctx.ticket_key),
        })
    }
}

/// Creates a new boxed `TicketGrantingProtocolServer` instance.
///
/// This function is typically used to register the ticket-granting protocol
/// with the server's protocol dispatcher.
pub fn new_protocol<T: 'static>(storage: Arc<Storage>) -> Box<dyn Protocol<T>> {
    Box::new(TicketGrantingProtocolServer::new(TicketGrantingProtocolServerImpl { storage }))
}
