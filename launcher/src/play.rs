//! The Play screen: join a server (Connect), see what's ready, fix
//! what isn't, and start the game.

use std::net::IpAddr;
use std::process::Child;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use eframe::egui;
use eframe::egui::RichText;
use setup::diagnose::Check;
use setup::diagnose::Fix;
use setup::diagnose::Status;
use setup::game::GameVersion;

use crate::app::App;
use crate::app::View;
use crate::app::Game;
use crate::app::Notices;
use crate::flow;
use crate::flow::Support;
use crate::task::Slot;
use crate::theme;

/// How often the checklist refreshes on its own.
const REFRESH_EVERY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Play {
    facts: Slot<(setup::diagnose::Facts, Support)>,
    /// Friends playing on other servers sharing friends, fetched with the checklist.
    fetching_elsewhere: Slot<Result<Vec<server_api::friends::FriendElsewhere>, String>>,
    elsewhere: Vec<server_api::friends::FriendElsewhere>,
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
    browsing: Slot<Result<Vec<(setup::directory::Listing, Option<u32>)>, String>>,
    directory: Option<Vec<(setup::directory::Listing, Option<u32>)>>,
    /// The directory was browsed on its own since the last setup: once is enough.
    browsed: bool,
    /// The server field holds a server the launcher picked, not one typed:
    /// a newer pick may replace it.
    server_picked: bool,
    /// Why the directory couldn't be read, shown quietly with the servers.
    directory_error: Option<String>,

    setup: Slot<Result<flow::Done, String>>,
    /// A switch to a server another server named, waiting for the player to confirm it.
    switching: Option<Switch>,
    log: flow::Log,
    setup_error: Option<String>,
    fixing: Slot<Result<String, String>>,
    identifying: Slot<Result<(), String>>,
    running: Option<Child>,
    /// The game runs (started through Steam, or by hand); checked every few
    /// seconds.
    game_seen: bool,
    game_checked: Option<Instant>,
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
}

/// A server to switch to, from the network's list or a friend's.
#[derive(Debug, Clone)]
struct Switch {
    /// Which card asks.
    from: SwitchFrom,
    host: String,
    /// The name for an account made there, if the player has none (None: the setup asks).
    new_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwitchFrom {
    Network,
    Friend,
}

impl Play {
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
            .get_or_insert_with(|| {
                setup::player_identity::load()
                    .ok()
                    .flatten()
                    .map(|id| identity::short(&id.global_id()))
            })
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
        if !self.fetching_elsewhere.running() && game.managed.is_none() {
            let cfg = cfg.clone();
            self.fetching_elsewhere.start(ctx, move || flow::friends_elsewhere(&cfg));
        }
        self.facts.start(ctx, move || flow::gather(&dir, &cfg, crate::dll_utils::bundled()));
    }

    fn poll(&mut self, ctx: &egui::Context, game: &Game, notices: &mut Notices) {
        // A failed fetch keeps the last list: it's a hint, not a check.
        if let Some(Ok(elsewhere)) = self.fetching_elsewhere.poll() {
            self.elsewhere = elsewhere;
        }
        if let Some((facts, support)) = self.facts.poll() {
            self.checks = setup::diagnose::checklist(&facts);
            self.support = Some(support);
            self.refreshed = Some(Instant::now());
        }
        let mut changed = false;
        if let Some(result) = self.setup.poll() {
            match result {
                Ok(flow::Done::Ready) => {
                    flow::forget_account_check();
                    notices.info("You're set up.");
                    self.editing = false;
                    self.needs_name = None;
                    // The setup may have brought a directory: look at its servers again.
                    self.browsed = false;
                    self.directory = None;
                    self.setup_error = None;
                }
                Ok(flow::Done::NeedsName { server, suggested }) => {
                    // The network's best server, or the one typed: the account is made there.
                    self.server = server.clone();
                    self.server_picked = false;
                    self.nick = suggested;
                    self.needs_name = Some(server);
                    self.editing = true;
                    self.setup_error = None;
                }
                Err(e) => self.setup_error = Some(e),
            }
            changed = true;
        }
        if let Some(result) = self
            .fixing
            .poll()
            .or_else(|| self.identifying.poll().map(|r| r.map(|()| "The game version is now supported.".to_string())))
        {
            match result {
                Ok(msg) => notices.info(msg),
                Err(e) => notices.error(e),
            }
            changed = true;
        }
        if let Some(Ok(Some(_))) = self.running.as_mut().map(Child::try_wait) {
            self.running = None;
            changed = true;
        }
        if self.game_checked.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.game_checked = Some(Instant::now());
            let seen = setup::game::game_running();
            changed |= self.game_seen && !seen;
            self.game_seen = seen;
        }
        let stale = self.refreshed.is_none_or(|t| t.elapsed() > REFRESH_EVERY);
        if (changed || stale) && !self.busy() {
            self.refresh(ctx, game);
        }
    }
}

/// The servers that answered on this network, when several did: the player
/// picks the one they meant.
fn found_list(play: &mut Play, ui: &mut egui::Ui) {
    if play.found.is_empty() {
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
            Ok(servers) => {
                if play.server.trim().is_empty() || play.server_picked {
                    if let Some(best) = setup::directory::best(&servers) {
                        play.server = servers[best].0.host.clone();
                        play.server_picked = true;
                    }
                }
                play.directory = Some(servers);
                play.directory_error = None;
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
        play.browsing.start(ctx, move || crate::services::rt().block_on(crate::network::server_directory(&url)));
    }
}

/// Where a screen asks to go next (a card's link to another screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Go {
    View(View),
    NetworkSettings,
}

fn go(app: &mut App, to: Option<Go>) {
    match to {
        Some(Go::View(view)) => app.set_view(view),
        Some(Go::NetworkSettings) => app.open_network_settings(),
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
            setup_screen(play, game, &ctx, ui);
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
                            theme::mark(ui, 12.0);
                            ui.add_space(14.0);
                            ui.label(theme::display("Get online in a minute", 28.0));
                            ui.add_space(6.0);
                            ui.label(RichText::new("The launcher sets everything up and keeps the game's original files, so you can undo it any time. You need your own copy of the game.").color(theme::SOFT));
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
                    ui.painter().set(fill, egui::Shape::rect_filled(side, egui::CornerRadius { nw: 18, ne: 0, sw: 18, se: 0 }, theme::SUNKEN));
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
fn poll_lookups(play: &mut Play) {
    if let Some(found) = play.looking.poll() {
        match found {
            Ok(ips) if ips.len() == 1 => {
                play.server = ips[0].to_string();
                play.address = ips[0].to_string();
                play.server_picked = false;
                play.found.clear();
            }
            Ok(ips) => play.found = ips,
            Err(e) => play.setup_error = Some(e),
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
    play.browsing.start(ctx, move || crate::services::rt().block_on(crate::network::server_directory(&url)));
}

/// The guided setup's server and name steps (the setup itself installs the
/// client, makes or finds the account and pins the adapter).
fn setup_screen(play: &mut Play, game: &mut Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    poll_lookups(play);
    // With a directory, its servers are pinged as soon as the form opens, and the best preselected.
    auto_browse(play, ctx);
    let naming = play.needs_name.is_some();
    wizard(ui, if naming { 2 } else { 1 }, |ui| {
        ui.label(RichText::new(if naming { "Pick your name" } else { "Choose a server" }).family(theme::strong()).size(24.0));
        ui.add_space(4.0);
        if let Some(server) = &play.needs_name {
            ui.label(RichText::new(format!("You don't have an account on {server} yet. Choose the name other players will see; your identity signs you in to it from now on.")).color(theme::SOFT));
        } else {
            ui.label(RichText::new("Pick the closest server, or type a server's or a network's address. Connecting also installs the 5th Echelon client, keeping the game's original file so you can undo it.").color(theme::SOFT));
        }
        ui.add_space(14.0);
        egui::Frame::new().fill(theme::SUNKEN).stroke(egui::Stroke::new(1.0, theme::LINE)).corner_radius(12).inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
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
                ui.add(egui::TextEdit::singleline(&mut play.nick).hint_text("what other players see").desired_width(320.0).min_size(egui::vec2(0.0, 40.0)));
            } else {
                server_choices(play, ui);
                ui.add_space(10.0);
                ui.label(theme::caps("Or a server's address"));
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::TextEdit::singleline(&mut play.server).hint_text("play.example.org or 192.168.1.20").desired_width(300.0).min_size(egui::vec2(0.0, 38.0)))
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
                .add_enabled(ready, theme::primary(if naming { "Create account" } else { "Connect" }).min_size(egui::vec2(170.0, 48.0)).corner_radius(12))
                .clicked()
            {
                let name = naming.then(|| play.nick.trim().to_string());
                start_setup(play, game, ctx, name, false);
            }
            if game.cfg.current_profile().is_some_and(|p| !p.server.is_empty())
                && !play.setup.running()
                && ui.add(theme::secondary("Cancel").min_size(egui::vec2(0.0, 48.0))).clicked()
            {
                play.editing = false;
                play.needs_name = None;
            }
        });
        ui.add_space(6.0);
        setup_progress(play, ui);
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
    for (i, (s, ping)) in servers.iter().enumerate() {
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
        let response = ui.interact(inner.response.rect, ui.id().with(("server", i)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
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

/// A ping, coloured by how it plays: green is good, amber is far.
fn ping_text(ping: Option<u32>) -> RichText {
    match ping {
        None => theme::muted("no answer"),
        Some(ms) => RichText::new(format!("{ms} ms")).monospace().color(ping_color(ms)),
    }
}

fn ping_color(ms: u32) -> egui::Color32 {
    match ms {
        0..=90 => theme::OK,
        91..=180 => theme::FG,
        _ => theme::WARN,
    }
}

/// A small label on a server: filled for the one to pick, outlined otherwise.
fn badge(ui: &mut egui::Ui, text: &str, strong: bool) {
    let frame = egui::Frame::new().corner_radius(6).inner_margin(egui::Margin::symmetric(8, 2));
    let frame = if strong { frame.fill(theme::ACCENT) } else { frame.stroke(egui::Stroke::new(1.0, theme::CONTROL_LINE)) };
    frame.show(ui, |ui| {
        ui.label(RichText::new(text.to_uppercase()).size(10.5).extra_letter_spacing(1.0).color(if strong { theme::ON_ACCENT } else { theme::MUTED }));
    });
}

/// The home screen: the game's banner, the bar that starts it, and cards
/// for friends, the checks and the server.
fn home(play: &mut Play, game: &mut Game, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) -> Option<Go> {
    let mut to = None;
    if let Some(managed) = &game.managed {
        egui::Frame::new().fill(theme::ACCENT.linear_multiply(0.12)).inner_margin(egui::Margin::symmetric(32, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("{} sets up this install. Change the server or account there; you can still play from here.", managed.by)));
        });
    }
    load_art(play, game, ctx);
    load_news(play, game, ctx);
    hero(play, ui);
    launch_bar(play, game, notices, &mut to, ui);
    theme::page().show(ui, |ui| {
        let width = ui.available_width();
        let columns = if width >= 960.0 { 3 } else if width >= 620.0 { 2 } else { 1 };
        let news = !play.news.is_empty();
        ui.columns(columns, |cols| match (columns, news) {
            (3, true) => {
                status_card(play, game, ctx, &mut to, &mut cols[0]);
                friends_card(play, game, ctx, &mut cols[1]);
                server_overview(play, game, &mut to, &mut cols[1]);
                news_card(play, &mut cols[2]);
            }
            (3, false) => {
                status_card(play, game, ctx, &mut to, &mut cols[0]);
                friends_card(play, game, ctx, &mut cols[1]);
                server_overview(play, game, &mut to, &mut cols[2]);
            }
            (2, _) => {
                status_card(play, game, ctx, &mut to, &mut cols[0]);
                news_card(play, &mut cols[0]);
                friends_card(play, game, ctx, &mut cols[1]);
                server_overview(play, game, &mut to, &mut cols[1]);
            }
            _ => {
                status_card(play, game, ctx, &mut to, &mut cols[0]);
                friends_card(play, game, ctx, &mut cols[0]);
                server_overview(play, game, &mut to, &mut cols[0]);
                news_card(play, &mut cols[0]);
            }
        });
        if play.setup.running() || play.setup_error.is_some() {
            ui.add_space(10.0);
            setup_progress(play, ui);
        }
    });
    to
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
/// night-vision grid until there is one), with how ready everything is in the
/// corner.
fn hero(play: &Play, ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 320.0), egui::Sense::hover());
    theme::grid_backdrop(ui.painter(), rect);
    if let Some(texture) = &play.art_texture {
        theme::cover_image(ui.painter(), rect, texture);
        theme::scrim(ui.painter(), rect);
    }
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], egui::Stroke::new(1.0, theme::LINE));
    let inner = rect.shrink2(egui::vec2(36.0, 26.0));
    let mut top = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Min)));
    top.label(theme::caps(env!("FE_PRODUCT")));
    top.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        let (color, text) = readiness(play);
        theme::pill(ui, color, &text);
    });
    let mut bottom = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::bottom_up(egui::Align::LEFT)));
    bottom.label(RichText::new("Co-op and Spies vs Mercs on community servers. No VPN, no port forwarding.").size(16.0).color(theme::SOFT));
    bottom.add(egui::Label::new(theme::display("Splinter Cell: Blacklist", if width < 700.0 { 34.0 } else { 50.0 })).wrap());
}

/// How ready everything is, for the banner's corner.
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

/// The bar under the banner: the server, who you are, and Play.
fn launch_bar(play: &mut Play, game: &mut Game, notices: &mut Notices, to: &mut Option<Go>, ui: &mut egui::Ui) {
    let profile = game.cfg.current_profile().cloned().unwrap_or_default();
    egui::Frame::new().fill(theme::SUNKEN).inner_margin(egui::Margin::symmetric(32, 16)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let wide = ui.available_width() >= 820.0;
        ui.horizontal(|ui| {
            // The server: click for the Servers screen.
            let listing = play.directory.as_ref().and_then(|d| d.iter().find(|(s, _)| s.host == profile.server));
            let name = listing.map(|(s, _)| server_name(s)).unwrap_or_else(|| profile.server.clone());
            let chip = egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0, theme::LINE))
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.set_min_width(if wide { 260.0 } else { 200.0 });
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(theme::caps("Server"));
                            ui.label(RichText::new(hooks_config::text::clip(&name, 40)).family(theme::strong()));
                        });
                        if let Some(ping) = listing.and_then(|(_, p)| *p) {
                            ui.add_space(12.0);
                            ui.label(ping_text(Some(ping)));
                        }
                    });
                });
            if ui
                .interact(chip.response.rect, ui.id().with("server-chip"), egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Servers")
                .clicked()
            {
                *to = Some(Go::View(View::Servers));
            }
            if wide && !profile.user.username.is_empty() {
                let identity = play.identity_short().unwrap_or_default();
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                    .corner_radius(12)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 18.0, theme::CONTROL);
                            let initial = profile.user.username.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::new(16.0, theme::strong()), theme::ACCENT);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(hooks_config::text::clip(&profile.user.username, 32)).family(theme::strong()));
                                if !identity.is_empty() {
                                    ui.label(theme::muted(format!("Identity {identity}")).small().monospace());
                                }
                            });
                        });
                    });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                play_button(play, game, notices, ui);
                renderer(play, game, notices, ui);
            });
        });
    });
}

/// DirectX 11 or 9, when both are installed.
fn renderer(play: &mut Play, game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) {
    let installed = setup::game::installed_versions(&game.dir);
    if installed.len() < 2 {
        return;
    }
    let mut version = setup::game::pick_version(&game.dir, game.cfg.default_game).unwrap_or(game.cfg.default_game);
    egui::ComboBox::from_id_salt("version").selected_text(version.label()).width(130.0).show_ui(ui, |ui| {
        for v in &installed {
            ui.selectable_value(&mut version, *v, v.label());
        }
    });
    if version != game.cfg.default_game {
        game.update(notices, |c| c.default_game = version);
        play.refreshed = None;
    }
}

/// A card on the home screen, with a small uppercase title.
fn home_card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        // Left-aligned, not the columns' justified layout (which spreads words and stretches buttons).
        ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
            ui.label(theme::caps(title));
            ui.add_space(6.0);
            body(ui);
        });
    });
    ui.add_space(14.0);
}

/// The checks: anything that needs fixing, with its fix; when all is well,
/// a short summary that opens into the full list.
fn status_card(play: &mut Play, game: &mut Game, ctx: &egui::Context, to: &mut Option<Go>, ui: &mut egui::Ui) {
    home_card(ui, "Status", |ui| {
        if play.checks.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted("Checking…"));
            });
            return;
        }
        let issues = play.checks.iter().any(|c| c.status != Status::Ok);
        let fixable = play.checks.iter().any(|c| c.status == Status::Fail && c.fix.is_some());
        let has_server = game.cfg.current_profile().is_some_and(|p| !p.server.is_empty());
        // All of them when asked; else the problems, or a few that matter when there are none.
        const SUMMARY: [&str; 3] = ["client", "account", "save"];
        let checks: Vec<Check> = play
            .checks
            .iter()
            .filter(|c| play.show_all_checks || if issues { c.status != Status::Ok } else { SUMMARY.contains(&c.id) })
            .cloned()
            .collect();
        for check in &checks {
            ui.horizontal_top(|ui| {
                theme::status_marker(ui, check.status);
                ui.vertical(|ui| {
                    ui.label(&check.title);
                    if !check.detail.is_empty() && check.status != Status::Ok {
                        ui.label(theme::muted(check.detail.as_str()).small());
                    }
                    // Under the problem, so a narrow card never has to fit it beside it.
                    if let Some(fix) = check.fix.filter(|_| check.status != Status::Ok && game.managed.is_none()) {
                        if ui.horizontal(|ui| ui.add_enabled(!play.busy(), theme::secondary(fix_label(fix)))).inner.clicked() {
                            run_fix(play, game, fix, ctx);
                        }
                    }
                });
            });
        }
        support_row(play, game, ctx, ui);
        if fixable && has_server && game.managed.is_none() && !play.busy() {
            ui.add_space(4.0);
            if ui.horizontal(|ui| ui.add(theme::primary("Fix everything").min_size(egui::vec2(0.0, 40.0)).corner_radius(10))).inner.clicked() {
                let profile = game.cfg.current_profile().cloned().unwrap_or_default();
                play.server = profile.server;
                let name = Some(profile.user.username).filter(|n| !n.is_empty());
                start_setup(play, game, ctx, name, false);
            }
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui.link(if play.show_all_checks { "Show less" } else { "Show all checks" }).clicked() {
                play.show_all_checks = !play.show_all_checks;
            }
            ui.label(theme::muted("·"));
            if play.facts.running() || play.busy() {
                ui.spinner();
            } else if ui.link("Check again").clicked() {
                play.refresh(ctx, game);
            }
            ui.label(theme::muted("·"));
            if ui.link("Connection test").clicked() {
                *to = Some(Go::NetworkSettings);
            }
        });
    });
}

/// Friends playing on another server of the network, and a way to join them
/// there: players on different servers can't see or invite each other.
fn friends_card(play: &mut Play, game: &Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    let playing = play.game_seen || play.running.is_some();
    let mut switch_to = None;
    home_card(ui, "Friends", |ui| {
        if play.elsewhere.is_empty() || game.managed.is_some() {
            ui.label(RichText::new("Friends, friend requests and invites are in the game: press F5 for the overlay.").color(theme::SOFT));
            return;
        }
        ui.label(RichText::new("On other servers. You only see and invite friends on your own server, so one of you joins the other's.").color(theme::SOFT));
        ui.add_space(4.0);
        // Names and servers are cut and cleaned when fetched (`flow::friends_elsewhere`).
        for friend in &play.elsewhere {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, theme::WARN);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&friend.username).family(theme::strong()));
                    let server = if friend.region.is_empty() { friend.server.clone() } else { format!("{} ({})", friend.server, friend.region) };
                    // The address it goes to, in full: the server's name and region are only its word.
                    ui.label(theme::muted(format!("on {server}")).small())
                        .on_hover_text(hooks_config::text::clip(&friend.host, 64));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Only a server on the internet: one server can't send players into their own network.
                    let public = setup::directory::listable_host(&friend.host);
                    let button = ui.add_enabled(public && !playing && !play.setup.running(), theme::secondary("Switch"));
                    let button = if !public {
                        button.on_disabled_hover_text("Not a server address on the internet")
                    } else if playing {
                        button.on_disabled_hover_text("Quit the game first")
                    } else {
                        button
                    };
                    if button.clicked() {
                        switch_to = Some(friend.host.clone());
                    }
                });
            });
        }
        confirm_switch(play, game, SwitchFrom::Friend, ctx, ui);
    });
    if let Some(host) = switch_to {
        play.switching = Some(Switch { from: SwitchFrom::Friend, host, new_name: None });
    }
}

/// The server's news: what its game news screen says too.
fn news_card(play: &Play, ui: &mut egui::Ui) {
    if play.news.is_empty() {
        return;
    }
    home_card(ui, "Server news", |ui| {
        for (i, item) in play.news.iter().take(4).enumerate() {
            if i > 0 {
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(2.0);
            }
            // The server's words, shown as text only.
            ui.label(RichText::new(hooks_config::text::clip(&item.title, 80)).family(theme::strong()));
            if !item.text.is_empty() {
                ui.label(RichText::new(hooks_config::text::clip(&item.text, 400)).color(theme::SOFT).size(14.0));
            }
            // Only a web address opens, and the player sees where it goes.
            if item.link.starts_with("https://") {
                ui.hyperlink_to("Read more", &item.link).on_hover_text(hooks_config::text::clip(&item.link, 120));
            }
        }
    });
}

/// The server you play on: its numbers, and the way to another.
fn server_overview(play: &mut Play, game: &Game, to: &mut Option<Go>, ui: &mut egui::Ui) {
    let Some(profile) = game.cfg.current_profile().cloned() else { return };
    home_card(ui, "Server", |ui| {
        let listing = play.directory.as_ref().and_then(|d| d.iter().find(|(s, _)| s.host == profile.server)).cloned();
        match &listing {
            Some((s, _)) => {
                ui.label(RichText::new(server_name(s)).family(theme::strong()).size(17.0));
            }
            None => {
                ui.label(RichText::new(profile.server.as_str()).family(theme::strong()).size(17.0));
            }
        }
        let who = if profile.user.username.is_empty() { "no account yet".to_string() } else { format!("as {}", profile.user.username) };
        let over = profile.adapter.as_deref().map(|a| format!(" · over \"{a}\"")).unwrap_or_default();
        ui.label(theme::muted(format!("{} · {who}{over}", profile.server)).small());
        if let Some((s, ping)) = &listing {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                theme::stat(ui, "Ping", &ping.map_or_else(|| "–".into(), |ms| format!("{ms} ms")), ping.map_or(theme::MUTED, ping_color));
                theme::stat(ui, "Online", &s.players_online.to_string(), theme::FG);
            });
        }
        if game.managed.is_none() {
            ui.add_space(6.0);
            if ui.horizontal(|ui| ui.add(theme::secondary("Change server"))).inner.clicked() {
                *to = Some(Go::View(View::Servers));
            }
        }
    });
}

/// The Servers screen: the network's servers with your ping to each, a server
/// by its address, or one on this network.
pub fn show_servers(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut to = None;
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
            poll_lookups(play);
            auto_browse(play, &ctx);
            to = servers_page(play, game, &ctx, ui);
        });
    });
    go(app, to);
}

fn servers_page(play: &mut Play, game: &mut Game, ctx: &egui::Context, ui: &mut egui::Ui) -> Option<Go> {
    let mut to = None;
    let current = game.cfg.current_profile().map(|p| p.server.clone()).unwrap_or_default();
    let locked = game.managed.is_some();
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(theme::display("Servers", 32.0));
            match crate::app::Prefs::directory() {
                Some(url) => ui.label(theme::muted(format!("Network {} · your friends and identity follow you between these servers", url.trim_start_matches("https://").trim_end_matches('/')))),
                None => ui.label(theme::muted("No server network yet: join a server by its address, or find one on your network.")),
            };
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if play.browsing.running() {
                ui.spinner();
            } else if let Some(url) = crate::app::Prefs::directory().filter(|_| ui.add(theme::secondary("Ping again")).clicked()) {
                ping_again(play, ctx, url);
            }
        });
    });
    if locked {
        ui.label(theme::muted("Another tool manages this install: change the server there."));
    }
    ui.add_space(14.0);
    if let Some(e) = &play.directory_error {
        ui.label(theme::muted(e.as_str()).small());
    }
    let playing = play.game_seen || play.running.is_some();
    if let Some(servers) = play.directory.clone() {
        let best = setup::directory::best(&servers);
        let columns = if ui.available_width() >= 760.0 { 2 } else { 1 };
        let mut switch_to = None;
        ui.columns(columns, |cols| {
            for (i, (s, ping)) in servers.iter().enumerate() {
                let ui = &mut cols[i % columns];
                let here = s.host == current;
                let friends_here = play.elsewhere.iter().filter(|f| f.host == s.host).count();
                theme::card()
                    .stroke(egui::Stroke::new(if here { 2.0 } else { 1.0 }, if here { theme::ACCENT } else { theme::LINE }))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if best == Some(i) {
                                badge(ui, "Best for you", true);
                            }
                            if s.friends_mode == "mutual" {
                                badge(ui, "Friends only", false);
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if here {
                                    ui.label(RichText::new("Connected").color(theme::OK));
                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                                    ui.painter().circle_filled(rect.center(), 4.0, theme::OK);
                                } else if !locked {
                                    let label = if current.is_empty() { "Connect" } else { "Switch" };
                                    let button = ui.add_enabled(!playing && !play.setup.running(), theme::secondary(label));
                                    let button = if playing { button.on_disabled_hover_text("Quit the game first") } else { button };
                                    if button.clicked() {
                                        switch_to = Some(s.host.clone());
                                    }
                                }
                            });
                        });
                        ui.add_space(4.0);
                        ui.label(RichText::new(if s.region.is_empty() { s.name.clone() } else { s.region.clone() }).family(theme::strong()).size(19.0));
                        let sub = if s.region.is_empty() { s.host.clone() } else { format!("{} · {}", s.name, s.host) };
                        ui.label(theme::muted(sub).small());
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            theme::stat(ui, "Ping", &ping.map_or_else(|| "–".into(), |ms| format!("{ms} ms")), ping.map_or(theme::MUTED, ping_color));
                            theme::stat(ui, "Online", &s.players_online.to_string(), theme::FG);
                            if friends_here > 0 {
                                theme::stat(ui, "Friends", &friends_here.to_string(), theme::FG);
                            }
                        });
                    });
                ui.add_space(14.0);
            }
        });
        if let Some(host) = switch_to {
            if current.is_empty() {
                // No server yet: the guided setup takes it from here.
                play.server = host;
                play.server_picked = false;
                play.editing = true;
                start_setup(play, game, ctx, None, false);
                to = Some(Go::View(View::Play));
            } else {
                // The same name on the network's other server: made there if need be, once confirmed.
                let new_name = game.cfg.current_profile().map(|p| p.user.username.clone()).filter(|n| !n.is_empty());
                play.switching = Some(Switch { from: SwitchFrom::Network, host, new_name });
            }
        }
        confirm_switch(play, game, SwitchFrom::Network, ctx, ui);
    } else if play.browsing.running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(theme::muted("Pinging the servers in this network…"));
        });
    }
    if play.setup.running() || play.setup_error.is_some() {
        setup_progress(play, ui);
    }
    if locked {
        return to;
    }
    ui.add_space(8.0);
    let columns = if ui.available_width() >= 760.0 { 2 } else { 1 };
    ui.columns(columns, |cols| {
        let ui = &mut cols[0];
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::caps("Join by address"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut play.address).hint_text("play.example.org or 192.168.1.20").desired_width(ui.available_width() - 110.0).min_size(egui::vec2(0.0, 38.0)));
                let ready = !play.address.trim().is_empty() && !play.setup.running() && !playing;
                if ui.add_enabled(ready, theme::primary("Connect").min_size(egui::vec2(90.0, 38.0))).clicked() {
                    play.server = play.address.trim().to_string();
                    play.server_picked = false;
                    play.needs_name = None;
                    play.editing = true;
                    let name = game.cfg.current_profile().map(|p| p.user.username.clone()).filter(|n| !n.is_empty());
                    start_setup(play, game, ctx, name, false);
                    to = Some(Go::View(View::Play));
                }
            });
            ui.label(theme::muted("A private server for your group, or another community network.").small());
        });
        let ui = &mut cols[1 % columns];
        if columns == 1 {
            ui.add_space(14.0);
        }
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::caps("On your network"));
            ui.add_space(6.0);
            ui.label(RichText::new("Looks for a server running at a LAN party.").color(theme::SOFT));
            if play.looking.running() {
                ui.spinner();
            } else if ui.horizontal(|ui| ui.add(theme::secondary("Find on my network"))).inner.clicked() {
                find_on_network(play, ctx);
            }
            if !play.found.is_empty() {
                ui.label(theme::muted("Several servers answered. Choose one:"));
                let mut pick = None;
                ui.horizontal_wrapped(|ui| {
                    for ip in &play.found {
                        if ui.button(ip.to_string()).clicked() {
                            pick = Some(*ip);
                        }
                    }
                });
                if let Some(ip) = pick {
                    play.address = ip.to_string();
                    play.found.clear();
                }
            }
        });
    });
    to
}

/// Runs the setup; `new_name` names the account if the player has none on
/// the server (None: the setup asks). `public_only` when another server named
/// it (see [`flow::Plan`]).
fn start_setup(play: &mut Play, game: &Game, ctx: &egui::Context, new_name: Option<String>, public_only: bool) {
    let mut plan = flow::Plan {
        game_dir: game.dir.clone(),
        server: play.server.trim().to_string(),
        new_name,
        public_only,
    };
    play.setup_error = None;
    play.switching = None;
    play.log = Arc::default();
    let log = Arc::clone(&play.log);
    play.setup.start(ctx, move || {
        // A network's address: set up on its best server, which its directory named.
        if let Some(host) = flow::pick_from_network(&plan.server, &log)? {
            plan.server = host;
            plan.public_only = true;
        }
        flow::run_setup(&plan, crate::dll_utils::bundled(), &log)
    });
}

/// Asks before switching to a server another server named: its address in full, and what
/// happens there. Shown in the card that asked.
fn confirm_switch(play: &mut Play, game: &Game, from: SwitchFrom, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some(switch) = play.switching.clone().filter(|s| s.from == from) else {
        return;
    };
    ui.add_space(6.0);
    let account = match &switch.new_name {
        Some(name) => format!("signs you in there with your identity, as {name} (an account is made there if you have none)"),
        None => String::from("signs you in there with your identity (if you have no account there, you choose a name first)"),
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("Switch to {}? This {account}.", switch.host)).color(theme::WARN));
        if ui.button(format!("Switch to {}", switch.host)).clicked() {
            play.server = switch.host.clone();
            play.server_picked = false;
            play.needs_name = None;
            start_setup(play, game, ctx, switch.new_name.clone(), true);
        }
        if ui.button("Cancel").clicked() {
            play.switching = None;
        }
    });
}

fn setup_progress(play: &Play, ui: &mut egui::Ui) {
    let log = play.log.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for line in log.iter() {
        ui.label(theme::muted(line.as_str()));
    }
    if let Some(e) = &play.setup_error {
        ui.label(RichText::new(e).color(theme::BAD));
    }
}

fn fix_label(fix: Fix) -> &'static str {
    match fix {
        Fix::FindGame => "Find",
        Fix::InstallClient => "Install",
        Fix::ChooseServer => "Choose",
        Fix::SetUpAccount => "Connect",
        Fix::PinAdapter => "Pin",
        Fix::CreateSave => "Create",
        Fix::RaiseSave => "Raise to rank 5",
    }
}

fn run_fix(play: &mut Play, game: &Game, fix: Fix, ctx: &egui::Context) {
    let dir = game.dir.clone();
    match fix {
        Fix::FindGame => {}
        Fix::ChooseServer | Fix::SetUpAccount => {
            let profile = game.cfg.current_profile().cloned().unwrap_or_default();
            play.editing = true;
            play.needs_name = None;
            play.server = profile.server;
        }
        Fix::InstallClient => play.fixing.start(ctx, move || flow::install_client(&dir, crate::dll_utils::bundled())),
        Fix::PinAdapter => play.fixing.start(ctx, move || flow::pin_adapter(&dir)),
        Fix::CreateSave | Fix::RaiseSave => play.fixing.start(ctx, move || flow::fix_save(&dir)),
    }
}

fn support_row(play: &mut Play, game: &Game, ctx: &egui::Context, ui: &mut egui::Ui) {
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
                }
            }
        });
    });
}

fn play_button(play: &mut Play, game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) {
    let ready = !play.checks.is_empty() && setup::diagnose::ready(&play.checks) && !matches!(play.support, Some(Support::Unsupported(_)));
    if play.running.is_some() || play.game_seen {
        ui.add_enabled(false, theme::play_button("Playing"));
        return;
    }
    let button = ui.add_enabled(!play.busy(), theme::play_button("Play"));
    let button = if !ready && !play.checks.is_empty() { button.on_hover_text("Some checks failed; the game may not connect.") } else { button };
    if button.clicked() {
        launch(play, game, notices);
    }
}

fn launch(play: &mut Play, game: &mut Game, notices: &mut Notices) {
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
