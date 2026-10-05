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

/// Whether `ip` is this machine or a trusted reverse proxy (`[public] proxies`).
pub(crate) fn is_proxy(ip: IpAddr) -> bool {
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

/// The server's one limiter for failed password checks per address, shared
/// by every login route (through [`begin_login`]).
pub fn logins() -> &'static RateLimit {
    static LIMIT: std::sync::OnceLock<RateLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| RateLimit::new((limits().failed_logins_per_10_minutes, Duration::from_secs(10 * 60))))
}

/// Sign-ins per address, failed or not: each one hashes a password, so even
/// one account's correct password can't be used to keep the server hashing.
fn attempts() -> &'static RateLimit {
    static LIMIT: std::sync::OnceLock<RateLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| RateLimit::new((limits().logins_per_10_minutes, Duration::from_secs(10 * 60))))
}

/// Whether new accounts may be made (`[limits] open_registration`).
pub fn registration_open() -> bool {
    limits().open_registration
}

/// Whether every account must be linked to an identity (`[limits] require_identity`).
pub fn identity_required() -> bool {
    limits().require_identity
}

/// Whether a request carrying a password or a sign-in may be taken, under
/// `[limits] require_tls_for_credentials`: through a trusted proxy only when
/// it reports the client used HTTPS (`X-Forwarded-Proto`, which Caddy sets),
/// and straight (no proxy header) only from this machine or a private
/// network. Without the setting, always.
pub fn credentials_allowed(peer: Option<IpAddr>, forwarded_proto: Option<&str>) -> bool {
    !limits().require_tls_for_credentials || encrypted_or_local(peer, forwarded_proto)
}

fn encrypted_or_local(peer: Option<IpAddr>, forwarded_proto: Option<&str>) -> bool {
    let Some(peer) = peer else { return false };
    match forwarded_proto {
        Some(proto) if is_proxy(peer) => proto.split(',').next().is_some_and(|p| p.trim().eq_ignore_ascii_case("https")),
        _ => is_local_network(peer),
    }
}

/// This machine, a private network (RFC 1918, IPv6 unique local), link-local,
/// or shared address space (100.64/10, which VPNs such as Tailscale use).
fn is_local_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_local_network(IpAddr::V4(v4)),
            None => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80,
        },
    }
}

/// The answer for credentials refused by [`credentials_allowed`].
pub const TLS_REQUIRED: &str = "This server takes passwords and sign-ins over HTTPS only; connect to it with https:// (update the 5th Echelon launcher)";

/// Starts a sign-in for `name` from `peer`. It counts now, before the
/// password is checked, so a burst of guesses can't all get through before the
/// first failure is recorded; false when the address is over either of its
/// limits, or the account is closed to this address (see [`AccountLimit`]).
pub fn begin_login(peer: Option<IpAddr>, name: &str) -> bool {
    !accounts().blocked(name, peer) && attempts().check(peer) && logins().check(peer)
}

/// The sign-in worked: it isn't a failure after all (it still counts as an
/// attempt), and the address is one this account signs in from.
pub fn login_succeeded(peer: Option<IpAddr>, name: &str) {
    logins().refund(peer);
    accounts().succeeded(name, peer);
}

/// The sign-in failed: it counts against the account from this address, and
/// towards the account's slowdown.
pub fn login_failed(peer: Option<IpAddr>, name: &str) {
    accounts().record(name, peer);
}

/// A login that checks no password (the game's plain `Login`, for the
/// server's own accounts): it counts against the address's logins only.
pub fn plain_login(peer: Option<IpAddr>) -> bool {
    attempts().check(peer)
}

/// The longest name the login limits keep (longer ones can't be accounts, and
/// are cut before they're used as keys).
pub const MAX_NAME: usize = 64;

/// Name checks per address (the launcher's, as a player types a name): as many as
/// sign-ins, counted on their own.
pub fn name_checks() -> &'static RateLimit {
    static LIMIT: std::sync::OnceLock<RateLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| RateLimit::new((limits().logins_per_10_minutes, Duration::from_secs(10 * 60))))
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

/// The game's diagnostic log lines (Misc.ClientLog) kept per player: 60 a minute, a few
/// times what a game in trouble sends; the rest are counted, not kept.
pub fn client_log_lines() -> &'static PlayerLimit {
    static LIMIT: std::sync::OnceLock<PlayerLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| PlayerLimit::new(60, Duration::from_secs(60)))
}

/// Stats and leaderboard requests: 240 a minute, far above the game's menus and a
/// match's end, so one player can't keep the server busy reading and writing stats.
pub fn stats_requests() -> &'static PlayerLimit {
    static LIMIT: std::sync::OnceLock<PlayerLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| PlayerLimit::new(240, Duration::from_secs(60)))
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
pub(crate) fn bucket_of(ip: IpAddr) -> IpAddr {
    bucket(Some(ip))
}

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

/// Failed sign-ins per account. Counted per address (IPv6 by /64): 10 in 10
/// minutes stop that address trying that account, so a stranger guessing
/// can't lock its owner out. Guesses spread over many addresses still slow
/// down: after 50 failures in 10 minutes from anywhere, only addresses that
/// have signed in to the account before (since the server started) may try
/// it, until the failures age out.
pub struct AccountLimit {
    seen: Mutex<AccountSeen>,
}

#[derive(Default)]
struct AccountSeen {
    /// Failures by (account, address).
    failures: HashMap<(String, IpAddr), VecDeque<Instant>>,
    /// Failures by account, from anywhere.
    all: HashMap<String, VecDeque<Instant>>,
    /// Addresses each account signed in from lately, newest last.
    known: HashMap<String, VecDeque<(IpAddr, Instant)>>,
}

pub fn accounts() -> &'static AccountLimit {
    static LIMIT: std::sync::OnceLock<AccountLimit> = std::sync::OnceLock::new();
    LIMIT.get_or_init(AccountLimit::new)
}

impl AccountLimit {
    const MAX: usize = 10;
    const MAX_ANYWHERE: usize = 50;
    const WINDOW: Duration = Duration::from_secs(10 * 60);
    /// How long, and how many, addresses an account is known to sign in from.
    const KNOWN_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);
    const KNOWN_MAX: usize = 8;

    fn new() -> Self {
        Self {
            seen: Mutex::new(AccountSeen::default()),
        }
    }

    fn key(name: &str) -> String {
        identity::name_key(&name.chars().take(MAX_NAME).collect::<String>())
    }

    fn trim(times: &mut VecDeque<Instant>, now: Instant) {
        while times.front().is_some_and(|t| now.duration_since(*t) >= Self::WINDOW) {
            times.pop_front();
        }
    }

    /// Whether `peer` may not try `name` now.
    pub fn blocked(&self, name: &str, peer: Option<IpAddr>) -> bool {
        self.blocked_at(name, peer, Instant::now())
    }

    fn blocked_at(&self, name: &str, peer: Option<IpAddr>, now: Instant) -> bool {
        if peer.is_some_and(|p| p.is_loopback()) {
            return false;
        }
        let key = Self::key(name);
        let addr = bucket(peer);
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(times) = seen.failures.get_mut(&(key.clone(), addr)) {
            Self::trim(times, now);
            if times.len() >= Self::MAX {
                return true;
            }
        }
        let Some(times) = seen.all.get_mut(&key) else { return false };
        Self::trim(times, now);
        if times.len() < Self::MAX_ANYWHERE {
            return false;
        }
        // Under attack: an address the account has signed in from still gets to try.
        !seen
            .known
            .get(&key)
            .is_some_and(|known| known.iter().any(|(ip, t)| *ip == addr && now.duration_since(*t) < Self::KNOWN_FOR))
    }

    /// Counts a failed sign-in for `name` from `peer`.
    pub fn record(&self, name: &str, peer: Option<IpAddr>) {
        self.record_at(name, peer, Instant::now());
    }

    fn record_at(&self, name: &str, peer: Option<IpAddr>, now: Instant) {
        if peer.is_some_and(|p| p.is_loopback()) {
            return;
        }
        let key = Self::key(name);
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.failures.len() > 100_000 {
            seen.failures.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < Self::WINDOW));
        }
        if seen.all.len() > 100_000 {
            seen.all.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < Self::WINDOW));
        }
        seen.failures.entry((key.clone(), bucket(peer))).or_default().push_back(now);
        let all = seen.all.entry(key).or_default();
        all.push_back(now);
        // Only the count that blocks matters; no more than that is kept.
        while all.len() > Self::MAX_ANYWHERE {
            all.pop_front();
        }
    }

    /// Remembers that `name` signed in from `peer`.
    pub fn succeeded(&self, name: &str, peer: Option<IpAddr>) {
        self.succeeded_at(name, peer, Instant::now());
    }

    fn succeeded_at(&self, name: &str, peer: Option<IpAddr>, now: Instant) {
        if peer.is_some_and(|p| p.is_loopback()) {
            return;
        }
        let key = Self::key(name);
        let addr = bucket(peer);
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        seen.failures.remove(&(key.clone(), addr));
        if seen.known.len() > 100_000 {
            seen.known.retain(|_, known| known.back().is_some_and(|(_, t)| now.duration_since(*t) < Self::KNOWN_FOR));
        }
        let known = seen.known.entry(key).or_default();
        known.retain(|(ip, _)| *ip != addr);
        known.push_back((addr, now));
        while known.len() > Self::KNOWN_MAX {
            known.pop_front();
        }
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
    fn failures_are_counted_per_account_and_address() {
        let limit = AccountLimit::new();
        let stranger = Some(IpAddr::from([198, 51, 100, 1]));
        let owner = Some(IpAddr::from([203, 0, 113, 5]));
        let t0 = Instant::now();
        for _ in 0..AccountLimit::MAX {
            assert!(!limit.blocked_at("Kiwi", stranger, t0));
            limit.record_at("kiwi", stranger, t0);
        }
        assert!(limit.blocked_at("KIWI", stranger, t0), "any case");
        assert!(!limit.blocked_at("Tank", stranger, t0));
        assert!(!limit.blocked_at("Kiwi", owner, t0), "a stranger's guesses don't lock the owner out");
        assert!(!limit.blocked_at("Kiwi", stranger, t0 + AccountLimit::WINDOW), "failures expire");
    }

    #[test]
    fn guesses_from_everywhere_leave_known_addresses_in() {
        let limit = AccountLimit::new();
        let owner = Some(IpAddr::from([203, 0, 113, 5]));
        let new_place = Some(IpAddr::from([203, 0, 113, 6]));
        let t0 = Instant::now();
        limit.succeeded_at("Kiwi", owner, t0);
        for i in 0..AccountLimit::MAX_ANYWHERE {
            limit.record_at("Kiwi", Some(IpAddr::from([198, 51, 100, i as u8])), t0);
        }
        assert!(!limit.blocked_at("Kiwi", owner, t0), "signed in from here before");
        assert!(limit.blocked_at("Kiwi", new_place, t0), "a new address waits");
        assert!(!limit.blocked_at("Kiwi", new_place, t0 + AccountLimit::WINDOW));
        assert!(!limit.blocked_at("Tank", new_place, t0));
        // An IPv6 owner is known by their /64.
        let v6 = Some("2001:db8:1:2::1".parse().unwrap());
        limit.succeeded_at("Kiwi", v6, t0);
        assert!(!limit.blocked_at("Kiwi", Some("2001:db8:1:2::9".parse().unwrap()), t0));
    }

    #[test]
    fn long_names_are_cut() {
        assert_eq!(AccountLimit::key(&"k".repeat(10_000)).len(), MAX_NAME);
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
    fn credentials_need_https_through_a_proxy() {
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        let public: IpAddr = "198.51.100.7".parse().unwrap();
        assert!(encrypted_or_local(Some(local), Some("https")));
        assert!(!encrypted_or_local(Some(local), Some("http")), "Caddy says the client used plain http");
        assert!(encrypted_or_local(Some(local), None), "a tool on this machine");
        assert!(encrypted_or_local(Some(lan), None), "a LAN player straight to the server");
        assert!(!encrypted_or_local(Some(public), None), "plain h2c from the internet");
        assert!(!encrypted_or_local(Some(public), Some("https")), "only a trusted proxy's word counts");
        assert!(encrypted_or_local(Some("fd00::5".parse().unwrap()), None));
        assert!(!encrypted_or_local(Some("2001:db8::5".parse().unwrap()), None));
        assert!(!encrypted_or_local(None, None));
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
