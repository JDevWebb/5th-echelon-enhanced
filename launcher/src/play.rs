//! The Play screen: join a server (Connect), see what's ready, fix
//! what isn't, and start the game.

use std::net::IpAddr;
use std::process::Child;
use std::time::Duration;
use std::time::Instant;

use eframe::egui;
use eframe::egui::RichText;
use setup::diagnose::Check;
use setup::diagnose::Fix;
use setup::diagnose::Status;

use crate::activity::Action;
use crate::app::App;
use crate::app::Game;
use crate::app::Notices;
use crate::app::View;
use crate::connect::place;
use crate::flow;
use crate::flow::Support;
use crate::server_menu::ping_text;
use crate::task::Slot;
use crate::theme;

/// How often the checklist refreshes on its own.
const REFRESH_EVERY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Play {
    /// The server just refused this launcher as outdated: the app looks for a newer release.
    pub(crate) refused_as_outdated: bool,
    /// The player asked to update the launcher (the checklist's Update).
    pub(crate) update_asked: bool,
    facts: Slot<(setup::diagnose::Facts, Support)>,
    /// Friends online here and on other servers sharing friends, fetched with the checklist.
    fetching_friends: Slot<Result<flow::Friends, String>>,
    /// A friend the player clicked Join on ((id, name)), acted on in `show` where the game is
    /// mutable (it may need starting first).
    join_requested: Option<(String, String)>,
    /// A join in flight (`flow::join_friend`): its line to show when it's done.
    joining: Slot<Result<String, String>>,
    friends: flow::Friends,
    /// An action on someone in "Online now" (asked to be friends, invited, blocked).
    acting: Slot<Result<String, String>>,
    /// The Friends screen: friends, requests, finding players, blocked.
    people: PeopleWindow,
    /// Whether the player's server was asked if it moved its players (global matchmaking), and
    /// the answer on its way.
    move_asked: bool,
    moving: Slot<Result<Option<(String, String)>, String>>,
    checks: Vec<Check>,
    support: Option<Support>,
    refreshed: Option<Instant>,

    /// The join form, shown when there's no server yet or on "Change server".
    editing: bool,
    /// The guided setup ran this session from a first run: it asks about the new game options
    /// in a step of its own once there's a server (the window after an update waits).
    fresh_setup: bool,
    /// That step's options and choices so far, once it showed.
    setup_options: Option<(Vec<&'static crate::new_options::NewOption>, Vec<bool>)>,
    server: String,
    /// The name for a new account, asked for when `needs_name` is set.
    nick: String,
    /// The server the player has no account on yet (their identity found
    /// none): the form asks for a name to make one.
    needs_name: Option<String>,
    looking: Slot<Result<Vec<IpAddr>, String>>,
    /// The servers that answered on this network, when more than one did.
    found: Vec<IpAddr>,
    /// The coordinator's server directory, with this PC's ping to each.
    browsing: Slot<Result<crate::network::Browsed, String>>,
    directory: Option<Vec<(setup::directory::Listing, Option<u32>)>>,
    /// The directory was browsed on its own since the last setup: once is enough.
    browsed: bool,
    /// The server field holds a server the launcher picked, not one typed:
    /// a newer pick may replace it.
    server_picked: bool,
    /// Why the directory couldn't be read, or that the servers are its last list,
    /// shown quietly with the servers.
    directory_error: Option<String>,
    /// When the directory was last read, and the release it says is going out.
    browsed_at: Option<Instant>,
    rollout: Option<setup::directory::Rollout>,
    /// The network's own maintenance window, as the directory last said.
    network_maintenance: Option<setup::directory::Window>,
    /// The player's server as the directory last listed it (it leaves the list while it restarts).
    my_listing: Option<setup::directory::Listing>,
    /// A newer launcher, when the app found one (for the update notice).
    pub(crate) newer_launcher: Option<String>,

    /// The setup: how it ended, the server it set up, and whether a network's directory chose it.
    setup: Slot<Result<(flow::Done, String, bool), String>>,
    /// A friend's server to switch to, waiting for the player to confirm it.
    switching: Option<Switch>,
    /// The server menu's row the arrow keys are on.
    menu_focus: usize,
    /// Asking for a name for a new account (or confirming a server of the player's own).
    account: Option<AccountDialog>,
    /// What's typed in Servers › Use another network, and its check.
    network_typed: String,
    checking_network: Slot<Result<(String, usize), String>>,
    /// The setup's activity, and what it was asked (to try again).
    setup_activity: Option<crate::activity::Handle>,
    last_setup: Option<crate::connect::Asked>,
    fixing: Slot<Result<String, String>>,
    identifying: Slot<Result<(), String>>,
    /// The fix's (or identifying the game's) activity, and the fix to try again.
    fix_activity: Option<(crate::activity::Handle, Option<Fix>)>,
    /// The activities that failed, for their Try again.
    setup_failed: Option<u64>,
    fix_failed: Option<(u64, Fix)>,
    running: Option<Child>,
    /// The game runs (started through Steam, or by hand); checked every few
    /// seconds.
    game_seen: bool,
    game_checked: Option<Instant>,
    /// The game just closed (its exit code, when the launcher started it), for the feedback
    /// ask; and when a close was last seen (both ways of seeing one fire for one close).
    closed: Option<Option<i32>>,
    closed_at: Option<Instant>,
    /// The home screen's checks: every one, not only what matters.
    show_all_checks: bool,
    /// What's typed in Servers › Join by address.
    address: String,
    /// This PC's identity in short, read once (it's decrypted from disk).
    identity: Option<Option<String>>,
    /// The banner's art, from the game's own loading screens (see `setup::key_art`).
    art: Slot<Result<setup::key_art::Art, String>>,
    art_texture: Option<egui::TextureHandle>,
    art_started: bool,
    /// The server's news, which server it's from, and when it was fetched.
    fetching_news: Slot<Result<(String, Vec<setup::server_info::NewsItem>), String>>,
    news: Vec<setup::server_info::NewsItem>,
    news_from: Option<(String, Instant)>,
    /// What the drawer beside the stage shows, once the player picked a tab.
    drawer: Option<Drawer>,
    /// The news item the home card shows, and when it last turned.
    news_index: usize,
    news_turned: Option<Instant>,
    /// What's being played on the player's server (`/api/playing`), which server it's from,
    /// and when it was asked.
    fetching_playing: Slot<Result<(String, Option<server_api::playing::Playing>), String>>,
    playing_now: Option<(String, server_api::playing::Playing)>,
    playing_from: Option<(String, Instant)>,
    /// What Play starts (the split button's choice), read from the launcher's settings once.
    play_mode: Option<crate::app::PlayMode>,
    /// Choosing your match (matches.rs): whether the player's server offers it (and which
    /// server that was), asked with the checklist.
    asking_offered: Slot<Result<(String, bool), String>>,
    offers_matches: Option<(String, bool)>,
    /// The open matches' window, its list, and a join in flight.
    choosing: bool,
    listing: Slot<Result<crate::matches::Matches, String>>,
    open_matches: Option<crate::matches::Matches>,
    joining_match: Slot<Result<String, String>>,
    /// A new match asked for (into this mode's lobby once the server said yes).
    asking_new: Slot<Result<Duration, String>>,
    new_mode: Option<crate::app::PlayMode>,
    /// The Matches screen.
    matches: MatchesPage,
}

/// A friend's server to switch to.
#[derive(Debug, Clone)]
struct Switch {
    host: String,
    /// The name for an account made there, if the player has none (None: the setup asks).
    new_name: Option<String>,
}

impl Play {
    /// Whether the guided setup asks about the new game options itself (new_options.rs).
    pub fn asks_new_options(&self) -> bool {
        self.fresh_setup
    }

    /// Back to the community network's directory: its servers fill the server menu again.
    pub fn use_community_network(&mut self, notices: &mut Notices) {
        crate::app::Prefs::use_community_network();
        self.browsed = false;
        self.directory = None;
        self.directory_error = None;
        notices.info("Back on the community network: its servers are in the server menu.");
    }

    pub fn game_changed(&mut self) {
        self.art_started = false;
        self.art_texture = None;
        self.checks.clear();
        self.support = None;
        self.refreshed = None;
        self.editing = false;
    }

    /// The identity's short form ("K7QF-2M9D"), if this PC has one.
    fn identity_short(&mut self) -> Option<String> {
        self.identity
            .get_or_insert_with(|| setup::player_identity::load().ok().flatten().map(|id| identity::short(&id.global_id())))
            .clone()
    }

    fn busy(&self) -> bool {
        self.setup.running() || self.fixing.running() || self.identifying.running()
    }

    fn refresh(&mut self, ctx: &egui::Context, game: &Game) {
        if self.facts.running() {
            return;
        }
        let dir = game.dir.clone();
        let cfg = (*game.cfg).clone();
        if !self.fetching_friends.running() && game.managed.is_none() {
            let cfg = cfg.clone();
            self.fetching_friends.start(ctx, move || flow::friends(&cfg));
        }
        let server = cfg.current_profile().map(|p| p.server.clone()).unwrap_or_default();
        if !server.is_empty() && !self.asking_offered.running() && game.managed.is_none() {
            // Asked of the server at most every 10 minutes (flow::offers).
            self.asking_offered.start(ctx, move || {
                let offered = flow::offers(&server, "choose-match");
                Ok((server, offered))
            });
        }
        self.facts.start(ctx, move || flow::gather(&dir, &cfg, crate::dll_utils::bundled()));
    }

    fn poll(&mut self, ctx: &egui::Context, game: &Game, notices: &mut Notices) {
        // A failed fetch keeps the last list: it's a hint, not a check.
        if let Some(Ok(friends)) = self.fetching_friends.poll() {
            self.friends = friends;
        }
        if let Some(result) = self.people.lists.poll() {
            self.people.shown = Some(result);
        }
        if let Some(result) = self.people.searching.poll() {
            self.people.found = Some(result);
        }
        if let Some(result) = self.acting.poll() {
            match result {
                Ok(done) => notices.info(done),
                Err(e) => notices.error(e),
            }
            // The Friends screen shows what changed.
            if self.people.open && !self.people.lists.running() {
                let cfg = (*game.cfg).clone();
                self.people.lists.start(ctx, move || flow::people(&cfg));
            }
            if !self.fetching_friends.running() && game.managed.is_none() {
                let cfg = (*game.cfg).clone();
                self.fetching_friends.start(ctx, move || flow::friends(&cfg));
            }
        }
        if let Some(result) = self.joining.poll() {
            match result {
                Ok(msg) => notices.info(msg),
                Err(msg) => notices.error(msg),
            }
        }
        if let Some(Ok(offered)) = self.asking_offered.poll() {
            self.offers_matches = Some(offered);
        }
        if let Some(listed) = self.listing.poll() {
            self.open_matches = Some(listed.unwrap_or_default());
        }
        if let Some(result) = self.joining_match.poll() {
            match result {
                Ok(msg) => notices.info(msg),
                Err(msg) => notices.error(msg),
            }
        }
        if let Some((facts, support)) = self.facts.poll() {
            let outdated = matches!(facts.account, Some(setup::diagnose::AccountFact::Outdated(_)));
            // Once per refusal, not on every refresh while it lasts.
            if outdated && !self.checks.iter().any(|c| c.fix == Some(Fix::UpdateLauncher)) {
                self.refused_as_outdated = true;
            }
            self.checks = setup::diagnose::checklist(&facts);
            self.support = Some(support);
            self.refreshed = Some(Instant::now());
        }
        let mut changed = false;
        if let Some(result) = self.setup.poll() {
            let activity = self.setup_activity.take();
            let asked = self.last_setup.clone();
            match result {
                Ok((flow::Done::Ready, server, via_network)) => {
                    if let Some(asked) = &asked {
                        if let Some(a) = &activity {
                            a.done(asked.done.clone());
                        }
                        crate::connect::ready(asked, &server, via_network, self.directory.as_deref());
                    } else {
                        flow::forget_account_check();
                    }
                    self.editing = false;
                    self.needs_name = None;
                    // The setup may have brought a directory: look at its servers again.
                    self.browsed = false;
                    self.directory = None;
                }
                Ok((flow::Done::NeedsName { server, suggested, taken }, _, _)) => {
                    if let Some(a) = &activity {
                        a.finish_quietly();
                    }
                    let first_run = game.cfg.current_profile().is_none_or(|p| p.server.is_empty());
                    if first_run {
                        // The guided setup asks for the name in its own step.
                        self.server = server.clone();
                        self.server_picked = false;
                        self.nick = suggested;
                        self.needs_name = Some(server);
                        self.editing = true;
                    } else {
                        // The network's server, or a friend's: the account dialog asks.
                        let own = !asked.as_ref().is_some_and(|a| a.plan.public_only);
                        self.account = Some(AccountDialog::new(ctx, game, &server, suggested, taken, own, self.directory.as_deref()));
                    }
                }
                Err(e) => match (&activity, &asked) {
                    (Some(a), Some(asked)) => {
                        a.title(asked.failed.clone());
                        a.fail(e, &[Action::Retry, Action::CopyDetails]);
                    }
                    _ => notices.error(e),
                },
            }
            changed = true;
        }
        // Try again, from the activity bar.
        if let (Some(id), Some(asked)) = (self.setup_failed, self.last_setup.clone()) {
            if notices.take_action(id) == Some(Action::Retry) && !self.busy() {
                self.server = asked.plan.server.clone();
                start_setup(self, game, ctx, notices, asked.plan.new_name.clone(), asked.plan.public_only);
            }
        }
        if let Some(result) = self
            .fixing
            .poll()
            .or_else(|| self.identifying.poll().map(|r| r.map(|()| "The game version is now supported.".to_string())))
        {
            let activity = self.fix_activity.take();
            match (result, &activity) {
                (Ok(msg), Some((a, _))) => a.done(msg),
                (Err(e), Some((a, fix))) => a.fail(e, if fix.is_some() { &[Action::Retry, Action::CopyDetails] } else { &[Action::CopyDetails] }),
                (Ok(msg), None) => notices.info(msg),
                (Err(e), None) => notices.error(e),
            }
            self.fix_failed = activity.and_then(|(a, fix)| Some((a.id(), fix?)));
            changed = true;
        }
        if let Some((id, fix)) = self.fix_failed {
            if notices.take_action(id) == Some(Action::Retry) && !self.busy() {
                self.fix_failed = None;
                run_fix(self, game, notices, fix, ctx);
            }
        }
        if let Some(Ok(Some(status))) = self.running.as_mut().map(Child::try_wait) {
            self.running = None;
            self.game_closed(status.code());
            changed = true;
        }
        if self.game_checked.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.game_checked = Some(Instant::now());
            let seen = setup::game::game_running();
            if self.game_seen && !seen {
                changed = true;
                if self.running.is_none() {
                    self.game_closed(None);
                }
            }
            self.game_seen = seen;
        }
        let stale = self.refreshed.is_none_or(|t| t.elapsed() > REFRESH_EVERY);
        if (changed || stale) && !self.busy() {
            self.refresh(ctx, game);
        }
    }
}

impl Play {
    fn game_closed(&mut self, code: Option<i32>) {
        if self.closed_at.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return;
        }
        self.closed_at = Some(Instant::now());
        self.closed = Some(code);
    }

    /// The game closed since last asked: its exit code if known, and the checklist as text.
    pub(crate) fn take_closed(&mut self) -> Option<(Option<i32>, String)> {
        let code = self.closed.take()?;
        Some((code, self.checks_text()))
    }

    /// The home screen's checklist as text, one check a line.
    pub(crate) fn checks_text(&self) -> String {
        self.checks
            .iter()
            .map(|c| format!("{:?}\t{}\t{}", c.status, c.title, c.detail.replace('\n', " ")))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The servers that answered on this network, when several did: the player
/// picks the one they meant.
fn found_list(play: &mut Play, ui: &mut egui::Ui) {
    if play.found.len() < 2 {
        return;
    }
    ui.add_space(8.0);
    ui.label(theme::muted("Several servers answered on this network. Choose the one you want:"));
    let mut pick = None;
    ui.horizontal_wrapped(|ui| {
        for ip in &play.found {
            if ui.button(ip.to_string()).clicked() {
                pick = Some(*ip);
            }
        }
    });
    if let Some(ip) = pick {
        play.server = ip.to_string();
        play.found.clear();
    }
}

/// Takes the directory's servers and pings when they arrive. In the join
/// form, the best is preselected unless the player typed a server.
fn poll_directory(play: &mut Play) {
    if let Some(found) = play.browsing.poll() {
        match found {
            Ok(crate::network::Browsed {
                servers,
                note,
                rollout,
                network_maintenance,
            }) => {
                if play.server.trim().is_empty() || play.server_picked {
                    if let Some(best) = setup::directory::best(&servers) {
                        play.server = servers[best].0.host.clone();
                        play.server_picked = true;
                    }
                }
                play.directory = Some(servers);
                play.directory_error = note;
                play.rollout = rollout;
                play.network_maintenance = network_maintenance;
            }
            Err(e) => play.directory_error = Some(format!("The server directory couldn't be read: {e}")),
        }
    }
}

/// Pings the directory's servers once, when there's a directory and nothing
/// is being browsed already.
fn auto_browse(play: &mut Play, ctx: &egui::Context) {
    if play.browsed || play.browsing.running() || play.setup.running() {
        return;
    }
    if let Some(url) = crate::app::Prefs::directory() {
        play.browsed = true;
        play.browsed_at = Some(Instant::now());
        play.browsing.start(ctx, move || crate::services::rt().block_on(crate::network::server_directory(&url)));
    }
}

/// Where a screen asks to go next (a card's link to another screen).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Go {
    View(View),
    Settings(crate::settings::Section),
    /// Support, with a message started.
    Support(String),
}

fn go(app: &mut App, to: Option<Go>) {
    match to {
        Some(Go::View(view)) => app.set_view(view),
        Some(Go::Settings(section)) => app.open_settings(section),
        Some(Go::Support(text)) => app.open_support(text),
        None => {}
    }
}

/// The Play screen: the guided setup until there's a server and an account,
/// then the home screen.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut to = None;
    // Chat, for the drawer's Squad tab: the latest in Global, and what's unread.
    let comms = {
        let (chat, _, _) = app.chat_mut();
        Comms {
            latest: chat.latest_global(),
            unread: chat.unread(),
        }
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if app.game.is_none() {
            no_game(app, ui);
            return;
        }
        let (play, game, notices) = app.play_mut();
        let game = game.as_mut().expect("checked above");
        play.poll(&ctx, game, notices);
        // A server that moved its players (global matchmaking): asked once; the launcher follows.
        if !play.move_asked && game.managed.is_none() && play.running.is_none() {
            play.move_asked = true;
            let cfg = (*game.cfg).clone();
            play.moving.start(&ctx, move || Ok(crate::flow::moved(&cfg)));
        }
        if let Some(Ok(Some((from, to)))) = play.moving.poll() {
            if !play.setup.running() && play.running.is_none() {
                notices.info(format!("{from} moved its players to {to}: setting you up there with your name and identity."));
                play.server = to;
                let name = game.cfg.current_profile().map(|p| p.user.username.clone()).filter(|n| !n.is_empty());
                start_setup(play, game, &ctx, notices, name, true);
            }
        }
        let has_server = game.cfg.current_profile().is_some_and(|p| !p.server.is_empty());
        if game.managed.is_none() && (!has_server || play.editing) {
            play.fresh_setup |= !has_server;
            setup_screen(play, game, notices, &ctx, ui);
        } else if play.fresh_setup && options_step(play, game, notices, ui) {
            // The guided setup's last question before Play.
        } else {
            to = home(play, game, notices, &comms, &ctx, ui);
            // A Join click on the Friends card: start the game if it isn't running, then ask the
            // server to send it into the friend's match (JoinNow). Done here, where the game is
            // mutable.
            if let Some((id, name)) = play.join_requested.take() {
                // Only with a game to take the join: running, or started now (not when the
                // launcher held back, for a question first, say).
                let running = play.running.is_some() || play.game_seen || setup::game::game_running();
                if running || launch(play, game, notices, crate::app::PlayMode::Play) {
                    let cfg = game.cfg.clone();
                    let game_dir = game.dir.clone();
                    play.joining.start(&ctx, move || flow::join_friend(&cfg, &game_dir, &id, &name));
                }
            }
        }
    });
    go(app, to);
}

/// The steps of the guided setup, in order.
const STEPS: [&str; 5] = ["Find the game", "Choose a server", "Pick your name", "Game options", "Play"];

/// The guided setup's frame: the steps on the left (`step` is the current
/// one), the step itself on the right.
fn wizard(ui: &mut egui::Ui, step: usize, body: impl FnOnce(&mut egui::Ui)) {
    // On the night-vision glow, as the Play screen's stage.
    let screen = ui.clip_rect();
    theme::grid_backdrop(ui.painter(), screen);
    ui.add_space(28.0);
    let width = (ui.available_width() - 32.0).min(980.0);
    let side = ((ui.available_width() - width) / 2.0).max(16.0);
    let stacked = width < 700.0;
    ui.horizontal(|ui| {
        ui.add_space(side);
        egui::Frame::new()
            .fill(theme::SURFACE)
            .stroke(egui::Stroke::new(1.0, theme::LINE))
            .corner_radius(6)
            .show(ui, |ui| {
                ui.set_width(width);
                let aside = |ui: &mut egui::Ui| {
                    egui::Frame::new()
                        // Side by side, the fill is painted to the card's full height afterwards.
                        .fill(if stacked { theme::SUNKEN } else { egui::Color32::TRANSPARENT })
                        .corner_radius(if stacked {
                            egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }
                        } else {
                            egui::CornerRadius { nw: 6, ne: 0, sw: 6, se: 0 }
                        })
                        .inner_margin(egui::Margin::symmetric(30, 32))
                        .show(ui, |ui| {
                            if stacked {
                                ui.set_width(width - 60.0);
                            } else {
                                ui.set_width(250.0);
                                ui.set_min_height(470.0);
                            }
                            ui.vertical(|ui| {
                                theme::lights(ui, 7.0);
                                ui.add_space(18.0);
                                theme::title(ui, "Operative setup", "Get online in a minute");
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(
                                        "The launcher sets everything up and keeps the game's original files, so you can undo it any time. You need your own copy of the game.",
                                    )
                                    .color(theme::SOFT),
                                );
                                ui.add_space(16.0);
                                for (i, name) in STEPS.iter().enumerate() {
                                    step_row(ui, i, name, step);
                                }
                            });
                        })
                        .response
                        .rect
                };
                let main = |ui: &mut egui::Ui, w: f32| {
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(36, 32)).show(ui, |ui| {
                        ui.set_width(w - 72.0);
                        ui.vertical(|ui| {
                            ui.label(theme::kicker(&format!("Step {} of {}", step + 1, STEPS.len())));
                            ui.add_space(2.0);
                            body(ui);
                        });
                    });
                };
                if stacked {
                    ui.vertical(|ui| {
                        aside(ui);
                        main(ui, width);
                    });
                } else {
                    let fill = ui.painter().add(egui::Shape::Noop);
                    let row = ui.horizontal_top(|ui| {
                        let side = aside(ui);
                        ui.vertical(|ui| main(ui, width - 320.0));
                        side
                    });
                    let side = egui::Rect::from_min_max(row.response.rect.min, egui::pos2(row.inner.right(), row.response.rect.bottom()));
                    ui.painter()
                        .set(fill, egui::Shape::rect_filled(side, egui::CornerRadius { nw: 6, ne: 0, sw: 6, se: 0 }, theme::SUNKEN));
                }
            });
    });
    ui.add_space(28.0);
}

/// One step in the list: done (a tick), current (lit, with the green edge) or to come.
fn step_row(ui: &mut egui::Ui, i: usize, name: &str, current: usize) {
    let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
    let p = ui.painter();
    if i == current {
        p.rect_filled(row.expand2(egui::vec2(10.0, 0.0)), 4, theme::SELECTED);
        p.rect_filled(
            egui::Rect::from_min_size(row.left_top() - egui::vec2(10.0, 0.0), egui::vec2(3.0, row.height())),
            0,
            theme::ACCENT,
        );
    }
    let square = egui::Rect::from_center_size(egui::pos2(row.left() + 13.0, row.center().y), egui::vec2(24.0, 24.0));
    let c = square.center();
    if i < current {
        p.rect_filled(square, 3, theme::SELECTED);
        let s = egui::Stroke::new(2.2, theme::ACCENT);
        p.line_segment([c + egui::vec2(-5.0, 0.5), c + egui::vec2(-1.5, 4.0)], s);
        p.line_segment([c + egui::vec2(-1.5, 4.0), c + egui::vec2(5.5, -3.5)], s);
    } else {
        let color = if i == current { theme::ACCENT } else { theme::CONTROL_LINE };
        p.rect_stroke(square, 3, egui::Stroke::new(1.2, color), egui::StrokeKind::Inside);
        p.text(
            c,
            egui::Align2::CENTER_CENTER,
            (i + 1).to_string(),
            egui::FontId::monospace(12.0),
            if i == current { theme::ACCENT } else { theme::MUTED },
        );
    }
    let color = match i.cmp(&current) {
        std::cmp::Ordering::Equal => theme::FG,
        std::cmp::Ordering::Less => theme::SOFT,
        std::cmp::Ordering::Greater => theme::MUTED,
    };
    let font = if i == current {
        egui::FontId::new(15.0, theme::strong())
    } else {
        egui::FontId::proportional(15.0)
    };
    p.text(egui::pos2(row.left() + 38.0, row.center().y), egui::Align2::LEFT_CENTER, name, font, color);
    ui.add_space(2.0);
}

fn no_game(app: &mut App, ui: &mut egui::Ui) {
    let finding = app.finding_games();
    let mut pick = false;
    let mut look = false;
    wizard(ui, 0, |ui| {
        ui.label(RichText::new("Find Splinter Cell: Blacklist").family(theme::strong()).size(24.0));
        ui.add_space(4.0);
        if finding {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Looking in Steam, Ubisoft Connect and the usual folders…");
            });
            return;
        }
        ui.label(RichText::new("It wasn't in Steam, Ubisoft Connect or the usual folders. Choose the folder you installed it to.").color(theme::SOFT));
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            pick = ui.add(theme::primary("Choose folder…").min_size(egui::vec2(0.0, 46.0))).clicked();
            look = ui.add(theme::secondary("Look again").min_size(egui::vec2(0.0, 46.0))).clicked();
        });
    });
    if pick {
        app.pick_game_folder();
    }
    if look {
        let ctx = ui.ctx().clone();
        app.find_games(&ctx);
    }
}

/// Picks up a network search and the directory's servers when they arrive.
fn poll_lookups(play: &mut Play, notices: &mut Notices) {
    if let Some(found) = play.looking.poll() {
        match found {
            Ok(ips) if ips.len() == 1 => {
                play.server = ips[0].to_string();
                play.address = ips[0].to_string();
                play.server_picked = false;
                play.found = ips;
            }
            Ok(ips) => play.found = ips,
            Err(e) => notices.error(e),
        }
    }
    poll_directory(play);
}

fn find_on_network(play: &mut Play, ctx: &egui::Context) {
    play.looking.start(ctx, crate::connect::find_on_network);
}

fn ping_again(play: &mut Play, ctx: &egui::Context, url: String) {
    play.browsed_at = Some(Instant::now());
    play.browsing.start(ctx, move || crate::services::rt().block_on(crate::network::server_directory(&url)));
}

/// The guided setup's game options step, once there's a server: the new game options with the
/// recommended settings chosen (new_options.rs). False once there's nothing (left) to ask.
fn options_step(play: &mut Play, game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) -> bool {
    if play.setup_options.is_none() {
        let options = crate::new_options::pending(Some(game));
        if options.is_empty() {
            play.fresh_setup = false;
            return false;
        }
        let choices = crate::new_options::offered(&options, &game.dir);
        play.setup_options = Some((options, choices));
    }
    let mut done = false;
    if let Some((options, choices)) = play.setup_options.as_mut() {
        wizard(ui, 3, |ui| {
            ui.label(RichText::new("Game options").family(theme::strong()).size(24.0));
            ui.add_space(4.0);
            crate::new_options::setup_step(ui, options, choices, &game.dir);
            ui.add_space(20.0);
            done = ui.add(theme::primary("Continue").min_size(egui::vec2(170.0, 48.0)).corner_radius(4)).clicked();
        });
    }
    if done {
        if let Some((options, choices)) = play.setup_options.take() {
            crate::new_options::save(game, notices, &options, &choices);
        }
        play.fresh_setup = false;
    }
    true
}

/// The guided setup's server and name steps (the setup itself installs the
/// client, makes or finds the account and pins the adapter).
fn setup_screen(play: &mut Play, game: &mut Game, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    poll_lookups(play, notices);
    // With a directory, its servers are pinged as soon as the form opens, and the best preselected.
    auto_browse(play, ctx);
    let naming = play.needs_name.is_some();
    wizard(ui, if naming { 2 } else { 1 }, |ui| {
        ui.label(theme::display(if naming { "Pick your name" } else { "Choose a server" }, 36.0));
        ui.add_space(4.0);
        if let Some(server) = &play.needs_name {
            ui.label(
                RichText::new(format!(
                    "You don't have an account on {server} yet. Choose the name other players will see; your identity signs you in to it from now on."
                ))
                .color(theme::SOFT),
            );
        } else {
            ui.label(RichText::new("Pick the closest server, or type a server's or a network's address. Connecting also installs the 5th Echelon client, keeping the game's original file so you can undo it.").color(theme::SOFT));
        }
        ui.add_space(14.0);
        egui::Frame::new()
            .fill(theme::SUNKEN)
            .stroke(egui::Stroke::new(1.0, theme::LINE))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::status_marker(ui, Status::Ok);
                    ui.vertical(|ui| {
                        ui.label("Splinter Cell: Blacklist found");
                        ui.label(theme::muted(game.dir.display().to_string()).small().monospace());
                    });
                });
            });
        ui.add_space(14.0);
        ui.add_enabled_ui(!play.setup.running(), |ui| {
            if naming {
                ui.label(theme::caps("Your name"));
                ui.add(
                    egui::TextEdit::singleline(&mut play.nick)
                        .margin(egui::Margin::symmetric(8, 5))
                        .hint_text(theme::hint("what other players see"))
                        .char_limit(setup::account::MAX_NAME)
                        .desired_width(320.0)
                        .min_size(egui::vec2(0.0, 40.0)),
                );
            } else {
                server_choices(play, ui);
                ui.add_space(10.0);
                ui.label(theme::caps("Or a server's address"));
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut play.server)
                                .margin(egui::Margin::symmetric(8, 5))
                                .hint_text(theme::hint("play.example.org or 192.168.1.20"))
                                .desired_width(300.0)
                                .min_size(egui::vec2(0.0, 38.0)),
                        )
                        .changed()
                    {
                        play.server_picked = false;
                        play.needs_name = None;
                    }
                    if play.looking.running() || play.browsing.running() {
                        ui.spinner();
                    } else {
                        if let Some(url) = crate::app::Prefs::directory().filter(|_| ui.add(theme::secondary("Ping again")).clicked()) {
                            ping_again(play, ctx, url);
                        }
                        if ui.add(theme::secondary("Find on my network")).clicked() {
                            find_on_network(play, ctx);
                        }
                    }
                });
                found_list(play, ui);
                if let Some(e) = &play.directory_error {
                    ui.label(theme::muted(e.as_str()).small());
                }
            }
        });
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            let ready = !play.server.trim().is_empty() && (!naming || !play.nick.trim().is_empty());
            if play.setup.running() {
                ui.spinner();
                ui.label(if naming { "Creating your account…" } else { "Connecting…" });
            } else if ui
                .add_enabled(
                    ready,
                    theme::primary(if naming { "Create account" } else { "Connect" })
                        .min_size(egui::vec2(170.0, 48.0))
                        .corner_radius(6),
                )
                .clicked()
            {
                let name = naming.then(|| play.nick.trim().to_string());
                start_setup(play, game, ctx, notices, name, false);
            }
            if game.cfg.current_profile().is_some_and(|p| !p.server.is_empty())
                && !play.setup.running()
                && ui.add(theme::secondary("Cancel").min_size(egui::vec2(0.0, 48.0))).clicked()
            {
                play.editing = false;
                play.needs_name = None;
            }
        });
    });
}

/// The directory's servers as rows to pick from: name, ping, players.
fn server_choices(play: &mut Play, ui: &mut egui::Ui) {
    let Some(servers) = play.directory.as_ref() else {
        if play.browsing.running() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted("Pinging the servers…"));
            });
        }
        return;
    };
    if servers.is_empty() {
        ui.label(theme::muted("The directory lists no servers right now."));
        return;
    }
    let best = setup::directory::best(servers);
    let mut pick = None;
    for i in crate::server_menu::ranked(servers) {
        let (s, ping) = &servers[i];
        let chosen = play.server.trim() == s.host;
        let frame = egui::Frame::new()
            .fill(if chosen { theme::ACCENT.linear_multiply(0.08) } else { theme::SUNKEN })
            .stroke(egui::Stroke::new(if chosen { 2.0 } else { 1.0 }, if chosen { theme::ACCENT } else { theme::LINE }))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(16, 10));
        let inner = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // The name wraps in its own column; the ping and a badge keep the right.
                let left = (ui.available_width() - 90.0).max(160.0);
                ui.allocate_ui_with_layout(egui::vec2(left, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.label(RichText::new(server_name(s)).family(theme::strong()));
                    if let Some(now) = s.playing.as_ref().map(server_api::playing::Playing::summary).filter(|t| !t.is_empty()) {
                        ui.label(RichText::new(now).size(12.5).color(theme::SOFT));
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(theme::muted(format!("{} · {} online", s.host, s.players_online)).small());
                        if best == Some(i) {
                            badge(ui, "Best for you", true);
                        } else if s.friends_mode == "mutual" {
                            badge(ui, "Friends only", false);
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(ping_text(*ping));
                });
            });
        });
        let response = ui
            .interact(inner.response.rect, ui.id().with(("server", i)), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() && !chosen {
            pick = Some(s.host.clone());
        }
        ui.add_space(4.0);
    }
    if let Some(host) = pick {
        play.server = host;
        play.server_picked = false;
    }
}

fn server_name(s: &setup::directory::Listing) -> String {
    if s.region.is_empty() {
        s.name.clone()
    } else {
        format!("{} ({})", s.name, s.region)
    }
}

/// A small label on a server: filled for the one to pick, outlined otherwise.
/// Painted at its own size, so a taller row never stretches it.
pub(crate) fn badge(ui: &mut egui::Ui, text: &str, strong: bool) {
    let color = if strong { theme::ON_ACCENT } else { theme::MUTED };
    let mut job = egui::text::LayoutJob::single_section(
        text.to_uppercase(),
        egui::TextFormat {
            font_id: egui::FontId::proportional(10.5),
            color,
            extra_letter_spacing: 1.0,
            ..Default::default()
        },
    );
    job.wrap.max_rows = 1;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(16.0, 8.0), egui::Sense::hover());
    let p = ui.painter();
    if strong {
        p.rect_filled(rect, 6, theme::ACCENT);
    } else {
        p.rect_stroke(rect, 6, egui::Stroke::new(1.0, theme::CONTROL_LINE), egui::StrokeKind::Inside);
    }
    p.galley(rect.min + egui::vec2(8.0, 4.0), galley, color);
}

/// The home screen: the game's banner with your profile, the bar that starts
/// it, and three cards: status, friends and the server's news. Laid out for
/// the launcher's fixed window.
/// Chat as the Play screen's drawer shows it.
struct Comms {
    latest: Option<(String, String)>,
    unread: u32,
}

fn home(play: &mut Play, game: &mut Game, notices: &mut Notices, comms: &Comms, ctx: &egui::Context, ui: &mut egui::Ui) -> Option<Go> {
    let mut to = None;
    load_art(play, game, ctx);
    load_news(play, game, ctx);
    load_playing(play, game, ctx);
    // The network's servers, for the server's name, ping and players here too.
    poll_directory(play);
    if game.managed.is_none() {
        auto_browse(play, ctx);
    }
    if let Some(managed) = &game.managed {
        egui::Frame::new()
            .fill(theme::ACCENT.linear_multiply(0.12))
            .inner_margin(egui::Margin::symmetric(32, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!(
                    "{} sets up this install. Change the server or account there; you can still play from here.",
                    managed.by
                )));
            });
    }
    if let Some(name) = name_not_allowed(play, game) {
        egui::Frame::new()
            .fill(theme::BAD.linear_multiply(0.14))
            .inner_margin(egui::Margin::symmetric(32, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width() - 250.0, 28.0),
                        egui::Layout::top_down(egui::Align::LEFT).with_main_align(egui::Align::Center),
                        |ui| {
                            ui.label(format!(
                                "Your name {name} breaks this network's rules (no slurs or hate). Choose another to play: \
                                 friends and invites carry on."
                            ));
                        },
                    );
                    if ui.button("Rename…").clicked() {
                        to = Some(Go::Settings(crate::settings::Section::Servers));
                    }
                    // Support is the community network's: elsewhere, the server's own admins.
                    let community = crate::app::Prefs::directory().is_some_and(|d| crate::app::Prefs::is_community(&d));
                    if community
                        && ui
                            .button("Ask for a review")
                            .on_hover_text("Think it's a mistake? The admins check the name and can allow it")
                            .clicked()
                    {
                        to = Some(Go::Support(format!(
                            "Please review my name {name}: I was asked to choose another, and I think that's a mistake. \
                             What it means: "
                        )));
                    }
                });
            });
    }
    if game.uplay_emulator && !game.uplay_emulator_told {
        let dismissed = egui::Frame::new()
            .fill(theme::WARN.linear_multiply(0.12))
            .inner_margin(egui::Margin {
                left: 32,
                right: 24,
                top: 10,
                bottom: 10,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width() - 40.0, 28.0),
                        egui::Layout::top_down(egui::Align::LEFT).with_main_align(egui::Align::Center),
                        |ui| {
                            ui.label(
                                "This game's Ubisoft Connect DLL has been replaced by an emulator. That's up to you and we don't mind, \
                                 but it isn't a setup we support: if something doesn't work with it, we may not be able to help.",
                            );
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        crate::activity::close_button(ui).on_hover_text("Dismiss: don't show this again").clicked()
                    })
                    .inner
                })
                .inner
            })
            .inner;
        if dismissed {
            game.uplay_emulator_told = true;
            crate::app::Prefs::set_uplay_emulator_told();
        }
    }
    let notice = update_notice(play, game, ctx);
    // The stage (the game, the server, Play) and beside it the drawer (status, friends, news).
    // The drawer takes about a third, so the stage keeps room for its controls in a small window.
    let width = ui.available_width();
    let drawer = (width * 0.3).clamp(300.0, 380.0);
    let height = ui.available_height().max(520.0);
    // Too narrow for both side by side: the stage takes the width, the drawer goes under it
    // (the screen scrolls to it).
    if width < 980.0 {
        let spacing = ui.ctx().style().spacing.item_spacing;
        // A little short of the window, so the drawer's tabs show under it.
        let height = (ui.available_height() - 56.0).max(480.0);
        // In a scroll area, or the drawer under the stage can't be reached: at the smallest
        // window (800x560) only its tabs showed, the checks and Fix everything below the edge.
        // Each takes its whole height (the drawer paints to its rectangle without using it up),
        // so the scroll area knows there's more below.
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.allocate_ui_with_layout(egui::vec2(width, height), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.set_min_height(height);
                ui.spacing_mut().item_spacing = spacing;
                stage(play, game, notices, notice.as_ref(), &mut to, ui);
            });
            ui.allocate_ui_with_layout(egui::vec2(width, 440.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.set_min_height(440.0);
                drawer_panel(play, game, notices, comms, ctx, &mut to, ui);
            });
        });
        return finish(play, game, notices, notice, ctx, ui, to);
    }
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let spacing = ui.ctx().style().spacing.item_spacing;
        ui.allocate_ui_with_layout(egui::vec2(width - drawer, height), egui::Layout::top_down(egui::Align::LEFT), |ui| {
            ui.spacing_mut().item_spacing = spacing;
            stage(play, game, notices, notice.as_ref(), &mut to, ui);
        });
        ui.allocate_ui_with_layout(egui::vec2(drawer, height), egui::Layout::top_down(egui::Align::LEFT), |ui| {
            ui.spacing_mut().item_spacing = spacing;
            drawer_panel(play, game, notices, comms, ctx, &mut to, ui);
        });
    });
    finish(play, game, notices, notice, ctx, ui, to)
}

/// After the stage and the drawer: the notice of an update or maintenance, and what it asks for.
fn finish(
    play: &mut Play,
    game: &mut Game,
    notices: &mut Notices,
    notice: Option<crate::update_notice::Notice>,
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    to: Option<Go>,
) -> Option<Go> {
    if let Some(notice) = &notice {
        match crate::update_notice::show(ui, notice) {
            Some(crate::update_notice::Action::UpdateLauncher) => play.update_asked = true,
            Some(crate::update_notice::Action::PlayOn { host, .. }) if !play.busy() => {
                play.server = host;
                play.server_picked = false;
                play.needs_name = None;
                let name = game.cfg.current_profile().map(|p| p.user.username.clone()).filter(|n| !n.is_empty());
                start_setup(play, game, ctx, notices, name, true);
            }
            _ => {}
        }
    }
    to
}

/// The notice under the launch bar, if there's one for this player: a release going out to
/// the network's servers, or maintenance on theirs or on the network (the one that matters
/// most). The directory is read again while it shows (every minute, or 15 s while their
/// server restarts or is down).
fn update_notice(play: &mut Play, game: &Game, ctx: &egui::Context) -> Option<crate::update_notice::Notice> {
    if game.managed.is_some() {
        return None;
    }
    let current = game.cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty())?;
    let servers = play.directory.clone().unwrap_or_default();
    let notice = crate::update_notice::for_player(
        &current,
        &servers,
        play.rollout.as_ref(),
        play.network_maintenance.as_ref(),
        &mut play.my_listing,
        play.newer_launcher.as_deref(),
    );
    if play.rollout.is_some() || notice.is_some() {
        let every = notice.as_ref().map_or(std::time::Duration::from_secs(60), crate::update_notice::Notice::refresh_every);
        let due = play.browsed_at.is_none_or(|t| t.elapsed() >= every);
        if due && !play.browsing.running() && !play.setup.running() {
            if let Some(url) = crate::app::Prefs::directory() {
                ping_again(play, ctx, url);
            }
        }
        ctx.request_repaint_after(every);
    }
    notice
}

/// Starts reading one of the game's loading screens for the banner (a
/// different one each start), and takes it when it's ready.
fn load_art(play: &mut Play, game: &Game, ctx: &egui::Context) {
    if !play.art_started {
        play.art_started = true;
        let dir = game.dir.clone();
        let pick = rand::random::<u32>() as usize;
        play.art.start(ctx, move || setup::key_art::load(&dir, pick).map_err(|e| e.to_string()));
    }
    match play.art.poll() {
        Some(Ok(art)) => {
            let image = egui::ColorImage::from_rgba_unmultiplied([art.width, art.height], &art.rgba);
            play.art_texture = Some(ctx.load_texture("key-art", image, egui::TextureOptions::LINEAR));
        }
        // The grid stays: a game without the package, or another version of it.
        Some(Err(e)) => tracing::info!("No banner art from the game: {e}"),
        None => {}
    }
}

/// How long the server's news is kept before it's asked again.
const NEWS_EVERY: Duration = Duration::from_secs(15 * 60);

/// Fetches the server's news when the server changes, and now and then.
fn load_news(play: &mut Play, game: &Game, ctx: &egui::Context) {
    if let Some(Ok((server, news))) = play.fetching_news.poll() {
        play.news = news;
        play.news_from = Some((server, Instant::now()));
        play.news_index = 0;
    }
    let Some(server) = game.cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty()) else {
        play.news.clear();
        return;
    };
    let fresh = play.news_from.as_ref().is_some_and(|(from, at)| *from == server && at.elapsed() < NEWS_EVERY);
    if !fresh && !play.fetching_news.running() {
        if play.news_from.as_ref().is_some_and(|(from, _)| *from != server) {
            play.news.clear();
        }
        // Marked as fetched now, so a server that doesn't answer isn't asked every frame.
        play.news_from = Some((server.clone(), Instant::now()));
        play.fetching_news.start(ctx, move || {
            let news = setup::server_info::news(&server, Duration::from_secs(5));
            Ok((server, news))
        });
    }
}

/// How often what's being played on the player's server is asked for.
const PLAYING_EVERY: Duration = Duration::from_secs(15);

/// Asks the player's server what's being played there every [`PLAYING_EVERY`], while the
/// home screen shows.
fn load_playing(play: &mut Play, game: &Game, ctx: &egui::Context) {
    if let Some(Ok((server, playing))) = play.fetching_playing.poll() {
        play.playing_now = playing.map(|p| (server, p));
    }
    let Some(server) = game.cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty()) else {
        play.playing_now = None;
        return;
    };
    if play.playing_now.as_ref().is_some_and(|(from, _)| *from != server) {
        play.playing_now = None;
    }
    let fresh = play.playing_from.as_ref().is_some_and(|(from, at)| *from == server && at.elapsed() < PLAYING_EVERY);
    if !fresh && !play.fetching_playing.running() {
        play.playing_from = Some((server.clone(), Instant::now()));
        play.fetching_playing
            .start(ctx, move || Ok((server.clone(), setup::server_info::playing(&server, Duration::from_secs(5)))));
    }
    ctx.request_repaint_after(PLAYING_EVERY);
}

/// The banner: the game's name over one of its loading screens (or the
/// night-vision grid until there is one), and who's playing in the corner.
/// The stage: the game's art under a night-vision glow, the HUD along the top (the product, the
/// link to the server, who's playing), the game's name, the launch controls, and the latest news
/// along the foot.
fn stage(play: &mut Play, game: &mut Game, notices: &mut Notices, notice: Option<&crate::update_notice::Notice>, to: &mut Option<Go>, ui: &mut egui::Ui) {
    let size = egui::vec2(ui.available_width(), ui.available_height());
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    theme::grid_backdrop(ui.painter(), rect);
    if let Some(texture) = &play.art_texture {
        theme::cover_image(ui.painter(), rect, texture);
        theme::scrim(ui.painter(), rect);
        theme::glow(ui.painter(), rect, rect.left_bottom(), theme::BG.gamma_multiply(0.6));
    }
    let inner = rect.shrink2(egui::vec2(36.0, 26.0));
    // The HUD line: the product's label centred on the player's card beside it.
    let mut top = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Min)));
    // The product's name is the title bar's; the stage's corner keeps the server and the player.
    if !crate::window_frame::custom() {
        top.vertical(|ui| {
            ui.add_space(22.0);
            ui.label(theme::caps(&format!("{} //", env!("FE_PRODUCT"))));
        });
    }
    top.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if profile_card(play, game, ui).clicked() {
            *to = Some(Go::Settings(crate::settings::Section::Identity));
        }
    });
    let mut bottom = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::bottom_up(egui::Align::LEFT)));
    news_line(play, to, &mut bottom);
    // A hairline over the news, apart from the controls above it.
    bottom.add_space(12.0);
    let line = bottom.allocate_exact_size(egui::vec2(inner.width(), 1.0), egui::Sense::hover()).0;
    bottom.painter().rect_filled(line, 0, theme::CONTROL_LINE);
    bottom.add_space(22.0);
    // On a wide stage the controls stay together, Play beside the server, not across the screen.
    let controls = inner.width().min(940.0);
    bottom.scope(|ui| {
        ui.set_max_width(controls);
        launch_bar(play, game, notices, notice, to, ui);
    });
    bottom.add_space(20.0);
    bottom.label(
        RichText::new("Co-op and Spies vs Mercs on community servers. No VPN, no port forwarding.")
            .size(16.0)
            .color(theme::SOFT),
    );
    bottom.add_space(6.0);
    // The name in two lines, the second in the accent, as big as the stage allows.
    let big = (inner.width() / 11.0).clamp(46.0, 84.0);
    bottom.label(theme::display("Blacklist", big).color(theme::ACCENT));
    bottom.add_space(-big * 0.32);
    bottom.label(theme::display("Splinter Cell", big));
    bottom.add_space(-big * 0.12);
    bottom.label(theme::kicker("Tom Clancy's"));
}

/// The latest news along the foot of the stage: its headline, and the way to all of it.
fn news_line(play: &Play, to: &mut Option<Go>, ui: &mut egui::Ui) {
    let latest = play.news.first().map(|n| hooks_config::text::clip(&n.title, 90));
    ui.horizontal(|ui| {
        ui.label(theme::kicker("Intel"));
        match &latest {
            Some(title) => {
                ui.add(egui::Label::new(RichText::new(title).color(theme::SOFT)).truncate());
            }
            None => {
                ui.label(theme::muted("No news from this server yet."));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if latest.is_some() && ui.link("All news").clicked() {
                *to = Some(Go::View(View::News));
            }
        });
    });
}

/// The latest in Global, and the way to Chat, under the drawer's friends.
fn comms_box(comms: &Comms, to: &mut Option<Go>, width: f32, ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(theme::SUNKEN)
        .stroke(egui::Stroke::new(1.0, theme::LINE))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(width - 24.0);
            ui.horizontal(|ui| {
                ui.label(theme::caps("Global · latest"));
                if comms.unread > 0 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(theme::kicker(&format!("{} new for you", comms.unread)));
                    });
                }
            });
            match &comms.latest {
                Some((name, body)) => {
                    let text = format!("{name}: {}", hooks_config::text::clip(body, 120));
                    ui.add(egui::Label::new(RichText::new(text).color(theme::SOFT)).truncate());
                }
                None => {
                    ui.label(theme::muted("Nothing said yet."));
                }
            }
            if ui.add(theme::secondary("Open Chat").min_size(egui::vec2(ui.available_width(), 34.0))).clicked() {
                *to = Some(Go::View(View::Chat));
            }
        });
}

#[cfg(all(test, feature = "shots"))]
impl Play {
    /// The screenshots' states: a drawer tab, or the setup's name step.
    pub(crate) fn shot_drawer(&mut self, drawer: Drawer) {
        self.drawer = Some(drawer);
    }

    pub(crate) fn shot_name_step(&mut self, server: &str) {
        self.needs_name = Some(server.to_string());
        self.editing = true;
    }

    /// The Matches screen with example matches (not read again while the shot is taken).
    pub(crate) fn shot_matches(&mut self) {
        self.matches.data = Some(crate::matches::sample());
        self.matches.loaded_at = Some(Instant::now());
        self.matches.picked = None;
    }

    /// The Friends screen with example friends and requests.
    pub(crate) fn shot_friends(&mut self, tab: PeopleTab) {
        let person = |id: &str, name: &str, relation: &'static str, online: bool, activity: Option<&str>| flow::Person {
            id: id.into(),
            name: name.into(),
            online,
            relation,
            activity: activity.map(String::from),
            ..flow::Person::default()
        };
        self.people.shown = Some(Ok(flow::People {
            friends: vec![
                person("1", "Raven", "friend", true, Some("SvM Classic · Penthouse")),
                person("2", "Tank", "friend", true, None),
                person("3", "Nova", "friend", false, None),
            ],
            received: vec![person("4", "Kiwi", "received", true, None)],
            sent: vec![person("5", "Ghost", "sent", false, None)],
            blocked: vec![],
            ..flow::People::default()
        }));
        self.people.open = true;
        self.people.tab = tab;
    }
}

/// What the drawer beside the stage shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Drawer {
    Status,
    Squad,
}

/// The drawer: the checks and friends, one at a time under tabs (the news is along the stage's foot
/// and on the Intel screen). Until the player picks one
/// it shows the checks when something needs fixing, and friends otherwise.
fn drawer_panel(play: &mut Play, game: &mut Game, notices: &mut Notices, comms: &Comms, ctx: &egui::Context, to: &mut Option<Go>, ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0, theme::SURFACE);
    ui.painter().line_segment([rect.left_top(), rect.left_bottom()], egui::Stroke::new(1.0, theme::LINE));
    let (color, _) = readiness(play);
    let trouble = color == theme::BAD;
    let shown = play.drawer.unwrap_or(if trouble { Drawer::Status } else { Drawer::Squad });
    let friends_online = play.friends.online.len();
    let tabs = [
        (Drawer::Status, if trouble { "Status · fix".to_string() } else { "Status".to_string() }),
        (
            Drawer::Squad,
            if friends_online > 0 {
                format!("Squad · {friends_online}")
            } else {
                "Squad".to_string()
            },
        ),
    ];
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(0.0, 0.0)))
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );
    let ui = &mut inner;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        let w = rect.width() / tabs.len() as f32;
        for (tab, label) in &tabs {
            let (r, response) = ui.allocate_exact_size(egui::vec2(w, 52.0), egui::Sense::click());
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            let on = shown == *tab;
            let painter = ui.painter();
            painter.line_segment([r.left_bottom(), r.right_bottom()], egui::Stroke::new(1.0, theme::LINE));
            if on {
                painter.line_segment(
                    [r.left_bottom() - egui::vec2(0.0, 1.0), r.right_bottom() - egui::vec2(0.0, 1.0)],
                    egui::Stroke::new(2.0, theme::ACCENT),
                );
            }
            let color = if on {
                theme::FG
            } else if response.hovered() {
                theme::SOFT
            } else {
                theme::MUTED
            };
            painter.text(r.center(), egui::Align2::CENTER_CENTER, label.to_uppercase(), egui::FontId::monospace(11.5), color);
            response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
            if response.clicked() {
                play.drawer = Some(*tab);
            }
        }
    });
    let width = rect.width() - 24.0;
    egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 12)).show(ui, |ui| match shown {
        Drawer::Status => status_card(play, game, notices, ctx, to, width, ui),
        // Friends, and under them the latest in Global, on the community network.
        Drawer::Squad => {
            let community = crate::app::Prefs::directory().is_some_and(|d| crate::app::Prefs::is_community(&d));
            if community {
                // Friends take what the box under them leaves.
                let friends = (ui.available_height() - 140.0).max(200.0);
                ui.allocate_ui_with_layout(egui::vec2(width, friends), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    friends_card(play, game, to, width, ui)
                });
                ui.add_space(10.0);
                comms_box(comms, to, width, ui);
            } else {
                friends_card(play, game, to, width, ui);
            }
        }
    });
}

/// Who's playing: name, identity and server, on the banner's corner. Opens
/// Settings › Identity and friends.
fn profile_card(play: &mut Play, game: &Game, ui: &mut egui::Ui) -> egui::Response {
    let profile = game.cfg.current_profile().cloned().unwrap_or_default();
    let identity = play.identity_short().unwrap_or_default();
    let name = if profile.user.username.is_empty() {
        String::from("No account yet")
    } else {
        hooks_config::text::clip(&profile.user.username, 24)
    };
    let online = play.checks.iter().any(|c| c.id == "account" && c.status == Status::Ok);
    // Painted at a fixed size, so it sits in the corner whatever the layout around it.
    let (rect, response) = ui.allocate_exact_size(egui::vec2(236.0, 60.0), egui::Sense::click());
    let p = ui.painter();
    p.rect(
        rect,
        14,
        theme::BG.gamma_multiply(0.82),
        egui::Stroke::new(1.0, if response.hovered() { theme::CONTROL_LINE } else { theme::LINE }),
        egui::StrokeKind::Inside,
    );
    let avatar = egui::pos2(rect.left() + 32.0, rect.center().y);
    p.circle_filled(avatar, 20.0, theme::CONTROL);
    p.circle_stroke(avatar, 20.0, egui::Stroke::new(1.5, theme::ACCENT.gamma_multiply(0.6)));
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    p.text(avatar, egui::Align2::CENTER_CENTER, initial, egui::FontId::new(18.0, theme::condensed()), theme::ACCENT);
    // The presence dot on the avatar.
    let dot = avatar + egui::vec2(14.0, 14.0);
    p.circle_filled(dot, 6.0, theme::BG);
    p.circle_filled(dot, 4.0, if online { theme::OK } else { theme::MUTED });
    let x = rect.left() + 62.0;
    let width = rect.right() - x - 12.0;
    let line = |text: String, font: egui::FontId, color: egui::Color32| {
        let mut job = egui::text::LayoutJob::single_section(text, egui::TextFormat::simple(font, color));
        job.wrap = egui::text::TextWrapping::truncate_at_width(width);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let title = line(name, egui::FontId::new(15.5, theme::strong()), theme::FG);
    let sub = if identity.is_empty() {
        String::from("No identity yet")
    } else {
        format!("ID {identity}")
    };
    let sub = line(sub, egui::FontId::monospace(11.5), theme::MUTED);
    p.galley(egui::pos2(x, rect.center().y - title.size().y), title, theme::FG);
    p.galley(egui::pos2(x, rect.center().y + 2.0), sub, theme::MUTED);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Identity and friends")
}

/// How ready everything is, for the launch bar.
fn readiness(play: &Play) -> (egui::Color32, String) {
    let running = play.running.is_some() || play.game_seen;
    let (tone, line) = crate::checks::readiness(&play.checks, play.support.as_ref(), running);
    let color = match tone {
        None => theme::MUTED,
        Some(Status::Ok) => theme::OK,
        Some(Status::Warn) => theme::WARN,
        Some(Status::Fail) => theme::BAD,
    };
    (color, line)
}

/// The bar under the banner: the server you're on, how ready you are, and Play.
fn launch_bar(play: &mut Play, game: &mut Game, notices: &mut Notices, notice: Option<&crate::update_notice::Notice>, to: &mut Option<Go>, ui: &mut egui::Ui) {
    let profile = game.cfg.current_profile().cloned().unwrap_or_default();
    let listing = play.directory.as_ref().and_then(|d| d.iter().find(|(s, _)| s.host == profile.server)).cloned();
    let playing = play.game_seen || play.running.is_some();
    let menu_id = egui::Id::new("server-menu");
    let menu_open = egui::Popup::is_id_open(ui.ctx(), menu_id);
    let run_by = crate::connect::run_by(listing.is_some(), play.directory.is_some());
    let mut caption_rect = None;
    egui::Frame::new().show(ui, |ui| {
        ui.set_width(ui.available_width());
        // The server card gives way first when the stage is narrow, and the readiness line goes
        // under the controls.
        let chip_width = (ui.available_width() * 0.4).clamp(220.0, 300.0);
        let narrow = ui.available_width() < 700.0;
        let status = match notice {
            Some(n) if n.blocks_play.is_none() => (theme::WARN, n.status.clone()),
            Some(n) => (theme::BAD, n.status.clone()),
            None => readiness(play),
        };
        let readiness_line = |ui: &mut egui::Ui| {
            let (color, text) = &status;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 5.0, *color);
            ui.add(egui::Label::new(RichText::new(text).size(15.0)).truncate()).on_hover_text(text);
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            // The server: click for the server menu.
            let chip = ui.allocate_ui_with_layout(egui::vec2(chip_width, 60.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                egui::Frame::new()
                    .fill(theme::SURFACE.gamma_multiply(0.85))
                    .stroke(egui::Stroke::new(1.0, if menu_open { theme::ACCENT } else { theme::CONTROL_LINE }))
                    .corner_radius(4)
                    .inner_margin(egui::Margin {
                        left: 16,
                        right: 10,
                        top: 10,
                        bottom: 10,
                    })
                    .show(ui, |ui| {
                        ui.set_width(chip_width - 26.0);
                        // The ping and players on the right, as wide as they are ("no answer" is
                        // wider than "53 ms"): the server's caption and name get the rest.
                        let right = listing
                            .as_ref()
                            .map(|(s, ping)| (ping_text(*ping), theme::muted(format!("{} online", s.players_online)).small()));
                        let right_width = right.as_ref().map_or(0.0, |(a, b)| {
                            let width = |t: &RichText| {
                                egui::WidgetText::from(t.clone())
                                    .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body)
                                    .size()
                                    .x
                            };
                            width(a).max(width(b))
                        });
                        let spacing = ui.spacing().item_spacing.x;
                        let left_width = (ui.available_width() - right_width - 18.0 - 3.0 * spacing - 6.0).max(60.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                // A long network name is cut short ("…"), not given the whole card.
                                ui.set_max_width(left_width);
                                ui.spacing_mut().item_spacing.y = 2.0;
                                // Who runs it, when it isn't the community network: hovered, says so.
                                let caption = match &run_by {
                                    Some((who, _)) => format!("Server · {who}"),
                                    None => "Server".to_string(),
                                };
                                caption_rect = Some(ui.add(egui::Label::new(theme::caps(&caption)).truncate()).rect);
                                // Its region is enough here ("Sydney, Australia").
                                let name = listing.as_ref().map(|(s, _)| place(s)).unwrap_or_else(|| profile.server.clone());
                                ui.add(egui::Label::new(RichText::new(hooks_config::text::clip(&name, 24)).family(theme::strong())).truncate());
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                chevron_down(ui, menu_open);
                                ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    if let Some((ping, online)) = right {
                                        // Never wrapped: squeezed, they'd stand a letter to a line.
                                        ui.add(egui::Label::new(ping).wrap_mode(egui::TextWrapMode::Extend));
                                        ui.add(egui::Label::new(online).wrap_mode(egui::TextWrapMode::Extend));
                                    }
                                });
                            });
                        });
                    });
            });
            // Shown, but closed, while the game runs or a task is busy.
            let blocked = if playing {
                Some("Quit the game first")
            } else if play.busy() {
                Some("Wait for the current task to finish")
            } else if game.managed.is_some() {
                Some("Another tool manages this install: change the server there")
            } else {
                None
            };
            let chip = ui.interact(
                chip.response.rect,
                ui.id().with("server-chip"),
                if blocked.is_none() { egui::Sense::click() } else { egui::Sense::hover() },
            );
            let chip = match blocked {
                Some(why) => chip.on_hover_text(why),
                None => {
                    let chip = chip.on_hover_cursor(egui::CursorIcon::PointingHand);
                    // The caption's line, over the caption only (the card itself opens the menu).
                    let over_caption = caption_rect.is_some_and(|r| chip.hover_pos().is_some_and(|p| r.contains(p)));
                    match &run_by {
                        Some((_, why)) if over_caption => chip.on_hover_text(why.as_str()),
                        _ => chip,
                    }
                }
            };
            let popup = egui::Popup::from_toggle_button_response(&chip)
                .id(menu_id)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .align(egui::RectAlign::BOTTOM_START)
                .gap(6.0)
                .width(440.0)
                .frame(
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .stroke(egui::Stroke::new(1.0, theme::CONTROL_LINE))
                        .corner_radius(6)
                        .inner_margin(8)
                        .shadow(egui::Shadow {
                            offset: [0, 18],
                            blur: 50,
                            spread: 0,
                            color: egui::Color32::from_black_alpha(140),
                        }),
                );
            let network = crate::app::Prefs::directory();
            let header = crate::connect::menu_header(network.as_deref());
            let clock = setup::clock::Clock::local();
            let rows = play
                .directory
                .as_ref()
                .map(|d| crate::server_menu::rows(d, &profile.server, identity::now(), &clock))
                .unwrap_or_default();
            let pinging = play.browsing.running();
            let note = play.directory_error.clone();
            let mut focus = play.menu_focus;
            if let Some(picked) = popup
                .show(|ui| crate::server_menu::show(ui, &header, &rows, pinging, note.as_deref(), &mut focus))
                .and_then(|r| r.inner)
            {
                egui::Popup::close_id(ui.ctx(), menu_id);
                match picked {
                    crate::server_menu::Picked::Server(host) if blocked.is_none() => {
                        // A community server: switched to at once, in the activity bar.
                        play.server = host;
                        play.server_picked = false;
                        play.needs_name = None;
                        let name = Some(profile.user.username.clone()).filter(|n| !n.is_empty());
                        start_setup(play, game, ui.ctx(), notices, name, true);
                    }
                    crate::server_menu::Picked::Server(_) => {}
                    crate::server_menu::Picked::PingAgain => {
                        if let Some(url) = network {
                            ping_again(play, ui.ctx(), url);
                        }
                    }
                    crate::server_menu::Picked::ServersScreen => *to = Some(Go::View(View::Servers)),
                }
            }
            play.menu_focus = focus;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                play_button(play, game, notices, notice.and_then(|n| n.blocks_play.as_deref()), ui);
                // A release going out, or maintenance, says what it means for playing now.
                if !narrow {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), readiness_line);
                }
            });
        });
        if narrow {
            ui.add_space(8.0);
            ui.horizontal(readiness_line);
        }
        // Beta features on (Settings › Game): said where the game is started, with what it
        // means, so nobody forgets they're trying one out.
        let beta = game.cfg.hook_config.beta.on();
        if !beta.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "Beta features on ({}): experimental, and your game's log is sent to the server's admins after each game. Settings › Game to turn off.",
                    beta.len()
                ))
                .color(theme::WARN)
                .size(13.0),
            );
        }
    });
}

/// The chevron on the server card: down, or up while the menu is open.
fn chevron_down(ui: &mut egui::Ui, open: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    let c = rect.center();
    let d = if open { -1.0 } else { 1.0 };
    let s = egui::Stroke::new(2.0, theme::MUTED);
    ui.painter().line_segment([c + egui::vec2(-5.0, -2.5 * d), c + egui::vec2(0.0, 2.5 * d)], s);
    ui.painter().line_segment([c + egui::vec2(0.0, 2.5 * d), c + egui::vec2(5.0, -2.5 * d)], s);
}

/// A card of the home screen's row: fixed size, a small uppercase title with
/// an optional note on the right, the body, and a footer kept at the bottom.
fn home_card(ui: &mut egui::Ui, width: f32, title: &str, note: Option<RichText>, body: impl FnOnce(&mut egui::Ui), footer: impl FnOnce(&mut egui::Ui)) {
    // As tall as the room it's given (the drawer's), at least as tall as its content wants.
    let height = ui.available_height().max(260.0);
    ui.allocate_ui_with_layout(egui::vec2(width, height), egui::Layout::top_down(egui::Align::LEFT), |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.set_height(height - 12.0);
            // The drawer's tab names the card; its note (a count, "2 of 5") stays.
            let _ = title;
            if let Some(note) = note {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.label(note.size(12.5));
                });
            }
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                footer(ui);
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    egui::ScrollArea::vertical().id_salt(title).auto_shrink([false, false]).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        body(ui);
                    });
                });
            });
        });
    });
}

/// The checks: anything that needs fixing, with its fix; when all is well,
/// a short summary that opens into the full list.
fn status_card(play: &mut Play, game: &mut Game, notices: &mut Notices, ctx: &egui::Context, to: &mut Option<Go>, width: f32, ui: &mut egui::Ui) {
    let fixable = crate::checks::fixable(&play.checks);
    let has_server = game.cfg.current_profile().is_some_and(|p| !p.server.is_empty());
    let checks: Vec<Check> = crate::checks::shown(&play.checks, play.show_all_checks).into_iter().cloned().collect();
    let mut fix = None;
    let mut fix_all = false;
    let mut toggle = false;
    let mut again = false;
    let busy = play.busy();
    let checking = play.facts.running() || busy;
    let managed = game.managed.is_some();
    let show_all = play.show_all_checks;
    home_card(
        ui,
        width,
        "Status",
        None,
        |ui| {
            if checks.is_empty() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(theme::muted("Checking…"));
                });
            }
            for check in &checks {
                ui.horizontal_top(|ui| {
                    theme::status_marker(ui, check.status);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.label(&check.title);
                        if !check.detail.is_empty() && check.status != Status::Ok {
                            ui.label(theme::muted(check.detail.as_str()).small());
                        }
                        // Under the problem, so the narrow card never has to fit it beside it.
                        if let Some(f) = check.fix.filter(|_| check.status != Status::Ok && !managed) {
                            if ui.horizontal(|ui| ui.add_enabled(!busy, theme::secondary(crate::checks::fix_label(f)))).inner.clicked() {
                                fix = Some(f);
                            }
                        }
                    });
                });
                ui.add_space(2.0);
            }
            support_row(play, game, notices, ctx, ui);
        },
        |ui| {
            ui.horizontal_wrapped(|ui| {
                // Each link whole: the row wraps between them, not inside one ("Connection" /
                // "test" across two lines, the second at the window's edge, 1150 wide).
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                if ui.link(if show_all { "Show less" } else { "All checks" }).clicked() {
                    toggle = true;
                }
                ui.label(theme::muted("·"));
                if checking {
                    ui.spinner();
                } else if ui.link("Recheck").on_hover_text("Run every check again").clicked() {
                    again = true;
                }
                ui.label(theme::muted("·"));
                if ui.link("Connection test").clicked() {
                    *to = Some(Go::Settings(crate::settings::Section::Network));
                }
            });
            if fixable && has_server && !managed && !busy {
                fix_all = ui
                    .horizontal(|ui| ui.add(theme::primary("Fix everything").min_size(egui::vec2(0.0, 36.0)).corner_radius(4)))
                    .inner
                    .clicked();
            }
        },
    );
    if let Some(f) = fix {
        run_fix(play, game, notices, f, ctx);
    }
    if toggle {
        play.show_all_checks = !play.show_all_checks;
    }
    if again {
        // Asked for: signs in again, even after a refusal that otherwise holds.
        crate::flow::forget_account_check();
        play.refresh(ctx, game);
    }
    if fix_all {
        let profile = game.cfg.current_profile().cloned().unwrap_or_default();
        play.server = profile.server;
        let name = Some(profile.user.username).filter(|n| !n.is_empty());
        start_setup(play, game, ctx, notices, name, false);
    }
}

/// Friends online here and what they're playing, then friends on other
/// servers with a way to join them.
fn friends_card(play: &mut Play, game: &Game, to: &mut Option<Go>, width: f32, ui: &mut egui::Ui) {
    let playing = play.game_seen || play.running.is_some();
    let friends = play.friends.clone();
    let mut switch_to = None;
    let mut join_click: Option<(String, String)> = None;
    let mut open_people: Option<PeopleTab> = None;
    let note = (!friends.online.is_empty()).then(|| RichText::new(format!("{} online", friends.online.len())).color(theme::OK));
    home_card(
        ui,
        width,
        "Friends",
        note,
        |ui| {
            if game.managed.is_some() || (friends.online.is_empty() && friends.elsewhere.is_empty()) {
                let text = if friends.offline > 0 {
                    "None of your friends are online right now.".to_string()
                } else {
                    format!("No friends yet. Add some in the game: press {} for the overlay.", game.cfg.hook_config.overlay_key)
                };
                ui.label(RichText::new(text).color(theme::SOFT));
            }
            for friend in &friends.online {
                ui.horizontal(|ui| {
                    friend_row(
                        ui,
                        theme::OK,
                        &friend.name,
                        friend.activity.as_deref().unwrap_or("Online"),
                        if friends.join_offered { 70.0 } else { 0.0 },
                    );
                    // Join their game, on a server that offers it: the server sends our game into
                    // the party or public match they're in, hosting it or not (the game is started
                    // first if it isn't running). A private match still needs an invite.
                    if friends.join_offered {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let can = friend.joinable && !play.setup.running() && !play.joining.running();
                            let button = ui.add_enabled(can, theme::secondary("Join"));
                            let button = if friend.joinable {
                                button.on_hover_text("Join this friend's game")
                            } else if friend.activity.is_some() {
                                button.on_disabled_hover_text(format!(
                                    "{}'s match can't be joined now: it's a private match (you need an invitation), or it has started",
                                    friend.name
                                ))
                            } else {
                                button.on_disabled_hover_text(format!("{} isn't in a game right now", friend.name))
                            };
                            if button.clicked() {
                                join_click = Some((friend.id.clone(), friend.name.clone()));
                            }
                        });
                    }
                });
            }
            for friend in &friends.elsewhere {
                let server = if friend.region.is_empty() { friend.server.clone() } else { friend.region.clone() };
                ui.horizontal(|ui| {
                    friend_row(ui, theme::WARN, &friend.username, &format!("On {server}"), 70.0);
                    // Only a server on the internet: one server can't send players into their own network.
                    let public = setup::directory::listable_host(&friend.host);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let button = ui.add_enabled(public && !playing && !play.setup.running(), theme::secondary("Join"));
                        let button = if playing {
                            button.on_disabled_hover_text("Quit the game first")
                        } else {
                            button.on_hover_text(friend.host.as_str())
                        };
                        if button.clicked() {
                            switch_to = Some(friend.host.clone());
                        }
                    });
                });
            }
            online_now(play, game, &friends.others, playing, ui);
            if let Some((_, now)) = &play.playing_now {
                playing_now(ui, now);
            }
        },
        // Bottom up: the hint last, any requests above it.
        |ui| {
            let offline = if friends.offline > 0 {
                format!("{} offline · ", friends.offline)
            } else {
                String::new()
            };
            ui.horizontal(|ui| {
                ui.label(theme::muted(format!("{offline}In game: {}", game.cfg.hook_config.overlay_key)).small());
                if game.managed.is_none() && ui.link("Manage friends").clicked() {
                    open_people = Some(PeopleTab::Friends);
                }
            });
            if friends.requests > 0 {
                let s = if friends.requests == 1 { "" } else { "s" };
                if ui
                    .link(RichText::new(format!("{} friend request{s} waiting", friends.requests)).color(theme::ACCENT).size(13.0))
                    .clicked()
                {
                    open_people = Some(PeopleTab::Requests);
                }
            }
        },
    );
    if let Some(host) = switch_to {
        // Confirmed on the Servers screen, which says where it goes and what happens there.
        play.switching = Some(Switch { host, new_name: None });
        *to = Some(Go::View(View::Servers));
    }
    if let Some(friend) = join_click {
        // Acted on in `show`, where the game can be started if it isn't running.
        play.join_requested = Some(friend);
    }
    if let Some(tab) = open_people {
        play.people.tab = tab;
        *to = Some(Go::View(View::Friends));
    }
}

/// The Friends screen's sections, as the Tauri launcher's Friends screen has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PeopleTab {
    #[default]
    Friends,
    Requests,
    Find,
    Blocked,
}

/// How often the Friends screen asks for the lists again while it's open.
const PEOPLE_EVERY: Duration = Duration::from_secs(15);

/// The Friends screen (the 0.5.0 plan's P1, a screen of the side menu as in the Tauri launcher):
/// friends, requests both ways, finding players and who's blocked, with the Tauri launcher's words.
#[derive(Default)]
struct PeopleWindow {
    open: bool,
    tab: PeopleTab,
    lists: Slot<Result<flow::People, String>>,
    /// When the lists were last asked for: again every [`PEOPLE_EVERY`] while the screen is open.
    lists_at: Option<Instant>,
    shown: Option<Result<flow::People, String>>,
    query: String,
    searching: Slot<Result<Vec<flow::Person>, String>>,
    found: Option<Result<Vec<flow::Person>, String>>,
    /// Remove or Block, waiting for the player to say yes.
    asking: Option<(crate::network::PlayerAction, flow::Person)>,
    /// A friend's server picked to switch to: the Servers screen confirms it.
    go_servers: bool,
}

/// A player's card on the Friends screen: their initial (a dot when online), name and line, and
/// `buttons` on the right.
fn person_card(ui: &mut egui::Ui, name: &str, online: bool, line: &str, buttons: &mut dyn FnMut(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::LINE))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                let c = rect.center();
                ui.painter()
                    .circle(c, 16.0, theme::SUNKEN, egui::Stroke::new(1.0, if online { theme::ACCENT } else { theme::CONTROL_LINE }));
                let initial = name.chars().next().map_or_else(|| "?".to_string(), |c| c.to_uppercase().to_string());
                ui.painter().text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    initial,
                    egui::FontId::new(14.0, theme::strong()),
                    if online { theme::ACCENT } else { theme::MUTED },
                );
                if online {
                    let dot = c + egui::vec2(11.0, 11.0);
                    ui.painter().circle_filled(dot, 5.0, theme::SURFACE);
                    ui.painter().circle_filled(dot, 3.5, theme::OK);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(name).family(theme::strong()).color(if online { theme::FG } else { theme::SOFT }));
                    ui.add(egui::Label::new(theme::muted(line).small()).wrap());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| buttons(ui));
            });
        });
    ui.add_space(2.0);
}

/// The Friends screen of the side menu.
pub fn show_friends(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    theme::page().show(ui, |ui| {
        let (play, game, notices) = app.play_mut();
        // As wide as the Tauri launcher's cards.
        ui.set_max_width(760.0);
        theme::title(ui, "Squad", "Friends");
        let Some(game) = game.as_mut() else {
            ui.label(theme::muted("Find the game first (Play)."));
            return;
        };
        if game.managed.is_some() {
            ui.label(theme::muted("This game is set up by another tool: its friends are in the game's overlay."));
            return;
        }
        play.poll(&ctx, game, notices);
        // The lists afresh each time the screen opens, after every change (`poll`), and every
        // PEOPLE_EVERY while it's open: a request accepted on the other side showed only after
        // leaving the screen and coming back (the gamebox, 2026-11-06).
        let due = play.people.lists_at.is_none_or(|t| t.elapsed() >= PEOPLE_EVERY);
        if (!play.people.open || due) && !play.people.lists.running() {
            let cfg = (*game.cfg).clone();
            play.people.lists.start(&ctx, move || flow::people(&cfg));
            play.people.lists_at = Some(Instant::now());
        }
        ctx.request_repaint_after(PEOPLE_EVERY);
        play.people.open = true;
        ui.add_space(6.0);
        people_page(play, game, ui);
        // A friend's Join: the game started if it isn't running, then sent to their match.
        if let Some((id, name)) = play.join_requested.take() {
            let running = play.running.is_some() || play.game_seen || setup::game::game_running();
            if running || launch(play, game, notices, crate::app::PlayMode::Play) {
                let cfg = game.cfg.clone();
                let game_dir = game.dir.clone();
                play.joining.start(&ctx, move || flow::join_friend(&cfg, &game_dir, &id, &name));
            }
        }
    });
    if std::mem::take(&mut app.play_mut().0.people.go_servers) {
        app.set_view(View::Servers);
    }
}

impl Play {
    /// Friend requests waiting for the player's answer (the side menu's dot).
    pub fn friend_requests(&self) -> usize {
        self.friends.requests
    }

    /// The Friends screen left: its lists are fetched afresh when it opens again.
    pub fn left_friends(&mut self) {
        self.people.open = false;
        self.people.asking = None;
    }
}

/// What a block does, said wherever one is offered.
const BLOCK_MEANS: &str = "Neither of you can message, invite or friend the other, you don't see each other online, and quick match leaves out matches they're in.";

fn people_page(play: &mut Play, game: &Game, ui: &mut egui::Ui) {
    use crate::network::PlayerAction;
    let ctx = ui.ctx().clone();
    let busy = play.acting.running();
    let mut act: Option<(PlayerAction, flow::Person)> = None;
    let mut ask: Option<(PlayerAction, flow::Person)> = None;
    let mut search = false;
    let mut join: Option<(String, String)> = None;
    let mut switch: Option<String> = None;
    {
        let w = &mut play.people;
        let count = |n: usize| if n > 0 { format!(" · {n}") } else { String::new() };
        let lists = w.shown.as_ref().and_then(|r| r.as_ref().ok());
        let (friends_n, requests_n, blocked_n) = lists.map_or((0, 0, 0), |p| (p.friends.len(), p.received.len() + p.sent.len(), p.blocked.len()));
        let online_n = lists.map_or(0, |p| p.friends.iter().filter(|f| f.online).count());
        ui.horizontal(|ui| {
            for (tab, label) in [
                (
                    PeopleTab::Friends,
                    if friends_n > 0 {
                        format!("Friends · {online_n}/{friends_n}")
                    } else {
                        "Friends".to_string()
                    },
                ),
                (PeopleTab::Requests, format!("Requests{}", count(requests_n))),
                (PeopleTab::Find, "Find players".to_string()),
                (PeopleTab::Blocked, format!("Blocked{}", count(blocked_n))),
            ] {
                ui.selectable_value(&mut w.tab, tab, label);
            }
        });
        ui.separator();
        if let Some((action, p)) = w.asking.clone() {
            let (title, means, button) = match action {
                PlayerAction::Block => (format!("Block {}?", p.name), BLOCK_MEANS.to_string(), "Block"),
                _ => (
                    format!("Remove {} from your friends?", p.name),
                    "You can ask them to be friends again later.".to_string(),
                    "Remove",
                ),
            };
            ui.label(RichText::new(title).family(theme::strong()));
            ui.label(theme::muted(means));
            ui.horizontal(|ui| {
                // In red, as the overlay's: the yes to a removal or a block.
                if ui.add_enabled(!busy, theme::danger(button)).clicked() {
                    act = Some((action, p.clone()));
                    w.asking = None;
                }
                if ui.add(theme::secondary("Cancel")).clicked() {
                    w.asking = None;
                }
            });
            ui.separator();
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // A card per player, as the Tauri launcher's: their initial with a dot when online,
            // what they're doing, and the actions on the right.
            let row = |ui: &mut egui::Ui, p: &flow::Person, buttons: &mut dyn FnMut(&mut egui::Ui)| {
                let mut line = if p.online {
                    p.activity.clone().unwrap_or_else(|| "Online".into())
                } else {
                    "Offline".into()
                };
                if let Some(id) = p.identity.as_deref().filter(|_| p.online) {
                    line = format!("{line} · {id}");
                }
                person_card(ui, &p.name, p.online, &line, buttons);
            };
            if w.tab == PeopleTab::Find {
                ui.horizontal(|ui| {
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut w.query)
                            .margin(egui::Margin::symmetric(8, 5))
                            .hint_text(theme::hint("A player's name"))
                            .desired_width(260.0)
                            // As tall as the Search button beside it.
                            .min_size(egui::vec2(0.0, 38.0)),
                    );
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.add_enabled(!w.searching.running(), theme::secondary("Search")).clicked() || enter) && !w.searching.running() {
                        search = true;
                    }
                });
                ui.add_space(6.0);
                match &w.found {
                    None if w.searching.running() => {
                        ui.spinner();
                    }
                    None => {
                        ui.label(theme::muted("Search by name, or leave it empty for who's online."));
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(theme::BAD));
                    }
                    Some(Ok(found)) if found.is_empty() => {
                        ui.label(theme::muted("Nobody by that name."));
                    }
                    Some(Ok(found)) => {
                        for p in found {
                            row(ui, p, &mut |ui| {
                                if ui.add_enabled(!busy, theme::secondary("Block")).clicked() {
                                    ask = Some((PlayerAction::Block, p.clone()));
                                }
                                let (label, action, enabled) = match p.relation {
                                    "friend" => ("Friends", PlayerAction::Request, false),
                                    "sent" => ("Asked", PlayerAction::Request, false),
                                    "received" => ("Accept", PlayerAction::Accept, true),
                                    "blocked" => ("Unblock", PlayerAction::Unblock, true),
                                    _ => ("Add friend", PlayerAction::Request, true),
                                };
                                if ui.add_enabled(enabled && !busy, theme::secondary(label)).clicked() {
                                    act = Some((action, p.clone()));
                                }
                            });
                        }
                    }
                }
                return;
            }
            let people = match &w.shown {
                None => {
                    ui.spinner();
                    return;
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::BAD));
                    return;
                }
                Some(Ok(p)) => p.clone(),
            };
            match w.tab {
                PeopleTab::Friends => {
                    if people.friends.is_empty() {
                        ui.label(theme::muted("No friends yet: Find players to add some."));
                    }
                    for p in &people.friends {
                        row(ui, p, &mut |ui| {
                            ui.menu_button(RichText::new("···").size(16.0).strong(), |ui| {
                                if ui.add_enabled(!busy, egui::Button::new("Remove friend")).clicked() {
                                    ask = Some((PlayerAction::Remove, p.clone()));
                                    ui.close();
                                }
                                if ui.add_enabled(!busy, egui::Button::new(format!("Block {}", p.name))).clicked() {
                                    ask = Some((PlayerAction::Block, p.clone()));
                                    ui.close();
                                }
                            })
                            .response
                            .on_hover_text(format!("More for {}", p.name));
                            if p.joinable && ui.add_enabled(!busy, theme::primary("Join")).clicked() {
                                join = Some((p.id.clone(), p.name.clone()));
                            }
                        });
                    }
                    if !people.elsewhere.is_empty() {
                        ui.add_space(8.0);
                        ui.label(theme::caps("Playing on other servers"));
                        for e in &people.elsewhere {
                            let place = if e.region.is_empty() {
                                e.server.clone()
                            } else {
                                format!("{} ({})", e.server, e.region)
                            };
                            person_card(
                                ui,
                                &e.name,
                                true,
                                &format!("On {place}: to play together, one of you joins the other's server"),
                                &mut |ui| {
                                    if setup::directory::listable_host(&e.host) && ui.add_enabled(!busy, theme::secondary("Switch server")).clicked() {
                                        switch = Some(e.host.clone());
                                    }
                                },
                            );
                        }
                    }
                }
                PeopleTab::Requests => {
                    if people.received.is_empty() && people.sent.is_empty() {
                        ui.label(theme::muted("No friend requests either way."));
                    }
                    if !people.received.is_empty() {
                        ui.label(theme::caps("Asked you"));
                    }
                    for p in &people.received {
                        row(ui, p, &mut |ui| {
                            if ui.add_enabled(!busy, theme::secondary("Decline")).clicked() {
                                act = Some((PlayerAction::Decline, p.clone()));
                            }
                            if ui.add_enabled(!busy, theme::secondary("Accept")).clicked() {
                                act = Some((PlayerAction::Accept, p.clone()));
                            }
                        });
                    }
                    if !people.sent.is_empty() {
                        ui.add_space(6.0);
                        ui.label(theme::caps("You asked"));
                    }
                    for p in &people.sent {
                        row(ui, p, &mut |ui| {
                            if ui.add_enabled(!busy, theme::secondary("Cancel request")).clicked() {
                                act = Some((PlayerAction::Decline, p.clone()));
                            }
                        });
                    }
                }
                PeopleTab::Blocked => {
                    ui.label(
                        theme::muted(format!(
                            "{BLOCK_MEANS} A block of a player with a 5th Echelon identity holds on every server that shares friends, when you have one too."
                        ))
                        .small(),
                    );
                    ui.add_space(4.0);
                    if people.blocked.is_empty() {
                        ui.label(theme::muted("You haven't blocked anyone."));
                    }
                    for p in &people.blocked {
                        row(ui, p, &mut |ui| {
                            if ui.add_enabled(!busy, theme::secondary("Unblock")).clicked() {
                                act = Some((PlayerAction::Unblock, p.clone()));
                            }
                        });
                    }
                }
                PeopleTab::Find => {}
            }
        });
    }
    if let Some(friend) = join {
        play.join_requested = Some(friend);
    }
    if let Some(host) = switch {
        // Confirmed on the Servers screen, which says where it goes and what happens there.
        play.switching = Some(Switch { host, new_name: None });
        play.people.go_servers = true;
    }
    let w = &mut play.people;
    if let Some(asked) = ask {
        w.asking = Some(asked);
    }
    if search {
        let (cfg, query) = ((*game.cfg).clone(), w.query.clone());
        w.found = None;
        w.searching.start(&ctx, move || flow::find_players(&cfg, &query));
    }
    if let Some((action, p)) = act {
        let cfg = (*game.cfg).clone();
        play.acting.start(&ctx, move || flow::player_action(&cfg, action, &p.id, "", &p.name));
    }
}

/// The most of the server's matches the Friends card lists.
const PLAYING_SHOWN: usize = 4;

/// How many others "Online now" lists before "and N more".
const ONLINE_SHOWN: usize = 8;

/// Others online on the player's server, under their friends (none on a friends-only server,
/// which shows only friends): each can be asked to be friends, invited while the game runs, or
/// blocked (right-click, so it isn't pressed by mistake).
fn online_now(play: &mut Play, game: &Game, others: &[flow::OnlinePlayer], playing: bool, ui: &mut egui::Ui) {
    use crate::network::PlayerAction;
    if others.is_empty() || game.managed.is_some() {
        return;
    }
    ui.add_space(6.0);
    ui.label(RichText::new(format!("Online now · {}", others.len())).color(theme::SOFT).small());
    let busy = play.acting.running();
    let mut act: Option<(PlayerAction, flow::OnlinePlayer)> = None;
    for p in others.iter().take(ONLINE_SHOWN) {
        let row = ui.horizontal(|ui| {
            friend_row(ui, theme::OK, &p.name, p.activity.as_deref().unwrap_or("Online"), if playing { 190.0 } else { 110.0 });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if playing && ui.add_enabled(!busy, theme::secondary("Invite")).on_hover_text("To the game you're in").clicked() {
                    act = Some((PlayerAction::Invite, p.clone()));
                }
                let (label, enabled) = match (p.asked_us, p.asked) {
                    (true, _) => ("Accept", true),
                    (false, true) => ("Asked", false),
                    (false, false) => ("Add friend", true),
                };
                if ui.add_enabled(enabled && !busy, theme::secondary(label)).clicked() {
                    act = Some((PlayerAction::Request, p.clone()));
                }
            });
        });
        row.response.context_menu(|ui| {
            if ui.button(format!("Block {}", p.name)).clicked() {
                act = Some((PlayerAction::Block, p.clone()));
                ui.close();
            }
        });
    }
    if others.len() > ONLINE_SHOWN {
        ui.label(theme::muted(format!("and {} more: Find players in the overlay lists them all", others.len() - ONLINE_SHOWN)).small());
    }
    if let Some((action, p)) = act {
        let cfg = (*game.cfg).clone();
        play.acting.start(ui.ctx(), move || flow::player_action(&cfg, action, &p.id, "", &p.name));
    }
}

/// What's being played on the player's server, under their friends: its public matches
/// (mode, map, players), the rest in a line.
fn playing_now(ui: &mut egui::Ui, now: &server_api::playing::Playing) {
    ui.add_space(8.0);
    ui.label(theme::caps("Playing now"));
    ui.add_space(2.0);
    if now.is_empty() {
        ui.label(theme::muted("Nobody's in a match or a lobby here right now.").small());
        return;
    }
    for m in now.matches.iter().take(PLAYING_SHOWN) {
        ui.label(RichText::new(m.line()).size(13.0));
    }
    let rest = server_api::playing::Playing {
        matches: now.matches.iter().skip(PLAYING_SHOWN).cloned().collect(),
        ..now.clone()
    };
    let more = rest.summary();
    if !more.is_empty() {
        let lead = if now.matches.is_empty() { "" } else { "Also: " };
        ui.label(theme::muted(format!("{lead}{more}")).small());
    }
}

/// One friend: a presence dot, the name, and a line under it.
/// `reserve`: room kept free on the right (a Join button after it): the name and line are cut
/// short with an ellipsis rather than run under the button (the Squad tab, 1150 wide).
fn friend_row(ui: &mut egui::Ui, dot: egui::Color32, name: &str, line: &str, reserve: f32) {
    let width = (ui.available_width() - reserve).max(80.0);
    ui.horizontal(|ui| {
        ui.set_max_width(width);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 30.0), egui::Sense::hover());
        ui.painter().circle_filled(egui::pos2(rect.center().x, rect.top() + 9.0), 4.0, dot);
        ui.vertical(|ui| {
            ui.set_max_width(width - 18.0);
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add(egui::Label::new(RichText::new(name).family(theme::strong())).truncate()).on_hover_text(name);
            ui.add(egui::Label::new(theme::muted(line).small()).truncate()).on_hover_text(line);
        });
    });
    ui.add_space(4.0);
}

/// The News screen: everything the server's news says, newest first.
pub fn show_news(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut go = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        theme::page().show(ui, |ui| {
            ui.set_width(ui.available_width().min(theme::MAX_PAGE));
            go = intel_header(View::News, ui);
            let (play, game, _) = app.play_mut();
            let Some(game) = game.as_mut() else {
                ui.label(theme::muted("Find the game first (Play)."));
                return;
            };
            load_news(play, game, &ctx);
            let server = game.cfg.current_profile().map(|p| p.server.clone()).unwrap_or_default();
            if server.is_empty() {
                ui.label(theme::muted("Choose a server first: its news shows here."));
                return;
            }
            ui.label(theme::muted(format!("From {server}, as the game's news screen shows it")));
            ui.add_space(14.0);
            if play.news.is_empty() {
                ui.label(
                    RichText::new(if play.fetching_news.running() {
                        "Fetching the news…"
                    } else {
                        "No news from this server."
                    })
                    .color(theme::SOFT),
                );
            }
            // Wide enough, and more than one: the briefings listed beside the one open, as the
            // game's own news screen pages them. Otherwise one card each.
            let wide = ui.available_width() >= 860.0 && play.news.len() > 1;
            if wide {
                let open = play.news_index.min(play.news.len() - 1);
                ui.horizontal_top(|ui| {
                    let list = 320.0;
                    ui.allocate_ui_with_layout(egui::vec2(list, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        theme::card().inner_margin(egui::Margin::same(0)).show(ui, |ui| {
                            ui.set_width(list);
                            for (i, item) in play.news.iter().enumerate() {
                                let on = i == open;
                                let title = RichText::new(hooks_config::text::clip(&item.title, 60)).family(theme::strong());
                                let button = egui::Button::new(title.color(if on { theme::FG } else { theme::SOFT }))
                                    .fill(if on { theme::SELECTED } else { egui::Color32::TRANSPARENT })
                                    .stroke(egui::Stroke::NONE)
                                    .corner_radius(0)
                                    .wrap()
                                    .min_size(egui::vec2(list, 52.0));
                                let response = ui.add(button);
                                if on {
                                    let r = response.rect;
                                    ui.painter()
                                        .rect_filled(egui::Rect::from_min_size(r.left_top(), egui::vec2(3.0, r.height())), 0, theme::ACCENT);
                                }
                                if response.clicked() {
                                    play.news_index = i;
                                    play.news_turned = Some(Instant::now());
                                }
                            }
                        });
                    });
                    ui.add_space(16.0);
                    let item = &play.news[open];
                    // Top-down in the room left: inside the row, the card's kicker, title and text
                    // went side by side and ran past the window (the gamebox, 1150 wide).
                    let rest = ui.available_width();
                    ui.allocate_ui_with_layout(egui::vec2(rest, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        theme::card().inner_margin(egui::Margin::symmetric(28, 24)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(theme::kicker(&format!("Briefing {} of {}", open + 1, play.news.len())));
                            ui.add_space(4.0);
                            ui.label(theme::display(&hooks_config::text::clip(&item.title, 80), 30.0));
                            if !item.text.is_empty() {
                                ui.add_space(6.0);
                                ui.label(RichText::new(hooks_config::text::clip(&item.text, 500)).color(theme::SOFT).size(16.0));
                            }
                            // Only a web address opens, and the player sees where it goes.
                            if item.link.starts_with("https://") {
                                ui.add_space(10.0);
                                ui.hyperlink_to("Read more", &item.link).on_hover_text(hooks_config::text::clip(&item.link, 120));
                            }
                        });
                    });
                });
            }
            for item in play.news.iter().filter(|_| !wide) {
                theme::card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(hooks_config::text::clip(&item.title, 80)).family(theme::strong()).size(18.0));
                    if !item.text.is_empty() {
                        ui.add_space(2.0);
                        ui.label(RichText::new(hooks_config::text::clip(&item.text, 500)).color(theme::SOFT));
                    }
                    // Only a web address opens, and the player sees where it goes.
                    if item.link.starts_with("https://") {
                        ui.add_space(4.0);
                        ui.hyperlink_to("Read more", &item.link).on_hover_text(hooks_config::text::clip(&item.link, 120));
                    }
                });
                ui.add_space(12.0);
            }
        });
    });
    if let Some(view) = go {
        app.set_view(view);
    }
}

/// Intel's title, over News (Briefings) and the Roadmap: the tabs between them, the Roadmap's
/// on the community network only. Answers the other screen when its tab is clicked.
pub(crate) fn intel_header(current: View, ui: &mut egui::Ui) -> Option<View> {
    let community = crate::app::Prefs::directory().is_some_and(|d| crate::app::Prefs::is_community(&d));
    let tabs: &[(View, &str)] = if community {
        &[(View::News, "Briefings"), (View::Roadmap, "Roadmap")]
    } else {
        &[(View::News, "Briefings")]
    };
    let kicker = if current == View::Roadmap { "What's being built" } else { "From your server" };
    theme::title_with_tabs(ui, kicker, "Intel", current, tabs).filter(|v| *v != current)
}

/// The Servers screen: the network the server menu shows (and another one), a server of
/// your own by its address, or one on this network.
pub fn show_servers(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut host = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        theme::page().show(ui, |ui| {
            ui.set_width(ui.available_width().min(theme::MAX_PAGE));
            let (play, game, notices) = app.play_mut();
            let Some(game) = game.as_mut() else {
                theme::title(ui, "Network", "Servers");
                ui.label(theme::muted("Find the game first (Play)."));
                return;
            };
            play.poll(&ctx, game, notices);
            poll_lookups(play, notices);
            auto_browse(play, &ctx);
            servers_page(play, game, notices, &ctx, ui);
            // Hosting a server of your own (until 0.5.0 a screen of the side menu's).
            ui.add_space(16.0);
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(theme::kicker("Host on this PC"));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Run a server for your group: a LAN party, or friends over the internet. Their launchers find it on your network.").color(theme::SOFT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::secondary("Host a server")).clicked() {
                            host = true;
                        }
                    });
                });
            });
        });
    });
    if host {
        app.set_view(View::Server);
    }
}

fn servers_page(play: &mut Play, game: &mut Game, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    let locked = game.managed.is_some();
    let playing = play.game_seen || play.running.is_some();
    theme::title(ui, "Network", "Servers");
    ui.label(RichText::new("Community servers are in the server menu on the Play screen. Here: a server of your own, a LAN party, or another network.").color(theme::SOFT));
    ui.add_space(14.0);
    if locked {
        ui.label(theme::muted("Another tool manages this install: change the server there."));
        return;
    }
    // A friend's server, asked for on the home screen.
    confirm_switch(play, game, notices, ctx, ui);
    if let Some(result) = play.checking_network.poll() {
        match result {
            Ok((url, count)) => {
                crate::app::Prefs::set_directory(Some(url.clone()));
                // The menu fills from the new network.
                play.browsed = false;
                play.directory = None;
                play.directory_error = None;
                play.network_typed.clear();
                let s = if count == 1 { "" } else { "s" };
                notices.info(format!(
                    "Using the network {}: its {count} server{s} are in the server menu on the Play screen.",
                    crate::connect::url_host(&url)
                ));
            }
            Err(e) => notices.error(e),
        }
    }
    network_card(play, notices, ctx, ui);
    ui.add_space(14.0);
    let columns = if ui.available_width() >= 760.0 { 2 } else { 1 };
    let mut connect = None;
    ui.columns(columns, |cols| {
        let ui = &mut cols[0];
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::caps("A server of your own"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let typed = ui.add(
                    egui::TextEdit::singleline(&mut play.address)
                        .margin(egui::Margin::symmetric(8, 5))
                        .hint_text(theme::hint("play.example.org or 192.168.1.20"))
                        .desired_width(ui.available_width() - 126.0)
                        .min_size(egui::vec2(0.0, 38.0)),
                );
                let ready = !play.address.trim().is_empty() && !play.busy() && !playing;
                let enter = typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let button = ui.add_enabled(ready, theme::primary("Connect").min_size(egui::vec2(90.0, 38.0)));
                let button = if playing { button.on_disabled_hover_text("Quit the game first") } else { button };
                if button.clicked() || (enter && ready) {
                    connect = Some(play.address.trim().to_string());
                }
            });
            ui.horizontal_wrapped(|ui| ui.label(theme::muted("A private server for your group, outside the network: the server menu takes you back.").small()));
            let recent = crate::app::Prefs::recent_servers();
            if !recent.is_empty() {
                ui.add_space(8.0);
                ui.label(theme::muted("Recent").small());
                let now = identity::now();
                for r in recent {
                    let label = r.name.clone().unwrap_or_else(|| r.address.clone());
                    let when = format!("used {}", setup::directory::ago(now - r.last_used).replace("within the last hour", "just now"));
                    if server_row(ui, &label, &when, !play.busy() && !playing) {
                        connect = Some(r.address.clone());
                    }
                }
            }
        });
        let ui = &mut cols[1 % columns];
        if columns == 1 {
            ui.add_space(14.0);
        }
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::caps("On your network"));
            ui.add_space(6.0);
            // Wrapped in a row: the columns' justified layout would justify the text too.
            ui.horizontal_wrapped(|ui| ui.label(RichText::new("Looks for a server running at a LAN party, on this network.").color(theme::SOFT)));
            ui.add_space(4.0);
            if play.looking.running() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(theme::muted("Looking…"));
                });
            } else if ui.horizontal(|ui| ui.add(theme::secondary("Find on my network"))).inner.clicked() {
                play.found.clear();
                find_on_network(play, ctx);
            }
            for ip in play.found.clone() {
                if server_row(ui, &ip.to_string(), "found on your network", !play.busy() && !playing) {
                    connect = Some(ip.to_string());
                }
            }
        });
    });
    // A server of the player's own: the account dialog, which is also the confirmation.
    if let Some(address) = connect {
        play.address = address.clone();
        let usual = flow::usual_name(&game.dir).unwrap_or_else(flow::windows_user);
        play.account = Some(AccountDialog::new(ctx, game, &address, usual, false, true, None));
    }
}

/// The network the server menu shows, and changing it.
fn network_card(play: &mut Play, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    let network = crate::app::Prefs::directory();
    let community = network.as_deref().is_some_and(crate::app::Prefs::is_community);
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(theme::caps("Network"));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                match &network {
                    Some(url) => {
                        ui.label(RichText::new(crate::connect::url_host(url)).family(theme::strong()).size(18.0));
                        let count = match play.directory.as_ref().map(Vec::len) {
                            Some(1) => "1 server · ".to_string(),
                            Some(n) => format!("{n} servers · "),
                            None => String::new(),
                        };
                        let what = if community { "The 5th Echelon community network · " } else { "" };
                        ui.label(theme::muted(format!("{what}{count}your friends and identity follow you between its servers")).small());
                    }
                    None => {
                        ui.label(RichText::new("No network").family(theme::strong()).size(18.0));
                        ui.label(theme::muted("The server menu on the Play screen is empty until you use one.").small());
                    }
                }
            });
            if network.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("In use").color(theme::OK).size(13.0));
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, theme::OK);
                });
            }
        });
        let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 13.0), egui::Sense::hover());
        ui.painter().hline(line.x_range(), line.center().y, egui::Stroke::new(1.0, theme::LINE));
        ui.label(RichText::new("Use another network").family(theme::strong()).size(14.0));
        ui.horizontal(|ui| {
            let typed = ui.add(
                egui::TextEdit::singleline(&mut play.network_typed)
                    .margin(egui::Margin::symmetric(8, 5))
                    .hint_text(theme::hint("its address, e.g. play.mygroup.org"))
                    .desired_width((ui.available_width() - 146.0).min(420.0))
                    .min_size(egui::vec2(0.0, 36.0)),
            );
            let ready = !play.network_typed.trim().is_empty();
            let enter = typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if play.checking_network.running() {
                ui.spinner();
            } else if (ui.add_enabled(ready, theme::secondary("Look it up").min_size(egui::vec2(0.0, 36.0))).clicked() || (enter && ready)) && ready {
                let typed = play.network_typed.clone();
                play.checking_network
                    .start(ctx, move || crate::services::rt().block_on(crate::network::check_network(&typed)));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(theme::muted("Another group's network of servers: its servers then fill the server menu on the Play screen.").small());
            if !community && ui.link(RichText::new("Back to the community network").color(theme::ACCENT).size(12.5)).clicked() {
                play.use_community_network(notices);
            }
        });
    });
}

/// A server to connect to: its name or address, a note, and Connect. Says whether it was pressed.
fn server_row(ui: &mut egui::Ui, label: &str, note: &str, enabled: bool) -> bool {
    let mut pressed = false;
    egui::Frame::new()
        .fill(theme::SUNKEN)
        .stroke(egui::Stroke::new(1.0, theme::LINE))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(hooks_config::text::clip(label, 40)).family(theme::strong()));
                ui.label(theme::muted(note).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    pressed = ui.add_enabled(enabled, theme::secondary("Connect")).clicked();
                });
            });
        });
    ui.add_space(4.0);
    pressed
}

/// Runs the setup; `new_name` names the account if the player has none on
/// the server (None: the setup asks). `public_only` when another server named
/// it (see [`flow::Plan`]). It reports in the activity bar.
fn start_setup(play: &mut Play, game: &Game, ctx: &egui::Context, notices: &mut Notices, new_name: Option<String>, public_only: bool) {
    let asked = crate::connect::ask(game, &play.server, new_name, public_only, play.directory.as_deref());
    play.switching = None;
    if let Some(id) = play.setup_failed.take() {
        notices.dismiss(id);
    }
    let log = notices.start(ctx, asked.title.clone());
    play.setup_failed = Some(log.id());
    play.setup_activity = Some(log.clone());
    play.last_setup = Some(asked.clone());
    play.setup.start(ctx, move || crate::connect::run(&asked, &log));
}

/// Asks before switching to a friend's server: its address in full, and what happens there.
fn confirm_switch(play: &mut Play, game: &Game, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some(switch) = play.switching.clone() else {
        return;
    };
    let account = match &switch.new_name {
        Some(name) => format!("signs you in there with your identity, as {name} (an account is made there if you have none)"),
        None => String::from("signs you in there with your identity (if you have no account there, you choose a name first)"),
    };
    theme::card().stroke(egui::Stroke::new(1.0, theme::WARN)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(format!("Switch to your friend's server {}? This {account}.", switch.host)).color(theme::WARN));
        ui.horizontal(|ui| {
            if ui.add(theme::primary(&format!("Switch to {}", switch.host))).clicked() {
                play.server = switch.host.clone();
                play.server_picked = false;
                play.needs_name = None;
                start_setup(play, game, ctx, notices, switch.new_name.clone(), true);
            }
            if ui.add(theme::secondary("Cancel")).clicked() {
                play.switching = None;
            }
        });
    });
    ui.add_space(14.0);
}

fn run_fix(play: &mut Play, game: &Game, notices: &mut Notices, fix: Fix, ctx: &egui::Context) {
    match fix {
        Fix::FindGame => {}
        Fix::ChooseServer | Fix::SetUpAccount => {
            let profile = game.cfg.current_profile().cloned().unwrap_or_default();
            play.editing = true;
            play.needs_name = None;
            play.server = profile.server;
        }
        Fix::UpdateLauncher => play.update_asked = true,
        _ => {
            let dir = game.dir.clone();
            play.fixing.start(ctx, move || crate::checks::run_fix(fix, &dir).unwrap_or_else(|| Ok(String::new())));
        }
    }
    if let Some(title) = crate::checks::fix_title(fix) {
        play.fix_activity = Some((notices.start(ctx, title), Some(fix)));
    }
}

fn support_row(play: &mut Play, game: &Game, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some((status, title, detail)) = crate::checks::support_line(play.support.as_ref()) else {
        return;
    };
    ui.horizontal_top(|ui| {
        theme::status_marker(ui, status);
        ui.vertical(|ui| {
            ui.label(title);
            ui.label(theme::muted(detail).small());
            if status == Status::Fail {
                if play.identifying.running() {
                    ui.spinner();
                } else if ui.horizontal(|ui| ui.add_enabled(!play.busy(), theme::secondary("Try to identify"))).inner.clicked() {
                    let (dir, version) = (game.dir.clone(), crate::checks::game_version(game));
                    play.identifying.start(ctx, move || crate::checks::identify(&dir, version).map(|_| ()));
                    play.fix_activity = Some((notices.start(ctx, "Identifying the game's version"), None));
                }
            }
        });
    });
}

/// The split Play button's width: the button and its arrow. "Playing" takes the same, so
/// nothing beside it moves when the game starts.
const SPLIT_PLAY_WIDTH: f32 = 220.0 + 2.0 + 42.0;

fn play_button(play: &mut Play, game: &mut Game, notices: &mut Notices, blocked: Option<&str>, ui: &mut egui::Ui) {
    let ready = !play.checks.is_empty() && setup::diagnose::ready(&play.checks) && !matches!(play.support, Some(Support::Unsupported(_)));
    choose_match(play, game, notices, ui);
    if play.running.is_some() || play.game_seen {
        ui.add_enabled(false, theme::play_button("Playing").min_size(egui::vec2(SPLIT_PLAY_WIDTH, 58.0)));
        return;
    }
    // A game set up by another tool (`[Managed]`) is started as it is: the launcher doesn't
    // write its uplay.toml, so it can't start straight into a lobby.
    let managed = game.managed.is_some();
    let chosen = *play.play_mode.get_or_insert_with(crate::app::Prefs::play_mode);
    let mode = if managed { crate::app::PlayMode::Play } else { chosen };
    // The game's files hashed for its sign-in first (game_files.rs): Play waits, saying so.
    let preparing = crate::game_files::preparing(&game.dir);
    if preparing {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    let enabled = !play.busy() && blocked.is_none() && !preparing;
    // A split button: the last choice is the button, the arrow beside it offers the others.
    let (button, arrow) = ui
        .allocate_ui_with_layout(egui::vec2(SPLIT_PLAY_WIDTH, 58.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let label = if preparing { "Checking files…" } else { mode.label() };
            let button = ui.add_enabled(enabled, theme::play_button(label).corner_radius(egui::CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }));
            let arrow = ui.add_enabled(
                enabled,
                egui::Button::new("")
                    .fill(theme::ACCENT)
                    .corner_radius(egui::CornerRadius { nw: 0, sw: 0, ne: 4, se: 0 })
                    .min_size(egui::vec2(42.0, 58.0)),
            );
            // The cut corner, as on a Field ops primary button: the page's colour over the last
            // corner of the pair.
            let r = arrow.rect;
            let cut = 14.0;
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    r.right_bottom() + egui::vec2(1.0, 1.0),
                    r.right_bottom() + egui::vec2(-cut, 1.0),
                    r.right_bottom() + egui::vec2(1.0, -cut),
                ],
                theme::BG,
                egui::Stroke::NONE,
            ));
            let c = arrow.rect.center();
            ui.painter().add(egui::Shape::convex_polygon(
                vec![c + egui::vec2(-7.0, -3.5), c + egui::vec2(7.0, -3.5), c + egui::vec2(0.0, 5.0)],
                theme::ON_ACCENT,
                egui::Stroke::NONE,
            ));
            (button, arrow)
        })
        .inner;
    let mut picked = None;
    let mut picked_match = None;
    let offered = !managed
        && play
            .offers_matches
            .as_ref()
            .is_some_and(|(s, on)| *on && game.cfg.current_profile().is_some_and(|p| &p.server == s));
    egui::Popup::menu(&arrow).show(|ui| {
        ui.set_min_width(160.0);
        for choice in crate::app::PlayMode::ALL {
            let can = !managed || choice == crate::app::PlayMode::Play;
            let item = ui.add_enabled(can, egui::Button::selectable(choice == mode, choice.label()));
            if item
                .on_disabled_hover_text("This game is set up by another tool, so the launcher starts it as it is.")
                .clicked()
            {
                picked = Some(choice);
            }
        }
        // Choosing your match, where the server offers it (matches.rs).
        if offered {
            ui.separator();
            if ui.button("New SvM match").on_hover_text("Host a new public match, whatever is open").clicked() {
                picked_match = Some(MatchAction::New(crate::app::PlayMode::Svm));
            }
            if ui.button("New Co-op match").on_hover_text("Host a new public match, whatever is open").clicked() {
                picked_match = Some(MatchAction::New(crate::app::PlayMode::Coop));
            }
            if ui
                .button("Choose a match...")
                .on_hover_text("The open public matches on your server, to join one")
                .clicked()
            {
                picked_match = Some(MatchAction::Choose);
            }
        }
    });
    if let Some(choice) = picked {
        crate::app::Prefs::set_play_mode(choice);
        play.play_mode = Some(choice);
        ui.close();
    }
    if let Some(action) = picked_match {
        match action {
            MatchAction::New(mode) => {
                let cfg = (*game.cfg).clone();
                play.new_mode = Some(mode);
                play.asking_new.start(ui.ctx(), move || crate::matches::new_match(&cfg));
            }
            MatchAction::Choose => {
                play.choosing = true;
                list_matches(play, game, ui.ctx());
            }
        }
    }
    let button = if let Some(why) = blocked {
        button.on_disabled_hover_text(why)
    } else if !ready && !play.checks.is_empty() {
        button.on_hover_text("Some checks failed; the game may not connect.")
    } else {
        button
    };
    if button.clicked() {
        launch(play, game, notices, mode);
    }
}

/// What the Play menu's match items asked for.
#[derive(Debug, Clone, Copy)]
enum MatchAction {
    New(crate::app::PlayMode),
    Choose,
}

/// Asks the player's server for its open matches (the window shows them).
fn list_matches(play: &mut Play, game: &Game, ctx: &egui::Context) {
    let server = game.cfg.current_profile().map(|p| p.server.clone()).unwrap_or_default();
    if !server.is_empty() && !play.listing.running() {
        play.listing.start(ctx, move || Ok(crate::matches::list(&server)));
    }
}

/// Choosing your match: a new match the server agreed to (the game started into that mode's
/// lobby, if it isn't running), and the open matches' window (Join starts the game first if
/// it isn't running, as a friend's Join does).
fn choose_match(play: &mut Play, game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) {
    answer_new_match(play, game, notices);
    if !play.choosing {
        return;
    }
    let mut open = true;
    let mut join = None;
    let mut refresh = false;
    egui::Window::new("Choose a match")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.set_min_width(420.0);
            match &play.open_matches {
                None => {
                    ui.label(theme::muted("Asking your server for its open matches..."));
                }
                Some(m) if !m.answered => {
                    ui.label(theme::muted("Your server didn't answer. Try again in a moment."));
                }
                Some(m) if m.rows.is_empty() => {
                    ui.label(theme::muted(
                        "No public match is open on your server right now: New match in the menu beside Play hosts one.",
                    ));
                }
                Some(m) => {
                    egui::Grid::new("open-matches").striped(true).num_columns(4).spacing([12.0, 6.0]).show(ui, |ui| {
                        for r in &m.rows {
                            ui.label(&r.mode);
                            ui.label(r.map.as_deref().unwrap_or(""));
                            ui.label(&r.players);
                            let why = if r.started {
                                Some("Under way: its host lets nobody in until it's over")
                            } else if r.full {
                                Some("Every seat is taken")
                            } else {
                                None
                            };
                            let button = ui.add_enabled(why.is_none() && !play.joining_match.running(), egui::Button::new("Join"));
                            if let Some(why) = why {
                                button.on_disabled_hover_text(why);
                            } else if button.clicked() {
                                join = Some(r.room);
                            }
                            ui.end_row();
                        }
                    });
                }
            }
            if let Some(m) = play.open_matches.as_ref().filter(|m| !m.others.is_empty()) {
                ui.label(theme::muted(&m.others));
            }
            ui.add_space(6.0);
            if ui.add_enabled(!play.listing.running(), egui::Button::new("Refresh")).clicked() {
                refresh = true;
            }
        });
    if refresh {
        list_matches(play, game, ui.ctx());
    }
    if let Some(room) = join {
        join_room(play, game, notices, room, ui.ctx());
        play.choosing = false;
    }
    if !open {
        play.choosing = false;
        play.open_matches = None;
    }
}

/// Starts the game, or says why not: whether it's starting. `mode` says whether it starts
/// straight into an online lobby.
fn launch(play: &mut Play, game: &mut Game, notices: &mut Notices, mode: crate::app::PlayMode) -> bool {
    let name_not_allowed = name_not_allowed(play, game).is_some();
    match crate::start::start(game, notices, mode, name_not_allowed) {
        Ok(setup::launch::Launched::Game(child)) => play.running = Some(child),
        Ok(setup::launch::Launched::GameProtonWithoutUmu(child)) => {
            play.running = Some(child);
            notices.error(setup::diagnose::NO_UMU);
        }
        Ok(setup::launch::Launched::Steam) => notices.info("Steam is starting the game."),
        Ok(setup::launch::Launched::GameNotSteam(child, why)) => {
            play.running = Some(child);
            notices.error(format!("Steam couldn't start the game ({why}), so the launcher did: no Steam Input this time."));
        }
        Ok(setup::launch::Launched::Elevated) => notices.info("The game was started as administrator, because it's set to run that way."),
        Err(crate::start::Refused::First(text)) => {
            notices.info(text);
            return false;
        }
        Err(crate::start::Refused::Error(text)) => {
            notices.error(text);
            return false;
        }
    }
    true
}

/// The player's name, when the server said it breaks the rules: only while it's still their
/// name (a rename takes effect at once, before the next look).
fn name_not_allowed(play: &Play, game: &Game) -> Option<String> {
    let name = play.friends.name_not_allowed.clone()?;
    game.cfg.current_profile().is_some_and(|p| p.user.username == name).then_some(name)
}

/// How long after the last key a name is checked.
const CHECK_AFTER: Duration = Duration::from_millis(400);

/// The account dialog: the name for a new account on a server, checked as it's typed.
pub(crate) struct AccountDialog {
    server: String,
    /// Where it is, and what it is ("North America · community server").
    place: String,
    kind: String,
    /// A server of the player's own, from the Servers screen (the dialog is its confirmation).
    own: bool,
    /// The name that was tried there and is taken.
    taken: Option<String>,
    name: String,
    typed_at: Instant,
    ping: Option<u32>,
    probing: Slot<Result<flow::Probe, String>>,
    probe: Option<Result<flow::Probe, String>>,
    checking: Slot<Result<(String, Result<flow::NameCheck, String>), String>>,
    checks: std::collections::HashMap<String, Result<flow::NameCheck, String>>,
    suggesting: Slot<Vec<String>>,
    suggestions: Vec<String>,
    suggested_for: Option<String>,
}

impl AccountDialog {
    fn new(ctx: &egui::Context, game: &Game, server: &str, suggested: String, taken: bool, own: bool, directory: Option<&[(setup::directory::Listing, Option<u32>)]>) -> Self {
        let listing = directory.and_then(|d| d.iter().find(|(s, _)| s.host.eq_ignore_ascii_case(server)));
        let (place, kind) = match listing {
            Some((s, _)) => (place(s), format!("{} · community server", s.name)),
            None if own => (server.to_string(), "A server of your own".to_string()),
            None => (server.to_string(), "A friend's server".to_string()),
        };
        let mut probing = Slot::default();
        let (dir, host) = (game.dir.clone(), server.to_string());
        probing.start(ctx, move || flow::probe(&dir, &host));
        let mut checks = std::collections::HashMap::new();
        if taken {
            checks.insert(suggested.clone(), Ok(flow::NameCheck::Taken(String::new())));
        }
        Self {
            server: server.to_string(),
            place,
            kind,
            own,
            taken: taken.then(|| suggested.clone()),
            name: suggested,
            typed_at: Instant::now(),
            ping: listing.and_then(|(_, ping)| *ping),
            probing,
            probe: None,
            checking: Slot::default(),
            checks,
            suggesting: Slot::default(),
            suggestions: Vec::new(),
            suggested_for: None,
        }
    }

    /// The server's API, once looked at, when it checks names.
    fn checker(&self) -> Option<String> {
        self.probe.as_ref().and_then(|p| p.as_ref().ok()).filter(|p| p.name_check).map(|p| p.api.clone())
    }

    /// Picks up answers, and checks the name a moment after the last key.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(probe) = self.probing.poll() {
            if let Ok(p) = &probe {
                self.ping = self.ping.or(p.ping);
            }
            self.probe = Some(probe);
        }
        if let Some(Ok((name, answer))) = self.checking.poll() {
            self.checks.insert(name, answer);
        }
        if let Some(found) = self.suggesting.poll() {
            self.suggestions = found;
        }
        let name = self.name.trim().to_string();
        let Some(api) = self.checker() else { return };
        if name.is_empty() || self.checks.contains_key(&name) || self.checking.running() {
            return;
        }
        let wait = CHECK_AFTER.saturating_sub(self.typed_at.elapsed());
        if !wait.is_zero() {
            ctx.request_repaint_after(wait);
            return;
        }
        self.checking.start(ctx, move || Ok((name.clone(), flow::check_name(&api, &name))));
    }

    /// Offers names when the one typed is taken.
    fn suggest(&mut self, ctx: &egui::Context) {
        let name = self.name.trim().to_string();
        let taken = matches!(self.checks.get(&name), Some(Ok(flow::NameCheck::Taken(_))));
        if !taken || self.suggested_for.as_deref() == Some(name.as_str()) || self.suggesting.running() {
            return;
        }
        let Some(api) = self.checker() else { return };
        self.suggested_for = Some(name.clone());
        self.suggestions.clear();
        self.suggesting.start(ctx, move || flow::free_suggestions(&api, &name));
    }
}

/// What the player did in the account dialog.
enum DialogChoice {
    Create(String),
    Close,
}

/// The account dialog, over whichever screen is open.
pub fn show_account_dialog(app: &mut App, ctx: &egui::Context) {
    if account_dialog_frame(app, ctx) {
        app.set_view(View::Play);
    }
}

/// Shows the dialog and acts on it; says whether a setup started (the Play screen shows it).
fn account_dialog_frame(app: &mut App, ctx: &egui::Context) -> bool {
    let (play, game, notices) = app.play_mut();
    let (Some(dialog), Some(game)) = (play.account.as_mut(), game.as_mut()) else {
        return false;
    };
    dialog.poll(ctx);
    dialog.suggest(ctx);
    let current = game
        .cfg
        .current_profile()
        .map(|p| p.server.clone())
        .filter(|s| !s.is_empty())
        .map(|host| play.directory.as_ref().and_then(|d| d.iter().find(|(s, _)| s.host == host)).map_or(host, |(s, _)| place(s)));
    let network = crate::app::Prefs::directory();
    let choice = account_dialog(ctx, dialog, current.as_deref(), network.as_deref());
    match choice {
        Some(DialogChoice::Create(name)) => {
            let dialog = play.account.take().expect("shown above");
            play.server = dialog.server.clone();
            play.server_picked = false;
            play.needs_name = None;
            start_setup(play, game, ctx, notices, Some(name), !dialog.own);
            true
        }
        Some(DialogChoice::Close) => {
            play.account = None;
            false
        }
        None => false,
    }
}

fn account_dialog(ctx: &egui::Context, dialog: &mut AccountDialog, current: Option<&str>, network: Option<&str>) -> Option<DialogChoice> {
    let mut choice = None;
    let modal = egui::Modal::new(egui::Id::new("account-dialog"))
        .frame(
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0, theme::CONTROL_LINE))
                .corner_radius(6)
                .inner_margin(egui::Margin { left: 26, right: 26, top: 24, bottom: 22 }),
        )
        .show(ctx, |ui| {
            ui.set_width(468.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            let title = if dialog.own { format!("Connect to {}", dialog.server) } else { "Choose your name on this server".to_string() };
            ui.label(theme::display(&title, 22.0));
            // The server: where, what, and how far.
            egui::Frame::new()
                .fill(theme::SUNKEN)
                .stroke(egui::Stroke::new(1.0, theme::LINE))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new(&dialog.place).family(theme::strong()));
                            let version = dialog.probe.as_ref().and_then(|p| p.as_ref().ok()).filter(|p| !p.version.is_empty()).map(|p| format!(" · 5th Echelon {}", p.version));
                            ui.label(theme::muted(format!("{}{}", dialog.kind, version.unwrap_or_default())).small());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match (&dialog.probe, dialog.ping) {
                            (_, Some(ms)) => {
                                ui.label(ping_text(Some(ms)));
                            }
                            (None, None) => {
                                ui.spinner();
                            }
                            (Some(_), None) => {
                                ui.label(theme::muted("no answer"));
                            }
                        });
                    });
                });
            let why = match &dialog.taken {
                Some(name) => format!("Someone on this server already has the name {name} (an account from before names were kept for each player), so you need another one here. Your friends still see it's you."),
                None => "You don't have an account here yet. Pick the name other players see. Your identity signs you in from now on: no password to remember.".to_string(),
            };
            ui.label(RichText::new(why).color(theme::SOFT));
            if let Some(Err(e)) = &dialog.probe {
                ui.label(RichText::new(e).color(theme::WARN).size(13.5));
            }
            ui.label(RichText::new("Your name").family(theme::strong()).size(14.0));
            let field = ui.add(
                egui::TextEdit::singleline(&mut dialog.name)
                .margin(egui::Margin::symmetric(8, 5))
                    .font(egui::FontId::new(16.0, theme::strong()))
                    .char_limit(setup::account::MAX_NAME)
                    .desired_width(f32::INFINITY)
                    .min_size(egui::vec2(0.0, 40.0)),
            );
            if field.changed() {
                dialog.typed_at = Instant::now();
            }
            let name = dialog.name.trim().to_string();
            let check = dialog.checks.get(&name).cloned();
            ui.horizontal(|ui| match &check {
                _ if name.is_empty() => {}
                Some(Ok(flow::NameCheck::Free)) => {
                    theme::status_marker(ui, Status::Ok);
                    ui.label(RichText::new("Free").color(theme::OK).size(13.5));
                }
                Some(Ok(flow::NameCheck::Taken(_))) => {
                    theme::status_marker(ui, Status::Fail);
                    ui.label(RichText::new("Taken on this server").color(theme::BAD).size(13.5));
                }
                Some(Ok(flow::NameCheck::NotAllowed(why))) => {
                    theme::status_marker(ui, Status::Fail);
                    ui.label(RichText::new(why).color(theme::BAD).size(13.5));
                }
                Some(Err(_)) | None if dialog.checker().is_none() => {}
                Some(Err(e)) => {
                    ui.label(theme::muted(format!("Couldn't check it: {e}")).small());
                }
                None => {
                    ui.spinner();
                    ui.label(theme::muted("Checking…").small());
                }
            });
            if matches!(check, Some(Ok(flow::NameCheck::Taken(_)))) && !dialog.suggestions.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for s in dialog.suggestions.clone() {
                        if ui.add(egui::Button::new(RichText::new(&s).family(theme::strong()).size(13.0)).corner_radius(99)).clicked() {
                            dialog.name = s.clone();
                            dialog.checks.insert(s, Ok(flow::NameCheck::Free));
                        }
                    }
                });
            }
            if dialog.own {
                let leaving = match network {
                    Some(url) if crate::app::Prefs::is_community(url) => "the community network".to_string(),
                    Some(url) => crate::connect::url_host(url).to_string(),
                    None => "your network".to_string(),
                };
                ui.label(theme::muted(format!(
                    "This leaves {leaving}: your friends and stats there stay there. The server menu on the Play screen takes you back."
                )).small());
            }
            let ready = !name.is_empty() && !matches!(check, Some(Ok(flow::NameCheck::Taken(_) | flow::NameCheck::NotAllowed(_))));
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let create = if dialog.own { "Create account and connect" } else { "Create account" };
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add_enabled(ready, theme::primary(create).min_size(egui::vec2(0.0, 38.0))).clicked() || (enter && ready) {
                    choice = Some(DialogChoice::Create(name.clone()));
                }
                let stay = match (dialog.own, current) {
                    (false, Some(place)) => format!("Stay on {place}"),
                    _ => "Cancel".to_string(),
                };
                if ui.add(theme::secondary(&stay).min_size(egui::vec2(0.0, 38.0))).clicked() {
                    choice = Some(DialogChoice::Close);
                }
            });
        });
    if modal.should_close() && choice.is_none() {
        choice = Some(DialogChoice::Close);
    }
    choice
}

// --- The Matches screen ---------------------------------------------------------------------

/// The Matches screen's mode filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum MatchKind {
    #[default]
    All,
    Svm,
    Coop,
}

/// How the Matches screen sorts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum MatchSort {
    #[default]
    Nearest,
    Busiest,
    Newest,
}

/// The Matches screen (the 0.5.0 lobby browser, a screen of the side menu as in the Tauri
/// launcher): every public match on the player's server in full, the player's filters, the one
/// picked, and the match watched for a seat (asked about every 10 s wherever the player is).
struct MatchesPage {
    loading: Slot<Result<crate::matches::Lobbies, String>>,
    data: Option<crate::matches::Lobbies>,
    loaded_at: Option<Instant>,
    picked: Option<u32>,
    kind: MatchKind,
    show_under_way: bool,
    show_full: bool,
    friends_first: bool,
    sort: MatchSort,
    /// "Tell me when a seat opens": the room and its line ("SvM Classic · Penthouse").
    watching: Option<(u32, String)>,
    watch_check: Slot<Result<crate::matches::Lobbies, String>>,
    watched_at: Option<Instant>,
    /// The watched match has a seat: the banner's Join.
    seat_open: Option<(u32, String)>,
}

impl Default for MatchesPage {
    fn default() -> Self {
        Self {
            loading: Slot::default(),
            data: None,
            loaded_at: None,
            picked: None,
            kind: MatchKind::All,
            show_under_way: true,
            show_full: false,
            friends_first: true,
            sort: MatchSort::Nearest,
            watching: None,
            watch_check: Slot::default(),
            watched_at: None,
            seat_open: None,
        }
    }
}

/// How often the list is read again while the screen is open, and the watched match asked about.
const MATCHES_EVERY: Duration = Duration::from_secs(5);
const WATCH_EVERY: Duration = Duration::from_secs(10);

/// The new match's answer (asked from the Play menu or the Matches screen): the game started
/// into that mode's lobby, if it isn't running.
fn answer_new_match(play: &mut Play, game: &mut Game, notices: &mut Notices) {
    let running = play.running.is_some() || play.game_seen;
    if let Some(result) = play.asking_new.poll() {
        let mode = play.new_mode.take().unwrap_or(crate::app::PlayMode::Svm);
        match result {
            Ok(holds) => {
                let minutes = holds.as_secs().div_ceil(60);
                if running {
                    notices.info(format!(
                        "Start matchmaking in the game: it hosts a new public match rather than join an open one (for the next {minutes} minutes)."
                    ));
                } else {
                    notices.info(format!(
                        "The game starts in the lobby: start matchmaking there and it hosts a new public match (for the next {minutes} minutes)."
                    ));
                    launch(play, game, notices, mode);
                }
            }
            Err(line) => notices.error(line),
        }
    }
}

/// Joins the public match in `room`, starting the game first if it isn't running.
fn join_room(play: &mut Play, game: &mut Game, notices: &mut Notices, room: u32, ctx: &egui::Context) {
    let running = play.running.is_some() || play.game_seen;
    if running || launch(play, game, notices, crate::app::PlayMode::Play) {
        let (cfg, dir) = ((*game.cfg).clone(), game.dir.clone());
        play.joining_match.start(ctx, move || crate::matches::join(&cfg, &dir, room));
    }
}

/// The watched match, asked about every 10 s wherever the player is in the launcher; once a seat
/// opens, a bar with Join along the top, and the window asks for attention. A match that's gone
/// ends the watch. Called every frame, before the screens.
pub fn watch_seat(app: &mut App, ctx: &egui::Context) {
    let (play, game, notices) = app.play_mut();
    let Some(game) = game.as_mut() else { return };
    let m = &mut play.matches;
    if let Some(Ok(got)) = m.watch_check.poll() {
        if let Some((room, label)) = m.watching.clone() {
            match got.lobbies.iter().find(|l| l.room == room) {
                None => m.watching = None,
                Some(l) if l.joinable => {
                    m.watching = None;
                    m.seat_open = Some((room, label));
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
                }
                Some(_) => {}
            }
        }
    }
    if m.watching.is_some() && !m.watch_check.running() && m.watched_at.is_none_or(|t| t.elapsed() >= WATCH_EVERY) {
        m.watched_at = Some(Instant::now());
        let cfg = (*game.cfg).clone();
        m.watch_check.start(ctx, move || Ok(crate::matches::lobbies(&cfg)));
    }
    let Some((room, label)) = m.seat_open.clone() else { return };
    let mut join = false;
    egui::TopBottomPanel::top("seat-open")
        .frame(egui::Frame::new().fill(theme::SELECTED).inner_margin(egui::Margin::symmetric(16, 8)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("A seat opened in {label}.")).color(theme::ACCENT).family(theme::strong()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(theme::secondary("Dismiss")).clicked() {
                        play.matches.seat_open = None;
                    }
                    if ui.add(theme::primary("Join")).clicked() {
                        join = true;
                    }
                });
            });
        });
    if join {
        play.matches.seat_open = None;
        join_room(play, game, notices, room, ctx);
    }
}

/// The Matches screen of the side menu.
pub fn show_matches(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let (play, game, notices) = app.play_mut();
    let Some(game) = game.as_mut() else {
        theme::page().show(ui, |ui| {
            theme::title(ui, "No server yet", "Matches");
            ui.label(theme::muted("Find the game first (Play)."));
        });
        return;
    };
    play.poll(&ctx, game, notices);
    answer_new_match(play, game, notices);
    // As the Tauri launcher's: listed once there's a server (joining asks for an account).
    let profile = game.cfg.current_profile().filter(|p| !p.server.is_empty()).map(|p| p.server.clone());
    let running = play.running.is_some() || play.game_seen;
    let m = &mut play.matches;
    if let Some(result) = m.loading.poll() {
        m.data = Some(result.unwrap_or_else(|e| crate::matches::Lobbies {
            error: Some(e),
            ..crate::matches::Lobbies::default()
        }));
    }
    if profile.is_some() && !m.loading.running() && m.loaded_at.is_none_or(|t| t.elapsed() >= MATCHES_EVERY) {
        m.loaded_at = Some(Instant::now());
        let cfg = (*game.cfg).clone();
        m.loading.start(&ctx, move || Ok(crate::matches::lobbies_or_open(&cfg)));
    }
    ctx.request_repaint_after(Duration::from_secs(1));
    let ready = profile.is_some() && !running && game.managed.is_none();
    let mut act: Option<MatchAct> = None;
    matches_page(&mut play.matches, profile.as_deref(), ready, &mut act, ui);
    match act {
        Some(MatchAct::Join(room)) => join_room(play, game, notices, room, &ctx),
        Some(MatchAct::New(mode)) => {
            let cfg = (*game.cfg).clone();
            play.new_mode = Some(mode);
            play.asking_new.start(&ctx, move || crate::matches::new_match(&cfg));
        }
        Some(MatchAct::Quick(mode)) => {
            launch(play, game, notices, mode);
        }
        Some(MatchAct::Servers) | None => {}
    }
    if matches!(act, Some(MatchAct::Servers)) {
        app.set_view(View::Servers);
    }
}

/// What the Matches screen asked for.
#[derive(Debug, Clone, Copy)]
enum MatchAct {
    Join(u32),
    New(crate::app::PlayMode),
    Quick(crate::app::PlayMode),
    Servers,
}

/// A ping's colour: near, far, or not known.
fn ping_color(ms: Option<u32>) -> egui::Color32 {
    match ms {
        None => theme::MUTED,
        Some(ms) if ms <= 120 => theme::OK,
        Some(_) => theme::WARN,
    }
}

fn ping_ms(ms: Option<u32>) -> String {
    ms.map_or_else(|| "–".to_string(), |ms| format!("{ms} ms"))
}

/// "Tank, your friend, is in it" / "Tank and 2 more of your friends are in it".
fn friend_line(friends: &[String]) -> String {
    match friends {
        [] => String::new(),
        [one] => format!("{one}, your friend, is in it"),
        [first, rest @ ..] => format!("{first} and {} more of your friends are in it", rest.len()),
    }
}

/// The matches the filters let through, sorted as asked.
fn shown_matches<'a>(m: &MatchesPage, all: &'a [crate::matches::Lobby]) -> Vec<&'a crate::matches::Lobby> {
    let mut list: Vec<&crate::matches::Lobby> = all
        .iter()
        .filter(|l| match m.kind {
            MatchKind::All => true,
            MatchKind::Svm => l.kind != "coop",
            MatchKind::Coop => l.kind == "coop",
        })
        .filter(|l| m.show_under_way || !l.under_way)
        // A full match under way is the Under way filter's.
        .filter(|l| m.show_full || !l.full || l.under_way || Some(l.room) == m.picked)
        .collect();
    list.sort_by_key(|l| {
        let friends = if m.friends_first { u8::from(l.friends.is_empty()) } else { 0 };
        let key = match m.sort {
            MatchSort::Nearest => i64::from(l.ping_ms.unwrap_or(9999)),
            MatchSort::Busiest => -(l.players as i64),
            MatchSort::Newest => i64::from(l.open_secs),
        };
        (friends, key)
    });
    list
}

fn matches_page(m: &mut MatchesPage, server: Option<&str>, ready: bool, act: &mut Option<MatchAct>, ui: &mut egui::Ui) {
    let quick_mode = if m.kind == MatchKind::Coop {
        crate::app::PlayMode::Coop
    } else {
        crate::app::PlayMode::Svm
    };
    // The header: the server, the title and the summary, with New match and Quick match.
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 32,
            right: 32,
            top: 22,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    theme::title(ui, server.unwrap_or("No server yet"), "Matches");
                    let summary = match &m.data {
                        None => "Reading the open matches…".to_string(),
                        Some(d) if d.summary.is_empty() => String::new(),
                        Some(d) => format!("{} · updated live", d.summary),
                    };
                    ui.label(RichText::new(summary).color(theme::SOFT).small());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(ready, theme::primary("Quick match").min_size(egui::vec2(0.0, 40.0))).clicked() {
                        *act = Some(MatchAct::Quick(quick_mode));
                    }
                    if ui.add_enabled(ready, theme::secondary("New match").min_size(egui::vec2(0.0, 40.0))).clicked() {
                        *act = Some(MatchAct::New(quick_mode));
                    }
                });
            });
        });
    let line = ui.available_rect_before_wrap();
    ui.painter().hline(line.x_range(), line.top(), egui::Stroke::new(1.0, theme::LINE));
    let pad = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(32, 16)).show(ui, |ui| add(ui));
    };
    let Some(server_set) = server.map(|_| ()) else {
        pad(ui, &mut |ui| {
            ui.label(theme::muted("Set up the game and a server first (Play)."));
        });
        return;
    };
    let _ = server_set;
    let Some(data) = m.data.clone() else {
        pad(ui, &mut |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted("Reading the open matches…"));
            });
        });
        return;
    };
    if let Some(e) = &data.error {
        let mut again = false;
        pad(ui, &mut |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Couldn't read the matches: {e}")).color(theme::BAD));
                if ui.link("Try again").clicked() {
                    again = true;
                }
            });
        });
        if again {
            m.loaded_at = None;
        }
        return;
    }
    let all = &data.lobbies;
    // The filters.
    if !all.is_empty() {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(32, 10)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let svm = all.iter().filter(|l| l.kind != "coop").count();
                let labels = [
                    (MatchKind::All, format!("All · {}", all.len())),
                    (MatchKind::Svm, format!("Spies vs Mercs · {svm}")),
                    (MatchKind::Coop, format!("Co-op · {}", all.len() - svm)),
                ];
                let options: Vec<(MatchKind, &str)> = labels.iter().map(|(k, l)| (*k, l.as_str())).collect();
                theme::segmented(ui, &mut m.kind, &options);
                ui.add_space(8.0);
                ui.checkbox(&mut m.show_under_way, "Under way");
                ui.checkbox(&mut m.show_full, "Full");
                ui.checkbox(&mut m.friends_first, "Friends first");
                // At the row's right end when there's room for it, else on the next line.
                let sort = |ui: &mut egui::Ui, m: &mut MatchesPage| {
                    egui::ComboBox::from_id_salt("matches-sort")
                        .selected_text(match m.sort {
                            MatchSort::Nearest => "Nearest first",
                            MatchSort::Busiest => "Most players",
                            MatchSort::Newest => "Newest",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut m.sort, MatchSort::Nearest, "Nearest first");
                            ui.selectable_value(&mut m.sort, MatchSort::Busiest, "Most players");
                            ui.selectable_value(&mut m.sort, MatchSort::Newest, "Newest");
                        });
                };
                let rest = ui.available_width();
                if rest > 230.0 {
                    // Only the rest of the row: right-aligned in it, never over the checkboxes.
                    ui.allocate_ui_with_layout(egui::vec2(rest, 36.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        sort(ui, m);
                        ui.label(theme::muted("Sort"));
                    });
                } else {
                    ui.end_row();
                    ui.label(theme::muted("Sort"));
                    sort(ui, m);
                }
            });
        });
        let line = ui.available_rect_before_wrap();
        ui.painter().hline(line.x_range(), line.top(), egui::Stroke::new(1.0, theme::LINE));
    }
    let shown = shown_matches(m, all);
    // The first one picked once the list comes, and again when the picked one goes.
    if !shown.iter().any(|l| Some(l.room) == m.picked) {
        m.picked = shown.first().map(|l| l.room);
    }
    if shown.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(theme::display(if all.is_empty() { "No public matches open" } else { "Nothing matches the filters" }, 24.0));
            ui.add_space(6.0);
            ui.label(theme::muted(if all.is_empty() {
                "Host one and it shows here for everyone; quick match finds you one as soon as anyone opens."
            } else {
                "Show the ones under way or full, or another mode."
            }));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let wide = 300.0;
                ui.add_space(((ui.available_width() - wide) / 2.0).max(0.0));
                if ui.add_enabled(ready, theme::primary("Host a match")).clicked() {
                    *act = Some(MatchAct::New(quick_mode));
                }
                if ui.add(theme::secondary("See other servers")).clicked() {
                    *act = Some(MatchAct::Servers);
                }
            });
            if !data.hidden.is_empty() {
                ui.add_space(6.0);
                ui.label(theme::muted(&data.hidden).small());
            }
        });
        return;
    }
    let picked = shown.iter().find(|l| Some(l.room) == m.picked).map(|l| (*l).clone());
    let narrow = ui.available_width() < 1000.0;
    // The details on the right, the list beside them.
    if let Some(sel) = &picked {
        egui::SidePanel::right("match-detail")
            .resizable(false)
            .exact_width(if narrow { 320.0 } else { 380.0 })
            .frame(egui::Frame::new().fill(theme::SURFACE).stroke(egui::Stroke::new(1.0, theme::LINE)))
            .show_inside(ui, |ui| match_detail(m, sel, act, ui));
    }
    egui::CentralPanel::default().frame(egui::Frame::new()).show_inside(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // The Tauri launcher's columns: the match 1.6 parts, players fixed, the status 1.2
            // parts, the ping fixed; each cell left-aligned at its width (the ping right-aligned).
            let gap = 12.0;
            let w = ui.available_width() - 44.0 - 3.0 * gap;
            let (players_w, ping_w) = (if narrow { 76.0 } else { 110.0 }, if narrow { 60.0 } else { 70.0 });
            let what_w = ((w - players_w - ping_w) * 1.6 / 2.8).max(120.0);
            let status_w = ((w - players_w - ping_w) * 1.2 / 2.8).max(100.0);
            let cell = |ui: &mut egui::Ui, width: f32, right: bool, add: &mut dyn FnMut(&mut egui::Ui)| {
                let layout = if right {
                    egui::Layout::right_to_left(egui::Align::Center)
                } else {
                    egui::Layout::left_to_right(egui::Align::Center)
                };
                ui.allocate_ui_with_layout(egui::vec2(width, 38.0), layout, |ui| {
                    ui.set_width(width);
                    add(ui);
                });
            };
            ui.add_space(6.0);
            egui::Frame::new().inner_margin(egui::Margin::symmetric(22, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (label, width, right) in [("Match", what_w, false), ("Players", players_w, false), ("Status", status_w, false), ("Ping", ping_w, true)] {
                        let layout = if right {
                            egui::Layout::right_to_left(egui::Align::Center)
                        } else {
                            egui::Layout::left_to_right(egui::Align::Center)
                        };
                        ui.allocate_ui_with_layout(egui::vec2(width, 16.0), layout, |ui| {
                            ui.set_width(width);
                            ui.label(theme::caps(label));
                        });
                    }
                });
            });
            for l in &shown {
                let on = Some(l.room) == m.picked;
                let row = egui::Frame::new()
                    .fill(if on { theme::SELECTED } else { egui::Color32::TRANSPARENT })
                    .inner_margin(egui::Margin::symmetric(22, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            cell(ui, what_w, false, &mut |ui| {
                                ui.vertical(|ui| {
                                    let title = match &l.map {
                                        Some(map) => format!("{} · {map}", l.mode),
                                        None => l.mode.clone(),
                                    };
                                    ui.add(egui::Label::new(RichText::new(title).family(theme::strong())).truncate());
                                    let mut under = Vec::new();
                                    // A friend hosting is said once: "Hosted by Tank, your friend",
                                    // not "Hosted by Tank · Tank, your friend, is in it".
                                    let host_friend = !l.host.is_empty() && l.friends.contains(&l.host);
                                    if !l.host.is_empty() {
                                        under.push(if host_friend {
                                            format!("Hosted by {}, your friend", l.host)
                                        } else {
                                            format!("Hosted by {}", l.host)
                                        });
                                    }
                                    let others: Vec<String> = l.friends.iter().filter(|f| **f != l.host).cloned().collect();
                                    if !others.is_empty() {
                                        under.push(friend_line(&others));
                                    }
                                    if let Some(made) = l.made_for {
                                        under.push(format!("{} seats (made for {made})", l.seats));
                                    }
                                    let color = if l.friends.is_empty() { theme::SOFT } else { theme::ACCENT };
                                    ui.add(egui::Label::new(RichText::new(under.join(" · ")).color(color).small()).truncate());
                                });
                            });
                            cell(ui, players_w, false, &mut |ui| {
                                ui.vertical(|ui| {
                                    let of = if l.seats > 0 { format!("/{}", l.seats) } else { String::new() };
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        ui.label(RichText::new(l.players.to_string()).family(theme::strong()));
                                        ui.label(theme::muted(of));
                                    });
                                    let (bar, _) = ui.allocate_exact_size(egui::vec2(players_w - 12.0, 4.0), egui::Sense::hover());
                                    ui.painter().rect_filled(bar, 2, theme::LINE);
                                    if l.seats > 0 {
                                        let fill = (l.players as f32 / l.seats as f32).min(1.0);
                                        let mut done = bar;
                                        done.set_width(bar.width() * fill);
                                        ui.painter().rect_filled(done, 2, if l.full { theme::WARN } else { theme::ACCENT });
                                    }
                                });
                            });
                            cell(ui, status_w, false, &mut |ui| {
                                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                                ui.painter().circle_filled(dot.center(), 4.0, if l.tone == "warn" { theme::WARN } else { theme::ACCENT });
                                ui.add(egui::Label::new(&l.status).truncate());
                            });
                            cell(ui, ping_w, true, &mut |ui| {
                                ui.label(RichText::new(ping_ms(l.ping_ms)).monospace().color(ping_color(l.ping_ms)));
                            });
                        });
                    })
                    .response;
                let row = ui
                    .interact(row.rect, ui.id().with(("match-row", l.room)), egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if on {
                    ui.painter()
                        .rect_filled(egui::Rect::from_min_size(row.rect.min, egui::vec2(3.0, row.rect.height())), 0, theme::ACCENT);
                } else if row.hovered() {
                    ui.painter().rect_filled(row.rect, 0, egui::Color32::from_white_alpha(6));
                }
                if row.clicked() {
                    m.picked = Some(l.room);
                }
                let line = row.rect;
                ui.painter().hline(line.x_range(), line.bottom(), egui::Stroke::new(1.0, theme::LINE));
            }
            if !data.hidden.is_empty() {
                ui.add_space(10.0);
                egui::Frame::new().inner_margin(egui::Margin::symmetric(22, 0)).show(ui, |ui| {
                    ui.label(theme::muted(format!("{} Private matches aren't listed.", data.hidden)).small());
                });
            }
        });
    });
}

/// A player's round badge with their initial.
fn avatar(ui: &mut egui::Ui, name: &str, friend: bool, empty: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
    let c = rect.center();
    if empty {
        ui.painter().circle_stroke(c, 12.0, egui::Stroke::new(1.0, theme::CONTROL_LINE));
        return;
    }
    ui.painter()
        .circle_filled(c, 13.0, if friend { egui::Color32::from_rgb(0x1f, 0x35, 0x24) } else { theme::LINE });
    let initial = name.chars().next().map_or_else(|| "?".to_string(), |c| c.to_uppercase().to_string());
    ui.painter().text(
        c,
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::new(12.0, theme::strong()),
        if friend { theme::ACCENT } else { theme::FG },
    );
}

fn person_row(ui: &mut egui::Ui, sel: &crate::matches::Lobby, p: &crate::matches::Person, ping: bool) {
    egui::Frame::new()
        .fill(if p.friend { theme::SELECTED } else { egui::Color32::TRANSPARENT })
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                avatar(ui, &p.name, p.friend, false);
                ui.add(egui::Label::new(RichText::new(&p.name).family(theme::strong())).truncate());
                if p.host {
                    ui.label(RichText::new("HOST").monospace().size(10.5).color(theme::ACCENT));
                }
                if p.friend {
                    ui.label(RichText::new("friend").color(theme::ACCENT).small());
                } else if p.party != 0 {
                    let others: Vec<&str> = sel.people.iter().filter(|q| q.party == p.party && q.name != p.name).map(|q| q.name.as_str()).collect();
                    let line = if others.is_empty() {
                        "in a party".to_string()
                    } else {
                        format!("party with {}", others.join(", "))
                    };
                    ui.label(theme::muted(line).small());
                }
                if ping {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(ms) = p.ping_ms {
                            ui.label(RichText::new(format!("{ms} ms")).monospace().small().color(ping_color(Some(ms))));
                        }
                    });
                }
            });
        });
}

fn match_detail(m: &mut MatchesPage, sel: &crate::matches::Lobby, act: &mut Option<MatchAct>, ui: &mut egui::Ui) {
    // The band: the mode, and the map in big type.
    let band = if sel.kind == "coop" {
        egui::Color32::from_rgb(0x12, 0x19, 0x2a)
    } else {
        egui::Color32::from_rgb(0x14, 0x20, 0x19)
    };
    // A short window: the band takes less, so the players stay in view (as the Tauri launcher's).
    let short = ui.available_height() < 560.0;
    egui::Frame::new()
        .fill(band)
        .inner_margin(egui::Margin {
            left: 22,
            right: 22,
            top: if short { 10 } else { 34 },
            bottom: if short { 8 } else { 12 },
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let kicker = format!("{} · public{}", sel.mode, if sel.full { " · full" } else { "" });
            ui.label(theme::kicker(&kicker).color(if sel.tone == "warn" { theme::WARN } else { theme::ACCENT }));
            ui.add(egui::Label::new(theme::display(sel.map.as_deref().unwrap_or(&sel.mode), if short { 22.0 } else { 28.0 })).truncate());
        });
    // Join, or watch for a seat, along the bottom.
    egui::TopBottomPanel::bottom("match-act")
        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(22, 14)).stroke(egui::Stroke::new(1.0, theme::LINE)))
        .show_inside(ui, |ui| {
            ui.vertical_centered_justified(|ui| {
                if sel.joinable {
                    if ui.add(theme::play_button("Join this match").min_size(egui::vec2(0.0, 46.0))).clicked() {
                        *act = Some(MatchAct::Join(sel.room));
                    }
                    let what = if sel.under_way { "match" } else { "lobby" };
                    let host = if sel.host.is_empty() { "its host".to_string() } else { sel.host.clone() };
                    if !short {
                        ui.label(theme::muted(format!("Starts the game if it isn't running, then joins {host}'s {what}.")).small());
                    }
                } else {
                    let why = match (sel.under_way, sel.full) {
                        (true, true) => "Under way · full",
                        (true, false) => "Under way",
                        _ => "Full",
                    };
                    ui.add_enabled(false, theme::play_button(why).min_size(egui::vec2(0.0, 46.0)));
                    let watching = m.watching.as_ref().is_some_and(|(r, _)| *r == sel.room);
                    let label = if watching {
                        "Watching: we'll tell you when it opens"
                    } else {
                        "Tell me when a seat opens"
                    };
                    if ui.add(theme::secondary(label).selected(watching)).clicked() {
                        m.watching = if watching {
                            None
                        } else {
                            let label = match &sel.map {
                                Some(map) => format!("{} · {map}", sel.mode),
                                None => sel.mode.clone(),
                            };
                            m.watched_at = None;
                            Some((sel.room, label))
                        };
                    }
                }
            });
        });
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(22, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            // Its state.
            egui::Frame::new()
                .fill(theme::SUNKEN)
                .stroke(egui::Stroke::new(
                    1.0,
                    if sel.tone == "warn" {
                        egui::Color32::from_rgb(0x3a, 0x30, 0x20)
                    } else {
                        theme::CONTROL_LINE
                    },
                ))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 5.0, if sel.tone == "warn" { theme::WARN } else { theme::ACCENT });
                        // The ping on the right first, so the lines beside it stop short of it.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if sel.ping_ms.is_some() {
                                ui.label(RichText::new(ping_ms(sel.ping_ms)).monospace().color(ping_color(sel.ping_ms)));
                            }
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.add(egui::Label::new(RichText::new(&sel.status).family(theme::strong())).truncate());
                                let mut open = Vec::new();
                                if !sel.open_for.is_empty() {
                                    open.push(format!("Open {}", sel.open_for));
                                }
                                if sel.joinable {
                                    open.push("joinable now".to_string());
                                }
                                ui.add(egui::Label::new(RichText::new(open.join(" · ")).color(theme::SOFT).small()).truncate());
                            });
                        });
                    });
                });
            ui.add_space(8.0);
            if sel.teams {
                ui.columns(2, |cols| {
                    for (col, (side, label)) in cols.iter_mut().zip([("spy", "Spies"), ("merc", "Mercs")]) {
                        let people: Vec<&crate::matches::Person> = sel.people.iter().filter(|p| p.team == side).collect();
                        col.label(theme::caps(&format!("{label} · {}", people.len())));
                        for p in people {
                            person_row(col, sel, p, false);
                        }
                    }
                });
            } else {
                let what = if sel.under_way { "match" } else { "lobby" };
                let of = if sel.seats > 0 { format!(" of {}", sel.seats) } else { String::new() };
                ui.label(theme::caps(&format!("In this {what} · {}{of}", sel.players)));
                for p in &sel.people {
                    person_row(ui, sel, p, true);
                }
                if sel.seats as usize > sel.players {
                    let open = sel.seats as usize - sel.players;
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        avatar(ui, "", false, true);
                        ui.label(theme::muted(format!("{open} open {}", if open == 1 { "seat" } else { "seats" })));
                    });
                }
            }
            if !sel.settings.is_empty() {
                ui.add_space(10.0);
                ui.label(theme::caps("Match settings"));
                egui::Grid::new("match-settings").num_columns(4).spacing([16.0, 4.0]).show(ui, |ui| {
                    for (i, (k, v)) in sel.settings.iter().enumerate() {
                        ui.label(RichText::new(k).color(theme::SOFT).small());
                        ui.label(RichText::new(v).small());
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            }
        });
    });
}

#[cfg(all(test, feature = "ui-tests"))]
mod split_button_tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn shown(play: &mut Play, game: &mut Game) -> egui_kittest::Harness<'static> {
        let mut notices = Notices::default();
        // The game's files hashed first (an empty folder: a moment), or Play waits on them and
        // keeps repainting.
        let waited = Instant::now();
        while crate::game_files::preparing(&game.dir) && waited.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Owned copies: the harness keeps its closure.
        let mut play = std::mem::take(play);
        let mut game = std::mem::replace(game, crate::theme::ui_test::game("split-tmp").1);
        crate::theme::ui_test::harness(egui::vec2(900.0, 300.0), move |ui| play_button(&mut play, &mut game, &mut notices, None, ui))
    }

    #[test]
    fn the_play_button_names_the_choice_and_a_managed_install_plays_plainly() {
        let (dir, mut game) = crate::theme::ui_test::game("split");
        let mut play = Play {
            play_mode: Some(crate::app::PlayMode::Svm),
            ..Play::default()
        };
        let h = shown(&mut play, &mut game);
        assert!(
            h.query_by_label(&crate::app::PlayMode::Svm.label().to_uppercase()).is_some(),
            "the last choice is the button"
        );

        let (dir2, mut managed) = crate::theme::ui_test::game("split-managed");
        managed.managed = Some(hooks_config::Managed {
            by: "Community Hub".into(),
            help_text: None,
        });
        let mut play = Play {
            play_mode: Some(crate::app::PlayMode::Svm),
            ..Play::default()
        };
        let h = shown(&mut play, &mut managed);
        assert!(
            h.query_by_label("PLAY").is_some() && h.query_by_label(&crate::app::PlayMode::Svm.label().to_uppercase()).is_none(),
            "managed: plain Play"
        );
        for d in [dir, dir2] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// Choose a match: the open matches, a started or full one not joinable, and the others.
    #[test]
    fn the_match_window_lists_the_open_matches() {
        let (dir, mut game) = crate::theme::ui_test::game("matches");
        let row = |room: u32, mode: &str, started: bool, full: bool| crate::matches::Row {
            room,
            mode: mode.into(),
            map: Some("Mining Town".into()),
            players: "3/8".into(),
            started,
            full,
        };
        let mut play = Play {
            choosing: true,
            open_matches: Some(crate::matches::Matches {
                offered: true,
                answered: true,
                rows: vec![row(1, "SvM Blacklist", false, false), row(2, "SvM Classic", true, false)],
                others: "1 private match".into(),
            }),
            ..Play::default()
        };
        let h = shown(&mut play, &mut game);
        assert!(h.query_by_label("SvM Blacklist").is_some() && h.query_by_label("SvM Classic").is_some());
        assert_eq!(h.query_all_by_label("Join").count(), 2);
        assert!(h.query_by_label("1 private match").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The Friends window: each section's people with its actions, and Remove asking first.
    #[test]
    fn the_friends_screen_lists_each_section_with_its_actions() {
        let (dir, game) = crate::theme::ui_test::game("people");
        let person = |id: &str, name: &str, relation: &'static str| flow::Person {
            id: id.into(),
            name: name.into(),
            online: true,
            relation,
            ..flow::Person::default()
        };
        let people = flow::People {
            friends: vec![person("1", "Tank", "friend")],
            received: vec![person("2", "Raven", "received")],
            sent: vec![person("3", "Nova", "sent")],
            blocked: vec![person("4", "Ghost", "blocked")],
            ..flow::People::default()
        };
        let window = |tab: PeopleTab, asking: Option<(crate::network::PlayerAction, flow::Person)>| {
            let mut play = Play {
                people: PeopleWindow {
                    open: true,
                    tab,
                    shown: Some(Ok(people.clone())),
                    asking,
                    ..PeopleWindow::default()
                },
                ..Play::default()
            };
            let game = std::sync::Arc::new(crate::theme::ui_test::game("people-shown").1);
            crate::theme::ui_test::harness(egui::vec2(700.0, 600.0), move |ui| people_page(&mut play, &game, ui))
        };
        let h = window(PeopleTab::Friends, None);
        assert!(h.query_by_label("Tank").is_some() && h.query_by_label("···").is_some(), "Remove and Block in the ··· menu");
        assert!(
            h.query_by_label("Friends · 1/1").is_some() && h.query_by_label("Requests · 2").is_some(),
            "counts on the tabs"
        );
        let h = window(PeopleTab::Requests, None);
        assert!(h.query_by_label("Accept").is_some() && h.query_by_label("Decline").is_some() && h.query_by_label("Cancel request").is_some());
        let h = window(PeopleTab::Blocked, None);
        assert!(h.query_by_label("Ghost").is_some() && h.query_by_label("Unblock").is_some());
        let h = window(PeopleTab::Friends, Some((crate::network::PlayerAction::Remove, person("1", "Tank", "friend"))));
        assert!(h.query_by_label("Remove Tank from your friends?").is_some(), "Remove asks first");
        drop(game);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn playing_is_as_wide_as_the_split_button() {
        let (dir, mut game) = crate::theme::ui_test::game("playing");
        let mut play = Play {
            game_seen: true,
            ..Play::default()
        };
        let h = shown(&mut play, &mut game);
        let width = h.get_by_label("PLAYING").rect().width();
        assert!((width - SPLIT_PLAY_WIDTH).abs() < 1.0, "{width} against {SPLIT_PLAY_WIDTH}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod matches_page_tests {
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::matches::Lobbies;
    use crate::matches::Lobby;
    use crate::matches::Person;

    fn person(name: &str, host: bool, friend: bool, team: &str) -> Person {
        Person {
            name: name.into(),
            host,
            friend,
            team: team.into(),
            ping_ms: Some(40),
            ..Person::default()
        }
    }

    fn sample() -> Lobbies {
        Lobbies {
            offered: true,
            summary: "2 public · 5 players".into(),
            lobbies: vec![
                Lobby {
                    room: 1,
                    mode: "SvM Classic".into(),
                    kind: "svm".into(),
                    map: Some("Penthouse".into()),
                    players: 3,
                    seats: 8,
                    host: "Tank".into(),
                    status: "In lobby · waiting for 1 more".into(),
                    tone: "ok",
                    joinable: true,
                    open_for: "4 min".into(),
                    open_secs: 240,
                    ping_ms: Some(60),
                    friends: vec!["Raven".into()],
                    people: vec![person("Tank", true, false, ""), person("Raven", false, true, ""), person("Nova", false, false, "")],
                    ..Lobby::default()
                },
                Lobby {
                    room: 2,
                    mode: "Co-op".into(),
                    kind: "coop".into(),
                    map: Some("Mine".into()),
                    players: 2,
                    seats: 2,
                    host: "Ghost".into(),
                    status: "Under way · 6 min".into(),
                    tone: "warn",
                    under_way: true,
                    full: true,
                    open_for: "9 min".into(),
                    open_secs: 540,
                    people: vec![person("Ghost", true, false, ""), person("Kiwi", false, false, "")],
                    ..Lobby::default()
                },
            ],
            ..Lobbies::default()
        }
    }

    fn page(m: MatchesPage) -> egui_kittest::Harness<'static> {
        let mut m = m;
        crate::theme::ui_test::harness(egui::vec2(1280.0, 800.0), move |ui| {
            let mut act = None;
            matches_page(&mut m, Some("scbl.example.net"), true, &mut act, ui);
        })
    }

    #[test]
    fn the_matches_screen_lists_matches_with_the_picked_ones_details() {
        let h = page(MatchesPage {
            data: Some(sample()),
            ..MatchesPage::default()
        });
        assert!(
            h.query_by_label("SvM Classic · Penthouse").is_some() && h.query_by_label("Co-op · Mine").is_some(),
            "both listed"
        );
        assert!(h.query_by_label("Raven, your friend, is in it").is_some() || h.query_by_label("Hosted by Tank · Raven, your friend, is in it").is_some());
        // Friends first: the SvM match is picked, so its details and Join show.
        assert!(h.query_by_label("JOIN THIS MATCH").is_some(), "the joinable one picked first");
        assert!(h.query_by_label("Quick match").is_some() && h.query_by_label("New match").is_some());
    }

    #[test]
    fn filters_and_a_watched_match() {
        // Co-op only: the full match under way is picked, so it offers the seat watch.
        let h = page(MatchesPage {
            data: Some(sample()),
            kind: MatchKind::Coop,
            ..MatchesPage::default()
        });
        assert!(h.query_by_label("SvM Classic · Penthouse").is_none(), "another mode filtered out");
        assert!(h.query_by_label("Tell me when a seat opens").is_some());
        // Under way hidden: nothing left in co-op.
        let h = page(MatchesPage {
            data: Some(sample()),
            kind: MatchKind::Coop,
            show_under_way: false,
            ..MatchesPage::default()
        });
        assert!(h.query_by_label("NOTHING MATCHES THE FILTERS").is_some());
        // Watching it: the button says so.
        let h = page(MatchesPage {
            data: Some(sample()),
            kind: MatchKind::Coop,
            watching: Some((2, "Co-op · Mine".into())),
            ..MatchesPage::default()
        });
        assert!(h.query_by_label("Watching: we'll tell you when it opens").is_some());
    }

    #[test]
    fn sorting_newest_and_busiest() {
        let m = MatchesPage {
            friends_first: false,
            show_full: true,
            sort: MatchSort::Newest,
            ..MatchesPage::default()
        };
        let data = sample();
        let rooms = |m: &MatchesPage| shown_matches(m, &data.lobbies).iter().map(|l| l.room).collect::<Vec<_>>();
        assert_eq!(rooms(&m), [1, 2], "newest (open least long) first");
        let m = MatchesPage { sort: MatchSort::Busiest, ..m };
        assert_eq!(rooms(&m), [1, 2], "most players first");
        let m = MatchesPage { sort: MatchSort::Nearest, ..m };
        assert_eq!(rooms(&m), [1, 2], "a known ping before an unknown one");
    }
}
