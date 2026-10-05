//! The window: header, navigation, and the game the screens work on.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use eframe::egui;
use serde::Deserialize;
use serde::Serialize;
use setup::config::Config;
use setup::config::ConfigMut;
use setup::overrides::Managed;

use crate::play::Play;
use crate::server::Server;
use crate::settings::Settings;
use crate::task::Slot;
use crate::theme;

const PRODUCT: &str = env!("FE_PRODUCT");
const RELEASE: &str = env!("FE_RELEASE");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Play,
    /// The servers to play on: the network's directory, an address, the LAN.
    Servers,
    /// The server's news.
    News,
    Settings,
    /// Hosting a server of your own.
    Server,
}

/// The game folder the launcher works on, with its settings.
pub struct Game {
    pub dir: PathBuf,
    pub cfg: ConfigMut,
    /// Set when another tool manages this install (`uplay.override.toml`).
    pub managed: Option<Managed>,
}

impl Game {
    pub fn open(dir: PathBuf) -> Self {
        let cfg = Config::load(&dir);
        let managed = setup::overrides::managed_by(&dir);
        Self { dir, cfg, managed }
    }

    /// Picks up changes to the settings made on another thread or by hand.
    pub fn reload(&mut self) {
        self.cfg.reload_if_changed();
        self.managed = setup::overrides::managed_by(&self.dir);
    }

    /// Changes the settings, reporting a failure to save.
    pub fn update(&mut self, notices: &mut Notices, f: impl FnOnce(&mut Config)) {
        if let Err(e) = self.cfg.update(f) {
            notices.error(format!("Couldn't save the settings: {e}"));
        }
    }
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
    game_dir: Option<PathBuf>,
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
    /// Servers of their own the player connected to (Servers › A server of your own), newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recent_servers: Vec<RecentServer>,
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
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let Some(path) = Self::path() else { return };
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
        if let Ok(s) = toml::to_string(self) {
            let _ = std::fs::write(path, s);
        }
    }

    fn remember(dir: &Path) {
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
        crate::scale::clamp(Self::load().ui_scale.unwrap_or(1.0))
    }

    pub fn set_ui_scale(size: f32) {
        let mut prefs = Self::load();
        let size = crate::scale::clamp(size);
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
        prefs.save();
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

    /// Whether `url` is the community network's directory.
    pub fn is_community(url: &str) -> bool {
        let key = |u: &str| u.trim().trim_end_matches('/').to_ascii_lowercase();
        key(url) == key(COMMUNITY_DIRECTORY)
    }

    /// Sets (or with None, clears) the server directory.
    pub fn set_directory(url: Option<String>) {
        let mut prefs = Self::load();
        prefs.directory = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        prefs.directory_set = true;
        prefs.save();
        Self::forget_cached_directory();
    }

    /// Uses the directory a server reports, when none is set yet (one the
    /// player chose is never replaced). Only `https://` ones. Says whether it
    /// was adopted.
    pub fn adopt_directory(url: &str) -> bool {
        let mut prefs = Self::load();
        if prefs.directory.is_some() || prefs.directory_set || !setup::directory::valid_coordinator(url) {
            return false;
        }
        prefs.directory = Some(url.trim().to_string());
        prefs.save();
        Self::forget_cached_directory();
        true
    }

    fn forget_cached_directory() {
        *DIRECTORY_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// Notices and tasks, in the activity bar along the bottom (activity.rs): `info` and
/// `error` make a one-line activity.
pub use crate::activity::Activities as Notices;

pub struct App {
    view: View,
    pub game: Option<Game>,
    /// Game folders found on this PC.
    pub found: Vec<PathBuf>,
    finding: Slot<Vec<PathBuf>>,
    pub notices: Notices,
    play: Play,
    /// "How did that go?" after a game (feedback.rs).
    feedback: crate::feedback::Feedback,
    /// Asking once about the game's diagnostics (diagnostics.rs).
    diagnostics: crate::diagnostics::Ask,
    settings: Settings,
    server: Server,
    /// The latest release, once looked up.
    pub latest: Option<crate::updater::Latest>,
    pub checking: Slot<anyhow::Result<crate::updater::Latest>>,
    updating: Slot<anyhow::Result<()>>,
    /// The update being installed, and the one asked for by the player's check.
    updating_activity: Option<crate::activity::Handle>,
    checking_activity: Option<crate::activity::Handle>,
    update_later: bool,
    /// When the last look for a release started: release builds look again now and then.
    checked_at: Option<std::time::Instant>,
    /// Install the release the running check finds (the player asked to update).
    install_found: bool,
    /// The player's size (Settings › Display), and whether the window was fitted to the screen.
    pub ui_scale: f32,
    sized: bool,
}

/// How often a release build that stays open looks for a newer release.
const RECHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut app = Self {
            view: View::Play,
            game: None,
            found: Vec::new(),
            finding: Slot::default(),
            notices: Notices::default(),
            play: Play::default(),
            feedback: crate::feedback::Feedback::default(),
            diagnostics: crate::diagnostics::Ask::default(),
            settings: Settings::default(),
            server: Server::default(),
            latest: None,
            checking: Slot::default(),
            updating: Slot::default(),
            updating_activity: None,
            checking_activity: None,
            update_later: false,
            checked_at: None,
            install_found: false,
            ui_scale: Prefs::ui_scale(),
            sized: false,
        };
        // Ctrl + and Ctrl − change the player's size instead (scale.rs).
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        // Release builds look for a newer release on their own.
        if !crate::updater::is_dev_build() {
            app.check_for_update(&cc.egui_ctx);
        }
        match Prefs::load().game_dir.filter(|d| setup::game::is_game_dir(d)) {
            Some(dir) => app.open_game(dir),
            None => app.find_games(&cc.egui_ctx),
        }
        app
    }

    /// Looks for the game in the background; the first one found is used if
    /// none is chosen yet.
    pub fn find_games(&mut self, ctx: &egui::Context) {
        self.finding.start(ctx, setup::game::find_game_dirs);
    }

    pub fn finding_games(&self) -> bool {
        self.finding.running()
    }

    pub fn choose_game(&mut self, dir: PathBuf) {
        Prefs::remember(&dir);
        self.open_game(dir);
        self.play.game_changed();
    }

    /// Opens a game folder, saying so when its settings came from an earlier launcher and
    /// started over.
    fn open_game(&mut self, dir: PathBuf) {
        let game = Game::open(dir);
        // Data version logging's files, from before it was off by default (0.4.2).
        if !game.cfg.hook_config.log_data_version {
            hooks_config::remove_data_version_files(&game.dir);
        }
        if let Some(kept) = game.cfg.started_over() {
            self.notices.info(format!(
                "Settings from an earlier 5th Echelon were set aside (kept as {} in the game folder); this launcher starts from its own.",
                kept.file_name().map_or_else(|| kept.display().to_string(), |n| n.to_string_lossy().into_owned())
            ));
        }
        self.game = Some(game);
    }

    /// Asks for the game folder with a folder picker.
    pub fn pick_game_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Choose the folder with Blacklist_game.exe").pick_folder() {
            match setup::game::game_dir_in(&dir) {
                Some(found) => self.choose_game(found),
                None => self.notices.error(format!("{} doesn't contain Splinter Cell: Blacklist.", dir.display())),
            }
        }
    }

    pub fn check_for_update(&mut self, ctx: &egui::Context) {
        if self.checking.running() {
            return;
        }

        self.checked_at = Some(std::time::Instant::now());
        self.checking.start(ctx, crate::updater::latest);
    }

    /// Looks for a newer release because the player asked, in the activity bar.
    pub fn check_for_update_shown(&mut self, ctx: &egui::Context) {
        if self.checking.running() {
            return;
        }
        self.checking_activity = Some(self.notices.start(ctx, "Looking for a newer launcher"));
        self.check_for_update(ctx);
    }

    /// Installs `latest` and restarts into it.
    fn install_update(&mut self, ctx: &egui::Context, latest: crate::updater::Latest) {
        if !self.updating.running() {
            let activity = self.notices.start(ctx, format!("Updating the launcher to {}", latest.version));
            activity.step("Downloading the new launcher; it restarts when it's ready");
            self.updating_activity = Some(activity);
            self.updating.start(ctx, move || crate::updater::update_self(&latest));
        }
    }

    /// Looks again now and then, a server refusing this launcher as outdated looks at once
    /// (and shows the update again even after "Later"), and the checklist's Update installs
    /// the release, or says there isn't one yet.
    fn keep_up_to_date(&mut self, ctx: &egui::Context) {
        let dev = crate::updater::is_dev_build();
        if std::mem::take(&mut self.play.refused_as_outdated) && !dev {
            self.update_later = false;
            self.check_for_update(ctx);
        }
        if std::mem::take(&mut self.play.update_asked) {
            if dev {
                self.notices
                    .error("This is a development build: it doesn't update itself. Get the new one from whoever sent it.");
            } else if let Some(latest) = self.latest.clone().filter(|l| l.newer()) {
                self.install_update(ctx, latest);
            } else {
                self.install_found = true;
                self.check_for_update_shown(ctx);
            }
        }
        if !dev {
            let due = self.checked_at.map_or(RECHECK_EVERY, |t| RECHECK_EVERY.saturating_sub(t.elapsed()));
            if due.is_zero() {
                self.check_for_update(ctx);
            } else {
                ctx.request_repaint_after(due);
            }
        }
    }

    /// Offers a newer release, unless another tool manages this install
    /// (it updates the launcher too).
    fn update_banner(&mut self, ctx: &egui::Context) {
        let managed = self.game.as_ref().is_some_and(|g| g.managed.is_some());
        let Some(latest) = self.latest.clone().filter(|l| l.newer() && !managed && !self.update_later) else {
            return;
        };
        egui::TopBottomPanel::top("update")
            .frame(egui::Frame::new().fill(theme::ACCENT.linear_multiply(0.14)).inner_margin(egui::Margin::symmetric(20, 8)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.updating.running() {
                        ui.spinner();
                        ui.label(format!("Updating to {}…", latest.version));
                        return;
                    }
                    ui.label(egui::RichText::new(format!("Version {} is available.", latest.version)).family(theme::strong()));
                    if ui.add(theme::primary("Update now")).clicked() {
                        self.install_update(ctx, latest.clone());
                    }
                    ui.hyperlink_to("What's new", &self.latest.as_ref().map(|l| l.page.clone()).unwrap_or_default());
                    if ui.button("Later").clicked() {
                        self.update_later = true;
                    }
                });
            });
    }

    /// The side menu: the mark, the screens, and the version at the bottom.
    fn rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("rail")
            .resizable(false)
            .exact_width(88.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::RAIL)
                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                    .inner_margin(egui::Margin::symmetric(11, 18)),
            )
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    theme::mark(ui, 34.0);
                    ui.add_space(22.0);
                    for (view, icon, label) in [
                        (View::Play, theme::Icon::Play, "Play"),
                        (View::Servers, theme::Icon::Servers, "Servers"),
                        (View::News, theme::Icon::News, "News"),
                        (View::Server, theme::Icon::Host, "Host"),
                        (View::Settings, theme::Icon::Settings, "Settings"),
                    ] {
                        if theme::nav_button(ui, icon, label, self.view == view).clicked() {
                            self.view = view;
                        }
                        ui.add_space(2.0);
                    }
                });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(RELEASE.split('-').next().unwrap_or(RELEASE)).monospace().size(10.5).color(theme::MUTED))
                        .on_hover_text(format!("{PRODUCT} {RELEASE}"));
                });
            });
    }

    pub fn set_view(&mut self, view: View) {
        self.view = view;
    }

    /// Settings, open at `section`.
    /// Changes the player's size, and remembers it.
    pub fn set_ui_scale(&mut self, size: f32) {
        self.ui_scale = crate::scale::clamp(size);
        Prefs::set_ui_scale(self.ui_scale);
    }

    /// The player wants to send feedback or report a problem (Settings › Feedback).
    pub fn ask_feedback(&mut self, ctx: &egui::Context) {
        match &self.game {
            Some(game) => self.feedback.ask_now(ctx, game.dir.clone(), self.play.checks_text()),
            None => self.notices.error("Find the game first (Settings › Game)."),
        }
    }

    pub fn feedback_busy(&self) -> bool {
        self.feedback.busy()
    }

    pub fn open_settings(&mut self, section: crate::settings::Section) {
        self.settings.section = section;
        self.view = View::Settings;
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(size) = crate::scale::apply(ctx, self.ui_scale, &mut self.sized) {
            self.set_ui_scale(size);
        }
        if let Some(found) = self.finding.poll() {
            if self.game.is_none() {
                if let Some(first) = found.first().cloned() {
                    self.choose_game(first);
                }
            }
            self.found = found;
        }
        if let Some(game) = &mut self.game {
            game.reload();
        }

        if let Some(result) = self.checking.poll() {
            let install = std::mem::take(&mut self.install_found);
            let activity = self.checking_activity.take();
            match result {
                Ok(latest) if install && latest.newer() => {
                    if let Some(a) = activity {
                        a.finish_quietly();
                    }
                    self.latest = Some(latest.clone());
                    self.install_update(ctx, latest);
                }
                Ok(latest) => {
                    match activity {
                        Some(a) if install => {
                            a.title("No newer launcher yet");
                            a.fail(
                                format!("{} is the latest: the server needs one that hasn't been published. Try again later.", latest.version),
                                &[],
                            );
                        }
                        Some(a) if latest.newer() => a.done(format!("Version {} is available", latest.version)),
                        Some(a) => a.done(format!("Up to date: {} is the latest release", latest.version)),
                        None => {}
                    }
                    self.latest = Some(latest);
                }
                Err(e) => {
                    tracing::warn!("Couldn't check for updates: {e}");
                    if let Some(a) = activity {
                        a.title("Couldn't look for a newer launcher");
                        a.fail(e.to_string(), &[crate::activity::Action::CopyDetails]);
                    }
                }
            }
        }
        self.keep_up_to_date(ctx);
        if let Some((code, checks)) = self.play.take_closed() {
            if let Some(game) = &self.game {
                self.feedback.game_closed(ctx, game.dir.clone(), code, checks);
            }
        }
        self.feedback.show(ctx, &mut self.notices);
        self.diagnostics.show(ctx, self.game.as_mut(), &mut self.notices);
        crate::play::show_account_dialog(self, ctx);
        if let Some(Err(e)) = self.updating.poll() {
            match self.updating_activity.take() {
                Some(a) => {
                    a.title("Couldn't update the launcher");
                    a.fail(e.to_string(), &[crate::activity::Action::CopyDetails]);
                }
                None => self.notices.error(format!("Couldn't update: {e}")),
            }
        }

        self.rail(ctx);
        self.update_banner(ctx);
        self.notices.show(ctx);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG)).show(ctx, |ui| match self.view {
            // Each screen scrolls on its own: Settings keeps its section list in place.
            View::Play => crate::play::show(self, ui),
            View::Servers => crate::play::show_servers(self, ui),
            View::News => crate::play::show_news(self, ui),
            View::Settings => crate::settings::show(self, ui),
            View::Server => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    theme::page().show(ui, |ui| {
                        ui.label(theme::display("Host", 32.0));
                        ui.label(theme::muted("Run a server for your group on this PC, or manage one you run elsewhere."));
                        ui.add_space(14.0);
                        crate::server::show(self, ui);
                    })
                });
            }
        });
        // Checks refresh on their own now and then; wake up for them.
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

/// The screens' own state, borrowed alongside the rest of the app.
impl App {
    pub fn play_mut(&mut self) -> (&mut Play, &mut Option<Game>, &mut Notices) {
        (&mut self.play, &mut self.game, &mut self.notices)
    }

    pub fn settings_mut(&mut self) -> (&mut Settings, &mut Option<Game>, &mut Notices) {
        (&mut self.settings, &mut self.game, &mut self.notices)
    }

    pub fn server_mut(&mut self) -> (&mut Server, &mut Notices) {
        (&mut self.server, &mut self.notices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(Prefs::is_community("https://play.scbl.jdevwebb.net/"));
        assert!(!Prefs::is_community("https://play.example.org"));
    }
}
