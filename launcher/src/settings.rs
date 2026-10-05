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
use crate::app::Prefs;
use crate::flow;
use crate::task::Slot;
use crate::theme;

/// Each check's name, how it went, and what to say about it.
type TestResults = Vec<(&'static str, setup::diagnose::Status, Option<String>)>;

/// The parts of Settings, one shown at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Game,
    Network,
    Servers,
    Identity,
    Save,
    Client,
    Advanced,
    Display,
    Feedback,
    About,
}

impl Section {
    const ALL: [Self; 10] = [
        Self::Game,
        Self::Network,
        Self::Servers,
        Self::Identity,
        Self::Save,
        Self::Client,
        Self::Advanced,
        Self::Display,
        Self::Feedback,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Game => "Game",
            Self::Network => "Network",
            Self::Servers => "Servers and accounts",
            Self::Identity => "Identity and friends",
            Self::Save => "Save game",
            Self::Client => "5th Echelon client",
            Self::Advanced => "Advanced",
            Self::Display => "Display",
            Self::Feedback => "Feedback",
            Self::About => "About",
        }
    }
}

#[derive(Default)]
pub struct Settings {
    /// The section shown.
    pub section: Section,
    tests: Slot<TestResults>,
    test_results: TestResults,
    working: Slot<Result<String, String>>,
    confirm_new_save: bool,
    confirm_uninstall: bool,
    show_passwords: bool,
    /// What's typed in Identity › Import.
    identity_import: String,
    /// The import waits for a yes: it would replace this PC's own identity.
    confirm_identity_import: bool,
    /// The server directory field, while it's being edited.
    directory: Option<String>,
    /// The identity as shown: its short id and export text, read once (it's
    /// decrypted from disk) and again after an import.
    identity: Option<Result<Option<(String, String)>, String>>,
    /// The profile being renamed, and the new name typed so far.
    renaming: Option<(String, String)>,
    /// Ubisoft Connect's save, as last looked for (it reads every save it finds), and when.
    ubisoft_save: Option<(std::time::Instant, Option<std::path::PathBuf>)>,
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    egui::SidePanel::left("settings-nav")
        .resizable(false)
        .exact_width(230.0)
        .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin {
            left: 16,
            right: 16,
            top: 26,
            bottom: 16,
        }))
        .show_inside(ui, |ui| {
            ui.label(theme::display("Settings", 26.0));
            ui.add_space(12.0);
            let current = app.settings_mut().0.section;
            for section in Section::ALL {
                let chosen = section == current;
                let text = RichText::new(section.label()).color(if chosen { theme::FG } else { theme::SOFT });
                let text = if chosen { text.family(theme::strong()) } else { text };
                let button = egui::Button::new(text)
                    .fill(if chosen { theme::SURFACE } else { egui::Color32::TRANSPARENT })
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(8)
                    .min_size(egui::vec2(ui.available_width(), 40.0));
                if ui.add(button).clicked() {
                    app.settings_mut().0.section = section;
                }
            }
        });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG)).show_inside(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            theme::page().show(ui, |ui| {
                ui.set_width(ui.available_width());
                body(app, ui);
            });
        });
    });
}

fn body(app: &mut App, ui: &mut egui::Ui) {
    let current = app.settings_mut().0.section;
    ui.label(RichText::new(current.label()).family(theme::strong()).size(22.0));
    ui.add_space(10.0);
    match current {
        Section::Game => game_folder(app, ui),
        Section::About => {
            about(app, ui);
            return;
        }
        Section::Display => {
            display(app, ui);
            return;
        }
        Section::Feedback => {
            feedback(app, ui);
            return;
        }
        _ => {}
    }
    let (settings, game, notices) = app.settings_mut();
    let Some(game) = game.as_mut() else {
        if current != Section::Game {
            ui.label(theme::muted("Find the game first (Settings › Game)."));
        }
        return;
    };
    let ctx = ui.ctx().clone();
    if let Some(result) = settings.working.poll() {
        match result {
            Ok(msg) => notices.info(msg),
            Err(e) => notices.error(e),
        }
        // The work may have changed uplay.toml (a rename, a pinned adapter).
        game.reload();
    }
    if let Some(results) = settings.tests.poll() {
        settings.test_results = results;
    }
    let locked = game.managed.is_some();
    if let Some(m) = &game.managed {
        ui.label(theme::muted(format!(
            "{} manages this install: server, account and network settings are read-only here.",
            m.by
        )));
        ui.add_space(6.0);
    }

    match current {
        Section::Game => section(ui, "Options", |ui| game_options(game, notices, ui)),
        Section::Network => {
            section(ui, "Internet play", |ui| ui.add_enabled_ui(!locked, |ui| internet_play(game, notices, ui)).inner);
            section(ui, "Connection test", |ui| connection_test(settings, game, &ctx, ui));
            section(ui, "Network adapter", |ui| network(game, notices, locked, ui));
        }
        Section::Servers => section(ui, "Servers and accounts", |ui| servers(settings, game, notices, locked, &ctx, ui)),
        Section::Identity => {
            let accounts = game.cfg.profiles.iter().filter(|p| p.has_account()).count();
            section(ui, "Identity and friends", |ui| identity_section(settings, accounts, notices, ui));
        }
        Section::Save => section(ui, "Save game", |ui| save_game(settings, game, &ctx, ui)),
        Section::Client => section(ui, "5th Echelon client", |ui| client(settings, game, &ctx, ui)),
        Section::Advanced => section(ui, "Hooks", |ui| hooks(game, notices, ui)),
        Section::About | Section::Display | Section::Feedback => {}
    }
}

/// Asking how games went, and sending a report now.
fn feedback(app: &mut App, ui: &mut egui::Ui) {
    let (_, game, notices) = app.settings_mut();
    if let Some(game) = game.as_mut() {
        section(ui, "While you play", |ui| {
            ui.label(theme::muted(
                "The game sends the server it's signed in to its warnings, errors and network events as they happen, so the server's admins can see why a join or a connection failed. Your PC's name, your user folder and your internet address are hidden first.",
            ));
            let mut send = game.cfg.hook_config.send_diagnostics;
            if ui.checkbox(&mut send, "Send the game's diagnostics to the server").changed() {
                game.update(notices, |c| c.hook_config.send_diagnostics = send);
                Prefs::set_diagnostics_asked();
            }
        });
    }
    section(ui, "After a game", |ui| {
        ui.label(theme::muted(
            "When something goes wrong in a game (a join that fails, a crash), and now and then otherwise, the launcher asks how it went once the game closes. Your answer goes to the server's admins, with your logs if you agree: your PC's name, your user folder and your internet address are hidden first.",
        ));
        let (_, off) = Prefs::feedback();
        let mut ask = !off;
        if ui.checkbox(&mut ask, "Ask me how my games went").changed() {
            Prefs::set_feedback_off(!ask);
        }
    });
    section(ui, "Report a problem", |ui| {
        ui.label(theme::muted("Something wrong now? Tell the admins of the server you play on, with your logs."));
        let ctx = ui.ctx().clone();
        if ui.add_enabled(!app.feedback_busy(), theme::secondary("Send feedback")).clicked() {
            app.ask_feedback(&ctx);
        }
    });
}

/// The launcher's size: it fits its window, and the player's size comes on top.
fn display(app: &mut App, ui: &mut egui::Ui) {
    section(ui, "Size", |ui| {
        ui.label(theme::muted(
            "The launcher scales with its window: resize it, maximise it, or press F11 for full screen. Make everything bigger or smaller here, or with Ctrl + and Ctrl −; Ctrl 0 puts it back.",
        ));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            // Buttons, not a slider: the page rescales as the size changes.
            let size = app.ui_scale;
            if ui
                .add_enabled(size > crate::scale::MIN_SIZE + 0.01, egui::Button::new("  −  "))
                .on_hover_text("Smaller (Ctrl −)")
                .clicked()
            {
                app.set_ui_scale(size - 0.1);
            }
            ui.label(RichText::new(format!("{:.0} %", size * 100.0)).family(theme::strong()));
            if ui
                .add_enabled(size < crate::scale::MAX_SIZE - 0.01, egui::Button::new("  +  "))
                .on_hover_text("Bigger (Ctrl +)")
                .clicked()
            {
                app.set_ui_scale(size + 0.1);
            }
            if (size - 1.0).abs() > 0.01 && ui.button("Reset").on_hover_text("Ctrl 0").clicked() {
                app.set_ui_scale(1.0);
            }
        });
        ui.add_space(6.0);
        let full = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
        if ui.button(if full { "Leave full screen (F11)" } else { "Full screen (F11)" }).clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
        }
    });
}

fn about(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    section(ui, "About", |ui| {
        ui.label(format!("{} {}", env!("FE_PRODUCT"), env!("FE_RELEASE")));
        ui.label(theme::muted("Developed by JDevWebb, building on 5th Echelon by unixoide and its contributors.").small());
        ui.label(theme::muted("Free, and provided as is, without warranty of any kind. Not affiliated with or endorsed by Ubisoft.").small());
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

/// The adapter choice that pins nothing.
const AUTOMATIC: &str = "Automatic (the one that reaches the server)";

fn network(game: &mut Game, notices: &mut Notices, locked: bool, ui: &mut egui::Ui) {
    let adapters = setup::net::adapters();
    let pinned = game.cfg.hook_config.networking.adapter.clone();
    ui.label(theme::muted(
        "The game uses one network adapter for matches: automatically, the one that reaches your server, chosen each time the game starts. Pin one only if your group plays over a VPN on purpose.",
    ));
    ui.add_enabled_ui(!locked, |ui| {
        ui.horizontal(|ui| {
            ui.label("Adapter");
            let label = pinned.clone().unwrap_or_else(|| AUTOMATIC.into());
            let mut choice: Option<Option<String>> = None;
            egui::ComboBox::from_id_salt("adapter").selected_text(label).width(260.0).show_ui(ui, |ui| {
                if ui.selectable_label(pinned.is_none(), AUTOMATIC).clicked() {
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
                    if adapter.is_none() {
                        c.hook_config.networking.require_adapter = false;
                    }
                });
            }
        });
        let mut require = game.cfg.hook_config.networking.require_adapter;
        if pinned.is_some()
            && ui
                .checkbox(&mut require, "Don't start the game without this adapter")
                .on_hover_text("Stops the game from quietly playing over the wrong network when your VPN is off.")
                .changed()
        {
            game.update(notices, |c| c.hook_config.networking.require_adapter = require);
        }
    });
}

/// How other players reach this PC over the internet (NAT traversal).
fn internet_play(game: &mut Game, notices: &mut Notices, ui: &mut egui::Ui) {
    use hooks_config::NatMode;
    let networking = &game.cfg.hook_config.networking;
    let (mut mode, mut port_mapping) = (networking.nat, networking.port_mapping);
    let label = |m: NatMode| match m {
        NatMode::Auto => "Automatic",
        NatMode::Relay => "Always through the server",
        NatMode::Off => "LAN or VPN only",
    };
    ui.label(theme::muted("How other players reach this PC."));
    theme::segmented(ui, &mut mode, &[NatMode::Auto, NatMode::Relay, NatMode::Off].map(|m| (m, label(m))));
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
    // DirectX 11 unless the player picks 9 here (only offered when both are installed).
    let installed = setup::game::installed_versions(&game.dir);
    if installed.len() > 1 {
        use setup::game::GameVersion;
        ui.label("Renderer");
        let mut version = game.cfg.default_game;
        let options = [
            (GameVersion::SplinterCellBlacklistDx11, "DirectX 11 (recommended)"),
            (GameVersion::SplinterCellBlacklistDx9, "DirectX 9"),
        ];
        if theme::segmented(ui, &mut version, &options) {
            game.update(notices, |c| c.default_game = version);
        }
        ui.add_space(6.0);
    }
    if let Some(wine) = flow::wine_facts(&game.dir).filter(|w| w.steam) {
        ui.label(theme::muted("Runs through Steam and Proton; Play asks Steam to start it."));
        if let Some(options) = wine.launch_options {
            ui.label(format!(
                "This CPU needs these Steam launch options (Properties › Launch options) or the game can freeze at start:"
            ));
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
        egui::ComboBox::from_id_salt("loglevel")
            .selected_text(hook.logging.level.as_ref().to_string())
            .show_ui(ui, |ui| {
                for level in [LogLevel::Error, LogLevel::Warning, LogLevel::Info, LogLevel::Debug, LogLevel::Trace] {
                    let label = level.as_ref().to_string();
                    ui.selectable_value(&mut hook.logging.level, level, label);
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Extra command line");
        ui.add(
            egui::TextEdit::singleline(&mut hook.internal_command_line)
                .hint_text("Unreal Engine options")
                .desired_width(300.0),
        );
    });
    if hook != game.cfg.hook_config {
        game.update(notices, |c| c.hook_config = hook);
    }
}

fn save_game(settings: &mut Settings, game: &mut Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some(path) = setup::save::save_path(&game.cfg.hook_config.save, &game.dir) else {
        ui.label(theme::muted(setup::wine::no_save_folder(&game.dir)));
        return;
    };
    let state = setup::save::check(&path);
    ui.label(path.display().to_string());
    ui.label(theme::muted(match &state {
        SaveState::Missing => "No save yet.".to_string(),
        SaveState::Unreadable => "Found.".to_string(),
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
            if settings.ubisoft_save.as_ref().is_none_or(|(at, _)| at.elapsed() >= std::time::Duration::from_secs(10)) {
                settings.ubisoft_save = Some((std::time::Instant::now(), setup::save::find_ubisoft_save(Some(&game.dir))));
            }
            if let Some(ubisoft) = settings.ubisoft_save.as_ref().and_then(|(_, p)| p.clone()) {
                if ui.button("Import from Ubisoft Connect").on_hover_text(ubisoft.display().to_string()).clicked() {
                    let to = path.clone();
                    settings.working.start(ctx, move || {
                        setup::save::import_ubisoft(&ubisoft, &to)
                            .map(|_| "Imported your Ubisoft Connect save.".to_string())
                            .map_err(|e| e.to_string())
                    });
                }
            }
            if ui
                .button("Import a save file…")
                .on_hover_text("A Blacklist save from another PC, Ubisoft Connect's 1.save, or one of the launcher's backups. Your current save is backed up first.")
                .clicked()
            {
                let start = path.parent().map(std::path::Path::to_path_buf);
                let mut dialog = rfd::FileDialog::new().set_title("Choose a Blacklist save");
                if let Some(dir) = start.filter(|d| d.is_dir()) {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(from) = dialog.pick_file() {
                    let to = path.clone();
                    settings.working.start(ctx, move || match setup::save::import_file(&from, &to) {
                        Ok(Some(backup)) => Ok(format!("Imported the save; the one it replaced is in {}.", backup.display())),
                        Ok(None) => Ok("That's already your save.".to_string()),
                        Err(e) => Err(format!("Couldn't import it: {e}")),
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

fn servers(settings: &mut Settings, game: &mut Game, notices: &mut Notices, locked: bool, ctx: &egui::Context, ui: &mut egui::Ui) {
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
                ui.label(if is_current {
                    RichText::new(&p.server).color(theme::ACCENT)
                } else {
                    RichText::new(&p.server)
                });
                let renaming_this = settings.renaming.as_ref().is_some_and(|(name, _)| *name == p.name);
                if renaming_this {
                    if let Some((_, new_name)) = settings.renaming.as_mut() {
                        ui.add(egui::TextEdit::singleline(new_name).hint_text("new name").desired_width(140.0));
                    }
                } else {
                    ui.label(&p.user.username);
                }
                ui.label(if settings.show_passwords {
                    p.user.secret().unwrap_or_else(|| String::from("(not readable here)"))
                } else {
                    "••••••••".into()
                });
                ui.horizontal(|ui| {
                    if renaming_this {
                        let busy = settings.working.running();
                        if ui.add_enabled(!busy, egui::Button::new("Save name")).clicked() {
                            if let Some((profile, new_name)) = settings.renaming.take() {
                                let dir = game.dir.clone();
                                settings.working.start(ctx, move || flow::rename(&dir, &profile, &new_name));
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            settings.renaming = None;
                        }
                        return;
                    }
                    if ui.button("Rename").on_hover_text("A new name on this server; friends and invites carry on").clicked() {
                        settings.renaming = Some((p.name.clone(), p.user.username.clone()));
                    }
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

/// Makes `identity` this PC's, keeping the one it replaces.
fn use_identity(settings: &mut Settings, identity: &setup::player_identity::Identity, notices: &mut Notices) {
    match setup::player_identity::replace(identity) {
        Ok(kept) => {
            let kept = kept.map(|p| format!(" The one it replaced is kept in {}.", p.display())).unwrap_or_default();
            notices.info(format!(
                "This PC now uses the identity {}. Press Connect on each server again to sign in with it.{kept}",
                identity::short(&identity.global_id())
            ));
            settings.identity_import.clear();
            settings.identity = None;
            settings.confirm_identity_import = false;
            // The checklist signs in again with the new identity's accounts.
            crate::flow::forget_account_check();
        }
        Err(e) => notices.error(e.to_string()),
    }
}

/// The player's identity across servers (see `setup::player_identity`), and
/// the server directory.
/// `accounts`: how many of this install's servers have an account saved.
fn identity_section(settings: &mut Settings, accounts: usize, notices: &mut Notices, ui: &mut egui::Ui) {
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
    ui.label(theme::muted(
        "On a new PC, import your identity before you connect to a server: connecting first makes a new identity, and a new account under another name.",
    ));
    ui.horizontal(|ui| {
        ui.label("Import");
        let edited = ui
            .add(
                egui::TextEdit::singleline(&mut settings.identity_import)
                    .hint_text("5th-echelon-identity:v1:…")
                    .password(true)
                    .desired_width(260.0),
            )
            .changed();
        if edited {
            settings.confirm_identity_import = false;
        }
        if ui
            .add_enabled(
                !settings.identity_import.trim().is_empty() && !settings.confirm_identity_import,
                egui::Button::new("Use this identity"),
            )
            .clicked()
        {
            match setup::player_identity::import(&settings.identity_import) {
                Ok(identity) => {
                    // Replacing a different identity asks first: its accounts answer to it alone.
                    let current = match settings.identity.as_ref() {
                        Some(Ok(Some((short, _)))) => Some(short.clone()),
                        _ => None,
                    };
                    if current.is_some_and(|short| short != identity::short(&identity.global_id())) {
                        settings.confirm_identity_import = true;
                    } else {
                        use_identity(settings, &identity, notices);
                    }
                }
                Err(e) => notices.error(e.to_string()),
            }
        }
    });
    if settings.confirm_identity_import {
        let current = match settings.identity.as_ref() {
            Some(Ok(Some((short, _)))) => short.clone(),
            _ => String::from("this PC's identity"),
        };
        theme::card().fill(theme::BAD.linear_multiply(0.10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let used = match accounts {
                0 => String::new(),
                1 => " Your saved account here is linked to it.".to_string(),
                n => format!(" Your {n} saved accounts here are linked to it."),
            };
            ui.label(format!(
                "This replaces {current}.{used} Accounts made with it can only be signed in to with it: if you'll want them, copy it first. A copy of it stays in the launcher's folder."
            ));
            ui.horizontal(|ui| {
                if ui.button("Replace it").clicked() {
                    match setup::player_identity::import(&settings.identity_import) {
                        Ok(identity) => use_identity(settings, &identity, notices),
                        Err(e) => notices.error(e.to_string()),
                    }
                }
                if ui.button("Cancel").clicked() {
                    settings.confirm_identity_import = false;
                }
            });
        });
    }
    ui.add_space(8.0);
    let saved = crate::app::Prefs::directory().unwrap_or_default();
    let editing = settings.directory.get_or_insert(saved.clone());
    ui.horizontal(|ui| {
        ui.label("Server directory");
        ui.add(egui::TextEdit::singleline(editing).hint_text("https://coordinator.example.com").desired_width(260.0));
        if *editing != saved && ui.button("Save").clicked() {
            if editing.trim().is_empty() || setup::directory::valid_coordinator(editing) {
                crate::app::Prefs::set_directory(Some(editing.clone()));
                notices.info("Saved. Browse servers on the Play screen.");
            } else {
                notices.error("The server directory must be an https:// address.");
            }
        }
    });
    ui.label(theme::muted(
        "The servers to choose from: the community network unless you join another network. Empty: none.",
    ));
}

fn connection_test(settings: &mut Settings, game: &Game, ctx: &egui::Context, ui: &mut egui::Ui) {
    let Some(profile) = game.cfg.current_profile().cloned() else {
        ui.label(theme::muted("Choose a server first."));
        return;
    };
    ui.label(theme::muted(
        "Checks each part of the connection in turn: the server's config, its API, signing in to the game service, how other players reach this PC (as the overlay shows it in game), and the server's helper for playing over the internet.",
    ));
    ui.horizontal(|ui| {
        if settings.tests.running() {
            ui.spinner();
            ui.label("Testing…");
        } else if ui.button("Run the test").clicked() {
            let nat_port = profile.nat_port.or(game.cfg.hook_config.networking.nat_port);
            let nat_mode = game.cfg.hook_config.networking.nat;
            settings.tests.start(ctx, move || run_tests(&profile, nat_port, nat_mode));
        }
    });
    for (name, status, note) in &settings.test_results {
        ui.horizontal_wrapped(|ui| {
            theme::status_marker(ui, *status);
            ui.label(*name);
            if let Some(note) = note {
                ui.label(theme::muted(note.as_str()).small());
            }
        });
    }
    if settings.test_results.iter().any(|(_, s, _)| *s != setup::diagnose::Status::Ok) && setup::game::game_running() {
        ui.label(theme::muted("The game is running and holds the ports some tests need; close it and test again."));
    }
}

fn run_tests(profile: &setup::config::Profile, game_nat_port: Option<u16>, nat_mode: hooks_config::NatMode) -> TestResults {
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
    use setup::diagnose::Status;
    let pass_fail = |r: Result<(), String>| match r {
        Ok(()) => (Status::Ok, None),
        Err(e) => (Status::Fail, Some(e)),
    };
    let mut results: TestResults = Vec::new();
    let mut add = |name, (status, note): (Status, Option<String>)| results.push((name, status, note));
    add("Config server (port 80)", pass_fail(run(Box::pin(network::test_cfg_server(&profile.server)))));
    add("API and account", pass_fail(run(Box::pin(network::test_login(api.clone(), user, pass)))));
    add(
        "Game service sign-in",
        pass_fail(run(Box::pin(network::test_quazal_login(&profile.server, profile.login_port(), user, pass)))),
    );
    // Only a router that forwards UDP 13000 (or no router) lets the server's packet in
    // unasked. If it doesn't, ask the router for the mapping the game asks for while it
    // runs (UPnP, then NAT-PMP), try again, and take the mapping away. The outcome is
    // worded like the overlay's line in game: without a port open, the server relays
    // this PC's matches (Automatic), or other players may not get through (LAN or VPN only).
    let timed_out = |e: &str| e.contains("in time") || e.contains("Deadline");
    let no_port = |why: String| match nat_mode {
        hooks_config::NatMode::Off => (
            Status::Warn,
            Some(format!(
                "Direct, no router port open ({why}): others may not be able to join you. Internet play is set to LAN or VPN only"
            )),
        ),
        _ => (
            Status::Ok,
            Some(format!(
                "Through the server's relay: no router port open ({why}), so matches go through the server, which adds a little delay"
            )),
        ),
    };
    let direct = match run(Box::pin(network::test_p2p(api.clone(), user, pass))) {
        Ok(()) => (Status::Ok, Some("Direct: reachable without any router set-up".into())),
        Err(e) if timed_out(&e) => {
            let pinned = profile
                .adapter
                .as_deref()
                .and_then(|name| setup::net::adapter_ip(name, &setup::net::adapters()))
                .and_then(|ip| match ip {
                    std::net::IpAddr::V4(v4) => Some(v4),
                    std::net::IpAddr::V6(_) => None,
                });
            match portmap::map(nat_proto::STORM_PORT, pinned, 120, "5th Echelon connection test") {
                Err(why) => no_port(format!("the router didn't forward one: {why}")),
                Ok(mapping) => {
                    let (public, how) = (mapping.public, mapping.how);
                    let outcome = if public.port() != nat_proto::STORM_PORT {
                        no_port(format!("the router offered {public} ({how}), not port {}; another PC may have it", nat_proto::STORM_PORT))
                    } else {
                        match run(Box::pin(network::test_p2p(api, user, pass))) {
                            Ok(()) => (Status::Ok, Some(format!("Direct, router port opened ({public}, {how}); the game opens it while it runs"))),
                            // The game reports this mapping, so the server treats the PC as
                            // reachable and doesn't relay it.
                            Err(e) if timed_out(&e) => (
                                Status::Warn,
                                Some(format!(
                                    "Direct, router port opened ({public}, {how}), but the server's packet still didn't arrive: a firewall on this PC, or another router in front. Others may not be able to join you; set Internet play to Always through the server"
                                )),
                            ),
                            Err(e) => (Status::Fail, Some(e)),
                        }
                    };
                    mapping.remove();
                    outcome
                }
            }
        }
        Err(e) => (Status::Fail, Some(e)),
    };
    let direct = match (nat_mode, direct) {
        (hooks_config::NatMode::Relay, (Status::Ok | Status::Warn, _)) => {
            (Status::Ok, Some("Through the server's relay: Internet play is set to Always through the server".into()))
        }
        (_, outcome) => outcome,
    };
    add("How players reach this PC", direct);
    let nat_port = game_nat_port.unwrap_or(nat_proto::DEFAULT_PORT);
    let nat = match rt.block_on(async { tokio::time::timeout(t, network::test_nat_helper(&profile.server, nat_port)).await }) {
        Ok(Ok(check)) => match check.symmetric {
            Some(true) => (
                Status::Warn,
                Some(format!(
                    "reachable; this PC is {} to the server, and the router changes ports per destination, so matches go through the server's relay",
                    check.observed
                )),
            ),
            _ => (Status::Ok, None),
        },
        Ok(Err(e)) => (Status::Fail, Some(format!("{e} (UDP {nat_port}-{}): only LAN or VPN play works", nat_port + 1))),
        Err(_) => (Status::Fail, Some(format!("no answer (UDP {nat_port}-{}): only LAN or VPN play works", nat_port + 1))),
    };
    add("Internet play helper", nat);
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
                        setup::install::uninstall(&dir)
                            .map(|()| "The game's own DLL is back.".to_string())
                            .map_err(|e| e.to_string())
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
    ui.add_space(10.0);
    ui.label(theme::muted(
        "For working out why two copies of the game refuse each other (\"a different version of the game\"): the game writes what its data version is made of to bl-dataversion.txt in its folder. Off, the file is deleted.",
    ));
    ui.checkbox(&mut hook.log_data_version, "Write the game's data version (bl-dataversion.txt)");
    if hook != game.cfg.hook_config {
        game.update(notices, |c| c.hook_config = hook);
    }
}
