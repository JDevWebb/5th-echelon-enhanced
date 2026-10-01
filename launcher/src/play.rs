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
    log: flow::Log,
    setup_error: Option<String>,
    fixing: Slot<Result<String, String>>,
    identifying: Slot<Result<(), String>>,
    running: Option<Child>,
    /// The game runs (started through Steam, or by hand); checked every few
    /// seconds.
    game_seen: bool,
    game_checked: Option<Instant>,
}

impl Play {
    pub fn game_changed(&mut self) {
        self.checks.clear();
        self.support = None;
        self.refreshed = None;
        self.editing = false;
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
        self.facts.start(ctx, move || flow::gather(&dir, &cfg, crate::dll_utils::bundled()));
    }

    fn poll(&mut self, ctx: &egui::Context, game: &Game, notices: &mut Notices) {
        if let Some((facts, support)) = self.facts.poll() {
            self.checks = setup::diagnose::checklist(&facts);
            self.support = Some(support);
            self.refreshed = Some(Instant::now());
        }
        let mut changed = false;
        if let Some(result) = self.setup.poll() {
            match result {
                Ok(flow::Done::Ready) => {
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

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if app.game.is_none() {
        no_game(app, ui);
        return;
    }
    let (play, game, notices) = app.play_mut();
    let game = game.as_mut().expect("checked above");
    play.poll(&ctx, game, notices);

    if let Some(managed) = &game.managed {
        theme::card().fill(theme::ACCENT.linear_multiply(0.12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::heading(&format!("Managed by {}", managed.by)));
            ui.label(theme::muted(format!(
                "{} sets up this install. Change the server or account there; you can still play from here.",
                managed.by
            )));
        });
        ui.add_space(10.0);
    }

    game_card(play, game, notices, ui);
    ui.add_space(10.0);

    let has_server = game.cfg.current_profile().is_some_and(|p| !p.server.is_empty());
    if game.managed.is_none() && (!has_server || play.editing) {
        join_card(play, game, &ctx, ui);
    } else {
        server_card(play, game, &ctx, ui);
    }
    ui.add_space(10.0);

    checklist_card(play, game, &ctx, ui);
    ui.add_space(14.0);
    play_button(play, game, notices, ui);
}

fn no_game(app: &mut App, ui: &mut egui::Ui) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        if app.finding_games() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Looking for Splinter Cell: Blacklist…");
            });
            return;
        }
        ui.label(theme::heading("Splinter Cell: Blacklist not found"));
        ui.label(theme::muted(
            "It wasn't in Steam, Ubisoft Connect or the usual folders. Choose the folder you installed it to.",
        ));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add(theme::primary("Choose folder…")).clicked() {
                app.pick_game_folder();
            }
            if ui.button("Look again").clicked() {
                let ctx = ui.ctx().clone();
                app.find_games(&ctx);
            }
        });
    });
}

fn game_card(play: &mut Play, game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(theme::heading("Splinter Cell: Blacklist"));
                ui.label(theme::muted(game.dir.display().to_string()).small());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let installed = setup::game::installed_versions(&game.dir);
                let mut version = setup::game::pick_version(&game.dir, game.cfg.default_game).unwrap_or(game.cfg.default_game);
                ui.add_enabled_ui(installed.len() > 1, |ui| {
                    egui::ComboBox::from_id_salt("version").selected_text(version.label()).show_ui(ui, |ui| {
                        for v in &installed {
                            ui.selectable_value(&mut version, *v, v.label());
                        }
                    });
                });
                if version != game.cfg.default_game {
                    game.update(notices, |c| c.default_game = version);
                    play.refreshed = None;
                }
            });
        });
    });
}

fn join_card(play: &mut Play, game: &mut Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    if let Some(found) = play.looking.poll() {
        match found {
            Ok(ips) if ips.len() == 1 => {
                play.server = ips[0].to_string();
                play.found.clear();
            }
            Ok(ips) => play.found = ips,
            Err(e) => play.setup_error = Some(e),
        }
    }
    poll_directory(play);
    // With a directory, its servers are pinged as soon as the form opens, and the best preselected.
    auto_browse(play, ctx);
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(theme::heading("Join a server"));
        ui.label(theme::muted(
            "A server's or a network's address (e.g. play.scbl.jdevwebb.net for the community), or find one on your network.",
        ));
        ui.add_space(4.0);
        ui.add_enabled_ui(!play.setup.running(), |ui| {
            egui::Grid::new("join").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label("Server");
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut play.server)
                                .hint_text("e.g. 10.8.0.10 or play.example.org")
                                .desired_width(260.0),
                        )
                        .changed()
                    {
                        play.server_picked = false;
                        play.needs_name = None;
                    }
                    if play.looking.running() || play.browsing.running() {
                        ui.spinner();
                    } else if let Some(url) = crate::app::Prefs::directory().filter(|_| ui.button("Ping servers again").clicked()) {
                        play.browsing.start(ctx, move || crate::services::rt().block_on(crate::network::server_directory(&url)));
                    } else if ui.button("Find on my network").clicked() {
                        play.looking.start(ctx, || {
                            let adapters = setup::net::adapters();
                            let refs: Vec<(&str, IpAddr)> = adapters.iter().map(|(n, ip)| (n.as_str(), *ip)).collect();
                            crate::services::rt()
                                .block_on(crate::network::try_locate_server(None, &refs))
                                .map_err(|_| "No server answered on this network.".to_string())
                        });
                    }
                });
                ui.end_row();

                if play.needs_name.is_some() {
                    ui.label("Your name");
                    ui.add(egui::TextEdit::singleline(&mut play.nick).hint_text("what other players see").desired_width(260.0));
                    ui.end_row();
                }
            });
        });
        if let Some(server) = &play.needs_name {
            ui.label(theme::muted(format!(
                "You don't have an account on {server} yet. Choose the name other players will see; your identity signs you in to it from now on."
            )));
        } else {
            ui.label(theme::muted("Your account is found with your identity: no username or password to remember.").small());
        }
        found_list(play, ui);
        directory_list(play, ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let naming = play.needs_name.is_some();
            let ready = !play.server.trim().is_empty() && (!naming || !play.nick.trim().is_empty());
            if play.setup.running() {
                ui.spinner();
                ui.label(if naming { "Creating your account…" } else { "Connecting…" });
            } else if ui.add_enabled(ready, theme::primary(if naming { "Create account" } else { "Connect" })).clicked() {
                let name = naming.then(|| play.nick.trim().to_string());
                start_setup(play, game, ctx, name);
            }
            if game.cfg.current_profile().is_some() && ui.button("Cancel").clicked() {
                play.editing = false;
                play.needs_name = None;
            }
        });
        setup_progress(play, ui);
    });
}

/// The directory's servers: nearest and busiest first is suggested, any can
/// be picked.
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

fn directory_list(play: &mut Play, ui: &mut egui::Ui) {
    if let Some(e) = &play.directory_error {
        ui.label(theme::muted(e.as_str()).small());
    }
    let Some(servers) = play.directory.as_ref() else {
        return;
    };
    ui.add_space(8.0);
    if servers.is_empty() {
        ui.label(theme::muted("The directory lists no servers right now."));
        return;
    }
    let best = setup::directory::best(servers);
    let mut pick = None;
    // The address under each name, so the list fits the window's default width.
    egui::Grid::new("directory").num_columns(5).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
        for (i, (s, ping)) in servers.iter().enumerate() {
            let chosen = play.server.trim() == s.host;
            let name = if s.region.is_empty() { s.name.clone() } else { format!("{} ({})", s.name, s.region) };
            ui.vertical(|ui| {
                ui.add(egui::Label::new(if chosen { RichText::new(name).color(theme::ACCENT) } else { RichText::new(name) }).extend());
                ui.add(egui::Label::new(theme::muted(s.host.as_str()).small()).extend());
            });
            ui.label(format!("{} online", s.players_online));
            ui.label(ping.map_or_else(|| String::from("no answer"), |ms| format!("{ms} ms")));
            ui.label(theme::muted(match (best == Some(i), s.friends_mode.as_str()) {
                (true, _) => "best for you",
                (_, "mutual") => "friends only",
                _ => "",
            }));
            if !chosen && ui.button("Choose").clicked() {
                pick = Some(s.host.clone());
            }
            ui.end_row();
        }
    });
    if let Some(host) = pick {
        play.server = host;
        play.server_picked = false;
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

/// The servers in this network, on the server card: your ping to each, and
/// a one-click switch to a better one (the identity signs in there, with the
/// same name).
fn network_list(play: &mut Play, game: &Game, current: &str, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some(servers) = play.directory.as_ref() else {
        if let Some(e) = &play.directory_error {
            ui.label(theme::muted(e.as_str()).small());
        } else if play.browsing.running() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted("Pinging the servers in this network…"));
            });
        }
        return;
    };
    if servers.len() < 2 && servers.iter().all(|(s, _)| s.host == current) {
        return;
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Servers in this network").strong());
    let best = setup::directory::best(servers);
    let mut switch_to = None;
    egui::Grid::new("network").num_columns(5).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
        for (i, (s, ping)) in servers.iter().enumerate() {
            let here = s.host == current;
            let name = if s.region.is_empty() { s.name.clone() } else { format!("{} ({})", s.name, s.region) };
            ui.label(if here { RichText::new(name).color(theme::ACCENT) } else { RichText::new(name) });
            ui.label(format!("{} online", s.players_online));
            ui.label(ping.map_or_else(|| String::from("no answer"), |ms| format!("{ms} ms")));
            ui.label(theme::muted(match (here, best == Some(i)) {
                (true, true) => "you're here; best for you",
                (true, false) => "you're here",
                (false, true) => "best for you",
                _ => "",
            }));
            if !here && !play.setup.running() && game.managed.is_none() && ui.button("Switch").on_hover_text(s.host.as_str()).clicked() {
                switch_to = Some(s.host.clone());
            }
            ui.end_row();
        }
    });
    if let Some(host) = switch_to {
        // The same name on the network's other server: made there at once if need be.
        let name = game.cfg.current_profile().map(|p| p.user.username.clone()).filter(|n| !n.is_empty());
        play.server = host;
        play.server_picked = false;
        start_setup(play, game, ctx, name);
    }
}

/// Runs the setup; `new_name` names the account if the player has none on
/// the server (None: the setup asks).
fn start_setup(play: &mut Play, game: &Game, ctx: &egui::Context, new_name: Option<String>) {
    let mut plan = flow::Plan {
        game_dir: game.dir.clone(),
        server: play.server.trim().to_string(),
        new_name,
    };
    play.setup_error = None;
    play.log = Arc::default();
    let log = Arc::clone(&play.log);
    play.setup.start(ctx, move || {
        // A network's address: set up on its best server.
        if let Some(host) = flow::pick_from_network(&plan.server, &log)? {
            plan.server = host;
        }
        flow::run_setup(&plan, crate::dll_utils::bundled(), &log)
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

fn server_card(play: &mut Play, game: &mut Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some(profile) = game.cfg.current_profile().cloned() else { return };
    poll_directory(play);
    if game.managed.is_none() {
        auto_browse(play, ctx);
    }
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(theme::heading(&format!("Server {}", profile.server)));
                let who = if profile.user.username.is_empty() {
                    "no account yet".to_string()
                } else {
                    format!("as {}", profile.user.username)
                };
                let over = profile.adapter.as_deref().map(|a| format!(" · over \"{a}\"")).unwrap_or_default();
                ui.label(theme::muted(format!("{who}{over}")));
            });
            if game.managed.is_none() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Change server").clicked() {
                        play.editing = true;
                        play.server_picked = false;
                        play.server = profile.server.clone();
                        play.needs_name = None;
                    }
                });
            }
        });
        if play.setup.running() || play.setup_error.is_some() {
            setup_progress(play, ui);
        }
        if game.managed.is_none() {
            network_list(play, game, &profile.server, ctx, ui);
        }
    });
}

fn status_dot(ui: &mut egui::Ui, status: Status) {
    theme::status_marker(ui, status);
}

fn checklist_card(play: &mut Play, game: &mut Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(theme::heading("Checklist"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if play.facts.running() || play.busy() {
                    ui.spinner();
                } else if ui.button("Check again").clicked() {
                    play.refresh(ctx, game);
                }
                let fixable = play.checks.iter().any(|c| c.status == Status::Fail && c.fix.is_some());
                let has_server = game.cfg.current_profile().is_some_and(|p| !p.server.is_empty());
                if fixable && has_server && game.managed.is_none() && !play.busy() && ui.add(theme::primary("Fix everything")).clicked() {
                    let profile = game.cfg.current_profile().cloned().unwrap_or_default();
                    play.server = profile.server;
                    let name = Some(profile.user.username).filter(|n| !n.is_empty());
                    start_setup(play, game, ctx, name);
                }
            });
        });
        ui.add_space(4.0);
        if play.checks.is_empty() {
            ui.label(theme::muted("Checking…"));
        }
        let checks = play.checks.clone();
        for check in &checks {
            ui.horizontal(|ui| {
                status_dot(ui, check.status);
                ui.vertical(|ui| {
                    ui.label(&check.title);
                    if !check.detail.is_empty() {
                        ui.label(theme::muted(check.detail.as_str()).small());
                    }
                });
                if let Some(fix) = check.fix.filter(|_| check.status != Status::Ok && game.managed.is_none()) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(!play.busy(), egui::Button::new(fix_label(fix))).clicked() {
                            run_fix(play, game, fix, ctx);
                        }
                    });
                }
            });
        }
        support_row(play, game, ctx, ui);
    });
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
    ui.horizontal(|ui| {
        status_dot(ui, status);
        ui.vertical(|ui| {
            ui.label(title);
            ui.label(theme::muted(detail.lines().next().unwrap_or_default()).small());
        });
        if status == Status::Fail {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if play.identifying.running() {
                    ui.spinner();
                } else if ui.add_enabled(!play.busy(), egui::Button::new("Try to identify")).clicked() {
                    let dir = game.dir.clone();
                    let version = setup::game::pick_version(&dir, game.cfg.default_game).unwrap_or(GameVersion::SplinterCellBlacklistDx11);
                    play.identifying
                        .start(ctx, move || flow::identify(&dir, version).map_err(|e| format!("Couldn't identify it: {e}")));
                }
            });
        }
    });
}

fn play_button(play: &mut Play, game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) {
    let ready = !play.checks.is_empty() && setup::diagnose::ready(&play.checks) && !matches!(play.support, Some(Support::Unsupported(_)));
    ui.horizontal(|ui| {
        if play.running.is_some() || play.game_seen {
            ui.add_enabled(false, theme::primary("Playing").min_size(egui::vec2(180.0, 46.0)));
            ui.label(theme::muted("Splinter Cell: Blacklist is running."));
            return;
        }
        let button = ui.add_enabled(!play.busy(), theme::primary("Play").min_size(egui::vec2(180.0, 46.0)));
        if !ready && !play.checks.is_empty() {
            ui.label(theme::muted("Some checks failed; the game may not connect."));
        }
        if button.clicked() {
            launch(play, game, notices);
        }
    });
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
