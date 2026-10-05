//! The activity bar: one place, along the bottom of the window, for every task that takes a
//! while and every error. A task on a thread reports through a [`Handle`] (its steps, how far
//! along it is, and how it ended); the window shows one activity at a time, and counts the
//! others waiting behind it.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use eframe::egui;
use eframe::egui::RichText;

use crate::theme;

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

impl Progress {
    fn fraction(self) -> Option<f32> {
        match self {
            Self::Unknown => None,
            Self::Bytes { sent, total } => Some(if total == 0 { 0.0 } else { (sent as f32 / total as f32).clamp(0.0, 1.0) }),
        }
    }
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
    fn label(self) -> &'static str {
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
    started: Instant,
    /// When it finished, and how long it shows after.
    ended: Option<Instant>,
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
            started: Instant::now(),
            ended: None,
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

    /// Gone from the bar: done and shown long enough.
    fn expired(&self, now: Instant) -> bool {
        self.state == State::Done && self.ended.is_some_and(|at| now.duration_since(at) >= self.linger)
    }

    /// "step 2 of 4", while it's at one of its stages.
    fn stage_text(&self) -> Option<String> {
        (self.state == State::Working && self.stage > 0 && !self.stages.is_empty()).then(|| format!("step {} of {}", self.stage.min(self.stages.len()), self.stages.len()))
    }

    /// The details to copy: what failed, and the steps before it.
    fn details(&self) -> String {
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
    ctx: Option<egui::Context>,
}

impl Handle {
    pub fn id(&self) -> u64 {
        self.id
    }

    fn update(&self, f: impl FnOnce(&mut Activity)) {
        f(&mut self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
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
    /// The steps list of the activity shown is open.
    steps_open: bool,
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

    /// Starts an activity for a task: it shows as working until the handle says otherwise.
    pub fn start(&mut self, ctx: &egui::Context, title: impl Into<String>) -> Handle {
        let id = self.new_id();
        let inner = self.push(Activity::new(id, title.into()));
        ctx.request_repaint();
        Handle {
            id,
            inner,
            ctx: Some(ctx.clone()),
        }
    }

    /// A one-line confirmation ("Saved"): shown for a few seconds, longer for a long one.
    pub fn info(&mut self, text: impl Into<String>) {
        let id = self.new_id();
        let mut a = Activity::new(id, text.into());
        a.linger = reading_time(&a.title);
        a.finish(State::Done, Instant::now());
        self.push(a);
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

    pub fn dismiss(&mut self, id: u64) {
        self.list.retain(|a| lock(a).id != id);
    }

    /// Drops what has gone, and answers the activity to show with how many others wait.
    fn current(&mut self, now: Instant) -> Option<(Activity, usize)> {
        self.list.retain(|a| !lock(a).expired(now));
        let shown = self.list.iter().map(|a| lock(a).clone()).enumerate().min_by_key(|(i, a)| (a.rank(), *i))?.1;
        Some((shown, self.list.len() - 1))
    }

    /// The bar, along the bottom of the main area (call after the side menu's panel).
    pub fn show(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let Some((activity, waiting)) = self.current(now) else {
            self.steps_open = false;
            return;
        };
        match (&activity.state, activity.ended) {
            (State::Done, Some(at)) => ctx.request_repaint_after(activity.linger.saturating_sub(now.duration_since(at))),
            (State::Working, _) => ctx.request_repaint_after(Duration::from_millis(if reduced_motion() { 500 } else { 16 })),
            _ => {}
        }
        let mut pressed = None;
        let mut dismiss = false;
        let mut toggle_steps = false;
        egui::TopBottomPanel::bottom("activity")
            .frame(egui::Frame::new().fill(theme::SUNKEN).stroke(egui::Stroke::new(1.0, theme::CONTROL_LINE)))
            .show(ctx, |ui| {
                track(ui, &activity, now);
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 32,
                        right: 24,
                        top: 10,
                        bottom: 10,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // One height for the row, so the spinner or icon sits level with the title.
                            ui.set_min_height(34.0);
                            ui.spacing_mut().item_spacing.x = 12.0;
                            mark(ui, &activity.state);
                            let right = 330.0_f32.min(ui.available_width() * 0.5);
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width() - right, 34.0),
                                egui::Layout::top_down(egui::Align::LEFT).with_main_align(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(RichText::new(&activity.title).family(theme::strong()));
                                        let step = [activity.step.clone(), activity.stage_text()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                                        if !step.is_empty() && !matches!(activity.state, State::Failed { .. }) {
                                            ui.label(theme::muted(step));
                                        }
                                    });
                                    if let State::Failed { message, .. } = &activity.state {
                                        if !message.is_empty() {
                                            ui.label(RichText::new(message).color(theme::SOFT).size(13.5));
                                        }
                                    }
                                },
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                if let State::Failed { actions, .. } = &activity.state {
                                    if close_button(ui).on_hover_text("Dismiss").clicked() {
                                        dismiss = true;
                                    }
                                    // Right to left: the first action ends up leftmost, as the main one.
                                    for (i, action) in actions.iter().enumerate().rev() {
                                        let button = if i == 0 {
                                            theme::secondary(action.label())
                                        } else {
                                            egui::Button::new(action.label()).frame(false)
                                        };
                                        if ui.add(button).clicked() {
                                            pressed = Some(*action);
                                        }
                                    }
                                }
                                if !activity.steps.is_empty() && steps_button(ui, self.steps_open).clicked() {
                                    toggle_steps = true;
                                }
                                if waiting > 0 {
                                    ui.label(RichText::new(format!("+{waiting} waiting")).monospace().color(theme::MUTED));
                                }
                                if let (Progress::Bytes { sent, total }, State::Working) = (activity.progress, &activity.state) {
                                    ui.label(RichText::new(format!("{} of {}", megabytes(sent), megabytes(total))).monospace().color(theme::MUTED));
                                }
                            });
                        });
                    });
                if self.steps_open {
                    steps(ui, &activity, now);
                }
            });
        if toggle_steps {
            self.steps_open = !self.steps_open;
        }
        let id = activity.id;
        match pressed {
            Some(Action::CopyDetails) => {
                ctx.copy_text(activity.details());
                self.info("Copied the details.");
            }
            Some(action) => {
                self.dismiss(id);
                self.pressed.push((id, action));
            }
            None => {}
        }
        if dismiss {
            self.dismiss(id);
            self.steps_open = false;
        }
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

/// "1.4 MB".
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// The 3 px line along the top of the bar: moving while the progress is unknown, real when
/// known, red when failed, green when done.
fn track(ui: &mut egui::Ui, a: &Activity, now: Instant) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0, theme::LINE);
    let part = |from: f32, to: f32| {
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + rect.width() * from, rect.top()),
            egui::pos2(rect.left() + rect.width() * to, rect.bottom()),
        )
    };
    match &a.state {
        State::Done => p.rect_filled(rect, 0, theme::OK),
        State::Failed { .. } => p.rect_filled(rect, 0, theme::BAD),
        State::Working => match a.progress.fraction() {
            Some(f) => p.rect_filled(part(0.0, f), 0, theme::ACCENT),
            None if reduced_motion() => p.rect_filled(rect, 0, theme::ACCENT.gamma_multiply(0.5)),
            None => {
                // A third of the width sweeping across every 1.3 s.
                let t = (now.duration_since(a.started).as_secs_f32() / 1.3).fract();
                let from = -0.3 + 1.3 * t;
                p.rect_filled(part(from.max(0.0), (from + 0.3).min(1.0)), 0, theme::ACCENT)
            }
        },
    };
}

/// "Steps" with a chevron (painted: the fonts have no arrows), down to open, up to close.
fn steps_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let galley = ui.fonts_mut(|f| f.layout_no_wrap("Steps".into(), egui::FontId::new(13.5, theme::strong()), theme::SOFT));
    let size = galley.size() + egui::vec2(34.0, 12.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let p = ui.painter();
    if response.hovered() {
        p.rect_filled(rect, 6, theme::CONTROL);
    }
    let text_at = egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0);
    p.galley(text_at, galley, theme::SOFT);
    let c = egui::pos2(rect.right() - 13.0, rect.center().y);
    let d = if open { -1.0 } else { 1.0 };
    let stroke = egui::Stroke::new(1.8, theme::SOFT);
    p.line_segment([c + egui::vec2(-4.0, -2.0 * d), c + egui::vec2(0.0, 2.0 * d)], stroke);
    p.line_segment([c + egui::vec2(0.0, 2.0 * d), c + egui::vec2(4.0, -2.0 * d)], stroke);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, if open { "Hide steps" } else { "Show steps" }));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small painted cross, to dismiss.
fn close_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let p = ui.painter();
    if response.hovered() {
        p.rect_filled(rect, 6, theme::CONTROL);
    }
    let c = rect.center();
    let stroke = egui::Stroke::new(1.8, theme::SOFT);
    p.line_segment([c + egui::vec2(-4.5, -4.5), c + egui::vec2(4.5, 4.5)], stroke);
    p.line_segment([c + egui::vec2(-4.5, 4.5), c + egui::vec2(4.5, -4.5)], stroke);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Dismiss"));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A spinner while working, a tick when done, a cross when failed.
fn mark(ui: &mut egui::Ui, state: &State) {
    if *state == State::Working && !reduced_motion() {
        ui.add(egui::Spinner::new().size(18.0).color(theme::ACCENT));
        return;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    match state {
        State::Working => {
            p.circle_stroke(c, 8.0, egui::Stroke::new(2.0, theme::ACCENT));
        }
        State::Done => {
            let s = egui::Stroke::new(2.2, theme::OK);
            p.line_segment([c + egui::vec2(-6.0, 0.5), c + egui::vec2(-1.5, 5.0)], s);
            p.line_segment([c + egui::vec2(-1.5, 5.0), c + egui::vec2(7.0, -5.0)], s);
        }
        State::Failed { .. } => {
            let s = egui::Stroke::new(2.2, theme::BAD);
            p.circle_stroke(c, 8.5, s);
            p.line_segment([c + egui::vec2(0.0, -4.5), c + egui::vec2(0.0, 1.5)], s);
            p.circle_filled(c + egui::vec2(0.0, 4.5), 1.2, theme::BAD);
        }
    }
}

/// The steps list: each step with how long it took, the one it's at, and the stages to come.
fn steps(ui: &mut egui::Ui, a: &Activity, now: Instant) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0, theme::LINE);
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 64,
            right: 32,
            top: 8,
            bottom: 12,
        })
        .show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let failed = matches!(a.state, State::Failed { .. });
                for (i, s) in a.steps.iter().enumerate() {
                    let last = i + 1 == a.steps.len();
                    let (dot, color) = match s.took {
                        Some(_) => (theme::OK, theme::SOFT),
                        None if last && failed => (theme::BAD, theme::FG),
                        None => (theme::ACCENT, theme::FG),
                    };
                    let time = match s.took {
                        Some(took) => seconds(took),
                        None if failed => String::new(),
                        None => format!("{}…", seconds(now.duration_since(s.started))),
                    };
                    step_row(ui, dot, RichText::new(&s.text).color(color), &time);
                }
                if a.state == State::Working {
                    for name in a.stages.iter().skip(a.stage) {
                        step_row(ui, theme::CONTROL_LINE, RichText::new(name).color(theme::MUTED), "");
                    }
                }
            });
        });
}

fn step_row(ui: &mut egui::Ui, dot: egui::Color32, text: RichText, time: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, dot);
        ui.label(text.size(13.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(time).monospace().size(12.0).color(theme::MUTED));
        });
    });
}

fn seconds(d: Duration) -> String {
    format!("{:.1} s", d.as_secs_f32())
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

    fn shown(a: &mut Activities, now: Instant) -> Option<(String, usize)> {
        a.current(now).map(|(a, n)| (a.title, n))
    }

    #[test]
    fn one_shows_and_the_rest_are_counted() {
        let ctx = egui::Context::default();
        let mut list = Activities::default();
        let now = Instant::now();
        assert_eq!(shown(&mut list, now), None);
        let first = list.start(&ctx, "Switching to Sydney");
        let _second = list.start(&ctx, "Sending your report");
        assert_eq!(shown(&mut list, now), Some(("Switching to Sydney".into(), 1)));
        first.step("Signing in");
        assert_eq!(list.current(now).unwrap().0.step.as_deref(), Some("Signing in"));
    }

    #[test]
    fn done_shows_for_a_while_then_goes() {
        let ctx = egui::Context::default();
        let mut list = Activities::default();
        let working = list.start(&ctx, "Sending your report");
        let switch = list.start(&ctx, "Switching to Sydney");
        switch.done("Switched to Sydney");
        let now = Instant::now();
        // What just finished shows before what's still working.
        assert_eq!(shown(&mut list, now), Some(("Switched to Sydney".into(), 1)));
        assert_eq!(shown(&mut list, now + DONE_FOR), Some(("Sending your report".into(), 0)));
        working.finish_quietly();
        assert_eq!(shown(&mut list, Instant::now()), None, "a quiet finish doesn't show");
    }

    #[test]
    fn failed_stays_until_dismissed() {
        let ctx = egui::Context::default();
        let mut list = Activities::default();
        let working = list.start(&ctx, "Switching to Sydney");
        let report = list.start(&ctx, "Sending your report");
        report.fail("The server took too long.", &[Action::Retry, Action::WithoutLogs, Action::CopyDetails]);
        let later = Instant::now() + Duration::from_secs(3600);
        let (a, waiting) = list.current(later).unwrap();
        assert_eq!((a.title.as_str(), waiting), ("Sending your report", 1));
        assert!(matches!(a.state, State::Failed { ref actions, .. } if actions.len() == 3));
        // A failed task can't be marked done afterwards.
        report.done("Sent");
        assert!(matches!(list.current(later).unwrap().0.state, State::Failed { .. }));
        list.dismiss(report.id());
        assert_eq!(shown(&mut list, later), Some(("Switching to Sydney".into(), 0)));
        drop(working);
    }

    #[test]
    fn notices_are_one_line_activities() {
        let mut list = Activities::default();
        let now = Instant::now();
        list.info("Saved.");
        list.error("Couldn't save the settings: disk full");
        // The error shows first, and stays; the notice goes after its few seconds.
        assert_eq!(shown(&mut list, now), Some(("Couldn't save the settings: disk full".into(), 1)));
        let later = now + Duration::from_secs(60);
        assert_eq!(shown(&mut list, later), Some(("Couldn't save the settings: disk full".into(), 0)));
        assert_eq!(reading_time("Saved."), DONE_FOR);
        assert!(reading_time(&"word ".repeat(30)) > DONE_FOR);
        assert_eq!(reading_time(&"word ".repeat(300)), NOTICE_AT_MOST);
    }

    #[test]
    fn steps_count_their_time_and_stages() {
        let ctx = egui::Context::default();
        let mut list = Activities::default();
        let h = list.start(&ctx, "Switching to Sydney");
        h.stages(&["Reaching the server", "Signing in", "Writing the game's settings", "Checking the connection"]);
        h.stage(1);
        h.step("Looking up oceania.example.net…");
        h.stage(2);
        h.step("Signing in…");
        let a = list.current(Instant::now()).unwrap().0;
        assert_eq!(a.stage_text().as_deref(), Some("step 2 of 4"));
        assert!(a.steps[0].took.is_some());
        assert!(a.steps[1].took.is_none());
        h.done("Switched to Sydney");
        let a = list.current(Instant::now()).unwrap().0;
        assert!(a.steps.iter().all(|s| s.took.is_some()));
        assert_eq!(a.stage_text(), None);
    }

    #[test]
    fn actions_go_to_the_owner_once() {
        let mut list = Activities::default();
        list.pressed.push((7, Action::Retry));
        assert_eq!(list.take_action(8), None);
        assert_eq!(list.take_action(7), Some(Action::Retry));
        assert_eq!(list.take_action(7), None);
    }

    #[test]
    fn progress_as_a_fraction() {
        assert_eq!(Progress::Unknown.fraction(), None);
        assert_eq!(Progress::Bytes { sent: 1, total: 4 }.fraction(), Some(0.25));
        assert_eq!(Progress::Bytes { sent: 1, total: 0 }.fraction(), Some(0.0));
        assert_eq!(Progress::Bytes { sent: 9, total: 4 }.fraction(), Some(1.0));
        assert_eq!(megabytes(1_400_000), "1.4 MB");
    }
}
