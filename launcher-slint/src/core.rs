//! What the screens ask for and do, without the window: the Tauri launcher's command layer
//! (its commands.rs, before launcher-tauri was removed) for Play and Matches, carried over with Tauri taken
//! out. The work itself is the egui launcher's library (`launcher`), so all the launchers set up
//! and start the game the same way and keep the same files.
//!
//! Every function here may block (the network, the game's files): the window calls them from
//! worker threads (ui.rs), never from its own.

use std::process::Child;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use launcher::activity::Action;
use launcher::activity::Activities;
use launcher::activity::Progress;
use launcher::activity::State;
use launcher::connect;
use launcher::prefs::Game;
use launcher::prefs::PlayMode;
use launcher::prefs::Prefs;
use setup::diagnose::Fix;
use setup::diagnose::Status;
use setup::directory::Listing;

/// How often the checklist is gathered again on its own.
pub const CHECK_EVERY: Duration = Duration::from_secs(30);
/// How often what's played on the player's server is asked for.
const PLAYING_EVERY: Duration = Duration::from_secs(15);
/// The most of the server's matches the Squad tab lists.
const PLAYING_SHOWN: usize = 4;
/// How often the server's news is asked for.
const NEWS_EVERY: Duration = Duration::from_secs(15 * 60);

/// Tells the window something changed (it asks again for what it shows).
pub type Notify = Arc<dyn Fn(Changed) + Send + Sync>;

/// What changed, for the window to ask for again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Changed {
    /// The activity bar.
    Activity,
    /// The checklist, the servers, the squad and the news (a setup or a fix ended).
    Checks,
    /// A setup that needs a name the player hasn't chosen: the account dialog (the server, the
    /// name to start with, and whether that one's taken).
    NeedsName,
}

/// What the window works on: the game folder, the activity bar's list, the game it started.
pub struct Core {
    inner: Mutex<Inner>,
    notify: Notify,
}

pub struct Inner {
    pub game: Option<Game>,
    pub notices: Activities,
    pub running: Option<Child>,
    /// The network's servers with this PC's ping to each, and what to say about the list.
    pub directory: Option<Vec<(Listing, Option<u32>)>>,
    pub directory_note: Option<String>,
    /// What the directory last said about a release going out and the network's maintenance,
    /// when it was read, and the player's server as last listed (it leaves the list while it
    /// restarts).
    pub rollout: Option<setup::directory::Rollout>,
    pub network_maintenance: Option<setup::directory::Window>,
    pub browsed_at: Option<Instant>,
    pub my_listing: Option<Listing>,
    /// The setup running or last run (to try again), and its activity when it failed.
    pub setup: Option<connect::Asked>,
    pub setup_running: bool,
    pub setup_failed: Option<u64>,
    /// The checklist as last gathered, the game version's support, and when.
    pub checks: Vec<setup::diagnose::Check>,
    pub support: Option<launcher::flow::Support>,
    pub checked: Option<Instant>,
    pub checking: bool,
    /// A fix running, and its activity with the fix when it failed (to try again).
    pub fixing: bool,
    pub fix_failed: Option<(u64, Fix)>,
    /// Friends as last fetched (with the checklist), and whether a fetch is running.
    pub friends: launcher::flow::Friends,
    pub fetching_friends: bool,
    /// The server's news and what's played there, with the server and when they were asked.
    pub news: Option<(String, Instant, Vec<setup::server_info::NewsItem>)>,
    pub playing: Option<(String, Instant, Option<server_api::playing::Playing>)>,
    /// Until when the player's "new match" holds: the Play button says so.
    pub new_match_until: Option<Instant>,
    /// A setup stopped for a name (the account dialog): the server, the name to start with,
    /// and whether it's taken there.
    pub needs_name: Option<(String, String, bool)>,
    /// How the last setup ended, for the guided setup: Ok(server), or why it failed.
    pub setup_ended: Option<Result<String, String>>,
}

impl Core {
    /// The game chosen before, or the first one found.
    pub fn new(notify: Notify) -> Self {
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
                new_match_until: None,
                needs_name: None,
                setup_ended: None,
            }),
            notify,
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn changed(&self, what: Changed) {
        (self.notify)(what);
    }

    /// How an activity reports back while it runs, for `Activities::start_with`.
    pub fn notifier(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let notify = self.notify.clone();
        Some(Arc::new(move || notify(Changed::Activity)))
    }

    /// The player's server, if they have one.
    fn server(&self) -> Option<String> {
        self.lock()
            .game
            .as_ref()
            .and_then(|g| g.cfg.current_profile())
            .map(|p| p.server.clone())
            .filter(|s| !s.is_empty())
    }
}

/// The release, the game folder, and the server and account the game is set up for.
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub game_dir: Option<String>,
    pub server: Option<String>,
    pub account: Option<String>,
    /// The launcher uses the community network (Chat, Support and the Roadmap are its).
    pub community: bool,
    pub play_mode: PlayMode,
    /// Another tool manages this install: the game starts as it is.
    pub managed: bool,
    pub running: bool,
    /// A setup is running.
    pub connecting: bool,
    /// This PC's identity in short ("K7QF-2M9D"), if it has one.
    pub identity: Option<String>,
    /// How many beta features are on.
    pub beta: usize,
    /// The player's name breaks the network's rules (the server said).
    pub name_not_allowed: Option<String>,
    /// The game's Uplay DLL is an emulator, and the player hasn't closed the notice.
    pub emulator_notice: bool,
    /// A new match asked for still holds.
    pub new_match: bool,
    /// Friend requests waiting for the player's answer.
    pub requests: usize,
    /// What's still being prepared before the game can start: Play waits, with this on it.
    pub preparing: Option<&'static str>,
}

pub fn info(core: &Core) -> Info {
    let mut inner = core.lock();
    let running = game_running(&mut inner);
    if let Some(game) = inner.game.as_mut() {
        game.reload();
    }
    let game = inner.game.as_ref();
    let profile = game.and_then(|g| g.cfg.current_profile().cloned());
    Info {
        game_dir: game.map(|g| g.dir.display().to_string()),
        server: profile.as_ref().map(|p| p.server.clone()).filter(|s| !s.is_empty()),
        account: profile.map(|p| p.user.username).filter(|u| !u.is_empty()),
        community: Prefs::directory().as_deref().is_some_and(Prefs::is_community),
        play_mode: Prefs::play_mode(),
        managed: game.is_some_and(|g| g.managed.is_some()),
        running,
        connecting: inner.setup_running,
        identity: identity_short(),
        beta: game.map_or(0, |g| g.cfg.hook_config.beta.on().len()),
        name_not_allowed: game.filter(|g| name_not_allowed(&inner.friends, g)).and_then(|_| inner.friends.name_not_allowed.clone()),
        emulator_notice: game.is_some_and(|g| g.uplay_emulator && !g.uplay_emulator_told),
        new_match: inner.new_match_until.is_some_and(|until| Instant::now() < until),
        requests: inner.friends.requests,
        preparing: game.filter(|g| launcher::game_files::preparing(&g.dir)).map(|_| launcher::game_files::PREPARING),
    }
}

/// Whether the game is running: the one started from here, or any (through Steam, as
/// administrator, or by hand).
pub fn game_running(inner: &mut Inner) -> bool {
    if inner.running.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(None))) {
        return true;
    }
    setup::game::game_running()
}

/// Starts the game (`mode`: straight into a lobby or not). Whether it's starting; what
/// happened shows in the activity bar.
/// Whether the launcher looks light now (its theme, or the system's for "As the system"): the
/// overlay takes the same look when the game starts.
static LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The window's look, as it is now (from the UI thread when it changes).
pub fn set_light(light: bool) {
    LIGHT.store(light, std::sync::atomic::Ordering::Relaxed);
}

pub fn play(core: &Core, mode: PlayMode) -> bool {
    let mut inner = core.lock();
    let Inner {
        game, notices, running, friends, ..
    } = &mut *inner;
    let Some(game) = game.as_mut() else {
        notices.error("Choose the game's folder first.");
        drop(inner);
        core.changed(Changed::Activity);
        return false;
    };
    let mode = if game.managed.is_some() { PlayMode::Play } else { mode };
    let theme = if LIGHT.load(std::sync::atomic::Ordering::Relaxed) {
        hooks_config::OverlayTheme::Light
    } else {
        hooks_config::OverlayTheme::Dark
    };
    if game.managed.is_none() && game.cfg.hook_config.overlay_theme != theme {
        game.update(notices, |c| c.hook_config.overlay_theme = theme);
    }
    use launcher::start::Refused;
    use setup::launch::Launched;
    let refused_name = name_not_allowed(friends, game);
    let started = match launcher::start::start(game, notices, mode, refused_name) {
        Ok(Launched::Game(child)) => {
            *running = Some(child);
            true
        }
        Ok(Launched::GameProtonWithoutUmu(child)) => {
            *running = Some(child);
            notices.error(setup::diagnose::NO_UMU);
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
    drop(inner);
    core.changed(Changed::Activity);
    started
}

/// The activity bar's activity: the one to show, and how many others wait.
#[derive(Debug, Clone, Default)]
pub struct ActivityView {
    pub id: u64,
    pub title: String,
    pub step: Option<String>,
    /// "working", "done" or "failed".
    pub state: &'static str,
    pub message: Option<String>,
    /// How far along (0 to 1), when it can say.
    pub progress: Option<f32>,
    /// What the player can do about a failure: "retry", "without-logs", "copy-details", with
    /// their labels.
    pub actions: Vec<(&'static str, &'static str)>,
    pub waiting: usize,
}

fn action_id(action: Action) -> &'static str {
    match action {
        Action::Retry => "retry",
        Action::WithoutLogs => "without-logs",
        Action::CopyDetails => "copy-details",
    }
}

pub fn activity(core: &Core) -> Option<ActivityView> {
    let (a, waiting) = core.lock().notices.current(Instant::now())?;
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
        actions,
        waiting,
    })
}

pub fn dismiss(core: &Core, id: u64) {
    core.lock().notices.dismiss(id);
    core.changed(Changed::Activity);
}

/// A button on a failed activity: Copy details answers the text to copy; Try again runs the
/// fix or the setup again.
pub fn activity_action(core: &Arc<Core>, id: u64, action: &str) -> Option<String> {
    let mut inner = core.lock();
    if action == "copy-details" {
        return inner.notices.details(id);
    }
    let action = match action {
        "retry" => Action::Retry,
        "without-logs" => Action::WithoutLogs,
        _ => return None,
    };
    inner.notices.press(id, action);
    if let Some((_, fix)) = inner.fix_failed.filter(|(failed, _)| *failed == id && action == Action::Retry) {
        inner.fix_failed = None;
        drop(inner);
        start_fix(core, fix);
        core.changed(Changed::Activity);
        return None;
    }
    let retry_setup = action == Action::Retry && inner.setup_failed == Some(id) && !inner.setup_running;
    let asked = inner.setup.clone().filter(|_| retry_setup);
    drop(inner);
    if let Some(asked) = asked {
        start_setup(core, asked);
    }
    core.changed(Changed::Activity);
    None
}

/// The network's servers, as the server menu shows them.
#[derive(Debug, Clone, Default)]
pub struct Servers {
    pub rows: Vec<launcher::server_menu::Row>,
    /// Why the list couldn't be read, or that it's the last one.
    pub note: Option<String>,
    /// The network's directory, when the launcher has one.
    pub network: Option<String>,
    /// The server menu's header ("Community servers · scbl.jdevwebb.net").
    pub header: String,
    pub card: Option<ServerCard>,
}

/// The card beside Play: the player's server.
#[derive(Debug, Clone, Default)]
pub struct ServerCard {
    /// "Server", or who runs it when it isn't the community network ("Server · Self-hosted"),
    /// with a line saying so.
    pub caption: String,
    pub caption_why: Option<String>,
    /// Where it is, for a listed server; its address otherwise.
    pub place: String,
    pub ping: Option<u32>,
    pub players: Option<u32>,
}

/// The network's servers, pinged now when `fresh` or not yet.
pub fn servers(core: &Core, fresh: bool) -> Servers {
    let network = Prefs::directory();
    let have = core.lock().directory.is_some();
    if fresh || !have {
        browse(core);
    }
    let inner = core.lock();
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

/// Reads the network's directory and pings its servers.
fn browse(core: &Core) {
    let Some(url) = Prefs::directory() else { return };
    let browsed = launcher::services::rt().block_on(launcher::network::server_directory(&url));
    let mut inner = core.lock();
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

fn start_setup(core: &Arc<Core>, asked: connect::Asked) {
    let mut inner = core.lock();
    if let Some(id) = inner.setup_failed.take() {
        inner.notices.dismiss(id);
    }
    let log = inner.notices.start_with(core.notifier(), asked.title.clone());
    inner.setup_failed = Some(log.id());
    inner.setup = Some(asked.clone());
    inner.setup_running = true;
    drop(inner);
    core.changed(Changed::Activity);
    let core = core.clone();
    std::thread::spawn(move || {
        let outcome = launcher::task::guarded(|| connect::run(&asked, &log));
        let mut inner = core.lock();
        inner.setup_running = false;
        let mut needs_name = false;
        match outcome {
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
                inner.setup_ended = Some(Ok(server));
            }
            Ok((launcher::flow::Done::NeedsName { server, suggested, taken }, _, _)) => {
                log.finish_quietly();
                inner.setup_failed = None;
                inner.needs_name = Some((server, suggested, taken));
                needs_name = true;
            }
            Err(message) => {
                log.title(asked.failed.clone());
                log.fail(message.clone(), &[Action::Retry, Action::CopyDetails]);
                inner.setup_ended = Some(Err(message));
            }
        }
        drop(inner);
        if needs_name {
            core.changed(Changed::NeedsName);
        }
        core.changed(Changed::Checks);
        core.changed(Changed::Activity);
    });
}

pub fn fix_id(fix: Fix) -> &'static str {
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

#[derive(Debug, Clone, Default)]
pub struct CheckView {
    pub id: &'static str,
    pub status: &'static str,
    pub title: String,
    pub detail: String,
    /// The fix's id and button, when the launcher can fix it (not where another tool manages
    /// the game).
    pub fix: Option<(&'static str, &'static str)>,
}

/// The Status tab's checklist and the line beside Play.
#[derive(Debug, Clone, Default)]
pub struct Checklist {
    pub checks: Vec<CheckView>,
    /// The game version's line, when it isn't supported ("fail", with Try to identify) or
    /// couldn't be told ("warn").
    pub support: Option<(&'static str, &'static str, String)>,
    /// How ready: "ok", "warn", "fail", or None while checking; and the line.
    pub tone: Option<&'static str>,
    pub line: String,
    /// "Fix everything" has something to do.
    pub fixable: bool,
    /// Gathering or fixing now.
    pub busy: bool,
    /// The account check passed (the presence dot): from every check, not only those shown
    /// (while something fails, only what fails is shown).
    pub signed_in: bool,
}

/// The checklist: gathered again when `fresh` (Check again: signing in again too) or stale.
/// `all`: every check, not only what matters.
pub fn checks(core: &Core, fresh: bool, all: bool) -> Checklist {
    let mut inner = core.lock();
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
            inner = core.lock();
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
        signed_in: inner.checks.iter().any(|c| c.id == "account" && c.status == Status::Ok),
    }
}

/// A check's fix. The ones the launcher makes run on (the activity bar says how it went);
/// "choose-server" and "set-up-account" answer false: they're the setup's.
pub fn fix(core: &Arc<Core>, fix: &str) -> bool {
    let Some(fix) = fix_from(fix) else { return false };
    if launcher::checks::fix_title(fix).is_none() {
        return false;
    }
    start_fix(core, fix);
    true
}

fn start_fix(core: &Arc<Core>, fix: Fix) {
    let mut inner = core.lock();
    let (Some(game), Some(title)) = (inner.game.as_ref(), launcher::checks::fix_title(fix)) else {
        return;
    };
    if inner.fixing || inner.setup_running {
        return;
    }
    let dir = game.dir.clone();
    let log = inner.notices.start_with(core.notifier(), title);
    inner.fixing = true;
    drop(inner);
    core.changed(Changed::Activity);
    let core = core.clone();
    std::thread::spawn(move || {
        let result = launcher::task::guarded(|| launcher::checks::run_fix(fix, &dir).unwrap_or_else(|| Ok(String::new())));
        fixed(&core, log, result, Some(fix));
    });
}

/// A fix (or identifying the game) ended: said in its activity, and the checklist gathered again.
fn fixed(core: &Core, log: launcher::activity::Handle, result: Result<String, String>, fix: Option<Fix>) {
    let mut inner = core.lock();
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
    core.changed(Changed::Checks);
    core.changed(Changed::Activity);
}

/// Looks for the hooks' addresses in an unsupported game version.
pub fn identify(core: &Arc<Core>) {
    let mut inner = core.lock();
    let Some(game) = inner.game.as_ref() else { return };
    if inner.fixing {
        return;
    }
    let (dir, version) = (game.dir.clone(), launcher::checks::game_version(game));
    let log = inner.notices.start_with(core.notifier(), "Identifying the game's version");
    inner.fixing = true;
    drop(inner);
    core.changed(Changed::Activity);
    let core = core.clone();
    std::thread::spawn(move || {
        let result = launcher::task::guarded(|| launcher::checks::identify(&dir, version));
        fixed(&core, log, result, None);
    });
}

/// Fix everything: the setup again on the player's server, with their name.
pub fn fix_everything(core: &Arc<Core>) -> bool {
    let inner = core.lock();
    if inner.setup_running {
        return false;
    }
    let Some(game) = inner.game.as_ref() else { return false };
    let profile = game.cfg.current_profile().cloned().unwrap_or_default();
    let name = Some(profile.user.username).filter(|n| !n.is_empty());
    let asked = connect::ask(game, &profile.server, name, false, inner.directory.as_deref());
    drop(inner);
    start_setup(core, asked);
    true
}

/// The player's server moved its players (global matchmaking): sets them up on the new one
/// with their name and identity, and says so. Asked once at start; nothing while the game runs,
/// a task runs, or another tool manages the game.
pub fn follow_move(core: &Arc<Core>) -> Option<String> {
    let mut inner = core.lock();
    if game_running(&mut inner) || inner.setup_running || inner.fixing {
        return None;
    }
    let game = inner.game.as_ref().filter(|g| g.managed.is_none())?;
    let cfg = (*game.cfg).clone();
    drop(inner);
    let (from, to) = launcher::task::guarded(|| Ok::<_, String>(launcher::flow::moved(&cfg))).ok().flatten()?;
    let mut inner = core.lock();
    let game = inner.game.as_ref()?;
    let name = game.cfg.current_profile().map(|p| p.user.username.clone()).filter(|n| !n.is_empty());
    let asked = connect::ask(game, &to, name, true, inner.directory.as_deref());
    let line = format!("{from} moved its players to {to}: setting you up there with your name and identity.");
    inner.notices.info(line.clone());
    drop(inner);
    start_setup(core, asked);
    Some(line)
}

/// Switches to a server from the menu (the network's): set up at once with the player's name,
/// in the activity bar. Why not, when it can't now.
pub fn switch_server(core: &Arc<Core>, host: &str) -> Result<(), String> {
    let mut inner = core.lock();
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
    let asked = connect::ask(game, host, name, true, inner.directory.as_deref());
    drop(inner);
    start_setup(core, asked);
    Ok(())
}

/// Switches to a friend's server (confirmed in the window): set up there with the player's
/// identity.
pub fn switch_to_friend(core: &Arc<Core>, host: &str) -> bool {
    let mut inner = core.lock();
    if game_running(&mut inner) || inner.setup_running {
        return false;
    }
    let Some(game) = inner.game.as_ref() else { return false };
    if !setup::directory::listable_host(host) {
        return false;
    }
    let asked = connect::ask(game, host, None, true, inner.directory.as_deref());
    drop(inner);
    start_setup(core, asked);
    true
}

/// The player's name, when the server said it breaks the rules: only while it's still their
/// name (a rename takes effect at once).
fn name_not_allowed(friends: &launcher::flow::Friends, game: &Game) -> bool {
    friends
        .name_not_allowed
        .as_ref()
        .is_some_and(|name| game.cfg.current_profile().is_some_and(|p| &p.user.username == name))
}

#[derive(Debug, Clone, Default)]
pub struct FriendView {
    pub id: String,
    pub name: String,
    pub activity: Option<String>,
    pub joinable: bool,
    /// The player is in that game with them already ("You're in it", no Join).
    pub with_me: bool,
}

#[derive(Debug, Clone, Default)]
pub struct OtherView {
    pub id: String,
    pub name: String,
    pub activity: Option<String>,
    pub asked: bool,
    pub asked_us: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ElsewhereView {
    pub name: String,
    /// Where they are (the server's region, else its name), and its address.
    pub place: String,
    pub host: String,
    /// On the internet: one server can't send players into their own network.
    pub public: bool,
}

/// The Squad tab: friends here and on other servers, others online, what's played here.
#[derive(Debug, Clone, Default)]
pub struct Squad {
    pub online: Vec<FriendView>,
    pub others: Vec<OtherView>,
    pub elsewhere: Vec<ElsewhereView>,
    pub offline: usize,
    pub requests: usize,
    /// The server joins a friend's game without an invitation: the Join buttons show.
    pub join_offered: bool,
    pub managed: bool,
    /// What's being played on the player's server: its public matches, and the rest in a line.
    pub playing: Option<Vec<String>>,
    pub playing_more: Option<String>,
}

/// The Squad tab: friends fetched again when `fresh` (after acting on someone), else every
/// half minute; what's played every 15 seconds.
pub fn squad(core: &Core, fresh: bool) -> Squad {
    let mut inner = core.lock();
    let Some(game) = inner.game.as_ref() else {
        return Squad::default();
    };
    let managed = game.managed.is_some();
    let cfg = (*game.cfg).clone();
    let server = game.cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty());
    let stale = fresh || inner.checked.is_none_or(|t| t.elapsed() > CHECK_EVERY) || inner.friends.online.is_empty() && inner.friends.offline == 0;
    if !managed && stale && !inner.fetching_friends {
        inner.fetching_friends = true;
        drop(inner);
        let friends = launcher::task::guarded(|| launcher::flow::friends(&cfg));
        inner = core.lock();
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
            let playing = setup::server_info::playing(&server, Duration::from_secs(5));
            inner = core.lock();
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
                with_me: o.with_me,
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
        managed,
        playing,
        playing_more,
    }
}

/// Joins a friend's game: the game is started first if it isn't running. The activity bar says
/// how it went.
pub fn join_friend(core: &Core, id: &str, name: &str) {
    let mut inner = core.lock();
    let running = game_running(&mut inner);
    drop(inner);
    if !running && !play(core, PlayMode::Play) {
        return;
    }
    let inner = core.lock();
    let Some(game) = inner.game.as_ref() else { return };
    let (cfg, dir) = (game.cfg.clone(), game.dir.clone());
    drop(inner);
    let result = launcher::task::guarded(|| launcher::flow::join_friend(&cfg, &dir, id, name));
    say(core, result);
}

/// A line in the activity bar: done, or what went wrong.
fn say(core: &Core, result: Result<String, String>) {
    let mut inner = core.lock();
    match result {
        Ok(line) => inner.notices.info(line),
        Err(line) => inner.notices.error(line),
    }
    drop(inner);
    core.changed(Changed::Activity);
}

/// The Matches screen: every public match on the player's server in full; a server before
/// 0.5.0 says no more than its open matches, turned into the same rows with no one named.
pub fn lobbies(core: &Core) -> launcher::matches::Lobbies {
    let inner = core.lock();
    let Some(cfg) = inner.game.as_ref().map(|g| (*g.cfg).clone()) else {
        return launcher::matches::Lobbies::default();
    };
    drop(inner);
    let got = launcher::task::guarded(|| Ok::<_, String>(launcher::matches::lobbies(&cfg))).unwrap_or_default();
    if got.offered || got.error.is_some() {
        return got;
    }
    let Some(server) = cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty()) else {
        return got;
    };
    let old = launcher::task::guarded(|| Ok::<_, String>(launcher::matches::list(&server))).unwrap_or_default();
    launcher::matches::Lobbies {
        offered: false,
        error: None,
        summary: old.others,
        hidden: String::new(),
        lobbies: old
            .rows
            .into_iter()
            .map(|r| {
                let mut parts = r.players.split('/');
                let players = parts.next().and_then(|p| p.trim().split(' ').next()?.parse().ok()).unwrap_or(0);
                let seats = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                launcher::matches::Lobby {
                    room: r.room,
                    kind: if r.mode.to_ascii_lowercase().contains("co-op") || r.mode.to_ascii_lowercase().contains("coop") {
                        "coop"
                    } else {
                        "svm"
                    }
                    .into(),
                    mode: r.mode,
                    map: r.map,
                    players,
                    seats,
                    status: if r.started { "Under way" } else { "In lobby" }.into(),
                    tone: if r.started || r.full { "warn" } else { "ok" },
                    under_way: r.started,
                    full: r.full,
                    joinable: !r.started && !r.full,
                    ..Default::default()
                }
            })
            .collect(),
    }
}

/// The player in line for `room` (a match under way or full), kept there, or out of it (`on`
/// false), on the server's queue: their place and how many wait; `Ok(None)` where the server
/// has none.
pub fn queue(core: &Core, room: u32, on: bool) -> Result<Option<(u32, u32)>, String> {
    let cfg = core.lock().game.as_ref().map(|g| (*g.cfg).clone()).ok_or("Choose the game's folder first.")?;
    launcher::task::guarded(|| launcher::matches::queue(&cfg, room, on))
}

/// Whether the player's server lets them pick a match or start a new one ("choose-match").
pub fn choose_match_offered(core: &Core) -> bool {
    let Some(server) = core.server() else { return false };
    launcher::task::guarded(|| Ok::<_, String>(launcher::flow::offers(&server, "choose-match"))).unwrap_or(false)
}

/// Joins the public match in `room`, picked from the list: the game is started first if it
/// isn't running, as for a friend's Join.
pub fn join_match(core: &Core, room: u32) {
    let mut inner = core.lock();
    let running = game_running(&mut inner);
    drop(inner);
    if !running && !play(core, PlayMode::Play) {
        return;
    }
    let inner = core.lock();
    let Some(game) = inner.game.as_ref() else { return };
    let (cfg, dir) = (game.cfg.clone(), game.dir.clone());
    drop(inner);
    let result = launcher::task::guarded(|| launcher::matches::join(&cfg, &dir, room));
    say(core, result);
}

/// A brand-new public match of `mode`: the player's searches find nothing until they host one,
/// and the game is started into that mode's lobby (and the Spies vs Mercs `playlist`) if it
/// isn't running.
pub fn new_match(core: &Core, mode: PlayMode, playlist: Option<u32>) {
    let inner = core.lock();
    let Some(game) = inner.game.as_ref() else { return };
    let (cfg, game_dir) = ((*game.cfg).clone(), game.dir.clone());
    drop(inner);
    let result = launcher::task::guarded(|| launcher::matches::new_match(&cfg));
    let mut inner = core.lock();
    let holds = match result {
        Ok(holds) => holds,
        Err(line) => {
            inner.notices.error(line);
            drop(inner);
            return core.changed(Changed::Activity);
        }
    };
    inner.new_match_until = Some(Instant::now() + holds);
    let running = game_running(&mut inner);
    let minutes = holds.as_secs().div_ceil(60);
    let playlist = playlist.filter(|_| mode == PlayMode::Svm);
    let hosts_itself = !running && launcher::matches::ask_game_to_matchmake(&game_dir, playlist, false);
    inner.notices.info(launcher::matches::new_match_notice(running, hosts_itself, minutes));
    drop(inner);
    if !running {
        play(core, mode);
    }
    core.changed(Changed::Activity);
}

/// Quick match in `mode` (and the Spies vs Mercs `playlist`): the game starts and starts
/// matchmaking by itself, joining an open match if there is one. A new match the server was
/// still asked for is taken back first, or the game would host.
pub fn quick_match(core: &Core, mode: PlayMode, playlist: Option<u32>) -> bool {
    let mut inner = core.lock();
    if game_running(&mut inner) {
        inner.notices.info("The game is running: use Quick match in its online menu.");
        drop(inner);
        core.changed(Changed::Activity);
        return false;
    }
    let Some(game) = inner.game.as_ref() else { return false };
    let (cfg, game_dir) = ((*game.cfg).clone(), game.dir.clone());
    let had_new_match = inner.new_match_until.take().is_some();
    drop(inner);
    if had_new_match {
        if let Err(e) = launcher::task::guarded(|| launcher::matches::cancel_new_match(&cfg)) {
            tracing::info!("quick match: the new match wasn't taken back ({e})");
        }
    }
    let playlist = playlist.filter(|_| mode == PlayMode::Svm);
    let by_itself = launcher::matches::ask_game_to_matchmake(&game_dir, playlist, true);
    core.lock().notices.info(launcher::matches::quick_match_notice(by_itself));
    let started = play(core, mode);
    if !started {
        launcher::matches::forget_game_hosting(&game_dir);
    }
    core.changed(Changed::Activity);
    started
}

/// Takes a new match back: searches find open matches again.
pub fn cancel_new_match(core: &Core) {
    let inner = core.lock();
    let Some(game) = inner.game.as_ref() else { return };
    let cfg = (*game.cfg).clone();
    launcher::matches::forget_game_hosting(&game.dir);
    drop(inner);
    let result = launcher::task::guarded(|| launcher::matches::cancel_new_match(&cfg));
    let mut inner = core.lock();
    match result {
        Ok(()) => {
            inner.new_match_until = None;
            inner.notices.info("Matchmaking finds open matches again.");
        }
        Err(line) => inner.notices.error(line),
    }
    drop(inner);
    core.changed(Changed::Activity);
}

/// Asks someone online to be friends ("request"), invites them to the game ("invite"), or
/// blocks them ("block"). The activity bar says how it went.
pub fn player_action(core: &Core, action: &str, id: &str, name: &str) {
    use launcher::network::PlayerAction;
    let action = match action {
        "request" => PlayerAction::Request,
        "invite" => PlayerAction::Invite,
        "block" => PlayerAction::Block,
        "accept" => PlayerAction::Accept,
        "decline" => PlayerAction::Decline,
        "cancel" => PlayerAction::Cancel,
        "remove" => PlayerAction::Remove,
        "unblock" => PlayerAction::Unblock,
        _ => return,
    };
    let inner = core.lock();
    let Some(game) = inner.game.as_ref() else { return };
    let cfg = (*game.cfg).clone();
    drop(inner);
    let result = launcher::task::guarded(|| launcher::flow::player_action(&cfg, action, id, "", name));
    say(core, result);
    // Asked again, so "Add friend" turns into "Asked".
    core.lock().checked = None;
}

/// The player's server's news in full, newest first: (title, text, link), the server's words as
/// text only.
pub fn news_items(core: &Core) -> Vec<(String, String, String)> {
    news(core);
    core.lock()
        .news
        .as_ref()
        .map(|(_, _, items)| {
            items
                .iter()
                .map(|n| (hooks_config::text::clip(&n.title, 80), hooks_config::text::clip(&n.text, 500), n.link.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The player's server's news, newest first: the server's words, as text only.
pub fn news(core: &Core) -> Vec<String> {
    let Some(server) = core.server() else { return Vec::new() };
    let fresh = core.lock().news.as_ref().is_some_and(|(from, at, _)| *from == server && at.elapsed() < NEWS_EVERY);
    if !fresh {
        let news = setup::server_info::news(&server, Duration::from_secs(5));
        core.lock().news = Some((server.clone(), Instant::now(), news));
    }
    let inner = core.lock();
    inner
        .news
        .as_ref()
        .filter(|(from, ..)| *from == server)
        .map(|(_, _, items)| items.iter().map(|n| hooks_config::text::clip(&n.title, 80)).collect())
        .unwrap_or_default()
}

/// One server in the notice.
#[derive(Debug, Clone, Default)]
pub struct NoticeRow {
    pub place: String,
    pub mine: bool,
    pub version: String,
    pub doing: String,
    pub meter: Option<f32>,
    /// "waiting", "now" or "done".
    pub tone: &'static str,
}

/// The notice under the launch bar: a release going out, or maintenance on the player's
/// server or the network.
#[derive(Debug, Clone, Default)]
pub struct NoticeView {
    pub heading: String,
    pub eta: String,
    pub text: String,
    pub note: Option<String>,
    /// Its button: playing on another server meanwhile (`host`), with its label.
    pub play_on: Option<(String, String)>,
    pub servers: Vec<NoticeRow>,
    /// What the launch bar says instead of how ready the player is, and whether Play waits.
    pub status: String,
    pub blocks_play: Option<String>,
    /// When to ask again.
    pub every: Duration,
}

/// The notice for the player, if there's one; the directory is read again as often as the
/// notice says (every minute, or 15 s while their server restarts or is down). The trial
/// doesn't update itself, so it never offers a newer launcher.
pub fn notice(core: &Core) -> Option<NoticeView> {
    let make = |inner: &mut Inner| {
        let game = inner.game.as_ref().filter(|g| g.managed.is_none())?;
        let current = game.cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty())?;
        let servers = inner.directory.clone().unwrap_or_default();
        launcher::update_notice::for_player(&current, &servers, inner.rollout.as_ref(), inner.network_maintenance.as_ref(), &mut inner.my_listing, None)
    };
    let mut inner = core.lock();
    let first = make(&mut inner);
    let every = first.as_ref().map_or(Duration::from_secs(60), launcher::update_notice::Notice::refresh_every);
    let due = inner.browsed_at.is_none_or(|t| t.elapsed() >= every);
    let watching = inner.rollout.is_some() || first.is_some();
    let busy = inner.setup_running;
    drop(inner);
    let notice = if watching && due && !busy {
        browse(core);
        make(&mut core.lock())
    } else {
        first
    };
    use launcher::update_notice::Action as A;
    use launcher::update_notice::Tone;
    notice.map(|n| NoticeView {
        every: n.refresh_every(),
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

/// Closes the notice that a Uplay emulator isn't a supported setup, for good.
pub fn dismiss_emulator_notice(core: &Core) {
    if let Some(game) = core.lock().game.as_mut() {
        game.uplay_emulator_told = true;
    }
    Prefs::set_uplay_emulator_told();
}

/// This PC's identity in short, read once it's there (it's decrypted from disk; the guided
/// setup makes it).
fn identity_short() -> Option<String> {
    static SHORT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut short = SHORT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if short.is_none() {
        *short = setup::player_identity::load().ok().flatten().map(|id| identity::short(&id.global_id()));
    }
    short.clone()
}

/// One of the game's loading screens for the stage, a different one each start: its pixels
/// (RGBA) and size. None for a game without them (the night-vision backdrop stays).
pub fn key_art(core: &Core) -> Option<(Vec<u8>, u32, u32)> {
    let dir = core.lock().game.as_ref()?.dir.clone();
    let art = setup::key_art::load(&dir, rand::random::<u32>() as usize)
        .map_err(|e| tracing::info!("No banner art from the game: {e}"))
        .ok()?;
    Some((art.rgba, art.width as u32, art.height as u32))
}

/// The Spies vs Mercs modes a match can be found or hosted in: (playlist, name).
pub fn svm_modes() -> Vec<(u32, &'static str)> {
    launcher::matches::svm_modes()
}

/// The game folders found on this PC (Steam, Ubisoft Connect, the usual places).
pub fn find_games() -> Vec<String> {
    setup::game::find_game_dirs().iter().map(|d| d.display().to_string()).collect()
}

/// Works on the game in `dir` (or a folder inside it) from now on. Whether it's a game folder.
pub fn choose_game_dir(core: &Core, dir: &str) -> bool {
    let mut inner = core.lock();
    let Some(found) = setup::game::game_dir_in(std::path::Path::new(dir)) else {
        inner.notices.error(format!("{dir} doesn't contain Splinter Cell: Blacklist."));
        drop(inner);
        core.changed(Changed::Activity);
        return false;
    };
    Prefs::remember(&found);
    inner.game = Some(Game::open_tidied(found, &mut inner.notices));
    inner.checked = None;
    drop(inner);
    core.changed(Changed::Checks);
    core.changed(Changed::Activity);
    true
}

/// Opens a link in the player's browser: only `https://` ones (the launcher's own links, and
/// the servers' news shown as text).
pub fn open_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| "Not a link the launcher opens.".to_string())?;
    // Only a plain web address: no credentials in it, and nothing a program handed it could take
    // as its own options (explorer.exe reads commas as switches).
    if parsed.scheme() != "https" || parsed.host_str().is_none() || !parsed.username().is_empty() || parsed.as_str().chars().any(|c| matches!(c, ',' | '"' | '\\' | ' ')) {
        return Err("Not a link the launcher opens.".into());
    }
    // Windows' own handler for web addresses (the default browser), not explorer.exe's command line.
    #[cfg(target_os = "windows")]
    let opened = std::process::Command::new("rundll32").arg("url.dll,FileProtocolHandler").arg(parsed.as_str()).spawn();
    #[cfg(not(target_os = "windows"))]
    let opened = std::process::Command::new("xdg-open").arg(parsed.as_str()).spawn();
    opened.map(|_| ()).map_err(|e| e.to_string())
}

/// The Friends screen's lists, whether the player has an account to manage friends with, or
/// why they couldn't be read.
pub fn people(core: &Core) -> (Option<launcher::flow::People>, bool, Option<String>) {
    let Some(cfg) = core.lock().game.as_ref().map(|g| (*g.cfg).clone()) else {
        return (None, false, None);
    };
    let signed_in = cfg.current_profile().is_some_and(|p| !p.server.is_empty() && p.has_account());
    match launcher::task::guarded(|| launcher::flow::people(&cfg)) {
        Ok(people) => (Some(people), signed_in, None),
        Err(e) => (None, signed_in, Some(e)),
    }
}

/// Find players: by name, or who's online when `query` is empty.
pub fn find_players(core: &Core, query: &str) -> Result<Vec<launcher::flow::Person>, String> {
    let Some(cfg) = core.lock().game.as_ref().map(|g| (*g.cfg).clone()) else {
        return Ok(Vec::new());
    };
    launcher::task::guarded(|| launcher::flow::find_players(&cfg, query))
}

/// Connects the game to `server` (as typed or picked): installs the client, signs in or makes
/// the account (`new_name`, from the account dialog), writes the settings. Runs on; the activity
/// bar says how it goes, and a server with no account yet asks for a name (Changed::NeedsName).
pub fn connect(core: &Arc<Core>, server: &str, new_name: Option<String>) -> bool {
    let inner = core.lock();
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
    let asked = connect::ask(game, server, new_name, public_only, inner.directory.as_deref());
    drop(inner);
    start_setup(core, asked);
    true
}

/// The account dialog's server: where and what it is, how far, and what the player leaves.
#[derive(Debug, Clone, Default)]
pub struct AccountServer {
    pub place: String,
    pub kind: String,
    pub ping: Option<u32>,
    /// A server of the player's own (one they typed): the dialog confirms it.
    pub own: bool,
    /// The server the player is on now, where they stay if they don't.
    pub current: Option<String>,
    /// The network a server of their own leaves ("the community network").
    pub leaving: String,
}

pub fn account_server(core: &Core, server: &str, own: Option<bool>) -> AccountServer {
    let inner = core.lock();
    let own = own.unwrap_or_else(|| !inner.setup.as_ref().is_some_and(|a| a.plan.public_only));
    let directory = inner.directory.as_deref().unwrap_or_default();
    let listing = directory.iter().find(|(s, _)| s.host.eq_ignore_ascii_case(server));
    let (place, kind) = match listing {
        Some((s, _)) => (connect::place(s), format!("{} · community server", s.name)),
        None if own => (server.to_string(), "A server of your own".to_string()),
        None => (server.to_string(), "A friend's server".to_string()),
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
pub fn account_probe(core: &Core, server: &str) -> Result<launcher::flow::Probe, String> {
    let dir = core.lock().game.as_ref().map(|g| g.dir.clone()).ok_or("Choose the game's folder first.")?;
    launcher::task::guarded(|| launcher::flow::probe(&dir, server))
}

/// Up to three free names like `name` on the server at `api`.
pub fn name_suggestions(api: &str, name: &str) -> Vec<String> {
    launcher::task::guarded(|| launcher::flow::free_suggestions(api, name.trim()))
}

/// Servers of the player's own they connected to: (address, label, "used 2 days ago" or "in
/// use now", in use).
pub fn recent_servers(core: &Core) -> Vec<(String, String, String, bool)> {
    let now = identity::now();
    let current = core.lock().game.as_ref().and_then(|g| g.cfg.current_profile().map(|p| p.server.clone()));
    Prefs::recent_servers()
        .into_iter()
        .map(|r| {
            let in_use = current.as_deref().is_some_and(|c| c.trim().eq_ignore_ascii_case(r.address.trim()));
            (
                r.address.clone(),
                hooks_config::text::clip(r.name.as_deref().unwrap_or(&r.address), 40),
                connect::recent_when(&r.address, r.last_used, now, current.as_deref()),
                in_use,
            )
        })
        .collect()
}

/// The name a new account on a server of the player's own starts with.
pub fn usual_name(core: &Core) -> String {
    let dir = core.lock().game.as_ref().map(|g| g.dir.clone());
    dir.and_then(|d| launcher::flow::usual_name(&d)).unwrap_or_else(launcher::flow::windows_user)
}

/// Uses another network (its address as typed), when it answers as one.
pub fn use_network(core: &Core, typed: &str) -> Result<(), String> {
    let (url, count) = launcher::services::rt().block_on(launcher::network::check_network(typed))?;
    Prefs::set_directory(Some(url.clone()));
    let mut inner = core.lock();
    inner.directory = None;
    inner.directory_note = None;
    let s = if count == 1 { "" } else { "s" };
    inner.notices.info(format!(
        "Using the network {}: its {count} server{s} are in the server menu on the Play screen.",
        connect::url_host(&url)
    ));
    drop(inner);
    core.changed(Changed::Activity);
    Ok(())
}

/// The servers that answered on this PC's networks.
pub fn find_on_network() -> Result<Vec<String>, String> {
    connect::find_on_network().map(|ips| ips.iter().map(ToString::to_string).collect())
}
