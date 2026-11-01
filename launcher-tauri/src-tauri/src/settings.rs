//! Settings (the egui launcher's settings.rs, through the shared library): what each section
//! shows, one command for each kind of change, and the work some of them start (shown in the
//! activity bar).

use hooks_config::ChatToasts;
use hooks_config::LogLevel;
use hooks_config::NatMode;
use launcher::activity::Action;
use launcher::prefs::Prefs;
use serde::Serialize;
use serde_json::Value;
use setup::diagnose::Status;
use tauri::Manager;

use crate::commands::changed;
use crate::commands::notifier;
use crate::commands::Launcher;

fn nat_id(m: NatMode) -> &'static str {
    match m {
        NatMode::Auto => "auto",
        NatMode::Relay => "relay",
        NatMode::Off => "off",
    }
}

fn toast_id(t: ChatToasts) -> &'static str {
    match t {
        ChatToasts::Never => "never",
        ChatToasts::OnlyMentions => "only-mentions",
        ChatToasts::OutsideMatches => "outside-matches",
        ChatToasts::Always => "always",
    }
}

/// The log level's name, as the egui launcher shows it.
fn level_name(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Trace => "Trace",
        LogLevel::Debug => "Debug",
        LogLevel::Info => "Info",
        LogLevel::Warning => "Warning",
        LogLevel::Error => "Error",
    }
}

const LOG_LEVELS: [LogLevel; 5] = [LogLevel::Error, LogLevel::Warning, LogLevel::Info, LogLevel::Debug, LogLevel::Trace];

#[derive(Serialize)]
pub struct GameView {
    dir: Option<String>,
    /// Both renderers installed: the choice ("dx11", "dx9").
    renderer: Option<&'static str>,
    /// What the game last ran with, and whether that isn't what was chosen (with why).
    renderer_note: Option<String>,
    renderer_warn: bool,
    /// A Steam copy on Windows: whether Play asks Steam to start it.
    through_steam: Option<bool>,
    /// Under Proton through Steam: the launch options this CPU needs, if any.
    proton: Option<Option<String>>,
    overlay: bool,
    overlay_key: String,
    overlay_keys: Vec<&'static str>,
    auto_join_invite: bool,
    /// Start without Ubisoft Connect: on or off where offered; None where it isn't (yet).
    without_ubisoft_connect: Option<bool>,
    /// Not offered yet: a start through Ubisoft Connect must work first.
    ubisoft_connect_later: bool,
    /// Fusion Fix removes the mouse acceleration itself: why this one is off.
    mouse_accel_blocked: Option<&'static str>,
    disable_mouse_accel: bool,
    brighten: bool,
    /// What the switch does here (Blacklist.ini on Windows, vkBasalt on Linux).
    brighten_hover: &'static str,
    /// Linux: brightened with vkBasalt (the slider shows its strength, Home toggles it).
    brighten_vkbasalt: bool,
    /// Linux without vkBasalt's 32-bit layer: why it can't be turned on.
    brighten_missing: Option<&'static str>,
    brightness: u8,
    brightness_range: (u8, u8),
    disable_autoclimb: bool,
    log_level: &'static str,
    log_levels: Vec<&'static str>,
    command_line: String,
    running: bool,
    mouse_turn_per_second: bool,
    beta_features: Vec<(&'static str, &'static str)>,
}

#[derive(Serialize)]
pub struct NetworkView {
    nat: &'static str,
    port_mapping: bool,
    adapters: Vec<(String, String)>,
    adapter: Option<String>,
    require_adapter: bool,
    automatic: &'static str,
}

#[derive(Serialize)]
pub struct ProfileView {
    name: String,
    server: String,
    username: String,
    current: bool,
}

#[derive(Serialize)]
pub struct SaveView {
    path: Option<String>,
    no_folder: Option<String>,
    state: Option<String>,
    missing: bool,
    ubisoft: Option<String>,
}

#[derive(Serialize)]
pub struct ClientView {
    state: &'static str,
    version: Option<&'static str>,
    can_install: bool,
    can_uninstall: bool,
}

#[derive(Serialize)]
pub struct View {
    sections: Vec<(&'static str, &'static str)>,
    /// Another tool manages this install: who.
    managed: Option<String>,
    game: Option<GameView>,
    network: Option<NetworkView>,
    notifications: Option<[&'static str; 3]>,
    toasts: Vec<(&'static str, &'static str)>,
    profiles: Vec<ProfileView>,
    save: Option<SaveView>,
    client: Option<ClientView>,
    hooks_all: bool,
    hooks: Vec<(String, &'static str, bool)>,
    log_data_version: bool,
    send_diagnostics: bool,
    ask_feedback: bool,
    directory: String,
    size: f32,
    size_range: (f32, f32),
    /// Linux: the classic (egui) interface can be chosen instead.
    classic: Option<bool>,
    product: &'static str,
    release: &'static str,
}

fn section_id(s: launcher::settings::Section) -> &'static str {
    use launcher::settings::Section as S;
    match s {
        S::Game => "game",
        S::Network => "network",
        S::Servers => "servers",
        S::Identity => "identity",
        S::Notifications => "notifications",
        S::Save => "save",
        S::Client => "client",
        S::Advanced => "advanced",
        S::Display => "display",
        S::Feedback => "feedback",
        S::About => "about",
    }
}

/// Everything Settings shows.
#[tauri::command(async)]
pub fn settings(state: tauri::State<Launcher>) -> View {
    let mut inner = state.lock();
    if let Some(g) = inner.game.as_mut() {
        g.reload();
    }
    let game = inner.game.as_ref();
    let running = setup::game::game_running();
    let game_view = game.map(|g| {
        let h = &g.cfg.hook_config;
        let installed = setup::game::installed_versions(&g.dir);
        let offer = launcher::settings::ubisoft_connect_offer(&g.dir, h.without_ubisoft_connect);
        let renderer_note = (installed.len() > 1).then(|| launcher::settings::renderer_note(&g.dir, g.cfg.default_game)).flatten();
        GameView {
            dir: Some(g.dir.display().to_string()),
            renderer: (installed.len() > 1).then_some(match g.cfg.default_game {
                setup::game::GameVersion::SplinterCellBlacklistDx9 => "dx9",
                setup::game::GameVersion::SplinterCellBlacklistDx11 => "dx11",
            }),
            renderer_note: renderer_note.as_ref().map(|(t, _)| t.clone()),
            renderer_warn: renderer_note.as_ref().is_some_and(|(_, w)| *w),
            through_steam: (cfg!(target_os = "windows") && !hooks_config::running_under_wine() && setup::launch::steam_copy(&g.dir)).then(Prefs::start_through_steam),
            proton: launcher::flow::wine_facts(&g.dir).filter(|w| w.steam).map(|w| w.launch_options),
            overlay: h.enable_overlay,
            overlay_key: h.overlay_key.clone(),
            overlay_keys: hooks_config::OVERLAY_KEYS.iter().map(|(n, _)| *n).collect(),
            auto_join_invite: h.auto_join_invite,
            without_ubisoft_connect: (offer == Some(true)).then_some(h.without_ubisoft_connect),
            ubisoft_connect_later: offer == Some(false),
            mouse_accel_blocked: hooks_config::fusion_fix::removes_mouse_accel(&g.dir).then_some(launcher::new_options::FUSION_FIX_MOUSE),
            disable_mouse_accel: h.disable_mouse_accel,
            brighten: g.cfg.brighten,
            brighten_hover: launcher::new_options::BRIGHTEN_HOVER,
            brighten_vkbasalt: launcher::new_options::BRIGHTEN_WITH_VKBASALT,
            brighten_missing: (launcher::new_options::BRIGHTEN_WITH_VKBASALT && launcher::new_options::vkbasalt_missing())
                .then_some(launcher::new_options::VKBASALT_MISSING),
            brightness: g.cfg.brightness,
            brightness_range: (setup::gamma::MIN_BRIGHTNESS, setup::gamma::MAX_BRIGHTNESS),
            disable_autoclimb: h.disable_autoclimb,
            log_level: level_name(h.logging.level),
            log_levels: LOG_LEVELS.into_iter().map(level_name).collect(),
            command_line: h.internal_command_line.clone(),
            running,
            mouse_turn_per_second: h.beta.mouse_turn_per_second,
            beta_features: launcher::settings::BETA_FEATURES.to_vec(),
        }
    });
    let network = game.map(|g| {
        let n = &g.cfg.hook_config.networking;
        NetworkView {
            nat: nat_id(n.nat),
            port_mapping: n.port_mapping,
            adapters: setup::net::adapters().into_iter().map(|(name, ip)| (name, ip.to_string())).collect(),
            adapter: n.adapter.clone(),
            require_adapter: n.require_adapter,
            automatic: launcher::settings::AUTOMATIC,
        }
    });
    let notifications = game.map(|g| {
        let n = g.cfg.hook_config.notifications;
        [toast_id(n.global), toast_id(n.lobby), toast_id(n.direct)]
    });
    let current = game.and_then(|g| g.cfg.current_profile().map(|p| p.name.clone()));
    let profiles = game
        .map(|g| {
            g.cfg
                .profiles
                .iter()
                .map(|p| ProfileView {
                    name: p.name.clone(),
                    server: p.server.clone(),
                    username: p.user.username.clone(),
                    current: current.as_deref() == Some(p.name.as_str()),
                })
                .collect()
        })
        .unwrap_or_default();
    let save = game.map(|g| match setup::save::save_path(&g.cfg.hook_config.save, &g.dir) {
        None => SaveView {
            path: None,
            no_folder: Some(setup::wine::no_save_folder(&g.dir)),
            state: None,
            missing: true,
            ubisoft: None,
        },
        Some(path) => {
            use setup::save::SaveState;
            let state = setup::save::check(&path);
            SaveView {
                path: Some(path.display().to_string()),
                no_folder: None,
                missing: state == SaveState::Missing,
                state: Some(match state {
                    SaveState::Missing => "No save yet.".to_string(),
                    SaveState::Unreadable => "Found.".to_string(),
                    SaveState::Ok { xp } => format!("{xp} XP{}", if xp < setup::save::RANK5_XP { " (below rank 5)" } else { "" }),
                }),
                ubisoft: setup::save::find_ubisoft_save(Some(&g.dir)).map(|p| p.display().to_string()),
            }
        }
    });
    let client = game.map(|g| {
        use setup::install::ClientState as C;
        let state = launcher::dll_utils::bundled().map(|dll| setup::install::client_state(&g.dir, dll));
        ClientView {
            state: match state {
                Some(C::Installed) => "Installed.",
                Some(C::Different) => "A different version is installed.",
                Some(C::NotInstalled) => "Not installed.",
                Some(C::NoGameDll) => "The game's uplay_r1_loader.dll is missing.",
                Some(C::Removed) => "Removed from the game folder (often by antivirus software): Install puts it back.",
                None => "This build doesn't carry the client.",
            },
            version: launcher::dll_utils::BUNDLED_VERSION,
            can_install: state.is_some() && state != Some(C::Installed) && g.managed.is_none(),
            can_uninstall: g.dir.join(setup::install::ORIG_DLL_NAME).exists() && g.managed.is_none(),
        }
    });
    let hooks = game
        .map(|g| {
            hooks_config::Hook::VARIANTS
                .iter()
                .zip(hooks_config::Hook::LABELS)
                .map(|(h, label)| (format!("{h:?}"), label, g.cfg.hook_config.enable_hooks.contains(h)))
                .collect()
        })
        .unwrap_or_default();
    View {
        sections: launcher::settings::Section::ALL.iter().map(|s| (section_id(*s), s.label())).collect(),
        managed: game.and_then(|g| g.managed.as_ref().map(|m| m.by.clone())),
        game: game_view,
        network,
        notifications,
        toasts: ChatToasts::ALL.iter().map(|t| (toast_id(*t), t.label())).collect(),
        profiles,
        save,
        client,
        hooks_all: game.is_some_and(|g| g.cfg.hook_config.enable_all_hooks),
        hooks,
        log_data_version: game.is_some_and(|g| g.cfg.hook_config.log_data_version),
        send_diagnostics: game.is_some_and(|g| g.cfg.hook_config.send_diagnostics),
        ask_feedback: !Prefs::feedback().1,
        directory: Prefs::directory().unwrap_or_default(),
        size: Prefs::ui_scale(),
        classic: cfg!(target_os = "linux").then(Prefs::classic_interface),
        size_range: (launcher::prefs::MIN_SIZE, launcher::prefs::MAX_SIZE),
        product: "5th Echelon Enhanced",
        release: env!("FE_RELEASE"),
    }
}

/// Changes one setting (`key`, its new `value`), as the egui launcher's controls do.
#[tauri::command(async)]
pub fn set(app: tauri::AppHandle, state: tauri::State<Launcher>, key: String, value: Value) -> Result<(), String> {
    let mut inner = state.lock();
    let crate::commands::Inner { game, notices, .. } = &mut *inner;
    // The launcher's own, without a game.
    match key.as_str() {
        "ask_feedback" => {
            Prefs::set_feedback_off(!value.as_bool().unwrap_or(true));
            return Ok(());
        }
        "size" => {
            Prefs::set_ui_scale(value.as_f64().unwrap_or(1.0) as f32);
            return Ok(());
        }
        "through_steam" => {
            Prefs::set_start_through_steam(value.as_bool().unwrap_or(false));
            return Ok(());
        }
        _ => {}
    }
    let game = game.as_mut().ok_or("Find the game first (Settings › Game).")?;
    let b = || value.as_bool().ok_or_else(|| format!("{key} wants on or off"));
    let s = || value.as_str().map(str::to_string).ok_or_else(|| format!("{key} wants a word"));
    match key.as_str() {
        "renderer" => {
            let version = if s()? == "dx9" {
                setup::game::GameVersion::SplinterCellBlacklistDx9
            } else {
                setup::game::GameVersion::SplinterCellBlacklistDx11
            };
            game.update(notices, |c| c.default_game = version);
        }
        "overlay" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.enable_overlay = on);
        }
        "overlay_key" => {
            let key = s()?;
            if !hooks_config::OVERLAY_KEYS.iter().any(|(n, _)| *n == key) {
                return Err("Not a key the overlay opens with.".into());
            }
            game.update(notices, |c| c.hook_config.overlay_key = key);
        }
        "without_ubisoft_connect" => {
            let on = b()?;
            if on && launcher::settings::ubisoft_connect_offer(&game.dir, false) != Some(true) {
                return Err(launcher::settings::WITHOUT_UBISOFT_CONNECT_LATER.into());
            }
            game.update(notices, |c| c.hook_config.without_ubisoft_connect = on);
        }
        "auto_join_invite" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.auto_join_invite = on);
        }
        "disable_mouse_accel" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.disable_mouse_accel = on);
        }
        "disable_autoclimb" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.disable_autoclimb = on);
        }
        "brighten" => launcher::new_options::set_brighten(game, notices, b()?),
        "brightness" => {
            let level = value
                .as_u64()
                .ok_or("brightness wants a number")?
                .clamp(u64::from(setup::gamma::MIN_BRIGHTNESS), u64::from(setup::gamma::MAX_BRIGHTNESS)) as u8;
            if launcher::new_options::BRIGHTEN_WITH_VKBASALT {
                // Written for vkBasalt when the game next starts.
                game.update(notices, |c| c.brightness = level);
            } else {
                let mut backup = game.cfg.gamma_backup.clone();
                match setup::gamma::enable(&game.dir, level, &mut backup) {
                    Ok(()) => game.update(notices, |c| {
                        c.brightness = level;
                        c.gamma_backup = backup;
                    }),
                    Err(e) => notices.error(format!("Couldn't change the brightness: {e}")),
                }
            }
        }
        "log_level" => {
            let word = s()?;
            let level = LOG_LEVELS.into_iter().find(|l| level_name(*l) == word).ok_or("Not a log level.")?;
            game.update(notices, |c| c.hook_config.logging.level = level);
        }
        "command_line" => {
            let line = s()?;
            game.update(notices, |c| c.hook_config.internal_command_line = line);
        }
        "mouse_turn_per_second" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.beta.mouse_turn_per_second = on);
        }
        "nat" => {
            let mode = match s()?.as_str() {
                "relay" => NatMode::Relay,
                "off" => NatMode::Off,
                _ => NatMode::Auto,
            };
            game.update(notices, |c| c.hook_config.networking.nat = mode);
        }
        "port_mapping" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.networking.port_mapping = on);
        }
        "adapter" => launcher::settings::set_adapter(game, notices, value.as_str().map(str::to_string)),
        "require_adapter" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.networking.require_adapter = on);
        }
        "notify_global" | "notify_lobby" | "notify_direct" => {
            let word = s()?;
            let choice = ChatToasts::ALL.into_iter().find(|t| toast_id(*t) == word).ok_or("Not a choice.")?;
            game.update(notices, |c| match key.as_str() {
                "notify_global" => c.hook_config.notifications.global = choice,
                "notify_lobby" => c.hook_config.notifications.lobby = choice,
                _ => c.hook_config.notifications.direct = choice,
            });
        }
        "hooks_all" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.enable_all_hooks = on);
        }
        "hook" => {
            let (name, on) = (
                value.get(0).and_then(Value::as_str).unwrap_or_default(),
                value.get(1).and_then(Value::as_bool).unwrap_or(false),
            );
            let hook = *hooks_config::Hook::VARIANTS.iter().find(|h| format!("{h:?}") == name).ok_or("Not a hook.")?;
            game.update(notices, |c| {
                if on {
                    c.hook_config.enable_hooks.insert(hook);
                } else {
                    c.hook_config.enable_hooks.remove(&hook);
                }
            });
        }
        "log_data_version" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.log_data_version = on);
        }
        "send_diagnostics" => {
            let on = b()?;
            game.update(notices, |c| c.hook_config.send_diagnostics = on);
            Prefs::set_diagnostics_asked();
        }
        "use_profile" | "remove_profile" => {
            let name = s()?;
            let profile = game.cfg.profiles.iter().find(|p| p.name == name).cloned().ok_or("That server isn't set up any more.")?;
            if key == "use_profile" {
                game.update(notices, |c| {
                    c.upsert_profile(profile.clone());
                    c.apply_profile(&profile);
                });
            } else {
                game.update(notices, |c| {
                    c.profiles.retain(|q| q.name != profile.name);
                    if c.default_profile == profile.name {
                        c.default_profile = c.profiles.first().map(|q| q.name.clone()).unwrap_or_default();
                    }
                });
            }
        }
        _ => return Err(format!("No setting {key}.")),
    }
    drop(inner);
    changed(&app);
    let _ = tauri::Emitter::emit(&app, "checks", ());
    Ok(())
}

/// A saved password, shown when the player asks (Show passwords).
#[tauri::command(async)]
pub fn profile_password(state: tauri::State<Launcher>, name: String) -> String {
    let inner = state.lock();
    inner
        .game
        .as_ref()
        .and_then(|g| g.cfg.profiles.iter().find(|p| p.name == name))
        .and_then(|p| p.user.secret())
        .unwrap_or_else(|| "(not readable here)".into())
}

/// Runs work in the background, in the activity bar as `title`.
fn work(app: &tauri::AppHandle, title: &'static str, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
    let activity = app.state::<Launcher>().lock().notices.start_with(notifier(app), title);
    changed(app);
    let app = app.clone();
    std::thread::spawn(move || {
        match launcher::task::guarded(f) {
            Ok(done) => activity.done(done),
            Err(e) => activity.fail(e, &[Action::CopyDetails]),
        }
        // The work may have changed uplay.toml (a rename) or the save.
        if let Some(g) = app.state::<Launcher>().lock().game.as_mut() {
            g.reload();
        }
        let _ = tauri::Emitter::emit(&app, "settings", ());
        changed(&app);
    });
}

fn game_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.state::<Launcher>()
        .lock()
        .game
        .as_ref()
        .map(|g| g.dir.clone())
        .ok_or_else(|| "Find the game first (Settings › Game).".into())
}

fn save_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let launcher = app.state::<Launcher>();
    let inner = launcher.lock();
    let g = inner.game.as_ref().ok_or("Find the game first (Settings › Game).")?;
    setup::save::save_path(&g.cfg.hook_config.save, &g.dir).ok_or_else(|| setup::wine::no_save_folder(&g.dir))
}

/// Settings' actions: "backup-save", "import-ubisoft", "import-save" (a file picked), "new-save",
/// "open-save-folder", "open-game-folder", "install", "uninstall", "rename" (`arg`: [profile,
/// new name]).
#[tauri::command(async)]
pub fn act(app: tauri::AppHandle, action: String, arg: Option<Value>) -> Result<(), String> {
    match action.as_str() {
        "backup-save" => {
            let path = save_path(&app)?;
            work(&app, "Backing up your save", move || {
                setup::save::backup(&path)
                    .map(|b| format!("Backed up to {}", b.display()))
                    .map_err(|e| format!("Couldn't back up: {e}"))
            });
        }
        "import-ubisoft" => {
            let (to, dir) = (save_path(&app)?, game_dir(&app)?);
            let from = setup::save::find_ubisoft_save(Some(&dir)).ok_or("No Ubisoft Connect save found.")?;
            work(&app, "Importing your Ubisoft Connect save", move || {
                setup::save::import_ubisoft(&from, &to)
                    .map(|_| "Imported your Ubisoft Connect save.".to_string())
                    .map_err(|e| e.to_string())
            });
        }
        "import-save" => {
            use tauri_plugin_dialog::DialogExt;
            let to = save_path(&app)?;
            let mut dialog = app.dialog().file().set_title("Choose a Blacklist save");
            if let Some(dir) = to.parent().filter(|d| d.is_dir()) {
                dialog = dialog.set_directory(dir);
            }
            let Some(from) = dialog.blocking_pick_file().and_then(|f| f.into_path().ok()) else {
                return Ok(());
            };
            work(&app, "Importing the save", move || match setup::save::import_file(&from, &to) {
                Ok(Some(backup)) => Ok(format!("Imported the save; the one it replaced is in {}.", backup.display())),
                Ok(None) => Ok("That's already your save.".to_string()),
                Err(e) => Err(format!("Couldn't import it: {e}")),
            });
        }
        "new-save" => {
            let to = save_path(&app)?;
            work(&app, "Making a new rank 5 save", move || {
                setup::save::create_rank5(&to).map(|_| "Created a new rank 5 save.".to_string()).map_err(|e| e.to_string())
            });
        }
        "open-save-folder" => {
            if let Some(dir) = save_path(&app)?.parent() {
                let _ = std::fs::create_dir_all(dir);
                launcher::flow::open_folder(dir);
            }
        }
        "open-game-folder" => launcher::flow::open_folder(&game_dir(&app)?),
        "install" => {
            let dir = game_dir(&app)?;
            work(&app, "Installing the 5th Echelon client", move || {
                launcher::flow::install_client(&dir, launcher::dll_utils::bundled())
            });
        }
        "uninstall" => {
            let dir = game_dir(&app)?;
            work(&app, "Putting the game's own DLL back", move || {
                setup::install::uninstall(&dir)
                    .map(|()| "The game's own DLL is back. Your 5th Echelon settings and saves are still here.".to_string())
                    .map_err(|e| e.to_string())
            });
        }
        "rename" => {
            let arg = arg.unwrap_or_default();
            let (profile, name) = (
                arg.get(0).and_then(Value::as_str).unwrap_or_default().to_string(),
                arg.get(1).and_then(Value::as_str).unwrap_or_default().to_string(),
            );
            let dir = game_dir(&app)?;
            work(&app, "Renaming your account", move || launcher::flow::rename(&dir, &profile, &name));
        }
        _ => return Err(format!("No action {action}.")),
    }
    Ok(())
}

/// The relays the player's server offers (a network's matchmaking server), each pinged now, the
/// one chosen (None: the nearest) and the one the game would use. Empty elsewhere.
#[tauri::command(async)]
pub fn relays(app: tauri::AppHandle) -> serde_json::Value {
    let server = {
        let launcher = app.state::<Launcher>();
        let inner = launcher.lock();
        inner.game.as_ref().and_then(|g| g.cfg.current_profile().map(|p| p.server.clone())).unwrap_or_default()
    };
    if server.is_empty() {
        return serde_json::json!({ "list": [] });
    }
    let (list, in_use) = launcher::task::guarded(|| Ok::<_, String>(launcher::relays::offered(&server))).unwrap_or_default();
    serde_json::json!({ "list": list, "in_use": in_use, "chosen": launcher::prefs::Prefs::relay() })
}

/// Chooses the relay the game uses (None: the nearest), from the next start.
#[tauri::command(async)]
pub fn choose_relay(relay: Option<String>) {
    launcher::relays::choose(relay);
}

/// The connection test's results: each part's name, how it went ("ok", "warn", "fail") and
/// what to say about it. Runs now, in the activity bar too.
#[tauri::command(async)]
pub fn connection_test(app: tauri::AppHandle) -> Result<Vec<(&'static str, &'static str, Option<String>)>, String> {
    let (profile, nat_port, nat_mode) = {
        let launcher = app.state::<Launcher>();
        let inner = launcher.lock();
        let g = inner.game.as_ref().ok_or("Find the game first (Settings › Game).")?;
        let profile = g.cfg.current_profile().cloned().ok_or("Choose a server first.")?;
        let nat_port = profile.nat_port.or(g.cfg.hook_config.networking.nat_port);
        (profile, nat_port, g.cfg.hook_config.networking.nat)
    };
    let activity = app.state::<Launcher>().lock().notices.start_with(notifier(&app), "Testing the connection");
    activity.step("the server's config, its API, signing in, how players reach you, the internet play helper");
    changed(&app);
    let results = launcher::task::guarded(|| Ok::<_, String>(launcher::settings::run_tests(&profile, nat_port, nat_mode)))?;
    let failed = results.iter().filter(|(_, s, _)| *s == Status::Fail).count();
    activity.done(match failed {
        0 => "Connection test: everything answered".to_string(),
        1 => "Connection test: 1 part didn't work (Settings › Network)".to_string(),
        n => format!("Connection test: {n} parts didn't work (Settings › Network)"),
    });
    changed(&app);
    Ok(results
        .into_iter()
        .map(|(name, status, note)| {
            (
                name,
                match status {
                    Status::Ok => "ok",
                    Status::Warn => "warn",
                    Status::Fail => "fail",
                },
                note,
            )
        })
        .collect())
}

/// The identity, for Identity and friends: its short form, or why it can't be read.
#[derive(Serialize)]
pub struct IdentityView {
    short: Option<String>,
    error: Option<String>,
    /// How many of this install's servers have an account saved (they answer to it).
    accounts: usize,
}

#[tauri::command(async)]
pub fn identity(state: tauri::State<Launcher>) -> IdentityView {
    let accounts = state.lock().game.as_ref().map_or(0, |g| g.cfg.profiles.iter().filter(|p| p.has_account()).count());
    match setup::player_identity::load() {
        Ok(id) => IdentityView {
            short: id.map(|id| identity::short(&id.global_id())),
            error: None,
            accounts,
        },
        Err(e) => IdentityView {
            short: None,
            error: Some(e.to_string()),
            accounts,
        },
    }
}

/// The identity as text, to move it to another PC.
#[tauri::command(async)]
pub fn export_identity(app: tauri::AppHandle) -> Option<String> {
    let id = setup::player_identity::load().ok().flatten()?;
    app.state::<Launcher>().lock().notices.info("Copied. Anyone with it can sign in as you: keep it private.");
    changed(&app);
    Some(setup::player_identity::export(&id))
}

/// Uses an identity pasted from another PC. Replacing a different one asks first (`confirm`):
/// "confirm" when it would, "done" when it's in use.
#[tauri::command(async)]
pub fn import_identity(app: tauri::AppHandle, text: String, confirm: bool) -> Result<&'static str, String> {
    let identity = setup::player_identity::import(&text).map_err(|e| e.to_string())?;
    let current = setup::player_identity::load().ok().flatten().map(|id| identity::short(&id.global_id()));
    if !confirm && current.is_some_and(|short| short != identity::short(&identity.global_id())) {
        return Ok("confirm");
    }
    let launcher = app.state::<Launcher>();
    let ok = launcher::settings::replace_identity(&identity, &mut launcher.lock().notices);
    changed(&app);
    if ok {
        Ok("done")
    } else {
        Err("Couldn't use it.".into())
    }
}

/// Sets the server directory (empty: none). Only `https://` ones.
#[tauri::command(async)]
pub fn set_directory(app: tauri::AppHandle, state: tauri::State<Launcher>, url: String) -> Result<(), String> {
    if !(url.trim().is_empty() || setup::directory::valid_coordinator(&url)) {
        return Err("The server directory must be an https:// address.".into());
    }
    Prefs::set_directory(Some(url));
    let mut inner = state.lock();
    inner.directory = None;
    inner.notices.info("Saved. Browse servers on the Play screen.");
    drop(inner);
    changed(&app);
    Ok(())
}

/// The player's reports and the admins' replies (Settings › Feedback).
#[derive(Serialize)]
pub struct ReportView {
    line: String,
    said: String,
    reply: String,
}

#[tauri::command(async)]
pub fn my_reports() -> Result<Option<Vec<ReportView>>, String> {
    if !launcher::roadmap::has_identity() {
        return Ok(None);
    }
    let clock = setup::clock::Clock::local();
    let mine = launcher::task::guarded(launcher::feedback::fetch_mine)?;
    Ok(Some(
        mine.into_iter()
            .map(|r| ReportView {
                line: format!(
                    "{} · {} · {}",
                    clock.date(r.created_at),
                    if r.server.is_empty() { "a server" } else { &r.server },
                    if r.status == "resolved" { "resolved" } else { "open" }
                ),
                said: if r.comment.is_empty() {
                    "(no comment)".to_string()
                } else {
                    format!("\u{201c}{}\u{201d}", r.comment)
                },
                reply: r.reply,
            })
            .collect(),
    ))
}

/// Linux: the classic (egui) interface from now on. The egui launcher that opened this one
/// shows its window as this one closes.
#[tauri::command(async)]
pub fn use_classic(app: tauri::AppHandle) {
    Prefs::set_classic_interface(true);
    app.state::<crate::host::Host>().stop();
    app.exit(launcher::front_door::SWITCH_TO_CLASSIC);
}
