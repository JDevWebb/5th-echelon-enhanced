//! The Support screen: the player's conversation with the community network's admins
//! (`setup::support`), signed with their identity. They write, with the game's or the
//! launcher's log if they choose (private details hidden first, as reports do); the admins'
//! answers show here, and a notice says when one came. The overlay says so in the game too
//! (through the player's server).

use std::time::Duration;
use std::time::Instant;

use eframe::egui;
use eframe::egui::RichText;
use setup::support::Thread;

use crate::activity::Action;
use crate::app::App;
use crate::app::Notices;
use crate::app::Prefs;
use crate::task::Slot;
use crate::theme;

/// Where support lives: the community network's coordinator.
const COORDINATOR: &str = setup::directory::COMMUNITY;
/// How often the launcher looks for answers: while the screen shows, and otherwise.
const LOOK_WHILE_OPEN: Duration = Duration::from_secs(60);
const LOOK_EVERY: Duration = Duration::from_secs(10 * 60);

/// A message as sent, to send again.
#[derive(Debug, Clone)]
struct Draft {
    text: String,
    game_log: bool,
    launcher_log: bool,
    game_dir: Option<std::path::PathBuf>,
    server: Option<String>,
    name: String,
}

pub struct Support {
    /// The conversation, as last read.
    thread: Option<Thread>,
    error: Option<String>,
    reading: Slot<Result<(Thread, bool), String>>,
    read_at: Option<Instant>,
    /// The screen showed last frame (its first frame reads at once, marking answers read).
    open: bool,

    text: String,
    game_log: bool,
    launcher_log: bool,
    sending: Slot<Result<(), String>>,
    sent: Option<(crate::activity::Handle, Draft)>,
    failed: Option<(u64, Draft)>,
}

impl Default for Support {
    fn default() -> Self {
        Self {
            thread: None,
            error: None,
            reading: Slot::default(),
            read_at: None,
            open: false,
            text: String::new(),
            // The logs help most; the player sees what they are and can untick them.
            game_log: true,
            launcher_log: true,
            sending: Slot::default(),
            sent: None,
            failed: None,
        }
    }
}

fn read(read: bool) -> Result<(Thread, bool), String> {
    let identity = crate::roadmap::load_identity()?;
    let url = setup::support::mine_url(COORDINATOR, &identity, identity::now(), read);
    crate::services::rt().block_on(async {
        let resp = crate::roadmap::client()?
            .get(url)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(crate::roadmap::unreachable)?;
        let body = crate::roadmap::body(resp).await?;
        let thread = setup::support::parse_thread(&body).map_err(|_| "The project's server sent an answer the launcher can't read.".to_string())?;
        Ok((thread, read))
    })
}

fn send(draft: &Draft) -> Result<(), String> {
    let identity = crate::roadmap::load_identity()?;
    // The files, as reports send them: private details hidden, logs cut to their end.
    let files = if draft.game_log || draft.launcher_log {
        let keep: Vec<std::net::Ipv4Addr> = draft
            .server
            .as_deref()
            .map(setup::net::resolve)
            .into_iter()
            .flatten()
            .filter_map(|ip| match ip {
                std::net::IpAddr::V4(v4) => Some(v4),
                std::net::IpAddr::V6(_) => None,
            })
            .collect();
        let private = setup::feedback::Private::of_this_pc(keep);
        let launcher_log = if draft.launcher_log { crate::logging::log_path() } else { None };
        let mut files = match &draft.game_dir {
            Some(dir) => setup::feedback::prepare(dir, launcher_log.as_deref(), &[], &private),
            // No game folder found yet: the launcher's log alone.
            None => launcher_log
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| setup::feedback::attach("launcher.log", &String::from_utf8_lossy(&b), &private).ok())
                .into_iter()
                .collect(),
        };
        if !draft.game_log {
            files.retain(|f| !f.name.starts_with("bl-"));
        }
        files
    } else {
        Vec::new()
    };
    let sender = setup::support::Sender {
        name: &draft.name,
        server: draft.server.as_deref(),
        launcher: env!("FE_RELEASE"),
    };
    let json = setup::support::message_body(COORDINATOR, &identity, &sender, &draft.text, &files, identity::now());
    crate::services::rt().block_on(async {
        // Files take a while on a slow line: longer than a read.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .post(format!("{COORDINATOR}/v1/support"))
            .header("content-type", "application/json")
            .body(json.to_string())
            .send()
            .await
            .map_err(crate::roadmap::unreachable)?;
        crate::roadmap::body(resp).await.map(|_| ())
    })
}

/// Whether Support is offered: on the community network, once there's an identity.
pub fn available() -> bool {
    Prefs::directory().is_some_and(|d| Prefs::is_community(&d)) && crate::roadmap::has_identity()
}

impl Support {
    /// Answers the player hasn't seen: the rail's badge.
    pub fn unread(&self) -> u32 {
        self.thread.as_ref().map_or(0, |t| t.unread)
    }

    /// Every frame, on whatever screen: looks for answers now and then (a notice when one
    /// came since the launcher last said so), and what came of sending.
    pub fn tick(&mut self, ctx: &egui::Context, notices: &mut Notices, showing: bool) {
        let opened = showing && !self.open;
        self.open = showing;
        match self.reading.poll() {
            Some(Ok((thread, read))) => {
                let last = thread.last_answer();
                if read {
                    Prefs::set_support_seen(last);
                } else if last > Prefs::support_seen().unwrap_or(0) && thread.unread > 0 {
                    // Told once per answer: the badge stays until the screen shows it.
                    notices.info("The admins answered your support message: see Support.");
                    Prefs::set_support_seen(last);
                }
                self.thread = Some(thread);
                self.error = None;
            }
            Some(Err(e)) => self.error = Some(e),
            None => {}
        }
        if available() && !self.reading.running() {
            let every = if showing { LOOK_WHILE_OPEN } else { LOOK_EVERY };
            if opened || self.read_at.is_none_or(|t| t.elapsed() >= every) {
                self.read_at = Some(Instant::now());
                self.reading.start(ctx, move || read(showing));
            }
        }
        if let Some(result) = self.sending.poll() {
            if let Some((activity, draft)) = self.sent.take() {
                match result {
                    Ok(()) => {
                        activity.done("Sent. The admins answer here; you'll be told when they do.");
                        if self.text.replace("\r\n", "\n").trim() == draft.text {
                            self.text.clear();
                        }
                        self.read_at = None;
                    }
                    Err(e) => {
                        activity.title("Couldn't send your support message");
                        activity.fail(e, &[Action::Retry, Action::CopyDetails]);
                        self.failed = Some((activity.id(), draft));
                    }
                }
            }
        }
        if let Some((id, draft)) = self.failed.clone() {
            if notices.take_action(id) == Some(Action::Retry) {
                self.failed = None;
                self.start_sending(ctx, notices, draft);
            }
        }
    }

    fn start_sending(&mut self, ctx: &egui::Context, notices: &mut Notices, draft: Draft) {
        let activity = notices.start(ctx, "Sending your support message");
        let what = match (draft.game_log, draft.launcher_log) {
            (true, true) => "with the game's and the launcher's logs",
            (true, false) => "with the game's log",
            (false, true) => "with the launcher's log",
            (false, false) => "without logs",
        };
        activity.step(what);
        let sending = draft.clone();
        self.sending.start(ctx, move || send(&sending));
        self.sent = Some((activity, draft));
    }
}

/// The Support screen.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let (page, game, notices) = app.support_mut();
    let profile = game.as_ref().and_then(|g| g.cfg.current_profile().cloned());
    let game_dir = game.as_ref().map(|g| g.dir.clone());
    egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        theme::page().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::display("Support", 32.0));
            ui.label(
                RichText::new("Write to the community network's admins about anything that goes wrong. They answer here, and you're told when they do, here and in the game.")
                    .color(theme::SOFT),
            );
            ui.add_space(14.0);
            if !crate::roadmap::has_identity() {
                ui.label(RichText::new("Connect to a server first: your messages are signed with the identity the launcher makes then.").color(theme::SOFT));
                return;
            }
            conversation(page, ui);
            ui.add_space(16.0);
            form(page, profile.as_ref(), game_dir, notices, &ctx, ui);
        });
    });
}

/// The messages, oldest first: the player's on the right, the admins' on the left.
fn conversation(page: &Support, ui: &mut egui::Ui) {
    if let Some(e) = &page.error {
        ui.label(theme::muted(e.as_str()));
        ui.add_space(6.0);
    }
    let Some(thread) = &page.thread else {
        if page.reading.running() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted("Reading your conversation…"));
            });
        }
        return;
    };
    if thread.messages.is_empty() {
        ui.label(theme::muted(
            "Nothing yet. Say what went wrong: when, on which server, and what you saw. Ticking the logs below helps the admins find it.",
        ));
        return;
    }
    let clock = setup::clock::Clock::local();
    let width = ui.available_width();
    for m in &thread.messages {
        let admin = m.from_admin();
        let layout = if admin {
            egui::Layout::left_to_right(egui::Align::TOP)
        } else {
            egui::Layout::right_to_left(egui::Align::TOP)
        };
        ui.with_layout(layout, |ui| {
            egui::Frame::new()
                .fill(if admin { theme::SURFACE } else { theme::SUNKEN })
                .stroke(egui::Stroke::new(1.0, if admin { theme::ACCENT.linear_multiply(0.35) } else { theme::LINE }))
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_max_width((width * 0.78).max(280.0));
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let who = if admin { m.admin.clone().unwrap_or_else(|| "The admins".into()) } else { "You".into() };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(who).family(theme::strong()).color(if admin { theme::ACCENT } else { theme::FG }));
                        ui.label(theme::muted(format!("{} {}", clock.date(m.at), clock.clock_time(m.at))).small());
                    });
                    ui.add(egui::Label::new(RichText::new(&m.text).color(theme::FG)).wrap());
                    if !m.files.is_empty() {
                        let names: Vec<&str> = m.files.iter().map(|f| f.name.as_str()).collect();
                        ui.label(theme::muted(format!("Sent with {}", names.join(", "))).small());
                    }
                });
        });
        ui.add_space(8.0);
    }
    if thread.status.as_deref() == Some("resolved") {
        ui.label(theme::muted("The admins marked this solved. Write again if it isn't, and it opens again.").small());
    }
}

/// The text box, the logs to send with it, and Send.
fn form(page: &mut Support, profile: Option<&setup::config::Profile>, game_dir: Option<std::path::PathBuf>, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(theme::caps("Message"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(theme::muted(format!("{}/{}", page.text.chars().count(), setup::support::MAX_TEXT)).small());
            });
        });
        ui.add(
            egui::TextEdit::multiline(&mut page.text)
                .hint_text("What went wrong, when, and on which server?")
                .char_limit(setup::support::MAX_TEXT)
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        let has_game = game_dir.is_some();
        ui.add_enabled(has_game, egui::Checkbox::new(&mut page.game_log, "The game's log (its last part, and the one before)"));
        ui.checkbox(&mut page.launcher_log, "The launcher's log");
        ui.label(theme::muted("Your PC's name, your user folder and your internet address are hidden in them first.").small());
        ui.add_space(8.0);
        let checked = setup::support::check(&page.text);
        let sending = page.sending.running();
        ui.horizontal(|ui| {
            let send = ui.add_enabled(checked.is_ok() && !sending, theme::primary("Send"));
            if sending {
                ui.spinner();
            } else if let Err(why) = &checked {
                if !page.text.trim().is_empty() {
                    ui.label(theme::muted(why.as_str()));
                }
            }
            if send.clicked() && checked.is_ok() {
                let draft = Draft {
                    text: page.text.replace("\r\n", "\n").trim().to_string(),
                    game_log: page.game_log && has_game,
                    launcher_log: page.launcher_log,
                    game_dir: game_dir.clone(),
                    server: profile.map(|p| p.server.clone()).filter(|s| !s.is_empty()),
                    name: profile.map(|p| p.user.username.clone()).unwrap_or_default(),
                };
                page.start_sending(ctx, notices, draft);
            }
        });
    });
}
