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

/// Reverse proxies whose `X-Forwarded-For` is trusted (`[public] proxies`).
static PROXIES: std::sync::OnceLock<Vec<Proxy>> = std::sync::OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Proxy {
    net: IpAddr,
    prefix: u8,
}

impl Proxy {
    fn parse(s: &str) -> Option<Self> {
        let (addr, prefix) = s.trim().split_once('/').map_or((s.trim(), None), |(a, p)| (a, Some(p)));
        let net: IpAddr = addr.parse().ok()?;
        let max = if net.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.map_or(Some(max), |p| p.parse().ok().filter(|p| *p <= max))?;
        Some(Self { net, prefix })
    }

    fn contains(self, ip: IpAddr) -> bool {
        let bits = |ip: IpAddr| -> u128 {
            match ip {
                IpAddr::V4(v4) => u128::from(u32::from(v4)) << 96,
                IpAddr::V6(v6) => u128::from(v6),
            }
        };
        if self.net.is_ipv4() != ip.is_ipv4() {
            return false;
        }
        // IPv4 addresses sit in the top 32 bits, so one prefix length works for both.
        let len = u32::from(self.prefix);
        let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
        bits(self.net) & mask == bits(ip) & mask
    }
}

/// Trusts `X-Forwarded-For` from these proxies (addresses or subnets);
/// invalid entries are returned.
pub fn trust_proxies(list: &[String]) -> Vec<String> {
    let (ok, bad): (Vec<_>, Vec<_>) = list.iter().map(|s| (s, Proxy::parse(s))).partition(|(_, p)| p.is_some());
    let _ = PROXIES.set(ok.into_iter().filter_map(|(_, p)| p).collect());
    bad.into_iter().map(|(s, _)| s.clone()).collect()
}

fn is_proxy(ip: IpAddr) -> bool {
    ip.is_loopback() || PROXIES.get().is_some_and(|list| list.iter().any(|p| p.contains(ip)))
}

/// The client a request comes from: the peer, or, when the peer is a
/// trusted reverse proxy, the last address in `X-Forwarded-For` that isn't
/// one (proxies append the address they saw).
pub fn client_ip(peer: Option<IpAddr>, forwarded_for: Option<&str>) -> Option<IpAddr> {
    let peer = peer?;
    if !is_proxy(peer) {
        return Some(peer);
    }
    let Some(list) = forwarded_for else {
        return Some(peer);
    };
    let hops: Vec<IpAddr> = list.split(',').filter_map(|a| a.trim().parse().ok()).collect();
    Some(hops.iter().rev().copied().find(|ip| !is_proxy(*ip)).or(hops.first().copied()).unwrap_or(peer))
}

/// The server's one limiter for failed password checks, shared by every
/// login route: check [`RateLimit::blocked`] first, [`RateLimit::record`]
/// each failure.
pub fn logins() -> &'static RateLimit {
    static LIMIT: std::sync::OnceLock<RateLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| RateLimit::new((limits().failed_logins_per_10_minutes, Duration::from_secs(10 * 60))))
}

/// Whether new accounts may be made (`[limits] open_registration`).
pub fn registration_open() -> bool {
    limits().open_registration
}

/// Starts a sign-in for `name` from `peer`. It counts now, before the
/// password is checked, so a burst of guesses can't all get through before the
/// first failure is recorded; false when the address or the account is over
/// its limit.
pub fn begin_login(peer: Option<IpAddr>, name: &str) -> bool {
    !accounts().blocked(name) && logins().check(peer)
}

/// The sign-in worked: its count is given back (only failures count).
pub fn login_succeeded(peer: Option<IpAddr>) {
    logins().refund(peer);
}

/// The sign-in failed: it counts against the account too.
pub fn login_failed(name: &str) {
    accounts().record(name);
}

/// The server's one limiter for new accounts.
pub fn registrations() -> &'static RateLimit {
    static LIMIT: std::sync::OnceLock<RateLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| RateLimit::new((limits().registrations_per_hour, Duration::from_secs(60 * 60))))
}

/// At most `max` actions per player in any `window`: invites, friend
/// requests and searches, which a signed-in player could otherwise repeat
/// without end.
pub struct PlayerLimit {
    max: usize,
    window: Duration,
    seen: Mutex<HashMap<u32, VecDeque<Instant>>>,
}

impl PlayerLimit {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Counts an action by `player` and says whether it's allowed.
    pub fn check(&self, player: u32) -> bool {
        self.check_at(player, Instant::now())
    }

    fn check_at(&self, player: u32, now: Instant) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        seen.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < self.window));
        let times = seen.entry(player).or_default();
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

/// Invites a player may send: 20 a minute.
pub fn invites() -> &'static PlayerLimit {
    static LIMIT: std::sync::OnceLock<PlayerLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| PlayerLimit::new(20, Duration::from_secs(60)))
}

/// Friend requests, blocks and other friend changes: 30 a minute.
pub fn friend_changes() -> &'static PlayerLimit {
    static LIMIT: std::sync::OnceLock<PlayerLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| PlayerLimit::new(30, Duration::from_secs(60)))
}

/// Game sessions a player may create: 30 a minute.
pub fn sessions() -> &'static PlayerLimit {
    static LIMIT: std::sync::OnceLock<PlayerLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| PlayerLimit::new(30, Duration::from_secs(60)))
}

/// Game-protocol requests that make the server work or send to others
/// (session searches, lookups, probes): 120 a minute.
pub fn game_requests() -> &'static PlayerLimit {
    static LIMIT: std::sync::OnceLock<PlayerLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| PlayerLimit::new(120, Duration::from_secs(60)))
}

/// Requests the overlay and the game poll (friend lists, events, session
/// announcements): 240 a minute, well above what they need.
pub fn polls() -> &'static PlayerLimit {
    static LIMIT: std::sync::OnceLock<PlayerLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| PlayerLimit::new(240, Duration::from_secs(60)))
}

/// Player searches: 60 a minute.
pub fn searches() -> &'static PlayerLimit {
    static LIMIT: std::sync::OnceLock<PlayerLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| PlayerLimit::new(60, Duration::from_secs(60)))
}

/// The budget an address counts against: itself, or for IPv6 its /64 (one
/// home or server has a whole /64, so single addresses cost nothing to change).
fn bucket(peer: Option<IpAddr>) -> IpAddr {
    match peer {
        Some(IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_none() => {
            let bits = u128::from(v6) & (u128::MAX << 64);
            IpAddr::V6(bits.into())
        }
        Some(IpAddr::V6(v6)) => IpAddr::V4(v6.to_ipv4_mapped().unwrap_or(std::net::Ipv4Addr::UNSPECIFIED)),
        Some(ip) => ip,
        None => IpAddr::from([0, 0, 0, 0]),
    }
}

/// Failed sign-ins per account: 10 in 10 minutes, from anywhere, so guesses
/// spread over many addresses still stop.
pub struct AccountLimit {
    seen: Mutex<HashMap<String, VecDeque<Instant>>>,
}

pub fn accounts() -> &'static AccountLimit {
    static LIMIT: std::sync::OnceLock<AccountLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| AccountLimit { seen: Mutex::new(HashMap::new()) })
}

impl AccountLimit {
    const MAX: usize = 10;
    const WINDOW: Duration = Duration::from_secs(10 * 60);

    fn key(name: &str) -> String {
        identity::name_key(name)
    }

    /// Whether `name` has had too many failed sign-ins lately.
    pub fn blocked(&self, name: &str) -> bool {
        let now = Instant::now();
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(times) = seen.get_mut(&Self::key(name)) else { return false };
        while times.front().is_some_and(|t| now.duration_since(*t) >= Self::WINDOW) {
            times.pop_front();
        }
        times.len() >= Self::MAX
    }

    /// Counts a failed sign-in for `name`.
    pub fn record(&self, name: &str) {
        let now = Instant::now();
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.len() > 100_000 {
            seen.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < Self::WINDOW));
        }
        seen.entry(Self::key(name)).or_default().push_back(now);
    }
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

    /// Gives back the latest count for `peer`: an attempt counted by
    /// [`RateLimit::check`] that turned out fine (a sign-in that worked).
    pub fn refund(&self, peer: Option<IpAddr>) {
        let key = bucket(peer);
        if let Some(times) = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_mut(&key) {
            times.pop_back();
        }
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
        let key = bucket(peer);
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
        let key = bucket(peer);
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        seen.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < self.window));
        seen.entry(key).or_default().push_back(now);
    }

    fn check_at(&self, peer: Option<IpAddr>, now: Instant) -> bool {
        // This machine: the launcher and tools next to the server.
        if peer.is_some_and(|p| p.is_loopback()) {
            return true;
        }
        let key = bucket(peer);
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
    fn player_limits_are_per_player() {
        let limit = PlayerLimit::new(2, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limit.check_at(1, t0) && limit.check_at(1, t0));
        assert!(!limit.check_at(1, t0), "third within the window");
        assert!(limit.check_at(2, t0), "another player has their own budget");
        assert!(limit.check_at(1, t0 + Duration::from_secs(61)));
    }

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
    fn ipv6_addresses_share_their_64() {
        let limit = RateLimit::new((2, Duration::from_secs(60)));
        let t0 = Instant::now();
        assert!(limit.check_at(Some("2001:db8:1:2::1".parse().unwrap()), t0));
        assert!(limit.check_at(Some("2001:db8:1:2::ffff".parse().unwrap()), t0));
        assert!(!limit.check_at(Some("2001:db8:1:2:abcd::9".parse().unwrap()), t0), "same /64");
        assert!(limit.check_at(Some("2001:db8:1:3::1".parse().unwrap()), t0), "another /64");
    }

    #[test]
    fn a_refunded_attempt_frees_its_place() {
        let limit = RateLimit::new((1, Duration::from_secs(60)));
        let peer = Some(IpAddr::from([192, 0, 2, 3]));
        assert!(limit.check(peer));
        limit.refund(peer);
        assert!(limit.check(peer), "the first attempt succeeded and was given back");
        assert!(!limit.check(peer));
    }

    #[test]
    fn failures_are_counted_per_account_too() {
        let limit = AccountLimit { seen: Mutex::new(HashMap::new()) };
        for _ in 0..AccountLimit::MAX {
            assert!(!limit.blocked("Kiwi"));
            limit.record("kiwi");
        }
        assert!(limit.blocked("KIWI"), "any case");
        assert!(!limit.blocked("Tank"));
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

    #[test]
    fn forwarded_for_is_believed_only_from_proxies() {
        let client: IpAddr = "198.51.100.7".parse().unwrap();
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        // A proxy on this machine (e.g. Caddy) names the client.
        assert_eq!(client_ip(Some(local), Some("198.51.100.7")), Some(client));
        // A spoofed header added by the client itself is skipped.
        assert_eq!(client_ip(Some(local), Some("10.9.9.9, 198.51.100.7")), Some(client));
        // A local request without the header is local.
        assert_eq!(client_ip(Some(local), None), Some(local));
        // Anyone else can't claim another address.
        assert_eq!(client_ip(Some(client), Some("203.0.113.1")), Some(client));

        let docker = Proxy::parse("172.17.0.0/16").unwrap();
        assert!(docker.contains("172.17.3.4".parse().unwrap()) && !docker.contains("172.18.0.1".parse().unwrap()));
        assert!(Proxy::parse("10.0.0.1").unwrap().contains("10.0.0.1".parse().unwrap()));
        assert!(Proxy::parse("10.0.0.0/33").is_none() && Proxy::parse("proxy").is_none());
    }
}
