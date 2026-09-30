//! The launcher's settings: `uplay.toml` in the game folder, which the hook
//! also reads. Same format as upstream's launcher, so existing profiles
//! carry over.

use std::fs;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use tracing::error;

use crate::game::GameVersion;

/// Which of upstream's two launcher UIs to show. Kept so the file still
/// reads in upstream's launcher; this launcher has one UI.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
pub enum UIVersion {
    Old,
    New,
}

/// A server the player uses, with their account on it.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Profile {
    pub name: String,
    /// The server's address (IP or host name).
    pub server: String,
    pub api_server_url: Option<url::Url>,
    #[serde(flatten)]
    pub user: hooks_config::User,
    /// The network adapter to play on, by friendly name. None: any.
    pub adapter: Option<String>,
    /// The server's game login port, when it isn't the usual 21126 (a
    /// server behind a proxy reports its ports; see `server_info`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_port: Option<u16>,
    /// The server's NAT helper port, when it isn't the usual 21128.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nat_port: Option<u16>,
}

/// The NAT helper's usual port (`nat_proto::DEFAULT_PORT`).
const fn nat_proto_default() -> u16 {
    21128
}

impl Profile {
    /// The gRPC API: the given URL, or the server on its default port.
    pub fn api_server_url(&self) -> url::Url {
        self.api_server_url.clone().unwrap_or_else(|| {
            let host = if self.server.is_empty() { "localhost" } else { &self.server };
            format!("http://{host}:{}", crate::API_PORT).parse().expect("valid URL")
        })
    }

    /// The game login port on the server.
    pub fn login_port(&self) -> u16 {
        self.login_port.unwrap_or(crate::QUAZAL_PORT)
    }

    /// The API port on the server.
    pub fn api_port(&self) -> u16 {
        self.api_server_url().port_or_known_default().unwrap_or(crate::API_PORT)
    }

    /// Takes the ports the server reports: its API URL, login and NAT
    /// helper ports. Defaults are left unset, so the file stays as before.
    pub fn use_ports(&mut self, ports: &crate::server_info::Ports) {
        let host = if self.server.is_empty() { "localhost" } else { self.server.trim() };
        self.api_server_url = match ports.api_tls {
            // HTTPS when the server offers it: passwords and tokens never travel readable.
            Some(443) => format!("https://{host}").parse().ok(),
            Some(port) => format!("https://{host}:{port}").parse().ok(),
            None => (ports.api != crate::API_PORT).then(|| format!("http://{host}:{}", ports.api).parse().ok()).flatten(),
        };
        self.login_port = (ports.login != crate::QUAZAL_PORT).then_some(ports.login);
        self.nat_port = ports.nat.filter(|p| *p != nat_proto_default());
    }

    /// Whether the profile has an account to sign in with.
    pub fn has_account(&self) -> bool {
        !self.user.username.is_empty() && (!self.user.password.is_empty() || !self.user.protected_password.is_empty())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub default_profile: String,
    #[serde(flatten)]
    pub hook_config: hooks_config::Config,
    #[serde(default = "default_game")]
    pub default_game: GameVersion,
    #[serde(default)]
    pub ui_version: Option<UIVersion>,
}

fn default_game() -> GameVersion {
    GameVersion::SplinterCellBlacklistDx11
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            default_profile: String::new(),
            hook_config: hooks_config::default(),
            default_game: default_game(),
            ui_version: None,
        }
    }
}

impl Config {
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// The profile the player last used.
    pub fn current_profile(&self) -> Option<&Profile> {
        self.profile(&self.default_profile).or_else(|| self.profiles.first())
    }

    /// Adds `profile`, or replaces the one with its name, and makes it the default.
    pub fn upsert_profile(&mut self, profile: Profile) {
        self.default_profile = profile.name.clone();
        match self.profiles.iter_mut().find(|p| p.name == profile.name) {
            Some(p) => *p = profile,
            None => self.profiles.push(profile),
        }
    }

    /// Points the hook's settings at `profile`: what the game uses when it
    /// starts.
    pub fn apply_profile(&mut self, profile: &Profile) {
        let hook = &mut self.hook_config;
        hook.config_server = Some(profile.server.clone());
        hook.api_server = profile.api_server_url();
        let cd_keys = std::mem::take(&mut hook.user.cd_keys);
        hook.user = hooks_config::User {
            cd_keys: if profile.user.cd_keys.is_empty() { cd_keys } else { profile.user.cd_keys.clone() },
            account_id: if profile.user.account_id.is_empty() { profile.user.username.clone() } else { profile.user.account_id.clone() },
            ..profile.user.clone()
        };
        hook.networking.adapter = profile.adapter.clone();
        hook.networking.nat_port = profile.nat_port;
        self.default_profile = profile.name.clone();
    }

    /// Reads `uplay.toml` from `game_dir`. A missing file gives the defaults
    /// (written on the first change); an unreadable one is kept as
    /// `uplay.toml.broken` before starting over, never silently lost.
    pub fn load(game_dir: &Path) -> ConfigMut {
        let path = hooks_config::get_config_path(game_dir);
        let inner = match fs::read_to_string(&path) {
            Ok(s) => match toml::from_str::<Config>(&s) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Can't parse {}: {e}", path.display());
                    let backup = path.with_extension("toml.broken");
                    if let Err(e) = fs::copy(&path, &backup) {
                        error!("Couldn't keep the unreadable config: {e}");
                    }
                    Config::default()
                }
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    error!("Can't read {}: {e}", path.display());
                }
                Config::default()
            }
        };
        ConfigMut {
            inner,
            loaded: modified(&path),
            path,
        }
    }
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The settings plus their file: every change goes through [`ConfigMut::update`].
#[derive(Debug)]
pub struct ConfigMut {
    inner: Config,
    path: PathBuf,
    /// The file's modification time when last read or written.
    loaded: Option<std::time::SystemTime>,
}

impl ConfigMut {
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn save(&mut self) -> anyhow::Result<()> {
        // It holds the account passwords: readable by this user only on Linux.
        crate::write_private(&self.path, toml::to_string_pretty(&self.inner)?.as_bytes())?;
        self.loaded = modified(&self.path);
        Ok(())
    }

    /// Picks up changes made to the file since it was read (by another tool,
    /// or by hand), so saving doesn't undo them.
    pub fn reload_if_changed(&mut self) {
        let now = modified(&self.path);
        if now.is_none() || now == self.loaded {
            return;
        }
        match fs::read_to_string(&self.path).map(|s| toml::from_str::<Config>(&s)) {
            Ok(Ok(cfg)) => {
                self.inner = cfg;
                self.loaded = now;
            }
            Ok(Err(e)) => error!("Config changed on disk but can't be parsed, keeping ours: {e}"),
            Err(e) => error!("Config changed on disk but can't be read: {e}"),
        }
    }

    /// Changes the settings with `f` and saves them if anything changed.
    pub fn update<T>(&mut self, f: impl FnOnce(&mut Config) -> T) -> anyhow::Result<T> {
        self.reload_if_changed();
        let before = self.inner.clone();
        let result = f(&mut self.inner);
        if before != self.inner || self.loaded.is_none() {
            self.save()?;
        }
        Ok(result)
    }
}

impl Deref for ConfigMut {
    type Target = Config;

    fn deref(&self) -> &Config {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn ports_a_server_reports_are_used_and_defaults_stay_unset() {
        let mut p = Profile {
            server: "blacklist.example.com".into(),
            ..Default::default()
        };
        p.use_ports(&crate::server_info::Ports { api: 80, login: 31126, nat: Some(31128), api_tls: None });
        assert_eq!(p.api_server_url().as_str(), "http://blacklist.example.com/");
        assert_eq!((p.api_port(), p.login_port(), p.nat_port), (80, 31126, Some(31128)));
        let mut cfg = Config::default();
        cfg.apply_profile(&p);
        assert_eq!(cfg.hook_config.networking.nat_port, Some(31128));

        p.use_ports(&crate::server_info::Ports { api: 50051, login: 21126, nat: Some(21128), api_tls: None });
        assert_eq!((p.api_server_url.clone(), p.login_port, p.nat_port), (None, None, None));
        assert_eq!(p.api_server_url().as_str(), "http://blacklist.example.com:50051/");

        // HTTPS offered: used.
        p.use_ports(&crate::server_info::Ports { api: 80, login: 21126, nat: None, api_tls: Some(443) });
        assert_eq!(p.api_server_url().as_str(), "https://blacklist.example.com/");
    }

    #[test]
    fn upstream_launcher_files_still_load() {
        let dir = temp_dir("config-upstream");
        std::fs::write(
            dir.join("uplay.toml"),
            r#"
Profiles = [{ Name = "Home", Server = "192.168.1.10", Username = "Nexus", Password = "pw12345678", AccountId = "Nexus", Adapter = "Ethernet" }]
DefaultProfile = "Home"
DefaultGame = "SplinterCellBlacklistDx9"
UiVersion = "New"
ApiServer = "http://192.168.1.10:50051"
ConfigServer = "192.168.1.10"
[User]
Username = "Nexus"
Password = "pw12345678"
"#,
        )
        .unwrap();
        let cfg = Config::load(&dir);
        assert_eq!(cfg.current_profile().unwrap().user.username, "Nexus");
        assert_eq!(cfg.default_game, GameVersion::SplinterCellBlacklistDx9);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_new_install_has_no_sample_accounts_and_saves_on_first_change() {
        let dir = temp_dir("config-new");
        let mut cfg = Config::load(&dir);
        assert!(cfg.profiles.is_empty());
        assert_eq!(cfg.default_game, GameVersion::SplinterCellBlacklistDx11);
        cfg.update(|c| {
            c.upsert_profile(Profile {
                name: "LAN".into(),
                server: "10.8.0.10".into(),
                user: hooks_config::User {
                    username: "Kiwi".into(),
                    password: "pw12345678".into(),
                    protected_password: String::new(),
                    cd_keys: vec![],
                    account_id: String::new(),
                },
                adapter: Some("Game VPN".into()),
                ..Default::default()
            });
            let p = c.current_profile().unwrap().clone();
            c.apply_profile(&p);
        })
        .unwrap();
        let again = Config::load(&dir);
        let hook = &again.hook_config;
        assert_eq!(hook.config_server.as_deref(), Some("10.8.0.10"));
        assert_eq!(hook.api_server.as_str(), "http://10.8.0.10:50051/");
        assert_eq!(hook.user.account_id, "Kiwi", "account id defaults to the username");
        assert_eq!(hook.networking.adapter.as_deref(), Some("Game VPN"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_files_are_kept() {
        let dir = temp_dir("config-broken");
        std::fs::write(dir.join("uplay.toml"), "this is = = not toml").unwrap();
        let cfg = Config::load(&dir);
        assert!(cfg.profiles.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("uplay.toml.broken")).unwrap(), "this is = = not toml");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
