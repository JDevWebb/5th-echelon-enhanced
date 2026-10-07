//! The coordinator: shares friends between 5th Echelon servers, and lists
//! them in a server directory for launchers.
//!
//! Servers join with a join token (from whoever runs the coordinator) and get
//! a secret for everything after. Then:
//!
//! * `POST /v1/heartbeat`: a server's directory entry (name, region, address,
//!   ports, players online), every 30 seconds.
//! * `GET /v1/servers`: the directory, for launchers (no sign-in), with the rollout of a
//!   release while one is going out.
//! * `POST /v1/changes`: a server's friend changes, in order: links of
//!   accounts to identities (signed by the player, checked here), friendships
//!   and blocks between players linked on that server.
//! * `GET /v1/relations/<identity>`: a linked player's friends and blocks,
//!   for the server they're on.
//!
//! * `POST /v1/names/claim`, `GET /v1/names/<name>`: names are reserved
//!   across the group, one identity each, so "Kiwi" is the same player on
//!   every member server.
//!
//! A server may only act for players linked on it, and a link needs the
//! player's signature, so no server can speak for someone who never used it.
//!
//! * `POST /v1/pulse`: a server's live numbers, every ten seconds (for the
//!   admin UI, kept a day); the answer lists what admins asked it to do to
//!   players (see [`players`]),
//! * `POST /v1/metrics`: a server's metrics, every minute (see [`metrics`]),
//!   for the admin UI ([`admin`], on its own listener).
//! * `POST /v1/players`: a server's players and play sessions, and
//!   `POST /v1/actions/<id>`: what came of an admin's action (see [`players`]).
//! * `POST /v1/pings`: a launcher's pings to the servers (no sign-in).
//! * `POST /v1/stats`, `GET /v1/leaderboards`, `POST /v1/leaderboards/players` and
//!   `POST /v1/stats/players`: each person's stats across the network, and the global
//!   leaderboards (see [`stats`]).
//! * `POST /v1/reports`: a player's feedback or problem report, forwarded by the server they
//!   played on, with their logs and the server's (see [`reports`]).
//! * `POST /v1/events`: players' session events, for the admin UI's Sessions page (see
//!   [`sessions`]).
//! * Heartbeat answers carry the release a server should install (see
//!   [`updates`]); servers that don't keep up leave the directory.

pub mod admin;
pub mod alerts;
pub mod content;
pub mod game_names;
pub mod limits;
pub mod maintenance;
pub mod metrics;
pub mod players;
pub mod reports;
pub mod roadmap;
pub mod sessions;
pub mod standby;
pub mod stats;
pub mod support;
pub mod updates;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::extract::DefaultBodyLimit;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::routing::get;
use axum::routing::post;
use axum::Json;
use axum::Router;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use sha2::Digest as _;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePool;

/// How long a server stays in the directory after its last heartbeat.
pub const LISTED_FOR: Duration = Duration::from_secs(120);
/// The most changes one request may carry.
const MAX_CHANGES: usize = 200;
/// A name claimed for an account that's still being made isn't released for
/// this long, even though no link uses it yet.
const CLAIM_GRACE_SECS: i64 = 60 * 60;
/// Names one server may claim in an hour that no link uses (yet).
const MAX_UNLINKED_CLAIMS_PER_HOUR: i64 = 200;
/// The largest request body.
const MAX_BODY: usize = 256 * 1024;
/// The most players online a listing says (the load test ran 1,000 on one core).
const MAX_LISTED_PLAYERS: u32 = 5_000;
/// New links one server may make in an hour, and links it may have in all: a member can't
/// reserve names by the thousand with throwaway identities.
const MAX_NEW_LINKS_PER_HOUR: i64 = 120;
const MAX_LINKS_PER_SERVER: i64 = 50_000;
/// A link's signature is taken this long after it was made (a server queues links while the
/// coordinator is down, and sends them when it's back).
const LINK_SIGNED_FOR: i64 = 7 * 24 * 3600;
/// Host names (and addresses) one server may hold.
const MAX_SERVER_NAMES: i64 = 32;

/// The key an address's requests are counted under: IPv4 addresses one by
/// one, IPv6 by /64 (one subscriber's network, any address of which they can
/// use).
pub(crate) fn limit_key(ip: std::net::IpAddr) -> String {
    match ip {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
    }
}

/// Whether `ip` is a public address: not this machine, a private or shared
/// network, link-local, documentation, multicast or reserved.
pub(crate) fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_documentation()
                || v4.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                // Benchmarking (198.18.0.0/15), often used inside networks.
                || (a == 198 && (b & 0xfe) == 18)
                || (a == 192 && b == 0 && c == 0))
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_ip(std::net::IpAddr::V4(v4));
            }
            let [first, second, ..] = v6.segments();
            // NAT64 (64:ff9b::/96) reaches IPv4 addresses, private ones too: as the one inside.
            if let [0x0064, 0xff9b, 0, 0, 0, 0, hi, lo] = v6.segments() {
                let [a, b] = hi.to_be_bytes();
                let [c, d] = lo.to_be_bytes();
                return public_ip(std::net::IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d)));
            }
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80 || (first == 0x2001 && second == 0x0db8))
        }
    }
}

/// A request's source address: the peer, or, from a proxy on this machine,
/// the last address in `X-Forwarded-For`.
/// The largest body a request to `path` may have, and whether only a member server may send
/// it (as `DefaultBodyLimit` on the routes).
fn body_rule(path: &str) -> (usize, bool) {
    match path {
        "/v1/reports" => (reports::MAX_BODY, true),
        "/v1/content" => (content::MAX_BODY, true),
        "/v1/players" => (players::MAX_BODY, true),
        "/v1/stats" => (stats::MAX_BODY, true),
        "/v1/events" => (sessions::MAX_BODY, true),
        "/v1/support" => (support::MAX_BODY, false),
        _ => (MAX_BODY, false),
    }
}

/// Every API request with a body: a member server's (its secret checked first, before the
/// body is read) within its budget; anyone else's within the support or public one, a few at
/// once per address, and refused for routes only members may use (see [`limits::Budgets`]).
/// Each within the deadline, so streams of half-sent bodies can't fill the memory.
async fn bounded(State(c): State<Shared>, request: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::response::IntoResponse;
    if matches!(*request.method(), axum::http::Method::GET | axum::http::Method::HEAD) {
        return next.run(request).await;
    }
    let (limit, members_only) = body_rule(request.uri().path());
    let member = if request.headers().contains_key(axum::http::header::AUTHORIZATION) {
        c.server(request.headers()).await
    } else {
        Err(fail(StatusCode::UNAUTHORIZED, "sign in with the server's secret"))
    };
    match member {
        Ok(_) => c.budgets.member.run(limit, next.run(request)).await,
        Err(refused) if members_only => refused.into_response(),
        Err(_) => {
            let peer = request.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|ci| ci.0);
            let ip = peer.map_or(std::net::IpAddr::from([0, 0, 0, 0]), |peer| client_ip(peer, request.headers()));
            let (in_flight, budget) = if request.uri().path() == "/v1/support" {
                (&c.budgets.support_in_flight, &c.budgets.support)
            } else {
                (&c.budgets.public_in_flight, &c.budgets.public)
            };
            let Some(_slot) = in_flight.take(ip) else {
                return limits::too_many();
            };
            budget.run(limit, next.run(request)).await
        }
    }
}

fn client_ip(peer: std::net::SocketAddr, headers: &HeaderMap) -> std::net::IpAddr {
    if peer.ip().to_canonical().is_loopback() {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse().ok())
        {
            return ip;
        }
    }
    peer.ip()
}

/// At most `max` requests per key in a minute (or another `window`).
struct Limit {
    max: usize,
    window: Duration,
    seen: std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<std::time::Instant>>>,
}

impl Limit {
    fn new(max: usize) -> Self {
        Self::per(max, Duration::from_secs(60))
    }

    fn per(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            seen: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn check(&self, key: &str) -> bool {
        let now = std::time::Instant::now();
        let window = self.window;
        let mut seen = self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.len() > 50_000 {
            seen.retain(|_, t| t.back().is_some_and(|t| now.duration_since(*t) < window));
        }
        let times = seen.entry(key.to_string()).or_default();
        while times.front().is_some_and(|t| now.duration_since(*t) >= window) {
            times.pop_front();
        }
        if times.len() >= self.max {
            return false;
        }
        times.push_back(now);
        true
    }
}

/// Whether a link signed at `time` may still be taken at `now`.
fn link_signed_lately(time: i64, now: i64) -> bool {
    time <= now + identity::MAX_CLOCK_SKEW && now - time <= LINK_SIGNED_FOR
}

/// Whether `name` may be a player's name (as servers accept new ones).
fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// The HTTP client for GitHub.
pub fn http() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent(concat!("5th-echelon-coordinator/", env!("FE_RELEASE")))
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build()
                .expect("HTTP client")
        })
        .clone()
}

/// Whether `host` may be a server's host name or address.
/// `host` written one way, for telling whether two names are one place: lower case, without
/// a port, brackets or a trailing dot, an address as `IpAddr` writes it. None for a name made
/// only of digits and dots that isn't an address (some resolvers read "167772161" as one).
fn canonical_host(host: &str) -> Option<String> {
    // "[v6]" and "[v6]:port": the address inside; anything else as names are keyed.
    let host = match host.trim().strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        Some((inside, _)) => inside.to_lowercase(),
        None => identity::host_key(host),
    };
    let host = host.trim_end_matches('.');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Some(ip.to_canonical().to_string());
    }
    if host.is_empty() || host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    Some(host.to_string())
}

fn valid_host(host: &str) -> bool {
    (1..=253).contains(&host.len()) && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

/// Whether `c` turns the text after it around, or hides in it (so a name could read as
/// another).
pub(crate) fn hidden_char(c: char) -> bool {
    matches!(c, '\u{200b}' | '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2060}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// Printable text of at most `max` characters, without [`hidden_char`]s.
fn valid_text(text: &str, max: usize) -> bool {
    text.chars().count() <= max && !text.chars().any(|c| c.is_control() || hidden_char(c))
}

/// Whether `s` is spelled like a global id (52 upper-case base32 characters).
/// Cheap: whether it's a valid key is checked when it's linked, and only
/// linked ones are used.
fn spelled_like_global_id(s: &str) -> bool {
    s.len() == 52 && s.bytes().all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Ports {
    api: u16,
    login: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secure: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nat: Option<u16>,
    /// The API over HTTPS, when the server has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_tls: Option<u16>,
}

/// A server's directory entry, as it sends it (and nothing else is stored).
#[derive(Debug, Clone, Deserialize, Serialize)]
struct Listing {
    /// The names players reach it by (for links; not listed).
    #[serde(default, skip_serializing)]
    names: Vec<String>,
    name: String,
    #[serde(default)]
    region: String,
    #[serde(default = "yes")]
    listed: bool,
    host: String,
    #[serde(default)]
    ports: Option<Ports>,
    #[serde(default)]
    version: String,
    #[serde(default)]
    players_online: u32,
    #[serde(default)]
    players_total: u32,
    #[serde(default)]
    friends_mode: String,
    /// Whether it installs the releases rolled out (see [`updates`]).
    #[serde(default)]
    auto_update: bool,
    /// The identities of its players online now (for friends on other servers; not listed).
    #[serde(default, skip_serializing)]
    online: Vec<String>,
}

fn yes() -> bool {
    true
}

impl Listing {
    fn check(&self) -> Result<(), &'static str> {
        if self.name.trim().is_empty() || !valid_text(&self.name, 48) {
            return Err("the name is 1-48 printable characters");
        }
        if !valid_text(&self.region, 32) || !valid_text(&self.version, 32) {
            return Err("region and version are at most 32 printable characters");
        }
        if !valid_host(&self.host) {
            return Err("the host isn't a host name or address");
        }
        if let Some(p) = &self.ports {
            if [Some(p.api), Some(p.login), p.secure, p.content, p.nat, p.api_tls]
                .into_iter()
                .flatten()
                .any(|port| port == 0)
            {
                return Err("port 0");
            }
        }
        if !matches!(self.friends_mode.as_str(), "" | "everyone" | "mutual") {
            return Err("friends_mode is everyone or mutual");
        }
        if self.names.len() > 16 || !self.names.iter().all(|n| valid_host(n)) {
            return Err("at most 16 names, each a host name or address");
        }
        if self.online.len() > MAX_ONLINE || !self.online.iter().all(|id| spelled_like_global_id(id)) {
            return Err("online is at most 20000 identities");
        }
        Ok(())
    }
}

/// Players one server may report online at once.
const MAX_ONLINE: usize = 20_000;
/// How long a player counts as online on a server after its last report (it reports every 30 s).
const ONLINE_FOR: i64 = 90;

/// One server's word that a player is online there.
#[derive(Debug, Clone, Copy)]
struct Seen {
    /// Since when (without a break), and when it last said so.
    since: i64,
    at: i64,
}

pub struct Coordinator {
    pool: SqlitePool,
    /// Who is online where: identity -> server -> since when, and when that server last said
    /// so. Each server's word is kept apart, so one can't overwrite where a player really is.
    presence: std::sync::Mutex<HashMap<String, HashMap<String, Seen>>>,
    /// Each server's names that another server holds (or past the limit), from its last
    /// heartbeat.
    pub(crate) name_clashes: std::sync::Mutex<HashMap<String, Vec<String>>>,
    join_token: String,
    joins: Limit,
    reads: Limit,
    changes: Limit,
    pings: Limit,
    /// When each launcher address last reported a ping to each server.
    ping_reporters: std::sync::Mutex<HashMap<(std::net::IpAddr, String), std::time::Instant>>,
    heartbeats: Limit,
    metrics: Limit,
    /// The release this machine's updater was last asked for, and when.
    own_update_asked: std::sync::Mutex<Option<(String, i64)>>,
    /// Where player addresses are (for launchers' ping reports).
    pub geo: std::sync::OnceLock<Arc<geo::Geo>>,
    /// The coordinator's folder (for its own update requests).
    pub data_dir: std::sync::OnceLock<std::path::PathBuf>,
    /// The admin UI's settings, once it's on.
    pub admin: std::sync::OnceLock<admin::Config>,
    /// What the admin UI's live connections hear about (see `admin::live`).
    pub(crate) live: tokio::sync::broadcast::Sender<admin::live::Event>,
    /// The servers' live numbers (their pulses) of the last half hour, and the recent
    /// events made from them (see `admin::live`). Stored too, and loaded at start.
    pub(crate) pulses: std::sync::Mutex<HashMap<String, admin::live::Pulses>>,
    pub(crate) feed: std::sync::Mutex<std::collections::VecDeque<Value>>,
    pulse_limit: Limit,
    /// Per server: players reports (a roster comes in chunks) and action results.
    player_posts: Limit,
    action_posts: Limit,
    /// Per server: players' session events (see [`sessions`]).
    event_posts: Limit,
    /// Per server: stat writes and stats lookups; the leaderboards answer, kept a minute.
    stat_posts: Limit,
    stat_reads: Limit,
    pub(crate) stats_cache: tokio::sync::Mutex<stats::Cache>,
    /// Per server: new reports (see [`reports`]).
    report_posts: Limit,
    content_posts: Limit,
    /// Per address: players' suggestions for the roadmap (see [`roadmap`]).
    suggestion_posts: Limit,
    /// Players' support messages: per player and per address (see [`support`]).
    support_posts: Limit,
    support_address_posts: Limit,
    /// Support files each address sent today (the day, and bytes as sent), and when a refusal
    /// was last said in the log (once an hour each).
    support_address_bytes: std::sync::Mutex<HashMap<String, (i64, i64)>>,
    support_refusals_said: Limit,
    /// The roadmap and players' suggestions are kept here: on the community network's
    /// coordinator only, whose roadmap every launcher reads (`--roadmap`). Off, its routes
    /// and the admin UI's page aren't there.
    roadmap: std::sync::atomic::AtomicBool,
    /// The names launchers reach this coordinator by (`--name`): what players' signed
    /// requests must be made for. Unset, the request's own Host is taken, which anyone
    /// replaying a signature made for another coordinator can set to that one's name.
    pub names: std::sync::OnceLock<Vec<String>>,
    /// Request bodies being read at once (see [`limits::Budgets`]).
    pub(crate) budgets: limits::Budgets,
    /// The folder the database is in: reports' files go under it.
    files_dir: std::path::PathBuf,
    /// The most the reports' files may take ([`reports::STORAGE_CAP`]; less in tests).
    pub(crate) report_storage_cap: std::sync::atomic::AtomicU64,
    /// Report alerts not posted yet, past a few a minute.
    pub(crate) report_batch: std::sync::Mutex<reports::Batcher>,
}

type Shared = Arc<Coordinator>;
type Answer = (StatusCode, Json<Value>);

/// How often a launcher's address adds a ping sample for a server.
const PING_SAMPLE_EVERY: Duration = Duration::from_secs(10 * 60);

fn ok(v: Value) -> Answer {
    (StatusCode::OK, Json(v))
}

fn fail(status: StatusCode, msg: &str) -> Answer {
    (status, Json(json!({ "error": msg })))
}

/// A request body as `T`. Handlers take the raw body and parse it only once the caller is
/// known (a server's secret checked first), and a body that doesn't parse gets one plain
/// answer: serde's errors would tell a stranger which fields each call takes.
fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Answer> {
    serde_json::from_slice(body).map_err(|_| fail(StatusCode::BAD_REQUEST, "not a valid request"))
}

fn internal(e: impl std::fmt::Display) -> Answer {
    tracing::error!("{e}");
    fail(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

impl Coordinator {
    /// Whether `ip`'s ping to `server` is the first in [`PING_SAMPLE_EVERY`] (and notes it).
    fn first_ping_in_a_while(&self, ip: std::net::IpAddr, server: &str) -> bool {
        let now = std::time::Instant::now();
        let mut seen = self.ping_reporters.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.len() > 50_000 {
            seen.retain(|_, at| now.duration_since(*at) < PING_SAMPLE_EVERY);
        }
        if server.len() > 64 {
            return false;
        }
        match seen.get(&(ip, server.to_string())) {
            Some(at) if now.duration_since(*at) < PING_SAMPLE_EVERY => false,
            _ => {
                seen.insert((ip, server.to_string()), now);
                true
            }
        }
    }

    /// Opens (creating if needed) the database at `path`.
    pub async fn open(path: &str, join_token: String) -> eyre::Result<Self> {
        // WAL: readers don't wait for a writer, and the live backup (Litestream) needs it. The
        // mode stays with the file, so the release before still opens it.
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool = SqlitePool::connect_with(options).await?;
        // A database a newer release migrated still opens: after a rollback, the
        // release before runs on it (migrations only add to the schema).
        let mut migrator = sqlx::migrate!("./migrations");
        migrator.set_ignore_missing(true);
        migrator.run(&pool).await?;
        let c = Self {
            pool,
            join_token,
            presence: std::sync::Mutex::new(HashMap::new()),
            name_clashes: std::sync::Mutex::new(HashMap::new()),
            // Per address: joins (the token is guessed at nowhere near this rate) and the
            // public reads; per server: changes.
            joins: Limit::new(10),
            reads: Limit::new(600),
            changes: Limit::new(2000),
            // Launchers report pings when they look at the directory: a few a minute at most.
            pings: Limit::new(6),
            ping_reporters: std::sync::Mutex::new(HashMap::new()),
            // Per server: a heartbeat every 30 seconds and metrics every minute, with room
            // for a retry.
            heartbeats: Limit::new(6),
            metrics: Limit::new(2),
            own_update_asked: std::sync::Mutex::new(None),
            geo: std::sync::OnceLock::new(),
            data_dir: std::sync::OnceLock::new(),
            admin: std::sync::OnceLock::new(),
            live: tokio::sync::broadcast::channel(256).0,
            pulses: std::sync::Mutex::new(HashMap::new()),
            feed: std::sync::Mutex::new(std::collections::VecDeque::new()),
            // A pulse every ten seconds, with room for a retry.
            pulse_limit: Limit::new(9),
            // A roster of 50,000 players is 25 requests.
            player_posts: Limit::new(40),
            // A batch every five seconds, and a backlog after an outage in a few minutes.
            event_posts: Limit::new(60),
            action_posts: Limit::new(120),
            // A batch of writes every few seconds at most, on average; lookups as players
            // sign in and look at the leaderboards.
            stat_posts: Limit::per(600, Duration::from_secs(3600)),
            stat_reads: Limit::new(120),
            stats_cache: tokio::sync::Mutex::new(stats::Cache::default()),
            report_posts: Limit::per(reports::PER_HOUR, Duration::from_secs(3600)),
            content_posts: Limit::per(content::PER_HOUR, Duration::from_secs(3600)),
            suggestion_posts: Limit::per(roadmap::PER_ADDRESS_A_DAY, Duration::from_secs(86_400)),
            support_posts: Limit::per(support::PER_PLAYER_AN_HOUR, Duration::from_secs(3600)),
            support_address_posts: Limit::per(support::PER_ADDRESS_AN_HOUR, Duration::from_secs(3600)),
            support_address_bytes: std::sync::Mutex::default(),
            support_refusals_said: Limit::per(1, Duration::from_secs(3600)),
            roadmap: std::sync::atomic::AtomicBool::new(false),
            names: std::sync::OnceLock::new(),
            budgets: limits::Budgets::default(),
            files_dir: std::path::Path::new(path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map_or_else(|| std::path::PathBuf::from("."), std::path::Path::to_path_buf),
            report_storage_cap: std::sync::atomic::AtomicU64::new(reports::STORAGE_CAP),
            report_batch: std::sync::Mutex::new(reports::Batcher::default()),
        };
        c.claim_linked_names().await?;
        c.load_live().await?;
        Ok(c)
    }

    /// Releases names claimed for accounts that never linked (a registration
    /// that failed, or someone claiming names they don't use). Run now and then.
    pub async fn sweep(&self) -> sqlx::Result<u64> {
        let done = sqlx::query(
            "DELETE FROM names WHERE claimed_at < ?
               AND NOT EXISTS (SELECT 1 FROM links l WHERE l.global_id = names.global_id AND lower(l.username) = names.name_key)",
        )
        .bind(identity::now() - CLAIM_GRACE_SECS)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }

    /// Removes a member server: its links, and names only it used. For the
    /// operator (`coordinator remove-server`).
    pub async fn remove_server(&self, server_id: &str) -> sqlx::Result<bool> {
        let done = sqlx::query("DELETE FROM servers WHERE id = ?").bind(server_id).execute(&self.pool).await?;
        sqlx::query("DELETE FROM player_content WHERE server_id = ?").bind(server_id).execute(&self.pool).await?;
        self.sweep().await?;
        Ok(done.rows_affected() > 0)
    }

    /// Releases the names `server` reserved that nobody uses: its links made over an hour
    /// ago for identities never seen online anywhere and linked on no other server (throwaway
    /// identities, made to squat names), and the names only those links held. For the
    /// operator (`coordinator purge-names`, or the admin UI). Answers how many links went.
    ///
    /// A real player is online as they link, so their server reports them within 30 seconds
    /// (links from before this was recorded count as seen). One that left sooner, and never
    /// came back, loses their link too. A server that also reports its throwaway identities
    /// online isn't caught by this: remove it instead.
    pub async fn purge_unused_names(&self, server_id: &str) -> sqlx::Result<u64> {
        let done = sqlx::query(
            "DELETE FROM links WHERE server_id = ? AND linked_at < ?
               AND NOT EXISTS (SELECT 1 FROM seen_online s WHERE s.global_id = links.global_id)
               AND NOT EXISTS (SELECT 1 FROM links o WHERE o.global_id = links.global_id AND o.server_id != links.server_id)",
        )
        .bind(server_id)
        .bind(identity::now() - CLAIM_GRACE_SECS)
        .execute(&self.pool)
        .await?;
        self.sweep().await?;
        Ok(done.rows_affected())
    }

    /// Whether `host` is one of `server`'s names.
    async fn owns_host(&self, server: &str, host: &str) -> sqlx::Result<bool> {
        let owner: Option<String> = sqlx::query_scalar("SELECT server_id FROM server_names WHERE name = ?")
            .bind(identity::host_key(host))
            .fetch_optional(&self.pool)
            .await?;
        Ok(owner.as_deref() == Some(server))
    }

    /// Reserves the names of accounts linked before names were reserved,
    /// oldest link first. Harmless to repeat.
    async fn claim_linked_names(&self) -> sqlx::Result<()> {
        let links: Vec<(String, String, i64)> = sqlx::query_as("SELECT global_id, username, linked_at FROM links ORDER BY linked_at")
            .fetch_all(&self.pool)
            .await?;
        for (global_id, username, at) in links {
            self.claim(&global_id, &username, at).await?;
        }
        Ok(())
    }

    /// Claims `name` for `global_id` unless someone else has it. Returns
    /// whether it's theirs now.
    async fn claim(&self, global_id: &str, name: &str, now: i64) -> sqlx::Result<bool> {
        self.claim_for(global_id, name, now, None).await
    }

    async fn claim_for(&self, global_id: &str, name: &str, now: i64, server: Option<&str>) -> sqlx::Result<bool> {
        let key = identity::name_key(name);
        sqlx::query("INSERT OR IGNORE INTO names (name_key, name, global_id, claimed_at, server_id) VALUES (?, ?, ?, ?, ?)")
            .bind(&key)
            .bind(name.trim())
            .bind(global_id)
            .bind(now)
            .bind(server)
            .execute(&self.pool)
            .await?;
        Ok(self.owner(&key).await?.as_deref() == Some(global_id))
    }

    async fn owner(&self, key: &str) -> sqlx::Result<Option<String>> {
        sqlx::query_scalar("SELECT global_id FROM names WHERE name_key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
    }

    /// Releases `global_id`'s names that none of its accounts use any more
    /// (after a rename or an unlink), past the grace for new accounts.
    async fn release_unused(&self, global_id: &str, now: i64) -> sqlx::Result<()> {
        let used: Vec<String> = sqlx::query_scalar("SELECT username FROM links WHERE global_id = ?")
            .bind(global_id)
            .fetch_all(&self.pool)
            .await?;
        let used: Vec<String> = used.iter().map(|u| identity::name_key(u)).collect();
        let owned: Vec<(String, i64)> = sqlx::query_as("SELECT name_key, claimed_at FROM names WHERE global_id = ?")
            .bind(global_id)
            .fetch_all(&self.pool)
            .await?;
        for (key, claimed_at) in owned {
            if !used.contains(&key) && now - claimed_at >= CLAIM_GRACE_SECS {
                sqlx::query("DELETE FROM names WHERE name_key = ? AND global_id = ?")
                    .bind(&key)
                    .bind(global_id)
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// The HTTP API.
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/v1/info", get(info))
            .route("/v1/join", post(join))
            .route("/v1/heartbeat", post(heartbeat))
            .route("/v1/servers", get(servers))
            .route("/v1/roadmap", get(roadmap_public))
            .route("/v1/suggestions", post(suggest))
            .route("/v1/suggestions/mine", get(my_suggestions))
            .route("/v1/reports/mine", get(my_reports))
            .route("/v1/support", post(support_send).layer(DefaultBodyLimit::max(support::MAX_BODY)))
            .route("/v1/support/mine", get(my_support))
            .route("/v1/changes", post(changes))
            .route("/v1/relations/{global_id}", get(relations))
            .route("/v1/names/claim", post(claim_name))
            .route("/v1/names/{name}", get(name_owner))
            .route("/v1/metrics", post(metrics_report))
            .route("/v1/pulse", post(pulse))
            .route("/v1/players", post(players_report).layer(DefaultBodyLimit::max(players::MAX_BODY)))
            .route("/v1/actions/{id}", post(action_result))
            .route("/v1/pings", post(pings))
            .route("/v1/stats", post(stats_report).layer(DefaultBodyLimit::max(stats::MAX_BODY)))
            .route("/v1/stats/players", post(stats_of_players))
            .route("/v1/leaderboards", get(leaderboards))
            .route("/v1/leaderboards/players", post(leaderboard_players))
            .route("/v1/reports", post(report).layer(DefaultBodyLimit::max(reports::MAX_BODY)))
            .route("/v1/content", post(player_content).layer(DefaultBodyLimit::max(content::MAX_BODY)))
            .route("/v1/events", post(events_report).layer(DefaultBodyLimit::max(sessions::MAX_BODY)))
            .layer(DefaultBodyLimit::max(MAX_BODY))
            .layer(axum::middleware::from_fn_with_state(Arc::clone(&self), bounded))
            .with_state(self)
    }

    /// The server a request comes from, by its secret.
    async fn server(&self, headers: &HeaderMap) -> Result<String, Answer> {
        let secret = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "sign in with the server's secret"))?;
        sqlx::query_scalar::<_, String>("SELECT id FROM servers WHERE secret_hash = ?")
            .bind(hash(secret.trim()))
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
            .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "unknown server; join again"))
    }

    /// Records who is online on `server` now: those of `online` linked there (a server only
    /// speaks for its own players). Whoever it no longer lists is offline there. Only a string
    /// lookup per id: linked ids were checked when they linked.
    async fn set_online(&self, server: &str, online: &[String]) -> sqlx::Result<()> {
        let linked: std::collections::HashSet<String> = if online.is_empty() {
            std::collections::HashSet::new()
        } else {
            sqlx::query_scalar("SELECT global_id FROM links WHERE server_id = ?")
                .bind(server)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .collect()
        };
        let now = identity::now();
        let newly: Vec<String> = {
            let mut presence = self.presence.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            // What this server said before (to keep "since"), and anything gone quiet anywhere.
            let mut before: HashMap<String, Seen> = HashMap::new();
            presence.retain(|id, on| {
                if let Some(seen) = on.remove(server) {
                    before.insert(id.clone(), seen);
                }
                on.retain(|_, seen| now - seen.at <= ONLINE_FOR);
                !on.is_empty()
            });
            let mut newly = Vec::new();
            for id in online.iter().filter(|id| linked.contains(*id)) {
                let since = match before.get(id) {
                    Some(seen) if now - seen.at <= ONLINE_FOR => seen.since,
                    _ => {
                        newly.push(id.clone());
                        now
                    }
                };
                presence.entry(id.clone()).or_default().insert(server.to_string(), Seen { since, at: now });
            }
            newly
        };
        // Where each player has been seen online, for vouching friendships (see `vouched`).
        if !newly.is_empty() {
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            for id in &newly {
                sqlx::query("INSERT OR IGNORE INTO seen_online (global_id, server_id, first_seen) VALUES (?, ?, ?)")
                    .bind(id)
                    .bind(server)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }

    /// The servers other than `except` that say `global_id` is online on them now, each with
    /// since when.
    fn online_on(&self, global_id: &str, except: &str) -> Vec<(String, i64)> {
        let now = identity::now();
        let presence = self.presence.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        presence
            .get(global_id)
            .into_iter()
            .flatten()
            .filter(|(on, seen)| *on != except && now - seen.at <= ONLINE_FOR)
            .map(|(on, seen)| (on.clone(), seen.since))
            .collect()
    }

    /// Whether a server that said `a` and `b` are friends has seen both of them online (a
    /// friendship from before servers were recorded: any server). A member can make up a
    /// friendship between any two players linked on it; this at least needs it to say both
    /// played there.
    async fn vouched(&self, a: &str, b: &str) -> sqlx::Result<bool> {
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM seen_online x JOIN seen_online y ON y.server_id = x.server_id
              WHERE x.global_id = ? AND y.global_id = ?
                AND EXISTS (SELECT 1 FROM friendship_servers f WHERE f.a = ? AND f.b = ? AND f.server_id IN (x.server_id, '*'))",
        )
        .bind(a)
        .bind(b)
        .bind(a)
        .bind(b)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    /// Where `friend` is playing, for `me`'s friend list on the server `asking` (when it's
    /// another one): that server's name, region and host, and the friend's name there.
    ///
    /// A member server can say that anyone linked on it is online there. So each server's
    /// word is kept apart, and when several say so, the one where the friend linked most
    /// recently wins (a link needs the friend's own, recent signature), then the one they
    /// came on last. Unlisted servers aren't given out, and only for a vouched friendship.
    async fn elsewhere(&self, me: &str, friend: &str, asking: &str) -> sqlx::Result<Option<Value>> {
        let on = self.online_on(friend, asking);
        if on.is_empty() || !self.vouched(me, friend).await? {
            return Ok(None);
        }
        let links: Vec<(String, Option<String>, String, i64)> =
            sqlx::query_as("SELECT s.id, s.listing, l.username, l.linked_at FROM links l JOIN servers s ON s.id = l.server_id WHERE l.global_id = ?")
                .bind(friend)
                .fetch_all(&self.pool)
                .await?;
        let best = links
            .into_iter()
            .filter_map(|(id, listing, username, linked_at)| {
                let since = on.iter().find(|(s, _)| *s == id)?.1;
                let listing = serde_json::from_str::<Listing>(&listing?).ok().filter(|l| l.listed)?;
                Some(((linked_at, since), listing, username))
            })
            .max_by_key(|(order, ..)| *order);
        Ok(best.map(|(_, listing, username)| json!({ "username": username, "server": listing.name, "region": listing.region, "host": listing.host })))
    }

    /// Records the names `server` goes by (first come, first served, at most
    /// [`MAX_SERVER_NAMES`]). Answers why any of them isn't its, for its log and the admin UI.
    async fn claim_server_names(&self, server: &str, listing: &Listing) -> sqlx::Result<Vec<String>> {
        let mut keys: Vec<String> = listing.names.iter().chain([&listing.host]).map(|n| identity::host_key(n)).collect();
        keys.sort();
        keys.dedup();
        let mut held: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM server_names WHERE server_id = ?")
            .bind(server)
            .fetch_one(&self.pool)
            .await?;
        let mut clashes = Vec::new();
        for key in keys {
            let owner = |key: String| async move {
                sqlx::query_scalar::<_, String>("SELECT server_id FROM server_names WHERE name = ?")
                    .bind(key)
                    .fetch_optional(&self.pool)
                    .await
            };
            let mut found = owner(key.clone()).await?;
            if found.is_none() && held < MAX_SERVER_NAMES {
                sqlx::query("INSERT OR IGNORE INTO server_names (name, server_id) VALUES (?, ?)")
                    .bind(&key)
                    .bind(server)
                    .execute(&self.pool)
                    .await?;
                held += 1;
                found = owner(key.clone()).await?;
            }
            match found {
                Some(owner) if owner == server => {}
                Some(_) => clashes.push(format!("{key} is another member server's name: players' signatures for it aren't taken from this server")),
                None => clashes.push(format!(
                    "{key} wasn't recorded: a server has at most {MAX_SERVER_NAMES} names (the operator can remove it and let it join again)"
                )),
            }
        }
        let mut all = self.name_clashes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if all.get(server) != Some(&clashes) {
            for clash in &clashes {
                tracing::warn!("server {server}: {clash}");
            }
            if clashes.is_empty() {
                all.remove(server);
            } else {
                all.insert(server.to_string(), clashes.clone());
            }
        }
        Ok(clashes)
    }

    async fn linked(&self, server: &str, global_id: &str) -> sqlx::Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM links WHERE server_id = ? AND global_id = ?")
            .bind(server)
            .bind(global_id)
            .fetch_one(&self.pool)
            .await?
            > 0)
    }

    /// Applies one change from `server`; the error is for that change alone.
    /// A link answers whether its name clashes with another player's.
    async fn apply(&self, server: &str, change: &Value) -> Result<Value, String> {
        let text = |k: &str| change[k].as_str().unwrap_or_default().to_string();
        let now = identity::now();
        let result: Result<Value, String> = match change["op"].as_str().unwrap_or_default() {
            "link" => {
                let (global_id, username, signature, host) = (text("global_id"), text("username"), text("signature"), text("host"));
                let time = change["time"].as_i64().unwrap_or_default();
                if !identity::is_global_id(&global_id) || !valid_name(&username) {
                    return Err("not an identity, or not a name".into());
                }
                // Signed for a host this server goes by: not for another server's.
                if !self.owns_host(server, &host).await.map_err(|e| e.to_string())? {
                    return Err(format!("{host} isn't one of this server's names"));
                }
                // The server checked the time when the player linked (within minutes); a change
                // may arrive days later, after an outage, so here it's only a bound: an old
                // signature can't be brought back.
                if !link_signed_lately(time, now) {
                    return Err("the player's signature is too old, or from the future".into());
                }
                if !identity::verify(&global_id, &identity::link_message(&host, &username, time), &signature) {
                    return Err("the player's signature doesn't match".into());
                }
                // The same link again (a retry) changes nothing: it isn't a new link, and keeps
                // its time.
                if !self.has_link(server, &global_id, &username).await.map_err(|e| e.to_string())? {
                    let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(|e| e.to_string())?;
                    // One identity per account and one account per identity, on each server.
                    sqlx::query("DELETE FROM links WHERE server_id = ? AND (username = ? OR global_id = ?)")
                        .bind(server)
                        .bind(&username)
                        .bind(&global_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| e.to_string())?;
                    sqlx::query("INSERT INTO links (global_id, server_id, username, linked_at) VALUES (?, ?, ?, ?)")
                        .bind(&global_id)
                        .bind(server)
                        .bind(&username)
                        .bind(now)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| e.to_string())?;
                    tx.commit().await.map_err(|e| e.to_string())?;
                }
                // The name comes with the link: the player's own, or someone else's already.
                let ours = self.claim(&global_id, &username, now).await.map_err(|e| e.to_string())?;
                self.release_unused(&global_id, now).await.map_err(|e| e.to_string())?;
                Ok(json!({ "conflict": !ours }))
            }
            "unlink" => {
                let global_id = text("global_id");
                sqlx::query("DELETE FROM links WHERE server_id = ? AND global_id = ?")
                    .bind(server)
                    .bind(&global_id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?;
                self.release_unused(&global_id, now).await.map_err(|e| e.to_string())?;
                Ok(json!({}))
            }
            "friends" => {
                let (a, b) = (text("a"), text("b"));
                self.require_linked(server, &[&a, &b]).await?;
                if a == b {
                    return Err("a player can't befriend themselves".into());
                }
                let friends = change["friends"].as_bool().unwrap_or(false);
                // A block anywhere wins: no friendship while either blocks the other.
                if friends && self.blocked_either_way(&a, &b).await.map_err(|e| e.to_string())? {
                    return Err("one of them blocked the other".into());
                }
                self.set_friends(&a, &b, friends, now, server).await.map(|()| json!({})).map_err(|e| e.to_string())
            }
            "block" => {
                let (from, to) = (text("from"), text("to"));
                self.require_linked(server, &[&from, &to]).await?;
                let blocked = change["blocked"].as_bool().unwrap_or(false);
                sqlx::query(
                    "INSERT INTO blocks (from_id, to_id, blocked, updated) VALUES (?, ?, ?, ?)
                     ON CONFLICT(from_id, to_id) DO UPDATE SET blocked = excluded.blocked, updated = excluded.updated",
                )
                .bind(&from)
                .bind(&to)
                .bind(i64::from(blocked))
                .bind(now)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
                if blocked {
                    self.set_friends(&from, &to, false, now, server).await.map_err(|e| e.to_string())?;
                }
                Ok(json!({}))
            }
            other => Err(format!("unknown change {other:?}")),
        };
        result
    }

    async fn require_linked(&self, server: &str, ids: &[&str]) -> Result<(), String> {
        for id in ids {
            if !self.linked(server, id).await.map_err(|e| e.to_string())? {
                return Err(format!("{} isn't linked on this server", identity::short(id)));
            }
        }
        Ok(())
    }

    async fn blocked_either_way(&self, a: &str, b: &str) -> sqlx::Result<bool> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM blocks WHERE blocked = 1 AND ((from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?))")
                .bind(a)
                .bind(b)
                .bind(b)
                .bind(a)
                .fetch_one(&self.pool)
                .await?
                > 0,
        )
    }

    /// Records a friendship change from `server`, and which servers said they're friends
    /// (see `vouched`).
    async fn set_friends(&self, a: &str, b: &str, friends: bool, now: i64, server: &str) -> sqlx::Result<()> {
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT INTO friendships (a, b, friends, updated) VALUES (?, ?, ?, ?)
             ON CONFLICT(a, b) DO UPDATE SET friends = excluded.friends, updated = excluded.updated",
        )
        .bind(a)
        .bind(b)
        .bind(i64::from(friends))
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let sources = if friends {
            sqlx::query("INSERT OR IGNORE INTO friendship_servers (a, b, server_id) VALUES (?, ?, ?)")
                .bind(a)
                .bind(b)
                .bind(server)
        } else {
            sqlx::query("DELETE FROM friendship_servers WHERE a = ? AND b = ?").bind(a).bind(b)
        };
        sources.execute(&mut *tx).await?;
        tx.commit().await
    }

    async fn has_link(&self, server: &str, global_id: &str, username: &str) -> sqlx::Result<bool> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM links WHERE server_id = ? AND global_id = ? AND username = ?")
                .bind(server)
                .bind(global_id)
                .bind(username)
                .fetch_one(&self.pool)
                .await?
                > 0,
        )
    }

    /// Whether `server` may make the new links in `changes` now (within
    /// [`MAX_NEW_LINKS_PER_HOUR`] and [`MAX_LINKS_PER_SERVER`]). Repeats of links it has
    /// don't count.
    async fn links_allowed(&self, server: &str, changes: &[Value]) -> sqlx::Result<Result<(), &'static str>> {
        let mut new = 0;
        for change in changes.iter().filter(|c| c["op"] == "link") {
            let text = |k: &str| change[k].as_str().unwrap_or_default();
            if !self.has_link(server, text("global_id"), text("username")).await? {
                new += 1;
            }
        }
        if new == 0 {
            return Ok(Ok(()));
        }
        let (lately, all): (i64, i64) = sqlx::query_as("SELECT COALESCE(SUM(linked_at > ?), 0), COUNT(*) FROM links WHERE server_id = ?")
            .bind(identity::now() - 3600)
            .bind(server)
            .fetch_one(&self.pool)
            .await?;
        Ok(if all + new > MAX_LINKS_PER_SERVER {
            Err("this server has as many links as a server may have")
        } else if lately + new > MAX_NEW_LINKS_PER_HOUR {
            Err("too many new links this hour; send them again later")
        } else {
            Ok(())
        })
    }
}

/// The stored form of a server secret.
fn hash(secret: &str) -> String {
    identity::base32_encode(&sha2::Sha256::digest(secret.as_bytes()))
}

/// Compares secrets in time that doesn't depend on where they differ.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn info(State(c): State<Shared>) -> Answer {
    let servers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM servers").fetch_one(&c.pool).await.unwrap_or(0);
    ok(json!({ "name": "5th Echelon coordinator", "version": env!("FE_RELEASE"), "servers": servers }))
}

#[derive(Deserialize)]
struct JoinRequest {
    token: String,
    server_id: String,
}

async fn join(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    if !c.joins.check(&limit_key(client_ip(peer, &headers))) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many attempts; try again in a minute");
    }
    let req: JoinRequest = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if c.join_token.is_empty() || !same_secret(req.token.trim(), &c.join_token) {
        return fail(StatusCode::FORBIDDEN, "wrong join token");
    }
    let id = req.server_id.trim();
    if id.is_empty() || id.len() > 64 || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        return fail(StatusCode::BAD_REQUEST, "server ids are up to 64 letters, digits and -");
    }
    // An id that already joined is that server's: a new secret for it needs its current one
    // (the token alone would let any holder take over any member, whose ids are public).
    let exists: Option<String> = match sqlx::query_scalar("SELECT id FROM servers WHERE id = ?").bind(id).fetch_optional(&c.pool).await {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    if exists.is_some() && c.server(&headers).await.ok().as_deref() != Some(id) {
        tracing::warn!("refused a join as the existing server {id}");
        return fail(
            StatusCode::CONFLICT,
            "that server id has joined already; joining again needs its current secret (or the operator removes it)",
        );
    }
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    let secret = identity::base32_encode(&bytes);
    // Joining again (a lost credentials file) replaces the secret; holders of the token are
    // trusted with that.
    let done = sqlx::query(
        "INSERT INTO servers (id, secret_hash, joined_at) VALUES (?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET secret_hash = excluded.secret_hash",
    )
    .bind(id)
    .bind(hash(&secret))
    .bind(identity::now())
    .execute(&c.pool)
    .await;
    match done {
        Ok(_) => {
            tracing::info!("server {id} joined");
            ok(json!({ "secret": secret }))
        }
        Err(e) => internal(e),
    }
}

async fn heartbeat(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let mut listing: Listing = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !c.heartbeats.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "a heartbeat every 30 seconds is enough");
    }
    if let Err(why) = listing.check() {
        return fail(StatusCode::BAD_REQUEST, why);
    }
    // Its names, first come, first served: a name another server has stays theirs, and the
    // server hears about it (newer servers log it).
    let mut clashes = match c.claim_server_names(&server, &listing).await {
        Ok(clashes) => clashes,
        Err(e) => return internal(e),
    };
    // Listed under its own address only: one another member holds would send players to
    // that server under this one's name. Compared written one way ([`canonical_host`]), so
    // "server-a." or "[::1]:80" isn't another name for the same place.
    // Several members' names may come down to one place ("server-a" and "server-a."): it's
    // the first to claim one's (names are kept in the order they were claimed), so a later
    // spelling can't take a member's address from it.
    let names: Vec<(String, String)> = match sqlx::query_as("SELECT name, server_id FROM server_names ORDER BY rowid").fetch_all(&c.pool).await {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    let host = canonical_host(&listing.host);
    let owner = host
        .as_ref()
        .and_then(|h| names.iter().find(|(n, _)| canonical_host(n).as_ref() == Some(h)).map(|(_, owner)| owner));
    if listing.listed && owner != Some(&server) {
        listing.listed = false;
        clashes.push(format!("{} isn't this server's name: not listed in the directory", listing.host));
    }
    // A count the directory can sort by, but not past the server's own players, nor more than
    // any server here holds (one said 4,294,967,295 to top the list).
    // (A server too old to say its total says 0.)
    if listing.players_total > 0 {
        listing.players_online = listing.players_online.min(listing.players_total);
    }
    listing.players_online = listing.players_online.min(MAX_LISTED_PLAYERS);
    let text = match serde_json::to_string(&listing) {
        Ok(t) => t,
        Err(e) => return internal(e),
    };
    if let Err(e) = sqlx::query("UPDATE servers SET listing = ?, last_seen = ? WHERE id = ?")
        .bind(text)
        .bind(identity::now())
        .bind(&server)
        .execute(&c.pool)
        .await
    {
        return internal(e);
    }
    if let Err(e) = c.set_online(&server, &listing.online).await {
        return internal(e);
    }
    c.publish(admin::live::Event::Network);
    let mut answer = json!({});
    if !clashes.is_empty() {
        answer["warnings"] = json!(clashes);
    }
    // The release this server should install now, if any.
    match c.update_for(&server, &listing.version, u64::from(listing.players_online)).await {
        Ok(Some(version)) => answer["update"] = json!({ "version": version }),
        Ok(None) => {}
        Err(e) => return internal(e),
    }
    // Its maintenance windows and the network's, for its games' overlay.
    match c.heartbeat_maintenance(&server, identity::now()).await {
        Ok(windows) => answer["maintenance"] = windows,
        Err(e) => return internal(e),
    }
    ok(answer)
}

/// A server's metrics report (see [`metrics`]).
async fn metrics_report(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let report: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !c.metrics.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "metrics every minute are enough");
    }
    if report.to_string().len() > metrics::MAX_REPORT || !report["metrics"].is_object() {
        return fail(StatusCode::BAD_REQUEST, "not a metrics report");
    }
    match c.record_metrics(&server, &report).await {
        Ok(()) => {
            c.publish(admin::live::Event::Metrics);
            ok(json!({}))
        }
        Err(e) => internal(e),
    }
}

/// A server's live numbers, every ten seconds, for the admin UI. The answer carries the
/// actions admins asked of it (see [`players`]).
async fn pulse(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let p: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !c.pulse_limit.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "a pulse every ten seconds is enough");
    }
    // The list of who's online (for the admin UI's map) makes up most of it: 300 players,
    // checked and capped when it's kept (admin::live).
    if p.to_string().len() > 128 * 1024 || !p["players"].is_object() {
        return fail(StatusCode::BAD_REQUEST, "not a pulse");
    }
    if let Err(e) = c.record_pulse(&server, &p).await {
        return internal(e);
    }
    let actions = match c.pending_actions(&server).await {
        Ok(actions) => actions,
        Err(e) => return internal(e),
    };
    // Its players online with answers from support they haven't read: the overlay says so.
    let online: Vec<i64> = p["online"].as_array().into_iter().flatten().filter_map(|o| o["id"].as_i64()).collect();
    let support = if c.has_roadmap() {
        c.support_unread_for(&server, &online).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    ok(json!({ "actions": actions, "support": support }))
}

/// A server's players' session events (see [`sessions`]).
async fn events_report(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let events: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !c.event_posts.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many event reports; send the rest in a minute");
    }
    match events["events"].as_array() {
        None => return fail(StatusCode::BAD_REQUEST, "not an events report"),
        Some(list) if list.len() > sessions::MAX_EVENTS => return fail(StatusCode::PAYLOAD_TOO_LARGE, "at most 500 events a request"),
        Some(_) => {}
    }
    match c.record_events(&server, &events).await {
        Ok(kept) => ok(json!({ "kept": kept })),
        Err(e) => internal(e),
    }
}

/// A server's players and play sessions (see [`players`]).
async fn players_report(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let roster: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !c.player_posts.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many players reports; send the rest in a minute");
    }
    let count = |k: &str| roster[k].as_array().map_or(0, Vec::len);
    if !roster["players"].is_array() && !roster["sessions"].is_array() {
        return fail(StatusCode::BAD_REQUEST, "not a players report");
    }
    if count("players") > players::MAX_PLAYERS || count("sessions") > players::MAX_SESSIONS {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "at most 2000 players and 5000 sessions a request");
    }
    match c.record_players(&server, &roster).await {
        Ok(answer) => ok(answer),
        Err(e) => internal(e),
    }
}

/// What came of an action a server was asked to carry out (see [`players`]).
async fn action_result(State(c): State<Shared>, headers: HeaderMap, Path(id): Path<i64>, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let result: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !c.action_posts.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many results; slow down");
    }
    if !result["ok"].is_boolean() {
        return fail(StatusCode::BAD_REQUEST, "not an action result");
    }
    match c.action_result(&server, id, &result).await {
        Ok(true) => ok(json!({ "ok": true })),
        // Another server's, or none: nothing said about which.
        Ok(false) => fail(StatusCode::NOT_FOUND, "no such action for this server"),
        Err(e) => internal(e),
    }
}

/// A server's stat writes (see [`stats`]).
async fn stats_report(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let batch: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !c.stat_posts.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many stats requests this hour; send the rest later");
    }
    let (Some(epoch), Some(writes)) = (batch["epoch"].as_str().filter(|e| stats::valid_epoch(e)), batch["writes"].as_array()) else {
        return fail(StatusCode::BAD_REQUEST, "not a stats report");
    };
    if writes.len() > stats::MAX_WRITES {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "at most 1000 writes a request");
    }
    match c.apply_stats(&server, &epoch.to_ascii_lowercase(), writes).await {
        Ok(last_id) => ok(json!({ "ok": true, "last_id": last_id })),
        Err(e) => internal(e),
    }
}

/// A player's report, forwarded by their server (see [`reports`]).
/// What a player's game uploaded (content.rs): kept, the latest each, for the admins.
async fn player_content(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let v: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !c.content_posts.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too much content this hour; send the rest later");
    }
    let checked = match tokio::task::spawn_blocking(move || content::check(&v, identity::now())).await {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    let content = match checked {
        Ok(c) => c,
        Err(why) => {
            tracing::warn!("server {server}: refused content: {why}");
            return fail(StatusCode::BAD_REQUEST, &why);
        }
    };
    match c.store_content(&server, &content).await {
        Ok(content::Stored::Kept) => ok(json!({ "ok": true })),
        // Both sent again later (a 5xx), not dropped: the player list catches up, or room is made.
        Ok(content::Stored::UnknownPlayer) => fail(StatusCode::SERVICE_UNAVAILABLE, "that player isn't known here yet; send it later"),
        Ok(content::Stored::Full) => fail(StatusCode::INSUFFICIENT_STORAGE, "this server's snapshots take all the room they may"),
        Err(e) => internal(e),
    }
}

async fn report(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let v: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let Some(id) = v["id"].as_str().filter(|id| reports::valid_id(id)) else {
        return fail(StatusCode::BAD_REQUEST, "id is 32 hex digits");
    };
    // The same report again (a retry): taken already.
    match c.report_exists(id).await {
        Ok(true) => return ok(json!({ "ok": true })),
        Ok(false) => {}
        Err(e) => return internal(e),
    }
    if !c.report_posts.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many reports this hour; send the rest later");
    }
    // Decompressing the files to check them is work for a blocking thread.
    let checked = match tokio::task::spawn_blocking(move || reports::check(&v, identity::now())).await {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    let r = match checked {
        Ok(r) => r,
        Err(why) => {
            tracing::warn!("server {server}: refused a report: {why}");
            return fail(StatusCode::BAD_REQUEST, &why);
        }
    };
    match c.store_report(&server, &r).await {
        Ok(true) => {
            tracing::info!("server {server}: a report from {} ({} files)", r.player_name, r.files.len());
            c.publish(admin::live::Event::Report);
            c.alert_report(&server, &r).await;
            ok(json!({ "ok": true }))
        }
        Ok(false) => ok(json!({ "ok": true })),
        Err(e) => internal(e),
    }
}

/// The ids a stats lookup asks about, or the answer to give.
async fn stats_lookup(c: &Coordinator, headers: &HeaderMap, body: &[u8]) -> Result<Vec<Value>, Answer> {
    let server = c.server(headers).await?;
    let req: Value = parse(body)?;
    if !c.stat_reads.check(&server) {
        return Err(fail(StatusCode::TOO_MANY_REQUESTS, "too many stats lookups; slow down"));
    }
    let Some(ids) = req["ids"].as_array() else {
        return Err(fail(StatusCode::BAD_REQUEST, "not a stats lookup"));
    };
    if ids.len() > stats::MAX_IDS {
        return Err(fail(StatusCode::PAYLOAD_TOO_LARGE, "at most 200 ids a request"));
    }
    Ok(ids.clone())
}

/// Everything kept for some people (see [`stats`]).
async fn stats_of_players(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    match stats_lookup(&c, &headers, &body).await {
        Ok(ids) => c.player_stats(&ids).await.map_or_else(internal, ok),
        Err(e) => e,
    }
}

/// Where some people are on the leaderboards (see [`stats`]).
async fn leaderboard_players(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    match stats_lookup(&c, &headers, &body).await {
        Ok(ids) => c.player_ranks(&ids).await.map_or_else(internal, ok),
        Err(e) => e,
    }
}

/// The top of every leaderboard (see [`stats`]).
async fn leaderboards(State(c): State<Shared>, headers: HeaderMap, axum::extract::RawQuery(query): axum::extract::RawQuery) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    if !c.stat_reads.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "the leaderboards change once a minute; slow down");
    }
    let count = query
        .unwrap_or_default()
        .split('&')
        .find_map(|kv| kv.strip_prefix("count="))
        .map_or(Some(stats::MAX_COUNT), |n| n.parse::<usize>().ok().filter(|n| (1..=stats::MAX_COUNT).contains(n)));
    let Some(count) = count else {
        return fail(StatusCode::BAD_REQUEST, "count is 1 to 100");
    };
    c.leaderboards(count).await.map_or_else(internal, ok)
}

/// A launcher's pings to the servers in the directory.
async fn pings(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let ip = client_ip(peer, &headers);
    if !c.pings.check(&limit_key(ip)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many reports");
    }
    let body: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // One sample per address and server in a while: however often an address reports, it
    // can't outweigh the players in its city.
    let list: Vec<Value> = body["pings"]
        .as_array()
        .map(|l| {
            l.iter()
                .take(32)
                .filter(|p| p["server"].as_str().is_some_and(|s| c.first_ping_in_a_while(ip, s)))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    match c.record_player_pings(ip, &list).await {
        Ok(n) => ok(json!({ "recorded": n })),
        Err(e) => internal(e),
    }
}

/// The directory: servers that are listed and have sent a heartbeat lately,
/// most players online first.
async fn servers(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap) -> Answer {
    if !c.reads.check(&limit_key(client_ip(peer, &headers))) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    let since = identity::now() - i64::try_from(LISTED_FOR.as_secs()).unwrap_or(120);
    let rows: Vec<(String, Option<String>, Option<i64>)> = match sqlx::query_as("SELECT id, listing, last_seen FROM servers WHERE last_seen >= ?")
        .bind(since)
        .fetch_all(&c.pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => return internal(e),
    };
    let now = identity::now();
    let rollout = match c.rollout().await {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    // Maintenance booked: launchers warn the players of a server on the day.
    let (mut maintenance, network_maintenance) = match c.directory_maintenance(now).await {
        Ok(m) => m,
        Err(e) => return internal(e),
    };
    let mut list: Vec<Value> = rows
        .into_iter()
        .filter_map(|(id, listing, last_seen)| {
            let mut v: Value = serde_json::from_str(&listing?).ok()?;
            if !v.is_object() || !v["listed"].as_bool().unwrap_or(true) {
                return None;
            }
            // Members keep up with the network's releases, or leave the directory.
            let (version, auto_update) = (v["version"].as_str().unwrap_or_default(), v["auto_update"].as_bool().unwrap_or(false));
            if updates::delisted(&rollout, version, auto_update, now).is_some() {
                return None;
            }
            if let Some(windows) = maintenance.remove(&id) {
                v["maintenance"] = json!(windows);
            }
            v["id"] = json!(id);
            v["seen_secs_ago"] = json!(now - last_seen.unwrap_or(now));
            Some(v)
        })
        .collect();
    list.sort_by_key(|v| std::cmp::Reverse(v["players_online"].as_u64().unwrap_or(0)));
    // A release going out: launchers tell their players when their server updates.
    let mut answer = json!({ "servers": list });
    if let Some(rollout) = updates::public_rollout(&rollout) {
        answer["rollout"] = rollout;
    }
    if let Some(window) = network_maintenance {
        answer["network_maintenance"] = json!(window);
    }
    ok(answer)
}

/// The project's roadmap, as launchers show it (see [`roadmap`]).
impl Coordinator {
    /// Keeps the roadmap and players' suggestions here (see [`Coordinator::roadmap`]).
    pub fn enable_roadmap(&self) {
        self.roadmap.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether `address` may send `bytes` more of support files today
    /// ([`support::ADDRESS_FILES_A_DAY`]); if so, they're counted.
    fn support_bytes_allowed(&self, address: &str, bytes: i64, now: i64) -> bool {
        if bytes == 0 {
            return true;
        }
        let day = now / 86_400;
        let mut sent = self.support_address_bytes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if sent.len() > 50_000 {
            sent.retain(|_, (d, _)| *d == day);
        }
        let today = sent.entry(address.to_string()).or_insert((day, 0));
        if today.0 != day {
            *today = (day, 0);
        }
        if today.1 + bytes > support::ADDRESS_FILES_A_DAY {
            return false;
        }
        today.1 += bytes;
        true
    }

    /// Whether this coordinator keeps the roadmap.
    pub fn has_roadmap(&self) -> bool {
        self.roadmap.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The name a player's signed request was made for: the request's Host, if it's one of
    /// this coordinator's [`names`](Self::names) (or there are none set); else none, and the
    /// signature is for another coordinator.
    fn signed_for(&self, headers: &HeaderMap) -> Option<String> {
        let host = headers.get(axum::http::header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
        match self.names.get() {
            Some(names) if !names.is_empty() => names.contains(&identity::host_key(host)).then(|| host.to_string()),
            _ => Some(host.to_string()),
        }
    }
}

/// What a coordinator without the roadmap answers its routes.
fn no_roadmap() -> Answer {
    fail(StatusCode::NOT_FOUND, "no roadmap here")
}

async fn roadmap_public(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap) -> Answer {
    if !c.has_roadmap() {
        return no_roadmap();
    }
    if !c.reads.check(&limit_key(client_ip(peer, &headers))) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    c.public_roadmap().await.map_or_else(internal, ok)
}

/// A player's suggestion for the roadmap, from their launcher, signed with their identity.
async fn suggest(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    if !c.has_roadmap() {
        return no_roadmap();
    }
    if body.len() > 8 * 1024 {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "too long");
    }
    let mut s: roadmap::Suggestion = match parse(&body) {
        Ok(s) => s,
        Err(a) => return a,
    };
    let now = identity::now();
    if let Err(why) = s.check(now) {
        return fail(StatusCode::BAD_REQUEST, why);
    }
    // The name isn't signed: a linked player's is the one they linked by, anyone else is "a
    // player", so nobody can suggest as somebody else.
    s.name = match c.linked_name(&s.identity).await {
        Ok(Some(name)) => name,
        Ok(None) => "a player".into(),
        Err(e) => return internal(e),
    };
    if !c.suggestion_posts.check(&limit_key(client_ip(peer, &headers))) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "Too many suggestions from this address today; try again tomorrow.");
    }
    match c.add_suggestion(&s, now).await {
        Ok(Ok(id)) => {
            c.publish(admin::live::Event::Roadmap);
            ok(json!({ "id": id }))
        }
        Ok(Err(why)) => fail(StatusCode::TOO_MANY_REQUESTS, &why),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct Signed {
    identity: String,
    time: i64,
    signature: String,
}

/// A player's own suggestions, with the admins' replies (signed: only they read them).
async fn my_suggestions(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap, Query(q): Query<Signed>) -> Answer {
    if !c.has_roadmap() {
        return no_roadmap();
    }
    if !c.reads.check(&limit_key(client_ip(peer, &headers))) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    if !identity::is_global_id(&q.identity) || !identity::fresh(q.time, identity::now()) || !identity::verify(&q.identity, &identity::suggestions_message(q.time), &q.signature) {
        return fail(StatusCode::FORBIDDEN, "the signature doesn't match");
    }
    c.suggestions_of(&q.identity).await.map_or_else(internal, ok)
}

/// A player's message to the admins (see [`support`]), from their launcher, signed with
/// their identity for this coordinator.
async fn support_send(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    if !c.has_roadmap() {
        return no_roadmap();
    }
    let address = limit_key(client_ip(peer, &headers));
    if !c.support_address_posts.check(&address) {
        if c.support_refusals_said.check(&format!("address/{address}")) {
            tracing::warn!("support: refused messages from {address}: too many this hour (said once an hour)");
        }
        return fail(StatusCode::TOO_MANY_REQUESTS, "Too many messages from this address; try again in an hour.");
    }
    let sent: support::Sent = match parse(&body) {
        Ok(s) => s,
        Err(a) => return a,
    };
    let Some(host) = c.signed_for(&headers) else {
        return fail(StatusCode::FORBIDDEN, "the signature doesn't match");
    };
    let now = identity::now();
    // Unpacking the files to check them: off the async workers.
    let checked = tokio::task::spawn_blocking(move || sent.check(&host, now).map(|files| (sent, files))).await;
    let (mut sent, files) = match checked {
        Ok(Ok(v)) => v,
        Ok(Err(why)) => return fail(StatusCode::BAD_REQUEST, why),
        Err(e) => return internal(e),
    };
    // The name isn't signed: the one they linked by, not what the launcher says.
    match c.linked_name(&sent.identity).await {
        Ok(Some(name)) => sent.name = name,
        Ok(None) => {
            return fail(
                StatusCode::FORBIDDEN,
                "Support is for players of the community network: connect to one of its servers first.",
            )
        }
        Err(e) => return internal(e),
    }
    if !c.support_posts.check(&sent.identity) {
        if c.support_refusals_said.check(&format!("player/{}", sent.identity)) {
            tracing::warn!("support: refused messages from {}: too many this hour (said once an hour)", identity::short(&sent.identity));
        }
        return fail(
            StatusCode::TOO_MANY_REQUESTS,
            "That's a lot of messages in an hour; the admins will answer what you sent. Try again later.",
        );
    }
    // Past its address's files for the day, a message keeps its text only.
    let size: i64 = files.iter().map(|f| f.gzip.len() as i64).sum();
    let address_allows = c.support_bytes_allowed(&address, size, now);
    if !address_allows && c.support_refusals_said.check(&format!("bytes/{address}")) {
        tracing::warn!("support: {address} sent its files for the day; messages from it keep their text only (said once an hour)");
    }
    let files = if address_allows { files } else { vec![] };
    let dropped = !address_allows && size > 0;
    match c.add_support_message(&sent, &files, now).await {
        Ok((id, files_kept)) => {
            let files_kept = files_kept && !dropped;
            c.publish(admin::live::Event::Support);
            let first = sent.text.lines().next().unwrap_or_default();
            c.notify_support(&sent.name, first).await;
            ok(json!({ "id": id, "files_kept": files_kept }))
        }
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct SupportRead {
    identity: String,
    time: i64,
    signature: String,
    /// "1": the player's Support page shows it, so the answers are read.
    #[serde(default)]
    read: String,
}

/// A player's conversation with the admins (signed for this coordinator: only they read it).
async fn my_support(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap, Query(q): Query<SupportRead>) -> Answer {
    if !c.has_roadmap() {
        return no_roadmap();
    }
    if !c.reads.check(&limit_key(client_ip(peer, &headers))) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    let Some(host) = c.signed_for(&headers) else {
        return fail(StatusCode::FORBIDDEN, "the signature doesn't match");
    };
    let host = host.as_str();
    if !identity::is_global_id(&q.identity)
        || !identity::fresh(q.time, identity::now())
        || !identity::verify(&q.identity, &identity::support_read_message(host, q.time), &q.signature)
    {
        return fail(StatusCode::FORBIDDEN, "the signature doesn't match");
    }
    c.support_of(&q.identity, q.read == "1").await.map_or_else(internal, ok)
}

/// A player's own reports, with the admins' replies (signed: only they read them). On any
/// network, unlike suggestions: every coordinator takes reports.
async fn my_reports(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap, Query(q): Query<Signed>) -> Answer {
    if !c.reads.check(&limit_key(client_ip(peer, &headers))) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    // Signed for this coordinator, by the name it was reached at.
    let Some(host) = c.signed_for(&headers) else {
        return fail(StatusCode::FORBIDDEN, "the signature doesn't match");
    };
    let host = host.as_str();
    if !identity::is_global_id(&q.identity) || !identity::fresh(q.time, identity::now()) || !identity::verify(&q.identity, &identity::reports_message(host, q.time), &q.signature) {
        return fail(StatusCode::FORBIDDEN, "the signature doesn't match");
    }
    c.reports_of(&q.identity).await.map_or_else(internal, ok)
}

async fn changes(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let body: Value = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let list = body["changes"].as_array().cloned().unwrap_or_default();
    if list.len() > MAX_CHANGES {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "too many changes at once");
    }
    if !(0..list.len()).all(|_| c.changes.check(&server)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many changes; slow down");
    }
    // Refused whole, so the server keeps them and tries again (a refused change is dropped).
    match c.links_allowed(&server, &list).await {
        Ok(Ok(())) => {}
        Ok(Err(why)) => {
            tracing::warn!("server {server}: {why}");
            return fail(StatusCode::TOO_MANY_REQUESTS, why);
        }
        Err(e) => return internal(e),
    }
    let mut results = Vec::with_capacity(list.len());
    for change in &list {
        results.push(match c.apply(&server, change).await {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!("server {server}: refused {change}: {e}");
                json!({ "error": e })
            }
        });
    }
    ok(json!({ "results": results }))
}

/// A linked player's friends and blocks, from their side.
async fn relations(State(c): State<Shared>, headers: HeaderMap, Path(global_id): Path<String>) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    match c.linked(&server, &global_id).await {
        Ok(true) => {}
        Ok(false) => return fail(StatusCode::FORBIDDEN, "that player isn't linked on this server"),
        Err(e) => return internal(e),
    }
    let friendships: Vec<(String, String, i64)> = match sqlx::query_as("SELECT a, b, friends FROM friendships WHERE a = ? OR b = ?")
        .bind(&global_id)
        .bind(&global_id)
        .fetch_all(&c.pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => return internal(e),
    };
    let blocks: Vec<(String, String, i64)> = match sqlx::query_as("SELECT from_id, to_id, blocked FROM blocks WHERE from_id = ? OR to_id = ?")
        .bind(&global_id)
        .bind(&global_id)
        .fetch_all(&c.pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => return internal(e),
    };
    let mut others: BTreeMap<String, Value> = BTreeMap::new();
    for (a, b, friends) in friendships {
        let other = if a == global_id { b } else { a };
        relation(&mut others, &other)["friends"] = json!(friends != 0);
    }
    for (from, to, blocked) in blocks {
        let (other, key) = if from == global_id { (to, "blocked") } else { (from, "blocked_by") };
        relation(&mut others, &other)[key] = json!(blocked != 0);
    }
    // Friends playing on another server of the group, and where: only friends, never
    // across a block.
    for (other, r) in &mut others {
        if r["friends"] != json!(true) || r["blocked"] == json!(true) || r["blocked_by"] == json!(true) {
            continue;
        }
        match c.elsewhere(&global_id, other, &server).await {
            Ok(Some(w)) => r["elsewhere"] = w,
            Ok(None) => {}
            Err(e) => return internal(e),
        }
    }
    ok(json!({ "relations": others.into_values().collect::<Vec<_>>() }))
}

#[derive(Deserialize)]
struct ClaimRequest {
    name: String,
    global_id: String,
    /// The host the player signed (one of the calling server's names).
    #[serde(default)]
    host: String,
    time: i64,
    /// The player's link signature for this name on the calling server.
    signature: String,
}

/// Reserves a name for a player about to make (or rename) an account on the
/// calling server. 409 when it's another player's.
async fn claim_name(State(c): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let req: ClaimRequest = match parse(&body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !valid_name(&req.name) {
        return fail(StatusCode::BAD_REQUEST, "not a name");
    }
    match c.owns_host(&server, &req.host).await {
        Ok(true) => {}
        Ok(false) => return fail(StatusCode::FORBIDDEN, "that host isn't one of this server's names"),
        Err(e) => return internal(e),
    }
    // Claimed while the player waits, so the signature is minutes old at most.
    if !identity::fresh(req.time, identity::now()) {
        return fail(StatusCode::FORBIDDEN, "the player's signature is too old, or from the future");
    }
    if !identity::is_global_id(&req.global_id) || !identity::verify(&req.global_id, &identity::link_message(&req.host, &req.name, req.time), &req.signature) {
        return fail(StatusCode::FORBIDDEN, "the player's signature doesn't match");
    }
    // A server can't reserve names by the thousand for identities that never link.
    let unlinked: i64 = match sqlx::query_scalar(
        "SELECT COUNT(*) FROM names n WHERE n.server_id = ? AND n.claimed_at > ?
           AND NOT EXISTS (SELECT 1 FROM links l WHERE l.global_id = n.global_id AND lower(l.username) = n.name_key)",
    )
    .bind(&server)
    .bind(identity::now() - 3600)
    .fetch_one(&c.pool)
    .await
    {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    if unlinked >= MAX_UNLINKED_CLAIMS_PER_HOUR {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many names claimed and not used; try later");
    }
    match c.claim_for(&req.global_id, &req.name, identity::now(), Some(&server)).await {
        Ok(true) => ok(json!({})),
        Ok(false) => fail(StatusCode::CONFLICT, "that name belongs to another player on the servers sharing friends"),
        Err(e) => internal(e),
    }
}

/// Whether a name is reserved, and by whom (for an account made without an identity).
async fn name_owner(State(c): State<Shared>, headers: HeaderMap, Path(name): Path<String>) -> Answer {
    if let Err(e) = c.server(&headers).await {
        return e;
    }
    match c.owner(&identity::name_key(&name)).await {
        // Whether it's taken; whose, no member needs to know.
        Ok(owner) => ok(json!({ "claimed": owner.is_some() })),
        Err(e) => internal(e),
    }
}

/// The entry for `other` in a relations answer, made on first use.
fn relation<'a>(others: &'a mut BTreeMap<String, Value>, other: &str) -> &'a mut Value {
    others
        .entry(other.to_string())
        .or_insert_with(|| json!({ "other": other, "friends": false, "blocked": false, "blocked_by": false }))
}

#[cfg(test)]
mod tests;
