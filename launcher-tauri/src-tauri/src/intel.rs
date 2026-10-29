//! Intel's Roadmap (the egui launcher's roadmap.rs, through the shared library): what's being
//! built, read from the project's server (the copy kept from before until it answers), a
//! suggestion signed with the player's identity, and their suggestions with the admins' replies.

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use launcher::activity::Action;
use launcher::roadmap as r;
use serde::Serialize;
use tauri::Manager;

use crate::commands::changed;
use crate::commands::notifier;
use crate::commands::Launcher;

/// How long the roadmap is kept before it's read again.
const REFRESH_EVERY: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
pub struct Intel {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The roadmap shown, when it was read (Unix seconds), and whether just now (not the copy
    /// kept from before); when it was last asked for, and why it couldn't be read.
    shown: Option<(setup::roadmap::Roadmap, i64, bool)>,
    cache_read: bool,
    loaded_at: Option<Instant>,
    error: Option<String>,
    /// A suggestion that couldn't be sent, by its activity, to send again.
    failed: Option<(u64, r::Draft)>,
}

impl Intel {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Serialize)]
pub struct RoadmapView {
    lanes: Vec<setup::roadmap::Lane>,
    /// The date of the copy shown, when it isn't the one just read.
    as_of: Option<String>,
    error: Option<String>,
}

/// The roadmap: the copy kept from before at first, then the project server's (read again
/// every 15 minutes).
#[tauri::command(async)]
pub fn roadmap(state: tauri::State<Intel>) -> RoadmapView {
    let mut st = state.lock();
    if !std::mem::replace(&mut st.cache_read, true) {
        if let Some((roadmap, at)) = setup::app_data_dir().and_then(|d| setup::roadmap::read_cache(&d)) {
            st.shown = Some((roadmap, at, false));
        }
    }
    if st.loaded_at.is_none_or(|t| t.elapsed() >= REFRESH_EVERY) {
        st.loaded_at = Some(Instant::now());
        drop(st);
        let read = launcher::task::guarded(r::fetch_roadmap);
        st = state.lock();
        match read {
            Ok(roadmap) => {
                st.shown = Some((roadmap, identity::now(), true));
                st.error = None;
            }
            Err(e) => st.error = Some(e),
        }
    }
    let clock = setup::clock::Clock::local();
    RoadmapView {
        lanes: st.shown.as_ref().map(|(roadmap, _, _)| roadmap.lanes.clone()).unwrap_or_default(),
        as_of: st.shown.as_ref().filter(|(_, _, fresh)| !fresh).map(|(_, at, _)| clock.date(*at)),
        error: st.error.clone(),
    }
}

/// The suggestion form: what a suggestion can be about, its limits, and whether there's an
/// identity to sign it with yet.
#[derive(Serialize)]
pub struct Form {
    areas: Vec<&'static str>,
    title_max: usize,
    text_max: usize,
    identity: bool,
}

#[tauri::command(async)]
pub fn suggestion_form() -> Form {
    Form {
        areas: setup::roadmap::AREAS.to_vec(),
        title_max: setup::roadmap::TITLE_MAX,
        text_max: setup::roadmap::TEXT_MAX,
        identity: r::has_identity(),
    }
}

/// Why the suggestion can't be sent as it is, if it can't.
#[tauri::command(async)]
pub fn check_suggestion(area: usize, title: String, text: String) -> Option<String> {
    let area = setup::roadmap::AREAS.get(area)?;
    setup::roadmap::check(area, &title, &text).err()
}

/// Sends the suggestion, with the player's name and server; it carries on in the activity bar.
#[tauri::command(async)]
pub fn suggest(app: tauri::AppHandle, area: usize, title: String, text: String) -> Result<(), String> {
    let area = setup::roadmap::AREAS.get(area).ok_or("Choose what it's about.")?;
    let (title, text) = setup::roadmap::check(area, &title, &text)?;
    let profile = app.state::<Launcher>().lock().game.as_ref().and_then(|g| g.cfg.current_profile().cloned());
    let draft = r::Draft {
        area: (*area).to_string(),
        title,
        text,
        name: profile.as_ref().map(|p| p.user.username.clone()).unwrap_or_default(),
        server: profile.map(|p| p.server).filter(|s| !s.is_empty()),
    };
    start_sending(&app, draft);
    Ok(())
}

fn start_sending(app: &tauri::AppHandle, draft: r::Draft) {
    let activity = app.state::<Launcher>().lock().notices.start_with(notifier(app), "Sending your suggestion");
    activity.step(format!("\"{}\"", hooks_config::text::clip(&draft.title, 40)));
    changed(app);
    let app = app.clone();
    std::thread::spawn(move || {
        match launcher::task::guarded(|| r::send(&draft)) {
            Ok(()) => activity.done("Thanks! Your suggestion is with the project's admins."),
            Err(e) => {
                activity.title("Couldn't send your suggestion");
                activity.fail(e, &[Action::Retry, Action::CopyDetails]);
                app.state::<Intel>().lock().failed = Some((activity.id(), draft));
            }
        }
        changed(&app);
    });
}

/// Try again on a suggestion that couldn't be sent: whether it was this one's.
pub fn take_action(app: &tauri::AppHandle, id: u64, action: Action) -> bool {
    let intel = app.state::<Intel>();
    let mut st = intel.lock();
    match st.failed.clone() {
        Some((failed, draft)) if failed == id && action == Action::Retry => {
            st.failed = None;
            drop(st);
            start_sending(app, draft);
            true
        }
        _ => false,
    }
}

/// One of the player's suggestions, with its status and the admins' reply.
#[derive(Serialize)]
pub struct MineView {
    title: String,
    /// "Launcher · 3 Oct".
    line: String,
    /// "Planned", "Done", "Declined", or another word.
    status: &'static str,
    reply: String,
}

#[tauri::command(async)]
pub fn my_suggestions() -> Result<Vec<MineView>, String> {
    if !r::has_identity() {
        return Ok(Vec::new());
    }
    let clock = setup::clock::Clock::local();
    let mine = launcher::task::guarded(r::fetch_mine)?;
    Ok(mine
        .into_iter()
        .map(|s| MineView {
            line: if s.created_at > 0 {
                format!("{} · {}", s.area, clock.date(s.created_at))
            } else {
                s.area.clone()
            },
            status: setup::roadmap::status_word(&s.status),
            title: s.title,
            reply: s.reply,
        })
        .collect())
}
