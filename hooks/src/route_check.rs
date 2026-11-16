//! Whether the game's connection to the server keeps losing its answers (0.5.5's SR). Some
//! players' routes to the network's matchmaking server lose the replies while their own packets
//! get through: the API's calls time out one after another, and the game can't go online
//! (Ghost_Leader and Captain_Kaput, 16 Nov 2026). When [`FAILURES`] calls fail that way within
//! [`WITHIN`], the add-on writes [`hooks_config::VIA_RELAY_FILE`] once, and from the next start the
//! launcher signs this network in through the nearest relay. Already through one, nothing is
//! written.

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use tracing::info;
use tracing::warn;

/// Calls that failed for want of an answer within [`WITHIN`]: enough to go through a relay.
const FAILURES: usize = 4;
const WITHIN: Duration = Duration::from_secs(180);

#[derive(Default)]
struct Seen {
    failures: Vec<Instant>,
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
fn note_at(seen: &mut Seen, lost: bool, now: Instant) -> bool {
    if !lost || seen.written {
        return false;
    }
    seen.failures.retain(|at| now.saturating_duration_since(*at) < WITHIN);
    seen.failures.push(now);
    if seen.failures.len() >= FAILURES {
        seen.written = true;
        return true;
    }
    false
}

/// An API call's outcome (the add-on's calls that wait for their answer).
pub fn note<T>(result: &Result<T, crate::api::Error>) {
    let Err(e) = result else { return };
    if !lost_answer(e) {
        return;
    }
    // Already through a relay: nothing more to try from here.
    if hooks_config::get().is_some_and(|c| c.networking.api_via.is_some()) {
        return;
    }
    let tipped = {
        let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        note_at(seen.get_or_insert_with(Seen::default), true, Instant::now())
    };
    if tipped {
        write();
    }
}

fn write() {
    let found = hooks_config::ViaRelayFound {
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or_default()),
        why: format!("{FAILURES} calls to the server got no answer within {} minutes", WITHIN.as_secs() / 60),
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
        assert!(!note_at(&mut seen, true, at(0)));
        assert!(!note_at(&mut seen, false, at(1)), "an answer that came doesn't count");
        assert!(!note_at(&mut seen, true, at(30)));
        assert!(!note_at(&mut seen, true, at(60)));
        assert!(note_at(&mut seen, true, at(90)), "the fourth within three minutes");
        assert!(!note_at(&mut seen, true, at(100)), "once");
    }

    #[test]
    fn spread_out_failures_dont() {
        let mut seen = Seen::default();
        let t = Instant::now();
        for i in 0..6 {
            assert!(!note_at(&mut seen, true, t + Duration::from_secs(i * 100)), "one every 100 s");
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
