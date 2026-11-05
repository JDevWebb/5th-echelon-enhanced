//! What the launcher keeps between runs and works on: its own settings (`launcher.toml`), the
//! game folder with its settings, and what the Play button starts. Shared by both interfaces
//! (the egui one, and launcher-tauri's).

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde::Serialize;
use setup::config::Config;
use setup::config::ConfigMut;
use setup::overrides::Managed;

/// The game folder the launcher works on, with its settings.
pub struct Game {
    pub dir: PathBuf,
    pub cfg: ConfigMut,
    /// Set when another tool manages this install (`uplay.override.toml`).
    pub managed: Option<Managed>,
    /// Set when the game's own Uplay DLL is an emulator (a crack): not a setup we support.
    pub uplay_emulator: bool,
    /// The player closed the notice about the emulator: not shown again.
    pub uplay_emulator_told: bool,
    /// The Uplay DLLs' sizes and times when [`Self::uplay_emulator`] was last worked out.
    uplay_dlls: Vec<Option<(u64, std::time::SystemTime)>>,
}

impl Game {
    pub fn open(dir: PathBuf) -> Self {
        let cfg = Config::load(&dir);
        let managed = setup::overrides::managed_by(&dir);
        let uplay_dlls = uplay_dlls(&dir);
        let uplay_emulator = setup::install::uplay_emulator(&dir);
        let uplay_emulator_told = Prefs::uplay_emulator_told();
        Self {
            dir,
            cfg,
            managed,
            uplay_emulator,
            uplay_emulator_told,
            uplay_dlls,
        }
    }

    /// Picks up changes to the settings made on another thread or by hand.
    pub fn reload(&mut self) {
        self.cfg.reload_if_changed();
        self.managed = setup::overrides::managed_by(&self.dir);
        // Read again only when a DLL changed (an install, a crack put in or taken out).
        let dlls = uplay_dlls(&self.dir);
        if dlls != self.uplay_dlls {
            self.uplay_emulator = setup::install::uplay_emulator(&self.dir);
            self.uplay_dlls = dlls;
        }
    }
}

impl Game {
    /// Opens a game folder to work on, tidying what earlier releases left there, and saying
    /// so when its settings came from an earlier launcher and started over.
    pub fn open_tidied(dir: PathBuf, notices: &mut crate::activity::Activities) -> Self {
        let game = Self::open(dir);
        // Data version logging's files, from before it was off by default (0.4.2).
        if !game.cfg.hook_config.log_data_version {
            hooks_config::remove_data_version_files(&game.dir);
        }
        // The addresses file earlier launchers left next to themselves (on the desktop, in
        // Downloads): deleted when it's only a copy of the built-in addresses.
        if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            hooks_addresses::tidy_stray_addresses(&exe_dir, &game.dir);
        }
        if let Some(kept) = game.cfg.started_over() {
            notices.info(format!(
                "Settings from an earlier 5th Echelon were set aside (kept as {} in the game folder); this launcher starts from its own.",
                kept.file_name().map_or_else(|| kept.display().to_string(), |n| n.to_string_lossy().into_owned())
            ));
        }
        game
    }

    /// The game folder chosen before, or else the first one found on this PC.
    pub fn chosen_or_found() -> Option<PathBuf> {
        Prefs::load()
            .game_dir
            .filter(|d| setup::game::is_game_dir(d))
            .or_else(|| setup::game::find_game_dirs().into_iter().next())
    }

    /// Changes the settings, reporting a failure to save.
    pub fn update(&mut self, notices: &mut crate::activity::Activities, f: impl FnOnce(&mut Config)) {
        if let Err(e) = self.cfg.update(f) {
            notices.error(format!("Couldn't save the settings: {e}"));
        }
    }
}

/// The player's size for the launcher: from this...
pub const MIN_SIZE: f32 = 0.7;
/// ...to this, in steps of [`STEP`].
pub const MAX_SIZE: f32 = 1.6;
pub const STEP: f32 = 0.1;

/// A size the player chose, kept within range and to a step.
pub fn clamp(size: f32) -> f32 {
    if !size.is_finite() {
        return 1.0;
    }
    ((size / STEP).round() * STEP).clamp(MIN_SIZE, MAX_SIZE)
}

/// The size and time of the game folder's Uplay DLLs, to tell when one changes.
fn uplay_dlls(dir: &Path) -> Vec<Option<(u64, std::time::SystemTime)>> {
    [setup::install::DLL_NAME, setup::install::ORIG_DLL_NAME]
        .iter()
        .map(|name| std::fs::metadata(dir.join(name)).ok().and_then(|m| Some((m.len(), m.modified().ok()?))))
        .collect()
}

/// The directory, as last read from disk.
type DirectoryPrefs = Option<String>;

/// The community network: the directory a new launcher browses, so setting up is
/// choosing from its servers. Another one replaces it (Settings, a network's address
/// typed in setup, or the first server joined that reports its own).
pub const COMMUNITY_DIRECTORY: &str = setup::directory::COMMUNITY;
static DIRECTORY_CACHE: std::sync::Mutex<Option<(Instant, DirectoryPrefs)>> = std::sync::Mutex::new(None);

/// The launcher's own settings (`%APPDATA%\5th-Echelon\launcher.toml`):
/// where the game is, for a launcher that isn't in the game folder, and the
/// server directory to browse.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Prefs {
    pub(crate) game_dir: Option<PathBuf>,
    /// A coordinator's URL, for its server directory. Learnt from the first
    /// server that reports one, or set in Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// The player set the directory (to none, too): no default then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    directory_set: bool,
    /// The player's size for the launcher (1.0: as designed), on top of fitting the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ui_scale: Option<f32>,
    /// When the player was last asked how a game went (feedback.rs).
    #[serde(default)]
    feedback: setup::feedback::Asked,
    /// The player said not to ask.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    feedback_off: bool,
    /// The player answered whether the game may send its diagnostics (diagnostics.rs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    diagnostics_asked: bool,
    /// The player knows the diagnostics include the 15 minutes of the game's log before a
    /// problem (from 0.4.3): told once, or asked with that wording.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    log_excerpt_told: bool,
    /// The new game options the player chose about (new_options.rs), by their ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    options_asked: Vec<String>,
    /// Servers of their own the player connected to (Servers › A server of your own), newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recent_servers: Vec<RecentServer>,
    /// The release whose What's new was last shown, or noted on a fresh install (whats_new.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    whats_new_seen: Option<String>,
    /// When the newest reply to the player's reports that they were told of was written
    /// (feedback.rs): newer ones get a notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replies_seen: Option<i64>,
    /// The newest support answer the player was told of (support.rs), by its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    support_seen: Option<i64>,
    /// The player closed the notice that a Uplay emulator isn't a supported setup.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    uplay_emulator_told: bool,
    /// Play asks Steam to start a Steam copy on Windows (for Steam Input), not the launcher.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    start_through_steam: bool,
    /// What the Play button starts: its split button's last choice.
    #[serde(default, skip_serializing_if = "PlayMode::is_default")]
    play_mode: PlayMode,
    /// The last start set `QuickStartLobby` in `uplay.toml`: a plain Play clears it again. A
    /// lobby set there by hand is never touched by a plain Play.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    quick_start_set: bool,
    /// The `QuickStartLobby` set by hand that the launcher's choice replaced: back with the
    /// next plain Play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quick_start_by_hand: Option<hooks_config::QuickStartLobby>,
    /// Linux: the classic (egui) interface, not the Tauri one (front_door.rs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    classic_interface: bool,
    /// The relay the player chose (its id; relays.rs), where their server offers it; None: the
    /// nearest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay: Option<String>,
}

/// What the Play button starts: the game as it is, or straight into an online lobby
/// (`QuickStartLobby` in `uplay.toml`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayMode {
    Svm,
    Coop,
    /// Also what a choice this launcher doesn't know reads as, so it never costs the other prefs.
    #[default]
    #[serde(other)]
    Play,
}

impl PlayMode {
    pub const ALL: [PlayMode; 3] = [PlayMode::Play, PlayMode::Svm, PlayMode::Coop];

    pub fn is_default(&self) -> bool {
        *self == Self::Play
    }

    /// The text on the button and in its menu.
    pub fn label(self) -> &'static str {
        match self {
            Self::Play => "Play",
            Self::Svm => "Play SvM",
            Self::Coop => "Play Co-op",
        }
    }

    /// The `QuickStartLobby` this start writes to `uplay.toml` (none: a plain start).
    pub fn lobby(self) -> Option<hooks_config::QuickStartLobby> {
        match self {
            Self::Play => None,
            Self::Svm => Some(hooks_config::QuickStartLobby::Svm),
            Self::Coop => Some(hooks_config::QuickStartLobby::Coop),
        }
    }

    /// Sets this start in the settings the game reads when it starts. A plain Play clears only a
    /// lobby the launcher set itself (`set_before`); one set by hand stays, and one a launcher
    /// choice replaced (`by_hand`, kept meanwhile) comes back (R6). Says whether the launcher
    /// has a lobby set now.
    pub fn apply(self, cfg: &mut setup::config::Config, set_before: bool, by_hand: &mut Option<hooks_config::QuickStartLobby>) -> bool {
        match self.lobby() {
            Some(lobby) => {
                if !set_before {
                    *by_hand = cfg.hook_config.quick_start_lobby;
                }
                cfg.hook_config.quick_start_lobby = Some(lobby);
                true
            }
            None => {
                if set_before {
                    cfg.hook_config.quick_start_lobby = by_hand.take();
                }
                false
            }
        }
    }
}

/// A server the player connected to by its address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentServer {
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Unix seconds.
    pub last_used: i64,
}

/// How many recent servers are kept.
const RECENT_SERVERS: usize = 5;

/// `list` with `address` used now, newest first, at most [`RECENT_SERVERS`].
fn note_recent(mut list: Vec<RecentServer>, address: &str, name: Option<String>, now: i64) -> Vec<RecentServer> {
    let address = address.trim();
    let before = list.iter().position(|r| r.address.eq_ignore_ascii_case(address)).map(|i| list.remove(i));
    list.insert(
        0,
        RecentServer {
            address: address.to_string(),
            name: name.or_else(|| before.and_then(|b| b.name)),
            last_used: now,
        },
    );
    list.truncate(RECENT_SERVERS);
    list
}

impl Prefs {
    fn path() -> Option<PathBuf> {
        setup::app_data_dir().map(|d| d.join("launcher.toml"))
    }

    pub fn load() -> Self {
        let mut prefs: Self = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        // The community network's old name (until 0.4.5), saved by a player who typed it or
        // set it in Settings: its new one, once and for good.
        if prefs
            .directory
            .as_deref()
            .is_some_and(|d| Self::is_community(d) && d.trim().trim_end_matches('/') != COMMUNITY_DIRECTORY)
        {
            prefs.directory = Some(COMMUNITY_DIRECTORY.to_string());
            prefs.save();
        }
        prefs
    }

    /// `url` as kept: the community network by its current name, anything else as given.
    fn directory_as_kept(url: &str) -> String {
        if Self::is_community(url) {
            COMMUNITY_DIRECTORY.to_string()
        } else {
            url.trim().to_string()
        }
    }

    fn save(&self) {
        let Some(path) = Self::path() else { return };
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
        if let Ok(s) = toml::to_string(self) {
            let _ = std::fs::write(path, s);
        }
    }

    pub fn remember(dir: &Path) {
        let mut prefs = Self::load();
        prefs.game_dir = Some(dir.to_path_buf());
        prefs.save();
    }

    /// The server directory, re-read from disk at most every few seconds
    /// (screens ask every frame).
    pub fn directory() -> Option<String> {
        Self::directory_prefs()
    }

    fn directory_prefs() -> DirectoryPrefs {
        let mut cache = DIRECTORY_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match cache.as_ref() {
            Some((at, prefs)) if at.elapsed() < Duration::from_secs(3) => prefs.clone(),
            _ => {
                let loaded = Self::load();
                let prefs = loaded.directory.or_else(|| (!loaded.directory_set).then(|| COMMUNITY_DIRECTORY.to_string()));
                *cache = Some((Instant::now(), prefs.clone()));
                prefs
            }
        }
    }

    /// The player's size for the launcher (Settings › Display).
    pub fn ui_scale() -> f32 {
        clamp(Self::load().ui_scale.unwrap_or(1.0))
    }

    pub fn set_ui_scale(size: f32) {
        let mut prefs = Self::load();
        let size = clamp(size);
        prefs.ui_scale = ((size - 1.0).abs() > f32::EPSILON).then_some(size);
        prefs.save();
    }

    /// When the player was last asked how a game went, and whether they said not to ask.
    pub fn feedback() -> (setup::feedback::Asked, bool) {
        let prefs = Self::load();
        (prefs.feedback, prefs.feedback_off)
    }

    pub fn set_feedback_asked(asked: setup::feedback::Asked) {
        let mut prefs = Self::load();
        prefs.feedback = asked;
        prefs.save();
    }

    /// Whether the player answered the question about the game's diagnostics.
    pub fn diagnostics_asked() -> bool {
        Self::load().diagnostics_asked
    }

    pub fn set_diagnostics_asked() {
        let mut prefs = Self::load();
        prefs.diagnostics_asked = true;
        prefs.log_excerpt_told = true;
        prefs.save();
    }

    /// Whether a player who agreed before 0.4.3 still has to be told that the diagnostics
    /// now include the 15 minutes of the game's log before a problem; noted as told.
    pub fn tell_log_excerpt() -> bool {
        let mut prefs = Self::load();
        if !prefs.diagnostics_asked || prefs.log_excerpt_told {
            return false;
        }
        prefs.log_excerpt_told = true;
        prefs.save();
        true
    }

    /// The new game options the player chose about (new_options.rs).
    pub fn options_asked() -> Vec<String> {
        Self::load().options_asked
    }

    pub fn add_options_asked<'a>(ids: impl IntoIterator<Item = &'a str>) {
        let mut prefs = Self::load();
        for id in ids {
            if !prefs.options_asked.iter().any(|a| a == id) {
                prefs.options_asked.push(id.to_string());
            }
        }
        prefs.save();
    }

    /// The release whose What's new was last shown or noted.
    pub fn whats_new_seen() -> Option<String> {
        Self::load().whats_new_seen
    }

    pub fn set_whats_new_seen(release: &str) {
        let mut prefs = Self::load();
        prefs.whats_new_seen = Some(release.to_string());
        prefs.save();
    }

    pub fn uplay_emulator_told() -> bool {
        Self::load().uplay_emulator_told
    }

    pub fn start_through_steam() -> bool {
        Self::load().start_through_steam
    }

    pub fn play_mode() -> PlayMode {
        Self::load().play_mode
    }

    pub fn quick_start_set() -> bool {
        Self::load().quick_start_set
    }

    pub fn set_quick_start_set(set: bool) {
        let mut prefs = Self::load();
        if prefs.quick_start_set != set {
            prefs.quick_start_set = set;
            prefs.save();
        }
    }

    pub fn quick_start_by_hand() -> Option<hooks_config::QuickStartLobby> {
        Self::load().quick_start_by_hand
    }

    pub fn set_quick_start_by_hand(lobby: Option<hooks_config::QuickStartLobby>) {
        let mut prefs = Self::load();
        if prefs.quick_start_by_hand != lobby {
            prefs.quick_start_by_hand = lobby;
            prefs.save();
        }
    }

    /// Linux: the player chose the classic (egui) interface.
    pub fn classic_interface() -> bool {
        Self::load().classic_interface
    }

    pub fn relay() -> Option<String> {
        Self::load().relay
    }

    pub fn set_relay(relay: Option<String>) {
        let mut prefs = Self::load();
        prefs.relay = relay;
        prefs.save();
    }

    pub fn set_classic_interface(on: bool) {
        let mut prefs = Self::load();
        prefs.classic_interface = on;
        prefs.save();
    }

    pub fn set_play_mode(mode: PlayMode) {
        let mut prefs = Self::load();
        prefs.play_mode = mode;
        prefs.save();
    }

    pub fn set_start_through_steam(on: bool) {
        let mut prefs = Self::load();
        prefs.start_through_steam = on;
        prefs.save();
    }

    pub fn set_uplay_emulator_told() {
        let mut prefs = Self::load();
        prefs.uplay_emulator_told = true;
        prefs.save();
    }

    pub fn replies_seen() -> Option<i64> {
        Self::load().replies_seen
    }

    pub fn set_replies_seen(at: i64) {
        let mut prefs = Self::load();
        prefs.replies_seen = Some(at);
        prefs.save();
    }

    pub fn support_seen() -> Option<i64> {
        Self::load().support_seen
    }

    pub fn set_support_seen(id: i64) {
        let mut prefs = Self::load();
        if prefs.support_seen.is_none_or(|seen| id > seen) {
            prefs.support_seen = Some(id);
            prefs.save();
        }
    }

    pub fn set_feedback_off(off: bool) {
        let mut prefs = Self::load();
        prefs.feedback_off = off;
        prefs.save();
    }

    /// Servers of their own the player connected to, newest first.
    pub fn recent_servers() -> Vec<RecentServer> {
        Self::load().recent_servers
    }

    /// Notes a server of the player's own as just used.
    pub fn remember_server(address: &str, name: Option<String>) {
        let mut prefs = Self::load();
        prefs.recent_servers = note_recent(std::mem::take(&mut prefs.recent_servers), address, name, identity::now());
        prefs.save();
    }

    /// Back to the community network's directory.
    pub fn use_community_network() {
        let mut prefs = Self::load();
        prefs.directory = None;
        prefs.directory_set = false;
        prefs.save();
        Self::forget_cached_directory();
    }

    /// Whether `url` is the community network's directory, by either of its names.
    pub fn is_community(url: &str) -> bool {
        setup::directory::is_community(url)
    }

    /// Sets (or with None, clears) the server directory.
    pub fn set_directory(url: Option<String>) {
        let mut prefs = Self::load();
        prefs.directory = url.map(|u| Self::directory_as_kept(&u)).filter(|u| !u.is_empty());
        prefs.directory_set = true;
        prefs.save();
        Self::forget_cached_directory();
    }

    /// Uses the directory a server reports, only when the player has none (they cleared
    /// it): never in place of the community network's, which is the default, nor one they
    /// chose. Any HTTPS server can name one, and it decides the server list, the automatic
    /// pick and the notices, so taking it unasked would hand all that to whoever runs the
    /// server. Only `https://` ones. Says whether it was adopted.
    pub fn adopt_directory(url: &str) -> bool {
        let mut prefs = Self::load();
        if prefs.directory.is_some() || !prefs.directory_set || !setup::directory::valid_coordinator(url) {
            return false;
        }
        prefs.directory = Some(Self::directory_as_kept(url));
        prefs.save();
        Self::forget_cached_directory();
        true
    }

    fn forget_cached_directory() {
        *DIRECTORY_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_play_mode_is_kept_and_reaches_the_game() {
        // Kept in launcher.toml, and not written while it's the plain Play.
        let mut prefs = Prefs::default();
        assert!(!toml::to_string(&prefs).unwrap().contains("play_mode"));
        prefs.play_mode = PlayMode::Coop;
        let back: Prefs = toml::from_str(&toml::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(back.play_mode, PlayMode::Coop);
        let newer: Prefs = toml::from_str("play_mode = \"something-newer\"\nstart_through_steam = true\n").unwrap();
        assert_eq!((newer.play_mode, newer.start_through_steam), (PlayMode::Play, true), "an unknown choice is a plain Play");
        // What the game's DLL reads from the uplay.toml the launcher writes.
        let read = |cfg: &setup::config::Config| toml::from_str::<hooks_config::Config>(&toml::to_string_pretty(cfg).unwrap()).unwrap().quick_start_lobby;
        let mut cfg = setup::config::Config::default();
        let mut set = false;
        let mut by_hand = None;
        for (mode, lobby) in [
            (PlayMode::Svm, Some(hooks_config::QuickStartLobby::Svm)),
            (PlayMode::Coop, Some(hooks_config::QuickStartLobby::Coop)),
            (PlayMode::Play, None),
            (PlayMode::Play, None),
        ] {
            set = mode.apply(&mut cfg, set, &mut by_hand);
            assert_eq!(read(&cfg), lobby, "{mode:?}");
            assert_eq!(set, lobby.is_some());
        }
        // A lobby set by hand: a plain Play leaves it, a launcher choice replaces it for that
        // start, and the next plain Play puts it back.
        cfg.hook_config.quick_start_lobby = Some(hooks_config::QuickStartLobby::Coop);
        assert!(!PlayMode::Play.apply(&mut cfg, false, &mut by_hand));
        assert_eq!(read(&cfg), Some(hooks_config::QuickStartLobby::Coop), "set by hand, kept");
        assert!(PlayMode::Svm.apply(&mut cfg, false, &mut by_hand));
        assert_eq!(read(&cfg), Some(hooks_config::QuickStartLobby::Svm));
        assert!(!PlayMode::Play.apply(&mut cfg, true, &mut by_hand));
        assert_eq!(read(&cfg), Some(hooks_config::QuickStartLobby::Coop), "the hand-set one, back");
        assert_eq!(by_hand, None);
        // None set by hand: the launcher's own, cleared.
        cfg.hook_config.quick_start_lobby = None;
        assert!(PlayMode::Coop.apply(&mut cfg, false, &mut by_hand));
        assert!(!PlayMode::Play.apply(&mut cfg, true, &mut by_hand));
        assert_eq!(read(&cfg), None, "the launcher's own, cleared");
    }

    #[test]
    fn recent_servers_newest_first_and_few() {
        let mut list = Vec::new();
        for (i, a) in ["a.example", "b.example", "c.example", "d.example", "e.example", "f.example"].iter().enumerate() {
            list = note_recent(list, a, None, i as i64);
        }
        let addresses: Vec<&str> = list.iter().map(|r| r.address.as_str()).collect();
        assert_eq!(addresses, ["f.example", "e.example", "d.example", "c.example", "b.example"]);
        // Used again: to the top, keeping the name it had.
        list[2].name = Some("LAN party".into());
        let list = note_recent(list, " D.example ", None, 99);
        assert_eq!((list[0].address.as_str(), list[0].name.as_deref(), list[0].last_used), ("D.example", Some("LAN party"), 99));
        assert_eq!(list.len(), 5);
        assert!(Prefs::is_community("https://play.scbl.jdevwebb.net/"), "its name until 0.4.5");
        assert!(Prefs::is_community(COMMUNITY_DIRECTORY));
        assert_eq!(Prefs::directory_as_kept(" https://play.scbl.jdevwebb.net/ "), COMMUNITY_DIRECTORY);
        assert_eq!(Prefs::directory_as_kept(" https://play.example.org "), "https://play.example.org");
        assert!(!Prefs::is_community("https://play.example.org"));
    }
}
