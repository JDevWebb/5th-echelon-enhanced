//! Per-address rate limits, for everything that checks a password.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// The limits from `[limits]` in service.toml, set once at startup (the
/// defaults otherwise, e.g. in tests).
static LIMITS: std::sync::OnceLock<crate::config::LimitsConfig> = std::sync::OnceLock::new();

pub fn configure(limits: crate::config::LimitsConfig) {
    let _ = LIMITS.set(limits);
}

fn limits() -> crate::config::LimitsConfig {
    LIMITS.get().copied().unwrap_or_default()
}

/// The server's one limiter for failed password checks, shared by every
/// login route: check [`RateLimit::blocked`] first, [`RateLimit::record`]
/// each failure.
pub fn logins() -> &'static RateLimit {
    static LIMIT: std::sync::OnceLock<RateLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| RateLimit::new((limits().failed_logins_per_10_minutes, Duration::from_secs(10 * 60))))
}

/// The server's one limiter for new accounts.
pub fn registrations() -> &'static RateLimit {
    static LIMIT: std::sync::OnceLock<RateLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| RateLimit::new((limits().registrations_per_hour, Duration::from_secs(60 * 60))))
}

/// At most `max` requests per address in any `window`.
pub struct RateLimit {
    pub(crate) max: usize,
    window: Duration,
    seen: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl RateLimit {
    pub fn new((max, window): (usize, Duration)) -> Self {
        Self {
            max,
            window,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Counts a request from `peer` and says whether it's allowed. Requests
    /// without a known address share one budget.
    pub fn check(&self, peer: Option<IpAddr>) -> bool {
        self.check_at(peer, Instant::now())
    }

    /// Whether `peer` has used up its budget, without counting anything.
    pub fn blocked(&self, peer: Option<IpAddr>) -> bool {
        self.blocked_at(peer, Instant::now())
    }

    /// Counts one event (e.g. a failed login) for `peer`.
    pub fn record(&self, peer: Option<IpAddr>) {
        self.record_at(peer, Instant::now());
    }

    fn blocked_at(&self, peer: Option<IpAddr>, now: Instant) -> bool {
        if peer.is_some_and(|p| p.is_loopback()) {
            return false;
        }
        let key = peer.unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(times) = seen.get_mut(&key) else { return false };
        while times.front().is_some_and(|t| now.duration_since(*t) >= self.window) {
            times.pop_front();
        }
        times.len() >= self.max
    }

    fn record_at(&self, peer: Option<IpAddr>, now: Instant) {
        if peer.is_some_and(|p| p.is_loopback()) {
            return;
        }
        let key = peer.unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        seen.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < self.window));
        seen.entry(key).or_default().push_back(now);
    }

    fn check_at(&self, peer: Option<IpAddr>, now: Instant) -> bool {
        // This machine: the launcher and tools next to the server.
        if peer.is_some_and(|p| p.is_loopback()) {
            return true;
        }
        let key = peer.unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Forget addresses whose requests have all expired, so the map stays small.
        seen.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < self.window));
        let times = seen.entry(key).or_default();
        while times.front().is_some_and(|t| now.duration_since(*t) >= self.window) {
            times.pop_front();
        }
        if times.len() >= self.max {
            return false;
        }
        times.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_window_slides() {
        let limit = RateLimit::new((2, Duration::from_secs(60)));
        let peer = Some(IpAddr::from([192, 0, 2, 1]));
        let t0 = Instant::now();
        assert!(limit.check_at(peer, t0));
        assert!(limit.check_at(peer, t0 + Duration::from_secs(10)));
        assert!(!limit.check_at(peer, t0 + Duration::from_secs(20)), "third within the window");
        assert!(limit.check_at(peer, t0 + Duration::from_secs(61)), "the first request has expired");
        assert!(!limit.check_at(peer, t0 + Duration::from_secs(62)));
        // Idle addresses are forgotten.
        assert!(limit.check_at(Some(IpAddr::from([192, 0, 2, 9])), t0 + Duration::from_secs(300)));
        assert_eq!(limit.seen.lock().unwrap().len(), 1);
    }


    #[test]
    fn only_recorded_failures_block() {
        let limit = RateLimit::new((2, Duration::from_secs(60)));
        let peer = Some(IpAddr::from([192, 0, 2, 7]));
        let t0 = Instant::now();
        assert!(!limit.blocked_at(peer, t0));
        limit.record_at(peer, t0);
        limit.record_at(peer, t0 + Duration::from_secs(1));
        assert!(limit.blocked_at(peer, t0 + Duration::from_secs(2)));
        assert!(!limit.blocked_at(Some(IpAddr::from([192, 0, 2, 8])), t0), "per address");
        assert!(!limit.blocked_at(peer, t0 + Duration::from_secs(61)), "failures expire");
    }

    #[test]
    fn this_machine_is_never_limited() {
        let limit = RateLimit::new((1, Duration::from_secs(60)));
        let local = Some(IpAddr::from([127, 0, 0, 1]));
        let t0 = Instant::now();
        assert!(limit.check_at(local, t0) && limit.check_at(local, t0));
        limit.record_at(local, t0);
        assert!(!limit.blocked_at(local, t0));
    }
}
