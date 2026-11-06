//! The guided setup (the Tauri launcher's Setup.vue): find the game, choose a server, pick a
//! name when there's no account there yet, and on a first setup the new game options. The setup
//! itself (installing the client, the account, the settings) is the shared library's, run by
//! core::connect; its steps show in the activity bar. Shown instead of Play until there's a game
//! and a server, while the player changes server, and on a first setup until its options are
//! chosen.

use super::*;
use crate::commands::Launcher;
use crate::tauri::Manager;

const STEPS: [&str; 5] = ["Find the game", "Choose a server", "Pick your name", "Game options", "Play"];
const INTRO_SETUP: &str = "These change how the game plays. Each is set as we recommend; tick or untick any you like.";

#[derive(Default)]
pub struct SetupPage {
    /// A first setup (no game or no server when the launcher started): its options come last.
    pub fresh: bool,
    /// The player asked to change server (Play's "Choose a server"): the server step again.
    pub editing: bool,
    server: String,
    typed: bool,
    lan: Vec<String>,
    looked: bool,
    /// The name step: the server, and whether the name was taken.
    naming: Option<(String, bool)>,
    nick: String,
    choices: Option<Vec<bool>>,
}

impl SetupPage {
    pub fn new(fresh: bool) -> Self {
        Self { fresh, ..Default::default() }
    }

    /// Whether the setup shows instead of Play.
    pub fn shown(&self, info: &crate::core::Info) -> bool {
        !info.managed && (info.game_dir.is_none() || info.server.is_none() || self.editing || self.fresh || self.naming.is_some())
    }

    /// A setup that stopped for a name: its name step.
    pub fn needs_name(&mut self, server: String, suggested: String, taken: bool) {
        self.nick = suggested;
        self.naming = Some((server, taken));
    }

    fn step(&self, info: &crate::core::Info) -> usize {
        if info.game_dir.is_none() {
            0
        } else if self.naming.is_some() {
            2
        } else if info.server.is_none() || self.editing {
            1
        } else if self.fresh {
            3
        } else {
            1
        }
    }
}

impl Screen for SetupPage {
    fn live_edits(&self) -> bool {
        true
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        let core = app.state::<Launcher>().0.clone();
        let info = crate::core::info(&core);
        let step = self.step(&info);
        let mut r = Rows::default();
        const WHAT: &str = "The launcher sets everything up and keeps the game's original files, so you can undo it any time. You need your own copy of the game.";
        // A first setup's first page: the mark and the name, as the launcher's welcome.
        if self.fresh && step <= 1 && !info.connecting {
            r.add(mark("Welcome to 5th Echelon Enhanced", WHAT));
        } else {
            r.add(note(WHAT));
        }
        let steps: Vec<(String, String)> = STEPS.iter().enumerate().map(|(i, n)| (i.to_string(), format!("{}  {n}", i + 1))).collect();
        r.add(segments("steps", format!("Step {} of {}", step + 1, STEPS.len()), &steps, &step.to_string()));
        match step {
            0 => {
                r.card("Find Splinter Cell: Blacklist");
                r.add(note("It wasn't in Steam, Ubisoft Connect or the usual folders. Choose the folder you installed it to."));
                r.add(buttons(vec![super::b("pick", "Choose folder…").kind("primary"), super::b("find", "Look again")]));
            }
            1 => {
                r.card("Choose a server");
                r.add(note("Pick the closest server, or type a server's or a network's address. Connecting also installs the 5th Echelon client, keeping the game's original file so you can undo it."));
                r.add(item("Splinter Cell: Blacklist found", info.game_dir.clone().unwrap_or_default()).tone("ok"));
                let servers = crate::core::servers(&core, false);
                if let Some(best) = servers.rows.iter().find(|r| r.best) {
                    if self.server.trim().is_empty() || !self.typed {
                        self.server = best.host.clone();
                    }
                }
                if servers.network.is_some() && servers.rows.is_empty() {
                    r.add(note("The directory lists no servers right now."));
                }
                for s in &servers.rows {
                    let chosen = self.server.trim() == s.host;
                    let mut sub = format!("{} · {} online", s.host, s.players);
                    if s.best {
                        sub.push_str(" · Best for you");
                    } else if s.friends_only {
                        sub.push_str(" · Friends only");
                    }
                    if !s.playing.is_empty() {
                        sub = format!("{}\n{sub}", s.playing);
                    }
                    let ping = s.ping.map_or("no answer".into(), |p| format!("{p} ms"));
                    r.add(
                        item(s.title.clone(), sub).text(ping).on(chosen).tone(if chosen { "accent" } else { "" }).button(
                            super::b(format!("pick:{}", s.host), if chosen { "Chosen" } else { "Choose" })
                                .kind("fix")
                                .enabled(!chosen && !info.connecting),
                        ),
                    );
                }
                r.add(
                    field("server", "Or a server's address", self.server.clone())
                        .placeholder("play.example.org or 192.168.1.20")
                        .enabled(!info.connecting),
                );
                let mut row = Vec::new();
                if servers.network.is_some() {
                    row.push(super::b("ping", "Ping again"));
                }
                row.push(super::b("lan", "Find on my network"));
                r.add(buttons(row));
                if self.lan.len() > 1 {
                    r.add(note("Several servers answered on this network. Choose the one you want:"));
                    r.add(buttons(self.lan.iter().map(|ip| super::b(format!("pick:{ip}"), ip.clone())).collect()));
                } else if self.looked && self.lan.is_empty() {
                    r.add(note("No server answered on this network."));
                }
                if let Some(n) = &servers.note {
                    r.add(note(n.clone()));
                }
                if info.connecting {
                    r.add(working("Connecting…"));
                } else {
                    let mut go = vec![super::b("connect", "Connect").kind("primary").enabled(!self.server.trim().is_empty())];
                    if info.server.is_some() {
                        go.push(super::b("cancel", "Cancel"));
                    }
                    r.add(buttons(go));
                }
            }
            2 => {
                let (server, taken) = self.naming.clone().unwrap_or_default();
                r.card("Pick your name");
                r.add(note(format!(
                    "You don't have an account on {server} yet. Choose the name other players will see; your identity signs you in to it from now on."
                )));
                r.add_if(taken, || warn(format!("Someone on {server} already has that name: choose another.")));
                r.add(
                    field("nick", "Your name", self.nick.clone())
                        .placeholder("what other players see")
                        .enabled(!info.connecting),
                );
                if info.connecting {
                    r.add(working("Creating your account…"));
                } else {
                    let mut go = vec![super::b("create", "Create account").kind("primary").enabled(!self.nick.trim().is_empty())];
                    if info.server.is_some() {
                        go.push(super::b("cancel", "Cancel"));
                    }
                    r.add(buttons(go));
                }
            }
            _ => {
                let inner = core.lock();
                let game = inner.game.as_ref();
                let pending = launcher::new_options::pending(game);
                let offered = game.map(|g| launcher::new_options::offered(&pending, &g.dir)).unwrap_or_default();
                let blocked: Vec<Option<(&str, &str)>> = pending.iter().map(|o| game.and_then(|g| (o.blocked)(&g.dir))).collect();
                drop(inner);
                if pending.is_empty() {
                    self.fresh = false;
                }
                let choices = self.choices.get_or_insert_with(|| offered.clone());
                if choices.len() != pending.len() {
                    *choices = offered.clone();
                }
                r.card("Game options");
                r.add(note(INTRO_SETUP));
                for (i, o) in pending.iter().enumerate() {
                    let mut row = switch(format!("option:{i}"), o.title, choices[i]).sub(blocked[i].map_or(o.text, |b| b.1));
                    if let Some((badge, _)) = blocked[i] {
                        row = row.enabled(false);
                        row.text = badge.to_uppercase();
                    } else if o.recommended {
                        row.text = "RECOMMENDED".into();
                    }
                    r.add(row);
                }
                r.add(note(launcher::new_options::LATER));
                r.add(buttons(vec![super::b("options", "Continue").kind("primary")]));
            }
        }
        PageView {
            kicker: "Operative setup".into(),
            title: "Get online in a minute".into(),
            rows: r.done(),
            ..Default::default()
        }
    }

    fn toggled(&mut self, _app: &AppHandle, id: &str, on: bool) {
        if let Some(i) = id.strip_prefix("option:").and_then(|i| i.parse::<usize>().ok()) {
            if let Some(c) = self.choices.as_mut().and_then(|c| c.get_mut(i)) {
                *c = on;
            }
        }
    }

    fn edited(&mut self, id: &str, text: &str) {
        match id {
            "server" => {
                self.server = text.into();
                self.typed = true;
            }
            "nick" => self.nick = text.into(),
            _ => {}
        }
    }

    fn accepted(&mut self, app: &AppHandle, id: &str, text: &str) {
        self.edited(id, text);
        self.pressed(app, if id == "nick" { "create" } else { "connect" });
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        let core = app.state::<Launcher>().0.clone();
        match id {
            "pick" => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Choose the folder with Blacklist_game.exe").pick_folder() {
                    crate::core::choose_game_dir(&core, &dir.display().to_string());
                }
            }
            "find" => {
                if let Some(dir) = crate::core::find_games().first() {
                    crate::core::choose_game_dir(&core, dir);
                }
            }
            "ping" => {
                crate::core::servers(&core, true);
            }
            "lan" => {
                self.lan = crate::core::find_on_network().unwrap_or_default();
                self.looked = true;
                if let [one] = self.lan.as_slice() {
                    self.server = one.clone();
                    self.typed = false;
                }
            }
            "connect" => {
                if !self.server.trim().is_empty() && crate::core::connect(&core, self.server.trim(), None) {
                    self.editing = false;
                }
            }
            "create" => {
                if let Some((server, _)) = self.naming.take() {
                    crate::core::connect(&core, &server, Some(self.nick.trim().to_string()));
                }
            }
            "cancel" => {
                self.editing = false;
                self.naming = None;
            }
            "options" => {
                let mut inner = core.lock();
                let crate::core::Inner { game, notices, .. } = &mut *inner;
                if let Some(game) = game.as_mut() {
                    let pending = launcher::new_options::pending(Some(game));
                    if let Some(choices) = self.choices.take().filter(|c| c.len() == pending.len()) {
                        launcher::new_options::save(game, notices, &pending, &choices);
                    }
                }
                drop(inner);
                self.fresh = false;
                // What else waits before the first game (the diagnostics question).
                use crate::tauri::Emitter;
                let _ = app.emit("setup-done", ());
            }
            _ if id.starts_with("pick:") => {
                self.server = id[5..].to_string();
                self.typed = false;
                self.lan.clear();
            }
            _ => {}
        }
    }
}
