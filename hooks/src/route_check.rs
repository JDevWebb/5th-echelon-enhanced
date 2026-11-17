//! Whether the game's connection to the server keeps losing its answers (0.5.5's SR). Some
//! players' routes to the network's matchmaking server lose the replies while their own packets
//! get through: the API's calls time out one after another, and the game can't go online
//! (Ghost_Leader and Captain_Kaput, 16 Nov 2026). When [`FAILURES`] calls fail that way within
//! [`WITHIN`], the add-on writes [`hooks_config::VIA_RELAY_FILE`] once, and from the next start the
//! launcher signs this network in through the nearest relay. Already through one, nothing is
//! written. The API's calls (gRPC) and the server's HTTP (the add-on's /api/info and news) both
//! count, and the finding says which lost their answers ([`Kind`]): the night reports show
//! whether players who need the relay lose plain HTTP too (the relay carries both).

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use tracing::info;
use tracing::warn;

/// Calls that failed for want of an answer within [`WITHIN`]: enough to go through a relay.
const FAILURES: usize = 4;
const WITHIN: Duration = Duration::from_secs(180);

/// Which of the game's calls lost its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The API (gRPC).
    Api,
    /// The server's HTTP (its /api/info, news).
    Http,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Api => "api",
            Kind::Http => "http",
        }
    }
}

#[derive(Default)]
struct Seen {
    failures: Vec<(Instant, Kind)>,
    written: bool,
}

static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

/// Whether this error means the server's answer never came (a timeout, a connection that
/// couldn't be made or was cut), not that the server answered no.
pub fn lost_answer(e: &crate::api::Error) -> bool {
    match e {
        crate::api::Error::Transport(_) | crate::api::Error::NotConnected => true,
        crate::api::Error::GRPCStatus(s) => {
            let message = s.message();
            matches!(s.code(), tonic::Code::Unavailable | tonic::Code::DeadlineExceeded)
                || (matches!(s.code(), tonic::Code::Cancelled | tonic::Code::Unknown) && (message.contains("Timeout expired") || message.contains("transport error")))
        }
        _ => false,
    }
}

/// A call's outcome. Returns whether this one was the failure that tipped it.
fn note_at(seen: &mut Seen, lost: bool, kind: Kind, now: Instant) -> bool {
    if !lost || seen.written {
        return false;
    }
    seen.failures.retain(|(at, _)| now.saturating_duration_since(*at) < WITHIN);
    seen.failures.push((now, kind));
    if seen.failures.len() >= FAILURES {
        seen.written = true;
        return true;
    }
    false
}

/// The kinds among the failures that tipped it, each with its count: ["api 3", "http 1"].
fn kinds(seen: &Seen) -> Vec<String> {
    [Kind::Api, Kind::Http]
        .into_iter()
        .filter_map(|k| {
            let n = seen.failures.iter().filter(|(_, f)| *f == k).count();
            (n > 0).then(|| format!("{} {n}", k.name()))
        })
        .collect()
}

/// An API call's outcome (the add-on's calls that wait for their answer).
pub fn note<T>(result: &Result<T, crate::api::Error>) {
    let Err(e) = result else { return };
    if lost_answer(e) {
        lost(Kind::Api);
    }
}

/// One of the add-on's HTTP requests to the server got no answer (it timed out, or the
/// connection couldn't be made or was cut).
pub fn note_http(e: &std::io::Error) {
    use std::io::ErrorKind;
    if matches!(
        e.kind(),
        ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::UnexpectedEof
    ) {
        lost(Kind::Http);
    }
}

fn lost(kind: Kind) {
    // Already through a relay: nothing more to try from here.
    if hooks_config::get().is_some_and(|c| c.networking.api_via.is_some()) {
        return;
    }
    let tipped = {
        let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let seen = seen.get_or_insert_with(Seen::default);
        note_at(seen, true, kind, Instant::now()).then(|| kinds(seen))
    };
    if let Some(failed) = tipped {
        write(failed);
    }
}

fn write(failed: Vec<String>) {
    let found = hooks_config::ViaRelayFound {
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or_default()),
        why: format!(
            "{FAILURES} calls to the server got no answer within {} minutes ({})",
            WITHIN.as_secs() / 60,
            failed.join(", ")
        ),
        failed,
    };
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::Path::to_path_buf)) else {
        return;
    };
    match serde_json::to_string(&found).map(|text| std::fs::write(dir.join(hooks_config::VIA_RELAY_FILE), text)) {
        Ok(Ok(())) => info!("Connection: {}; the launcher signs in through the nearest relay from the next start", found.why),
        Ok(Err(e)) => warn!("Connection: couldn't keep what was found for the launcher: {e}"),
        Err(e) => warn!("Connection: couldn't keep what was found for the launcher: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_lost_answers_in_three_minutes_tip_it_once() {
        let mut seen = Seen::default();
        let t = Instant::now();
        let at = |s: u64| t + Duration::from_secs(s);
        assert!(!note_at(&mut seen, true, Kind::Api, at(0)));
        assert!(!note_at(&mut seen, false, Kind::Api, at(1)), "an answer that came doesn't count");
        assert!(!note_at(&mut seen, true, Kind::Http, at(30)));
        assert!(!note_at(&mut seen, true, Kind::Api, at(60)));
        assert!(note_at(&mut seen, true, Kind::Api, at(90)), "the fourth within three minutes, of either kind");
        assert_eq!(kinds(&seen), ["api 3", "http 1"]);
        assert!(!note_at(&mut seen, true, Kind::Api, at(100)), "once");
    }

    #[test]
    fn spread_out_failures_dont() {
        let mut seen = Seen::default();
        let t = Instant::now();
        for i in 0..6 {
            assert!(!note_at(&mut seen, true, Kind::Http, t + Duration::from_secs(i * 100)), "one every 100 s");
        }
    }

    #[test]
    fn only_lost_answers_count() {
        use crate::api::Error;
        assert!(lost_answer(&Error::GRPCStatus(tonic::Status::new(tonic::Code::Cancelled, "Timeout expired"))));
        assert!(lost_answer(&Error::GRPCStatus(tonic::Status::new(tonic::Code::Unknown, "transport error"))));
        assert!(lost_answer(&Error::GRPCStatus(tonic::Status::unavailable("down"))));
        assert!(lost_answer(&Error::NotConnected));
        assert!(!lost_answer(&Error::GRPCStatus(tonic::Status::permission_denied("banned"))), "the server answered");
        assert!(!lost_answer(&Error::LoginFailure));
    }
}
