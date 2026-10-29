//! What the page asks the Rust side for. The work itself is the egui launcher's (its library,
//! `launcher`), so the two set up and start the game the same way and keep the same files.

use std::process::Child;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use launcher::activity::Activities;
use launcher::activity::Progress;
use launcher::activity::State;
use launcher::prefs::Game;
use launcher::prefs::PlayMode;
use launcher::prefs::Prefs;
use serde::Serialize;
use tauri::Emitter;

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
}

impl Launcher {
    /// The game chosen before, or the first one found.
    pub fn new() -> Self {
        let mut notices = Activities::default();
        let game = Game::chosen_or_found().map(|dir| Game::open_tidied(dir, &mut notices));
        Self {
            inner: Mutex::new(Inner { game, notices, running: None }),
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
#[allow(dead_code)]
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
    let Inner { game, notices, running } = &mut *inner;
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
    waiting: usize,
}

#[tauri::command(async)]
pub fn activity(state: tauri::State<Launcher>) -> Option<ActivityView> {
    let (a, waiting) = state.lock().notices.current(Instant::now())?;
    let (state, message) = match &a.state {
        State::Working => ("working", None),
        State::Done => ("done", None),
        State::Failed { message, .. } => ("failed", Some(message.clone()).filter(|m| !m.is_empty())),
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
        waiting,
    })
}

#[tauri::command(async)]
pub fn dismiss(app: tauri::AppHandle, state: tauri::State<Launcher>, id: u64) {
    state.lock().notices.dismiss(id);
    changed(&app);
}
