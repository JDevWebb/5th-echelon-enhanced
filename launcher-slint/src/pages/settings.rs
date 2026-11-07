//! Settings (the Tauri launcher's Settings.vue, section by section): each control changes one
//! setting through settings.rs's `set`; the work some start (a save imported, the client
//! installed) shows in the activity bar.

use serde_json::json;
use serde_json::Value;

use super::*;
use crate::commands::Launcher;
use crate::tauri::Manager;
use crate::tauri::WindowCall;

const NAT: [(&str, &str); 3] = [("auto", "Automatic"), ("relay", "Always through the server"), ("off", "LAN or VPN only")];

fn nat_says(id: &str) -> &'static str {
    match id {
        "relay" => "All match traffic goes through the server. Use it when direct connections fail; it adds a little delay.",
        "off" => "The game advertises this PC's local address, as it always did: other players must be on your network or VPN.",
        _ => "The server tells the game this PC's public address, so players connect directly; when your router can't be reached, play goes through the server.",
    }
}

#[derive(Default)]
pub struct SettingsPage {
    pub section: String,
    /// A question open: "new-save", "uninstall", "import-identity".
    confirm: Option<&'static str>,
    renaming: Option<(String, String)>,
    show_passwords: bool,
    passwords: Vec<(String, String)>,
    tests: Option<Result<Vec<(String, String, String)>, String>>,
    testing: bool,
    command_line: Option<String>,
    import_text: String,
    import_error: Option<String>,
    directory: Option<String>,
    directory_error: Option<String>,
    reports: Option<Result<Option<Vec<Value>>, String>>,
    update: Option<Result<(String, bool), String>>,
    found: Vec<String>,
    size: f32,
}

impl SettingsPage {
    pub fn new() -> Self {
        Self {
            section: "game".into(),
            size: launcher::prefs::Prefs::ui_scale(),
            ..Default::default()
        }
    }

    fn set(&mut self, app: &AppHandle, key: &str, value: Value) {
        if let Err(e) = crate::settings::set(app.clone(), app.state::<Launcher>(), key.into(), value) {
            let launcher = app.state::<Launcher>();
            launcher.lock().notices.error(e);
            crate::commands::changed(app);
        }
    }

    fn act(&mut self, app: &AppHandle, action: &str, arg: Option<Value>) {
        if let Err(e) = crate::settings::act(app.clone(), action.into(), arg) {
            app.state::<Launcher>().lock().notices.error(e);
            crate::commands::changed(app);
        }
    }

    pub fn set_size(&mut self, app: &AppHandle, size: f32) {
        self.size = launcher::prefs::clamp((size * 10.0).round() / 10.0);
        launcher::prefs::Prefs::set_ui_scale(self.size);
        app.window(WindowCall::Size(self.size));
    }
}

impl Screen for SettingsPage {
    fn live_edits(&self) -> bool {
        true
    }

    fn section(&mut self, id: &str) {
        self.section = id.into();
        self.confirm = None;
        if id == "feedback" {
            self.reports = None;
        }
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        let v = json(&crate::settings::settings(app.state::<Launcher>()));
        let sections = pairs(arr(&v, "sections"));
        if !sections.iter().any(|(id, _)| *id == self.section) {
            self.section = "game".into();
        }
        let label = sections.iter().find(|(id, _)| *id == self.section).map(|(_, l)| l.clone()).unwrap_or_default();
        let managed = st(&v, "managed");
        let mut r = Rows::default();
        if !managed.is_empty() && matches!(self.section.as_str(), "network" | "servers") {
            r.add(note(format!("{managed} manages this install: server, account and network settings are read-only here.")));
        }
        let has_game = v.get("game").is_some_and(|g| !g.is_null());
        if !has_game && !matches!(self.section.as_str(), "game" | "identity" | "display" | "feedback" | "about") {
            r.add(note("Find the game first (Settings › Game)."));
        }
        match self.section.as_str() {
            "game" => self.game(app, &v, &mut r),
            "network" => self.network(app, &v, &mut r),
            "servers" => self.servers(app, &v, &mut r),
            "identity" => self.identity(app, &v, &mut r),
            "notifications" => notifications(&v, &mut r),
            "save" => self.save(&v, &mut r),
            "client" => self.client(&v, &mut r),
            "advanced" => advanced(&v, &mut r),
            "display" => self.display(&mut r),
            "feedback" => self.feedback(&v, &mut r),
            "about" => self.about(&v, &mut r),
            _ => {}
        }
        PageView {
            kicker: "Loadout".into(),
            title: "Settings".into(),
            summary: label,
            sections,
            section: self.section.clone(),
            rows: r.done(),
        }
    }

    fn toggled(&mut self, app: &AppHandle, id: &str, on: bool) {
        match id {
            "show_passwords" => {
                self.show_passwords = on;
                if on {
                    let names: Vec<String> = app
                        .state::<Launcher>()
                        .lock()
                        .game
                        .as_ref()
                        .map(|g| g.cfg.profiles.iter().map(|p| p.name.clone()).collect())
                        .unwrap_or_default();
                    self.passwords = names
                        .into_iter()
                        .map(|n| (n.clone(), crate::settings::profile_password(app.state::<Launcher>(), n)))
                        .collect();
                }
            }
            _ if id.starts_with("hook:") => self.set(app, "hook", json!([&id[5..], on])),
            _ => self.set(app, id, Value::Bool(on)),
        }
    }

    fn chosen(&mut self, app: &AppHandle, id: &str, index: usize) {
        let v = json(&crate::settings::settings(app.state::<Launcher>()));
        let g = v.get("game").cloned().unwrap_or(Value::Null);
        let n = v.get("network").cloned().unwrap_or(Value::Null);
        let pick = |list: Vec<String>| list.get(index).cloned();
        let value: Option<Value> = match id {
            "renderer" => pick(vec!["dx11".into(), "dx9".into()]).map(Value::String),
            "overlay_key" => pick(arr(&g, "overlay_keys").iter().filter_map(|k| k.as_str().map(String::from)).collect()).map(Value::String),
            "log_level" => pick(arr(&g, "log_levels").iter().filter_map(|k| k.as_str().map(String::from)).collect()).map(Value::String),
            "nat" => pick(NAT.iter().map(|(i, _)| i.to_string()).collect()).map(Value::String),
            "adapter" => {
                if index == 0 {
                    Some(Value::Null)
                } else {
                    arr(&n, "adapters").get(index - 1).and_then(|a| a.get(0)).cloned()
                }
            }
            "notify_global" | "notify_lobby" | "notify_direct" => pick(pairs(arr(&v, "toasts")).into_iter().map(|(i, _)| i).collect()).map(Value::String),
            "theme" => {
                let theme = [launcher::prefs::Theme::Dark, launcher::prefs::Theme::Light, launcher::prefs::Theme::System][index.min(2)];
                launcher::prefs::Prefs::set_theme(theme);
                app.window(WindowCall::Theme(theme));
                None
            }
            "other_install" => {
                if let Some(dir) = self.found.get(index).cloned() {
                    crate::core::choose_game_dir(&app.state::<Launcher>().0, &dir);
                }
                None
            }
            _ => None,
        };
        if let Some(value) = value {
            self.set(app, id, value);
        }
    }

    fn slid(&mut self, app: &AppHandle, id: &str, value: f32) {
        if id == "brightness" {
            self.set(app, "brightness", json!(value.round() as u32));
        }
    }

    fn edited(&mut self, id: &str, text: &str) {
        match id {
            "command_line" => self.command_line = Some(text.into()),
            "import" => {
                self.import_text = text.into();
                self.import_error = None;
                if self.confirm == Some("import-identity") {
                    self.confirm = None;
                }
            }
            "directory" => self.directory = Some(text.into()),
            "rename" => {
                if let Some((_, to)) = self.renaming.as_mut() {
                    *to = text.into();
                }
            }
            _ => {}
        }
    }

    fn accepted(&mut self, app: &AppHandle, id: &str, text: &str) {
        self.edited(id, text);
        match id {
            "command_line" => self.pressed(app, "save-command-line"),
            "import" => self.pressed(app, "import"),
            "directory" => self.pressed(app, "save-directory"),
            "rename" => self.pressed(app, "rename-save"),
            _ => {}
        }
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        match id {
            "pick-folder" => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Choose the folder with Blacklist_game.exe").pick_folder() {
                    crate::core::choose_game_dir(&app.state::<Launcher>().0, &dir.display().to_string());
                }
            }
            "save-command-line" => {
                if let Some(t) = self.command_line.take() {
                    self.set(app, "command_line", Value::String(t));
                }
            }
            _ if id.starts_with("copy:") => {
                app.window(WindowCall::Copy(id[5..].to_string()));
            }
            "test" => {
                self.testing = true;
                self.tests = Some(crate::settings::connection_test(app.clone()).map(|list| {
                    list.into_iter()
                        .map(|(name, status, note)| (name.to_string(), status.to_string(), note.unwrap_or_default()))
                        .collect()
                }));
                self.testing = false;
            }
            "export-identity" => {
                if let Some(text) = crate::settings::export_identity(app.clone()) {
                    app.window(WindowCall::Copy(text));
                }
            }
            "import" | "import-confirmed" => match crate::settings::import_identity(app.clone(), self.import_text.trim().into(), id == "import-confirmed") {
                Ok("confirm") => self.confirm = Some("import-identity"),
                Ok(_) => {
                    self.import_text.clear();
                    self.confirm = None;
                }
                Err(e) => self.import_error = Some(e),
            },
            "save-directory" => {
                if let Some(url) = self.directory.clone() {
                    match crate::settings::set_directory(app.clone(), app.state::<Launcher>(), url) {
                        Ok(()) => {
                            self.directory = None;
                            self.directory_error = None;
                        }
                        Err(e) => self.directory_error = Some(e),
                    }
                }
            }
            "rename-save" => {
                if let Some((name, to)) = self.renaming.take() {
                    if !to.trim().is_empty() {
                        self.act(app, "rename", Some(json!([name, to.trim()])));
                    }
                }
            }
            "rename-cancel" => self.renaming = None,
            "check-update" => {
                self.update = Some(crate::update::check_update(app.clone(), app.state::<crate::update::Update>()));
            }
            "reports" => self.reports = None,
            "send-feedback" => crate::feedback::ask_feedback(app.clone(), app.state::<crate::feedback::Feedback>()),
            "releases" => {
                let _ = crate::core::open_url("https://github.com/JDevWebb/5th-echelon-enhanced/releases");
            }
            "smaller" => self.set_size(app, self.size - 0.1),
            "bigger" => self.set_size(app, self.size + 0.1),
            "size-reset" => self.set_size(app, 1.0),
            "fullscreen" => {
                app.window(WindowCall::FullScreen);
            }
            "ask-new-save" => self.confirm = Some("new-save"),
            "ask-uninstall" => self.confirm = Some("uninstall"),
            "cancel" => self.confirm = None,
            "new-save" | "uninstall" => {
                self.confirm = None;
                self.act(app, id, None);
            }
            _ if id.starts_with("rename:") => {
                let name = id[7..].to_string();
                let user = app
                    .state::<Launcher>()
                    .lock()
                    .game
                    .as_ref()
                    .and_then(|g| g.cfg.profiles.iter().find(|p| p.name == name).map(|p| p.user.username.clone()))
                    .unwrap_or_default();
                self.renaming = Some((name, user));
            }
            _ if id.starts_with("use:") => self.set(app, "use_profile", Value::String(id[4..].into())),
            _ if id.starts_with("remove:") => self.set(app, "remove_profile", Value::String(id[7..].into())),
            _ if id.starts_with("act:") => self.act(app, &id[4..], None),
            _ => {}
        }
    }
}

impl SettingsPage {
    fn game(&mut self, app: &AppHandle, v: &Value, r: &mut Rows) {
        let g = v.get("game").filter(|g| !g.is_null());
        r.card("Game folder");
        let current = g.map(|g| st(g, "dir")).unwrap_or_default();
        let shown = ::setup::game::tidy_dir(std::path::Path::new(&current)).display().to_string();
        r.add(text(if current.is_empty() { "Not found yet.".into() } else { shown }));
        self.found = crate::core::find_games();
        // The same folder written another way ('/' for '\', another case) isn't another install.
        self.found
            .retain(|d| current.is_empty() || !::setup::game::same_dir(std::path::Path::new(d), std::path::Path::new(&current)));
        if !self.found.is_empty() {
            r.add(choice(
                "other_install",
                "Other installs found",
                &self.found.iter().map(|d| (d.clone(), d.clone())).collect::<Vec<_>>(),
                "",
            ));
        }
        let mut row = vec![super::b("pick-folder", "Choose folder…")];
        if g.is_some() {
            row.push(super::b("act:open-game-folder", "Open folder"));
        }
        r.add(buttons(row));
        let Some(g) = g else { return };
        let running = bo(g, "running");
        r.card("Options");
        if !st(g, "renderer").is_empty() {
            r.add(segments(
                "renderer",
                "Renderer",
                &[("dx11".into(), "DirectX 11 (recommended)".into()), ("dx9".into(), "DirectX 9".into())],
                &st(g, "renderer"),
            ));
            let note_text = st(g, "renderer_note");
            if !note_text.is_empty() {
                r.add(if bo(g, "renderer_warn") { warn(note_text) } else { note(note_text) });
            }
        }
        if let Some(on) = g.get("through_steam").and_then(Value::as_bool) {
            r.add(switch("through_steam", "Start the game through Steam (for Steam Input and controllers)", on));
            r.add_if(on, || note("Steam then picks DirectX 9 or 11 (its Play menu or launch options), not the Renderer above, and the launcher can't tell from its exit code whether the game crashed."));
        }
        if let Some(proton) = g.get("proton").filter(|p| !p.is_null()) {
            r.add(note("Runs through Steam and Proton; Play asks Steam to start it."));
            if let Some(options) = proton.as_str() {
                r.add(text(
                    "This CPU needs these Steam launch options (Properties › Launch options) or the game can freeze at start:",
                ));
                r.add(code(options).button(super::b(format!("copy:{options}"), "Copy")));
            }
        }
        let overlay = bo(g, "overlay");
        r.add(switch("overlay", "In-game overlay", overlay));
        let keys: Vec<(String, String)> = arr(g, "overlay_keys").iter().filter_map(|k| k.as_str()).map(|k| (k.to_string(), k.to_string())).collect();
        let key = st(g, "overlay_key");
        r.add(choice("overlay_key", "Opened with", &keys, &key).enabled(overlay));
        r.add_if(overlay && key.starts_with('F') && key[1..].chars().all(|c| c.is_ascii_digit()), || {
            note(format!(
                "On most laptops, and some keyboards, hold Fn and press {key}. If nothing happens, pick another key here."
            ))
        });
        r.add(switch("auto_join_invite", "Join invites automatically", bo(g, "auto_join_invite")));
        if let Some(on) = g.get("without_ubisoft_connect").and_then(Value::as_bool) {
            r.add(
                switch("without_ubisoft_connect", "Start without Ubisoft Connect", on)
                    .sub("On by default: the game starts without opening Ubisoft Connect. Ubisoft's own overlay isn't there; everything 5th Echelon does works the same. Turn it off to start through Ubisoft Connect."),
            );
        } else if bo(g, "ubisoft_connect_later") {
            r.add(note(
                "Starting without Ubisoft Connect is offered here once the game has started through it and signed in on this PC.",
            ));
        }
        let blocked = st(g, "mouse_accel_blocked");
        if !blocked.is_empty() {
            r.add(switch("disable_mouse_accel", "Mouse moves 1:1", false).enabled(false).sub(blocked));
        } else {
            let mut s = switch("disable_mouse_accel", "Mouse moves 1:1", bo(g, "disable_mouse_accel"))
                .sub("The game caps how far the view can turn in one frame, so fast flicks feel like the mouse slows down. This widens that cap for a linear, 1:1 feel.");
            s.text = "RECOMMENDED".into();
            r.add(s);
        }
        let brighten = bo(g, "brighten");
        let missing = st(g, "brighten_missing");
        r.add(
            switch("brighten", "Brighten the picture", brighten)
                .enabled(!running && (brighten || missing.is_empty()))
                .sub(st(g, "brighten_hover")),
        );
        if brighten {
            let range = arr(g, "brightness_range");
            let (min, max) = (
                range.first().and_then(Value::as_f64).unwrap_or(50.0) as f32,
                range.get(1).and_then(Value::as_f64).unwrap_or(200.0) as f32,
            );
            let value = num(g, "brightness") as f32;
            let shown = if bo(g, "brighten_vkbasalt") {
                format!("{:.2}", 1.0 + (value.clamp(min, max) - min) / (max - min))
            } else {
                format!("{:.2}", value / 100.0)
            };
            r.add(slider("brightness", "Brightness", value, min, max, shown).enabled(!running));
        }
        if !missing.is_empty() {
            r.add(note(missing));
        } else if bo(g, "brighten_vkbasalt") && brighten {
            r.add(note("In the game, Home turns the brightening off and on."));
        }
        r.add_if(running && brighten, || note("Close the game to change the picture brightness."));
        r.add(
            switch("disable_autoclimb", "No automatic climbing (Active Sprint)", bo(g, "disable_autoclimb"))
                .enabled(!running)
                .sub("The game climbs and vaults over edges by itself when you sprint into them. This turns that off, so climbing is only ever manual. Takes effect the next time the game starts."),
        );
        r.add_if(running, || note("Close the game to change automatic climbing."));
        let levels: Vec<(String, String)> = arr(g, "log_levels").iter().filter_map(|k| k.as_str()).map(|k| (k.to_string(), k.to_string())).collect();
        r.add(choice("log_level", "Client log detail", &levels, &st(g, "log_level")));
        let typed = self.command_line.clone().unwrap_or_else(|| st(g, "command_line"));
        r.add(
            field("command_line", "Extra command line", typed)
                .placeholder("Unreal Engine options")
                .button(super::b("save-command-line", "Save").enabled(self.command_line.is_some())),
        );
        r.card("Beta features");
        r.add(warn(
            "Experimental: these may feel wrong, stutter, or crash the game. Turn one off if it gives you trouble.",
        ));
        r.add(note("While any of them is on, the launcher sends the game's full log (names, folders and addresses on your PC hidden) to your server's admins each time the game closes, so they can see how it went. Each takes effect the next time the game starts."));
        let mut any = false;
        for f in arr(g, "beta_features") {
            let (Some(k), Some(l), Some(hover), Some(on)) = (
                f.get(0).and_then(Value::as_str),
                f.get(1).and_then(Value::as_str),
                f.get(2).and_then(Value::as_str),
                f.get(3).and_then(Value::as_bool),
            ) else {
                continue;
            };
            any |= on;
            r.add(switch(k, l, on).sub(hover));
        }
        r.add_if(any, || accent("Beta on: your game's log is sent after each game."));
        let _ = app;
    }

    fn network(&mut self, _app: &AppHandle, v: &Value, r: &mut Rows) {
        let Some(n) = v.get("network").filter(|n| !n.is_null()) else { return };
        let locked = !st(v, "managed").is_empty();
        let nat = st(n, "nat");
        r.card("Internet play");
        r.add(note("How other players reach this PC."));
        r.add(segments("nat", "", &NAT.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>(), &nat).enabled(!locked));
        r.add(note(nat_says(&nat)));
        r.add_if(nat == "off", || {
            warn("On a server with a relay, like the community network, players on the internet can't join you, and you can't join players whose games play through the relay. Use it for LAN or VPN servers.")
        });
        r.add(
            switch("port_mapping", "Open the game's port on the router (UPnP / NAT-PMP)", bo(n, "port_mapping"))
                .enabled(nat != "off" && !locked)
                .sub("Asks the router to forward UDP 13000 to this PC while the game runs, so other players can always reach you directly."),
        );
        r.card("Connection test");
        r.add(note("Checks each part of the connection in turn: the server's config, its API, signing in to the game service, how other players reach this PC (as the overlay shows it in game), and the server's helper for playing over the internet."));
        if self.testing {
            r.add(working("Testing…"));
        } else {
            r.add(buttons(vec![super::b("test", "Run the test")]));
        }
        match &self.tests {
            Some(Err(e)) => {
                r.add(note(e.clone()));
            }
            Some(Ok(list)) => {
                for (name, status, note_text) in list {
                    let tone = match status.as_str() {
                        "ok" => "ok",
                        "warn" => "warn",
                        _ => "fail",
                    };
                    r.add(result(name.clone(), tone, note_text.clone()));
                }
            }
            None => {}
        }
        r.card("Network adapter");
        r.add(note("The game uses one network adapter for matches: automatically, the one that reaches your server, chosen each time the game starts. Pin one only if your group plays over a VPN on purpose."));
        let mut options = vec![(String::new(), st(n, "automatic"))];
        for a in arr(n, "adapters") {
            if let (Some(name), Some(ip)) = (a.get(0).and_then(Value::as_str), a.get(1).and_then(Value::as_str)) {
                options.push((name.to_string(), format!("{name} · {ip}")));
            }
        }
        let adapter = st(n, "adapter");
        r.add(choice("adapter", "Adapter", &options, &adapter).enabled(!locked));
        r.add_if(!adapter.is_empty(), || {
            switch("require_adapter", "Don't start the game without this adapter", bo(n, "require_adapter"))
                .enabled(!locked)
                .sub("Stops the game from quietly playing over the wrong network when your VPN is off.")
        });
    }

    fn servers(&mut self, _app: &AppHandle, v: &Value, r: &mut Rows) {
        if v.get("game").is_none_or(Value::is_null) {
            return;
        }
        let locked = !st(v, "managed").is_empty();
        let profiles = arr(v, "profiles");
        r.card("Servers and accounts");
        if profiles.is_empty() {
            r.add(note("None yet. Join one on the Play screen."));
            return;
        }
        r.add(switch("show_passwords", "Show passwords", self.show_passwords));
        for p in profiles {
            let name = st(p, "name");
            let password = if self.show_passwords {
                self.passwords.iter().find(|(n, _)| *n == name).map_or("…".to_string(), |(_, pw)| pw.clone())
            } else {
                "••••••••".into()
            };
            if self.renaming.as_ref().is_some_and(|(n, _)| *n == name) {
                let to = self.renaming.as_ref().map(|(_, t)| t.clone()).unwrap_or_default();
                r.add(item(st(p, "server"), "A new name on this server; friends and invites carry on"));
                r.add(
                    field("rename", "", to.clone())
                        .placeholder("new name")
                        .button(super::b("rename-save", "Save name").enabled(!to.trim().is_empty()))
                        .button(super::b("rename-cancel", "Cancel")),
                );
                continue;
            }
            let mut actions = vec![super::b(format!("rename:{name}"), "Rename").kind("fix").enabled(!locked)];
            if !bo(p, "current") {
                actions.push(super::b(format!("use:{name}"), "Use").kind("fix").enabled(!locked));
            }
            actions.push(super::b(format!("remove:{name}"), "Remove").kind("fix").enabled(!locked));
            r.add(
                item(st(p, "server"), format!("{} · {password}", st(p, "username")))
                    .on(bo(p, "current"))
                    .tone(if bo(p, "current") { "accent" } else { "" })
                    .buttons(actions),
            );
        }
    }

    fn identity(&mut self, app: &AppHandle, v: &Value, r: &mut Rows) {
        let id = json(&crate::settings::identity(app.state::<Launcher>()));
        r.card("Identity and friends");
        r.add(note(
            "Your identity follows you between servers that share friends: friends you make on one show up on the others. It also signs you in to your accounts on a new PC.",
        ));
        let short = st(&id, "short");
        if !short.is_empty() {
            r.add(item(format!("Your identity {short}"), "").button(super::b("export-identity", "Copy to move it to another PC").kind("fix")));
        } else if !st(&id, "error").is_empty() {
            r.add(bad(format!("Your identity can't be read: {}", st(&id, "error"))));
        } else {
            r.add(note("None yet: it's made when you join a server that supports it."));
        }
        r.add(note(
            "On a new PC, import your identity before you connect to a server: connecting first makes a new identity, and a new account under another name.",
        ));
        r.add(
            field("import", "Import", self.import_text.clone())
                .placeholder("5th-echelon-identity:v1:…")
                .secret()
                .button(super::b("import", "Use this identity").enabled(!self.import_text.trim().is_empty() && self.confirm.is_none())),
        );
        if let Some(e) = &self.import_error {
            r.add(bad(e.clone()));
        }
        if self.confirm == Some("import-identity") {
            let accounts = num(&id, "accounts") as usize;
            let linked = match accounts {
                0 => String::new(),
                1 => " Your saved account here is linked to it.".into(),
                n => format!(" Your {n} saved accounts here are linked to it."),
            };
            let current = if short.is_empty() { "this PC's identity".to_string() } else { short.clone() };
            r.add(warn(format!(
                "This replaces {current}.{linked} Accounts made with it can only be signed in to with it: if you'll want them, copy it first. A copy of it stays in the launcher's folder."
            )));
            r.add(buttons(vec![super::b("import-confirmed", "Replace it").kind("danger"), super::b("cancel", "Cancel")]));
        }
        r.add(rule());
        let saved = st(v, "directory");
        let typed = self.directory.clone().unwrap_or_else(|| saved.clone());
        r.add(
            field("directory", "Server directory", typed.clone())
                .placeholder("https://coordinator.example.com")
                .button(super::b("save-directory", "Save").enabled(typed != saved)),
        );
        if let Some(e) = &self.directory_error {
            r.add(bad(e.clone()));
        }
        r.add(note("The servers to choose from: the community network unless you join another network. Empty: none."));
    }

    pub(crate) fn save(&mut self, v: &Value, r: &mut Rows) {
        let Some(s) = v.get("save").filter(|s| !s.is_null()) else { return };
        r.card("Save game");
        let no_folder = st(s, "no_folder");
        if !no_folder.is_empty() {
            r.add(note(no_folder));
            return;
        }
        // What the save is: when it was saved, how far along, its backups; then what can be
        // done with it. Bringing another in is a card of its own (five buttons in one row ran off
        // the card).
        if bo(s, "missing") {
            r.add(item("No save yet", "Play makes a rank 5 one, or bring one in below."));
        } else {
            r.add(item("Last saved", st(s, "saved")));
            let progress = st(s, "progress");
            if !progress.is_empty() {
                r.add(item("Progress", progress));
            }
            let backups = num(s, "backups") as u64;
            r.add(item(
                "Backups",
                match backups {
                    0 => "None yet: one's made before anything replaces it".to_string(),
                    1 => format!("1, made {}", st(s, "last_backup")),
                    n => format!("{n}, the latest {}", st(s, "last_backup")),
                },
            ));
        }
        r.add(note(st(s, "path")));
        let mut row = Vec::new();
        if !bo(s, "missing") {
            row.push(super::b("act:backup-save", "Back up now"));
        }
        row.push(super::b("act:open-save-folder", "Open folder"));
        r.add(buttons(row));

        r.card("Bring in another save");
        r.add(note(
            "Yours from Ubisoft Connect, a save file, or a new one at rank 5. The current save is backed up first.",
        ));
        let mut row = Vec::new();
        if !st(s, "ubisoft").is_empty() {
            row.push(super::b("act:import-ubisoft", "From Ubisoft Connect"));
        }
        row.push(super::b("act:import-save", "A save file…"));
        row.push(super::b("ask-new-save", "New rank 5 save…"));
        r.add(buttons(row));
        if self.confirm == Some("new-save") {
            r.add(warn("Replace your save with a new rank 5 one? The current save is backed up first."));
            r.add(buttons(vec![super::b("new-save", "Replace").kind("danger"), super::b("cancel", "Cancel")]));
        }
    }

    fn client(&mut self, v: &Value, r: &mut Rows) {
        let Some(c) = v.get("client").filter(|c| !c.is_null()) else { return };
        r.card("5th Echelon client");
        r.add(note(st(c, "state")));
        if !st(c, "version").is_empty() {
            r.add(note(format!("Carried by this launcher: {}", st(c, "version"))));
        }
        let mut row = Vec::new();
        if bo(c, "can_install") {
            row.push(super::b("act:install", "Install"));
        }
        if bo(c, "can_uninstall") {
            row.push(super::b("ask-uninstall", "Uninstall…"));
        }
        if !row.is_empty() {
            r.add(buttons(row));
        }
        if self.confirm == Some("uninstall") {
            r.add(warn("Put the game's own online DLL back? The game then won't connect to 5th Echelon servers. Your 5th Echelon settings, accounts, saves and identity stay on this PC, so Install brings it all back."));
            r.add(buttons(vec![super::b("uninstall", "Uninstall").kind("danger"), super::b("cancel", "Cancel")]));
        }
    }

    fn display(&mut self, r: &mut Rows) {
        r.card("Look");
        r.add(note(
            "Dark, light, or as your system is; the title bar's button changes it too. The game's overlay follows it.",
        ));
        let current = match launcher::prefs::Prefs::theme() {
            launcher::prefs::Theme::Dark => "dark",
            launcher::prefs::Theme::Light => "light",
            launcher::prefs::Theme::System => "system",
        };
        r.add(segments(
            "theme",
            "",
            &[("dark".into(), "Dark".into()), ("light".into(), "Light".into()), ("system".into(), "As the system".into())],
            current,
        ));
        r.card("Size");
        r.add(note(
            "The launcher scales with its window: resize it, maximise it, or go full screen. Make everything bigger or smaller here; Reset puts it back.",
        ));
        let (min, max) = (launcher::prefs::clamp(0.0), launcher::prefs::clamp(9.0));
        let mut row = vec![
            super::b("smaller", "−").enabled(self.size > min + 0.01),
            super::b("bigger", "+").enabled(self.size < max - 0.01),
        ];
        if (self.size - 1.0).abs() > 0.01 {
            row.push(super::b("size-reset", "Reset"));
        }
        row.push(super::b("fullscreen", "Full screen (F11)"));
        r.add(bold(format!("{} %", (self.size * 100.0).round())));
        r.add(buttons(row));
    }

    fn feedback(&mut self, v: &Value, r: &mut Rows) {
        if v.get("game").is_some_and(|g| !g.is_null()) {
            r.card("While you play");
            r.add(note("The game sends the server it's signed in to its warnings, errors and network events as they happen, and what it's doing (the game session it's in, its checkpoint saves, which of 5th Echelon's fixes went in and what they did), so the server's admins can see why a join, a connection or a mission failed. Your PC's name, your user folder and your internet address are hidden first."));
            r.add(switch("send_diagnostics", "Send the game's diagnostics to the server", bo(v, "send_diagnostics")));
            r.add(note("With it on, when the server sees a problem in your game (a join that failed, your connection dropping, the game restarting in a match), it can also ask for the 15 minutes of the game's log before it, at most once an hour (never the whole log), private details hidden the same way."));
        }
        r.card("After a game");
        r.add(note("When something goes wrong in a game (a join that fails, a crash), and now and then otherwise, the launcher asks how it went once the game closes. Your answer goes to the server's admins, with your logs if you agree: your PC's name, your user folder and your internet address are hidden first."));
        r.add(switch("ask_feedback", "Ask me how my games went", bo(v, "ask_feedback")));
        r.card("Report a problem");
        r.add(note("Something wrong now? Tell the admins of the server you play on, with your logs."));
        r.add(buttons(vec![super::b("send-feedback", "Send feedback")]));
        r.card("Your reports");
        r.add(note("What you sent, and what the admins replied."));
        if self.reports.is_none() {
            self.reports = Some(crate::settings::my_reports().map(|o| o.map(|list| list.iter().map(json).collect())));
        }
        match self.reports.as_ref() {
            Some(Ok(None)) => {
                r.add(note("Connect to a server first: your reports are read with the identity made then."));
            }
            Some(Err(e)) => {
                r.add(note(e.clone()));
            }
            Some(Ok(Some(list))) if list.is_empty() => {
                r.add(note("None yet."));
            }
            Some(Ok(Some(list))) => {
                for rep in list {
                    let reply = st(rep, "reply");
                    let mut row = item(st(rep, "line"), st(rep, "said"));
                    if !reply.is_empty() {
                        row = row.text(format!("Reply: {reply}")).tone("accent");
                    }
                    r.add(row);
                }
            }
            None => {}
        }
        r.add(buttons(vec![super::b("reports", "Check again").kind("link")]));
    }

    fn about(&mut self, v: &Value, r: &mut Rows) {
        r.card("About");
        r.add(bold(format!("{} {}", st(v, "product"), st(v, "release"))));
        r.add(note("Developed by JDevWebb, building on 5th Echelon by unixoide and its contributors."));
        r.add(note("Free, and provided as is, without warranty of any kind. Not affiliated with or endorsed by Ubisoft."));
        r.add(note(
            "The launcher is made with Slint (slint.dev), under its royalty-free licence; its type is IBM Plex (SIL Open Font License).",
        ));
        r.add(buttons(vec![super::b("check-update", "Check for updates"), super::b("releases", "Releases").kind("link")]));
        match &self.update {
            Some(Ok((version, true))) => {
                r.add(accent(format!("Version {version} is available.")));
            }
            Some(Ok((version, false))) => {
                r.add(note(format!("Up to date (latest release {version}).")));
            }
            Some(Err(e)) => {
                r.add(note(e.clone()));
            }
            None => {}
        }
    }
}

fn notifications(v: &Value, r: &mut Rows) {
    let Some(n) = v.get("notifications").and_then(Value::as_array) else { return };
    let toasts = pairs(arr(v, "toasts"));
    r.card("Chat in the game");
    r.add(note(
        "When a chat message pops up in the corner of the game. Messages that name you (@name) count as mentions. Invitations and friend requests always pop up.",
    ));
    for (i, (key, label)) in [("notify_global", "Global"), ("notify_lobby", "Lobby"), ("notify_direct", "Messages from friends")]
        .into_iter()
        .enumerate()
    {
        let current = n.get(i).and_then(Value::as_str).unwrap_or("");
        r.add(choice(key, label, &toasts, current));
    }
    r.add(note(
        "\"Outside matches\" keeps Global quiet while you play a match; it still tells you when someone names you.",
    ));
}

fn advanced(v: &Value, r: &mut Rows) {
    if v.get("game").is_none_or(Value::is_null) {
        return;
    }
    let all = bo(v, "hooks_all");
    r.card("Hooks");
    r.add(note("Which parts of the game the client patches. Leave these alone unless you're debugging."));
    r.add(switch("hooks_all", "All hooks", all));
    for h in arr(v, "hooks") {
        if let (Some(id), Some(label), Some(on)) = (h.get(0).and_then(Value::as_str), h.get(1).and_then(Value::as_str), h.get(2).and_then(Value::as_bool)) {
            r.add(switch(format!("hook:{id}"), label, on).enabled(!all));
        }
    }
    r.add(note("For working out why two copies of the game refuse each other (\"a different version of the game\"): the game writes what its data version is made of to bl-dataversion.txt in its folder. Off, the file is deleted."));
    r.add(switch("log_data_version", "Write the game's data version (bl-dataversion.txt)", bo(v, "log_data_version")));
}
