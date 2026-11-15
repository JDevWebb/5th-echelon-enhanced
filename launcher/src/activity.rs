//! The activity bar: one place, along the bottom of the window, for every task that takes a
//! while and every error. A task on a thread reports through a [`Handle`] (its steps, how far
//! along it is, and how it ended); the window shows one activity at a time, and counts the
//! others waiting behind it.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// How long a finished task shows before it goes.
const DONE_FOR: Duration = Duration::from_secs(4);
/// How long a one-line notice shows at most, however long it is.
const NOTICE_AT_MOST: Duration = Duration::from_secs(12);

/// How far along a task is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    /// It can't say: the bar moves to show it's working.
    Unknown,
    /// An upload or download.
    Bytes { sent: u64, total: u64 },
}

/// What the player can do about a task that failed. The task's owner acts on it
/// ([`Activities::take_action`]); copying the details is done here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Retry,
    /// Sending a report again, without its logs.
    WithoutLogs,
    CopyDetails,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Retry => "Try again",
            Self::WithoutLogs => "Without logs",
            Self::CopyDetails => "Copy details",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Working,
    Done,
    Failed { message: String, actions: Vec<Action> },
}

/// One line of a task's steps, and how long it took (None: still going).
#[derive(Debug, Clone)]
pub struct StepLine {
    pub text: String,
    started: Instant,
    took: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct Activity {
    pub id: u64,
    pub title: String,
    /// What it's doing now, shown beside the title.
    pub step: Option<String>,
    pub progress: Progress,
    pub steps: Vec<StepLine>,
    /// A task's fixed stages ("step 2 of 4"), and the one it's at (from 1).
    stages: Vec<String>,
    stage: usize,
    pub state: State,
    /// When it finished, when the bar first showed it, and how long it shows from then.
    ended: Option<Instant>,
    shown: Option<Instant>,
    linger: Duration,
}

impl Activity {
    fn new(id: u64, title: String) -> Self {
        Self {
            id,
            title,
            step: None,
            progress: Progress::Unknown,
            steps: Vec::new(),
            stages: Vec::new(),
            stage: 0,
            state: State::Working,
            ended: None,
            shown: None,
            linger: DONE_FOR,
        }
    }

    fn end_step(&mut self, now: Instant) {
        if let Some(last) = self.steps.last_mut().filter(|s| s.took.is_none()) {
            last.took = Some(now.duration_since(last.started));
        }
    }

    fn finish(&mut self, state: State, now: Instant) {
        if self.state != State::Working {
            return;
        }
        if matches!(state, State::Done) {
            self.end_step(now);
        }
        self.state = state;
        self.ended = Some(now);
    }

    /// Gone from the bar: done and shown long enough, counted from when the bar first showed it
    /// (one finished while the launcher was busy, as the tunnel's notice while the game starts,
    /// went before the bar ever had it: the Surface, 16 Nov 2026).
    fn expired(&self, now: Instant) -> bool {
        self.state == State::Done && self.ended.is_some() && self.shown.is_some_and(|at| now.duration_since(at) >= self.linger)
    }

    /// The details to copy: what failed, and the steps before it.
    pub fn details(&self) -> String {
        let mut text = self.title.clone();
        if let State::Failed { message, .. } = &self.state {
            if !message.is_empty() {
                text.push_str(&format!("\n{message}"));
            }
        }
        for s in &self.steps {
            text.push_str(&format!("\n- {}", s.text));
        }
        text.push_str(&format!("\n({} {}, {})", env!("FE_PRODUCT"), env!("FE_RELEASE"), std::env::consts::OS));
        text
    }

    /// Which shows first: failures (until dismissed), then what just finished, then the
    /// oldest task still working.
    fn rank(&self) -> u8 {
        match self.state {
            State::Failed { .. } => 0,
            State::Done => 1,
            State::Working => 2,
        }
    }
}

/// A task's way to report from its thread. Cloning it reports to the same activity.
#[derive(Clone)]
pub struct Handle {
    id: u64,
    inner: Arc<Mutex<Activity>>,
    /// Called after each change, so the interface shows it.
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Handle {
    pub fn id(&self) -> u64 {
        self.id
    }

    fn update(&self, f: impl FnOnce(&mut Activity)) {
        f(&mut self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        if let Some(notify) = &self.notify {
            notify();
        }
    }

    /// A step: shown beside the title, and kept in the steps list with how long it took.
    pub fn step(&self, text: impl Into<String>) {
        let text = text.into();
        let now = Instant::now();
        self.update(|a| {
            a.end_step(now);
            a.step = Some(text.clone());
            a.steps.push(StepLine { text, started: now, took: None });
        });
    }

    /// The task's fixed stages, for "step n of m" and the steps still to come.
    pub fn stages(&self, names: &[&str]) {
        self.update(|a| a.stages = names.iter().map(|s| (*s).to_string()).collect());
    }

    /// The stage it's at now (from 1).
    pub fn stage(&self, n: usize) {
        self.update(|a| a.stage = a.stage.max(n));
    }

    pub fn title(&self, title: impl Into<String>) {
        let title = title.into();
        self.update(|a| a.title = title);
    }

    pub fn progress(&self, progress: Progress) {
        self.update(|a| a.progress = progress);
    }

    /// It worked: shown for a few seconds as `title`.
    pub fn done(&self, title: impl Into<String>) {
        let title = title.into();
        self.update(|a| {
            a.title = title;
            a.finish(State::Done, Instant::now());
        });
    }

    /// It ended without anything to say (the screen that asked takes it from here).
    pub fn finish_quietly(&self) {
        self.update(|a| {
            a.linger = Duration::ZERO;
            a.finish(State::Done, Instant::now());
        });
    }

    /// It failed: shown until dismissed, with what the player can do.
    pub fn fail(&self, message: impl Into<String>, actions: &[Action]) {
        let state = State::Failed {
            message: message.into(),
            actions: actions.to_vec(),
        };
        self.update(|a| a.finish(state, Instant::now()));
    }
}

/// Every activity, in the order they started.
#[derive(Default)]
pub struct Activities {
    list: Vec<Arc<Mutex<Activity>>>,
    next_id: u64,
    /// Buttons pressed, for the tasks' owners.
    pressed: Vec<(u64, Action)>,
}

impl Activities {
    fn push(&mut self, activity: Activity) -> Arc<Mutex<Activity>> {
        let activity = Arc::new(Mutex::new(activity));
        self.list.push(Arc::clone(&activity));
        activity
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Starts an activity, `notify` called after each change it reports.
    pub fn start_with(&mut self, notify: Option<Arc<dyn Fn() + Send + Sync>>, title: impl Into<String>) -> Handle {
        let id = self.new_id();
        let inner = self.push(Activity::new(id, title.into()));
        Handle { id, inner, notify }
    }

    /// A one-line confirmation ("Saved"): shown for a few seconds, longer for a long one.
    pub fn info(&mut self, text: impl Into<String>) {
        let id = self.new_id();
        let mut a = Activity::new(id, text.into());
        tracing::info!("Notice: {}", a.title);
        a.linger = reading_time(&a.title);
        a.finish(State::Done, Instant::now());
        self.push(a);
    }

    /// A notice that matters for the whole of the game's run (the tunnel's): shown for as long as
    /// any is, as the game's window soon covers the launcher's.
    pub fn info_long(&mut self, text: impl Into<String>) {
        self.info(text);
        if let Some(a) = self.list.last() {
            lock(a).linger = NOTICE_AT_MOST;
        }
    }

    /// A one-line error: shown until dismissed.
    pub fn error(&mut self, text: impl Into<String>) {
        let id = self.new_id();
        let mut a = Activity::new(id, text.into());
        a.finish(
            State::Failed {
                message: String::new(),
                actions: vec![],
            },
            Instant::now(),
        );
        self.push(a);
    }

    /// The button the player pressed on activity `id`, once. Retrying or sending without
    /// logs also takes the failed activity away (the owner starts a new one).
    pub fn take_action(&mut self, id: u64) -> Option<Action> {
        let i = self.pressed.iter().position(|(a, _)| *a == id)?;
        Some(self.pressed.remove(i).1)
    }

    /// The player pressed `action` on activity `id` (not Copy details, which the interface
    /// does): the activity goes, and its owner takes the action ([`Self::take_action`]).
    pub fn press(&mut self, id: u64, action: Action) {
        self.dismiss(id);
        self.pressed.push((id, action));
    }

    /// The details of activity `id` to copy, if it's still there.
    pub fn details(&self, id: u64) -> Option<String> {
        self.list.iter().map(|a| lock(a)).find(|a| a.id == id).map(|a| a.details())
    }

    pub fn dismiss(&mut self, id: u64) {
        self.list.retain(|a| lock(a).id != id);
    }

    /// Drops what has gone, and answers the activity to show with how many others wait.
    pub fn current(&mut self, now: Instant) -> Option<(Activity, usize)> {
        self.list.retain(|a| !lock(a).expired(now));
        let (at, shown) = self.list.iter().map(|a| lock(a).clone()).enumerate().min_by_key(|(i, a)| (a.rank(), *i))?;
        if shown.state == State::Done && shown.shown.is_none() {
            lock(&self.list[at]).shown = Some(now);
        }
        Some((shown, self.list.len() - 1))
    }
}

fn lock(a: &Mutex<Activity>) -> std::sync::MutexGuard<'_, Activity> {
    a.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How long a one-line notice stays: long enough to read.
fn reading_time(text: &str) -> Duration {
    let words = text.split_whitespace().count() as u64;
    DONE_FOR.max(Duration::from_millis(words * 300)).min(NOTICE_AT_MOST)
}

/// Whether the player asked their system for less motion: no moving bar or spinner then.
pub fn reduced_motion() -> bool {
    static REDUCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REDUCED.get_or_init(|| {
        if std::env::var_os("FE_REDUCED_MOTION").is_some() {
            return true;
        }
        system_reduced_motion()
    })
}

/// Windows: "Show animations in Windows" (Settings › Accessibility › Visual effects).
#[cfg(target_os = "windows")]
fn system_reduced_motion() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
    use windows::Win32::UI::WindowsAndMessaging::SPI_GETCLIENTAREAANIMATION;
    use windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS;
    let mut on = windows::Win32::Foundation::BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL to the pointer given.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(std::ptr::addr_of_mut!(on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && !on.as_bool()
}

/// Linux desktops: GNOME's (and most others') "enable animations" setting.
#[cfg(not(target_os = "windows"))]
fn system_reduced_motion() -> bool {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "enable-animations"])
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "false")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A notice made while the launcher was busy (the game starting) is shown when the bar can
    /// show it, for as long as it would have been, not already gone.
    #[test]
    fn a_notice_shows_its_time_from_when_its_shown() {
        let mut list = Activities::default();
        list.info("Your connection blocks the game's traffic (UDP), so the game goes through HTTPS instead.");
        let made = Instant::now();
        let later = made + Duration::from_secs(10);
        let (shown, _) = list.current(later).expect("still there, never shown yet");
        assert!(shown.title.contains("HTTPS"));
        let linger = shown.linger;
        assert!(list.current(later + linger - Duration::from_millis(100)).is_some(), "shown for its time");
        assert!(list.current(later + linger).is_none(), "then gone");
    }
}
