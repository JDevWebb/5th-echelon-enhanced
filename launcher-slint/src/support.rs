//! Support (the egui launcher's support.rs, through the shared library): the player's
//! conversation with the community network's admins, signed with their identity, looked at now
//! and then for answers (a notice when one came), and messages sent with the logs they tick.

use std::sync::Mutex;
use std::time::Instant;

use launcher::activity::Action;
use launcher::prefs::Prefs;
use launcher::support as s;
use serde::Serialize;
use tauri::Emitter;
use tauri::Manager;

use crate::commands::changed;
use crate::commands::notifier;
use crate::commands::Launcher;
use crate::tauri;

#[derive(Default)]
pub struct Support {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    thread: Option<setup::support::Thread>,
    error: Option<String>,
    read_at: Option<Instant>,
    /// The screen shows (answers are read there, and looked for every minute).
    open: bool,
    /// An answer came that the screen hasn't shown yet: the side menu's dot (the notice is
    /// only a few seconds).
    answered: bool,
    /// A message started elsewhere (a name's review, from Settings), for the text box.
    draft: Option<String>,
    sending: bool,
    failed: Option<(u64, s::Draft)>,
}

impl Support {
    /// The conversation as if it had been read: `thread`, with the screen showing (the
    /// screenshots).
    pub(crate) fn read_as(&self, thread: setup::support::Thread) {
        let mut st = self.lock();
        st.thread = Some(thread);
        st.error = None;
        st.open = true;
        st.read_at = Some(Instant::now());
    }

    /// The screen is left: answers that come now aren't read by being polled, so the launcher's
    /// notice and the game's overlay still tell the player (the screen stayed "open" after a
    /// first visit, and each minute's poll marked every new answer read: the overlay never said
    /// so, seen on the gamebox 2026-11-12).
    pub(crate) fn closed(&self) {
        self.lock().open = false;
    }

    /// An answer waits to be read on the screen (the side menu's dot).
    pub(crate) fn answered(&self) -> bool {
        self.lock().answered
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Reads the conversation (marking answers read when the screen shows), saying once when the
/// admins answered.
fn read_now(app: &tauri::AppHandle) {
    let support = app.state::<Support>();
    let open = {
        let mut st = support.lock();
        st.read_at = Some(Instant::now());
        st.open
    };
    let read = launcher::task::guarded(|| s::read(open));
    let mut st = support.lock();
    match read {
        Ok((thread, read)) => {
            let last = thread.last_answer();
            st.answered = !read && thread.unread > 0;
            if read {
                Prefs::set_support_seen(last);
            } else if last > Prefs::support_seen().unwrap_or(0) && thread.unread > 0 {
                // Told once per answer: the badge stays until the screen shows it.
                app.state::<Launcher>().lock().notices.info("The admins answered your support message: see Support.");
                Prefs::set_support_seen(last);
                changed(app);
            }
            st.thread = Some(thread);
            st.error = None;
        }
        Err(e) => st.error = Some(e),
    }
    drop(st);
    let _ = app.emit("support", ());
}

/// Looks for answers from start to the end: every 10 minutes, every minute while the screen shows.
pub fn start(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        let (open, due) = {
            let support = app.state::<Support>();
            let st = support.lock();
            let every = if st.open { s::LOOK_WHILE_OPEN } else { s::LOOK_EVERY };
            (st.open, st.read_at.is_none_or(|t| t.elapsed() >= every))
        };
        if due && s::available() {
            read_now(&app);
        }
        std::thread::sleep(if open { std::time::Duration::from_secs(5) } else { std::time::Duration::from_secs(30) });
    });
}

#[derive(Serialize)]
pub struct MessageView {
    admin: bool,
    /// "You", or the admin's name.
    who: String,
    when: String,
    text: String,
    files: Option<String>,
}

#[derive(Serialize)]
pub struct ThreadView {
    /// There's an identity to sign with (made when the player first connects).
    identity: bool,
    messages: Option<Vec<MessageView>>,
    error: Option<String>,
    resolved: bool,
    unread: u32,
    max_text: usize,
    /// A message started elsewhere, for the text box (once).
    draft: Option<String>,
    has_game: bool,
    sending: bool,
    manual: Vec<(&'static str, &'static str)>,
}

/// The conversation, for the screen (`open`: it shows now; the first time, it's read at once).
pub fn support(app: tauri::AppHandle, state: tauri::State<Support>, open: bool) -> ThreadView {
    let opened = {
        let mut st = state.lock();
        let opened = open && !st.open;
        st.open = open;
        opened
    };
    if opened && s::available() {
        read_now(&app);
    }
    let has_game = app.state::<Launcher>().lock().game.is_some();
    let mut st = state.lock();
    let clock = setup::clock::Clock::local();
    ThreadView {
        identity: launcher::roadmap::has_identity(),
        messages: st.thread.as_ref().map(|t| {
            t.messages
                .iter()
                .map(|m| {
                    let admin = m.from_admin();
                    MessageView {
                        admin,
                        who: if admin { m.admin.clone().unwrap_or_else(|| "The admins".into()) } else { "You".into() },
                        when: format!("{} {}", clock.date(m.at), clock.clock_time(m.at)),
                        text: m.text.clone(),
                        files: (!m.files.is_empty()).then(|| format!("Sent with {}", m.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "))),
                    }
                })
                .collect()
        }),
        error: st.error.clone(),
        resolved: st.thread.as_ref().is_some_and(|t| t.status.as_deref() == Some("resolved")),
        unread: st.thread.as_ref().map_or(0, |t| t.unread),
        max_text: setup::support::MAX_TEXT,
        draft: st.draft.take(),
        has_game,
        sending: st.sending,
        manual: s::MANUAL.to_vec(),
    }
}

/// Starts a message elsewhere (a name's review), for Support's text box.
pub fn draft(app: &tauri::AppHandle, text: String) {
    app.state::<Support>().lock().draft = Some(text);
}

/// Why the message can't be sent as it is, if it can't.
pub fn support_check(text: String) -> Option<String> {
    setup::support::check(&text).err()
}

/// Sends the message with the logs ticked; it carries on in the activity bar.
pub fn support_send(app: tauri::AppHandle, text: String, game_log: bool, launcher_log: bool) -> Result<(), String> {
    setup::support::check(&text)?;
    let (game_dir, profile) = {
        let launcher = app.state::<Launcher>();
        let inner = launcher.lock();
        (
            inner.game.as_ref().map(|g| g.dir.clone()),
            inner.game.as_ref().and_then(|g| g.cfg.current_profile().cloned()),
        )
    };
    let draft = s::Draft {
        text: text.replace("\r\n", "\n").trim().to_string(),
        game_log: game_log && game_dir.is_some(),
        launcher_log,
        game_dir,
        server: profile.as_ref().map(|p| p.server.clone()).filter(|s| !s.is_empty()),
        name: profile.map(|p| p.user.username).unwrap_or_default(),
    };
    start_sending(&app, draft);
    Ok(())
}

fn start_sending(app: &tauri::AppHandle, draft: s::Draft) {
    let activity = app.state::<Launcher>().lock().notices.start_with(notifier(app), "Sending your support message");
    activity.step(match (draft.game_log, draft.launcher_log) {
        (true, true) => "with the game's and the launcher's logs",
        (true, false) => "with the game's log",
        (false, true) => "with the launcher's log",
        (false, false) => "without logs",
    });
    app.state::<Support>().lock().sending = true;
    changed(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let result = launcher::task::guarded(|| s::send(&draft));
        let support = app.state::<Support>();
        let mut st = support.lock();
        st.sending = false;
        let sent = match result {
            Ok(()) => {
                activity.done("Sent. The admins answer here; you'll be told when they do.");
                true
            }
            Err(e) => {
                activity.title("Couldn't send your support message");
                activity.fail(e, &[Action::Retry, Action::CopyDetails]);
                st.failed = Some((activity.id(), draft));
                false
            }
        };
        drop(st);
        if sent {
            read_now(&app);
            let _ = app.emit("support-sent", ());
        }
        changed(&app);
    });
}

/// Try again on a message that couldn't be sent: whether it was this one's.
pub fn take_action(app: &tauri::AppHandle, id: u64, action: Action) -> bool {
    let support = app.state::<Support>();
    let mut st = support.lock();
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
