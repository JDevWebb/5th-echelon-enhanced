//! The window: header, navigation, and the game the screens work on.

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use crate::play::Play;
use crate::server::Server;
use crate::settings::Settings;
use crate::task::Slot;
use crate::theme;

const PRODUCT: &str = env!("FE_PRODUCT");
const RELEASE: &str = env!("FE_RELEASE");

/// Where players find the project and its developer, from the side menu.
const GITHUB: &str = "https://github.com/JDevWebb/5th-echelon-enhanced";
const DISCORD_NAME: &str = "nexusx44";
/// The developer's Discord profile: Discord has no link that opens a direct message.
const DISCORD_PROFILE: &str = "https://discord.com/users/257386895418458132";

/// The GitHub and Discord marks (white on clear, from launcher/assets), as textures.
fn marks(ctx: &egui::Context) -> [egui::TextureHandle; 2] {
    let size: usize = env!("MARK_SIZE").parse().unwrap_or(96);
    let options = egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
    let load = |name: &str, rgba: &[u8]| ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied([size, size], rgba), options);
    [
        load("github-mark", include_bytes!(concat!(env!("OUT_DIR"), "/github.dat"))),
        load("discord-mark", include_bytes!(concat!(env!("OUT_DIR"), "/discord.dat"))),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Play,
    /// Every public match on the player's server (the 0.4.8 lobby browser).
    Matches,
    /// Friends, requests, finding players, blocked.
    Friends,
    /// The servers to play on: the network's directory, an address, the LAN.
    Servers,
    /// The server's news.
    News,
    /// What's being built, and suggestions for it.
    Roadmap,
    /// Chat on the community network: Global, the lobby, friends.
    Chat,
    /// A conversation with the community network's admins.
    Support,
    Settings,
    /// Hosting a server of your own.
    Server,
}

/// Notices and tasks, in the activity bar along the bottom (activity.rs): `info` and
/// `error` make a one-line activity.
pub use crate::activity::Activities as Notices;
pub use crate::prefs::Game;
pub use crate::prefs::PlayMode;
pub use crate::prefs::Prefs;

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
    /// New game options, asked once after an update brings them (new_options.rs).
    new_options: crate::new_options::Ask,
    /// What's new, once after an update (whats_new.rs).
    whats_new: crate::whats_new::WhatsNew,
    /// The side menu's GitHub and Discord marks.
    marks: Option<[egui::TextureHandle; 2]>,
    settings: Settings,
    server: Server,
    roadmap: crate::roadmap::Roadmap,
    support: crate::support::Support,
    chat: crate::chat::Chat,
    /// Whether the diagnostics notice for players who agreed before 0.4.3 was looked at.
    log_excerpt_checked: bool,
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
            new_options: crate::new_options::Ask::default(),
            whats_new: crate::whats_new::WhatsNew::default(),
            marks: None,
            settings: Settings::default(),
            server: Server::default(),
            roadmap: crate::roadmap::Roadmap::default(),
            support: crate::support::Support::default(),
            chat: crate::chat::Chat::default(),
            log_excerpt_checked: false,
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

    /// Opens a game folder (prefs.rs tidies it and says what it did).
    fn open_game(&mut self, dir: PathBuf) {
        self.game = Some(Game::open_tidied(dir, &mut self.notices));
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

    /// The side menu: the goggles' lights, the screens as icons (named on hover), then Support and
    /// Settings, the other sites and the version at the bottom.
    fn rail(&mut self, ctx: &egui::Context) {
        // The roadmap, chat and support are the community network's: shown to its players only.
        let community = Prefs::directory().is_some_and(|d| Prefs::is_community(&d));
        let current = self.view;
        let unread = [
            (View::Chat, self.chat.unread()),
            (View::Support, self.support.unread()),
            (View::Friends, u32::try_from(self.play.friend_requests()).unwrap_or(u32::MAX)),
        ];
        let requests = self.play.friend_requests();
        let friends_label = if requests > 0 {
            format!("Friends: {requests} waiting for your answer")
        } else {
            "Friends".to_string()
        };
        let mut go = None;
        let mut item = |ui: &mut egui::Ui, view: View, icon: theme::Icon, label: &str| {
            if matches!(view, View::Chat | View::Roadmap | View::Support) && !community {
                return;
            }
            // Hosting is reached from Servers, and lights it.
            let on = current == view || (view == View::Servers && current == View::Server) || (view == View::News && current == View::Roadmap);
            let button = theme::nav_button(ui, icon, label, on);
            // Direct messages and mentions, and answers from support, not seen yet: a dot.
            if !on && unread.iter().any(|(v, n)| *v == view && *n > 0) {
                let at = button.rect.right_top() + egui::vec2(-8.0, 8.0);
                ui.painter().circle_filled(at, 5.0, theme::RAIL);
                ui.painter().circle_filled(at, 3.5, theme::ACCENT);
            }
            if button.clicked() {
                go = Some(view);
            }
        };
        egui::SidePanel::left("rail")
            .resizable(false)
            .exact_width(60.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::RAIL)
                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                    .inner_margin(egui::Margin::symmetric(9, 18)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.vertical_centered(|ui| {
                    // The goggles' lights head the title bar when the launcher draws its own.
                    if !crate::window_frame::custom() {
                        theme::lights(ui, 6.0);
                        ui.add_space(20.0);
                    }
                    // The same screens, in the same order, as the Tauri launcher's side menu.
                    item(ui, View::Play, theme::Icon::Play, "Play");
                    item(ui, View::Matches, theme::Icon::Matches, "Matches");
                    item(ui, View::Friends, theme::Icon::Friends, &friends_label);
                    item(ui, View::Servers, theme::Icon::Servers, "Servers");
                    item(ui, View::Chat, theme::Icon::Chat, "Chat");
                    item(ui, View::News, theme::Icon::News, "Intel: news and the roadmap");
                });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    let version = ui
                        .add(
                            egui::Label::new(egui::RichText::new(RELEASE.split('-').next().unwrap_or(RELEASE)).monospace().size(10.5).color(theme::MUTED))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(format!("{PRODUCT} {RELEASE}: what's new"));
                    if version.clicked() && !self.whats_new.reopen() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(crate::updater::RELEASES_PAGE));
                    }
                    ui.add_space(4.0);
                    let [github, discord] = self.marks.get_or_insert_with(|| marks(ui.ctx())).clone();
                    let discord = theme::mark_button(ui, &discord, "Message the developer on Discord")
                        .on_hover_text(format!("Message {DISCORD_NAME} on Discord: bugs, questions, or a game.\nRight-click to copy the name."));
                    if discord.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(DISCORD_PROFILE));
                    }
                    if discord.secondary_clicked() {
                        ui.ctx().copy_text(DISCORD_NAME.to_string());
                        self.notices.info(format!("Copied {DISCORD_NAME}: add it in Discord to message the developer."));
                    }
                    let github = theme::mark_button(ui, &github, "5th Echelon Enhanced on GitHub").on_hover_text("5th Echelon Enhanced on GitHub: releases, issues and the code");
                    if github.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(GITHUB));
                    }
                    ui.add_space(10.0);
                    item(ui, View::Settings, theme::Icon::Settings, "Settings");
                    item(ui, View::Support, theme::Icon::Support, "Support");
                });
            });
        if let Some(view) = go {
            self.view = view;
        }
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

    /// The player's reports with the admins' replies, as last read.
    pub fn my_reports(&self) -> Option<&Result<Vec<setup::feedback::MyReport>, String>> {
        self.feedback.mine.as_ref()
    }

    pub fn read_my_reports(&mut self, ctx: &egui::Context) {
        self.feedback.read_mine(ctx);
    }

    pub fn open_settings(&mut self, section: crate::settings::Section) {
        self.settings.section = section;
        self.view = View::Settings;
    }

    /// Support, with a message started (`text`) for the player to finish and send.
    pub fn open_support(&mut self, text: String) {
        self.support.draft(text);
        self.view = View::Support;
    }
}

/// The screenshots' sub-screens (shots.rs): what `what` names, set up on a new app.
#[cfg(all(test, feature = "shots"))]
impl App {
    pub(crate) fn shot(&mut self, ctx: &egui::Context, what: &str) {
        use crate::play::Drawer;
        match what {
            "drawer-status" => self.play.shot_drawer(Drawer::Status),
            "drawer-squad" => self.play.shot_drawer(Drawer::Squad),
            "drawer-intel" => self.play.shot_drawer(Drawer::News),
            "setup-name" => self.play.shot_name_step("eu1.scbl.jdevwebb.net"),
            "matches-sample" => self.play.shot_matches(),
            "friends-sample" => self.play.shot_friends(crate::play::PeopleTab::Friends),
            "friends-requests" => self.play.shot_friends(crate::play::PeopleTab::Requests),
            "whats-new" => {
                self.whats_new.reopen();
            }
            "feedback" => self.ask_feedback(ctx),
            "server-menu" => egui::Popup::open_id(ctx, egui::Id::new("server-menu")),
            _ => {
                if let Some(section) = crate::settings::Section::ALL.into_iter().find(|s| what == format!("settings-{:?}", s).to_lowercase()) {
                    self.view = View::Settings;
                    self.settings.section = section;
                }
            }
        }
    }
}

impl eframe::App for App {
    /// Behind everything: the page's colour, so nothing between panels shows through.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

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
        // The update notice offers a newer launcher (not when another tool updates it).
        let managed = self.game.as_ref().is_some_and(|g| g.managed.is_some());
        self.play.newer_launcher = self.latest.as_ref().filter(|l| l.newer() && !managed).map(|l| l.version.clone());
        if let Some((code, checks)) = self.play.take_closed() {
            if let Some(game) = &self.game {
                self.feedback.game_closed(ctx, game.dir.clone(), code, checks);
            }
        }
        if let Some(game) = &self.game {
            self.feedback.resume(ctx, &mut self.notices, &game.cfg);
        }
        self.feedback.show(ctx, &mut self.notices);
        // Agreed before 0.4.3: told once what the diagnostics now include (not asked again).
        // Looked at once a run (it reads the prefs file).
        if !std::mem::replace(&mut self.log_excerpt_checked, true) && self.game.as_ref().is_some_and(|g| g.cfg.hook_config.send_diagnostics) && Prefs::tell_log_excerpt() {
            self.notices.info(
                "New in the game's diagnostics: when something goes wrong, the server can ask for the 15 minutes of the game's log before it (at most once an hour, never the whole log, private details hidden). Settings › Feedback turns the diagnostics off.",
            );
        }
        self.diagnostics.show(ctx, self.game.as_mut(), &mut self.notices);
        // After the diagnostics, unless the guided setup asks about them in its own step.
        let setup_asks = self.play.asks_new_options();
        self.new_options.show(ctx, self.game.as_mut(), &mut self.notices, self.diagnostics.asking() || setup_asks);
        self.whats_new.start(self.game.as_ref());
        let community = Prefs::directory().is_some_and(|d| Prefs::is_community(&d));
        let waiting = self.diagnostics.asking() || self.new_options.asking() || setup_asks;
        if let Some(crate::whats_new::Action::UseCommunityNetwork) = self.whats_new.show(ctx, waiting, community) {
            self.play.use_community_network(&mut self.notices);
            self.view = View::Play;
        }
        crate::play::show_account_dialog(self, ctx);
        self.roadmap.tick(ctx, &mut self.notices);
        self.support.tick(ctx, &mut self.notices, self.view == View::Support);
        self.chat.tick(ctx, self.game.as_ref(), &mut self.notices, self.view == View::Chat);
        if let Some(Err(e)) = self.updating.poll() {
            match self.updating_activity.take() {
                Some(a) => {
                    a.title("Couldn't update the launcher");
                    a.fail(e.to_string(), &[crate::activity::Action::CopyDetails]);
                }
                None => self.notices.error(format!("Couldn't update: {e}")),
            }
        }

        // The window's own title bar first, so it spans the side menu too.
        crate::window_frame::title_bar(ctx);
        crate::window_frame::edges(ctx);
        self.rail(ctx);
        self.update_banner(ctx);
        // A seat opened in the match the player watches: a bar along the top, wherever they are.
        crate::play::watch_seat(self, ctx);
        if self.view != View::Friends {
            self.play.left_friends();
        }
        self.notices.show(ctx);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG)).show(ctx, |ui| match self.view {
            // Each screen scrolls on its own: Settings keeps its section list in place.
            View::Play => crate::play::show(self, ui),
            View::Matches => crate::play::show_matches(self, ui),
            View::Friends => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| crate::play::show_friends(self, ui));
            }
            View::Servers => crate::play::show_servers(self, ui),
            View::News => crate::play::show_news(self, ui),
            View::Roadmap if Prefs::directory().is_some_and(|d| Prefs::is_community(&d)) => crate::roadmap::show(self, ui),
            View::Support if Prefs::directory().is_some_and(|d| Prefs::is_community(&d)) => crate::support::show(self, ui),
            View::Chat if crate::chat::available() => crate::chat::show(self, ui),
            // Left the community network while on it: back to Play.
            View::Roadmap | View::Support | View::Chat => self.view = View::Play,
            View::Settings => crate::settings::show(self, ui),
            View::Server => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    theme::page().show(ui, |ui| {
                        ui.set_max_width(theme::MAX_PAGE);
                        theme::title(ui, "On this PC", "Host");
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

    pub fn roadmap_mut(&mut self) -> (&mut crate::roadmap::Roadmap, &Option<Game>, &mut Notices) {
        (&mut self.roadmap, &self.game, &mut self.notices)
    }

    pub fn support_mut(&mut self) -> (&mut crate::support::Support, &Option<Game>, &mut Notices) {
        (&mut self.support, &self.game, &mut self.notices)
    }

    pub fn chat_mut(&mut self) -> (&mut crate::chat::Chat, &Option<Game>, &mut Notices) {
        (&mut self.chat, &self.game, &mut self.notices)
    }
}
