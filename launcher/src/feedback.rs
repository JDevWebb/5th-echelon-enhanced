//! "How did that go?": after the game closes, the launcher looks at what happened (the
//! client's log and what the server saw, `setup::feedback`) and, when something went wrong
//! or now and then, asks the player. Their answer goes to the server they play on with their
//! logs if they agree, names, folders and addresses on their PC hidden; the network's admins
//! read it in the admin UI's Reports.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use setup::config::Config;
use setup::config::Profile;
use setup::feedback::Attachment;

use crate::activity::Action;
use crate::app::Notices;
use crate::app::Prefs;
use crate::task::Slot;
use crate::theme;

/// The problems a player can tick: what the server takes, and how they read.
const PROBLEMS: [(&str, &str); 7] = [
    ("join", "Couldn't join a match or a friend"),
    ("lag", "Lag, or things out of sync"),
    ("connection", "Lost the connection"),
    ("crash", "The game crashed or froze"),
    ("version", "\"Different version of the game\""),
    ("signin", "Couldn't sign in"),
    ("other", "Something else"),
];

/// What made the launcher ask, as one line for the player.
fn why(triggers: &[&'static str]) -> &'static str {
    match triggers.first().copied() {
        Some("panic" | "exit_code") => "The game seems to have crashed.",
        Some("signin_refused") => "The server didn't let the game sign in.",
        Some("version_mismatch") => "A join was refused as \"a different version of the game\".",
        Some("failed_join") => "A join didn't work.",
        Some("relayed") => "Your matches went through the server's relay.",
        Some("manual") => "Tell the server's admins what happened.",
        _ => "Help us make the community servers better.",
    }
}

/// The problems to tick at first, from what went wrong.
fn ticked(triggers: &[&'static str]) -> BTreeSet<&'static str> {
    triggers
        .iter()
        .filter_map(|t| match *t {
            "panic" | "exit_code" => Some("crash"),
            "signin_refused" => Some("signin"),
            "version_mismatch" => Some("version"),
            "failed_join" => Some("join"),
            _ => None,
        })
        .collect()
}

/// An open "How did that go?".
#[derive(Clone)]
pub struct Ask {
    triggers: Vec<&'static str>,
    profile: Profile,
    files: Vec<Attachment>,
    /// 👍 or 👎, if picked.
    good: Option<bool>,
    problems: BTreeSet<&'static str>,
    comment: String,
    attach: bool,
    /// The file shown in the preview.
    viewing: Option<usize>,
}

#[derive(Default)]
pub struct Feedback {
    checking: Slot<Result<Option<Ask>, String>>,
    asking: Option<Ask>,
    sending: Slot<Result<String, String>>,
    /// The report being sent, with its activity: kept to send again.
    sent: Option<(crate::activity::Handle, Ask)>,
    /// A report that couldn't be sent, by its activity, for Try again and Without logs.
    failed: Option<(u64, Ask)>,
    /// The report left unsent last time, being sent (see [`pending`]).
    resumed: bool,
    resending: Slot<Result<String, String>>,
    resent: Option<(crate::activity::Handle, i64)>,
    /// The player's reports with the admins' replies (`GET /v1/reports/mine`): read once a
    /// run, and again from Settings › Feedback.
    reading_mine: Slot<Result<Vec<setup::feedback::MyReport>, String>>,
    pub(crate) mine: Option<Result<Vec<setup::feedback::MyReport>, String>>,
    asked_mine: bool,
    /// The player closed the launcher while a report was sending: asking whether to wait
    /// (`Some(true)`), or waiting to close once it's sent (`Some(false)`).
    closing: Option<bool>,
}

impl crate::task::FromPanic for Option<Ask> {
    fn from_panic(_: String) -> Self {
        None
    }
}

/// The game folder's current server account, if it has one.
fn current_profile(game_dir: &std::path::Path) -> Option<Profile> {
    Config::load(game_dir).current_profile().cloned().filter(Profile::has_account)
}

/// Looks at a game that just closed and builds the ask, if there should be one.
fn check(game_dir: PathBuf, exit_code: Option<i32>, checks: String, manual: bool) -> Result<Option<Ask>, String> {
    let profile = current_profile(&game_dir).ok_or("Connect to a server first.")?;
    let log = std::fs::read(game_dir.join("bl-tracing.log"))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let signals = setup::feedback::read_log(&log);
    let password = profile.user.secret();
    let server = password.as_deref().and_then(|password| {
        crate::services::rt().block_on(async {
            tokio::time::timeout(
                Duration::from_secs(8),
                crate::network::session_summary(profile.api_server_url().to_string(), &profile.user.username, password),
            )
            .await
            .ok()
            .and_then(Result::ok)
        })
    });
    let (asked, off) = Prefs::feedback();
    let now = identity::now();
    let saw = server.as_ref().map(|s| setup::feedback::ServerSaw {
        failed_joins: s.failed_joins,
        version_mismatches: s.version_mismatches,
        relayed: s.relayed,
    });
    let triggers = if manual {
        vec!["manual"]
    } else {
        let t = setup::feedback::triggers(signals, saw, exit_code, asked.routine_due(now));
        if off || !asked.may_ask(&t, now) {
            return Ok(None);
        }
        t
    };
    if !manual {
        let mut asked = asked;
        asked.note(&triggers, now);
        Prefs::set_feedback_asked(asked);
    }
    tracing::info!("Asking how the game went ({})", triggers.join(", "));
    let keep: Vec<std::net::Ipv4Addr> = setup::net::resolve(&profile.server)
        .into_iter()
        .filter_map(|ip| match ip {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(_) => None,
        })
        .collect();
    let private = setup::feedback::Private::of_this_pc(keep);
    let mut extra = vec![("checklist.txt", checks)];
    if let Some(s) = &server {
        extra.push(("server-summary.json", s.json.clone()));
    }
    let files = setup::feedback::prepare(&game_dir, crate::logging::log_path().as_deref(), &extra, &private);
    Ok(Some(Ask {
        problems: ticked(&triggers),
        triggers,
        profile,
        files,
        good: None,
        comment: String::new(),
        attach: true,
        viewing: None,
    }))
}

/// The server's name for the bar: its name in the directory, else its address.
fn server_shown(profile: &Profile) -> String {
    if profile.name.is_empty() || profile.name == profile.server {
        profile.server.clone()
    } else {
        profile.name.clone()
    }
}

/// Sends the player's answer to their server, reporting the bytes sent.
fn send(ask: &Ask, activity: &crate::activity::Handle) -> Result<String, String> {
    let password = ask
        .profile
        .user
        .secret()
        .ok_or("The saved password can't be read here; press Connect on the server again.")?;
    send_report_to(&ask.profile.api_server_url().to_string(), &ask.profile.user.username, &password, report_of(ask), activity)
}

/// The report `ask` makes.
fn report_of(ask: &Ask) -> server_api::misc::ReportRequest {
    let client = [
        ("launcher".to_string(), env!("FE_RELEASE").to_string()),
        ("os".to_string(), std::env::consts::OS.to_string()),
    ];
    server_api::misc::ReportRequest {
        rating: match ask.good {
            Some(true) => "good".into(),
            Some(false) => "bad".into(),
            None => String::new(),
        },
        problems: ask.problems.iter().map(|p| (*p).to_string()).collect(),
        comment: ask.comment.trim().to_string(),
        triggers: ask.triggers.iter().map(|t| (*t).to_string()).collect(),
        client: client.into_iter().collect(),
        files: if ask.attach {
            ask.files
                .iter()
                .map(|f| server_api::misc::ReportFile {
                    name: f.name.clone(),
                    gzip: f.gzip.clone(),
                    size: f.size,
                })
                .collect()
        } else {
            vec![]
        },
    }
}

/// Sends `report` to `server` as `username`, reporting the bytes sent.
fn send_report_to(server: &str, username: &str, password: &str, report: server_api::misc::ReportRequest, activity: &crate::activity::Handle) -> Result<String, String> {
    let total = crate::network::report_size(&report);
    activity.progress(crate::activity::Progress::Bytes { sent: 0, total });
    let progress = activity.clone();
    let on_sent = move |sent: u64| progress.progress(crate::activity::Progress::Bytes { sent: sent.min(total), total });
    crate::services::rt()
        .block_on(async {
            // A few MB of logs can take minutes from far away.
            tokio::time::timeout(
                Duration::from_secs(240),
                crate::network::send_report(server.to_string(), username, password, report, on_sent),
            )
            .await
        })
        .map_err(|_| "The server didn't answer in time.".to_string())?
        .map_err(|e| match e {
            crate::network::Error::Rpc(status) if status.code() == tonic::Code::Unimplemented => "This server doesn't take reports yet.".to_string(),
            // A proxy in front of the server gave up on the upload (servers set up before
            // 0.4.2 waited 30 s for it).
            crate::network::Error::Rpc(status) if status.code() == tonic::Code::Unavailable || status.code() == tonic::Code::Unknown => {
                format!("The server couldn't take your report ({}). Try again, or send it without your logs.", status.message())
            }
            crate::network::Error::Rpc(status) => status.message().to_string(),
            e => e.to_string(),
        })
}

/// A report that was sending when the launcher closed (or couldn't be sent): kept in the
/// launcher's folder and sent when it next starts. PlaySkill's report (eu1, 2026-10-06) never
/// arrived: the upload runs after **Send**, and closing the launcher before it finished
/// dropped it without a word. The server and account it's for, never the password (the
/// saved profile's is used).
mod pending {
    use std::path::PathBuf;

    use prost::Message as _;
    use serde::Deserialize;
    use serde::Serialize;

    /// Older than this, it's let go: the session it's about is long past.
    const KEEP_FOR: i64 = 7 * 86_400;

    #[derive(Serialize, Deserialize)]
    pub struct Header {
        pub server: String,
        pub username: String,
        pub saved_at: i64,
        /// Sends tried at start: after [`TRIES`] it's let go (a report the server refuses
        /// would otherwise come back every start for a week).
        #[serde(default)]
        pub tries: u32,
    }

    const TRIES: u32 = 3;

    fn paths() -> Option<(PathBuf, PathBuf)> {
        let dir = setup::app_data_dir()?;
        Some((dir.join("unsent-report.json"), dir.join("unsent-report.bin")))
    }

    pub fn save(server: &str, username: &str, report: &server_api::misc::ReportRequest) {
        let Some((head, body)) = paths() else { return };
        let header = Header {
            server: server.to_string(),
            username: username.to_string(),
            saved_at: identity::now(),
            tries: 0,
        };
        let saved = head.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
            std::fs::write(&body, report.encode_to_vec())?;
            std::fs::write(&head, serde_json::to_vec(&header).unwrap_or_default())
        });
        if let Err(e) = saved {
            tracing::warn!("Couldn't keep the report to send later: {e}");
        }
    }

    pub fn clear() {
        if let Some((head, body)) = paths() {
            let _ = std::fs::remove_file(head);
            let _ = std::fs::remove_file(body);
        }
    }

    /// [`clear`], only if the report kept is still the one saved at `saved_at` (a report sent
    /// since has replaced it).
    pub fn clear_if(saved_at: i64) {
        let still = paths()
            .and_then(|(head, _)| std::fs::read(head).ok())
            .and_then(|h| serde_json::from_slice::<Header>(&h).ok())
            .is_some_and(|h| h.saved_at == saved_at);
        if still {
            clear();
        }
    }

    /// Notes a try at sending the report kept; whether it may be tried (else it's let go).
    pub fn try_again(mut header: Header) -> Option<Header> {
        if header.tries >= TRIES {
            clear();
            return None;
        }
        header.tries += 1;
        let (head, _) = paths()?;
        let _ = std::fs::write(head, serde_json::to_vec(&header).unwrap_or_default());
        Some(header)
    }

    /// The report left unsent, if there's one still worth sending.
    pub fn load() -> Option<(Header, server_api::misc::ReportRequest)> {
        let (head, body) = paths()?;
        let header: Header = serde_json::from_slice(&std::fs::read(head).ok()?).ok()?;
        let report = std::fs::read(body).ok().and_then(|b| server_api::misc::ReportRequest::decode(b.as_slice()).ok());
        match report {
            Some(report) if identity::now() - header.saved_at < KEEP_FOR => Some((header, report)),
            _ => {
                clear();
                None
            }
        }
    }
}

/// The player's reports, from the coordinator the launcher uses (the community network's
/// unless they set another), signed with their identity.
fn fetch_mine() -> Result<Vec<setup::feedback::MyReport>, String> {
    let identity = crate::roadmap::load_identity()?;
    let coordinator = Prefs::directory().unwrap_or_else(|| setup::directory::COMMUNITY.to_string());
    let url = setup::feedback::my_reports_url(&coordinator, &identity, identity::now());
    crate::services::rt().block_on(async {
        let resp = crate::roadmap::client()?
            .get(url)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(crate::roadmap::unreachable)?;
        let body = crate::roadmap::body(resp).await?;
        setup::feedback::parse_my_reports(&body).map_err(|_| "The server sent an answer the launcher can't read.".to_string())
    })
}

impl Feedback {
    /// Reads the player's reports and the admins' replies (in the background).
    pub fn read_mine(&mut self, ctx: &egui::Context) {
        if !self.reading_mine.running() && crate::roadmap::has_identity() {
            self.reading_mine.start(ctx, fetch_mine);
        }
    }

    /// At start: sends the report left unsent last time, if the account it's for is still
    /// set up here (its saved password signs in); else lets it go.
    pub fn resume(&mut self, ctx: &egui::Context, notices: &mut Notices, cfg: &Config) {
        if self.resumed {
            return;
        }
        self.resumed = true;
        let Some((header, report)) = pending::load() else { return };
        let Some(header) = pending::try_again(header) else {
            tracing::info!("A report left unsent was tried {} times; let go", 3);
            return;
        };
        let password = cfg
            .profiles
            .iter()
            .find(|p| p.api_server_url().as_str() == header.server && p.user.username.eq_ignore_ascii_case(&header.username))
            .and_then(|p| p.user.secret());
        let Some(password) = password else {
            tracing::info!("A report left unsent is for an account no longer here; let go");
            pending::clear();
            return;
        };
        let activity = notices.start(ctx, "Sending the report you left unsent");
        let handle = activity.clone();
        let saved_at = header.saved_at;
        self.resending
            .start(ctx, move || send_report_to(&header.server, &header.username, &password, report, &handle));
        self.resent = Some((activity, saved_at));
    }

    /// The game closed: looks at what happened, in the background.
    pub fn game_closed(&mut self, ctx: &egui::Context, game_dir: PathBuf, exit_code: Option<i32>, checks: String) {
        if self.asking.is_some() || self.checking.running() || Prefs::feedback().1 {
            return;
        }
        self.checking.start(ctx, move || check(game_dir, exit_code, checks, false));
    }

    /// The player asked to send feedback (Settings).
    pub fn ask_now(&mut self, ctx: &egui::Context, game_dir: PathBuf, checks: String) {
        if self.asking.is_none() && !self.checking.running() {
            self.checking.start(ctx, move || check(game_dir, None, checks, true));
        }
    }

    pub fn busy(&self) -> bool {
        self.checking.running() || self.sending.running()
    }

    /// Closing the launcher while a report is sending: wait for it, or close and send it
    /// next time (it's kept either way).
    fn ask_before_closing(&mut self, ctx: &egui::Context) {
        let sending = self.sending.running() || self.resending.running();
        if sending && self.closing.is_none() && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing = Some(true);
        }
        let Some(asking) = self.closing else { return };
        if !sending {
            // Sent meanwhile: close as asked.
            self.closing = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !asking {
            ctx.request_repaint_after(Duration::from_millis(500));
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("closing-while-sending")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading("Your report is still sending");
            ui.label("Wait and the launcher closes as soon as it's sent, or close now and it's sent the next time you start the launcher.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(theme::primary("Wait, then close")).clicked() {
                    return Some(false);
                }
                ui.button("Close now, send it next time").clicked().then_some(true)
            })
            .inner
        });
        match modal.inner {
            Some(true) => {
                self.closing = None;
                // The report is kept on disk; let the window go.
                self.sending = Slot::default();
                self.resending = Slot::default();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(false) => self.closing = Some(false),
            None => {}
        }
    }

    /// Sends `ask` in the background, in the activity bar.
    fn start_sending(&mut self, ctx: &egui::Context, notices: &mut Notices, ask: Ask) {
        let files = if ask.attach { ask.files.len() } else { 0 };
        let activity = notices.start(ctx, format!("Sending your report to {}", server_shown(&ask.profile)));
        match files {
            0 => activity.step("without logs"),
            1 => activity.step("with 1 log file"),
            n => activity.step(format!("with {n} log files")),
        }
        // Kept until the server has it: closing the launcher meanwhile loses nothing.
        pending::save(&ask.profile.api_server_url().to_string(), &ask.profile.user.username, &report_of(&ask));
        let handle = activity.clone();
        let sending = ask.clone();
        self.sending.start(ctx, move || send(&sending, &handle));
        self.sent = Some((activity, ask));
    }

    /// Shows the ask, when there is one, and what came of sending.
    pub fn show(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        match self.checking.poll() {
            Some(Ok(Some(ask))) => self.asking = Some(ask),
            Some(Err(e)) => notices.error(e),
            _ => {}
        }
        if let Some(result) = self.sending.poll() {
            if let Some((activity, ask)) = self.sent.take() {
                match result {
                    Ok(_) => {
                        pending::clear();
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
                        self.failed = Some((activity.id(), ask));
                    }
                }
            }
        }
        if let Some(result) = self.resending.poll() {
            if let Some((activity, saved_at)) = self.resent.take() {
                match result {
                    Ok(_) => {
                        pending::clear_if(saved_at);
                        activity.done("Your report from last time is with the server's admins.");
                    }
                    // Kept: it goes again next time (three tries, for a week at most).
                    Err(e) => activity.fail(format!("{e} It'll be tried again next time."), &[crate::activity::Action::CopyDetails]),
                }
            }
        }
        self.ask_before_closing(ctx);
        // Once a run: the admins' replies, and a notice for any new since the player last saw.
        if !self.asked_mine {
            self.asked_mine = true;
            self.read_mine(ctx);
        }
        if let Some(result) = self.reading_mine.poll() {
            if let Ok(mine) = &result {
                let seen = Prefs::replies_seen();
                let newest = mine.iter().filter(|r| !r.reply.is_empty()).max_by_key(|r| r.replied_at.unwrap_or(0));
                if let Some(r) = newest.filter(|r| seen.is_none_or(|s| r.replied_at.unwrap_or(0) > s)) {
                    notices.info(format!(
                        "The admins of {} replied to your report: \"{}\" (Settings › Feedback)",
                        if r.server.is_empty() { "your server" } else { &r.server },
                        r.reply.lines().next().unwrap_or_default()
                    ));
                    Prefs::set_replies_seen(r.replied_at.unwrap_or(0));
                }
            }
            self.mine = Some(result);
        }
        if let Some((id, ask)) = self.failed.clone() {
            match notices.take_action(id) {
                Some(Action::Retry) => {
                    self.failed = None;
                    self.start_sending(ctx, notices, ask);
                }
                Some(Action::WithoutLogs) => {
                    self.failed = None;
                    self.start_sending(ctx, notices, Ask { attach: false, ..ask });
                }
                _ => {}
            }
        }
        let Some(ask) = self.asking.as_mut() else { return };
        let mut close = false;
        let mut send_now = false;
        // The window's height, in the points the launcher lays out in: the middle scrolls
        // so the buttons always fit, however the window is sized or scaled.
        let middle = (ctx.content_rect().height() - 230.0).max(120.0);
        let modal = egui::Modal::new(egui::Id::new("feedback")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.label(theme::heading("How did that go?"));
            ui.label(theme::muted(why(&ask.triggers)));
            ui.add_space(8.0);
            // A scroll bar beside the content, not over the comment box.
            ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
            egui::ScrollArea::vertical()
                .id_salt("feedback-middle")
                .max_height(middle)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(ask.good == Some(true), "👍  Good").clicked() {
                            ask.good = Some(true);
                        }
                        if ui.selectable_label(ask.good == Some(false), "👎  Not great").clicked() {
                            ask.good = Some(false);
                        }
                    });
                    ui.add_space(6.0);
                    ui.label("What went wrong? (tick any)");
                    for (id, label) in PROBLEMS {
                        let mut on = ask.problems.contains(id);
                        if ui.checkbox(&mut on, label).changed() {
                            if on {
                                ask.problems.insert(id);
                            } else {
                                ask.problems.remove(id);
                            }
                        }
                    }
                    ui.add_space(6.0);
                    ui.label("Anything else? (who you played with, what you were doing)");
                    ui.add(egui::TextEdit::multiline(&mut ask.comment).desired_rows(3).desired_width(f32::INFINITY).char_limit(2000));
                    ui.add_space(6.0);
                    ui.checkbox(&mut ask.attach, "Send my logs too");
                    ui.label(theme::muted("Your PC's name, your user folder and your internet address are hidden before they're sent."));
                    if ask.attach && !ask.files.is_empty() {
                        egui::CollapsingHeader::new("What's sent").show(ui, |ui| {
                            for (i, f) in ask.files.iter().enumerate() {
                                let label = format!("{}  ({} KB)", f.name, f.size.div_ceil(1024));
                                if ui.selectable_label(ask.viewing == Some(i), label).clicked() {
                                    ask.viewing = if ask.viewing == Some(i) { None } else { Some(i) };
                                }
                            }
                            if let Some(f) = ask.viewing.and_then(|i| ask.files.get(i)) {
                                egui::ScrollArea::both().max_height((middle * 0.5).min(220.0)).id_salt("feedback-file").show(ui, |ui| {
                                    // The last part: where the trouble usually is, and light to show.
                                    let start = f.text.len().saturating_sub(64 * 1024);
                                    let start = (start..f.text.len()).find(|i| f.text.is_char_boundary(*i)).unwrap_or(0);
                                    ui.label(egui::RichText::new(&f.text[start..]).monospace().size(11.0));
                                });
                            }
                        });
                    }
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let can_send = ask.good.is_some() || !ask.problems.is_empty() || !ask.comment.trim().is_empty();
                if ui.add_enabled(can_send, theme::primary("Send")).clicked() {
                    send_now = true;
                }
                if ui.add(theme::secondary("Not now")).clicked() {
                    close = true;
                }
                if !ask.triggers.contains(&"manual") && ui.add(theme::secondary("Don't ask again")).on_hover_text("Settings › Feedback asks again").clicked() {
                    Prefs::set_feedback_off(true);
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if send_now {
            // The window closes; the upload carries on in the activity bar.
            if let Some(ask) = self.asking.take() {
                self.start_sending(ctx, notices, ask);
            }
        } else if close {
            self.asking = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_went_wrong_is_ticked_for_the_player() {
        assert_eq!(ticked(&["version_mismatch", "failed_join", "relayed"]), BTreeSet::from(["version", "join"]));
        assert!(ticked(&["routine"]).is_empty());
        assert_eq!(why(&["exit_code"]), "The game seems to have crashed.");
        assert!(PROBLEMS
            .iter()
            .all(|(id, _)| ["join", "lag", "crash", "connection", "version", "signin", "other"].contains(id)));
    }
}
