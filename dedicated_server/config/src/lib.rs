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
    /// Accounts one address may create in an hour. Players sharing one
    /// address (a LAN party, a household) count together.
    #[serde(default = "default_registrations")]
    pub registrations_per_hour: usize,
}

const fn default_failed_logins() -> usize {
    30
}

const fn default_registrations() -> usize {
    20
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            failed_logins_per_10_minutes: default_failed_logins(),
            registrations_per_hour: default_registrations(),
        }
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
    pub fn set_addresses(&mut self, listen: std::net::IpAddr, public: std::net::IpAddr) {
        self.api_server.set_ip(listen);
        for svc in self.quazal.service.values_mut() {
            match svc {
                Service::Authentication(ctx) | Service::Secure(ctx) => {
                    ctx.listen.set_ip(listen);
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
                Service::Config(online) => online.set_ips(listen, public),
                Service::Content(content) => content.listen.set_ip(listen),
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
    }
}
