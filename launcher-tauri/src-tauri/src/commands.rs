//! What the page asks the Rust side for. The work itself is the egui launcher's (its library,
//! `launcher`), so the two set up and start the game the same way and keep the same files.

use std::process::Child;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use launcher::activity::Action;
use launcher::activity::Activities;
use launcher::activity::Progress;
use launcher::activity::State;
use launcher::connect;
use launcher::prefs::Game;
use launcher::prefs::PlayMode;
use launcher::prefs::Prefs;
use serde::Serialize;
use setup::directory::Listing;
use tauri::Emitter;
use tauri::Manager;

const PRODUCT: &str = "5th Echelon Enhanced";
const RELEASE: &str = env!("FE_RELEASE");

/// What the window works on: the game folder, the activity bar's list, the game it started.
pub struct Launcher {
    inner: Mutex<Inner>,
}

struct Inner {
    game: Option<Game>,
    notices: Activities,
    running: Option<Child>,
    /// The network's servers with this PC's ping to each, and what to say about the list.
    directory: Option<Vec<(Listing, Option<u32>)>>,
    directory_note: Option<String>,
    /// The setup running or last run (to try again), and its activity when it failed.
    setup: Option<connect::Asked>,
    setup_running: bool,
    setup_failed: Option<u64>,
}

impl Launcher {
    /// The game chosen before, or the first one found.
    pub fn new() -> Self {
        let mut notices = Activities::default();
        let game = Game::chosen_or_found().map(|dir| Game::open_tidied(dir, &mut notices));
        Self {
            inner: Mutex::new(Inner {
                game,
                notices,
                running: None,
                directory: None,
                directory_note: None,
                setup: None,
                setup_running: false,
                setup_failed: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Tells the page the activity bar changed (it asks for what to show, `activity`).
fn changed(app: &tauri::AppHandle) {
    let _ = app.emit("activity", ());
}

/// How an activity reports back while it runs, for `Activities::start_with`.
fn notifier(app: &tauri::AppHandle) -> Option<Arc<dyn Fn() + Send + Sync>> {
    let app = app.clone();
    Some(Arc::new(move || changed(&app)))
}

/// The release, the game folder, and the server and account the game is set up for.
#[derive(Serialize)]
pub struct Info {
    product: &'static str,
    release: &'static str,
    game_dir: Option<String>,
    server: Option<String>,
    account: Option<String>,
    /// The launcher uses the community network (Chat, Support and the Roadmap are its).
    community: bool,
    /// What the Play button starts.
    play_mode: PlayMode,
    /// Another tool manages this install: the game starts as it is.
    managed: bool,
    running: bool,
    /// A setup is running.
    connecting: bool,
    /// The longest name an account takes.
    max_name: usize,
}

#[tauri::command(async)]
pub fn info(state: tauri::State<Launcher>) -> Info {
    let mut inner = state.lock();
    let running = game_running(&mut inner);
    if let Some(game) = inner.game.as_mut() {
        game.reload();
    }
    let game = inner.game.as_ref();
    let profile = game.and_then(|g| g.cfg.current_profile().cloned());
    Info {
        product: PRODUCT,
        release: RELEASE,
        game_dir: game.map(|g| g.dir.display().to_string()),
        server: profile.as_ref().map(|p| p.server.clone()).filter(|s| !s.is_empty()),
        account: profile.map(|p| p.user.username).filter(|u| !u.is_empty()),
        community: Prefs::directory().as_deref().is_some_and(Prefs::is_community),
        play_mode: Prefs::play_mode(),
        managed: game.is_some_and(|g| g.managed.is_some()),
        running,
        connecting: inner.setup_running,
        max_name: setup::account::MAX_NAME,
    }
}

/// Whether the game is running: the one started from here, or any (through Steam, as
/// administrator, or by hand).
fn game_running(inner: &mut Inner) -> bool {
    if let Some(child) = inner.running.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return true;
        }
        inner.running = None;
    }
    setup::game::game_running()
}

#[tauri::command(async)]
pub fn set_play_mode(mode: PlayMode) {
    Prefs::set_play_mode(mode);
}

/// Starts the game (`mode`: straight into a lobby or not). Whether it's starting; what
/// happened shows in the activity bar.
#[tauri::command(async)]
pub fn play(app: tauri::AppHandle, state: tauri::State<Launcher>, mode: PlayMode) -> bool {
    let mut inner = state.lock();
    let Inner { game, notices, running, .. } = &mut *inner;
    let Some(game) = game.as_mut() else {
        notices.error("Choose the game's folder first.");
        changed(&app);
        return false;
    };
    let mode = if game.managed.is_some() { PlayMode::Play } else { mode };
    use launcher::start::Refused;
    use setup::launch::Launched;
    // The name check comes with friends (Squad, the next step); until then the server says.
    let started = match launcher::start::start(game, notices, mode, false) {
        Ok(Launched::Game(child)) => {
            *running = Some(child);
            true
        }
        Ok(Launched::Steam) => {
            notices.info("Steam is starting the game.");
            true
        }
        Ok(Launched::GameNotSteam(child, why)) => {
            *running = Some(child);
            notices.error(format!("Steam couldn't start the game ({why}), so the launcher did: no Steam Input this time."));
            true
        }
        Ok(Launched::Elevated) => {
            notices.info("The game was started as administrator, because it's set to run that way.");
            true
        }
        Err(Refused::First(text)) => {
            notices.info(text);
            false
        }
        Err(Refused::Error(text)) => {
            notices.error(text);
            false
        }
    };
    changed(&app);
    started
}

/// The question about diagnostics, while it's to be answered.
#[derive(Serialize)]
pub struct DiagnosticsQuestion {
    title: &'static str,
    what: &'static str,
    private: &'static str,
    later: &'static str,
}

/// A new game option to choose about.
#[derive(Serialize)]
pub struct OptionQuestion {
    id: &'static str,
    title: &'static str,
    text: &'static str,
    recommended: bool,
    /// As first offered: the recommendation, or off where it can't be used.
    on: bool,
    /// Why it can't be used here: a badge and a line.
    blocked: Option<(&'static str, &'static str)>,
}

/// What the player still has to answer before the game starts: the diagnostics first, then the
/// new game options.
#[derive(Serialize)]
pub struct Questions {
    diagnostics: Option<DiagnosticsQuestion>,
    options: Vec<OptionQuestion>,
    options_intro: &'static str,
    options_later: &'static str,
}

#[tauri::command(async)]
pub fn questions(state: tauri::State<Launcher>) -> Questions {
    use launcher::diagnostics as d;
    use launcher::new_options as o;
    let inner = state.lock();
    let game = inner.game.as_ref();
    let diagnostics = d::Ask::pending(game).then_some(DiagnosticsQuestion {
        title: d::TITLE,
        what: d::WHAT,
        private: d::PRIVATE,
        later: d::LATER,
    });
    let pending = o::pending(game);
    let offered = game.map(|g| o::offered(&pending, &g.dir)).unwrap_or_default();
    let options = pending
        .iter()
        .zip(offered)
        .map(|(option, on)| OptionQuestion {
            id: option.id,
            title: option.title,
            text: option.text,
            recommended: option.recommended,
            on,
            blocked: game.and_then(|g| (option.blocked)(&g.dir)),
        })
        .collect();
    Questions {
        diagnostics,
        options,
        options_intro: o::INTRO_UPDATE,
        options_later: o::LATER,
    }
}

/// The last lines of the player's own log that would have been sent, private parts hidden.
#[tauri::command(async)]
pub fn diagnostics_example(state: tauri::State<Launcher>) -> Vec<String> {
    state.lock().game.as_ref().map(launcher::diagnostics::example).unwrap_or_default()
}

#[tauri::command(async)]
pub fn answer_diagnostics(app: tauri::AppHandle, state: tauri::State<Launcher>, send: bool) {
    let mut inner = state.lock();
    let Inner { game, notices, .. } = &mut *inner;
    if let Some(game) = game.as_mut() {
        launcher::diagnostics::answer_with(game, notices, send);
    }
    changed(&app);
}

/// The new game options as chosen, in the order `questions` gave them.
#[tauri::command(async)]
pub fn answer_options(app: tauri::AppHandle, state: tauri::State<Launcher>, choices: Vec<bool>) {
    let mut inner = state.lock();
    let Inner { game, notices, .. } = &mut *inner;
    if let Some(game) = game.as_mut() {
        let pending = launcher::new_options::pending(Some(game));
        if pending.len() == choices.len() {
            launcher::new_options::save(game, notices, &pending, &choices);
            notices.info("Saved. Settings › Game changes them any time.");
        }
    }
    changed(&app);
}

/// The activity bar's activity: the one to show, and how many others wait.
#[derive(Serialize)]
pub struct ActivityView {
    id: u64,
    title: String,
    step: Option<String>,
    /// "working", "done" or "failed".
    state: &'static str,
    message: Option<String>,
    /// How far along (0 to 1), when it can say.
    progress: Option<f32>,
    steps: Vec<String>,
    /// What the player can do about a failure: "retry", "without-logs", "copy-details", with
    /// their labels.
    actions: Vec<(&'static str, &'static str)>,
    waiting: usize,
}

fn action_id(action: Action) -> &'static str {
    match action {
        Action::Retry => "retry",
        Action::WithoutLogs => "without-logs",
        Action::CopyDetails => "copy-details",
    }
}

#[tauri::command(async)]
pub fn activity(state: tauri::State<Launcher>) -> Option<ActivityView> {
    let (a, waiting) = state.lock().notices.current(Instant::now())?;
    let (state, message, actions) = match &a.state {
        State::Working => ("working", None, Vec::new()),
        State::Done => ("done", None, Vec::new()),
        State::Failed { message, actions } => (
            "failed",
            Some(message.clone()).filter(|m| !m.is_empty()),
            actions.iter().map(|a| (action_id(*a), a.label())).collect(),
        ),
    };
    Some(ActivityView {
        id: a.id,
        title: a.title,
        step: a.step,
        state,
        message,
        progress: match a.progress {
            Progress::Unknown => None,
            Progress::Bytes { sent, total } => Some(if total == 0 { 0.0 } else { (sent as f32 / total as f32).clamp(0.0, 1.0) }),
        },
        steps: a.steps.into_iter().map(|s| s.text).collect(),
        actions,
        waiting,
    })
}

#[tauri::command(async)]
pub fn dismiss(app: tauri::AppHandle, state: tauri::State<Launcher>, id: u64) {
    state.lock().notices.dismiss(id);
    changed(&app);
}

/// A button on a failed activity: Copy details answers the text to copy; the others go to the
/// activity's owner (the setup's Try again is run here).
#[tauri::command(async)]
pub fn activity_action(app: tauri::AppHandle, state: tauri::State<Launcher>, id: u64, action: String) -> Option<String> {
    let mut inner = state.lock();
    if action == "copy-details" {
        return inner.notices.details(id);
    }
    let action = match action.as_str() {
        "retry" => Action::Retry,
        "without-logs" => Action::WithoutLogs,
        _ => return None,
    };
    inner.notices.press(id, action);
    let retry_setup = action == Action::Retry && inner.setup_failed == Some(id) && !inner.setup_running;
    let asked = inner.setup.clone().filter(|_| retry_setup);
    drop(inner);
    if let Some(asked) = asked {
        start_setup(&app, asked);
    }
    changed(&app);
    None
}

/// The game folders found on this PC (Steam, Ubisoft Connect, the usual places).
#[tauri::command(async)]
pub fn find_games() -> Vec<String> {
    setup::game::find_game_dirs().iter().map(|d| d.display().to_string()).collect()
}

/// Works on the game in `dir` (or a folder inside it) from now on. Whether it's a game folder.
#[tauri::command(async)]
pub fn choose_game(app: tauri::AppHandle, state: tauri::State<Launcher>, dir: String) -> bool {
    let mut inner = state.lock();
    let Some(found) = setup::game::game_dir_in(std::path::Path::new(&dir)) else {
        inner.notices.error(format!("{dir} doesn't contain Splinter Cell: Blacklist."));
        changed(&app);
        return false;
    };
    Prefs::remember(&found);
    inner.game = Some(Game::open_tidied(found, &mut inner.notices));
    changed(&app);
    true
}

/// Asks for the game's folder with the system's folder picker, and works on it.
#[tauri::command(async)]
pub fn pick_game_folder(app: tauri::AppHandle, state: tauri::State<Launcher>) -> bool {
    use tauri_plugin_dialog::DialogExt;
    let Some(dir) = app.dialog().file().set_title("Choose the folder with Blacklist_game.exe").blocking_pick_folder() else {
        return false;
    };
    let Ok(dir) = dir.into_path() else { return false };
    choose_game(app, state, dir.display().to_string())
}

/// The network's servers, as the server list and menu show them.
#[derive(Serialize)]
pub struct Servers {
    rows: Vec<launcher::server_menu::Row>,
    /// Why the list couldn't be read, or that it's the last one.
    note: Option<String>,
    /// The network's directory, when the launcher has one.
    network: Option<String>,
}

/// The network's servers, pinged now when `fresh` or not yet.
#[tauri::command(async)]
pub fn servers(state: tauri::State<Launcher>, fresh: bool) -> Servers {
    let network = Prefs::directory();
    let have = state.lock().directory.is_some();
    if let Some(url) = network.clone().filter(|_| fresh || !have) {
        let browsed = launcher::services::rt().block_on(launcher::network::server_directory(&url));
        let mut inner = state.lock();
        match browsed {
            Ok(b) => {
                inner.directory = Some(b.servers);
                inner.directory_note = b.note;
            }
            Err(e) => inner.directory_note = Some(format!("The server directory couldn't be read: {e}")),
        }
    }
    let inner = state.lock();
    let current = inner.game.as_ref().and_then(|g| g.cfg.current_profile().map(|p| p.server.clone())).unwrap_or_default();
    let clock = setup::clock::Clock::local();
    Servers {
        rows: inner
            .directory
            .as_deref()
            .map(|d| launcher::server_menu::rows(d, &current, identity::now(), &clock))
            .unwrap_or_default(),
        note: inner.directory_note.clone(),
        network,
    }
}

/// The servers that answered on this PC's networks.
#[tauri::command(async)]
pub fn find_on_network() -> Result<Vec<String>, String> {
    connect::find_on_network().map(|ips| ips.iter().map(ToString::to_string).collect())
}

/// How a setup ended, for the page (the `setup` event).
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SetupEnded {
    Ready {
        server: String,
    },
    /// No account there yet: the player picks a name, and it runs again with it.
    NeedsName {
        server: String,
        suggested: String,
        taken: bool,
    },
    Failed {
        message: String,
    },
}

/// Connects the game to `server` (as typed or picked): installs the client, signs in or makes
/// the account (`new_name`, from the name step), writes the settings. Runs on; the `setup`
/// event says how it ended, the activity bar how it goes.
#[tauri::command(async)]
pub fn connect(app: tauri::AppHandle, state: tauri::State<Launcher>, server: String, new_name: Option<String>) -> bool {
    let inner = state.lock();
    if inner.setup_running {
        return false;
    }
    let Some(game) = inner.game.as_ref() else { return false };
    let new_name = new_name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let asked = connect::ask(game, &server, new_name, false, inner.directory.as_deref());
    drop(inner);
    start_setup(&app, asked);
    true
}

fn start_setup(app: &tauri::AppHandle, asked: connect::Asked) {
    let state = app.state::<Launcher>();
    let mut inner = state.lock();
    if let Some(id) = inner.setup_failed.take() {
        inner.notices.dismiss(id);
    }
    let log = inner.notices.start_with(notifier(app), asked.title.clone());
    inner.setup_failed = Some(log.id());
    inner.setup = Some(asked.clone());
    inner.setup_running = true;
    drop(inner);
    changed(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let outcome = launcher::task::guarded(|| connect::run(&asked, &log));
        let state = app.state::<Launcher>();
        let mut inner = state.lock();
        inner.setup_running = false;
        let ended = match outcome {
            Ok((launcher::flow::Done::Ready, server, via_network)) => {
                log.done(asked.done.clone());
                connect::ready(&asked, &server, via_network, inner.directory.as_deref());
                inner.setup_failed = None;
                // The setup may have brought a directory: its servers are read again.
                inner.directory = None;
                if let Some(game) = inner.game.as_mut() {
                    game.reload();
                }
                SetupEnded::Ready { server }
            }
            Ok((launcher::flow::Done::NeedsName { server, suggested, taken }, _, _)) => {
                log.finish_quietly();
                inner.setup_failed = None;
                SetupEnded::NeedsName { server, suggested, taken }
            }
            Err(message) => {
                log.title(asked.failed.clone());
                log.fail(message.clone(), &[Action::Retry, Action::CopyDetails]);
                SetupEnded::Failed { message }
            }
        };
        drop(inner);
        let _ = app.emit("setup", ended);
        changed(&app);
    });
}
