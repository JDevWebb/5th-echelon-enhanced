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
use setup::game::GameVersion;

use crate::activity::Action;
use crate::app::App;
use crate::app::Game;
use crate::app::Notices;
use crate::app::View;
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
    friends: flow::Friends,
    checks: Vec<Check>,
    support: Option<Support>,
    refreshed: Option<Instant>,

    /// The join form, shown when there's no server yet or on "Change server".
    editing: bool,
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
    last_setup: Option<SetupAsked>,
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
    /// The news item the home card shows, and when it last turned.
    news_index: usize,
    news_turned: Option<Instant>,
}

/// A setup as it was started, to start it again.
#[derive(Debug, Clone)]
struct SetupAsked {
    server: String,
    new_name: Option<String>,
    public_only: bool,
    /// What the activity says when it worked, and when it didn't.
    done: String,
    failed: String,
}

/// A friend's server to switch to.
#[derive(Debug, Clone)]
struct Switch {
    host: String,
    /// The name for an account made there, if the player has none (None: the setup asks).
    new_name: Option<String>,
}

impl Play {
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
        self.facts.start(ctx, move || flow::gather(&dir, &cfg, crate::dll_utils::bundled()));
    }

    fn poll(&mut self, ctx: &egui::Context, game: &Game, notices: &mut Notices) {
        // A failed fetch keeps the last list: it's a hint, not a check.
        if let Some(Ok(friends)) = self.fetching_friends.poll() {
            self.friends = friends;
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
                    flow::forget_account_check();
                    if let (Some(a), Some(asked)) = (&activity, &asked) {
                        a.done(asked.done.clone());
                    }
                    // A server of the player's own (not the network's, nor one a server named): kept
                    // with the recent ones on the Servers screen.
                    let listed = self.directory.as_ref().is_some_and(|d| d.iter().any(|(s, _)| s.host.eq_ignore_ascii_case(&server)));
                    if !via_network && !listed && !asked.as_ref().is_some_and(|a| a.public_only) {
                        crate::app::Prefs::remember_server(&server, None);
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
                        let own = !asked.as_ref().is_some_and(|a| a.public_only);
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
                self.server = asked.server.clone();
                start_setup(self, game, ctx, notices, asked.new_name.clone(), asked.public_only);
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Go {
    View(View),
    Settings(crate::settings::Section),
}

fn go(app: &mut App, to: Option<Go>) {
    match to {
        Some(Go::View(view)) => app.set_view(view),
        Some(Go::Settings(section)) => app.open_settings(section),
        None => {}
    }
}

/// The Play screen: the guided setup until there's a server and an account,
/// then the home screen.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut to = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if app.game.is_none() {
            no_game(app, ui);
            return;
        }
        let (play, game, notices) = app.play_mut();
        let game = game.as_mut().expect("checked above");
        play.poll(&ctx, game, notices);
        let has_server = game.cfg.current_profile().is_some_and(|p| !p.server.is_empty());
        if game.managed.is_none() && (!has_server || play.editing) {
            setup_screen(play, game, notices, &ctx, ui);
        } else {
            to = home(play, game, notices, &ctx, ui);
        }
    });
    go(app, to);
}

/// The steps of the guided setup, in order.
const STEPS: [&str; 4] = ["Find the game", "Choose a server", "Pick your name", "Play"];

/// The guided setup's frame: the steps on the left (`step` is the current
/// one), the step itself on the right.
fn wizard(ui: &mut egui::Ui, step: usize, body: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(28.0);
    let width = (ui.available_width() - 32.0).min(980.0);
    let side = ((ui.available_width() - width) / 2.0).max(16.0);
    let stacked = width < 700.0;
    ui.horizontal(|ui| {
        ui.add_space(side);
        egui::Frame::new()
            .fill(theme::SURFACE)
            .stroke(egui::Stroke::new(1.0, theme::LINE))
            .corner_radius(18)
            .show(ui, |ui| {
                ui.set_width(width);
                let aside = |ui: &mut egui::Ui| {
                    egui::Frame::new()
                        // Side by side, the fill is painted to the card's full height afterwards.
                        .fill(if stacked { theme::SUNKEN } else { egui::Color32::TRANSPARENT })
                        .corner_radius(if stacked {
                            egui::CornerRadius { nw: 18, ne: 18, sw: 0, se: 0 }
                        } else {
                            egui::CornerRadius { nw: 18, ne: 0, sw: 18, se: 0 }
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
                                theme::mark(ui, 34.0);
                                ui.add_space(14.0);
                                ui.label(theme::display("Get online in a minute", 28.0));
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
                            ui.label(theme::caps(&format!("Step {} of {}", step + 1, STEPS.len())));
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
                        .set(fill, egui::Shape::rect_filled(side, egui::CornerRadius { nw: 18, ne: 0, sw: 18, se: 0 }, theme::SUNKEN));
                }
            });
    });
    ui.add_space(28.0);
}

/// One step in the list: done (a tick), current (a green ring) or to come.
fn step_row(ui: &mut egui::Ui, i: usize, name: &str, current: usize) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
        let c = rect.center();
        let p = ui.painter();
        if i < current {
            p.circle_filled(c, 13.0, theme::OK);
            let s = egui::Stroke::new(2.4, theme::ON_ACCENT);
            p.line_segment([c + egui::vec2(-5.0, 0.5), c + egui::vec2(-1.5, 4.0)], s);
            p.line_segment([c + egui::vec2(-1.5, 4.0), c + egui::vec2(5.5, -3.5)], s);
        } else {
            let color = if i == current { theme::ACCENT } else { theme::CONTROL_LINE };
            p.circle_stroke(c, 12.0, egui::Stroke::new(2.0, color));
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                (i + 1).to_string(),
                egui::FontId::proportional(13.0),
                if i == current { theme::ACCENT } else { theme::MUTED },
            );
        }
        let text = RichText::new(name);
        ui.label(match i.cmp(&current) {
            std::cmp::Ordering::Equal => text.family(theme::strong()).color(theme::FG),
            _ => text.color(theme::MUTED),
        });
    });
    ui.add_space(6.0);
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
    play.looking.start(ctx, || {
        let adapters = setup::net::adapters();
        let refs: Vec<(&str, IpAddr)> = adapters.iter().map(|(n, ip)| (n.as_str(), *ip)).collect();
        crate::services::rt()
            .block_on(crate::network::try_locate_server(None, &refs))
            .map_err(|_| "No server answered on this network.".to_string())
    });
}

fn ping_again(play: &mut Play, ctx: &egui::Context, url: String) {
    play.browsed_at = Some(Instant::now());
    play.browsing.start(ctx, move || crate::services::rt().block_on(crate::network::server_directory(&url)));
}

/// The guided setup's server and name steps (the setup itself installs the
/// client, makes or finds the account and pins the adapter).
fn setup_screen(play: &mut Play, game: &mut Game, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    poll_lookups(play, notices);
    // With a directory, its servers are pinged as soon as the form opens, and the best preselected.
    auto_browse(play, ctx);
    let naming = play.needs_name.is_some();
    wizard(ui, if naming { 2 } else { 1 }, |ui| {
        ui.label(RichText::new(if naming { "Pick your name" } else { "Choose a server" }).family(theme::strong()).size(24.0));
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
            .corner_radius(12)
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
                        .hint_text("what other players see")
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
                                .hint_text("play.example.org or 192.168.1.20")
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
                        .corner_radius(12),
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
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(16, 10));
        let inner = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // The name wraps in its own column; the ping and a badge keep the right.
                let left = (ui.available_width() - 90.0).max(160.0);
                ui.allocate_ui_with_layout(egui::vec2(left, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.label(RichText::new(server_name(s)).family(theme::strong()));
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
fn badge(ui: &mut egui::Ui, text: &str, strong: bool) {
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
fn home(play: &mut Play, game: &mut Game, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) -> Option<Go> {
    let mut to = None;
    load_art(play, game, ctx);
    load_news(play, game, ctx);
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
    hero(play, game, &mut to, ui);
    let notice = update_notice(play, game, ctx);
    launch_bar(play, game, notices, notice.as_ref(), &mut to, ui);
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
    theme::page().inner_margin(egui::Margin::symmetric(32, 22)).show(ui, |ui| {
        let gap = 16.0;
        let width = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            status_card(play, game, notices, ctx, &mut to, width, ui);
            friends_card(play, game, &mut to, width, ui);
            news_card(play, ctx, &mut to, width, ui);
        });
    });
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
    let listed = servers.iter().find(|(s, _)| s.host == current).cloned();
    if let Some((listing, _)) = &listed {
        play.my_listing = Some(listing.clone());
    }
    // Gone from the list while it restarts or is down for maintenance: as it was last
    // listed, not answering.
    let mine = listed.or_else(|| {
        play.my_listing
            .clone()
            .filter(|l| l.host == current && (play.rollout.is_some() || !l.maintenance.is_empty()))
            .map(|l| (l, None))
    });
    let clock = setup::clock::Clock::local();
    let now = identity::now();
    let mine = mine.as_ref().map(|(l, ping)| (l, *ping));
    let release = crate::update_notice::notice(&crate::update_notice::Input {
        rollout: play.rollout.as_ref(),
        servers: &servers,
        mine,
        launcher: env!("FE_RELEASE"),
        newer_launcher: play.newer_launcher.as_deref(),
        now,
        clock: &clock,
    });
    let maintenance = crate::maintenance::notice(&crate::maintenance::Input {
        servers: &servers,
        mine,
        now,
        clock: &clock,
    });
    let network = crate::maintenance::network_notice(play.network_maintenance.as_ref(), now, &clock);
    let notice = crate::update_notice::most_pressing([release, maintenance, network]);
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
/// How long each news item shows before the card moves to the next.
const NEWS_TURN: Duration = Duration::from_secs(8);

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

/// The banner: the game's name over one of its loading screens (or the
/// night-vision grid until there is one), and who's playing in the corner.
fn hero(play: &mut Play, game: &Game, to: &mut Option<Go>, ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 300.0), egui::Sense::hover());
    theme::grid_backdrop(ui.painter(), rect);
    if let Some(texture) = &play.art_texture {
        theme::cover_image(ui.painter(), rect, texture);
        theme::scrim(ui.painter(), rect);
    }
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], egui::Stroke::new(1.0, theme::LINE));
    let inner = rect.shrink2(egui::vec2(32.0, 24.0));
    let mut top = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Min)));
    top.label(theme::caps(env!("FE_PRODUCT")));
    top.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if profile_card(play, game, ui).clicked() {
            *to = Some(Go::Settings(crate::settings::Section::Identity));
        }
    });
    let mut bottom = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::bottom_up(egui::Align::LEFT)));
    bottom.label(
        RichText::new("Co-op and Spies vs Mercs on community servers. No VPN, no port forwarding.")
            .size(16.0)
            .color(theme::SOFT),
    );
    bottom.label(theme::display("Splinter Cell: Blacklist", 50.0));
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
    if play.running.is_some() || play.game_seen {
        return (theme::OK, "Splinter Cell: Blacklist is running".into());
    }
    if play.checks.is_empty() {
        return (theme::MUTED, "Checking…".into());
    }
    let fails = play.checks.iter().filter(|c| c.status == Status::Fail).count() + usize::from(matches!(play.support, Some(Support::Unsupported(_))));
    let warns = play.checks.iter().filter(|c| c.status == Status::Warn).count();
    match (fails, warns) {
        (0, 0) => (theme::OK, "Ready to play · everything checked".into()),
        (0, 1) => (theme::WARN, "Ready to play · 1 note".into()),
        (0, n) => (theme::WARN, format!("Ready to play · {n} notes")),
        (1, _) => (theme::BAD, "1 thing to fix".into()),
        (n, _) => (theme::BAD, format!("{n} things to fix")),
    }
}

/// The bar under the banner: the server you're on, how ready you are, and Play.
/// Who runs the server, for the server card's caption, when it isn't the community
/// network: "Self-hosted" for a server on no network, or the other network's name, with
/// a line saying so (`listed`: in the directory in use; `loaded`: the directory is known,
/// so a server missing from it really isn't there, and the label doesn't flash).
fn run_by(listed: bool, loaded: bool) -> Option<(String, String)> {
    let network = crate::app::Prefs::directory();
    // With no network there's no list to wait for.
    let loaded = loaded || network.is_none();
    if network.as_deref().is_some_and(crate::app::Prefs::is_community) && listed {
        return None;
    }
    match network {
        Some(url) if listed => {
            let host = url_host(&url).to_string();
            let why = format!("On {host}'s network, run by its admins, not by the community network.");
            Some((hooks_config::text::clip(&host, 22), why))
        }
        _ if !listed && loaded => Some((
            "Self-hosted".to_string(),
            "Run by whoever hosts it, not by the community network. Fine for your own or a friend's server.".to_string(),
        )),
        _ => None,
    }
}

fn launch_bar(play: &mut Play, game: &mut Game, notices: &mut Notices, notice: Option<&crate::update_notice::Notice>, to: &mut Option<Go>, ui: &mut egui::Ui) {
    let profile = game.cfg.current_profile().cloned().unwrap_or_default();
    let listing = play.directory.as_ref().and_then(|d| d.iter().find(|(s, _)| s.host == profile.server)).cloned();
    let playing = play.game_seen || play.running.is_some();
    let menu_id = egui::Id::new("server-menu");
    let menu_open = egui::Popup::is_id_open(ui.ctx(), menu_id);
    let run_by = run_by(listing.is_some(), play.directory.is_some());
    let mut caption_rect = None;
    egui::Frame::new().fill(theme::SUNKEN).inner_margin(egui::Margin::symmetric(32, 16)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            // The server: click for the server menu.
            let chip = ui.allocate_ui_with_layout(egui::vec2(300.0, 60.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, if menu_open { theme::ACCENT } else { theme::CONTROL_LINE }))
                    .corner_radius(12)
                    .inner_margin(egui::Margin {
                        left: 16,
                        right: 10,
                        top: 10,
                        bottom: 10,
                    })
                    .show(ui, |ui| {
                        ui.set_width(300.0 - 26.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                // Who runs it, when it isn't the community network: hovered, says so.
                                let caption = match &run_by {
                                    Some((who, _)) => format!("Server · {who}"),
                                    None => "Server".to_string(),
                                };
                                caption_rect = Some(ui.label(theme::caps(&caption)).rect);
                                // Its region is enough here ("Sydney, Australia").
                                let name = listing.as_ref().map(|(s, _)| place(s)).unwrap_or_else(|| profile.server.clone());
                                ui.label(RichText::new(hooks_config::text::clip(&name, 24)).family(theme::strong()));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                chevron_down(ui, menu_open);
                                ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    if let Some((s, ping)) = &listing {
                                        ui.label(ping_text(*ping));
                                        ui.label(theme::muted(format!("{} online", s.players_online)).small());
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
                        .corner_radius(12)
                        .inner_margin(8)
                        .shadow(egui::Shadow {
                            offset: [0, 18],
                            blur: 50,
                            spread: 0,
                            color: egui::Color32::from_black_alpha(140),
                        }),
                );
            let network = crate::app::Prefs::directory();
            let header = match &network {
                Some(url) if crate::app::Prefs::is_community(url) => format!("Community servers · {}", url_host(url)),
                Some(url) => format!("Servers · {}", url_host(url)),
                None => "No network".to_string(),
            };
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
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    // A release going out, or maintenance, says what it means for playing now.
                    let (color, text) = match notice {
                        Some(n) if n.blocks_play.is_none() => (theme::WARN, n.status.clone()),
                        Some(n) => (theme::BAD, n.status.clone()),
                        None => readiness(play),
                    };
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, color);
                    ui.add(egui::Label::new(RichText::new(&text).size(15.0)).truncate()).on_hover_text(&text);
                });
            });
        });
    });
}

/// A network's host, from its address.
fn url_host(url: &str) -> &str {
    url.trim_start_matches("https://").trim_end_matches('/')
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
    const HEIGHT: f32 = 300.0;
    ui.allocate_ui_with_layout(egui::vec2(width, HEIGHT), egui::Layout::top_down(egui::Align::LEFT), |ui| {
        theme::card().show(ui, |ui| {
            ui.set_width(width - 40.0);
            ui.set_height(HEIGHT - 36.0);
            ui.horizontal(|ui| {
                ui.label(theme::caps(title));
                if let Some(note) = note {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(note.size(12.5));
                    });
                }
            });
            ui.add_space(6.0);
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
    let issues = play.checks.iter().any(|c| c.status != Status::Ok);
    let fixable = play.checks.iter().any(|c| c.status == Status::Fail && c.fix.is_some());
    let has_server = game.cfg.current_profile().is_some_and(|p| !p.server.is_empty());
    // All of them when asked; else the problems, or a few that matter when there are none.
    const SUMMARY: [&str; 4] = ["client", "account", "network", "save"];
    let checks: Vec<Check> = play
        .checks
        .iter()
        .filter(|c| play.show_all_checks || if issues { c.status != Status::Ok } else { SUMMARY.contains(&c.id) })
        .cloned()
        .collect();
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
                            if ui.horizontal(|ui| ui.add_enabled(!busy, theme::secondary(fix_label(f)))).inner.clicked() {
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
                if ui.link(if show_all { "Show less" } else { "All checks" }).clicked() {
                    toggle = true;
                }
                ui.label(theme::muted("·"));
                if checking {
                    ui.spinner();
                } else if ui.link("Check again").clicked() {
                    again = true;
                }
                ui.label(theme::muted("·"));
                if ui.link("Connection test").clicked() {
                    *to = Some(Go::Settings(crate::settings::Section::Network));
                }
            });
            if fixable && has_server && !managed && !busy {
                fix_all = ui
                    .horizontal(|ui| ui.add(theme::primary("Fix everything").min_size(egui::vec2(0.0, 36.0)).corner_radius(10)))
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
                friend_row(ui, theme::OK, &friend.name, friend.activity.as_deref().unwrap_or("Online"));
            }
            for friend in &friends.elsewhere {
                let server = if friend.region.is_empty() { friend.server.clone() } else { friend.region.clone() };
                ui.horizontal(|ui| {
                    friend_row(ui, theme::WARN, &friend.username, &format!("On {server}"));
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
        },
        // Bottom up: the hint last, any requests above it.
        |ui| {
            let offline = if friends.offline > 0 {
                format!("{} offline · ", friends.offline)
            } else {
                String::new()
            };
            ui.label(theme::muted(format!("{offline}Add and invite friends in game with {}", game.cfg.hook_config.overlay_key)).small());
            if friends.requests > 0 {
                let s = if friends.requests == 1 { "" } else { "s" };
                ui.label(RichText::new(format!("{} friend request{s} waiting", friends.requests)).color(theme::ACCENT).size(13.0));
            }
        },
    );
    if let Some(host) = switch_to {
        // Confirmed on the Servers screen, which says where it goes and what happens there.
        play.switching = Some(Switch { host, new_name: None });
        *to = Some(Go::View(View::Servers));
    }
}

/// One friend: a presence dot, the name, and a line under it.
fn friend_row(ui: &mut egui::Ui, dot: egui::Color32, name: &str, line: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 30.0), egui::Sense::hover());
        ui.painter().circle_filled(egui::pos2(rect.center().x, rect.top() + 9.0), 4.0, dot);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(name).family(theme::strong()));
            ui.label(theme::muted(line).small());
        });
    });
    ui.add_space(4.0);
}

/// The server's news, one item at a time (the newest first), turning on its
/// own; View all opens the News screen.
fn news_card(play: &mut Play, ctx: &egui::Context, to: &mut Option<Go>, width: f32, ui: &mut egui::Ui) {
    let count = play.news.len();
    if count > 1 && play.news_turned.is_none_or(|t| t.elapsed() >= NEWS_TURN) {
        if play.news_turned.is_some() {
            play.news_index = (play.news_index + 1) % count;
        }
        play.news_turned = Some(Instant::now());
    }
    if count > 1 {
        ctx.request_repaint_after(NEWS_TURN);
    }
    let index = if count == 0 { 0 } else { play.news_index % count };
    let item = play.news.get(index).cloned();
    let mut step: Option<isize> = None;
    let mut pick = None;
    home_card(
        ui,
        width,
        "Server news",
        (count > 1).then(|| theme::muted(format!("{} of {count}", index + 1))),
        |ui| match &item {
            None => {
                ui.label(RichText::new("No news from this server.").color(theme::SOFT));
            }
            // The server's words, shown as text only.
            Some(item) => {
                ui.label(RichText::new(hooks_config::text::clip(&item.title, 80)).family(theme::strong()).size(16.5));
                ui.add_space(2.0);
                ui.label(RichText::new(hooks_config::text::clip(&item.text, 260)).color(theme::SOFT).size(14.0));
            }
        },
        |ui| {
            ui.horizontal(|ui| {
                if count > 1 {
                    if theme::chevron(ui, true).clicked() {
                        step = Some(-1);
                    }
                    for i in 0..count.min(8) {
                        let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 20.0), egui::Sense::click());
                        let color = if i == index { theme::ACCENT } else { theme::CONTROL_LINE };
                        ui.painter().circle_filled(rect.center(), if i == index { 4.0 } else { 3.0 }, color);
                        if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            pick = Some(i);
                        }
                    }
                    if theme::chevron(ui, false).clicked() {
                        step = Some(1);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if count > 0 && ui.link("View all").clicked() {
                        *to = Some(Go::View(View::News));
                    }
                });
            });
        },
    );
    if count > 0 {
        if let Some(step) = step {
            play.news_index = (index as isize + step).rem_euclid(count as isize) as usize;
            play.news_turned = Some(Instant::now());
        }
        if let Some(i) = pick {
            play.news_index = i;
            play.news_turned = Some(Instant::now());
        }
    }
}

/// The News screen: everything the server's news says, newest first.
pub fn show_news(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        theme::page().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::display("News", 32.0));
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
            for item in &play.news {
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
}

/// The Servers screen: the network the server menu shows (and another one), a server of
/// your own by its address, or one on this network.
pub fn show_servers(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        theme::page().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (play, game, notices) = app.play_mut();
            let Some(game) = game.as_mut() else {
                ui.label(theme::display("Servers", 32.0));
                ui.label(theme::muted("Find the game first (Play)."));
                return;
            };
            play.poll(&ctx, game, notices);
            poll_lookups(play, notices);
            auto_browse(play, &ctx);
            servers_page(play, game, notices, &ctx, ui);
        });
    });
}

fn servers_page(play: &mut Play, game: &mut Game, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    let locked = game.managed.is_some();
    let playing = play.game_seen || play.running.is_some();
    ui.label(theme::display("Servers", 32.0));
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
                    url_host(&url)
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
                        .hint_text("play.example.org or 192.168.1.20")
                        .desired_width(ui.available_width() - 110.0)
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
            ui.label(theme::muted("A private server for your group, outside the network: the server menu takes you back.").small());
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
            ui.label(RichText::new("Looks for a server running at a LAN party, on this network.").color(theme::SOFT));
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
                        ui.label(RichText::new(url_host(url)).family(theme::strong()).size(18.0));
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
                    .hint_text("its address, e.g. play.mygroup.org")
                    .desired_width((ui.available_width() - 130.0).min(420.0))
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
    let server = play.server.trim().to_string();
    let current = game.cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty());
    let shown = play
        .directory
        .as_ref()
        .and_then(|d| d.iter().find(|(s, _)| s.host == server))
        .map_or_else(|| server.clone(), |(s, _)| place(s));
    let (title, done, failed) = match current.as_deref() {
        Some(c) if c == server => (format!("Setting up {shown} again"), format!("You're set up on {shown}"), format!("Couldn't set up {shown}")),
        Some(_) => (format!("Switching to {shown}"), format!("Switched to {shown}"), format!("Couldn't switch to {shown}")),
        None => (
            format!("Connecting to {shown}"),
            format!("You're set up on {shown}"),
            format!("Couldn't connect to {shown}"),
        ),
    };
    // A community server: an account is made there with the player's usual name, without asking.
    let listed = play.directory.as_ref().is_some_and(|d| d.iter().any(|(s, _)| s.host.eq_ignore_ascii_case(&server)));
    let mut plan = flow::Plan {
        game_dir: game.dir.clone(),
        server: server.clone(),
        new_name: new_name.clone(),
        public_only,
        automatic: listed,
    };
    play.switching = None;
    if let Some(id) = play.setup_failed.take() {
        notices.dismiss(id);
    }
    let log = notices.start(ctx, title);
    play.setup_failed = Some(log.id());
    play.setup_activity = Some(log.clone());
    play.last_setup = Some(SetupAsked {
        server,
        new_name,
        public_only,
        done,
        failed,
    });
    play.setup.start(ctx, move || {
        // A network's address: set up on its best server, which its directory named.
        let via_network = match flow::pick_from_network(&plan.server, &log)? {
            Some(host) => {
                plan.server = host;
                plan.public_only = true;
                plan.automatic = true;
                true
            }
            None => false,
        };
        flow::run_setup(&plan, crate::dll_utils::bundled(), &log).map(|done| (done, plan.server.clone(), via_network))
    });
}

/// Where a server is: its region ("Sydney, Australia"), else its name.
fn place(s: &setup::directory::Listing) -> String {
    if s.region.is_empty() {
        s.name.clone()
    } else {
        s.region.clone()
    }
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

fn fix_label(fix: Fix) -> &'static str {
    match fix {
        Fix::FindGame => "Find",
        Fix::InstallClient => "Install",
        Fix::ChooseServer => "Choose",
        Fix::SetUpAccount => "Connect",
        Fix::AutoAdapter => "Choose automatically",
        Fix::CreateSave => "Set up a save",
        Fix::RaiseSave => "Raise to rank 5",
        Fix::UpdateLauncher => "Update",
    }
}

fn run_fix(play: &mut Play, game: &Game, notices: &mut Notices, fix: Fix, ctx: &egui::Context) {
    let dir = game.dir.clone();
    let title = match fix {
        Fix::InstallClient => "Installing the 5th Echelon client",
        Fix::AutoAdapter => "Choosing the network adapter automatically",
        Fix::CreateSave | Fix::RaiseSave => "Fixing your save",
        Fix::FindGame | Fix::ChooseServer | Fix::SetUpAccount | Fix::UpdateLauncher => "",
    };
    match fix {
        Fix::FindGame => {}
        Fix::ChooseServer | Fix::SetUpAccount => {
            let profile = game.cfg.current_profile().cloned().unwrap_or_default();
            play.editing = true;
            play.needs_name = None;
            play.server = profile.server;
        }
        Fix::InstallClient => play.fixing.start(ctx, move || flow::install_client(&dir, crate::dll_utils::bundled())),
        Fix::AutoAdapter => play.fixing.start(ctx, move || flow::auto_adapter(&dir)),
        Fix::CreateSave | Fix::RaiseSave => play.fixing.start(ctx, move || flow::fix_save(&dir)),
        Fix::UpdateLauncher => play.update_asked = true,
    }
    if !title.is_empty() {
        play.fix_activity = Some((notices.start(ctx, title), Some(fix)));
    }
}

fn support_row(play: &mut Play, game: &Game, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    let (status, title, detail) = match &play.support {
        None | Some(Support::Supported) => return,
        Some(Support::Unsupported(why)) => (Status::Fail, "This game version isn't supported yet", why.clone()),
        Some(Support::Unknown(why)) => (Status::Warn, "Couldn't check the game version", why.clone()),
    };
    ui.horizontal_top(|ui| {
        theme::status_marker(ui, status);
        ui.vertical(|ui| {
            ui.label(title);
            ui.label(theme::muted(detail.lines().next().unwrap_or_default()).small());
            if status == Status::Fail {
                if play.identifying.running() {
                    ui.spinner();
                } else if ui.horizontal(|ui| ui.add_enabled(!play.busy(), theme::secondary("Try to identify"))).inner.clicked() {
                    let dir = game.dir.clone();
                    let version = setup::game::pick_version(&dir, game.cfg.default_game).unwrap_or(GameVersion::SplinterCellBlacklistDx11);
                    play.identifying
                        .start(ctx, move || flow::identify(&dir, version).map_err(|e| format!("Couldn't identify it: {e}")));
                    play.fix_activity = Some((notices.start(ctx, "Identifying the game's version"), None));
                }
            }
        });
    });
}

fn play_button(play: &mut Play, game: &mut Game, notices: &mut Notices, blocked: Option<&str>, ui: &mut egui::Ui) {
    let ready = !play.checks.is_empty() && setup::diagnose::ready(&play.checks) && !matches!(play.support, Some(Support::Unsupported(_)));
    if play.running.is_some() || play.game_seen {
        ui.add_enabled(false, theme::play_button("Playing"));
        return;
    }
    let button = ui.add_enabled(!play.busy() && blocked.is_none(), theme::play_button("Play"));
    let button = if let Some(why) = blocked {
        button.on_disabled_hover_text(why)
    } else if !ready && !play.checks.is_empty() {
        button.on_hover_text("Some checks failed; the game may not connect.")
    } else {
        button
    };
    if button.clicked() {
        launch(play, game, notices);
    }
}

fn launch(play: &mut Play, game: &mut Game, notices: &mut Notices) {
    // The question about diagnostics is answered first (diagnostics.rs shows it).
    if crate::diagnostics::Ask::pending(Some(game)) {
        notices.info("First, say whether the game may send its diagnostics.");
        return;
    }
    if setup::game::game_running() {
        notices.error("Splinter Cell: Blacklist is already running.");
        return;
    }
    // The hook reads uplay.toml when the game starts: make sure it has the
    // current profile.
    if game.managed.is_none() {
        if let Some(profile) = game.cfg.current_profile().cloned() {
            game.update(notices, |c| c.apply_profile(&profile));
        }
    }
    let Some(version) = setup::game::pick_version(&game.dir, game.cfg.default_game) else {
        notices.error("The game's executable is missing.");
        return;
    };
    match setup::launch::launch(&game.dir, version) {
        Ok(setup::launch::Launched::Game(child)) => play.running = Some(child),
        Ok(setup::launch::Launched::Steam) => notices.info("Steam is starting the game."),
        Err(e) => notices.error(format!("Couldn't start the game: {e}")),
    }
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
                .corner_radius(16)
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
                .corner_radius(10)
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
                    Some(url) => url_host(url).to_string(),
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
