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
use setup::diagnose::Fix;
use setup::diagnose::Status;
use setup::directory::Listing;
use tauri::Emitter;
use tauri::Manager;

const PRODUCT: &str = "5th Echelon Enhanced";
const RELEASE: &str = env!("FE_RELEASE");

/// What the window works on: the game folder, the activity bar's list, the game it started.
pub struct Launcher {
    inner: Mutex<Inner>,
}

pub(crate) struct Inner {
    pub(crate) game: Option<Game>,
    pub(crate) notices: Activities,
    pub(crate) running: Option<Child>,
    /// The network's servers with this PC's ping to each, and what to say about the list.
    pub(crate) directory: Option<Vec<(Listing, Option<u32>)>>,
    pub(crate) directory_note: Option<String>,
    /// What the directory last said about a release going out and the network's maintenance,
    /// when it was read, and the player's server as last listed (it leaves the list while it
    /// restarts).
    pub(crate) rollout: Option<setup::directory::Rollout>,
    pub(crate) network_maintenance: Option<setup::directory::Window>,
    pub(crate) browsed_at: Option<Instant>,
    pub(crate) my_listing: Option<Listing>,
    /// The setup running or last run (to try again), and its activity when it failed.
    pub(crate) setup: Option<connect::Asked>,
    pub(crate) setup_running: bool,
    pub(crate) setup_failed: Option<u64>,
    /// The checklist as last gathered, the game version's support, and when.
    pub(crate) checks: Vec<setup::diagnose::Check>,
    pub(crate) support: Option<launcher::flow::Support>,
    pub(crate) checked: Option<Instant>,
    pub(crate) checking: bool,
    /// A fix running, and its activity with the fix when it failed (to try again).
    pub(crate) fixing: bool,
    pub(crate) fix_failed: Option<(u64, Fix)>,
    /// Friends as last fetched (with the checklist), and whether a fetch is running.
    pub(crate) friends: launcher::flow::Friends,
    pub(crate) fetching_friends: bool,
    /// The server's news and what's played there, with the server and when they were asked.
    pub(crate) news: Option<(String, Instant, Vec<setup::server_info::NewsItem>)>,
    pub(crate) playing: Option<(String, Instant, Option<server_api::playing::Playing>)>,
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
                rollout: None,
                network_maintenance: None,
                browsed_at: None,
                my_listing: None,
                setup: None,
                setup_running: false,
                setup_failed: None,
                checks: Vec::new(),
                support: None,
                checked: None,
                checking: false,
                fixing: false,
                fix_failed: None,
                friends: Default::default(),
                fetching_friends: false,
                news: None,
                playing: None,
            }),
        }
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Tells the page the activity bar changed (it asks for what to show, `activity`).
pub(crate) fn changed(app: &tauri::AppHandle) {
    let _ = app.emit("activity", ());
}

/// How an activity reports back while it runs, for `Activities::start_with`.
pub(crate) fn notifier(app: &tauri::AppHandle) -> Option<Arc<dyn Fn() + Send + Sync>> {
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
    /// This PC's identity in short ("K7QF-2M9D"), if it has one.
    identity: Option<String>,
    /// How many beta features are on (Settings › Game).
    beta: usize,
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
        identity: identity_short(),
        beta: game.map_or(0, |g| g.cfg.hook_config.beta.on().len()),
    }
}

/// Whether the game is running: the one started from here, or any (through Steam, as
/// administrator, or by hand). The one started here stays until the watcher takes its exit
/// (feedback.rs).
pub(crate) fn game_running(inner: &mut Inner) -> bool {
    if inner.running.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(None))) {
        return true;
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
    let Inner {
        game, notices, running, friends, ..
    } = &mut *inner;
    let Some(game) = game.as_mut() else {
        notices.error("Choose the game's folder first.");
        changed(&app);
        return false;
    };
    let mode = if game.managed.is_some() { PlayMode::Play } else { mode };
    use launcher::start::Refused;
    use setup::launch::Launched;
    // The server said the name breaks its rules, while it's still the player's name.
    let name_not_allowed = inner_name_not_allowed(friends, game);
    let started = match launcher::start::start(game, notices, mode, name_not_allowed) {
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
    drop(inner);
    if crate::feedback::take_action(&app, id, action) {
        changed(&app);
        return None;
    }
    let mut inner = state.lock();
    if let Some((failed, fix)) = inner.fix_failed.filter(|(failed, _)| *failed == id && action == Action::Retry) {
        let _ = failed;
        inner.fix_failed = None;
        drop(inner);
        start_fix(&app, fix);
        changed(&app);
        return None;
    }
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
    /// The server menu's header ("Community servers · scbl.jdevwebb.net").
    header: String,
    /// The network is the community network.
    community: bool,
    /// The player's server, for the card beside Play.
    card: Option<ServerCard>,
}

/// The card beside Play: the player's server.
#[derive(Serialize)]
pub struct ServerCard {
    /// "Server", or who runs it when it isn't the community network ("Server · Self-hosted"),
    /// with a line saying so.
    caption: String,
    caption_why: Option<String>,
    /// Where it is, for a listed server; its address otherwise.
    place: String,
    ping: Option<u32>,
    players: Option<u32>,
}

/// The network's servers, pinged now when `fresh` or not yet.
#[tauri::command(async)]
pub fn servers(state: tauri::State<Launcher>, fresh: bool) -> Servers {
    let network = Prefs::directory();
    let have = state.lock().directory.is_some();
    if fresh || !have {
        browse(&state);
    }
    let inner = state.lock();
    let current = inner.game.as_ref().and_then(|g| g.cfg.current_profile().map(|p| p.server.clone())).unwrap_or_default();
    let clock = setup::clock::Clock::local();
    let listing = inner.directory.as_deref().and_then(|d| d.iter().find(|(s, _)| s.host == current));
    let run_by = connect::run_by(listing.is_some(), inner.directory.is_some());
    Servers {
        rows: inner
            .directory
            .as_deref()
            .map(|d| launcher::server_menu::rows(d, &current, identity::now(), &clock))
            .unwrap_or_default(),
        note: inner.directory_note.clone(),
        header: connect::menu_header(network.as_deref()),
        community: network.as_deref().is_some_and(Prefs::is_community),
        network,
        card: (!current.is_empty()).then(|| ServerCard {
            caption: run_by.as_ref().map_or_else(|| "Server".to_string(), |(who, _)| format!("Server · {who}")),
            caption_why: run_by.map(|(_, why)| why),
            place: listing.map_or_else(|| current.clone(), |(s, _)| connect::place(s)),
            ping: listing.and_then(|(_, ping)| *ping),
            players: listing.map(|(s, _)| s.players_online),
        }),
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
    // The name step after a friend's server: still one the player didn't type.
    let public_only = inner
        .setup
        .as_ref()
        .is_some_and(|a| a.plan.public_only && a.plan.server.eq_ignore_ascii_case(server.trim()));
    let asked = connect::ask(game, &server, new_name, public_only, inner.directory.as_deref());
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
                inner.checked = None;
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
        let _ = app.emit("checks", ());
        changed(&app);
    });
}

/// How often the checklist is gathered again on its own.
const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

fn fix_id(fix: Fix) -> &'static str {
    match fix {
        Fix::FindGame => "find-game",
        Fix::InstallClient => "install-client",
        Fix::ChooseServer => "choose-server",
        Fix::SetUpAccount => "set-up-account",
        Fix::AutoAdapter => "auto-adapter",
        Fix::CreateSave => "create-save",
        Fix::RaiseSave => "raise-save",
        Fix::UpdateLauncher => "update-launcher",
    }
}

fn fix_from(id: &str) -> Option<Fix> {
    [
        Fix::FindGame,
        Fix::InstallClient,
        Fix::ChooseServer,
        Fix::SetUpAccount,
        Fix::AutoAdapter,
        Fix::CreateSave,
        Fix::RaiseSave,
        Fix::UpdateLauncher,
    ]
    .into_iter()
    .find(|f| fix_id(*f) == id)
}

fn status_id(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Warn => "warn",
        Status::Fail => "fail",
    }
}

#[derive(Serialize)]
pub struct CheckView {
    id: &'static str,
    status: &'static str,
    title: String,
    detail: String,
    /// The fix's id and button, when the launcher can fix it (not where another tool manages
    /// the game).
    fix: Option<(&'static str, &'static str)>,
}

/// The Status tab's checklist and the line beside Play.
#[derive(Serialize)]
pub struct Checklist {
    checks: Vec<CheckView>,
    /// The game version's line, when it isn't supported ("fail", with Try to identify) or
    /// couldn't be told ("warn").
    support: Option<(&'static str, &'static str, String)>,
    /// How ready: "ok", "warn", "fail", or None while checking; and the line.
    tone: Option<&'static str>,
    line: String,
    /// "Fix everything" has something to do.
    fixable: bool,
    /// Gathering or fixing now.
    busy: bool,
}

/// The checklist: gathered again when `fresh` (Check again: signing in again too) or stale.
/// `all`: every check, not only what matters.
#[tauri::command(async)]
pub fn checks(state: tauri::State<Launcher>, fresh: bool, all: bool) -> Checklist {
    let mut inner = state.lock();
    let stale = inner.checked.is_none_or(|t| t.elapsed() > CHECK_EVERY);
    let running = game_running(&mut inner);
    let busy = inner.setup_running || inner.fixing;
    if (fresh || stale) && !inner.checking && !busy {
        if let Some(game) = inner.game.as_mut() {
            game.reload();
            let (dir, cfg) = (game.dir.clone(), (*game.cfg).clone());
            inner.checking = true;
            drop(inner);
            if fresh {
                launcher::flow::forget_account_check();
            }
            let (facts, support) = launcher::task::guarded(|| launcher::flow::gather(&dir, &cfg, launcher::dll_utils::bundled()));
            inner = state.lock();
            inner.checks = setup::diagnose::checklist(&facts);
            inner.support = Some(support);
            inner.checked = Some(Instant::now());
            inner.checking = false;
        }
    }
    let managed = inner.game.as_ref().is_some_and(|g| g.managed.is_some());
    let has_server = inner.game.as_ref().and_then(|g| g.cfg.current_profile()).is_some_and(|p| !p.server.is_empty());
    let (tone, line) = launcher::checks::readiness(&inner.checks, inner.support.as_ref(), running);
    Checklist {
        checks: launcher::checks::shown(&inner.checks, all)
            .into_iter()
            .map(|c| CheckView {
                id: c.id,
                status: status_id(c.status),
                title: c.title.clone(),
                detail: if c.status == Status::Ok { String::new() } else { c.detail.clone() },
                fix: c.fix.filter(|_| c.status != Status::Ok && !managed).map(|f| (fix_id(f), launcher::checks::fix_label(f))),
            })
            .collect(),
        support: launcher::checks::support_line(inner.support.as_ref()).map(|(s, title, why)| (status_id(s), title, why)),
        tone: tone.map(status_id),
        line,
        fixable: launcher::checks::fixable(&inner.checks) && has_server && !managed && !busy,
        busy: inner.checking || busy,
    }
}

/// A check's fix. The ones the launcher makes run on (the activity bar says how it went, the
/// `checks` event when it's done); "choose-server" and "set-up-account" answer false: the page
/// opens the setup's server step.
#[tauri::command(async)]
pub fn fix(app: tauri::AppHandle, fix: String) -> bool {
    let Some(fix) = fix_from(&fix) else { return false };
    if launcher::checks::fix_title(fix).is_none() {
        return false;
    }
    start_fix(&app, fix);
    true
}

fn start_fix(app: &tauri::AppHandle, fix: Fix) {
    let state = app.state::<Launcher>();
    let mut inner = state.lock();
    let (Some(game), Some(title)) = (inner.game.as_ref(), launcher::checks::fix_title(fix)) else {
        return;
    };
    if inner.fixing || inner.setup_running {
        return;
    }
    let dir = game.dir.clone();
    let log = inner.notices.start_with(notifier(app), title);
    inner.fixing = true;
    drop(inner);
    changed(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let result = launcher::task::guarded(|| launcher::checks::run_fix(fix, &dir).unwrap_or_else(|| Ok(String::new())));
        fixed(&app, log, result, Some(fix));
    });
}

/// A fix (or identifying the game) ended: said in its activity, and the checklist gathered again.
fn fixed(app: &tauri::AppHandle, log: launcher::activity::Handle, result: Result<String, String>, fix: Option<Fix>) {
    let state = app.state::<Launcher>();
    let mut inner = state.lock();
    inner.fixing = false;
    inner.checked = None;
    match result {
        Ok(done) => log.done(done),
        Err(e) => {
            log.fail(e, if fix.is_some() { &[Action::Retry, Action::CopyDetails] } else { &[Action::CopyDetails] });
            inner.fix_failed = fix.map(|f| (log.id(), f));
        }
    }
    drop(inner);
    let _ = app.emit("checks", ());
    changed(app);
}

/// Looks for the hooks' addresses in an unsupported game version.
#[tauri::command(async)]
pub fn identify(app: tauri::AppHandle, state: tauri::State<Launcher>) {
    let mut inner = state.lock();
    let Some(game) = inner.game.as_ref() else { return };
    if inner.fixing {
        return;
    }
    let (dir, version) = (game.dir.clone(), launcher::checks::game_version(game));
    let log = inner.notices.start_with(notifier(&app), "Identifying the game's version");
    inner.fixing = true;
    drop(inner);
    changed(&app);
    std::thread::spawn(move || {
        let result = launcher::task::guarded(|| launcher::checks::identify(&dir, version));
        fixed(&app, log, result, None);
    });
}

/// Fix everything: the setup again on the player's server, with their name.
#[tauri::command(async)]
pub fn fix_everything(app: tauri::AppHandle, state: tauri::State<Launcher>) -> bool {
    let inner = state.lock();
    if inner.setup_running {
        return false;
    }
    let Some(game) = inner.game.as_ref() else { return false };
    let profile = game.cfg.current_profile().cloned().unwrap_or_default();
    let name = Some(profile.user.username).filter(|n| !n.is_empty());
    let asked = connect::ask(game, &profile.server, name, false, inner.directory.as_deref());
    drop(inner);
    start_setup(&app, asked);
    true
}

/// Switches to a server from the menu (the network's): set up at once with the player's name,
/// in the activity bar. Why not, when it can't now.
#[tauri::command(async)]
pub fn switch_server(app: tauri::AppHandle, state: tauri::State<Launcher>, host: String) -> Result<(), String> {
    let mut inner = state.lock();
    if game_running(&mut inner) {
        return Err("Quit the game first".into());
    }
    if inner.setup_running || inner.fixing {
        return Err("Wait for the current task to finish".into());
    }
    let Some(game) = inner.game.as_ref() else {
        return Err("Choose the game's folder first.".into());
    };
    if game.managed.is_some() {
        return Err("Another tool manages this install: change the server there".into());
    }
    let name = game.cfg.current_profile().map(|p| p.user.username.clone()).filter(|n| !n.is_empty());
    let asked = connect::ask(game, &host, name, true, inner.directory.as_deref());
    drop(inner);
    start_setup(&app, asked);
    Ok(())
}

/// The player's name, when the server said it breaks the rules: only while it's still their
/// name (a rename takes effect at once).
fn inner_name_not_allowed(friends: &launcher::flow::Friends, game: &Game) -> bool {
    friends
        .name_not_allowed
        .as_ref()
        .is_some_and(|name| game.cfg.current_profile().is_some_and(|p| &p.user.username == name))
}

#[derive(Serialize)]
pub struct FriendView {
    id: String,
    name: String,
    activity: Option<String>,
    joinable: bool,
}

#[derive(Serialize)]
pub struct OtherView {
    id: String,
    name: String,
    activity: Option<String>,
    asked: bool,
    asked_us: bool,
}

#[derive(Serialize)]
pub struct ElsewhereView {
    name: String,
    /// Where they are (the server's region, else its name), and its address.
    place: String,
    host: String,
    /// On the internet: one server can't send players into their own network.
    public: bool,
}

/// The Squad tab: friends here and on other servers, others online, what's played here.
#[derive(Serialize)]
pub struct Squad {
    online: Vec<FriendView>,
    others: Vec<OtherView>,
    elsewhere: Vec<ElsewhereView>,
    offline: usize,
    requests: usize,
    /// The server joins a friend's game without an invitation: the Join buttons show.
    join_offered: bool,
    /// The key that opens the game's overlay, for the hints.
    overlay_key: String,
    /// The player's name breaks the network's rules.
    name_not_allowed: Option<String>,
    managed: bool,
    /// What's being played on the player's server: its public matches, and the rest in a line.
    playing: Option<Vec<String>>,
    playing_more: Option<String>,
}

/// How often what's played on the player's server is asked for.
const PLAYING_EVERY: std::time::Duration = std::time::Duration::from_secs(15);
/// The most of the server's matches the Squad tab lists.
const PLAYING_SHOWN: usize = 4;

/// The Squad tab: friends fetched again when `fresh` (after acting on someone), else every
/// half minute; what's played every 15 seconds.
#[tauri::command(async)]
pub fn squad(state: tauri::State<Launcher>, fresh: bool) -> Squad {
    let mut inner = state.lock();
    let Some(game) = inner.game.as_ref() else { return empty_squad() };
    let managed = game.managed.is_some();
    let cfg = (*game.cfg).clone();
    let server = game.cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty());
    let overlay_key = game.cfg.hook_config.overlay_key.to_string();
    let stale = fresh || inner.checked.is_none_or(|t| t.elapsed() > CHECK_EVERY) || inner.friends.online.is_empty() && inner.friends.offline == 0;
    if !managed && stale && !inner.fetching_friends {
        inner.fetching_friends = true;
        drop(inner);
        let friends = launcher::task::guarded(|| launcher::flow::friends(&cfg));
        inner = state.lock();
        inner.fetching_friends = false;
        // A failed fetch keeps the last list: it's a hint, not a check.
        if let Ok(friends) = friends {
            inner.friends = friends;
        }
    }
    if let Some(server) = server.clone() {
        let fresh = inner.playing.as_ref().is_some_and(|(from, at, _)| *from == server && at.elapsed() < PLAYING_EVERY);
        if !fresh {
            drop(inner);
            let playing = setup::server_info::playing(&server, std::time::Duration::from_secs(5));
            inner = state.lock();
            inner.playing = Some((server, Instant::now(), playing));
        }
    }
    let f = &inner.friends;
    let now = inner.playing.as_ref().filter(|(from, ..)| Some(from) == server.as_ref()).and_then(|(_, _, p)| p.clone());
    let (playing, playing_more) = match now {
        None => (None, None),
        Some(now) => {
            let lines = now.matches.iter().take(PLAYING_SHOWN).map(server_api::playing::Match::line).collect::<Vec<_>>();
            let rest = server_api::playing::Playing {
                matches: now.matches.iter().skip(PLAYING_SHOWN).cloned().collect(),
                ..now.clone()
            };
            let more = rest.summary();
            let lead = if now.matches.is_empty() { "" } else { "Also: " };
            (Some(lines), (!more.is_empty()).then(|| format!("{lead}{more}")))
        }
    };
    Squad {
        online: f
            .online
            .iter()
            .map(|o| FriendView {
                id: o.id.clone(),
                name: o.name.clone(),
                activity: o.activity.clone(),
                joinable: o.joinable,
            })
            .collect(),
        others: f
            .others
            .iter()
            .map(|o| OtherView {
                id: o.id.clone(),
                name: o.name.clone(),
                activity: o.activity.clone(),
                asked: o.asked,
                asked_us: o.asked_us,
            })
            .collect(),
        elsewhere: f
            .elsewhere
            .iter()
            .map(|e| ElsewhereView {
                name: e.username.clone(),
                place: if e.region.is_empty() { e.server.clone() } else { e.region.clone() },
                host: e.host.clone(),
                public: setup::directory::listable_host(&e.host),
            })
            .collect(),
        offline: f.offline,
        requests: f.requests,
        join_offered: f.join_offered,
        overlay_key,
        name_not_allowed: f.name_not_allowed.clone(),
        managed,
        playing,
        playing_more,
    }
}

fn empty_squad() -> Squad {
    Squad {
        online: Vec::new(),
        others: Vec::new(),
        elsewhere: Vec::new(),
        offline: 0,
        requests: 0,
        join_offered: false,
        overlay_key: String::new(),
        name_not_allowed: None,
        managed: false,
        playing: None,
        playing_more: None,
    }
}

/// Joins a friend's game (`JoinNow`): the game is started first if it isn't running (not
/// when the launcher holds back, for a question first). The activity bar says how it went.
#[tauri::command(async)]
pub fn join_friend(app: tauri::AppHandle, state: tauri::State<Launcher>, id: String, name: String) {
    let mut inner = state.lock();
    let running = game_running(&mut inner);
    drop(inner);
    if !running && !play(app.clone(), state.clone(), PlayMode::Play) {
        return;
    }
    let inner = state.lock();
    let Some(game) = inner.game.as_ref() else { return };
    let (cfg, dir) = (game.cfg.clone(), game.dir.clone());
    drop(inner);
    let result = launcher::task::guarded(|| launcher::flow::join_friend(&cfg, &dir, &id, &name));
    let mut inner = state.lock();
    match result {
        Ok(line) => inner.notices.info(line),
        Err(line) => inner.notices.error(line),
    }
    drop(inner);
    changed(&app);
}

/// Asks someone online to be friends ("request"), invites them to the game ("invite"), or
/// blocks them ("block"). The activity bar says how it went.
#[tauri::command(async)]
pub fn player_action(app: tauri::AppHandle, state: tauri::State<Launcher>, action: String, id: String, name: String) {
    use launcher::network::PlayerAction;
    let action = match action.as_str() {
        "request" => PlayerAction::Request,
        "invite" => PlayerAction::Invite,
        "block" => PlayerAction::Block,
        _ => return,
    };
    let inner = state.lock();
    let Some(game) = inner.game.as_ref() else { return };
    let cfg = (*game.cfg).clone();
    drop(inner);
    let result = launcher::task::guarded(|| launcher::flow::player_action(&cfg, action, &id, "", &name));
    let mut inner = state.lock();
    match result {
        Ok(line) => inner.notices.info(line),
        Err(line) => inner.notices.error(line),
    }
    // Asked again, so "Add friend" turns into "Asked".
    inner.checked = None;
    drop(inner);
    changed(&app);
}

#[derive(Serialize)]
pub struct NewsView {
    title: String,
    text: String,
    link: String,
}

/// How often the server's news is asked for.
const NEWS_EVERY: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The player's server's news, newest first: the server's words, as text only.
#[tauri::command(async)]
pub fn news(state: tauri::State<Launcher>) -> Vec<NewsView> {
    let inner = state.lock();
    let Some(server) = inner
        .game
        .as_ref()
        .and_then(|g| g.cfg.current_profile())
        .map(|p| p.server.clone())
        .filter(|s| !s.is_empty())
    else {
        return Vec::new();
    };
    let fresh = inner.news.as_ref().is_some_and(|(from, at, _)| *from == server && at.elapsed() < NEWS_EVERY);
    drop(inner);
    if !fresh {
        let news = setup::server_info::news(&server, std::time::Duration::from_secs(5));
        state.lock().news = Some((server.clone(), Instant::now(), news));
    }
    let inner = state.lock();
    inner
        .news
        .as_ref()
        .filter(|(from, ..)| *from == server)
        .map(|(_, _, items)| {
            items
                .iter()
                .map(|n| NewsView {
                    title: hooks_config::text::clip(&n.title, 80),
                    text: hooks_config::text::clip(&n.text, 500),
                    link: n.link.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Switches to a friend's server (confirmed on the page): set up there with the player's
/// identity, the name step following if they have no account there.
#[tauri::command(async)]
pub fn switch_to_friend(app: tauri::AppHandle, state: tauri::State<Launcher>, host: String) -> bool {
    let mut inner = state.lock();
    if game_running(&mut inner) || inner.setup_running {
        return false;
    }
    let Some(game) = inner.game.as_ref() else { return false };
    if !setup::directory::listable_host(&host) {
        return false;
    }
    let asked = connect::ask(game, &host, None, true, inner.directory.as_deref());
    drop(inner);
    start_setup(&app, asked);
    true
}

/// Reads the network's directory and pings its servers.
fn browse(state: &Launcher) {
    let Some(url) = Prefs::directory() else { return };
    let browsed = launcher::services::rt().block_on(launcher::network::server_directory(&url));
    let mut inner = state.lock();
    inner.browsed_at = Some(Instant::now());
    match browsed {
        Ok(b) => {
            inner.directory = Some(b.servers);
            inner.directory_note = b.note;
            inner.rollout = b.rollout;
            inner.network_maintenance = b.network_maintenance;
        }
        Err(e) => inner.directory_note = Some(format!("The server directory couldn't be read: {e}")),
    }
}

/// One server in the notice.
#[derive(Serialize)]
pub struct NoticeRow {
    place: String,
    mine: bool,
    version: String,
    doing: String,
    meter: Option<f32>,
    /// "waiting", "now" or "done".
    tone: &'static str,
}

/// The notice under the launch bar: a release going out, or maintenance on the player's
/// server or the network.
#[derive(Serialize)]
pub struct NoticeView {
    heading: String,
    eta: String,
    text: String,
    note: Option<String>,
    /// Its button: playing on another server meanwhile (`host`), with its label.
    play_on: Option<(String, String)>,
    servers: Vec<NoticeRow>,
    /// What the launch bar says instead of how ready the player is, and whether Play waits.
    status: String,
    blocks_play: Option<String>,
    /// When to ask again, in seconds.
    every: u64,
}

/// The notice for the player, if there's one; the directory is read again as often as the
/// notice says (every minute, or 15 s while their server restarts or is down).
#[tauri::command(async)]
pub fn notice(state: tauri::State<Launcher>) -> Option<NoticeView> {
    let make = |inner: &mut Inner| {
        let game = inner.game.as_ref().filter(|g| g.managed.is_none())?;
        let current = game.cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty())?;
        let servers = inner.directory.clone().unwrap_or_default();
        // A newer launcher: this one updates itself from step 3 of its move to Tauri.
        launcher::update_notice::for_player(&current, &servers, inner.rollout.as_ref(), inner.network_maintenance.as_ref(), &mut inner.my_listing, None)
    };
    let mut inner = state.lock();
    let first = make(&mut inner);
    let every = first.as_ref().map_or(std::time::Duration::from_secs(60), launcher::update_notice::Notice::refresh_every);
    let due = inner.browsed_at.is_none_or(|t| t.elapsed() >= every);
    let watching = inner.rollout.is_some() || first.is_some();
    let busy = inner.setup_running;
    drop(inner);
    let notice = if watching && due && !busy {
        browse(&state);
        make(&mut state.lock())
    } else {
        first
    };
    use launcher::update_notice::Action as A;
    use launcher::update_notice::Tone;
    notice.map(|n| NoticeView {
        every: n.refresh_every().as_secs(),
        heading: n.heading,
        eta: n.eta,
        text: n.text,
        note: n.note,
        play_on: n.action.and_then(|(action, label)| match action {
            A::PlayOn { host, .. } => Some((host, label)),
            A::UpdateLauncher => None,
        }),
        servers: n
            .servers
            .into_iter()
            .map(|r| NoticeRow {
                place: r.place,
                mine: r.mine,
                version: r.version,
                doing: r.doing,
                meter: r.meter,
                tone: match r.tone {
                    Tone::Waiting => "waiting",
                    Tone::Now => "now",
                    Tone::Done => "done",
                },
            })
            .collect(),
        status: n.status,
        blocks_play: n.blocks_play,
    })
}

/// Opens a link in the player's browser: only `https://` ones (the page's links are the
/// launcher's own, and the servers' news shown as text).
#[tauri::command(async)]
pub fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let parsed = url::Url::parse(&url).map_err(|_| "Not a link the launcher opens.".to_string())?;
    if parsed.scheme() != "https" {
        return Err("Not a link the launcher opens.".into());
    }
    app.opener().open_url(parsed.as_str(), None::<&str>).map_err(|e| e.to_string())
}

/// This PC's identity in short, read once (it's decrypted from disk).
fn identity_short() -> Option<String> {
    static SHORT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    SHORT
        .get_or_init(|| setup::player_identity::load().ok().flatten().map(|id| identity::short(&id.global_id())))
        .clone()
}

/// One of the game's loading screens for the stage, a different one each start, as a JPEG
/// data URL; None for a game without them (the night-vision backdrop stays).
#[tauri::command(async)]
pub fn key_art(state: tauri::State<Launcher>) -> Option<String> {
    static ART: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let dir = state.lock().game.as_ref()?.dir.clone();
    ART.get_or_init(|| {
        let art = setup::key_art::load(&dir, rand::random::<u32>() as usize)
            .map_err(|e| tracing::info!("No banner art from the game: {e}"))
            .ok()?;
        let rgba = image::RgbaImage::from_raw(art.width as u32, art.height as u32, art.rgba)?;
        let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85).encode_image(&rgb).ok()?;
        use base64::Engine;
        Some(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpeg)))
    })
    .clone()
}

/// The account dialog's server: where and what it is, how far, and what the player leaves.
#[derive(Serialize)]
pub struct AccountServer {
    /// Where it is ("North America"), and what it is ("5th Echelon Community NA · community
    /// server", "A server of your own", "A friend's server").
    place: String,
    kind: String,
    ping: Option<u32>,
    /// A server of the player's own (one they typed): the dialog confirms it.
    own: bool,
    /// The server the player is on now, where they stay if they don't.
    current: Option<String>,
    /// The network a server of their own leaves ("the community network").
    leaving: String,
}

/// What the account dialog says of `server`.
#[tauri::command(async)]
pub fn account_server(state: tauri::State<Launcher>, server: String, own: Option<bool>) -> AccountServer {
    let inner = state.lock();
    // A server of the player's own, from the Servers screen; else as the setup that asked was.
    let own = own.unwrap_or_else(|| !inner.setup.as_ref().is_some_and(|a| a.plan.public_only));
    let directory = inner.directory.as_deref().unwrap_or_default();
    let listing = directory.iter().find(|(s, _)| s.host.eq_ignore_ascii_case(&server));
    let (place, kind) = match listing {
        Some((s, _)) => (connect::place(s), format!("{} · community server", s.name)),
        None if own => (server.clone(), "A server of your own".to_string()),
        None => (server.clone(), "A friend's server".to_string()),
    };
    let current = inner
        .game
        .as_ref()
        .and_then(|g| g.cfg.current_profile())
        .map(|p| p.server.clone())
        .filter(|s| !s.is_empty())
        .map(|host| directory.iter().find(|(s, _)| s.host == host).map_or(host, |(s, _)| connect::place(s)));
    let leaving = match Prefs::directory() {
        Some(url) if Prefs::is_community(&url) => "the community network".to_string(),
        Some(url) => connect::url_host(&url).to_string(),
        None => "your network".to_string(),
    };
    AccountServer {
        place,
        kind,
        ping: listing.and_then(|(_, ping)| *ping),
        own,
        current,
        leaving,
    }
}

/// What the server says of itself for the account dialog: its API (to check names, when it
/// can), its release and its ping.
#[derive(Serialize)]
pub struct ProbeView {
    api: Option<String>,
    version: String,
    ping: Option<u32>,
}

#[tauri::command(async)]
pub fn account_probe(state: tauri::State<Launcher>, server: String) -> Result<ProbeView, String> {
    let dir = state.lock().game.as_ref().map(|g| g.dir.clone()).ok_or("Choose the game's folder first.")?;
    let probe = launcher::task::guarded(|| launcher::flow::probe(&dir, &server))?;
    Ok(ProbeView {
        api: probe.name_check.then_some(probe.api),
        version: probe.version,
        ping: probe.ping,
    })
}

/// Whether `name` is free on the server at `api`: "free", "taken", or why it isn't allowed.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NameView {
    Free,
    Taken,
    NotAllowed { why: String },
}

#[tauri::command(async)]
pub fn check_name(api: String, name: String) -> Result<NameView, String> {
    use launcher::flow::NameCheck;
    Ok(match launcher::flow::check_name(&api, name.trim())? {
        NameCheck::Free => NameView::Free,
        NameCheck::Taken(_) => NameView::Taken,
        NameCheck::NotAllowed(why) => NameView::NotAllowed { why },
    })
}

/// Up to three free names like `name` on the server at `api`.
#[tauri::command(async)]
pub fn name_suggestions(api: String, name: String) -> Vec<String> {
    launcher::task::guarded(|| launcher::flow::free_suggestions(&api, name.trim()))
}

/// A server of the player's own they connected to, for the Servers screen's Recent.
#[derive(Serialize)]
pub struct RecentView {
    address: String,
    label: String,
    /// "used 2 days ago".
    when: String,
}

#[tauri::command(async)]
pub fn recent_servers() -> Vec<RecentView> {
    let now = identity::now();
    Prefs::recent_servers()
        .into_iter()
        .map(|r| RecentView {
            label: hooks_config::text::clip(r.name.as_deref().unwrap_or(&r.address), 40),
            when: format!("used {}", setup::directory::ago(now - r.last_used).replace("within the last hour", "just now")),
            address: r.address,
        })
        .collect()
}

/// The name a new account on a server of the player's own starts with: the one they use, or
/// their Windows user name.
#[tauri::command(async)]
pub fn usual_name(state: tauri::State<Launcher>) -> String {
    let dir = state.lock().game.as_ref().map(|g| g.dir.clone());
    dir.and_then(|d| launcher::flow::usual_name(&d)).unwrap_or_else(launcher::flow::windows_user)
}

/// Uses another network (its address as typed), when it answers as one: its servers fill the
/// server menu.
#[tauri::command(async)]
pub fn use_network(app: tauri::AppHandle, state: tauri::State<Launcher>, typed: String) -> Result<(), String> {
    let (url, count) = launcher::services::rt().block_on(launcher::network::check_network(&typed))?;
    Prefs::set_directory(Some(url.clone()));
    let mut inner = state.lock();
    inner.directory = None;
    inner.directory_note = None;
    let s = if count == 1 { "" } else { "s" };
    inner.notices.info(format!(
        "Using the network {}: its {count} server{s} are in the server menu on the Play screen.",
        connect::url_host(&url)
    ));
    drop(inner);
    changed(&app);
    Ok(())
}
