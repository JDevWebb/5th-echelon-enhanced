//! Host (the egui launcher's server.rs, through the shared library): run a 5th Echelon server
//! on this PC, and manage a server (this one or another) through its admin API: players,
//! games and the log. A server started here stops with the launcher.

use std::net::IpAddr;
use std::process::Child;
use std::sync::Mutex;

use dedicated_server_config::Config as ServerConfig;
use launcher::server as s;
use launcher::services::Admin;
use serde::Deserialize;
use serde::Serialize;
use tauri::Manager;

use crate::commands::changed;
use crate::commands::notifier;
use crate::commands::Launcher;
use crate::tauri;

#[derive(Default)]
pub struct Host {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    loaded: bool,
    exe: Option<std::path::PathBuf>,
    cfg: Option<ServerConfig>,
    cfg_error: Option<String>,
    process: Option<Child>,
    /// When the server started here was started (its uptime).
    started: Option<std::time::Instant>,
    starting: bool,
    downloading: bool,
    /// The server being managed: this PC's (after Start) or one connected to.
    admin: Option<Admin>,
}

impl Host {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stops a server started here (the launcher is closing).
    pub fn stop(&self) {
        let mut st = self.lock();
        st.started = None;
        if let Some(mut child) = st.process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl State {
    fn load(&mut self) {
        self.loaded = true;
        self.exe = s::server_exe();
        self.cfg = None;
        self.cfg_error = None;
        if let Some(exe) = &self.exe {
            match s::load_config(exe) {
                Ok(cfg) => self.cfg = Some(cfg),
                Err(e) => self.cfg_error = Some(e),
            }
        }
    }

    fn running(&mut self) -> bool {
        match self.process.as_mut().map(Child::try_wait) {
            Some(Ok(None)) => true,
            Some(_) => {
                self.process = None;
                self.started = None;
                false
            }
            None => false,
        }
    }
}

/// The community API's switches (port 80, for launchers and the overlay).
#[derive(Serialize, Deserialize)]
pub struct CommunityApi {
    pub info: bool,
    pub presence: bool,
    pub accounts: bool,
    pub unhandled: bool,
}

#[derive(Serialize)]
pub struct HostView {
    exe_name: &'static str,
    /// The server is next to the launcher.
    found: bool,
    error: Option<String>,
    running: bool,
    starting: bool,
    downloading: bool,
    /// The form: the address it listens on (None: every address), this PC's adapters, the
    /// trusted subnet, the community API's switches.
    listen: Option<String>,
    adapters: Vec<(String, String)>,
    trusted_subnet: String,
    api: Option<CommunityApi>,
    /// The admin API being managed, and whether it's this PC's.
    managing: Option<String>,
    local: bool,
    /// How long the server started here has run, in seconds.
    uptime: Option<u64>,
}

pub fn host(state: tauri::State<Host>) -> HostView {
    let mut st = state.lock();
    if !st.loaded {
        st.load();
    }
    let running = st.running();
    let cfg = st.cfg.as_ref();
    HostView {
        exe_name: s::SERVER_EXE,
        found: st.exe.is_some(),
        error: st.cfg_error.clone(),
        running,
        starting: st.starting,
        downloading: st.downloading,
        listen: cfg.map(|c| c.api_server.ip()).filter(|ip| !ip.is_unspecified()).map(|ip| ip.to_string()),
        adapters: setup::net::adapters().into_iter().map(|(name, ip)| (name, ip.to_string())).collect(),
        trusted_subnet: cfg.map(s::trusted_subnet).unwrap_or_default(),
        api: cfg.map(|c| CommunityApi {
            info: c.community_api.info,
            presence: c.community_api.presence,
            accounts: c.community_api.accounts,
            unhandled: c.community_api.unhandled,
        }),
        managing: st.admin.as_ref().map(|a| a.api.clone()),
        local: st.process.is_some(),
        uptime: st.started.filter(|_| running).map(|t| t.elapsed().as_secs()),
    }
}

fn say(app: &tauri::AppHandle, result: Result<String, String>) {
    let launcher = app.state::<Launcher>();
    let mut inner = launcher.lock();
    match result {
        Ok(line) => inner.notices.info(line),
        Err(e) => inner.notices.error(e),
    }
    drop(inner);
    changed(app);
}

/// Writes the addresses and switches into `service.toml` and starts the server; this PC's
/// server is then the one managed.
pub fn host_start(app: tauri::AppHandle, state: tauri::State<Host>, listen: Option<String>, public_ip: String, trusted_subnet: String, api: CommunityApi) -> Result<(), String> {
    let listen: Option<IpAddr> = match listen.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => Some(l.parse().map_err(|_| format!("\"{l}\" isn't an IP address."))?),
        None => None,
    };
    let exe = {
        let mut st = state.lock();
        if st.running() || st.starting {
            return Ok(());
        }
        let exe = st.exe.clone().ok_or("The server isn't next to the launcher.")?;
        let cfg = st.cfg.as_mut().ok_or("The server's settings can't be read.")?;
        cfg.community_api.info = api.info;
        cfg.community_api.presence = api.presence;
        cfg.community_api.accounts = api.accounts;
        cfg.community_api.unhandled = api.unhandled;
        s::prepare(&exe, cfg, listen, &public_ip, &trusted_subnet)?;
        st.starting = true;
        exe
    };
    let started = launcher::task::guarded(|| s::spawn(&exe));
    let mut st = state.lock();
    st.starting = false;
    match started {
        Ok((child, key)) => {
            st.process = Some(child);
            st.started = Some(std::time::Instant::now());
            st.admin = st.cfg.as_ref().map(|c| Admin { api: s::local_api(c), key });
            drop(st);
            say(&app, Ok("The server is running.".into()));
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn host_stop(app: tauri::AppHandle, state: tauri::State<Host>) {
    state.stop();
    state.lock().admin = None;
    say(&app, Ok("The server stopped.".into()));
}

/// Downloads the server from the latest release, next to the launcher (in the activity bar).
pub fn host_download(app: tauri::AppHandle, state: tauri::State<Host>) {
    state.lock().downloading = true;
    let activity = app.state::<Launcher>().lock().notices.start_with(notifier(&app), "Downloading the server");
    changed(&app);
    let result = launcher::task::guarded(|| s::download().map_err(|e| e.to_string()));
    let mut st = state.lock();
    st.downloading = false;
    st.load();
    drop(st);
    match result {
        Ok(()) => activity.done("Downloaded the server."),
        Err(e) => activity.fail(format!("Couldn't download the server: {e}"), &[launcher::activity::Action::CopyDetails]),
    }
    changed(&app);
}

/// Manages a server through its admin API with its key: "plain" when the address isn't
/// encrypted and the player hasn't allowed sending the key that way (`allow_plain`), "ok" when
/// connected.
pub fn host_connect(state: tauri::State<Host>, address: String, key: String, allow_plain: bool) -> Result<&'static str, String> {
    let (api, plain) = s::admin_url(&address)?;
    if plain && !allow_plain {
        return Ok("plain");
    }
    state.lock().admin = Some(Admin { api, key: key.trim().to_string() });
    Ok("ok")
}

pub fn host_disconnect(state: tauri::State<Host>) {
    let mut st = state.lock();
    if st.process.is_none() {
        st.admin = None;
    }
}

#[derive(Serialize)]
pub struct PlayerView {
    id: String,
    name: String,
    ips: String,
}

#[derive(Serialize)]
pub struct GameView {
    id: u32,
    kind: String,
    who: String,
    /// Everyone in it by name, its creator first (what each player is doing, on the page).
    players: Vec<String>,
}

/// The managed server's players and games.
#[derive(Serialize)]
pub struct Lists {
    players: Result<Vec<PlayerView>, String>,
    games: Result<Vec<GameView>, String>,
}

pub fn host_lists(state: tauri::State<Host>) -> Option<Lists> {
    let admin = state.lock().admin.clone()?;
    let players = launcher::task::guarded(|| admin.users())
        .map(|list| {
            list.into_iter()
                .map(|u| PlayerView {
                    id: u.id,
                    name: u.username,
                    ips: u.ips.join(", "),
                })
                .collect()
        })
        .map_err(|e| format!("Couldn't load the players: {e}"));
    let games = launcher::task::guarded(|| admin.games())
        .map(|list| {
            list.into_iter()
                .map(|g| GameView {
                    id: g.id,
                    kind: g.game_type,
                    who: format!("{} with {}", g.creator, g.participants.join(", ")),
                    players: std::iter::once(g.creator.clone()).chain(g.participants.iter().cloned()).collect(),
                })
                .collect()
        })
        .map_err(|e| format!("Couldn't load the games: {e}"));
    Some(Lists { players, games })
}

/// Deletes a player's account ("player", by id) or ends a game ("game").
pub fn host_delete(app: tauri::AppHandle, state: tauri::State<Host>, what: String, id: String) {
    let Some(admin) = state.lock().admin.clone() else { return };
    let result = launcher::task::guarded(|| {
        if what == "player" {
            admin.delete_user(id.clone())
        } else {
            admin.delete_game(id.parse().unwrap_or_default())
        }
    });
    if let Err(e) = result {
        say(&app, Err(format!("Couldn't delete: {e}")));
    }
}

#[derive(Serialize)]
pub struct LogView {
    time: String,
    /// "warn", "error" or "".
    tone: &'static str,
    msg: String,
}

/// The last 300 lines of this PC's server log, from `min` up ("debug", "info", "warn", "error").
pub fn host_logs(state: tauri::State<Host>, min: String) -> Vec<LogView> {
    use s::LogLevel as L;
    let Some(exe) = state.lock().exe.clone() else { return Vec::new() };
    let min = match min.as_str() {
        "debug" => L::Debug,
        "warn" => L::Warn,
        "error" => L::Error,
        _ => L::Info,
    };
    s::read_logs(&exe, min, 300)
        .into_iter()
        .map(|i| LogView {
            time: i.ts.get(11..19).unwrap_or(&i.ts).to_string(),
            tone: match i.level {
                L::Warn => "warn",
                L::Error | L::Critical => "error",
                _ => "",
            },
            msg: i.msg,
        })
        .collect()
}
