//! The Settings screen: everything the automatic setup decides, for players
//! who want to decide it themselves, plus saves, connection tests and the
//! hook's switches.

use std::time::Duration;

use eframe::egui;
use eframe::egui::RichText;
use hooks_config::Hook;
use hooks_config::LogLevel;
use setup::save::SaveState;

use crate::app::App;
use crate::app::Game;
use crate::app::Notices;
use crate::flow;
use crate::task::Slot;
use crate::theme;

type TestResults = Vec<(&'static str, Result<(), String>)>;

#[derive(Default)]
pub struct Settings {
    tests: Slot<TestResults>,
    test_results: TestResults,
    working: Slot<Result<String, String>>,
    confirm_new_save: bool,
    confirm_uninstall: bool,
    show_passwords: bool,
    /// What's typed in Identity › Import.
    identity_import: String,
    /// The server directory field, while it's being edited.
    directory: Option<String>,
    /// The identity as shown: its short id and export text, read once (it's
    /// decrypted from disk) and again after an import.
    identity: Option<Result<Option<(String, String)>, String>>,
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    game_folder(app, ui);
    let (settings, game, notices) = app.settings_mut();
    let Some(game) = game.as_mut() else { return };
    let ctx = ui.ctx().clone();
    if let Some(result) = settings.working.poll() {
        match result {
            Ok(msg) => notices.info(msg),
            Err(e) => notices.error(e),
        }
    }
    if let Some(results) = settings.tests.poll() {
        settings.test_results = results;
    }
    let locked = game.managed.is_some();
    if let Some(m) = &game.managed {
        ui.label(theme::muted(format!("{} manages this install: server, account and network settings are read-only here.", m.by)));
        ui.add_space(6.0);
    }

    section(ui, "Network", |ui| network(game, notices, locked, &ctx, settings, ui));
    section(ui, "Game", |ui| game_options(game, notices, ui));
    section(ui, "Save game", |ui| save_game(settings, game, &ctx, ui));
    section(ui, "Servers and accounts", |ui| servers(settings, game, notices, locked, ui));
    section(ui, "Identity and friends", |ui| identity_section(settings, notices, ui));
    section(ui, "Connection test", |ui| connection_test(settings, game, &ctx, ui));
    section(ui, "5th Echelon client", |ui| client(settings, game, &ctx, ui));
    egui::CollapsingHeader::new(theme::heading("Hooks (advanced)")).default_open(false).show(ui, |ui| hooks(game, notices, ui));
    ui.add_space(10.0);
    about(app, ui);
}

fn about(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    section(ui, "About", |ui| {
        ui.label(format!("{} {}", env!("FE_PRODUCT"), env!("FE_RELEASE")));
        ui.label(theme::muted("A fork of 5th Echelon by unixoide and contributors.").small());
        ui.horizontal(|ui| {
            if app.checking.running() {
                ui.spinner();
            } else if ui.button("Check for updates").clicked() {
                app.check_for_update(&ctx);
            }
            match &app.latest {
                Some(l) if l.newer() => ui.label(RichText::new(format!("Version {} is available (see the banner).", l.version)).color(theme::ACCENT)),
                Some(l) => ui.label(theme::muted(format!("Up to date (latest release {}).", l.version))),
                None => ui.label(""),
            };
            ui.hyperlink_to("Releases", crate::updater::RELEASES_PAGE);
        });
    });
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(theme::heading(title));
        ui.add_space(4.0);
        body(ui);
    });
    ui.add_space(10.0);
}

fn game_folder(app: &mut App, ui: &mut egui::Ui) {
    let current = app.game.as_ref().map(|g| g.dir.clone());
    let found = app.found.clone();
    let mut chosen = None;
    let mut pick = false;
    section(ui, "Game folder", |ui| {
        match &current {
            Some(dir) => ui.label(dir.display().to_string()),
            None => ui.label(theme::muted("Not found yet.")),
        };
        ui.horizontal(|ui| {
            if found.len() > 1 {
                egui::ComboBox::from_id_salt("found").selected_text("Other installs found").show_ui(ui, |ui| {
                    for dir in &found {
                        if ui.selectable_label(Some(dir) == current.as_ref(), dir.display().to_string()).clicked() {
                            chosen = Some(dir.clone());
                        }
                    }
                });
            }
            pick = ui.button("Choose folder…").clicked();
            if let Some(dir) = &current {
                if ui.button("Open folder").clicked() {
                    flow::open_folder(dir);
                }
            }
        });
    });
    if let Some(dir) = chosen {
        app.choose_game(dir);
    }
    if pick {
        app.pick_game_folder();
    }
}

fn network(game: &mut Game, notices: &mut Notices, locked: bool, ctx: &egui::Context, settings: &mut Settings, ui: &mut egui::Ui) {
    let adapters = setup::net::adapters();
    let pinned = game.cfg.hook_config.networking.adapter.clone();
    ui.label(theme::muted(
        "The game uses one network adapter for matches. The setup pins the one your server is reached through.",
    ));
    ui.add_enabled_ui(!locked, |ui| {
        ui.horizontal(|ui| {
            ui.label("Adapter");
            let label = pinned.clone().unwrap_or_else(|| "Any (not pinned)".into());
            let mut choice: Option<Option<String>> = None;
            egui::ComboBox::from_id_salt("adapter").selected_text(label).width(260.0).show_ui(ui, |ui| {
                if ui.selectable_label(pinned.is_none(), "Any (not pinned)").clicked() {
                    choice = Some(None);
                }
                for (name, ip) in &adapters {
                    let selected = pinned.as_deref().is_some_and(|p| hooks_config::adapter_name_matches(name, p));
                    if ui.selectable_label(selected, format!("{name}  ·  {ip}")).clicked() {
                        choice = Some(Some(name.clone()));
                    }
                }
            });
            if let Some(adapter) = choice {
                game.update(notices, |c| {
                    if let Some(mut p) = c.current_profile().cloned() {
                        p.adapter = adapter.clone();
                        c.upsert_profile(p.clone());
                        c.apply_profile(&p);
                    } else {
                        c.hook_config.networking.adapter = adapter.clone();
                    }
                });
            }
            if ui.add_enabled(!settings.working.running(), egui::Button::new("Pick automatically")).clicked() {
                let dir = game.dir.clone();
                settings.working.start(ctx, move || flow::pin_adapter(&dir));
            }
        });
        let mut require = game.cfg.hook_config.networking.require_adapter;
        if ui
            .checkbox(&mut require, "Don't start the game without this adapter")
            .on_hover_text("Stops the game from quietly playing over the wrong network when your VPN is off.")
            .changed()
        {
            game.update(notices, |c| c.hook_config.networking.require_adapter = require);
        }
        ui.add_space(8.0);
        internet_play(game, notices, ui);
    });
}

/// How other players reach this PC over the internet (NAT traversal).
fn internet_play(game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) {
    use hooks_config::NatMode;
    let networking = &game.cfg.hook_config.networking;
    let (mut mode, mut port_mapping) = (networking.nat, networking.port_mapping);
    let label = |m: NatMode| match m {
        NatMode::Auto => "Automatic (recommended)",
        NatMode::Relay => "Always through the server",
        NatMode::Off => "LAN or VPN only",
    };
    ui.horizontal(|ui| {
        ui.label("Internet play");
        egui::ComboBox::from_id_salt("nat").selected_text(label(mode)).width(260.0).show_ui(ui, |ui| {
            for m in [NatMode::Auto, NatMode::Relay, NatMode::Off] {
                ui.selectable_value(&mut mode, m, label(m));
            }
        });
    });
    ui.label(theme::muted(match mode {
        NatMode::Auto => "The server tells the game this PC's public address, so players connect directly; when your router can't be reached, play goes through the server.",
        NatMode::Relay => "All match traffic goes through the server. Use it when direct connections fail; it adds a little delay.",
        NatMode::Off => "The game advertises this PC's local address, as it always did: other players must be on your network or VPN.",
    }));
    ui.add_enabled_ui(mode != NatMode::Off, |ui| {
        ui.checkbox(&mut port_mapping, "Open the game's port on the router (UPnP / NAT-PMP)")
            .on_hover_text("Asks the router to forward UDP 13000 to this PC while the game runs, so other players can always reach you directly.");
    });
    if (mode, port_mapping) != (networking.nat, networking.port_mapping) {
        game.update(notices, |c| {
            c.hook_config.networking.nat = mode;
            c.hook_config.networking.port_mapping = port_mapping;
        });
    }
}

fn game_options(game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) {
    if let Some(wine) = flow::wine_facts(&game.dir).filter(|w| w.steam) {
        ui.label(theme::muted("Runs through Steam and Proton; Play asks Steam to start it."));
        if let Some(options) = wine.launch_options {
            ui.label(format!("This CPU needs these Steam launch options (Properties › Launch options) or the game can freeze at start:"));
            ui.horizontal(|ui| {
                ui.monospace(&options);
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(options.clone());
                }
            });
        }
        ui.add_space(4.0);
    }
    let mut hook = game.cfg.hook_config.clone();
    ui.checkbox(&mut hook.enable_overlay, "In-game overlay (F5)");
    ui.checkbox(&mut hook.auto_join_invite, "Join invites automatically");
    ui.horizontal(|ui| {
        ui.label("Client log detail");
        egui::ComboBox::from_id_salt("loglevel").selected_text(hook.logging.level.as_ref().to_string()).show_ui(ui, |ui| {
            for level in [LogLevel::Error, LogLevel::Warning, LogLevel::Info, LogLevel::Debug, LogLevel::Trace] {
                let label = level.as_ref().to_string();
                ui.selectable_value(&mut hook.logging.level, level, label);
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label("Extra command line");
        ui.add(egui::TextEdit::singleline(&mut hook.internal_command_line).hint_text("Unreal Engine options").desired_width(300.0));
    });
    if hook != game.cfg.hook_config {
        game.update(notices, |c| c.hook_config = hook);
    }
}

fn save_game(settings: &mut Settings, game: &mut Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some(path) = setup::save::save_path(&game.cfg.hook_config.save, &game.dir) else {
        ui.label(theme::muted("The save folder can't be found."));
        return;
    };
    let state = setup::save::check(&path);
    ui.label(path.display().to_string());
    ui.label(theme::muted(match &state {
        SaveState::Missing => "No save yet.".to_string(),
        SaveState::Unreadable => "Not a save this launcher can read; it's left as it is.".to_string(),
        SaveState::Ok { xp } => format!("{xp} XP{}", if *xp < setup::save::RANK5_XP { " (below rank 5)" } else { "" }),
    }));
    ui.add_enabled_ui(!settings.working.running(), |ui| {
        ui.horizontal(|ui| {
            if state != SaveState::Missing && ui.button("Back up now").clicked() {
                match setup::save::backup(&path) {
                    Ok(b) => settings.working.start(ctx, move || Ok(format!("Backed up to {}", b.display()))),
                    Err(e) => settings.working.start(ctx, move || Err(format!("Couldn't back up: {e}"))),
                }
            }
            if let Some(ubisoft) = setup::save::find_ubisoft_save() {
                if ui.button("Import from Ubisoft Connect").on_hover_text(ubisoft.display().to_string()).clicked() {
                    let to = path.clone();
                    settings.working.start(ctx, move || {
                        setup::save::import_ubisoft(&ubisoft, &to).map(|_| "Imported your Ubisoft Connect save.".to_string()).map_err(|e| e.to_string())
                    });
                }
            }
            if ui.button("New rank 5 save…").clicked() {
                settings.confirm_new_save = true;
            }
            if let Some(dir) = path.parent() {
                if ui.button("Open folder").clicked() {
                    let _ = std::fs::create_dir_all(dir);
                    flow::open_folder(dir);
                }
            }
        });
        if settings.confirm_new_save {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Replace your save with a new rank 5 one? The current save is backed up first.").color(theme::WARN));
                if ui.button("Replace").clicked() {
                    settings.confirm_new_save = false;
                    let to = path.clone();
                    settings.working.start(ctx, move || {
                        setup::save::create_rank5(&to).map(|_| "Created a new rank 5 save.".to_string()).map_err(|e| e.to_string())
                    });
                }
                if ui.button("Cancel").clicked() {
                    settings.confirm_new_save = false;
                }
            });
        }
    });
}

fn servers(settings: &mut Settings, game: &mut Game, notices: &mut Notices, locked: bool, ui: &mut egui::Ui) {
    let profiles = game.cfg.profiles.clone();
    if profiles.is_empty() {
        ui.label(theme::muted("None yet. Join one on the Play screen."));
        return;
    }
    let current = game.cfg.current_profile().map(|p| p.name.clone());
    ui.checkbox(&mut settings.show_passwords, "Show passwords");
    ui.add_enabled_ui(!locked, |ui| {
        egui::Grid::new("profiles").num_columns(4).striped(true).spacing([16.0, 8.0]).show(ui, |ui| {
            for p in &profiles {
                let is_current = current.as_deref() == Some(p.name.as_str());
                ui.label(if is_current { RichText::new(&p.server).color(theme::ACCENT) } else { RichText::new(&p.server) });
                ui.label(&p.user.username);
                ui.label(if settings.show_passwords { p.user.secret().unwrap_or_else(|| String::from("(not readable here)")) } else { "••••••••".into() });
                ui.horizontal(|ui| {
                    if !is_current && ui.button("Use").clicked() {
                        game.update(notices, |c| {
                            c.upsert_profile(p.clone());
                            c.apply_profile(p);
                        });
                    }
                    if ui.button("Remove").clicked() {
                        game.update(notices, |c| {
                            c.profiles.retain(|q| q.name != p.name);
                            if c.default_profile == p.name {
                                c.default_profile = c.profiles.first().map(|q| q.name.clone()).unwrap_or_default();
                            }
                        });
                    }
                });
                ui.end_row();
            }
        });
    });
}

/// The player's identity across servers (see `setup::player_identity`), and
/// the server directory.
fn identity_section(settings: &mut Settings, notices: &mut Notices, ui: &mut egui::Ui) {
    ui.label(theme::muted(
        "Your identity follows you between servers that share friends: friends you make on one show up on the others. \
         It also signs you in to your accounts on a new PC.",
    ));
    ui.add_space(4.0);
    let shown = settings.identity.get_or_insert_with(|| {
        setup::player_identity::load()
            .map(|id| id.map(|id| (identity::short(&id.global_id()), setup::player_identity::export(&id))))
            .map_err(|e| e.to_string())
    });
    match shown {
        Ok(Some((short, export))) => {
            ui.horizontal(|ui| {
                ui.label("Your identity");
                ui.label(RichText::new(short.as_str()).strong());
                if ui.button("Copy to move it to another PC").clicked() {
                    ui.ctx().copy_text(export.clone());
                    notices.info("Copied. Anyone with it can sign in as you: keep it private.");
                }
            });
        }
        Ok(None) => {
            ui.label(theme::muted("None yet: it's made when you join a server that supports it."));
        }
        Err(e) => {
            ui.label(RichText::new(format!("Your identity can't be read: {e}")).color(theme::BAD));
        }
    }
    ui.horizontal(|ui| {
        ui.label("Import");
        ui.add(egui::TextEdit::singleline(&mut settings.identity_import).hint_text("5th-echelon-identity:v1:…").password(true).desired_width(260.0));
        if ui.add_enabled(!settings.identity_import.trim().is_empty(), egui::Button::new("Use this identity")).clicked() {
            match setup::player_identity::import(&settings.identity_import).and_then(|identity| setup::player_identity::save(&identity).map(|()| identity)) {
                Ok(identity) => {
                    notices.info(format!("This PC now uses the identity {}. Set up each server again to sign in with it.", identity::short(&identity.global_id())));
                    settings.identity_import.clear();
                    settings.identity = None;
                }
                Err(e) => notices.error(e.to_string()),
            }
        }
    });
    ui.add_space(8.0);
    let saved = crate::app::Prefs::directory().unwrap_or_default();
    let editing = settings.directory.get_or_insert(saved.clone());
    ui.horizontal(|ui| {
        ui.label("Server directory");
        ui.add(egui::TextEdit::singleline(editing).hint_text("https://coordinator.example.com").desired_width(260.0));
        if *editing != saved && ui.button("Save").clicked() {
            crate::app::Prefs::set_directory(Some(editing.clone()));
            notices.info("Saved. Browse servers on the Play screen.");
        }
    });
    ui.label(theme::muted("Filled in by the first server you join that shares friends; lists servers to choose from."));
}

fn connection_test(settings: &mut Settings, game: &Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some(profile) = game.cfg.current_profile().cloned() else {
        ui.label(theme::muted("Choose a server first."));
        return;
    };
    ui.label(theme::muted(
        "Checks each part of the connection in turn: the server's config, its API, signing in to the game service, whether the server can reach this PC directly, and the server's helper for playing over the internet.",
    ));
    ui.horizontal(|ui| {
        if settings.tests.running() {
            ui.spinner();
            ui.label("Testing…");
        } else if ui.button("Run the test").clicked() {
            let nat_port = profile.nat_port.or(game.cfg.hook_config.networking.nat_port);
            settings.tests.start(ctx, move || run_tests(&profile, nat_port));
        }
    });
    for (name, result) in &settings.test_results {
        ui.horizontal(|ui| {
            theme::status_marker(ui, if result.is_ok() { setup::diagnose::Status::Ok } else { setup::diagnose::Status::Fail });
            ui.label(*name);
            if let Err(e) = result {
                ui.label(theme::muted(e.as_str()).small());
            }
        });
    }
    if settings.test_results.iter().any(|(_, r)| r.is_err()) && setup::game::game_running() {
        ui.label(theme::muted("The game is running and holds the ports some tests need; close it and test again."));
    }
}

fn run_tests(profile: &setup::config::Profile, game_nat_port: Option<u16>) -> TestResults {
    use crate::network;
    let rt = crate::services::rt();
    let api = profile.api_server_url().to_string();
    let secret = profile.user.secret().unwrap_or_default();
    let (user, pass) = (profile.user.username.as_str(), secret.as_str());
    let t = Duration::from_secs(8);
    let run = |f: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), network::Error>> + Send + '_>>| -> Result<(), String> {
        // The timer must be made inside the runtime.
        match rt.block_on(async { tokio::time::timeout(t, f).await }) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("no answer in time".into()),
        }
    };
    let mut results: TestResults = vec![("Config server (port 80)", run(Box::pin(network::test_cfg_server(&profile.server))))];
    results.push(("API and account", run(Box::pin(network::test_login(api.clone(), user, pass)))));
    results.push(("Game service sign-in", run(Box::pin(network::test_quazal_login(&profile.server, profile.login_port(), user, pass)))));
    results.push(("Direct connection to this PC", run(Box::pin(network::test_p2p(api, user, pass)))));
    let nat_port = game_nat_port.unwrap_or(nat_proto::DEFAULT_PORT);
    let nat = match rt.block_on(async { tokio::time::timeout(t, network::test_nat_helper(&profile.server, nat_port)).await }) {
        Ok(Ok(check)) => match check.symmetric {
            Some(true) => Err(format!(
                "reachable; this PC is {} to the server, and the router changes ports per destination, so matches go through the server's relay",
                check.observed
            )),
            _ => Ok(()),
        },
        Ok(Err(e)) => Err(format!("{e} (UDP {nat_port}-{}): only LAN or VPN play works", nat_port + 1)),
        Err(_) => Err(format!("no answer (UDP {nat_port}-{}): only LAN or VPN play works", nat_port + 1)),
    };
    results.push(("Internet play helper", nat));
    results
}

fn client(settings: &mut Settings, game: &Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    let state = crate::dll_utils::bundled().map(|dll| setup::install::client_state(&game.dir, dll));
    ui.label(theme::muted(match state {
        Some(setup::install::ClientState::Installed) => "Installed.",
        Some(setup::install::ClientState::Different) => "A different version is installed.",
        Some(setup::install::ClientState::NotInstalled) => "Not installed.",
        Some(setup::install::ClientState::NoGameDll) => "The game's uplay_r1_loader.dll is missing.",
        None => "This build doesn't carry the client.",
    }));
    if let Some(version) = crate::dll_utils::BUNDLED_VERSION {
        ui.label(theme::muted(format!("Carried by this launcher: {version}")).small());
    }
    ui.add_enabled_ui(!settings.working.running() && game.managed.is_none(), |ui| {
        ui.horizontal(|ui| {
            if state != Some(setup::install::ClientState::Installed) && ui.button("Install").clicked() {
                let dir = game.dir.clone();
                settings.working.start(ctx, move || flow::install_client(&dir, crate::dll_utils::bundled()));
            }
            if game.dir.join(setup::install::ORIG_DLL_NAME).exists() && ui.button("Uninstall…").clicked() {
                settings.confirm_uninstall = true;
            }
        });
        if settings.confirm_uninstall {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Put the game's own online DLL back? The game then won't connect to 5th Echelon servers.").color(theme::WARN));
                if ui.button("Uninstall").clicked() {
                    settings.confirm_uninstall = false;
                    let dir = game.dir.clone();
                    settings.working.start(ctx, move || {
                        setup::install::uninstall(&dir).map(|()| "The game's own DLL is back.".to_string()).map_err(|e| e.to_string())
                    });
                }
                if ui.button("Cancel").clicked() {
                    settings.confirm_uninstall = false;
                }
            });
        }
    });
}

fn hooks(game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) {
    ui.label(theme::muted("Which parts of the game the client patches. Leave these alone unless you're debugging."));
    let mut hook = game.cfg.hook_config.clone();
    ui.checkbox(&mut hook.enable_all_hooks, "All hooks");
    ui.add_enabled_ui(!hook.enable_all_hooks, |ui| {
        egui::Grid::new("hooks").num_columns(3).show(ui, |ui| {
            for (i, (h, label)) in Hook::VARIANTS.iter().zip(Hook::LABELS).enumerate() {
                let mut on = hook.enable_hooks.contains(h);
                if ui.checkbox(&mut on, label).changed() {
                    if on {
                        hook.enable_hooks.insert(*h);
                    } else {
                        hook.enable_hooks.remove(h);
                    }
                }
                if i % 3 == 2 {
                    ui.end_row();
                }
            }
        });
    });
    if hook != game.cfg.hook_config {
        game.update(notices, |c| c.hook_config = hook);
    }
}
