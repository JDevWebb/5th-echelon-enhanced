//! Friends across servers, through a coordination server (`[federation]`).
//!
//! Servers that share a coordinator carry friendships and blocks between
//! players who have linked their accounts to their identity (a key their
//! launcher holds; see the `identity` crate). A friendship made on one server
//! then shows up on every other server both players use.
//!
//! * Local changes go into an outbox (the `federation_outbox` table) and are
//!   sent in order; a coordinator that's down just delays them.
//! * The friends of online, linked players are pulled every minute and when
//!   someone links, then applied here: friendships, and blocks both ways.
//!   Changes are pushed before anything is pulled, so a pull never undoes one
//!   that hasn't been sent yet.
//! * The coordinator's server directory lists this server (name, region,
//!   address, ports, players online), refreshed every 30 seconds.
//!
//! The coordinator trusts member servers to act for players they've signed
//! in, but only for players linked to that server; links need the player's
//! signature, so no server can claim someone else's identity.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use slog::Logger;

use crate::config::FederationConfig;
use crate::config::FriendsMode;
use crate::config::PublicPorts;
use crate::storage::Relation;
use crate::storage::Storage;

/// This server's id (not secret): the coordinator knows it by it, and
/// players sign it into their links. Next to the database.
pub const SERVER_ID_FILE: &str = "server-id.txt";
/// The credentials this server was given when it joined a coordinator.
pub const CREDENTIALS_FILE: &str = "federation.key";

const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);
const PULL_ONLINE_EVERY: Duration = Duration::from_secs(60);
const METRICS_EVERY: Duration = Duration::from_secs(60);
/// The live numbers for the admin UI (a coordinator without them is asked again only
/// now and then).
const PULSE_EVERY: Duration = Duration::from_secs(10);
const PULSE_UNSUPPORTED_WAIT: Duration = Duration::from_secs(600);
const OUTBOX_BATCH: u32 = 50;
const JOIN_RETRY: Duration = Duration::from_secs(60);
/// Changes the coordinator refused for now (429) are sent again this much later.
const CHANGES_WAIT: Duration = Duration::from_secs(60);
/// Players and their play sessions for the admin UI: what changed this often, everyone at
/// start and this much later again (a coordinator without the roster is asked again only
/// now and then).
const ROSTER_EVERY: Duration = Duration::from_secs(300);
const ROSTER_FULL_EVERY: Duration = Duration::from_secs(6 * 3600);
/// A player signed in or out: the roster goes this long after (their online state is
/// written by then, and a burst of them goes as one), but not sooner than
/// [`ROSTER_SOON_GAP`] after the last.
const ROSTER_SOON_DELAY: Duration = Duration::from_secs(5);
const ROSTER_SOON_GAP: Duration = Duration::from_secs(10);
const ROSTER_UNSUPPORTED_WAIT: Duration = Duration::from_secs(3600);
/// The most players and play sessions one roster request carries.
const ROSTER_PLAYERS: usize = 2000;
const ROSTER_SESSIONS: u32 = 5000;
/// Global stats: the leaderboards' top places are fetched this often, with the places and
/// stats of the players online and their friends; stat writes go in batches of this many.
const BOARDS_EVERY: Duration = Duration::from_secs(300);
const STATS_BATCH: u32 = 1000;
/// The places kept of each global leaderboard, and players asked about at once.
const BOARD_TOP: u32 = 100;
const STATS_PLAYERS: u32 = 200;
/// A coordinator without global stats is asked again only now and then.
const STATS_UNSUPPORTED_WAIT: Duration = Duration::from_secs(3600);
/// A coordinator without reports is asked again only now and then.
const REPORTS_UNSUPPORTED_WAIT: Duration = Duration::from_secs(3600);
/// After a report failed to go (the coordinator down, say): reports can be megabytes.
const REPORTS_RETRY_WAIT: Duration = Duration::from_secs(300);
/// Players' session events: how many go in one request, and the waits after a failure and
/// for a coordinator without them.
const EVENTS_BATCH: u32 = 200;
const EVENTS_RETRY_WAIT: Duration = Duration::from_secs(30);
const EVENTS_UNSUPPORTED_WAIT: Duration = Duration::from_secs(3600);
/// Admin actions already carried out, kept so one the coordinator sends again (its answer
/// lost) isn't done twice.
const ACTIONS_KEPT: usize = 200;

/// One change for the coordinator, as stored in the outbox and sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    /// A player linked their account here to their identity (with their
    /// signature, which the coordinator checks too).
    Link {
        global_id: String,
        username: String,
        /// The host the player signed (one of this server's names).
        host: String,
        time: i64,
        signature: String,
    },
    /// The account is gone.
    Unlink { global_id: String },
    /// Two players became friends, or stopped being.
    Friends { a: String, b: String, friends: bool },
    /// `from` blocked `to`, or unblocked them.
    Block { from: String, to: String, blocked: bool },
}

/// One entry of a pulled friend list, from the linked player's side.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PulledRelation {
    pub other: String,
    #[serde(default)]
    pub friends: bool,
    /// The player blocked `other`.
    #[serde(default)]
    pub blocked: bool,
    /// `other` blocked the player.
    #[serde(default)]
    pub blocked_by: bool,
    /// A friend online on another server of the group, and where.
    #[serde(default)]
    pub elsewhere: Option<Elsewhere>,
}

/// Where a friend is playing, on another server sharing friends.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Elsewhere {
    /// Their name on that server.
    pub username: String,
    /// The server's name, region and host (what players connect to).
    pub server: String,
    #[serde(default)]
    pub region: String,
    pub host: String,
}

/// Each player's friends online on other servers, from their last pull.
static ELSEWHERE: OnceLock<Mutex<std::collections::HashMap<u32, Vec<Elsewhere>>>> = OnceLock::new();

fn elsewhere_map() -> std::sync::MutexGuard<'static, std::collections::HashMap<u32, Vec<Elsewhere>>> {
    ELSEWHERE.get_or_init(Default::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `user`'s friends playing on other servers of the group (as of the last pull, at most
/// a minute old while they're online).
pub fn friends_elsewhere(user: u32) -> Vec<Elsewhere> {
    elsewhere_map().get(&user).cloned().unwrap_or_default()
}

impl Elsewhere {
    /// The entry as players may be shown it, or None when it can't be: the
    /// coordinator's text is clipped and loses control and direction
    /// characters, and the host must be a host name or an IPv4 address (the
    /// launcher connects to it).
    fn cleaned(&self) -> Option<Self> {
        let username = printable(&self.username, 32);
        let host = self.host.trim();
        if username.is_empty() || !(valid_host_name(host) || host.parse::<std::net::Ipv4Addr>().is_ok()) {
            return None;
        }
        Some(Self {
            username,
            server: printable(&self.server, 40),
            region: printable(&self.region, 32),
            host: host.to_string(),
        })
    }
}

/// `text` without control characters or the ones that change the direction
/// of what follows (which could make a name read as something else), at most
/// `max` characters.
fn printable(text: &str, max: usize) -> String {
    let bidi = |c: char| matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    text.chars().filter(|c| !c.is_control() && !bidi(*c)).take(max).collect::<String>().trim().to_string()
}

/// A DNS host name: dot-separated labels of letters, digits and `-`.
fn valid_host_name(host: &str) -> bool {
    let label = |l: &str| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    (1..=253).contains(&host.len()) && host.split('.').all(label)
}

/// What the directory shows about this server.
#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    /// Every name players reach this server by (its host, address and
    /// aliases): the coordinator only accepts links signed for these.
    pub names: Vec<String>,
    pub name: String,
    pub region: String,
    pub listed: bool,
    pub host: String,
    pub ports: PublicPorts,
    pub version: String,
    pub players_online: u32,
    pub players_total: u32,
    pub friends_mode: FriendsMode,
    /// Whether this server installs the releases the coordinator rolls out.
    pub auto_update: bool,
    /// The identities of the players online here, so their friends on other servers see
    /// where they are.
    pub online: Vec<String>,
}

struct State {
    server_id: String,
    enabled: bool,
    wake: tokio::sync::Notify,
    pulls: Mutex<HashSet<u32>>,
    /// Players whose places and stats across the network to fetch now (they signed in).
    stat_pulls: Mutex<HashSet<u32>>,
    /// When a player's roster entry changed (they signed in or out), if the roster hasn't
    /// gone since: it goes soon, not at the next [`ROSTER_EVERY`].
    roster_soon: Mutex<Option<Instant>>,
    /// When each player's friends were last asked for on their behalf
    /// ([`pull_now_and_then`]).
    asked: Mutex<std::collections::HashMap<u32, Instant>>,
    /// The coordinator's URL and this server's secret, once joined: for the
    /// calls made while a player waits (names).
    joined: Mutex<Option<(String, String)>>,
}

static STATE: OnceLock<State> = OnceLock::new();

/// Sets this server's id and whether a coordinator is configured. Called
/// once at start, before the API serves.
pub fn init(server_id: String, enabled: bool) {
    let _ = STATE.set(State {
        server_id,
        enabled,
        wake: tokio::sync::Notify::new(),
        pulls: Mutex::new(HashSet::new()),
        stat_pulls: Mutex::new(HashSet::new()),
        roster_soon: Mutex::new(None),
        asked: Mutex::new(std::collections::HashMap::new()),
        joined: Mutex::new(None),
    });
}

/// The names players reach this server by (see `[public] aliases`), as
/// [`identity::host_key`] writes them.
static OWN_NAMES: OnceLock<Vec<String>> = OnceLock::new();

/// Sets this server's names, once at start.
pub fn set_own_names(names: impl IntoIterator<Item = String>) {
    let mut names: Vec<String> = names.into_iter().map(|n| identity::host_key(&n)).filter(|n| identity::valid_field(n)).collect();
    names.sort();
    names.dedup();
    let _ = OWN_NAMES.set(names);
}

pub fn own_names() -> Vec<String> {
    OWN_NAMES.get().cloned().unwrap_or_else(|| vec![String::from("127.0.0.1"), String::from("localhost")])
}

/// Whether players reach this server as `host` (what their signatures name).
pub fn is_own_host(host: &str) -> bool {
    identity::valid_field(host) && own_names().contains(&identity::host_key(host))
}

/// When a heartbeat last reached the coordinator (Unix seconds; 0: not yet).
static COORDINATOR_SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn coordinator_seen_now() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    COORDINATOR_SEEN.store(now, std::sync::atomic::Ordering::Relaxed);
}

/// The maintenance windows the coordinator last told this server of (its own, and the
/// network's), for `/api/info`: the game's overlay warns players before one starts.
static MAINTENANCE: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// Keeps a heartbeat answer's maintenance windows (none when it has none: cancelled, or an
/// older coordinator), checked.
fn note_maintenance(answer: &Value) {
    let windows: Vec<Value> = answer["maintenance"]
        .as_array()
        .into_iter()
        .flatten()
        .take(4)
        .filter_map(|w| {
            let (start, end) = (w["start"].as_i64()?, w["end"].as_i64()?);
            (end > start).then(|| {
                json!({
                    "start": start, "end": end, "note": printable(w["note"].as_str().unwrap_or_default(), 100),
                    "network": w["network"].as_bool().unwrap_or(false),
                })
            })
        })
        .collect();
    *MAINTENANCE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = windows;
}

/// The maintenance windows not over yet at `now`, for `/api/info`.
pub fn maintenance(now: i64) -> Vec<Value> {
    let windows = MAINTENANCE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    windows.iter().filter(|w| w["end"].as_i64().is_some_and(|end| end > now)).cloned().collect()
}

/// How long ago a heartbeat last reached the coordinator, in seconds, for `/api/info`:
/// the standby coordinators on other servers ask before taking over (docs/failover.md).
pub fn coordinator_seen_ago() -> Option<u64> {
    let seen = COORDINATOR_SEEN.load(std::sync::atomic::Ordering::Relaxed);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    (seen > 0).then(|| now.saturating_sub(seen))
}

/// This server's id; "local" until [`init`] (tests).
pub fn server_id() -> &'static str {
    STATE.get().map_or("local", |s| s.server_id.as_str())
}

/// Whether this server shares friends through a coordinator.
pub fn enabled() -> bool {
    STATE.get().is_some_and(|s| s.enabled)
}

/// Reads this server's id from `path`, making one the first time.
pub fn load_or_create_server_id(path: &std::path::Path) -> eyre::Result<String> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let id = text.trim().to_string();
        if !id.is_empty() {
            return Ok(id);
        }
    }
    let mut bytes = [0u8; 10];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    let id = identity::base32_encode(&bytes).to_lowercase();
    std::fs::write(path, format!("{id}\n"))?;
    Ok(id)
}

/// Queues `change` for the coordinator (nothing without one).
pub async fn record(logger: &Logger, storage: &Storage, change: Change) {
    if !enabled() {
        return;
    }
    let body = match serde_json::to_string(&change) {
        Ok(body) => body,
        Err(e) => return error!(logger, "Federation: can't encode {change:?}: {e}"),
    };
    if let Err(e) = storage.outbox_push(&body).await {
        error!(logger, "Federation: can't queue {change:?}: {e}");
    }
    if let Some(state) = STATE.get() {
        state.wake.notify_one();
    }
}

/// Queues the friendship between two local players, if both are linked.
pub async fn record_friends(logger: &Logger, storage: &Storage, a: u32, b: u32, friends: bool) {
    if let Some((a, b)) = global_ids(storage, a, b).await {
        record(logger, storage, Change::Friends { a, b, friends }).await;
    }
}

/// Queues `from`'s block of `to`, if both are linked.
pub async fn record_block(logger: &Logger, storage: &Storage, from: u32, to: u32, blocked: bool) {
    if let Some((from, to)) = global_ids(storage, from, to).await {
        record(logger, storage, Change::Block { from, to, blocked }).await;
    }
}

async fn global_ids(storage: &Storage, a: u32, b: u32) -> Option<(String, String)> {
    if !enabled() {
        return None;
    }
    let a = storage.find_person(a).await.ok()??.global_id?;
    let b = storage.find_person(b).await.ok()??.global_id?;
    Some((a, b))
}

/// A player just linked: sends the link, their friendships and blocks with
/// other linked players here, and pulls their friends from other servers.
pub async fn linked(logger: &Logger, storage: &Storage, user: u32, link: Change) {
    if !enabled() {
        return;
    }
    record(logger, storage, link).await;
    if let Ok(friends) = storage.friends_of(user).await {
        for friend in friends {
            record_friends(logger, storage, user, friend.id, true).await;
        }
    }
    if let Ok(blocked) = storage.blocked_by(user).await {
        for other in blocked {
            record_block(logger, storage, user, other.id, true).await;
        }
    }
    pull_soon(user);
}

/// Fetches `user`'s and their friends' places and stats across the network soon (they
/// signed in), for the game's leaderboards.
pub fn stats_soon(user: u32) {
    if let Some(state) = STATE.get().filter(|s| s.enabled) {
        state.stat_pulls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(user);
        state.wake.notify_one();
    }
}

/// A player's roster entry changed (they signed in or out): the roster goes to the
/// coordinator within seconds, so the admin UI's list keeps up.
pub fn roster_soon() {
    if let Some(state) = STATE.get().filter(|s| s.enabled) {
        state.roster_soon.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Instant::now);
        state.wake.notify_one();
    }
}

/// Stats were written: they go to the coordinator soon.
pub fn stats_written() {
    if let Some(state) = STATE.get().filter(|s| s.enabled) {
        state.wake.notify_one();
    }
}

/// A player's report is waiting: it goes to the coordinator soon.
pub fn report_queued() {
    if let Some(state) = STATE.get().filter(|s| s.enabled) {
        state.wake.notify_one();
    }
}

/// Asks for `user`'s friends from other servers soon.
pub fn pull_soon(user: u32) {
    if let Some(state) = STATE.get().filter(|s| s.enabled) {
        state.pulls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(user);
        state.wake.notify_one();
    }
}

/// Asks for `user`'s friends from other servers, at most once a minute: when
/// they look at their friends (the overlay), whether or not the game is
/// connected right now.
pub fn pull_now_and_then(user: u32) {
    let Some(state) = STATE.get().filter(|s| s.enabled) else {
        return;
    };
    {
        let mut asked = state.asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        asked.retain(|_, t| t.elapsed() < PULL_ONLINE_EVERY);
        if asked.contains_key(&user) {
            return;
        }
        asked.insert(user, Instant::now());
    }
    pull_soon(user);
}

/// What the coordinator says about a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameCheck {
    /// No coordinator, or it didn't answer: the name is only checked here.
    Unknown,
    /// Nobody holds it across the group.
    Free,
    /// It's (now) this player's across the group.
    Ours,
    /// Another player holds it across the group.
    Taken,
}

/// How long a player waits on the coordinator when making an account.
const NAME_TIMEOUT: Duration = Duration::from_secs(4);
/// The largest answer read from the coordinator.
const MAX_ANSWER: usize = 1024 * 1024;

/// The one HTTP client for the coordinator: no redirects (the secret goes
/// only where configured), a timeout.
fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("an HTTP client")
    })
}

/// Reads an answer as JSON, at most [`MAX_ANSWER`] bytes.
async fn read_json(mut resp: reqwest::Response) -> eyre::Result<serde_json::Value> {
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_ANSWER {
            return Err(eyre::eyre!("the coordinator's answer is too large"));
        }
    }
    Ok(serde_json::from_slice(&body).unwrap_or_default())
}

/// Whether `url` may carry this server's secret: HTTPS, or plain HTTP to
/// this machine only.
fn safe_coordinator_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else { return false };
    match parsed.scheme() {
        "https" => true,
        "http" => matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

fn joined() -> Option<(String, String)> {
    STATE.get().filter(|s| s.enabled)?.joined.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// Reserves `name` for `global_id` across the group (signed with the
/// player's link signature for this server, reached as `host`).
pub async fn claim_name(global_id: &str, name: &str, host: &str, time: i64, signature: &str) -> NameCheck {
    let Some((base, secret)) = joined() else {
        return NameCheck::Unknown;
    };
    let body = serde_json::json!({ "name": name, "global_id": global_id, "host": identity::host_key(host), "time": time, "signature": signature });
    let sent = http()
        .post(format!("{base}/v1/names/claim"))
        .bearer_auth(secret)
        .timeout(NAME_TIMEOUT)
        .json(&body)
        .send()
        .await;
    match sent.map(|r| r.status()) {
        Ok(s) if s.is_success() => NameCheck::Ours,
        Ok(reqwest::StatusCode::CONFLICT) => NameCheck::Taken,
        _ => NameCheck::Unknown,
    }
}

/// Whether anyone holds `name` across the group (for an account made
/// without an identity: it may take a name nobody has reserved).
pub async fn name_holder(name: &str) -> NameCheck {
    let Some((base, secret)) = joined() else {
        return NameCheck::Unknown;
    };
    let url = format!("{base}/v1/names/{}", urlencode(name));
    let answer = async {
        let resp = http().get(url).bearer_auth(secret).timeout(NAME_TIMEOUT).send().await.ok()?;
        resp.status().is_success().then_some(())?;
        read_json(resp).await.ok()
    }
    .await;
    match answer.and_then(|v| v["claimed"].as_bool()) {
        Some(true) => NameCheck::Taken,
        Some(false) => NameCheck::Free,
        None => NameCheck::Unknown,
    }
}

/// Percent-encodes a path segment.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => char::from(b).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Applies a pulled friend list for local player `user` (linked as `me`):
/// friendships and blocks with others who are linked here too. Pairs the
/// coordinator doesn't know are left alone.
pub async fn apply(logger: &Logger, storage: &Storage, user: u32, relations: &[PulledRelation]) -> eyre::Result<()> {
    for r in relations {
        let Some(other) = storage.find_person_by_global_id(&r.other).await? else {
            continue;
        };
        let before = storage.relation(user, other.id).await?;
        // Blocks first: a block ends the friendship, and nothing overrides it.
        let mut changed = false;
        match (r.blocked, before == Relation::Blocked) {
            (true, false) => changed |= storage.block(user, other.id).await?.is_ok(),
            (false, true) => changed |= storage.unblock(user, other.id).await?,
            _ => {}
        }
        match (r.blocked_by, before == Relation::BlockedBy) {
            (true, false) => changed |= storage.block(other.id, user).await?.is_ok(),
            (false, true) => changed |= storage.unblock(other.id, user).await?,
            _ => {}
        }
        let now = storage.relation(user, other.id).await?;
        if r.friends && now != Relation::Friend && !matches!(now, Relation::Blocked | Relation::BlockedBy) {
            changed |= storage.make_friends(user, other.id).await?;
        } else if !r.friends && now == Relation::Friend {
            changed |= storage.remove_friend(user, other.id).await?;
        }
        if changed {
            info!(
                logger,
                "Federation: {user} and {} ({}) now {:?}",
                other.username,
                other.id,
                storage.relation(user, other.id).await?
            );
        }
    }
    Ok(())
}

/// The coordinator connection: joins, sends the outbox, pulls friends and
/// keeps the directory listing fresh. Runs until the process ends.
pub async fn run(logger: Logger, storage: Arc<Storage>, cfg: FederationConfig, listing: impl Fn() -> Listing + Send + 'static) {
    let Some(state) = STATE.get().filter(|s| s.enabled) else {
        return;
    };
    let base = cfg.coordinator.trim().trim_end_matches('/').to_string();
    if !safe_coordinator_url(&base) && !cfg.allow_http {
        return crit!(
            logger,
            "Federation: {base} isn't https:// (plain http only to this machine); not connecting, the secret would travel readable"
        );
    }
    let http = http().clone();
    info!(logger, "Federation: coordinator {base}, server id {}", state.server_id);
    let mut secret: Option<String> = None;
    let mut last_heartbeat: Option<Instant> = None;
    let mut last_metrics: Option<Instant> = None;
    let mut last_pulse: Option<Instant> = None;
    let mut pulse_wait = PULSE_EVERY;
    let mut last_online_pull: Option<Instant> = None;
    let mut last_join: Option<Instant> = None;
    // What the coordinator last said is wrong with this server's names (logged when it changes).
    let mut warnings: Vec<String> = Vec::new();
    // Changes the coordinator asked to send later (too many new links at once).
    let mut changes_wait: Option<Instant> = None;
    let mut roster = Roster::default();
    let mut boards = Boards::default();
    // When the coordinator last said it takes no reports.
    // When reports may be sent again, after a failure.
    let mut reports_after: Option<Instant> = None;
    // Players' uploads (uploads.rs) wait on their own, so a coordinator refusing or not
    // taking them never holds up reports.
    let mut content_after: Option<Instant> = None;
    // When session events may be sent again, after a failure.
    let mut events_after: Option<Instant> = None;
    let mut actions_done: Vec<(u64, crate::players::Outcome)> = Vec::new();
    let (mut heartbeats, mut changes, mut pulls) = (Failing::default(), Failing::default(), Failing::default());
    loop {
        if secret.is_none() {
            secret = credentials(&base);
            // Joining again only now and then: a coordinator that's down, or still getting
            // its certificate, shouldn't fill the log.
            if secret.is_none() && last_join.is_none_or(|t| t.elapsed() >= JOIN_RETRY) {
                last_join = Some(Instant::now());
                secret = join(&logger, &http, &base, &cfg, &state.server_id).await;
            }
        }
        if let Some(secret) = secret.as_deref() {
            *state.joined.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((base.clone(), secret.to_string()));
            let client = Coordinator { http: &http, base: &base, secret };
            if last_heartbeat.is_none_or(|t| t.elapsed() >= HEARTBEAT_EVERY) {
                let mut listing = listing();
                if let Ok((online, total)) = storage.player_counts().await {
                    (listing.players_online, listing.players_total) = (online, total);
                }
                if let Ok(online) = storage.online_linked().await {
                    listing.online = online.into_iter().filter_map(|p| p.global_id).take(20_000).collect();
                }
                match client.post("/v1/heartbeat", &listing).await {
                    Ok(answer) => {
                        heartbeats.worked(&logger, "the heartbeat");
                        last_heartbeat = Some(Instant::now());
                        coordinator_seen_now();
                        note_maintenance(&answer);
                        // The release the coordinator is rolling out to this server.
                        if let Some(version) = answer["update"]["version"].as_str() {
                            crate::self_update::request(&logger, version, cfg.auto_update);
                        }
                        // E.g. a name of this server's that another member server holds.
                        let now: Vec<String> = answer["warnings"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|w| w.as_str())
                            .take(8)
                            .map(|w| printable(w, 300))
                            .collect();
                        if now != warnings {
                            for w in &now {
                                warn!(logger, "Federation: the coordinator says: {w}");
                            }
                            warnings = now;
                        }
                    }
                    Err(e) => heartbeats.failed(&logger, "the heartbeat", &e),
                }
            }
            if last_metrics.is_none_or(|t| t.elapsed() >= METRICS_EVERY) {
                last_metrics = Some(Instant::now());
                let (metrics, matches) = crate::metrics::collect(&storage).await;
                let report = serde_json::json!({ "metrics": metrics, "update": crate::self_update::status(cfg.auto_update) });
                match client.post("/v1/metrics", &report).await {
                    // Finished matches count as reported only now: sent again next minute
                    // otherwise (the coordinator keeps one of each).
                    Ok(_) => {
                        if let Err(e) = storage.mark_matches_reported_async(&matches).await {
                            warn!(logger, "Federation: noting matches reported failed: {e:#}");
                        }
                    }
                    Err(e) => debug!(logger, "Federation: sending metrics failed: {e:#}"),
                }
            }
            if last_pulse.is_none_or(|t| t.elapsed() >= pulse_wait) {
                last_pulse = Some(Instant::now());
                let pulse = crate::metrics::pulse(&storage).await;
                pulse_wait = match client.post("/v1/pulse", &pulse).await {
                    Ok(answer) => {
                        carry_out_actions(&logger, &storage, &client, &answer, &mut actions_done).await;
                        PULSE_EVERY
                    }
                    // An older coordinator: not every ten seconds, then.
                    Err(e) if e.to_string().starts_with("404") => PULSE_UNSUPPORTED_WAIT,
                    Err(e) => {
                        debug!(logger, "Federation: sending the pulse failed: {e:#}");
                        PULSE_EVERY
                    }
                };
            }
            if boards.waited() {
                if let Err(e) = global_stats(&logger, &storage, &client, &state, &mut boards).await {
                    debug!(logger, "Federation: global stats failed: {e:#}");
                    if e.to_string().starts_with("404") {
                        boards.unsupported = Some(Instant::now());
                    }
                }
            }
            if reports_after.is_none_or(|t| Instant::now() >= t) {
                match send_reports(&storage, &client).await {
                    Ok(()) => reports_after = None,
                    Err(e) if e.to_string().starts_with("404") => reports_after = Some(Instant::now() + REPORTS_UNSUPPORTED_WAIT),
                    Err(e) => {
                        debug!(logger, "Federation: sending a report failed (will retry): {e:#}");
                        reports_after = Some(Instant::now() + REPORTS_RETRY_WAIT);
                    }
                }
            }
            if content_after.is_none_or(|t| Instant::now() >= t) {
                match send_content(&storage, &client).await {
                    Ok(()) => content_after = None,
                    // A coordinator from before 0.4.3: they wait, and go once it takes them.
                    Err(e) if e.to_string().starts_with("404") => content_after = Some(Instant::now() + REPORTS_UNSUPPORTED_WAIT),
                    Err(e) => {
                        debug!(logger, "Federation: sending a player's upload failed (will retry): {e:#}");
                        content_after = Some(Instant::now() + REPORTS_RETRY_WAIT);
                    }
                }
            }
            if events_after.is_none_or(|t| Instant::now() >= t) {
                match send_events(&storage, &client).await {
                    Ok(()) => events_after = None,
                    Err(e) if e.to_string().starts_with("404") => events_after = Some(Instant::now() + EVENTS_UNSUPPORTED_WAIT),
                    Err(e) => {
                        debug!(logger, "Federation: sending session events failed (will retry): {e:#}");
                        events_after = Some(Instant::now() + EVENTS_RETRY_WAIT);
                    }
                }
            }
            let soon = *state.roster_soon.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if roster.due() || roster.due_soon(soon) {
                state.roster_soon.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
                if let Err(e) = send_roster(&storage, &client, &mut roster).await {
                    debug!(logger, "Federation: sending players failed: {e:#}");
                }
            }
            if changes_wait.is_some_and(|t| t.elapsed() < CHANGES_WAIT) {
                // The coordinator asked for them later; friend lists wait too.
            } else if let Err(e) = flush(&logger, &storage, &client).await {
                changes.failed(&logger, "sending changes", &e);
                changes_wait = e.to_string().starts_with("429").then(Instant::now);
            } else {
                changes.worked(&logger, "sending changes");
                changes_wait = None;
                if last_online_pull.is_none_or(|t| t.elapsed() >= PULL_ONLINE_EVERY) {
                    last_online_pull = Some(Instant::now());
                    if let Ok(online) = storage.online_linked().await {
                        state
                            .pulls
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .extend(online.into_iter().map(|p| p.id));
                    }
                }
                let users: Vec<u32> = state.pulls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain().collect();
                for user in users {
                    match pull(&logger, &storage, &client, user).await {
                        Ok(_) => pulls.worked(&logger, "pulling friend lists"),
                        Err(e) => pulls.failed(&logger, &format!("pulling friends of {user}"), &e),
                    }
                }
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), state.wake.notified()).await;
    }
}

/// One kind of call to the coordinator failing: logged when it starts failing and when it
/// works again, not every five seconds while the coordinator is out of reach.
#[derive(Default)]
struct Failing {
    since: Option<Instant>,
}

impl Failing {
    fn failed(&mut self, logger: &Logger, what: &str, e: &eyre::Report) {
        if self.since.is_none() {
            self.since = Some(Instant::now());
            warn!(logger, "Federation: {what} failed (will keep trying; logged again when it works): {e:#}");
        } else {
            debug!(logger, "Federation: {what} failed again: {e:#}");
        }
    }

    fn worked(&mut self, logger: &Logger, what: &str) {
        if let Some(since) = self.since.take() {
            info!(logger, "Federation: {what} works again, after failing for {} s", since.elapsed().as_secs());
        }
    }
}

/// Players' reports waiting for the coordinator, sent one at a time. One it refuses as
/// invalid (400) is dropped; anything else is tried again.
/// A coordinator's answer that sending the same again won't change: a 4xx other than
/// 404 (no reports there yet), 408 and 429.
fn refused_for_good(e: &eyre::Report) -> bool {
    let text = e.to_string();
    text.get(..3)
        .and_then(|c| c.parse::<u16>().ok())
        .is_some_and(|c| (400..500).contains(&c) && !matches!(c, 404 | 408 | 429))
}

async fn send_reports(storage: &Storage, client: &Coordinator<'_>) -> eyre::Result<()> {
    for _ in 0..5 {
        let Some((id, body)) = storage.next_report().await? else { return Ok(()) };
        let Ok(body) = serde_json::from_str::<serde_json::Value>(&body) else {
            storage.report_sent(&id).await?;
            continue;
        };
        match client.post("/v1/reports", &body).await {
            Ok(_) => storage.report_sent(&id).await?,
            Err(e) if refused_for_good(&e) => {
                storage.report_sent(&id).await?;
                return Err(e.wrap_err(format!("the coordinator refused report {id}; dropped")));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Sends what players' games uploaded that the coordinator hasn't got (`POST /v1/content`):
/// each player's latest ShadowNet snapshot (uploads.rs), for the admins.
async fn send_content(storage: &Storage, client: &Coordinator<'_>) -> eyre::Result<()> {
    for _ in 0..5 {
        let Some((user, type_id, size, gzip, updated_at)) = storage.next_unsent_content().await? else {
            return Ok(());
        };
        let Some(person) = storage.find_person(user).await? else {
            storage.content_sent(user, type_id, updated_at).await?;
            continue;
        };
        let body = serde_json::json!({
            "player": { "id": user, "name": person.username, "identity": person.global_id },
            "type": type_id,
            "size": size,
            "updated_at": updated_at,
            "gzip_base64": sodiumoxide::base64::encode(&gzip, sodiumoxide::base64::Variant::Original),
        });
        match client.post("/v1/content", &body).await {
            Ok(_) => storage.content_sent(user, type_id, updated_at).await?,
            // A coordinator from before this answers 404: kept, and tried again in an hour.
            Err(e) if e.to_string().starts_with("404") => return Err(e),
            Err(e) if refused_for_good(&e) => {
                storage.content_sent(user, type_id, updated_at).await?;
                return Err(e.wrap_err(format!("the coordinator refused {user}'s content; dropped")));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Sends the session events the coordinator hasn't got (`POST /v1/events`), oldest first;
/// they're marked sent only once it took them.
async fn send_events(storage: &Storage, client: &Coordinator<'_>) -> eyre::Result<()> {
    for _ in 0..5 {
        let events = storage.unsent_session_events_async(EVENTS_BATCH).await?;
        if events.is_empty() {
            return Ok(());
        }
        client.post("/v1/events", &serde_json::json!({ "events": events })).await?;
        storage.mark_session_events_sent_async(&events).await?;
        if events.len() < EVENTS_BATCH as usize {
            return Ok(());
        }
    }
    Ok(())
}

/// When the global leaderboards were last fetched.
#[derive(Default)]
struct Boards {
    last: Option<Instant>,
    /// When the coordinator said it has no global stats.
    unsupported: Option<Instant>,
}

impl Boards {
    fn waited(&self) -> bool {
        self.unsupported.is_none_or(|t| t.elapsed() >= STATS_UNSUPPORTED_WAIT)
    }
}

/// Global stats: sends the stat writes waiting (`POST /v1/stats`), fetches the
/// leaderboards' top places now and then (`GET /v1/leaderboards`), and the places and stats
/// of the players online and their friends (`POST /v1/leaderboards/players`,
/// `POST /v1/stats/players`): all of them now and then, and those of a player who just
/// signed in at once.
async fn global_stats(logger: &Logger, storage: &Storage, client: &Coordinator<'_>, state: &State, boards: &mut Boards) -> eyre::Result<()> {
    let epoch = storage.stats_epoch().await?;
    for _ in 0..5 {
        let writes = storage.stats_outbox_peek(STATS_BATCH).await?;
        if writes.is_empty() {
            break;
        }
        let answer = client.post("/v1/stats", &serde_json::json!({ "epoch": epoch, "writes": writes })).await?;
        let Some(last) = answer["last_id"].as_i64() else {
            return Err(eyre::eyre!("the coordinator didn't say which stats it has"));
        };
        storage.stats_outbox_remove_upto(last).await?;
        if last < writes.last().map_or(0, |w| w.id) {
            // It took only some: the rest next time.
            break;
        }
    }
    boards.unsupported = None;
    let mut follow: Vec<String> = Vec::new();
    if boards.last.is_none_or(|t| t.elapsed() >= BOARDS_EVERY) {
        boards.last = Some(Instant::now());
        let answer = client.get(&format!("/v1/leaderboards?count={BOARD_TOP}")).await?;
        let lists: Vec<crate::storage::GlobalList> = serde_json::from_value(answer["lists"].clone())?;
        storage.replace_global_leaderboards(&lists).await?;
        follow = storage.global_ids_to_follow(STATS_PLAYERS * 5).await?;
    }
    let signed_in: Vec<u32> = state.stat_pulls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain().collect();
    for user in signed_in {
        follow.extend(storage.global_ids_around(user, STATS_PLAYERS).await?);
    }
    follow.sort_unstable();
    follow.dedup();
    for ids in follow.chunks(STATS_PLAYERS as usize) {
        let body = serde_json::json!({ "ids": ids });
        let answer = client.post("/v1/leaderboards/players", &body).await?;
        let ranks: Vec<crate::storage::GlobalRank> = serde_json::from_value(answer["ranks"].clone())?;
        storage.replace_global_ranks(ids, &ranks).await?;
        let answer = client.post("/v1/stats/players", &body).await?;
        let stats: Vec<crate::storage::GlobalStat> = serde_json::from_value(answer["stats"].clone())?;
        storage.replace_global_stats(ids, &stats).await?;
        debug!(logger, "Federation: places and stats of {} players across the network", ids.len());
    }
    Ok(())
}

/// When the roster last went, and from when play sessions are still to go.
#[derive(Default)]
struct Roster {
    last: Option<Instant>,
    last_full: Option<Instant>,
    wait: Option<Duration>,
    /// Play sessions changed at or after this (Unix seconds) haven't gone yet.
    sessions_since: i64,
}

impl Roster {
    fn due(&self) -> bool {
        self.last.is_none_or(|t| t.elapsed() >= self.wait.unwrap_or(ROSTER_EVERY))
    }

    /// Whether a change asked for at `soon` sends the roster now: after [`ROSTER_SOON_DELAY`],
    /// [`ROSTER_SOON_GAP`] after the last, and not while the coordinator said it takes none.
    fn due_soon(&self, soon: Option<Instant>) -> bool {
        soon.is_some_and(|t| t.elapsed() >= ROSTER_SOON_DELAY) && self.wait.is_none() && self.last.is_none_or(|t| t.elapsed() >= ROSTER_SOON_GAP)
    }
}

/// Runs `f` with the storage on a blocking thread (its calls block).
async fn blocking<T: Send + 'static>(storage: &Arc<Storage>, f: impl FnOnce(&Storage) -> eyre::Result<T> + Send + 'static) -> eyre::Result<T> {
    let storage = Arc::clone(storage);
    tokio::task::spawn_blocking(move || f(&storage)).await?
}

/// Sends the coordinator the players that changed (all of them now and then) and the play
/// sessions that did (`POST /v1/players`).
async fn send_roster(storage: &Arc<Storage>, client: &Coordinator<'_>, roster: &mut Roster) -> eyre::Result<()> {
    roster.last = Some(Instant::now());
    let full = roster.last_full.is_none_or(|t| t.elapsed() >= ROSTER_FULL_EVERY);
    let started = blocking(storage, Storage::now).await?;
    let mut ids: Vec<u32> = crate::players::take_changed().into_iter().collect();
    let result = async {
        let players = if full {
            blocking(storage, |s| s.player_records(None)).await?
        } else {
            let online = storage.online_player_ids().await.unwrap_or_default();
            ids.extend(online);
            ids.sort_unstable();
            ids.dedup();
            let wanted = ids.clone();
            blocking(storage, move |s| s.player_records(Some(&wanted))).await?
        };
        let since = roster.sessions_since;
        let mut sessions = Vec::new();
        for page in 0..10 {
            let batch = blocking(storage, move |s| s.play_sessions_changed_since(since, ROSTER_SESSIONS, page * ROSTER_SESSIONS)).await?;
            let last = batch.len() < ROSTER_SESSIONS as usize;
            sessions.push(batch);
            if last {
                break;
            }
        }
        // The whole roster in one request is authoritative (players missing from it are
        // gone); split, it's only an update.
        let whole = full && players.len() <= ROSTER_PLAYERS;
        let chunks = players.chunks(ROSTER_PLAYERS).count().max(sessions.len());
        for i in 0..chunks {
            let body = serde_json::json!({
                "full": whole && i == 0,
                "players": players.chunks(ROSTER_PLAYERS).nth(i).unwrap_or_default(),
                "sessions": sessions.get(i).map(Vec::as_slice).unwrap_or_default(),
            });
            client.post("/v1/players", &body).await?;
        }
        Ok::<_, eyre::Report>(())
    }
    .await;
    match result {
        Ok(()) => {
            roster.wait = None;
            roster.sessions_since = started;
            if full {
                roster.last_full = Some(Instant::now());
            }
            Ok(())
        }
        Err(e) => {
            crate::players::changed_again(ids);
            // An older coordinator: not every five minutes, then.
            if e.to_string().starts_with("404") {
                roster.wait = Some(ROSTER_UNSUPPORTED_WAIT);
            }
            Err(e)
        }
    }
}

/// Carries out the admin actions the coordinator sent with its answer to the pulse, and
/// tells it how each went (`POST /v1/actions/{id}`). One it sends again is answered from
/// `done`, not done twice.
async fn carry_out_actions(logger: &Logger, storage: &Arc<Storage>, client: &Coordinator<'_>, answer: &serde_json::Value, done: &mut Vec<(u64, crate::players::Outcome)>) {
    let Some(actions) = answer["actions"].as_array() else { return };
    for value in actions.iter().take(20) {
        let Ok(action) = serde_json::from_value::<crate::players::Action>(value.clone()) else {
            warn!(logger, "Federation: an admin action this server can't read: {}", printable(&value.to_string(), 200));
            continue;
        };
        let outcome = match done.iter().find(|(id, _)| *id == action.id) {
            Some((_, outcome)) => outcome.clone(),
            None => {
                let outcome = crate::players::perform(logger, storage, &action).await;
                done.push((action.id, outcome.clone()));
                if done.len() > ACTIONS_KEPT {
                    done.remove(0);
                }
                outcome
            }
        };
        if let Err(e) = client.post(&format!("/v1/actions/{}", action.id), &outcome).await {
            warn!(
                logger,
                "Federation: telling the coordinator how action {} went failed (will tell it again): {e:#}", action.id
            );
        }
    }
}

struct Coordinator<'a> {
    http: &'a reqwest::Client,
    base: &'a str,
    secret: &'a str,
}

impl Coordinator<'_> {
    async fn post<T: Serialize + ?Sized>(&self, path: &str, body: &T) -> eyre::Result<serde_json::Value> {
        let resp = self.http.post(format!("{}{path}", self.base)).bearer_auth(self.secret).json(body).send().await?;
        let status = resp.status();
        let value = read_json(resp).await?;
        if !status.is_success() {
            return Err(eyre::eyre!("{status}: {}", value["error"].as_str().unwrap_or_default()));
        }
        Ok(value)
    }

    async fn get(&self, path: &str) -> eyre::Result<serde_json::Value> {
        let resp = self.http.get(format!("{}{path}", self.base)).bearer_auth(self.secret).send().await?;
        let status = resp.status();
        let value = read_json(resp).await?;
        if !status.is_success() {
            return Err(eyre::eyre!("{status}: {}", value["error"].as_str().unwrap_or_default()));
        }
        Ok(value)
    }
}

/// Friends elsewhere kept per player (more than a friend list shows).
const MAX_ELSEWHERE: usize = 200;

/// The saved credentials, if they're for this coordinator.
fn credentials(base: &str) -> Option<String> {
    let (url, secret) = saved_credentials()?;
    (url == base).then_some(secret)
}

/// The coordinator address and secret saved when this server joined.
fn saved_credentials() -> Option<(String, String)> {
    let text = std::fs::read_to_string(CREDENTIALS_FILE).ok()?;
    let (url, secret) = text.trim().split_once('\n')?;
    Some((url.trim().to_string(), secret.trim().to_string()))
}

/// Whether two URLs name the same host.
fn same_host(a: &str, b: &str) -> bool {
    let host = |u: &str| reqwest::Url::parse(u).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    host(a).is_some_and(|h| Some(h) == host(b))
}

/// Whether two URLs reach the same machine: the same host, or names and
/// addresses that resolve to a shared address.
async fn same_machine(a: &str, b: &str) -> bool {
    async fn addresses(url: &str) -> Vec<std::net::IpAddr> {
        let Some((host, port)) = reqwest::Url::parse(url).ok().and_then(|u| Some((u.host_str()?.to_string(), u.port_or_known_default()?))) else {
            return vec![];
        };
        match tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host(format!("{host}:{port}"))).await {
            Ok(Ok(found)) => found.map(|a| a.ip()).collect(),
            _ => vec![],
        }
    }
    if same_host(a, b) {
        return true;
    }
    let (x, y) = (addresses(a).await, addresses(b).await);
    x.iter().any(|ip| y.contains(ip))
}

/// Joins the coordinator with the join token and saves the credentials.
async fn join(logger: &Logger, http: &reqwest::Client, base: &str, cfg: &FederationConfig, server_id: &str) -> Option<String> {
    if cfg.join_token.trim().is_empty() {
        error!(
            logger,
            "Federation: not joined yet, and [federation] join_token is empty; ask the coordinator's operator for one"
        );
        return None;
    }
    let body = serde_json::json!({ "token": cfg.join_token.trim(), "server_id": server_id });
    // Joined before, at another address of the same machine (the coordinator moved, e.g. to
    // https or from its IP address to a name): the coordinator only lets a server join again
    // with its current secret, so it goes along. Never to another machine: the secret is
    // that coordinator's, and anyone holding it can act as this server there.
    let mut previous = saved_credentials().filter(|(url, _)| url != base);
    if let Some((url, _)) = previous.take() {
        if same_machine(&url, base).await {
            info!(logger, "Federation: the coordinator was {url}; joining again at {base}");
            previous = saved_credentials();
        } else {
            info!(logger, "Federation: joined {url} before, which isn't {base}'s machine; joining {base} afresh");
        }
    }
    let result = async {
        let mut request = http.post(format!("{base}/v1/join")).json(&body);
        if let Some((_, secret)) = &previous {
            request = request.bearer_auth(secret);
        }
        let resp = request.send().await?;
        let status = resp.status();
        let value = read_json(resp).await?;
        if !status.is_success() {
            return Err(eyre::eyre!("{status}: {}", value["error"].as_str().unwrap_or_default()));
        }
        value["secret"].as_str().map(String::from).ok_or_else(|| eyre::eyre!("no secret in the answer"))
    }
    .await;
    match result {
        Ok(secret) => {
            if let Err(e) = crate::keys::write_secret_text(std::path::Path::new(CREDENTIALS_FILE), &format!("{base}\n{secret}")) {
                error!(logger, "Federation: joined, but couldn't save {CREDENTIALS_FILE}: {e}");
            }
            info!(logger, "Federation: joined {base}");
            Some(secret)
        }
        Err(e) => {
            error!(logger, "Federation: joining {base} failed (retrying in {} s): {e:#}", JOIN_RETRY.as_secs());
            None
        }
    }
}

/// Sends the outbox in order. Changes the coordinator refuses for good (a
/// player it doesn't know here) are logged and dropped, so they can't hold up
/// the rest.
async fn flush(logger: &Logger, storage: &Storage, client: &Coordinator<'_>) -> eyre::Result<()> {
    loop {
        let batch = storage.outbox_peek(OUTBOX_BATCH).await?;
        if batch.is_empty() {
            return Ok(());
        }
        let changes: Vec<serde_json::Value> = batch.iter().map(|(_, body)| serde_json::from_str(body).unwrap_or_default()).collect();
        let answer = client.post("/v1/changes", &serde_json::json!({ "changes": changes })).await?;
        let results = answer["results"].as_array().cloned().unwrap_or_default();
        for (i, (id, body)) in batch.iter().enumerate() {
            let result = results.get(i).cloned().unwrap_or_default();
            if let Some(err) = result["error"].as_str() {
                warn!(logger, "Federation: the coordinator refused {body}: {err}");
            }
            // A link says whether its name is someone else's across the group.
            if let (Some(conflict), Ok(Change::Link { global_id, username, .. })) = (result["conflict"].as_bool(), serde_json::from_str::<Change>(body)) {
                if conflict {
                    warn!(
                        logger,
                        "Federation: {username}'s name belongs to another player on the servers sharing friends; they'll be asked to rename"
                    );
                }
                storage.set_name_conflict(&global_id, conflict).await?;
            }
            storage.outbox_remove(*id).await?;
        }
    }
}

async fn pull(logger: &Logger, storage: &Storage, client: &Coordinator<'_>, user: u32) -> eyre::Result<()> {
    let Some(global_id) = storage.find_person(user).await?.and_then(|p| p.global_id) else {
        return Ok(());
    };
    let answer = client.get(&format!("/v1/relations/{global_id}")).await?;
    let relations: Vec<PulledRelation> = serde_json::from_value(answer["relations"].clone())?;
    let elsewhere: Vec<Elsewhere> = relations
        .iter()
        .filter(|r| r.friends && !r.blocked && !r.blocked_by)
        .filter_map(|r| r.elsewhere.as_ref().and_then(Elsewhere::cleaned))
        .take(MAX_ELSEWHERE)
        .collect();
    {
        let mut map = elsewhere_map();
        if elsewhere.is_empty() {
            map.remove(&user);
        } else {
            map.insert(user, elsewhere);
        }
    }
    apply(logger, storage, user, &relations).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn maintenance_windows_from_the_heartbeat_are_checked_and_expire() {
        use super::maintenance;
        use super::note_maintenance;
        super::note_maintenance(&serde_json::json!({ "maintenance": [
            { "start": 100, "end": 200, "note": "Moving\u{202e}", "network": false },
            { "start": 300, "end": 250, "note": "ends before it starts" },
            { "start": 400, "end": 500 },
        ] }));
        assert_eq!(
            maintenance(0),
            [
                serde_json::json!({ "start": 100, "end": 200, "note": "Moving", "network": false }),
                serde_json::json!({ "start": 400, "end": 500, "note": "", "network": false }),
            ]
        );
        assert_eq!(maintenance(200).len(), 1, "over at its end");
        // An answer without any (cancelled, or an older coordinator) clears them.
        note_maintenance(&serde_json::json!({}));
        assert!(maintenance(0).is_empty());
    }

    #[test]
    fn info_says_how_long_ago_the_coordinator_was_reached() {
        // Before a heartbeat has reached it, nothing is said (a standby elsewhere takes that
        // as not reaching it).
        if COORDINATOR_SEEN.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            assert_eq!(super::coordinator_seen_ago(), None);
        }
        super::coordinator_seen_now();
        assert!(super::coordinator_seen_ago().is_some_and(|ago| ago <= 1));
    }

    #[test]
    fn a_sign_in_sends_the_roster_within_seconds_not_minutes() {
        let ago = |secs| Instant::now().checked_sub(Duration::from_secs(secs));
        let roster = Roster {
            last: ago(60),
            ..Roster::default()
        };
        assert!(!roster.due(), "the regular roster waits five minutes");
        assert!(!roster.due_soon(None), "nothing changed");
        assert!(!roster.due_soon(ago(1)), "a moment for the online state to be written");
        assert!(roster.due_soon(ago(6)));
        // Not right after the last one, and not while the coordinator takes none.
        assert!(!Roster {
            last: ago(3),
            ..Roster::default()
        }
        .due_soon(ago(6)));
        assert!(!Roster {
            last: ago(60),
            wait: Some(ROSTER_UNSUPPORTED_WAIT),
            ..Roster::default()
        }
        .due_soon(ago(6)));
    }
    use super::*;
    use crate::storage::run as block_on;
    use crate::storage::tests::temp_storage;

    fn logger() -> Logger {
        Logger::root(slog::Discard, slog::o!())
    }

    fn linked_user(storage: &Storage, name: &str) -> (u32, String) {
        storage.register_user(name, "password1", Some(name)).unwrap();
        let id = storage.find_user_id_by_name(name).unwrap().unwrap();
        let gid = format!("GID-{name}");
        block_on(storage.link_global_id(id, &gid)).unwrap().unwrap();
        (id, gid)
    }

    fn pulled(other: &str, friends: bool, blocked: bool, blocked_by: bool) -> PulledRelation {
        PulledRelation {
            other: other.into(),
            friends,
            blocked,
            blocked_by,
            elsewhere: None,
        }
    }

    #[test]
    fn pulled_friends_and_blocks_apply_here() {
        let (s, dir) = temp_storage("fed-apply");
        let (kiwi, _) = linked_user(&s, "Kiwi");
        let (tank, tank_gid) = linked_user(&s, "Tank");
        let (pest, pest_gid) = linked_user(&s, "Pest");
        let log = logger();

        // Friends elsewhere: friends here, without asking again.
        block_on(apply(&log, &s, kiwi, &[pulled(&tank_gid, true, false, false), pulled("GID-NotHere", true, false, false)]))
            .unwrap()
            .unwrap();
        assert_eq!(block_on(s.relation(kiwi, tank)).unwrap().unwrap(), Relation::Friend);

        // A block elsewhere hides them here too, and wins over friendship.
        block_on(apply(&log, &s, kiwi, &[pulled(&pest_gid, true, false, true)])).unwrap().unwrap();
        assert_eq!(block_on(s.relation(kiwi, pest)).unwrap().unwrap(), Relation::BlockedBy);

        // Unfriended and unblocked elsewhere.
        block_on(apply(&log, &s, kiwi, &[pulled(&tank_gid, false, false, false), pulled(&pest_gid, false, false, false)]))
            .unwrap()
            .unwrap();
        assert_eq!(block_on(s.relation(kiwi, tank)).unwrap().unwrap(), Relation::None);
        assert_eq!(block_on(s.relation(kiwi, pest)).unwrap().unwrap(), Relation::None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn elsewhere_is_cleaned_before_players_see_it() {
        let e = |username: &str, server: &str, host: &str| Elsewhere {
            username: username.into(),
            server: server.into(),
            region: "Oceania".into(),
            host: host.into(),
        };
        let clean = e("Tank\u{202e}", "Server\nB", "server-b.example.com").cleaned().unwrap();
        assert_eq!((clean.username.as_str(), clean.server.as_str()), ("Tank", "ServerB"));
        assert_eq!(e(&"x".repeat(100), "B", "1.2.3.4").cleaned().unwrap().username.len(), 32);
        for host in ["", "evil.com/path", "a b", "[::1]", "host:80", "-x.com", "\u{202e}moc"] {
            assert!(e("Tank", "B", host).cleaned().is_none(), "{host:?}");
        }
        assert!(e("\u{200f}", "B", "b.example.com").cleaned().is_none(), "nothing left of the name");
    }

    #[test]
    fn the_secret_goes_only_to_the_same_host() {
        assert!(same_host("http://coord.example.com:8700", "https://coord.example.com"));
        assert!(same_host("https://Coord.example.com", "https://coord.example.com/"));
        assert!(!same_host("https://coord.example.com", "https://other.example.com"));
        assert!(!same_host("not a url", "not a url"));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert!(rt.block_on(same_machine("http://127.0.0.1:8700", "http://localhost:8700")), "a name for the same address");
        assert!(!rt.block_on(same_machine("http://127.0.0.1:8700", "http://192.0.2.1:8700")));
    }

    #[test]
    fn changes_encode_as_the_coordinator_expects() {
        let c = Change::Block {
            from: "A".into(),
            to: "B".into(),
            blocked: true,
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({ "op": "block", "from": "A", "to": "B", "blocked": true })
        );
    }
}
