//! The launcher's settings: `uplay.toml` in the game folder, which the hook
//! also reads.
//!
//! A file this launcher didn't write (upstream 5th Echelon's, or this one's before
//! [`FORMAT`] 2) isn't trusted: it carried stale addresses, adapters pinned to a VPN,
//! accounts on other servers and switches nobody remembered setting, which made the
//! first connection fail in ways the checklist couldn't explain. It is kept as a
//! backup and the settings start over; see [`fresh_start`].

use std::fs;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use tracing::error;
use tracing::info;

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
    /// The server has answered over HTTPS, with its API there. From then on
    /// only HTTPS is used for it: a plain answer could come from anyone on
    /// the way, so the setup stops rather than send the password readable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub https: bool,
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

    /// Whether its API is plain HTTP: passwords and sign-ins travel readable.
    pub fn unencrypted(&self) -> bool {
        self.api_server_url().scheme() != "https"
    }

    /// Whether the profile has an account to sign in with.
    pub fn has_account(&self) -> bool {
        !self.user.username.is_empty() && (!self.user.password.is_empty() || !self.user.protected_password.is_empty())
    }
}

/// The settings format this launcher writes. A file without it, or with an older
/// one, starts over ([`fresh_start`]).
pub const FORMAT: u32 = 2;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    /// [`FORMAT`] when this launcher wrote the file; 0 when it didn't.
    #[serde(default)]
    pub format: u32,
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
            format: FORMAT,
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
            account_id: if profile.user.account_id.is_empty() {
                profile.user.username.clone()
            } else {
                profile.user.account_id.clone()
            },
            ..profile.user.clone()
        };
        hook.networking.adapter = profile.adapter.clone();
        hook.networking.nat_port = profile.nat_port;
        self.default_profile = profile.name.clone();
    }

    /// Reads `uplay.toml` from `game_dir`. A missing file gives the defaults
    /// (written on the first change); an unreadable one is kept as
    /// `uplay.toml.broken` before starting over, never silently lost. A file
    /// this launcher didn't write is backed up and replaced ([`fresh_start`]).
    pub fn load(game_dir: &Path) -> ConfigMut {
        let path = hooks_config::get_config_path(game_dir);
        let mut started_over = None;
        let inner = match fs::read_to_string(&path) {
            Ok(s) => match toml::from_str::<Config>(&s) {
                // A folder another tool manages keeps its file as that tool wrote it.
                Ok(cfg) if cfg.format >= FORMAT || crate::overrides::managed_by(game_dir).is_some() => cfg,
                Ok(old) => {
                    let fresh = fresh_start(old);
                    match backup(&path) {
                        Ok(kept) => {
                            info!("Settings from an earlier launcher kept as {}; starting over", kept.display());
                            if let Err(e) = crate::write_private(&path, toml::to_string_pretty(&fresh).unwrap_or_default().as_bytes()) {
                                error!("Couldn't write the new settings: {e}");
                            }
                            started_over = Some(kept);
                        }
                        Err(e) => error!("Couldn't back up {}, so it is left alone for now: {e}", path.display()),
                    }
                    fresh
                }
                Err(e) => {
                    // Not the error itself: it quotes the line, which may be a password.
                    let line = e.span().map_or(0, |span| s[..span.start.min(s.len())].lines().count());
                    error!("Can't parse {} (line {line}): {}", path.display(), e.message());
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
            started_over,
        }
    }
}

/// The settings that replace a file this launcher didn't write. Kept from it: the game
/// version (DirectX 9 or 11), a save folder the player chose, and accounts on servers
/// this launcher set up (they answer over HTTPS, which upstream's never recorded), without
/// a pinned adapter. Everything else goes back to the defaults.
pub fn fresh_start(old: Config) -> Config {
    let mut fresh = Config {
        default_game: old.default_game,
        ..Config::default()
    };
    fresh.hook_config.save = old.hook_config.save;
    fresh.profiles = old.profiles.into_iter().filter(|p| p.https).map(|p| Profile { adapter: None, ..p }).collect();
    let current = fresh.profile(&old.default_profile).or_else(|| fresh.profiles.first()).cloned();
    if let Some(profile) = current {
        fresh.apply_profile(&profile);
    }
    fresh
}

/// Copies `path` to a backup beside it that doesn't exist yet: `uplay.toml.old`, then
/// `uplay.toml.old-2` and on.
fn backup(path: &Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("uplay.toml");
    let kept = (1..100)
        .map(|i| path.with_file_name(if i == 1 { format!("{name}.old") } else { format!("{name}.old-{i}") }))
        .find(|p| !p.exists())
        .ok_or_else(|| std::io::Error::other("too many backups"))?;
    fs::copy(path, &kept)?;
    Ok(kept)
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
    /// Where the earlier launcher's settings were kept, when this read replaced them.
    started_over: Option<PathBuf>,
}

impl ConfigMut {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where the earlier launcher's settings were kept, when loading replaced them.
    pub fn started_over(&self) -> Option<&Path> {
        self.started_over.as_deref()
    }

    fn save(&mut self) -> anyhow::Result<()> {
        // Written by this launcher now, whoever wrote what it was read from.
        self.inner.format = FORMAT;
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
            Ok(Err(e)) => error!("Config changed on disk but can't be parsed, keeping ours: {}", e.message()),
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
        p.use_ports(&crate::server_info::Ports {
            api: 80,
            login: 31126,
            nat: Some(31128),
            api_tls: None,
        });
        assert_eq!(p.api_server_url().as_str(), "http://blacklist.example.com/");
        assert_eq!((p.api_port(), p.login_port(), p.nat_port), (80, 31126, Some(31128)));
        let mut cfg = Config::default();
        cfg.apply_profile(&p);
        assert_eq!(cfg.hook_config.networking.nat_port, Some(31128));

        p.use_ports(&crate::server_info::Ports {
            api: 50051,
            login: 21126,
            nat: Some(21128),
            api_tls: None,
        });
        assert_eq!((p.api_server_url.clone(), p.login_port, p.nat_port), (None, None, None));
        assert_eq!(p.api_server_url().as_str(), "http://blacklist.example.com:50051/");

        assert!(p.unencrypted());
        // HTTPS offered: used.
        p.use_ports(&crate::server_info::Ports {
            api: 80,
            login: 21126,
            nat: None,
            api_tls: Some(443),
        });
        assert_eq!(p.api_server_url().as_str(), "https://blacklist.example.com/");
        assert!(!p.unencrypted());
    }

    #[test]
    fn a_server_with_https_is_remembered() {
        let dir = temp_dir("config-https");
        let mut cfg = Config::load(&dir);
        cfg.update(|c| {
            c.upsert_profile(Profile {
                name: "Kiwi".into(),
                server: "bl.example.com".into(),
                https: true,
                ..Default::default()
            });
        })
        .unwrap();
        assert!(std::fs::read_to_string(dir.join("uplay.toml")).unwrap().contains("Https = true"));
        assert!(Config::load(&dir).current_profile().unwrap().https);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_earlier_launchers_settings_are_backed_up_and_start_over() {
        let dir = temp_dir("config-upstream");
        let upstream = r#"
Profiles = [
    { Name = "Home", Server = "192.168.1.10", Username = "Nexus", Password = "pw12345678", AccountId = "Nexus", Adapter = "Radmin VPN" },
    { Name = "Community NA", Server = "na1.example.net", Username = "Kiwi", Password = "pw87654321", AccountId = "Kiwi", Adapter = "Wi-Fi", Https = true },
]
DefaultProfile = "Home"
DefaultGame = "SplinterCellBlacklistDx9"
UiVersion = "New"
ApiServer = "http://192.168.1.10:50051"
ConfigServer = "192.168.1.10"
ForwardAllCalls = true
[User]
Username = "Nexus"
Password = "pw12345678"
[Networking]
IpAddress = "26.70.109.161"
Adapter = "Radmin VPN"
[Save.SaveDir]
Custom = "D:/Saves"
"#;
        std::fs::write(dir.join("uplay.toml"), upstream).unwrap();
        let cfg = Config::load(&dir);
        let kept = dir.join("uplay.toml.old");
        assert_eq!(cfg.started_over(), Some(kept.as_path()));
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), upstream, "the old file, untouched");

        // Only the account this launcher set up, unpinned; the game version and save folder.
        assert_eq!(cfg.profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Community NA"]);
        assert_eq!(cfg.profiles[0].adapter, None);
        assert_eq!(cfg.default_game, GameVersion::SplinterCellBlacklistDx9);
        let hook = &cfg.hook_config;
        assert_eq!(hook.user.username, "Kiwi");
        assert_eq!(hook.config_server.as_deref(), Some("na1.example.net"));
        assert_eq!((hook.networking.ip_address, hook.networking.adapter.as_deref()), (None, None));
        assert!(!hook.forward_all_calls);
        assert_eq!(hook.save.save_dir, hooks_config::SaveDir::Custom("D:/Saves".into()));
        assert_eq!(cfg.format, FORMAT);

        // Written at once, so the game reads the new settings; done once.
        let again = Config::load(&dir);
        assert_eq!(again.started_over(), None);
        assert_eq!(again.profiles, cfg.profiles);
        assert!(!dir.join("uplay.toml.old-2").exists());

        // Another old file later goes next to the first backup.
        std::fs::write(dir.join("uplay.toml"), upstream).unwrap();
        assert_eq!(Config::load(&dir).started_over(), Some(dir.join("uplay.toml.old-2").as_path()));
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
