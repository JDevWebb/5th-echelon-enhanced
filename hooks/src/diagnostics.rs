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
use tracing::Level;
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Lines waiting at most, sent at most in one request, and how often.
const MAX_QUEUED: usize = 200;
const BATCH: usize = 50;
const EVERY: Duration = Duration::from_secs(5);
/// After a failed send (the server out of reach).
const RETRY: Duration = Duration::from_secs(30);
/// The most of one line sent.
const MAX_LINE: usize = 300;
/// Modules whose ordinary (INFO) lines go too: the network code.
const NETWORK: &[&str] = &["hooks::hooks::nat", "hooks::hooks::portmap", "hooks::hooks::nla"];

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

/// Whether a log line goes to the server: this client's warnings and errors, and the network
/// code's lines but its packet dumps (those are for the player's own packet logging).
fn wanted(level: Level, target: &str, message: &str) -> bool {
    if !target.starts_with("hooks") || target.starts_with("hooks::diagnostics") {
        return false;
    }
    if level <= Level::WARN {
        return true;
    }
    level == Level::INFO && NETWORK.iter().any(|m| target.starts_with(m)) && !message.starts_with("sendto ") && !message.starts_with("recvfrom ")
}

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
        if !wanted(*meta.level(), meta.target(), &text.0) {
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
        loop {
            tokio::time::sleep(EVERY).await;
            let (batch, dropped) = {
                let q = QUEUE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                (q.lines.iter().take(BATCH).cloned().collect::<Vec<_>>(), q.dropped)
            };
            if batch.is_empty() && dropped == 0 {
                continue;
            }
            let sent = batch.len();
            let lines = batch
                .into_iter()
                .map(|mut l| {
                    l.message = hooks_config::redact::redact(&l.message, &private);
                    l
                })
                .collect();
            match crate::api::client_log(lines, dropped).await {
                Ok(()) => {
                    let mut q = QUEUE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let n = sent.min(q.lines.len());
                    q.lines.drain(..n);
                    q.dropped = q.dropped.saturating_sub(dropped);
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

#[cfg(test)]
mod tests {
    use tracing::Level;

    #[test]
    fn warnings_errors_and_network_lines_go_packet_dumps_dont() {
        use super::wanted;
        assert!(wanted(Level::WARN, "hooks::overlay", "Lost the server"));
        assert!(wanted(Level::ERROR, "hooks::api", "sign-in failed"));
        assert!(wanted(Level::INFO, "hooks::hooks::nat", "NAT: told the game to advertise 1.2.3.4:5"));
        assert!(!wanted(Level::INFO, "hooks::hooks::nat", "sendto Some(1.2.3.4:5): 00ff"));
        assert!(!wanted(Level::INFO, "hooks::overlay", "refreshing"));
        assert!(!wanted(Level::DEBUG, "hooks::hooks::nat", "probe"));
        assert!(!wanted(Level::ERROR, "hyper::proto", "connection error"), "only this client's own");
        assert!(!wanted(Level::WARN, "hooks::diagnostics", "x"), "never its own");
    }
}
