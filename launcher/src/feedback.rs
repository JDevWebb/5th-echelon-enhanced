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
    let client = [
        ("launcher".to_string(), env!("FE_RELEASE").to_string()),
        ("os".to_string(), std::env::consts::OS.to_string()),
    ];
    let report = server_api::misc::ReportRequest {
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
    };
    let total = crate::network::report_size(&report);
    activity.progress(crate::activity::Progress::Bytes { sent: 0, total });
    let progress = activity.clone();
    let on_sent = move |sent: u64| progress.progress(crate::activity::Progress::Bytes { sent: sent.min(total), total });
    crate::services::rt()
        .block_on(async {
            // A few MB of logs can take minutes from far away.
            tokio::time::timeout(
                Duration::from_secs(240),
                crate::network::send_report(ask.profile.api_server_url().to_string(), &ask.profile.user.username, &password, report, on_sent),
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

impl Feedback {
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

    /// Sends `ask` in the background, in the activity bar.
    fn start_sending(&mut self, ctx: &egui::Context, notices: &mut Notices, ask: Ask) {
        let files = if ask.attach { ask.files.len() } else { 0 };
        let activity = notices.start(ctx, format!("Sending your report to {}", server_shown(&ask.profile)));
        match files {
            0 => activity.step("without logs"),
            1 => activity.step("with 1 log file"),
            n => activity.step(format!("with {n} log files")),
        }
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
                    Ok(_) => activity.done("Thanks! Your report is with the server's admins."),
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
