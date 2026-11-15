//! "How did that go?": after the game closes, the launcher looks at what happened (the
//! client's log and what the server saw, `setup::feedback`) and, when something went wrong
//! or now and then, asks the player. Their answer goes to the server they play on with their
//! logs if they agree, names, folders and addresses on their PC hidden; the network's admins
//! read it in the admin UI's Reports.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use setup::config::Config;
use setup::config::Profile;
use setup::feedback::Attachment;
use setup::feedback::GameExit;

use crate::prefs::Prefs;

/// The problems a player can tick: what the server takes, and how they read.
pub const PROBLEMS: [(&str, &str); 7] = [
    ("join", "Couldn't join a match or a friend"),
    ("lag", "Lag, or things out of sync"),
    ("connection", "Lost the connection"),
    ("crash", "The game crashed or froze"),
    ("version", "\"Different version of the game\""),
    ("signin", "Couldn't sign in"),
    ("other", "Something else"),
];

/// What made the launcher ask, as one line for the player.
pub fn why(triggers: &[&'static str]) -> &'static str {
    match triggers.first().copied() {
        Some("panic" | "exit_code") => "The game seems to have crashed.",
        Some("froze") => "The game seems to have frozen.",
        Some("signin_refused") => "The server didn't let the game sign in.",
        Some("version_mismatch") => "A join was refused as \"a different version of the game\".",
        Some("failed_join") => "A join didn't work.",
        Some("search_mismatch") => "Searching found no lobbies, though some were open: their game data differs from yours.",
        Some("relayed") => "Your matches went through the server's relay.",
        Some("manual") => "Tell the server's admins what happened.",
        _ => "Help us make the community servers better.",
    }
}

/// The problems to tick at first, from what went wrong.
pub fn ticked(triggers: &[&'static str]) -> BTreeSet<&'static str> {
    triggers
        .iter()
        .filter_map(|t| match *t {
            "panic" | "exit_code" | "froze" => Some("crash"),
            "signin_refused" => Some("signin"),
            "version_mismatch" => Some("version"),
            "failed_join" | "search_mismatch" => Some("join"),
            _ => None,
        })
        .collect()
}

/// An open "How did that go?".
#[derive(Clone)]
pub struct Ask {
    pub triggers: Vec<&'static str>,
    /// Whose game data differs, after a join refused as a different version, when the
    /// server could tell.
    pub mismatch: Option<String>,
    pub profile: Profile,
    pub files: Vec<Attachment>,
    /// 👍 or 👎, if picked.
    pub good: Option<bool>,
    pub problems: BTreeSet<&'static str>,
    pub comment: String,
    pub attach: bool,
    /// The file shown in the preview.
    pub viewing: Option<usize>,
    /// How the game ended, sent as the report's `exit_code` (and in its checklist.txt).
    pub exit: GameExit,
}

/// The game's log, sent by itself when the game closes while beta features are on (the player
/// was told so where they turned them on: Settings › Beta features). Filed with the logs the
/// server asks for, not as a report of the player's: no rating, no comment.
pub struct BetaLog {
    pub server: String,
    pub username: String,
    pub password: String,
    pub shown: String,
    pub report: server_api::misc::ReportRequest,
}

/// The beta features a game ran with, as its log says at its start: the add-on's own line
/// (`hooks::hooks: <line>: Beta features on: a, b; ...`; the last one, for a log a run was
/// added to), only names of betas there are.
pub fn betas_in_log(log: &str) -> Vec<String> {
    let own = |l: &&str| {
        l.split_once("Beta features on: ").is_some_and(|(before, _)| {
            before
                .trim_end()
                .strip_suffix(':')
                .and_then(|b| b.rsplit_once(" hooks::hooks: "))
                .is_some_and(|(_, line)| !line.is_empty() && line.bytes().all(|c| c.is_ascii_digit()))
        })
    };
    log.lines()
        .rev()
        .find(own)
        .and_then(|l| l.split_once("Beta features on: ").map(|(_, rest)| rest))
        .map(|rest| {
            rest.split(';')
                .next()
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|b| hooks_config::Beta::NAMES.contains(b))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The beta log of the game in `game_dir` that just closed, if there's one to send: beta features
/// on, an account here, and not a run of a few seconds (see [`check`]).
pub fn beta_log(game_dir: &std::path::Path, exit: GameExit, checks: &str) -> Result<Option<BetaLog>, String> {
    let Some(profile) = current_profile(game_dir) else { return Ok(None) };
    let path = game_dir.join("bl-tracing.log");
    let log = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    if setup::game::game_running() || log.is_empty() || hooks_config::diagnostics::short_run(&log, SHORT_RUN_SECS) {
        return Ok(None);
    }
    // The betas both on now (the player's consent stands) and on when the game started, as
    // the add-on's own start-up line says (R5: a game started without them sends nothing);
    // none, nothing to send. The log's line alone never sends it: other text reaches the log.
    let now = Config::load(game_dir).hook_config.beta.on();
    let ran = betas_in_log(&log);
    let on: Vec<&str> = now.into_iter().filter(|b| ran.iter().any(|r| r == b)).collect();
    if on.is_empty() {
        return Ok(None);
    }
    // A log already sent isn't sent again (R5: a game that died before writing its log left
    // the last one, which went once already).
    let stamp = std::fs::metadata(&path).ok().map(|m| {
        (
            m.len(),
            m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()),
        )
    });
    let sent = game_dir.join("bl-tracing.sent");
    let stamp_text = stamp.map(|(len, at)| format!("{at} {len}"));
    if stamp_text.is_some() && std::fs::read_to_string(&sent).ok().as_deref().map(str::trim) == stamp_text.as_deref() {
        return Ok(None);
    }
    if let Some(t) = &stamp_text {
        let _ = std::fs::write(&sent, t);
    }
    let password = profile
        .user
        .secret()
        .ok_or("Beta features are on, but the saved password can't be read here to send the game's log; press Connect on the server again.")?;
    let keep: Vec<std::net::Ipv4Addr> = setup::net::resolve(&profile.server)
        .into_iter()
        .filter_map(|ip| match ip {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(_) => None,
        })
        .collect();
    let private = setup::feedback::Private::of_this_pc(keep).showing(&profile.user.username);
    let checks = if checks.is_empty() {
        exit.checklist_line()
    } else {
        format!("{checks}\n{}", exit.checklist_line())
    };
    let files = setup::feedback::prepare(game_dir, crate::logging::log_path().as_deref(), &[("checklist.txt", checks)], &private);
    let triggers = std::iter::once("beta".to_string()).chain(on.iter().map(|b| format!("beta:{b}"))).collect();
    let client = [
        ("launcher".to_string(), env!("FE_RELEASE").to_string()),
        ("os".to_string(), std::env::consts::OS.to_string()),
        ("exit_code".to_string(), exit.value()),
    ];
    Ok(Some(BetaLog {
        server: profile.api_server_url().to_string(),
        username: profile.user.username.clone(),
        password,
        shown: server_shown(&profile),
        report: server_api::misc::ReportRequest {
            rating: String::new(),
            problems: vec![],
            comment: String::new(),
            triggers,
            client: client.into_iter().collect(),
            files: files
                .iter()
                .map(|f| server_api::misc::ReportFile {
                    name: f.name.clone(),
                    gzip: f.gzip.clone(),
                    size: f.size,
                })
                .collect(),
        },
    }))
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

/// Looks at a game that just closed and builds the ask, if there should be one. `exit`: how
/// it ended; for a report the player asked to send (`manual`), how the last game the launcher
/// saw close ended, unless one is running now.
/// A game that ran less than this long isn't asked about (see [`check`]).
const SHORT_RUN_SECS: i64 = 10;
/// How long after the server's last play session what it saw still counts in asking.
const RECENT_SESSION_SECS: i64 = 2 * 3600;

/// Whether the server's last play session (`started`, `ended`: Unix seconds, 0 for none) is
/// recent enough to be the game that just closed.
fn recent_session(started: i64, ended: i64, now: i64) -> bool {
    started > 0 && now - ended.max(started) < RECENT_SESSION_SECS
}

pub fn check(game_dir: PathBuf, exit: GameExit, checks: String, manual: bool) -> Result<Option<Ask>, String> {
    let profile = current_profile(&game_dir).ok_or("Connect to a server first.")?;
    let exit = if manual && setup::game::game_running() { GameExit::Running } else { exit };
    let log = std::fs::read(game_dir.join("bl-tracing.log"))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    // A run of a few seconds is Steam's or Ubisoft Connect's start-and-close (the game they
    // start right after it is the one played), or a game that has only just started: either
    // way not a game to ask about. The game that follows is asked about when it closes.
    if !manual && (setup::game::game_running() || (!log.is_empty() && hooks_config::diagnostics::short_run(&log, SHORT_RUN_SECS))) {
        tracing::info!("Not asking how the game went: it ran a few seconds, or another started");
        return Ok(None);
    }
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
    // What the server saw counts only for a session that ended a short while ago: the last one
    // it has may be from another day (and was asked about then).
    let saw = server.as_ref().filter(|s| recent_session(s.started, s.ended, now)).map(|s| setup::feedback::ServerSaw {
        failed_joins: s.failed_joins,
        version_mismatches: s.version_mismatches,
        relayed: s.relayed,
        searches_other_data: s.searches_other_data,
        joins_explained: s.joins_explained,
        mismatches_explained: s.mismatches_explained,
        searches_explained: s.searches_explained,
    });
    let triggers = if manual {
        vec!["manual"]
    } else {
        let t = setup::feedback::triggers(signals, saw, exit.code(), asked.routine_due(now));
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
    let private = setup::feedback::Private::of_this_pc(keep).showing(&profile.user.username);
    // Servers before 0.4.4 drop the report's `exit_code`: the checklist carries it too.
    let checks = if checks.is_empty() {
        exit.checklist_line()
    } else {
        format!("{checks}\n{}", exit.checklist_line())
    };
    let mut extra = vec![("checklist.txt", checks)];
    if let Some(s) = &server {
        extra.push(("server-summary.json", s.json.clone()));
    }
    let files = setup::feedback::prepare(&game_dir, crate::logging::log_path().as_deref(), &extra, &private);
    let mismatch = server.as_ref().and_then(|s| {
        if triggers.contains(&"version_mismatch") {
            setup::feedback::mismatch_line(&s.mismatch_host, &s.mismatch_mine, &s.mismatch_theirs)
        } else if triggers.contains(&"search_mismatch") {
            setup::feedback::search_mismatch_line(&s.other_data_mine, &s.other_data_theirs)
        } else {
            None
        }
    });
    Ok(Some(Ask {
        mismatch,
        problems: ticked(&triggers),
        triggers,
        profile,
        files,
        good: None,
        comment: String::new(),
        attach: true,
        viewing: None,
        exit,
    }))
}

/// The server's name for the bar: its name in the directory, else its address.
pub fn server_shown(profile: &Profile) -> String {
    if profile.name.is_empty() || profile.name == profile.server {
        profile.server.clone()
    } else {
        profile.name.clone()
    }
}

/// Sends the player's answer to their server, reporting the bytes sent.
pub fn send(ask: &Ask, activity: &crate::activity::Handle) -> Result<String, String> {
    let password = ask
        .profile
        .user
        .secret()
        .ok_or("The saved password can't be read here; press Connect on the server again.")?;
    send_report_to(&ask.profile.api_server_url().to_string(), &ask.profile.user.username, &password, report_of(ask), activity)
}

/// The report `ask` makes.
pub fn report_of(ask: &Ask) -> server_api::misc::ReportRequest {
    let client = [
        ("launcher".to_string(), env!("FE_RELEASE").to_string()),
        ("os".to_string(), std::env::consts::OS.to_string()),
        ("exit_code".to_string(), ask.exit.value()),
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
pub fn send_report_to(server: &str, username: &str, password: &str, report: server_api::misc::ReportRequest, activity: &crate::activity::Handle) -> Result<String, String> {
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
pub mod pending {
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
        // The report holds the game's log and the player's words: only the user may read it.
        let saved = head.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
            setup::write_private(&body, &report.encode_to_vec())?;
            setup::write_private(&head, &serde_json::to_vec(&header).unwrap_or_default())
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
        let _ = setup::write_private(&head, &serde_json::to_vec(&header).unwrap_or_default());
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
pub fn fetch_mine() -> Result<Vec<setup::feedback::MyReport>, String> {
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

#[cfg(test)]
mod tests {

    #[test]
    fn only_a_recent_session_counts_in_asking() {
        let now = 1_792_000_000;
        assert!(recent_session(now - 3600, now - 60, now), "ended a minute ago");
        assert!(recent_session(now - 600, 0, now), "still going");
        assert!(!recent_session(now - 30 * 3600, now - 26 * 3600, now), "yesterday's");
        assert!(!recent_session(0, 0, now), "none");
    }

    use super::*;

    #[test]
    fn what_went_wrong_is_ticked_for_the_player() {
        assert_eq!(ticked(&["version_mismatch", "failed_join", "relayed"]), BTreeSet::from(["version", "join"]));
        assert_eq!(ticked(&["search_mismatch"]), BTreeSet::from(["join"]));
        assert!(ticked(&["routine"]).is_empty());
        assert_eq!(why(&["exit_code"]), "The game seems to have crashed.");
        assert!(PROBLEMS
            .iter()
            .all(|(id, _)| ["join", "lag", "crash", "connection", "version", "signin", "other"].contains(id)));
    }

    /// A report says how the game ended, so a crash, a kill and a quit can be told apart.
    #[test]
    fn reports_carry_the_exit_code() {
        let ask = |exit| Ask {
            triggers: vec!["exit_code"],
            profile: Profile::default(),
            files: vec![],
            good: None,
            problems: BTreeSet::new(),
            comment: String::new(),
            attach: true,
            viewing: None,
            mismatch: None,
            exit,
        };
        #[allow(clippy::cast_possible_wrap)]
        let crashed = report_of(&ask(GameExit::Code(0xC000_0005_u32 as i32)));
        assert_eq!(crashed.client.get("exit_code").map(String::as_str), Some("0xC0000005"));
        let started_elsewhere = report_of(&ask(GameExit::Unknown));
        assert_eq!(started_elsewhere.client.get("exit_code").map(String::as_str), Some("unknown"));
    }

    #[test]
    fn a_logs_betas_are_the_ones_it_started_with() {
        let line = |rest: &str| format!("2026-10-31T01:00:00.000Z  INFO ThreadId(01) hooks::hooks: 796: Beta features on: {rest}; the launcher sends this log");
        assert_eq!(
            betas_in_log(&format!("{}\nmore", line("mouse_turn_per_second, made_up"))),
            ["mouse_turn_per_second"],
            "only betas there are"
        );
        assert_eq!(
            betas_in_log(&line("mouse_turn_per_second, host_migration")),
            ["mouse_turn_per_second"],
            "a 0.5.0 test build's host_migration beta isn't a beta any more"
        );
        assert!(betas_in_log("no betas here").is_empty());
        // Text from elsewhere in the log (a chat line, say) doesn't count.
        assert!(betas_in_log("2026-10-31T01:00:00Z  INFO hooks::chat: 12: Kiwi: Beta features on: mouse_turn_per_second; x").is_empty());
        assert!(betas_in_log("Beta features on: mouse_turn_per_second; x").is_empty());
        // A log a second run was added to: the later run's.
        let two = format!("{}\n{}", line("mouse_turn_per_second"), line(""));
        assert!(betas_in_log(&two).is_empty());
    }
}
