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
    Settings,
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
}

impl Prefs {
    fn path() -> Option<PathBuf> {
        setup::app_data_dir().map(|d| d.join("launcher.toml"))
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
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
        static CACHE: std::sync::Mutex<Option<(Instant, Option<String>)>> = std::sync::Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match cache.as_ref() {
            Some((at, url)) if at.elapsed() < Duration::from_secs(3) => url.clone(),
            _ => {
                let url = Self::load().directory;
                *cache = Some((Instant::now(), url.clone()));
                url
            }
        }
    }

    /// Sets (or with None, clears) the server directory.
    pub fn set_directory(url: Option<String>) {
        let mut prefs = Self::load();
        prefs.directory = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        prefs.save();
    }
}

/// Short messages at the bottom of the window.
#[derive(Default)]
pub struct Notices(Vec<(String, bool, Instant)>);

impl Notices {
    pub fn info(&mut self, text: impl Into<String>) {
        self.0.push((text.into(), false, Instant::now()));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.0.push((text.into(), true, Instant::now()));
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.0.retain(|(_, error, at)| at.elapsed() < if *error { Duration::from_secs(12) } else { Duration::from_secs(5) });
        if self.0.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        egui::TopBottomPanel::bottom("notices").frame(egui::Frame::new().fill(theme::SURFACE).inner_margin(egui::Margin::symmetric(16, 8))).show(ctx, |ui| {
            for (text, error, _) in &self.0 {
                ui.label(egui::RichText::new(text).color(if *error { theme::BAD } else { theme::FG }));
            }
        });
    }
}

pub struct App {
    view: View,
    pub game: Option<Game>,
    /// Game folders found on this PC.
    pub found: Vec<PathBuf>,
    finding: Slot<Vec<PathBuf>>,
    pub notices: Notices,
    logo: egui::TextureHandle,
    play: Play,
    settings: Settings,
    server: Server,
    /// The latest release, once looked up.
    pub latest: Option<crate::updater::Latest>,
    pub checking: Slot<anyhow::Result<crate::updater::Latest>>,
    updating: Slot<anyhow::Result<()>>,
    update_later: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let logo = cc.egui_ctx.load_texture("logo", crate::logo(), egui::TextureOptions::LINEAR);
        let mut app = Self {
            view: View::Play,
            game: None,
            found: Vec::new(),
            finding: Slot::default(),
            notices: Notices::default(),
            logo,
            play: Play::default(),
            settings: Settings::default(),
            server: Server::default(),
            latest: None,
            checking: Slot::default(),
            updating: Slot::default(),
            update_later: false,
        };
        // Release builds look for a newer release on their own.
        if !crate::updater::is_dev_build() {
            app.check_for_update(&cc.egui_ctx);
        }
        match Prefs::load().game_dir.filter(|d| setup::game::is_game_dir(d)) {
            Some(dir) => app.game = Some(Game::open(dir)),
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
        self.game = Some(Game::open(dir));
        self.play.game_changed();
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
        self.checking.start(ctx, crate::updater::latest);
    }

    /// Offers a newer release, unless another tool manages this install
    /// (it updates the launcher too).
    fn update_banner(&mut self, ctx: &egui::Context) {
        let managed = self.game.as_ref().is_some_and(|g| g.managed.is_some());
        let Some(latest) = self.latest.clone().filter(|l| l.newer() && !managed && !self.update_later) else { return };
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
                        self.updating.start(ctx, move || crate::updater::update_self(&latest));
                    }
                    ui.hyperlink_to("What's new", &self.latest.as_ref().map(|l| l.page.clone()).unwrap_or_default());
                    if ui.button("Later").clicked() {
                        self.update_later = true;
                    }
                });
            });
    }

    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 20, right: 20, top: 14, bottom: 10 }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&self.logo).fit_to_exact_size(egui::vec2(40.0, 40.0)));
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(PRODUCT).family(theme::strong()).size(20.0));
                        ui.label(theme::muted(format!("Release {RELEASE}")).small());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (view, label) in [(View::Server, "Server"), (View::Settings, "Settings"), (View::Play, "Play")].into_iter() {
                            let selected = self.view == view;
                            let text = egui::RichText::new(label).size(15.5).family(theme::strong());
                            let text = if selected { text.color(theme::ACCENT) } else { text.color(theme::MUTED) };
                            if ui.add(egui::Button::new(text).frame(false)).clicked() {
                                self.view = view;
                            }
                        }
                    });
                });
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
            match result {
                Ok(latest) => self.latest = Some(latest),
                Err(e) => tracing::warn!("Couldn't check for updates: {e}"),
            }
        }
        if let Some(Err(e)) = self.updating.poll() {
            self.notices.error(format!("Couldn't update: {e}"));
        }

        self.header(ctx);
        self.update_banner(ctx);
        self.notices.show(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 20, right: 20, top: 8, bottom: 16 }))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.view {
                    View::Play => crate::play::show(self, ui),
                    View::Settings => crate::settings::show(self, ui),
                    View::Server => crate::server::show(self, ui),
                });
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
