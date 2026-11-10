//! "How did that go?" after a game, and the reports it sends (the egui launcher's
//! feedback.rs, through the shared library): the game's close is watched for, the player is
//! asked unless they said not to, and a report left unsent is sent the next time. With beta
//! features on, the game's log goes after each game whatever the answer.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use launcher::activity::Action;
use launcher::feedback as f;
use launcher::prefs::Prefs;
use serde::Serialize;
use setup::feedback::GameExit;
use tauri::Emitter;
use tauri::Manager;

use crate::commands::changed;
use crate::commands::notifier;
use crate::commands::Launcher;
use crate::tauri;

/// How often the game is looked for.
const WATCH_EVERY: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Feedback {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The ask open now.
    asking: Option<f::Ask>,
    checking: bool,
    sending: bool,
    /// A report that couldn't be sent, by its activity, for Try again and Without logs.
    failed: Option<(u64, f::Ask)>,
    /// How the last game seen closing ended, for a report sent from Settings.
    last_exit: Option<GameExit>,
    /// When a close was last seen (both ways of seeing one fire for one close).
    closed_at: Option<Instant>,
}

impl Feedback {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The checklist as text, one check a line, for the report's checklist.txt.
fn checks_text(app: &tauri::AppHandle) -> String {
    let state = app.state::<Launcher>();
    let inner = state.lock();
    inner
        .checks
        .iter()
        .map(|c| format!("{:?}\t{}\t{}", c.status, c.title, c.detail.replace('\n', " ")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn game_dir(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.state::<Launcher>().lock().game.as_ref().map(|g| g.dir.clone())
}

/// Watches for the game closing (the one the launcher started, by its exit, or any by its
/// process), from start to the end; and sends a report left unsent last time, and says when the
/// admins replied to one.
pub fn start(app: &tauri::AppHandle) {
    resume(app);
    replies(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let mut seen = false;
        loop {
            std::thread::sleep(WATCH_EVERY);
            let state = app.state::<Launcher>();
            let mut inner = state.lock();
            let exited = inner.running.as_mut().and_then(|c| c.try_wait().ok().flatten()).map(|s| s.code());
            if exited.is_some() {
                inner.running = None;
            }
            let running = inner.running.is_some() || setup::game::game_running();
            drop(inner);
            match exited {
                Some(code) => closed(&app, code),
                None if seen && !running => closed(&app, None),
                None => {}
            }
            seen = running;
        }
    });
}

/// The game closed: the beta log goes if beta features are on, and the player is asked.
fn closed(app: &tauri::AppHandle, code: Option<i32>) {
    let feedback = app.state::<Feedback>();
    let mut st = feedback.lock();
    if st.closed_at.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
        return;
    }
    st.closed_at = Some(Instant::now());
    let exit = GameExit::closed(code);
    st.last_exit = Some(exit);
    let ask = st.asking.is_none() && !st.checking && !Prefs::feedback().1;
    st.checking |= ask;
    drop(st);
    let Some(dir) = game_dir(app) else { return };
    let checks = checks_text(app);
    // The checklist's next look: the game just closed.
    app.state::<Launcher>().lock().checked = None;
    let _ = app.emit("checks", ());
    let app = app.clone();
    std::thread::spawn(move || {
        beta(&app, &dir, exit, &checks);
        if ask {
            let found = launcher::task::guarded(|| f::check(dir, exit, checks, false));
            opened(&app, found);
        }
    });
}

/// With beta features on: the game's log, to the server's admins.
fn beta(app: &tauri::AppHandle, dir: &std::path::Path, exit: GameExit, checks: &str) {
    match launcher::task::guarded(|| f::beta_log(dir, exit, checks)) {
        Ok(Some(log)) => {
            let activity = app
                .state::<Launcher>()
                .lock()
                .notices
                .start_with(notifier(app), format!("Sending the game's log to {} (beta features are on)", log.shown));
            changed(app);
            match launcher::task::guarded(|| f::send_report_to(&log.server, &log.username, &log.password, log.report, &activity)) {
                Ok(_) => activity.done("The game's log is with the server's admins (beta features are on)."),
                Err(e) => activity.fail(format!("Couldn't send the game's log: {e}"), &[Action::CopyDetails]),
            }
        }
        Ok(None) => {}
        Err(e) => {
            app.state::<Launcher>().lock().notices.error(e);
            changed(app);
        }
    }
}

/// What looking at the game's close (or the player's own ask) found.
fn opened(app: &tauri::AppHandle, found: Result<Option<f::Ask>, String>) {
    let feedback = app.state::<Feedback>();
    let mut st = feedback.lock();
    st.checking = false;
    match found {
        Ok(Some(ask)) => {
            st.asking = Some(ask);
            drop(st);
            let _ = app.emit("feedback", ());
        }
        Ok(None) => {}
        Err(e) => {
            drop(st);
            app.state::<Launcher>().lock().notices.error(e);
            changed(app);
        }
    }
}

/// At start: the report left unsent last time, if the account it's for is still set up here.
fn resume(app: &tauri::AppHandle) {
    let Some((header, report)) = f::pending::load() else { return };
    let Some(header) = f::pending::try_again(header) else {
        tracing::info!("A report left unsent was tried 3 times; let go");
        return;
    };
    let launcher = app.state::<Launcher>();
    let mut inner = launcher.lock();
    let password = inner.game.as_ref().and_then(|g| {
        g.cfg
            .profiles
            .iter()
            .find(|p| p.api_server_url().as_str() == header.server && p.user.username.eq_ignore_ascii_case(&header.username))
            .and_then(|p| p.user.secret())
    });
    let Some(password) = password else {
        f::pending::clear();
        return;
    };
    let activity = inner.notices.start_with(notifier(app), "Sending the report you left unsent");
    drop(inner);
    let saved_at = header.saved_at;
    std::thread::spawn(
        move || match launcher::task::guarded(|| f::send_report_to(&header.server, &header.username, &password, report, &activity)) {
            Ok(_) => {
                f::pending::clear_if(saved_at);
                activity.done("Your report from last time is with the server's admins.");
            }
            // Kept: it goes again next time (three tries, for a week at most).
            Err(e) => activity.fail(format!("{e} It'll be tried again next time."), &[Action::CopyDetails]),
        },
    );
}

/// Once a run: a notice for any reply from the admins since the player last saw one.
fn replies(app: &tauri::AppHandle) {
    if !launcher::roadmap::has_identity() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(mine) = launcher::task::guarded(f::fetch_mine) else { return };
        let seen = Prefs::replies_seen();
        let newest = mine.iter().filter(|r| !r.reply.is_empty()).max_by_key(|r| r.replied_at.unwrap_or(0));
        if let Some(r) = newest.filter(|r| seen.is_none_or(|s| r.replied_at.unwrap_or(0) > s)) {
            app.state::<Launcher>().lock().notices.info(format!(
                "The admins of {} replied to your report: \"{}\" (Settings › Feedback)",
                if r.server.is_empty() { "your server" } else { &r.server },
                r.reply.lines().next().unwrap_or_default()
            ));
            Prefs::set_replies_seen(r.replied_at.unwrap_or(0));
            changed(&app);
        }
    });
}

/// The ask, for the page.
#[derive(Serialize)]
pub struct AskView {
    /// What made the launcher ask, and whose game data differs (after a version mismatch).
    why: &'static str,
    mismatch: Option<String>,
    /// The problems to tick (id, label), and those ticked at first.
    problems: Vec<(&'static str, &'static str)>,
    ticked: Vec<&'static str>,
    /// The logs that go with it: name and size in KB.
    files: Vec<(String, u64)>,
    attach: bool,
    /// Asked for from Settings: no "Don't ask again".
    manual: bool,
    server: String,
}

pub fn feedback(state: tauri::State<Feedback>) -> Option<AskView> {
    let st = state.lock();
    let ask = st.asking.as_ref()?;
    Some(AskView {
        why: f::why(&ask.triggers),
        mismatch: ask.mismatch.clone(),
        problems: f::PROBLEMS.to_vec(),
        ticked: ask.problems.iter().copied().collect(),
        files: ask.files.iter().map(|a| (a.name.clone(), a.size.div_ceil(1024))).collect(),
        attach: ask.attach,
        manual: ask.triggers.contains(&"manual"),
        server: f::server_shown(&ask.profile),
    })
}

/// Sends the report as the player filled it in; the upload carries on in the activity bar.
pub fn feedback_send(app: tauri::AppHandle, state: tauri::State<Feedback>, good: Option<bool>, problems: Vec<String>, comment: String, attach: bool) {
    let mut st = state.lock();
    let Some(mut ask) = st.asking.take() else { return };
    ask.good = good;
    ask.problems = f::PROBLEMS.iter().map(|(id, _)| *id).filter(|id| problems.iter().any(|p| p == id)).collect::<BTreeSet<_>>();
    ask.comment = comment.chars().take(2000).collect();
    ask.attach = attach;
    drop(st);
    start_sending(&app, ask);
}

fn start_sending(app: &tauri::AppHandle, ask: f::Ask) {
    let files = if ask.attach { ask.files.len() } else { 0 };
    let activity = app
        .state::<Launcher>()
        .lock()
        .notices
        .start_with(notifier(app), format!("Sending your report to {}", f::server_shown(&ask.profile)));
    match files {
        0 => activity.step("without logs"),
        1 => activity.step("with 1 log file"),
        n => activity.step(format!("with {n} log files")),
    }
    // Kept until the server has it: closing the launcher meanwhile loses nothing.
    f::pending::save(&ask.profile.api_server_url().to_string(), &ask.profile.user.username, &f::report_of(&ask));
    app.state::<Feedback>().lock().sending = true;
    changed(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let result = launcher::task::guarded(|| f::send(&ask, &activity));
        let feedback = app.state::<Feedback>();
        let mut st = feedback.lock();
        st.sending = false;
        match result {
            Ok(_) => {
                f::pending::clear();
                activity.done("Thanks! Your report is with the server's admins.");
            }
            Err(e) => {
                activity.title("Couldn't send your report");
                let actions: &[Action] = if ask.attach && !ask.files.is_empty() {
                    &[Action::Retry, Action::WithoutLogs, Action::CopyDetails]
                } else {
                    &[Action::Retry, Action::CopyDetails]
                };
                activity.fail(e, actions);
                st.failed = Some((activity.id(), ask));
            }
        }
        drop(st);
        changed(&app);
    });
}

/// Not now, or (`never`) don't ask again (Settings › Feedback asks again).
pub fn feedback_close(state: tauri::State<Feedback>, never: bool) {
    state.lock().asking = None;
    if never {
        Prefs::set_feedback_off(true);
    }
}

/// The player wants to send feedback or report a problem (Settings › Feedback).
pub fn ask_feedback(app: tauri::AppHandle, state: tauri::State<Feedback>) {
    let mut st = state.lock();
    if st.asking.is_some() || st.checking {
        return;
    }
    st.checking = true;
    let exit = st.last_exit.unwrap_or(GameExit::Unknown);
    drop(st);
    let Some(dir) = game_dir(&app) else {
        state.lock().checking = false;
        return;
    };
    let checks = checks_text(&app);
    let found = launcher::task::guarded(|| f::check(dir, exit, checks, true));
    opened(&app, found);
}

/// Try again or Without logs on a report that couldn't be sent: whether it was this one's.
pub fn take_action(app: &tauri::AppHandle, id: u64, action: Action) -> bool {
    let feedback = app.state::<Feedback>();
    let mut st = feedback.lock();
    let Some((failed, ask)) = st.failed.clone() else { return false };
    if failed != id {
        return false;
    }
    st.failed = None;
    drop(st);
    match action {
        Action::Retry => start_sending(app, ask),
        Action::WithoutLogs => start_sending(app, f::Ask { attach: false, ..ask }),
        Action::CopyDetails => {}
    }
    true
}

/// Whether a report is still sending (or the one left unsent last time), for the close ask.
pub fn report_sending(state: tauri::State<Feedback>) -> bool {
    state.lock().sending
}
