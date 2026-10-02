use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::net::SocketAddr;

use quazal::ContentServer;
use quazal::Context;
use quazal::OnlineConfig;
use quazal::Service;
use serde::Deserialize;
use serde::Serialize;
use slog::error;
use slog::Logger;

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
pub struct DebugConfig {
    #[serde(default)]
    pub mark_all_as_online: bool,
    #[serde(default)]
    pub force_joins: bool,
    /// Pushes a notification to get a guest into a private match.
    ///
    /// This is the mechanism the game expects, not a workaround, hence on by default. A guest
    /// joining a private match deliberately sends no `JoinSession`: `StateJoin::vf08` reads
    /// `[object+0x5A8]`, sees the room is private, skips the join and waits on
    /// `[session+0x42A]`. Only `NetOnlineSessionServiceRdv::vf28` (0x007BDFB0) sets that byte,
    /// and it is reached solely through a notification from the server.
    ///
    /// Without one the guest sits at "creating game session" and opens sessions of its own in
    /// a loop. See `add_participants` for the three fields that have to be right.
    #[serde(default = "enabled")]
    pub push_notifications: bool,
    /// Sends that notification reliably (with `Reliable`/`NeedAck`).
    ///
    /// On by default: the client acknowledges the packet. The concern that an unacknowledged
    /// packet could stall the ordered stream does not materialise as long as the push only
    /// goes to private rooms.
    #[serde(default = "enabled")]
    pub push_notifications_reliable: bool,
    /// gRPC reflection: lists every API call to anyone who asks. For
    /// debugging with tools like grpcurl.
    #[serde(default)]
    pub grpc_reflection: bool,
    /// Only a session's host and participants may change it or remove
    /// others (anyone may add or remove themselves). Switch off only to
    /// rule it out when a join fails.
    #[serde(default = "enabled")]
    pub session_owner_checks: bool,
}

const fn enabled() -> bool {
    true
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            mark_all_as_online: false,
            force_joins: false,
            push_notifications: true,
            push_notifications_reliable: true,
            grpc_reflection: false,
            session_owner_checks: true,
        }
    }
}

/// The community API: a small JSON API on the config server's port (80) for
/// launchers, overlays and tools (see `community_api.rs`).
///
/// Only `info` is on by default. The rest shares player data or creates
/// accounts, so an operator switches each part on for their own community.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
pub struct CommunityApiConfig {
    /// `GET /api/info`: name, version and features of this server.
    #[serde(default = "enabled")]
    pub info: bool,
    /// `GET /api/presence`: every registered player, who's online, and what
    /// they're playing.
    #[serde(default)]
    pub presence: bool,
    /// `POST /api/register` and `POST /api/login`: one-click accounts for
    /// launchers. Both are rate-limited per address.
    #[serde(default)]
    pub accounts: bool,
    /// `GET /api/unhandled`: the game's RMC calls this server couldn't answer.
    #[serde(default)]
    pub unhandled: bool,
}

impl Default for CommunityApiConfig {
    fn default() -> Self {
        Self {
            info: true,
            presence: false,
            accounts: false,
            unhandled: false,
        }
    }
}

/// Per-address limits on everything that checks a password or creates an
/// account. Requests from this machine (loopback) are never limited.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
pub struct LimitsConfig {
    /// Failed logins one address may make in ten minutes, over every login
    /// route. Successful logins don't count.
    #[serde(default = "default_failed_logins")]
    pub failed_logins_per_10_minutes: usize,
    /// Logins one address may make in ten minutes, failed or not (each
    /// checks a password, which takes the server real work).
    #[serde(default = "default_logins")]
    pub logins_per_10_minutes: usize,
    /// Accounts one address may create in an hour. Players sharing one
    /// address (a LAN party, a household) count together.
    #[serde(default = "default_registrations")]
    pub registrations_per_hour: usize,
    /// Whether anyone may make an account (the launcher, `/api/register`).
    /// Off: only accounts that exist can sign in.
    #[serde(default = "enabled")]
    pub open_registration: bool,
    /// Whether every account must be linked to a player identity (the
    /// launcher's key, see `identity`): new accounts need one, and accounts
    /// can't be unlinked. Off: password-only accounts may be made (tools,
    /// `/api/register`), as before identities.
    #[serde(default)]
    pub require_identity: bool,
}

const fn default_failed_logins() -> usize {
    30
}

const fn default_logins() -> usize {
    120
}

const fn default_registrations() -> usize {
    20
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            failed_logins_per_10_minutes: default_failed_logins(),
            logins_per_10_minutes: default_logins(),
            registrations_per_hour: default_registrations(),
            open_registration: true,
            require_identity: false,
        }
    }
}

/// Who is on a player's friend list.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FriendsMode {
    /// Every player on the server, as before friend lists existed: right for
    /// a LAN or a group that all know each other. Friend requests still
    /// work (friends sort first), and blocks still hide people.
    #[default]
    Everyone,
    /// Only friends (a request, then accepted), and only friends can invite.
    /// For public servers.
    Mutual,
}

/// Friend lists (`[friends]`).
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
pub struct FriendsConfig {
    #[serde(default)]
    pub mode: FriendsMode,
}

/// A coordination server shared with other community servers
/// (`[federation]`): friends follow players between them, and this server
/// is listed in the coordinator's server directory. Off until
/// `coordinator` is set.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct FederationConfig {
    /// The coordinator's URL, e.g. "https://coordinator.example.com".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub coordinator: String,
    /// The coordinator's join token, from its operator. Only needed until
    /// this server has joined (its credentials are then kept in
    /// `federation.key`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub join_token: String,
    /// The name players see in the server directory.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Where the server is, e.g. "Sydney" or "EU West", for the directory.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub region: String,
    /// Whether to appear in the server directory (friends sync either way).
    #[serde(default = "enabled")]
    pub listed: bool,
    /// Allow a plain `http://` coordinator on another machine. Only for
    /// tests on a private network: the server's secret travels readable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_http: bool,
    /// Install the releases the coordinator rolls out (signed with the
    /// release key, checked by the updater the installer sets up). Off: the
    /// coordinator delists this server once it falls behind.
    #[serde(default = "enabled")]
    pub auto_update: bool,
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self {
            coordinator: String::new(),
            join_token: String::new(),
            name: String::new(),
            region: String::new(),
            listed: true,
            allow_http: false,
            auto_update: true,
        }
    }
}

impl FederationConfig {
    pub fn enabled(&self) -> bool {
        !self.coordinator.trim().is_empty()
    }
}

/// The admin API (accounts and games, on the gRPC port), for managing the
/// server from the launcher. Always on when started by the launcher
/// (`--launcher`); `enabled` turns it on otherwise. Its key is written to
/// `admin-key.txt` next to the database.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
pub struct AdminConfig {
    #[serde(default)]
    pub enabled: bool,
}

/// How the NAT helper relays match traffic.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RelayMode {
    /// Never; players whose NAT can't be punched through can't join others.
    Off,
    /// Only players who need it: a NAT that changes ports per destination
    /// (symmetric, most carrier-grade NAT), a player on this server's own
    /// network without a router port mapping, or one who asked for it.
    #[default]
    Auto,
    /// Every player. The most reliable, but all match traffic then goes
    /// through this server (roughly 20-60 KB/s per player).
    All,
}

/// The NAT helper: lets players join each other over the internet without a
/// VPN. The game's hook asks it (from the game's peer-to-peer socket) for
/// its public address, and it relays match traffic for players whose NAT
/// can't be punched through. Players need UDP `listen`'s port and the one
/// after it (21128-21129) to reach this server.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
pub struct NatConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_nat_listen")]
    pub listen: SocketAddr,
    #[serde(default)]
    pub relay: RelayMode,
    /// Relay addresses are this server's address with a port from this range.
    /// They're virtual (the hook wraps packets for them), so nothing listens
    /// on them and no firewall rule is needed.
    #[serde(default = "default_relay_ports")]
    pub relay_ports: (u16, u16),
    /// The most one player may send through the relay, in KB/s.
    #[serde(default = "default_relay_kbps")]
    pub relay_kbps_per_player: u32,
    /// The address players reach this server on; set with --public-address.
    /// Without it, the secure service's address is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_address: Option<std::net::IpAddr>,
}

const fn default_true() -> bool {
    true
}

fn default_nat_listen() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 21128))
}

const fn default_relay_ports() -> (u16, u16) {
    (40000, 40999)
}

const fn default_relay_kbps() -> u32 {
    2048
}

impl Default for NatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: default_nat_listen(),
            relay: RelayMode::default(),
            relay_ports: default_relay_ports(),
            relay_kbps_per_player: default_relay_kbps(),
            public_address: None,
        }
    }
}

/// What players connect to, when it isn't what the services listen on: the
/// server behind a reverse proxy, or with ports forwarded to other numbers.
/// Each port defaults to the one its service listens on. Every address the
/// server hands out uses these, and `/api/info` reports them so the launcher
/// sets players up with them.
#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
pub struct PublicConfig {
    /// The host name players use. Content downloads are addressed to it, so
    /// a proxy can route them by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The API (gRPC) port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<u16>,
    /// The API's HTTPS port, when a proxy serves it with TLS (the installer's
    /// Caddy: 443). Launchers then use it, so passwords and tokens are never
    /// sent readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_tls: Option<u16>,
    /// Game login (UDP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<u16>,
    /// Game service (UDP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure: Option<u16>,
    /// Content (HTTP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<u16>,
    /// The NAT helper (UDP); the port after it must lead to the helper's
    /// second port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nat: Option<u16>,
    /// Reverse proxies whose `X-Forwarded-For` names the real client (for
    /// the rate limits), as addresses or subnets, e.g. "172.17.0.0/16".
    /// A proxy on this machine (loopback) is always trusted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxies: Vec<String>,
    /// Other names or addresses players reach this server by (a LAN
    /// address, a second domain). Players' identity signatures name the
    /// host they typed; the server accepts its `host`, its public address
    /// and these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

impl PublicConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The ports players use, as `/api/info` reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PublicPorts {
    pub api: u16,
    pub login: u16,
    pub secure: u16,
    pub content: u16,
    /// None when the NAT helper is off.
    pub nat: Option<u16>,
    /// The API over HTTPS, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_tls: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(flatten)]
    pub quazal: quazal::Config,
    pub api_server: SocketAddr,
    pub debug: DebugConfig,
    #[serde(default)]
    pub community_api: CommunityApiConfig,
    #[serde(default)]
    pub admin: AdminConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
    #[serde(default)]
    pub nat: NatConfig,
    #[serde(default, skip_serializing_if = "PublicConfig::is_default")]
    pub public: PublicConfig,
    #[serde(default)]
    pub friends: FriendsConfig,
    #[serde(default, skip_serializing_if = "FederationConfig::is_off")]
    pub federation: FederationConfig,
}

impl FederationConfig {
    fn is_off(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    IO(#[from] std::io::Error),
    #[error("Parsing error: {0}")]
    Deserialize(#[from] toml::de::Error),
    #[error("Serializing error: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl Config {
    /// Listens on `listen` (0.0.0.0 for every address) and tells players to
    /// connect to `public`: the address they reach this server on. They
    /// differ behind NAT or in a container. Every service's address and the
    /// online config the game downloads are updated.
    ///
    /// A service listening on loopback stays there: it's meant to be
    /// reached through a reverse proxy on this machine.
    pub fn set_addresses(&mut self, listen: std::net::IpAddr, public: std::net::IpAddr) {
        let set = |addr: &mut SocketAddr| {
            if !addr.ip().is_loopback() {
                addr.set_ip(listen);
            }
        };
        set(&mut self.api_server);
        self.nat.listen.set_ip(listen);
        self.nat.public_address = Some(public);
        for svc in self.quazal.service.values_mut() {
            match svc {
                Service::Authentication(ctx) | Service::Secure(ctx) => {
                    set(&mut ctx.listen);
                    if let Some(addr) = ctx.secure_server_addr.as_mut() {
                        addr.set_ip(public);
                    }
                    if let Some(host) = ctx.settings.get_mut("storage_host") {
                        if let Ok(mut addr) = host.parse::<SocketAddr>() {
                            addr.set_ip(public);
                            *host = addr.to_string();
                        }
                    }
                }
                Service::Config(online) => {
                    let keep = online.listen;
                    online.set_ips(listen, public);
                    if keep.ip().is_loopback() {
                        online.listen = keep;
                    }
                }
                Service::Content(content) => set(&mut content.listen),
            }
        }
    }

    fn listen_ports(&self) -> (Option<u16>, Option<u16>, Option<u16>) {
        let (mut login, mut secure, mut content) = (None, None, None);
        for svc in self.quazal.service.values() {
            match svc {
                Service::Authentication(ctx) => login = Some(ctx.listen.port()),
                Service::Secure(ctx) => secure = Some(ctx.listen.port()),
                Service::Content(c) => content = Some(c.listen.port()),
                Service::Config(_) => {}
            }
        }
        (login, secure, content)
    }

    /// The ports players use: `[public]`, or else the ones the services
    /// listen on.
    pub fn public_ports(&self) -> PublicPorts {
        let (login, secure, content) = self.listen_ports();
        PublicPorts {
            api: self.public.api.unwrap_or(self.api_server.port()),
            login: self.public.login.or(login).unwrap_or(21126),
            secure: self.public.secure.or(secure).unwrap_or(21127),
            content: self.public.content.or(content).unwrap_or(8000),
            nat: self.nat.enabled.then(|| self.public.nat.unwrap_or(self.nat.listen.port())),
            api_tls: self.public.api_tls,
        }
    }

    /// With `[public]` set, hands out the ports players use instead of the
    /// ones in the service settings: the login port in the online config,
    /// the game service's port in tickets, and the content address (by
    /// `host`, when set). Each is `[public]`'s, or else the port its
    /// service listens on, so moving a listening port is enough. Only in
    /// memory; without `[public]`, nothing changes.
    pub fn apply_public(&mut self) {
        if self.public.is_default() {
            return;
        }
        let ports = self.public_ports();
        let host = self.public.host.clone();
        for svc in self.quazal.service.values_mut() {
            match svc {
                Service::Authentication(ctx) => {
                    if let Some(addr) = ctx.secure_server_addr.as_mut() {
                        addr.set_port(ports.secure);
                    }
                }
                Service::Secure(ctx) => {
                    if let Some(storage) = ctx.settings.get_mut("storage_host") {
                        let old_host = storage.rsplit_once(':').map_or(storage.as_str(), |(h, _)| h).to_string();
                        *storage = format!("{}:{}", host.clone().unwrap_or(old_host), ports.content);
                    }
                }
                Service::Config(online) => online.set_login_port(ports.login),
                Service::Content(_) => {}
            }
        }
    }

    pub fn load_from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self, Error> {
        let data = std::fs::read_to_string(path)?;
        let w: Config = toml::from_str(&data)?;
        Ok(w)
    }

    pub fn save_to_file<P: AsRef<std::path::Path>>(&self, path: P) -> Result<(), Error> {
        let data = toml::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    pub fn load_from_file_or_default<P: AsRef<std::path::Path>>(logger: &Logger, path: P) -> eyre::Result<Self> {
        let e = match Self::load_from_file(path.as_ref()) {
            Ok(cfg) => return Ok(cfg),
            Err(e) => e,
        };

        if !matches!(e, Error::IO(_)) {
            return Err(e.into());
        }

        error!(logger, "Couldn't load service file, generating default"; "error" => %e);

        let cfg = Self::default();
        if let Err(e) = cfg.save_to_file(path) {
            error!(logger, "Couldn't save service file"; "error" => %e);
        }
        Ok(cfg)
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut online_cfg = OnlineConfig::default();
        online_cfg.listen.set_ip(std::net::IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));

        let server_ip = std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

        let mut content_srv = ContentServer::default();
        content_srv.listen.set_ip(online_cfg.listen.ip());

        let mut ctx = Context::splinter_cell_blacklist();
        ctx.listen.set_ip(online_cfg.listen.ip());
        ctx.listen.set_port(21126);
        let mut secure_ctx = ctx.clone();

        let mut content_server_addr = content_srv.listen;
        content_server_addr.set_ip(server_ip);
        secure_ctx.settings.insert(String::from("storage_host"), content_server_addr.to_string());
        if let Some(path) = content_srv.files.keys().next() {
            secure_ctx.settings.insert(String::from("storage_path"), path.clone());
        }
        secure_ctx.listen.set_port(ctx.listen.port() + 1);

        let mut secure_server_addr = secure_ctx.listen;
        secure_server_addr.set_ip(server_ip);
        ctx.secure_server_addr = Some(secure_server_addr);

        let quazal_config = quazal::Config {
            services: ["onlineconfig", "content", "sc_bl_secure", "sc_bl_auth"].into_iter().map(String::from).collect(),
            service: HashMap::from([
                ("sc_bl_auth".to_string(), Service::Authentication(ctx)),
                ("sc_bl_secure".to_string(), Service::Secure(secure_ctx)),
                ("onlineconfig".to_string(), Service::Config(online_cfg)),
                ("content".to_string(), Service::Content(content_srv)),
            ]),
        };

        Config {
            api_server: "0.0.0.0:50051".parse().unwrap(),
            quazal: quazal_config,
            debug: DebugConfig::default(),
            community_api: CommunityApiConfig::default(),
            admin: AdminConfig::default(),
            limits: LimitsConfig::default(),
            nat: NatConfig::default(),
            public: PublicConfig::default(),
            friends: FriendsConfig::default(),
            federation: FederationConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_addresses_points_every_service_at_the_public_address() {
        let mut cfg = Config::default();
        let (listen, public) = ("0.0.0.0".parse().unwrap(), "203.0.113.10".parse().unwrap());
        cfg.set_addresses(listen, public);
        let text = toml::to_string(&cfg).unwrap();
        assert!(!text.contains("127.0.0.1"), "nothing still points at localhost:\n{text}");
        assert!(text.contains("203.0.113.10:21127"), "secure server");
        assert!(text.contains("203.0.113.10:8000"), "content (storage_host)");
        assert!(text.contains("prudp:/address=203.0.113.10;port=21126"), "online config");
        assert_eq!(cfg.api_server.ip(), listen);
        assert_eq!(cfg.nat.public_address, Some(public));
    }

    #[test]
    fn public_ports_replace_the_listening_ones_in_what_players_get() {
        let mut cfg = Config::default();
        cfg.set_addresses("0.0.0.0".parse().unwrap(), "203.0.113.10".parse().unwrap());
        assert_eq!(
            cfg.public_ports(),
            PublicPorts {
                api: 50051,
                login: 21126,
                secure: 21127,
                content: 8000,
                nat: Some(21128),
                api_tls: None,
            }
        );
        cfg.public = PublicConfig {
            host: Some("blacklist.example.com".into()),
            api: Some(80),
            api_tls: None,
            login: Some(31126),
            secure: Some(31127),
            content: Some(80),
            nat: Some(31128),
            proxies: vec![],
            aliases: vec![],
        };
        cfg.apply_public();
        let text = toml::to_string(&cfg).unwrap();
        assert!(text.contains("prudp:/address=203.0.113.10;port=31126"), "login in the online config:\n{text}");
        assert!(text.contains("203.0.113.10:31126"), "SandboxUrlWS");
        assert!(text.contains("203.0.113.10:31127"), "the game service in tickets");
        assert!(text.contains("storage_host = \"blacklist.example.com:80\""), "content by host name");
        assert!(text.contains("0.0.0.0:21126") && text.contains("0.0.0.0:21127"), "still listening on the old ports");
        assert_eq!(cfg.public_ports().nat, Some(31128));

        assert_eq!(cfg.public_ports().api, 80);
        cfg.nat.enabled = false;
        assert_eq!(cfg.public_ports().nat, None);
        // Only moved listening ports, and a host: those ports are handed out.
        let mut cfg = Config::default();
        cfg.set_addresses("0.0.0.0".parse().unwrap(), "203.0.113.10".parse().unwrap());
        for svc in cfg.quazal.service.values_mut() {
            match svc {
                Service::Authentication(ctx) => ctx.listen.set_port(41126),
                Service::Secure(ctx) => ctx.listen.set_port(41127),
                _ => {}
            }
        }
        cfg.public.host = Some("blacklist.example.com".into());
        cfg.apply_public();
        let text = toml::to_string(&cfg).unwrap();
        assert!(text.contains("prudp:/address=203.0.113.10;port=41126"), "login:\n{text}");
        assert!(text.contains("203.0.113.10:41127"), "the game service");
        assert!(text.contains("storage_host = \"blacklist.example.com:8000\""), "content");

        // Without [public], hand-made settings are left alone.
        let mut cfg = Config::default();
        let before = toml::to_string(&cfg).unwrap();
        cfg.apply_public();
        assert_eq!(toml::to_string(&cfg).unwrap(), before);
    }

    #[test]
    fn services_behind_a_local_proxy_stay_on_loopback() {
        let mut cfg = Config::default();
        cfg.api_server = "127.0.0.1:50051".parse().unwrap();
        for svc in cfg.quazal.service.values_mut() {
            match svc {
                Service::Config(online) => online.listen = "127.0.0.1:8080".parse().unwrap(),
                Service::Content(content) => content.listen = "127.0.0.1:8000".parse().unwrap(),
                _ => {}
            }
        }
        cfg.set_addresses("0.0.0.0".parse().unwrap(), "203.0.113.10".parse().unwrap());
        let text = toml::to_string(&cfg).unwrap();
        for local in ["127.0.0.1:50051", "127.0.0.1:8080", "127.0.0.1:8000"] {
            assert!(text.contains(local), "{local} moved:\n{text}");
        }
        assert!(text.contains("0.0.0.0:21126") && text.contains("203.0.113.10:21127"), "UDP still public");
        assert!(text.contains("prudp:/address=203.0.113.10;port=21126"));
    }

    #[test]
    fn old_service_files_get_the_nat_helper() {
        // The file as written before the NAT helper existed: no [nat] table.
        let text = toml::to_string(&Config::default()).unwrap();
        let mut in_nat = false;
        let old: String = text
            .lines()
            .filter(|line| {
                if line.starts_with('[') {
                    in_nat = *line == "[nat]";
                }
                !in_nat
            })
            .map(|line| format!("{line}\n"))
            .collect();
        assert!(!old.contains("relay_ports"));
        let cfg: Config = toml::from_str(&old).unwrap();
        assert_eq!(cfg.nat, NatConfig::default());
        assert!(cfg.nat.enabled);
        assert_eq!(cfg.nat.listen.port(), 21128);
    }
}
