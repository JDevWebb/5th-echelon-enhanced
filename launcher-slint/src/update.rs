//! The launcher keeping itself up to date (the egui launcher's, through the shared updater):
//! a newer release looked for at start and every 4 hours (not in a development build), offered
//! in a banner, and installed when the player says (the launcher replaces itself and restarts).
//! A server refusing this launcher as outdated looks at once, and offers it again after Later.

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use launcher::activity::Action;
use launcher::updater;
use serde::Serialize;
use tauri::Emitter;
use tauri::Manager;

use crate::commands::changed;
use crate::commands::notifier;
use crate::commands::Launcher;
use crate::tauri;

/// How often a newer release is looked for.
const RECHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);

#[derive(Default)]
pub struct Update {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    latest: Option<updater::Latest>,
    checked_at: Option<Instant>,
    checking: bool,
    updating: bool,
    /// The player said Later: the banner waits until a server refuses this launcher.
    later: bool,
}

impl Update {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A newer release's version, when there is one (for the update notice).
    pub fn newer(&self) -> Option<String> {
        self.lock().latest.as_ref().filter(|l| l.newer()).map(|l| l.version.clone())
    }
}

/// At start: the previous launcher left by an update goes, and a newer release is looked for
/// now and then.
pub fn start(app: &tauri::AppHandle) {
    std::thread::spawn(updater::clean_up);
    if updater::is_dev_build() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        let due = app.state::<Update>().lock().checked_at.is_none_or(|t| t.elapsed() >= RECHECK_EVERY);
        if due {
            check(&app, None);
        }
        std::thread::sleep(Duration::from_secs(60));
    });
}

/// Looks for a newer release. `shown`: the activity the player sees it in, and whether to
/// install what's found (the checklist's Update).
fn check(app: &tauri::AppHandle, shown: Option<(launcher::activity::Handle, bool)>) {
    let update = app.state::<Update>();
    {
        let mut st = update.lock();
        if st.checking {
            return;
        }
        st.checking = true;
        st.checked_at = Some(Instant::now());
    }
    let found = launcher::task::guarded(|| updater::latest().map_err(|e| e.to_string()));
    let mut st = update.lock();
    st.checking = false;
    match found {
        Ok(latest) => {
            let install = match &shown {
                Some((a, true)) if latest.newer() => {
                    a.finish_quietly();
                    true
                }
                Some((a, true)) => {
                    a.title("No newer launcher yet");
                    a.fail(
                        format!("{} is the latest: the server needs one that hasn't been published. Try again later.", latest.version),
                        &[],
                    );
                    false
                }
                Some((a, false)) if latest.newer() => {
                    a.done(format!("Version {} is available", latest.version));
                    false
                }
                Some((a, false)) => {
                    a.done(format!("Up to date: {} is the latest release", latest.version));
                    false
                }
                None => false,
            };
            st.latest = Some(latest);
            drop(st);
            if install {
                update_now(app.clone(), app.state::<Update>());
            }
        }
        Err(e) => {
            tracing::warn!("Couldn't check for updates: {e}");
            if let Some((a, _)) = shown {
                a.title("Couldn't look for a newer launcher");
                a.fail(e, &[Action::CopyDetails]);
            }
        }
    }
    let _ = app.emit("update", ());
    changed(app);
}

/// A server refused this launcher as outdated: look at once, and offer it again after Later.
pub fn outdated(app: &tauri::AppHandle) {
    if updater::is_dev_build() {
        return;
    }
    app.state::<Update>().lock().later = false;
    let app = app.clone();
    std::thread::spawn(move || check(&app, None));
}

/// The banner: a newer release, unless the player said Later or another tool manages the
/// install (it updates the launcher too).
#[derive(Serialize)]
pub struct Banner {
    version: String,
    page: String,
    updating: bool,
}

pub fn update_banner(app: tauri::AppHandle, state: tauri::State<Update>) -> Option<Banner> {
    let managed = app.state::<Launcher>().lock().game.as_ref().is_some_and(|g| g.managed.is_some());
    let st = state.lock();
    let latest = st.latest.as_ref().filter(|l| l.newer() && !managed && !st.later)?;
    Some(Banner {
        version: latest.version.clone(),
        page: latest.page.clone(),
        updating: st.updating,
    })
}

pub fn update_later(state: tauri::State<Update>) {
    state.lock().later = true;
}

/// Installs the newer release and restarts into it (a server started here stops first).
pub fn update_now(app: tauri::AppHandle, state: tauri::State<Update>) {
    let Some(latest) = ({
        let mut st = state.lock();
        if st.updating {
            return;
        }
        let latest = st.latest.clone().filter(updater::Latest::newer);
        st.updating = latest.is_some();
        latest
    }) else {
        return;
    };
    let activity = app
        .state::<Launcher>()
        .lock()
        .notices
        .start_with(notifier(&app), format!("Updating the launcher to {}", latest.version));
    activity.step("Downloading the new launcher; it restarts when it's ready");
    changed(&app);
    let _ = app.emit("update", ());
    app.state::<crate::host::Host>().stop();
    // On success this doesn't return: the new launcher starts and this one exits.
    let result = launcher::task::guarded(|| updater::update_self(&latest).map_err(|e| e.to_string()));
    state.lock().updating = false;
    if let Err(e) = result {
        activity.title("Couldn't update the launcher");
        activity.fail(e, &[Action::CopyDetails]);
    }
    let _ = app.emit("update", ());
    changed(&app);
}

/// The checklist's Update (and the notice's): installs the newer release, or looks for one and
/// installs it, or says there isn't one yet.
pub fn update_asked(app: tauri::AppHandle, state: tauri::State<Update>) {
    if updater::is_dev_build() {
        app.state::<Launcher>()
            .lock()
            .notices
            .error("This is a development build: it doesn't update itself. Get the new one from whoever sent it.");
        changed(&app);
        return;
    }
    if state.newer().is_some() {
        update_now(app.clone(), state);
        return;
    }
    let activity = app.state::<Launcher>().lock().notices.start_with(notifier(&app), "Looking for a newer launcher");
    check(&app, Some((activity, true)));
}

/// Settings › About's Check for updates: in the activity bar, and the answer for the page.
pub fn check_update(app: tauri::AppHandle, state: tauri::State<Update>) -> Result<(String, bool), String> {
    let activity = app.state::<Launcher>().lock().notices.start_with(notifier(&app), "Looking for a newer launcher");
    check(&app, Some((activity, false)));
    state
        .lock()
        .latest
        .as_ref()
        .map(|l| (l.version.clone(), l.newer()))
        .ok_or_else(|| "Couldn't check for updates.".into())
}
