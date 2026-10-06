//! The game's diagnostics for the server's admins: its warnings and errors, and what the
//! network code says (the NAT helper, the router's port mapping), sent to the server it's
//! signed in to every few seconds (Misc.ClientLog). They show on the admin UI's Sessions
//! page next to what the server saw, so a join that fails can be followed from both ends.
//!
//! Private names, home folders and the player's public address are hidden first (as in the
//! launcher's reports). Settings › Feedback in the launcher turns it off
//! (`send_diagnostics`). At most [`MAX_QUEUED`] lines wait; past that they're counted.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

use hooks_config::redact::Private;
use server_api::misc::ClientLogLine;
use tracing::field::Field;
use tracing::field::Visit;
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Lines waiting at most, sent at most in one request, and how often.
const MAX_QUEUED: usize = 200;
const BATCH: usize = 50;
const EVERY: Duration = Duration::from_secs(5);
/// After a failed send (the server out of reach).
const RETRY: Duration = Duration::from_secs(30);
/// With the player's agreement to send the whole log, the game checks in this often even
/// with nothing to say, so the server can ask for it.
const CHECK_IN: Duration = Duration::from_secs(30);
/// The most of the game's log sent when the server asks (its end kept), before gzip.
const MAX_FULL_LOG: usize = 3 * 1024 * 1024;
/// The most of one line sent.
const MAX_LINE: usize = 300;
struct Queue {
    lines: VecDeque<ClientLogLine>,
    dropped: u32,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue {
    lines: VecDeque::new(),
    dropped: 0,
});
/// Set when the player turned it off or the server doesn't take it: nothing is kept.
static OFF: AtomicBool = AtomicBool::new(false);

/// The tracing layer that picks the lines out (installed with the log, lib.rs).
pub struct DiagnosticsLayer;

impl<S: Subscriber> Layer<S> for DiagnosticsLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if OFF.load(Ordering::Relaxed) {
            return;
        }
        let meta = event.metadata();
        let mut text = Text::default();
        event.record(&mut text);
        if !hooks_config::diagnostics::wanted(meta.level().as_str(), meta.target(), &text.0) {
            return;
        }
        let line = ClientLogLine {
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0)),
            level: meta.level().to_string(),
            target: meta.target().to_string(),
            message: text.0.chars().take(MAX_LINE).collect(),
        };
        let mut q = QUEUE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.lines.len() >= MAX_QUEUED {
            q.dropped = q.dropped.saturating_add(1);
            return;
        }
        q.lines.push_back(line);
    }
}

/// An event's message, then its other fields as `name=value`.
#[derive(Default)]
struct Text(String);

impl Visit for Text {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}{}", self.0);
        } else {
            self.0.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}

fn stop() {
    OFF.store(true, Ordering::Relaxed);
    let mut q = QUEUE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    q.lines.clear();
    q.dropped = 0;
}

/// Starts sending (once the config is loaded): or stops keeping lines, when the player
/// turned it off.
pub fn start(config: &hooks_config::Config) {
    if !config.send_diagnostics {
        stop();
        return;
    }
    let Ok(rt) = crate::api::runtime() else { return };
    let server = config.api_server.clone();
    // The same agreement covers the game's log when something goes wrong (the launcher's
    // question says so): the server may ask for it.
    let full_logs = true;
    rt.spawn(async move {
        // The server's own addresses aren't the player's: kept, the rest hidden.
        let mut keep = Vec::new();
        if let (Some(host), Some(port)) = (server.host_str(), server.port_or_known_default()) {
            if let Ok(addrs) = tokio::net::lookup_host((host, port)).await {
                keep.extend(addrs.filter_map(|a| match a.ip() {
                    std::net::IpAddr::V4(v4) => Some(v4),
                    std::net::IpAddr::V6(_) => None,
                }));
            }
        }
        let private = Private::of_this_pc(keep);
        let mut last_sent = std::time::Instant::now();
        loop {
            tokio::time::sleep(EVERY).await;
            let (batch, dropped) = {
                let q = QUEUE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                (q.lines.iter().take(BATCH).cloned().collect::<Vec<_>>(), q.dropped)
            };
            if batch.is_empty() && dropped == 0 && !(full_logs && last_sent.elapsed() >= CHECK_IN) {
                continue;
            }
            last_sent = std::time::Instant::now();
            let sent = batch.len();
            let lines = batch
                .into_iter()
                .map(|mut l| {
                    l.message = hooks_config::redact::redact(&l.message, &private);
                    l
                })
                .collect();
            match crate::api::client_log(lines, dropped, full_logs).await {
                Ok(answer) => {
                    {
                        let mut q = QUEUE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        let n = sent.min(q.lines.len());
                        q.lines.drain(..n);
                        q.dropped = q.dropped.saturating_sub(dropped);
                    }
                    // Something went wrong and the player agreed: the log it asks for.
                    if full_logs && answer.send_log_since > 0 {
                        let private = private.clone();
                        tokio::spawn(send_full_log(answer.send_log_since, answer.send_log_problem, private));
                    }
                }
                // A server from before this: nothing to send to.
                Err(crate::api::Error::GRPCStatus(s)) if s.code() == tonic::Code::Unimplemented => {
                    stop();
                    return;
                }
                // Not signed in yet, or the server out of reach: they wait.
                Err(_) => tokio::time::sleep(RETRY).await,
            }
        }
    });
}

/// How far back the game's log is ever sent from (the server asks for 15 minutes).
const FULL_LOG_SPAN: i64 = 20 * 60;
/// The most of a log file read (its end): the log can grow large (packet logging), and the
/// game is a 32-bit process.
const MAX_LOG_READ: u64 = 16 * 1024 * 1024;

/// The end of a log file, at most [`MAX_LOG_READ`], from its first whole line.
fn tail(path: &std::path::Path) -> Option<String> {
    use std::io::Read as _;
    use std::io::Seek as _;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(MAX_LOG_READ);
    f.seek(std::io::SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    f.take(MAX_LOG_READ).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(if start > 0 {
        text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string())
    } else {
        text
    })
}

/// The game's log from `since`: this run's, and the run before's (`bl-tracing.prev.log`)
/// when this one started after `since` (the game restarted, or crashed, since the problem).
fn log_text(path: &std::path::Path, since: i64) -> Option<String> {
    let now = tail(path)?;
    let first = now.lines().find_map(hooks_config::diagnostics::line_time);
    let before = if first.is_some_and(|t| t > since) {
        tail(&path.with_file_name("bl-tracing.prev.log"))
    } else {
        None
    };
    let mut text = String::new();
    if let Some(before) = before {
        text.push_str(&hooks_config::diagnostics::log_since(&before, since));
        text.push_str("(the game started again here)\n");
    }
    text.push_str(&hooks_config::diagnostics::log_since(&now, since));
    Some(text)
}

/// Sends the game's log from `since` (Unix seconds) as the report the server asked for:
/// without the call tracing, redacted, its end kept within [`MAX_FULL_LOG`].
async fn send_full_log(since: i64, problem: String, private: Private) {
    use std::io::Write as _;
    let Some(path) = crate::LOG_PATH.get().cloned() else { return };
    // Never more than the 15 minutes or so before the problem, whatever the server says.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let since = since.clamp(now - FULL_LOG_SPAN, now);
    // Reading and filtering a large log is work for a blocking thread, not the API's.
    let built = tokio::task::spawn_blocking(move || {
        let text = log_text(&path, since)?;
        let text = hooks_config::redact::redact(&hooks_config::diagnostics::without_call_tracing(&text), &private);
        Some(text)
    })
    .await;
    let Ok(Some(mut text)) = built else {
        tracing::warn!("The server asked for the game's log, which can't be read");
        return;
    };
    if text.len() > MAX_FULL_LOG {
        let mut start = text.len() - MAX_FULL_LOG;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text = format!("(the first {start} bytes left out)\n{}", &text[start..]);
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    if gz.write_all(text.as_bytes()).is_err() {
        return;
    }
    let Ok(gzip) = gz.finish() else { return };
    let problem: String = problem.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(30).collect();
    let report = server_api::misc::ReportRequest {
        rating: String::new(),
        problems: vec![],
        comment: String::new(),
        triggers: vec!["auto".into(), format!("auto:{problem}")],
        client: [("client".to_string(), concat!("game/", env!("FE_RELEASE")).to_string())].into_iter().collect(),
        files: vec![server_api::misc::ReportFile {
            name: "bl-tracing.log".into(),
            size: text.len() as u64,
            gzip,
        }],
    };
    match crate::api::report(report).await {
        Ok(id) => tracing::info!("Sent the game's log as the server asked ({problem}): report {id}"),
        Err(e) => tracing::warn!("Couldn't send the game's log the server asked for: {e}"),
    }
}
