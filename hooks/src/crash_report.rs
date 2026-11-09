//! The last run's crashes and freezes, sent to the server once the game has signed in (the
//! 0.5.0 crash work, item 1). Before this only the players who sent a report from the launcher
//! were seen: 33 of the 186 who played in a week (2026-11-09), and a crash ends the game before
//! anything could be sent then. Now every player's game says, at its next start, what the
//! previous run's log has (`hooks_config::crash_log` reads it; crash.rs wrote it).
//!
//! The same agreement as the game's diagnostics (Settings › Feedback, `send_diagnostics`): off
//! there, nothing is sent. The lines are redacted as the diagnostics are; they hold module
//! names, addresses and what the game was doing, nothing of the player's. A log already sent
//! isn't sent again: its first line's time is kept beside it.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use hooks_config::crash_log;
use hooks_config::redact::Private;
use tracing::info;
use tracing::warn;

/// The most crashes sent from one log (its last ones).
const MOST: usize = 5;
/// How long to wait for the sign-in before giving up for this run.
const WAIT_FOR_SIGN_IN: Duration = Duration::from_secs(15 * 60);
/// The file beside the log that says which log was sent.
const SENT: &str = "bl-tracing.prev.log.sent";
/// The longest the previous log is read (its end).
const MOST_READ: u64 = 16 * 1024 * 1024;

/// Starts it: a thread that waits for the sign-in, then sends the last run's crashes.
pub fn start(config: &hooks_config::Config) {
    if !config.send_diagnostics {
        return;
    }
    let Some(log) = crate::LOG_PATH.get() else { return };
    let prev = log.with_file_name("bl-tracing.prev.log");
    let username = config.user.username.clone();
    let spawned = std::thread::Builder::new().name("crash-report".into()).spawn(move || send(&prev, &username));
    if spawned.is_err() {
        warn!("Crash reports: the thread couldn't start");
    }
}

fn send(prev: &Path, username: &str) {
    let Some(text) = tail(prev) else { return };
    let Some(first) = text.lines().find_map(hooks_config::diagnostics::line_time) else {
        return;
    };
    let marker: PathBuf = prev.with_file_name(SENT);
    if std::fs::read_to_string(&marker).ok().and_then(|s| s.trim().parse::<i64>().ok()) == Some(first) {
        return;
    }
    let crashes = crash_log::crashes(&text);
    if crashes.is_empty() {
        let _ = std::fs::write(&marker, first.to_string());
        return;
    }
    let started = std::time::Instant::now();
    while !crate::api::has_token() {
        if started.elapsed() > WAIT_FOR_SIGN_IN {
            return;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    // The server's address is the server's, not the player's: hidden like the rest only if
    // it's this PC's; the in-game name stays (the server knows it).
    let private = Private::of_this_pc(Vec::new()).showing(username);
    let clean = |s: &str| hooks_config::redact::redact(s, &private);
    let skip = crashes.len().saturating_sub(MOST);
    let mut sent = 0;
    for c in &crashes[skip..] {
        let request = server_api::misc::CrashRequest {
            kind: c.kind.as_str().into(),
            at: c.at,
            code: clean(&c.code),
            place: clean(&c.place),
            stack: clean(&c.stack),
            registers: clean(&c.registers),
            callers: clean(&c.callers),
            doing: clean(&c.doing),
            fatal: c.fatal,
            release: env!("FE_RELEASE").into(),
        };
        match crate::api::crash(request) {
            Ok(()) => sent += 1,
            // A server from before this: it can't take them.
            Err(crate::api::Error::GRPCStatus(s)) if s.code() == tonic::Code::Unimplemented => break,
            Err(e) => {
                warn!("Crash reports: sending the last run's failed: {e}");
                return;
            }
        }
    }
    let _ = std::fs::write(&marker, first.to_string());
    if sent > 0 {
        info!("Crash reports: told the server of the last run's {sent} crash(es) and freeze(s)");
    }
}

/// The end of the file, at most [`MOST_READ`], from its first whole line.
fn tail(path: &Path) -> Option<String> {
    use std::io::Read as _;
    use std::io::Seek as _;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(MOST_READ);
    f.seek(std::io::SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    f.take(MOST_READ).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(if start > 0 {
        text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string())
    } else {
        text
    })
}
