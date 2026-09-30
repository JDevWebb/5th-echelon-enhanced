//! A test player: logs in like the game (auth server, ticket, secure server)
//! and like its DLL (gRPC API), and makes the session and invite calls the
//! game makes.

use std::net::IpAddr;
use std::net::SocketAddr;
use std::time::Duration;

use eyre::bail;
use eyre::eyre;
use eyre::Result;
use md5::Digest;
use md5::Md5;
use quazal::prudp::packet::crypt_key;
use quazal::rmc::basic::FromStream;
use quazal::rmc::basic::ReadStream;
use quazal::rmc::basic::ToStream;
use quazal::rmc::types::Any;
use quazal::rmc::types::Property;
use quazal::rmc::types::QList;
use quazal::rmc::types::StationURL;
use sc_bl_protocols::authentication_foundation::ticket_granting_protocol as tg;
use sc_bl_protocols::game_session_service::game_session_protocol as gs;
use sc_bl_protocols::game_session_service::types::GameSession;
use sc_bl_protocols::game_session_service::types::GameSessionKey;
use sc_bl_protocols::game_session_service::types::GameSessionSearchWithParticipantsResult;
use sc_bl_protocols::ubi_authentication::types::UbiAuthenticationLoginCustomData;
use server_api::friends::friends_client::FriendsClient;
use server_api::misc::misc_client::MiscClient;
use server_api::users::users_client::UsersClient;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

use crate::conn::Conn;

/// Default ports of a 5th Echelon server.
pub const AUTH_PORT: u16 = 21126;
pub const API_PORT: u16 = 50051;

/// Where the server is, and on which ports: the defaults, or what its
/// `/api/info` reports (a server behind a reverse proxy).
#[derive(Debug, Clone)]
pub struct Target {
    /// The name the API is reached by (a proxy routes by it).
    pub host: String,
    pub api: u16,
    pub auth: u16,
    pub nat: u16,
}

static TARGET: std::sync::OnceLock<Target> = std::sync::OnceLock::new();

pub fn set_target(target: Target) {
    let _ = TARGET.set(target);
}

pub fn target(server: IpAddr) -> Target {
    TARGET.get().cloned().unwrap_or_else(|| Target {
        host: server.to_string(),
        api: API_PORT,
        auth: AUTH_PORT,
        nat: nat_proto::DEFAULT_PORT,
    })
}

fn api_url(server: IpAddr) -> String {
    let t = target(server);
    format!("http://{}:{}", t.host, t.api)
}
/// The secure server's principal id (tickets are issued for it).
const SERVER_PID: u32 = 0x1000;
/// Notification protocol and its one method (server -> client).
const NOTIFICATION_PROTOCOL: u16 = 14;
const PROCESS_NOTIFICATION_EVENT: u32 = 1;

/// Session attribute sets the game uses (see game_session.rs tests):
/// a party lobby ("anteroom") and a private match room.
pub const LOBBY: &str = "113 => 1;103 => 0;3 => 8;4 => 0";
pub const PRIVATE_MATCH: &str = "113 => 0;3 => 0;4 => 8;102 => 7";
/// The session type the game's friend searches use.
pub const SESSION_TYPE: u32 = 1;

pub struct Bot {
    pub name: String,
    pub pid: u32,
    /// The NAT helper ticket from signing in.
    pub nat_ticket: nat_proto::Ticket,
    token: String,
    api: Channel,
    secure: Conn,
}

fn decode<T: FromStream>(data: &[u8]) -> Result<T> {
    ReadStream::from_bytes(data).read().map_err(|e| eyre!("decoding answer: {e:?}"))
}

/// The key a ticket is encrypted with for principal `pid` (Quazal's
/// Kerberos: MD5 of the password, repeated 65000 + pid % 1024 times).
fn ticket_key(pid: u32, password: &str) -> Vec<u8> {
    let mut key = password.as_bytes().to_vec();
    for _ in 0..(65000 + pid % 1024) {
        key = Md5::digest(&key).to_vec();
    }
    key
}

/// Signs in to `name` with its identity key (signed for `host`), optionally
/// setting a new password.
pub async fn key_login(server: IpAddr, identity: &identity::Identity, host: &str, name: &str, time: i64, new_password: &str) -> std::result::Result<(), tonic::Status> {
    let channel = Channel::from_shared(api_url(server))
        .map_err(|e| tonic::Status::internal(e.to_string()))?
        .connect()
        .await
        .map_err(|e| tonic::Status::unavailable(e.to_string()))?;
    UsersClient::new(channel)
        .key_login(server_api::users::KeyLoginRequest {
            username: name.into(),
            global_id: identity.global_id(),
            time,
            signature: identity.sign_login(host, name, time, new_password),
            new_password: new_password.into(),
            host: host.into(),
        })
        .await?;
    Ok(())
}

/// A registration with the NAT helper, as the hook makes it.
#[derive(Debug, Clone, Copy)]
pub struct NatRegistration {
    pub observed: std::net::SocketAddrV4,
    pub advertise: std::net::SocketAddrV4,
    pub relayed: bool,
    pub tag: nat_proto::Tag,
    pub cookie: nat_proto::Cookie,
}

/// Registers `name` (with its `ticket`) from `socket` at the NAT helper
/// `nat`: a probe for the cookie, then one with it.
pub async fn nat_register(socket: &tokio::net::UdpSocket, nat: std::net::SocketAddrV4, flags: u8, name: &str, ticket: nat_proto::Ticket) -> Result<NatRegistration> {
    use nat_proto::Message;
    let mut cookie = [0u8; 16];
    let mut buf = [0u8; 256];
    for _ in 0..10 {
        let nonce: u32 = rand::random::<u32>() | 1;
        let msg = Message::Probe {
            flags,
            nonce,
            mapping: None,
            name: name.into(),
            ticket,
            cookie,
        }
        .encode();
        socket.send_to(&msg, nat).await?;
        if let Ok(Ok((n, _))) = tokio::time::timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await {
            if let Some(Message::ProbeReply {
                nonce: got,
                observed,
                advertise,
                flags,
                cookie: c,
                tag,
                ..
            }) = Message::decode(&buf[..n])
            {
                if got != nonce {
                    continue;
                }
                cookie = c;
                if tag != [0; 8] {
                    return Ok(NatRegistration {
                        observed,
                        advertise,
                        relayed: flags & nat_proto::reply_flags::RELAYED != 0,
                        tag,
                        cookie,
                    });
                }
            }
        }
    }
    bail!("the NAT helper didn't register {name}")
}

/// Parses `a => b;c => d` into session properties.
pub fn properties(attrs: &str) -> QList<Property> {
    attrs.parse().expect("valid attributes")
}

/// The auth server's part of signing in: LoginEx and a ticket for the secure
/// server. Returns the player's pid, the secure server's address and the ticket.
async fn request_ticket(server: IpAddr, name: &str, password: &str) -> Result<(u32, SocketAddr, tg::RequestTicketResponse)> {
    // Auth server: LoginEx and a ticket for the secure server.
    let (mut auth, _) = Conn::connect(SocketAddr::new(server, target(server).auth), vec![]).await?;
    let login = tg::LoginExRequest {
        str_user_name: name.into(),
        o_extra_data: Any::new(
            "UbiAuthenticationLoginCustomData".into(),
            UbiAuthenticationLoginCustomData {
                data: quazal::rmc::types::Data,
                user_name: name.into(),
                online_key: "AAAA-BBBB-CCCC".into(),
                password: password.into(),
            }
            .to_bytes(),
        ),
    };
    let resp: tg::LoginExResponse =
        decode(&auth.call(tg::TICKET_GRANTING_PROTOCOL_ID, tg::TicketGrantingProtocolMethod::LoginEx as u32, login.to_bytes()).await?)?;
    let pid = resp.pid_principal;
    let url = &resp.p_connection_data.url_regular_protocols;
    let secure_addr = SocketAddr::new(if url.address == "0.0.0.0" { server } else { url.address.parse()? }, url.port);
    let ticket: tg::RequestTicketResponse = decode(
        &auth
            .call(
                tg::TICKET_GRANTING_PROTOCOL_ID,
                tg::TicketGrantingProtocolMethod::RequestTicket as u32,
                tg::RequestTicketRequest { id_source: pid, id_target: SERVER_PID }.to_bytes(),
            )
            .await?,
    )?;
    auth.disconnect().await?;
    Ok((pid, secure_addr, ticket))
}

impl Bot {
    /// Creates an account through the gRPC API (as the launcher's Register).
    pub async fn register(server: IpAddr, name: &str, password: &str) -> Result<()> {
        Ok(Self::register_as(server, name, password, None).await?)
    }

    /// Creates an account, linked to `identity` (signed for the server with
    /// id `server_id`) when given, as the launcher does.
    pub async fn register_as(server: IpAddr, name: &str, password: &str, identity: Option<(&identity::Identity, &str)>) -> std::result::Result<(), tonic::Status> {
        let channel = Channel::from_shared(api_url(server))
            .map_err(|e| tonic::Status::internal(e.to_string()))?
            .connect()
            .await
            .map_err(|e| tonic::Status::unavailable(e.to_string()))?;
        let time = identity::now();
        let (global_id, signature) = identity.map_or_else(Default::default, |(id, host)| (id.global_id(), id.sign_link(host, name, time)));
        let host = identity.map(|(_, host)| host.to_string()).unwrap_or_default();
        UsersClient::new(channel)
            .register(server_api::users::RegisterRequest {
                username: name.into(),
                password: password.into(),
                ubi_id: String::new(),
                global_id,
                time,
                signature,
                host,
            })
            .await?;
        Ok(())
    }

    /// Logs in like the game and its DLL: LoginEx on the auth server, a
    /// ticket for the secure server, the secure connection, and the gRPC API.
    pub async fn login(server: IpAddr, name: &str, password: &str) -> Result<Bot> {
        let (pid, secure_addr, ticket) = request_ticket(server, name, password).await?;
        Self::connect_secure(server, name, password, pid, secure_addr, ticket).await
    }

    /// Signs in to the auth server and takes a ticket for the secure server,
    /// then stops there, as the launcher's connection test does.
    pub async fn ticket_only(server: IpAddr, name: &str, password: &str) -> Result<()> {
        request_ticket(server, name, password).await.map(|_| ())
    }

    async fn connect_secure(server: IpAddr, name: &str, password: &str, pid: u32, secure_addr: SocketAddr, ticket: tg::RequestTicketResponse) -> Result<Bot> {

        // The ticket: RC4 under the account's key (the dummy password for
        // accounts with only a hash), then an HMAC we don't need to check.
        let buf = &ticket.buf_response;
        if buf.len() < 16 + 4 + 16 {
            bail!("ticket too short");
        }
        let plain = crypt_key(&ticket_key(pid, "UbiDummyPwd"), &buf[..buf.len() - 16]);
        let session_key = plain[..16].to_vec();
        let sealed: Vec<u8> = decode(&plain[20..])?;

        // Secure server: present the sealed ticket and an encrypted challenge.
        let challenge: u32 = rand::random();
        let mut connect_data = pid.to_bytes();
        connect_data.extend(0u32.to_bytes());
        connect_data.extend(challenge.to_bytes());
        let mut payload = sealed.to_bytes();
        payload.extend(crypt_key(&session_key, &connect_data).to_bytes());
        let (secure, answer) = Conn::connect(secure_addr, payload).await?;
        let answer: Vec<u8> = decode(&answer).map_err(|_| eyre!("the secure server rejected the ticket"))?;
        if decode::<u32>(&answer)? != challenge.wrapping_add(1) {
            bail!("the secure server answered the challenge wrongly");
        }

        // The DLL's gRPC session.
        let api = Channel::from_shared(api_url(server))?.connect().await?;
        let login = UsersClient::new(api.clone())
            .login(server_api::users::LoginRequest { username: name.into(), password: password.into() })
            .await?
            .into_inner();
        let nat_ticket = login.nat_ticket.as_slice().try_into().unwrap_or([0; 16]);
        Ok(Bot {
            name: name.into(),
            pid,
            nat_ticket,
            token: login.token,
            api,
            secure,
        })
    }

    fn authed<T>(&self, msg: T) -> tonic::Request<T> {
        let mut req = tonic::Request::new(msg);
        req.metadata_mut().insert("authorization", MetadataValue::try_from(self.token.as_str()).expect("token"));
        req
    }

    async fn gs<Resp: FromStream>(&mut self, method: gs::GameSessionProtocolMethod, req: impl ToStream) -> Result<Resp> {
        decode(&self.secure.call(gs::GAME_SESSION_PROTOCOL_ID, method as u32, req.to_bytes()).await?)
    }

    /// Registers where other players reach this one (the game's own station URLs).
    pub async fn register_urls(&mut self, urls: &[&str]) -> Result<()> {
        let station_urls = QList(urls.iter().map(|u| u.parse::<StationURL>().map_err(|e| eyre!("{u}: {e:?}"))).collect::<Result<_>>()?);
        self.gs::<gs::RegisterUrLsResponse>(gs::GameSessionProtocolMethod::RegisterUrLs, gs::RegisterUrLsRequest { station_urls }).await?;
        Ok(())
    }

    /// Creates a session with the given attributes and returns its id.
    pub async fn create_session(&mut self, attrs: &str) -> Result<u32> {
        let resp: gs::CreateSessionResponse = self
            .gs(
                gs::GameSessionProtocolMethod::CreateSession,
                gs::CreateSessionRequest { game_session: GameSession { type_id: SESSION_TYPE, attributes: properties(attrs) } },
            )
            .await?;
        Ok(resp.game_session_key.session_id)
    }

    /// Creates a session, sending the request twice (a retransmission).
    pub async fn create_session_twice(&mut self, attrs: &str) -> Result<u32> {
        let req = gs::CreateSessionRequest { game_session: GameSession { type_id: SESSION_TYPE, attributes: properties(attrs) } };
        let resp: gs::CreateSessionResponse = decode(
            &self
                .secure
                .call_twice(gs::GAME_SESSION_PROTOCOL_ID, gs::GameSessionProtocolMethod::CreateSession as u32, req.to_bytes())
                .await?,
        )?;
        Ok(resp.game_session_key.session_id)
    }

    /// Matchmaking search (GameSessionEx): other players' sessions whose
    /// attributes match `attrs`. Returns (session id, host pid).
    pub async fn search_sessions(&mut self, attrs: &str) -> Result<Vec<(u32, u32)>> {
        use sc_bl_protocols::game_session_ex_service::game_session_ex_protocol as gsx;
        use sc_bl_protocols::game_session_service::types::GameSessionQuery;
        let req = gsx::SearchSessionsRequest {
            game_session_query: GameSessionQuery { type_id: SESSION_TYPE, query_id: 0, parameters: properties(attrs) },
        };
        let resp: gsx::SearchSessionsResponse =
            decode(&self.secure.call(gsx::GAME_SESSION_EX_PROTOCOL_ID, gsx::GameSessionExProtocolMethod::SearchSessions as u32, req.to_bytes()).await?)?;
        Ok(resp
            .search_results
            .0
            .iter()
            .map(|r| (r.game_session_search_result.session_key.session_id, r.game_session_search_result.host_pid))
            .collect())
    }

    /// Simulated loss: the next push from the server is dropped unacknowledged.
    pub fn drop_next_push(&mut self) {
        self.secure.drop_requests += 1;
    }

    /// Adds participants to a session (the host adding a guest, or a guest itself).
    pub async fn add_participants(&mut self, session: u32, public: &[u32], private: &[u32]) -> Result<()> {
        self.gs::<gs::AddParticipantsResponse>(
            gs::GameSessionProtocolMethod::AddParticipants,
            gs::AddParticipantsRequest {
                game_session_key: key(session),
                public_participant_ids: QList(public.to_vec()),
                private_participant_ids: QList(private.to_vec()),
            },
        )
        .await?;
        Ok(())
    }

    /// The friend search an invited game makes: sessions the given players are in.
    pub async fn search_with_participants(&mut self, pids: &[u32]) -> Result<Vec<GameSessionSearchWithParticipantsResult>> {
        let resp: gs::SearchSessionsWithParticipantsResponse = self
            .gs(
                gs::GameSessionProtocolMethod::SearchSessionsWithParticipants,
                gs::SearchSessionsWithParticipantsRequest { game_session_type_id: SESSION_TYPE, participant_ids: QList(pids.to_vec()) },
            )
            .await?;
        Ok(resp.search_results.0)
    }

    pub async fn join_session(&mut self, session: u32) -> Result<()> {
        self.gs::<gs::JoinSessionResponse>(gs::GameSessionProtocolMethod::JoinSession, gs::JoinSessionRequest { game_session_key: key(session) })
            .await?;
        Ok(())
    }

    pub async fn leave_session(&mut self, session: u32) -> Result<()> {
        self.gs::<gs::LeaveSessionResponse>(gs::GameSessionProtocolMethod::LeaveSession, gs::LeaveSessionRequest { game_session_key: key(session) })
            .await?;
        Ok(())
    }

    pub async fn abandon_session(&mut self, session: u32) -> Result<()> {
        self.gs::<gs::AbandonSessionResponse>(
            gs::GameSessionProtocolMethod::AbandonSession,
            gs::AbandonSessionRequest { game_session_key: key(session) },
        )
        .await?;
        Ok(())
    }

    /// Splits out of a session (the invite route); returns the session to use.
    pub async fn split_session(&mut self, session: u32) -> Result<u32> {
        let resp: gs::SplitSessionResponse = self
            .gs(gs::GameSessionProtocolMethod::SplitSession, gs::SplitSessionRequest { game_session_key: key(session) })
            .await?;
        Ok(resp.game_session_key_migrated.session_id)
    }

    /// Invites a friend (by account name) as the game's DLL does.
    pub async fn invite(&self, friend: &str) -> Result<()> {
        FriendsClient::new(self.api.clone()).invite(self.authed(server_api::friends::InviteRequest { id: friend.into() })).await?;
        Ok(())
    }

    /// Polls for an invitation as the DLL does every second; returns the
    /// inviter's name.
    pub async fn poll_invite(&self, wait: Duration) -> Result<Option<String>> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let resp = MiscClient::new(self.api.clone()).event(self.authed(server_api::misc::EventRequest {})).await?.into_inner();
            if let Some(sender) = resp.invite.and_then(|i| i.sender) {
                return Ok(Some(sender.username));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Friends as the game's friend list shows them: (name, online).
    pub async fn friends(&self) -> Result<Vec<(String, bool)>> {
        let resp = FriendsClient::new(self.api.clone()).list(self.authed(server_api::friends::ListRequest {})).await?.into_inner();
        Ok(resp.friends.into_iter().map(|f| (f.username, f.is_online)).collect())
    }

    /// Tries to invite a player, returning the server's refusal if any.
    pub async fn try_invite(&self, friend: &str) -> std::result::Result<(), tonic::Status> {
        FriendsClient::new(self.api.clone()).invite(self.authed(server_api::friends::InviteRequest { id: friend.into() })).await?;
        Ok(())
    }

    /// A friend change towards the player called `name`, as the overlay makes it.
    pub async fn friend_change(&self, change: &str, name: &str) -> std::result::Result<server_api::friends::Relation, tonic::Status> {
        let mut c = FriendsClient::new(self.api.clone());
        let req = self.authed(server_api::friends::TargetRequest {
            id: String::new(),
            username: name.into(),
        });
        let resp = match change {
            "request" => c.request(req).await?,
            "accept" => c.accept(req).await?,
            "decline" => c.decline(req).await?,
            "remove" => c.remove(req).await?,
            "block" => c.block(req).await?,
            "unblock" => c.unblock(req).await?,
            other => return Err(tonic::Status::invalid_argument(format!("no change {other}"))),
        };
        Ok(resp.into_inner().relation())
    }

    /// Renames this account (signed with `identity` for `server_id` if linked).
    pub async fn rename(&self, new_name: &str, identity: Option<(&identity::Identity, &str)>) -> std::result::Result<String, tonic::Status> {
        let time = identity::now();
        let signature = identity.map(|(id, host)| id.sign_link(host, new_name, time)).unwrap_or_default();
        let host = identity.map(|(_, host)| host.to_string()).unwrap_or_default();
        let resp = FriendsClient::new(self.api.clone())
            .rename(self.authed(server_api::friends::RenameRequest {
                new_name: new_name.into(),
                time,
                signature,
                host,
            }))
            .await?;
        Ok(resp.into_inner().username)
    }

    /// Friends, requests both ways and blocks.
    pub async fn relationships(&self) -> Result<server_api::friends::RelationshipsResponse> {
        Ok(FriendsClient::new(self.api.clone())
            .relationships(self.authed(server_api::friends::RelationshipsRequest {}))
            .await?
            .into_inner())
    }

    /// Player search: names and whether each is online.
    pub async fn search_online(&self, query: &str) -> Result<Vec<(String, bool)>> {
        let found = FriendsClient::new(self.api.clone())
            .search(self.authed(server_api::friends::SearchRequest { query: query.into() }))
            .await?
            .into_inner()
            .players;
        Ok(found.into_iter().map(|p| (p.username, p.is_online)).collect())
    }

    /// Player search: names and how they stand to us.
    pub async fn search(&self, query: &str) -> Result<Vec<(String, server_api::friends::Relation)>> {
        let found = FriendsClient::new(self.api.clone())
            .search(self.authed(server_api::friends::SearchRequest { query: query.into() }))
            .await?
            .into_inner()
            .players;
        Ok(found.into_iter().map(|p| (p.username.clone(), p.relation())).collect())
    }

    /// Links this account to `identity`, signed for `host` (the server as reached).
    pub async fn link(&self, identity: &identity::Identity, host: &str, time: i64) -> std::result::Result<(), tonic::Status> {
        FriendsClient::new(self.api.clone())
            .link_identity(self.authed(server_api::friends::LinkIdentityRequest {
                global_id: identity.global_id(),
                time,
                signature: identity.sign_link(host, &self.name, time),
                host: host.into(),
            }))
            .await?;
        Ok(())
    }

    /// The launcher's direct-connection test: the server sends `challenge` to
    /// this machine's UDP 13000, from wherever it sees this player, and
    /// returns what came back.
    pub async fn test_direct(&self, challenge: Vec<u8>) -> std::result::Result<Vec<u8>, tonic::Status> {
        Ok(MiscClient::new(self.api.clone())
            .test_p2p(self.authed(server_api::misc::TestP2pRequest { challenge }))
            .await?
            .into_inner()
            .challenge)
    }

    /// Unlinks this account from its identity.
    pub async fn unlink(&self) -> std::result::Result<(), tonic::Status> {
        FriendsClient::new(self.api.clone()).unlink_identity(self.authed(server_api::friends::UnlinkIdentityRequest {})).await?;
        Ok(())
    }

    /// Waits for a friend event (a request or an accepted one): its kind
    /// and who from.
    pub async fn poll_friend_event(&self, wait: Duration) -> Result<Option<(server_api::misc::friend_event::Kind, String)>> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let resp = MiscClient::new(self.api.clone()).event(self.authed(server_api::misc::EventRequest {})).await?.into_inner();
            if let Some(event) = resp.friend {
                let from = event.from.as_ref().map(|u| u.username.clone()).unwrap_or_default();
                return Ok(Some((event.kind(), from)));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Waits for a notification the server pushes to this player.
    pub async fn wait_notification(&mut self, wait: Duration) -> Result<Option<NotificationEvent>> {
        match self.secure.wait_request(NOTIFICATION_PROTOCOL, PROCESS_NOTIFICATION_EVENT, wait).await? {
            Some(req) => Ok(Some(NotificationEvent::decode(&req.parameters)?)),
            None => Ok(None),
        }
    }

    /// Leaves the secure server (the game closing).
    pub async fn disconnect(self) -> Result<()> {
        self.secure.disconnect().await
    }
}

/// A NotificationEvent the server pushes (protocol 14). Decoded here because
/// sc_bl_protocols' protocol_foundation feature doesn't build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationEvent {
    pub pid_source: u32,
    pub ui_type: u32,
    pub ui_param_1: u32,
    pub ui_param_2: u32,
    pub str_param: String,
    pub ui_param_3: u32,
}

impl NotificationEvent {
    fn decode(data: &[u8]) -> Result<Self> {
        let mut s = ReadStream::from_bytes(data);
        let mut next = || -> Result<u32> { s.read::<u32>().map_err(|e| eyre!("{e:?}")) };
        let (pid_source, ui_type, ui_param_1, ui_param_2) = (next()?, next()?, next()?, next()?);
        let str_param: String = s.read().map_err(|e| eyre!("{e:?}"))?;
        let ui_param_3: u32 = s.read().map_err(|e| eyre!("{e:?}"))?;
        Ok(Self { pid_source, ui_type, ui_param_1, ui_param_2, str_param, ui_param_3 })
    }
}

fn key(session_id: u32) -> GameSessionKey {
    GameSessionKey { type_id: SESSION_TYPE, session_id }
}
