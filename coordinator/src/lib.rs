//! The coordinator: shares friends between 5th Echelon servers, and lists
//! them in a server directory for launchers.
//!
//! Servers join with a join token (from whoever runs the coordinator) and get
//! a secret for everything after. Then:
//!
//! * `POST /v1/heartbeat`: a server's directory entry (name, region, address,
//!   ports, players online), every 30 seconds.
//! * `GET /v1/servers`: the directory, for launchers (no sign-in).
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
//! * `POST /v1/metrics`: a server's metrics, every minute (see [`metrics`]),
//!   for the admin UI ([`admin`], on its own listener).
//! * `POST /v1/pings`: a launcher's pings to the servers (no sign-in).
//! * Heartbeat answers carry the release a server should install (see
//!   [`updates`]); servers that don't keep up leave the directory.

pub mod admin;
pub mod metrics;
pub mod updates;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::extract::DefaultBodyLimit;
use axum::extract::Path;
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
/// Host names (and addresses) one server may hold.
const MAX_SERVER_NAMES: i64 = 32;

/// A request's source address: the peer, or, from a proxy on this machine,
/// the last address in `X-Forwarded-For`.
fn client_ip(peer: std::net::SocketAddr, headers: &HeaderMap) -> std::net::IpAddr {
    if peer.ip().is_loopback() {
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

/// At most `max` requests per key in a minute.
struct Limit {
    max: usize,
    seen: std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<std::time::Instant>>>,
}

impl Limit {
    fn new(max: usize) -> Self {
        Self {
            max,
            seen: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn check(&self, key: &str) -> bool {
        let now = std::time::Instant::now();
        let window = Duration::from_secs(60);
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
fn valid_host(host: &str) -> bool {
    (1..=253).contains(&host.len()) && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

/// Printable text of at most `max` characters.
fn valid_text(text: &str, max: usize) -> bool {
    text.chars().count() <= max && !text.chars().any(char::is_control)
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
            if [Some(p.api), Some(p.login), p.secure, p.content, p.nat].into_iter().flatten().any(|port| port == 0) {
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
    heartbeats: Limit,
    metrics: Limit,
    /// Where player addresses are (for launchers' ping reports).
    pub geo: std::sync::OnceLock<Arc<geo::Geo>>,
    /// The coordinator's folder (for its own update requests).
    pub data_dir: std::sync::OnceLock<std::path::PathBuf>,
    /// The admin UI's settings, once it's on.
    pub admin: std::sync::OnceLock<admin::Config>,
}

type Shared = Arc<Coordinator>;
type Answer = (StatusCode, Json<Value>);

fn ok(v: Value) -> Answer {
    (StatusCode::OK, Json(v))
}

fn fail(status: StatusCode, msg: &str) -> Answer {
    (status, Json(json!({ "error": msg })))
}

fn internal(e: impl std::fmt::Display) -> Answer {
    tracing::error!("{e}");
    fail(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

impl Coordinator {
    /// Opens (creating if needed) the database at `path`.
    pub async fn open(path: &str, join_token: String) -> eyre::Result<Self> {
        let options = SqliteConnectOptions::new().filename(path).create_if_missing(true).foreign_keys(true);
        let pool = SqlitePool::connect_with(options).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
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
            // Per server: a heartbeat every 30 seconds and metrics every minute, with room
            // for a retry.
            heartbeats: Limit::new(6),
            metrics: Limit::new(2),
            geo: std::sync::OnceLock::new(),
            data_dir: std::sync::OnceLock::new(),
            admin: std::sync::OnceLock::new(),
        };
        c.claim_linked_names().await?;
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
        self.sweep().await?;
        Ok(done.rows_affected() > 0)
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
            .route("/v1/changes", post(changes))
            .route("/v1/relations/{global_id}", get(relations))
            .route("/v1/names/claim", post(claim_name))
            .route("/v1/names/{name}", get(name_owner))
            .route("/v1/metrics", post(metrics_report))
            .route("/v1/pings", post(pings))
            .layer(DefaultBodyLimit::max(MAX_BODY))
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
            let mut tx = self.pool.begin().await?;
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
                // The server checked the time when the player linked; a change may arrive
                // hours later, after an outage, so only the signature is checked here.
                if !identity::verify(&global_id, &identity::link_message(&host, &username, time), &signature) {
                    return Err("the player's signature doesn't match".into());
                }
                let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
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
        let mut tx = self.pool.begin().await?;
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

async fn join(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap, Json(req): Json<JoinRequest>) -> Answer {
    if !c.joins.check(&client_ip(peer, &headers).to_string()) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many attempts; try again in a minute");
    }
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

async fn heartbeat(State(c): State<Shared>, headers: HeaderMap, Json(listing): Json<Listing>) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
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
    let clashes = match c.claim_server_names(&server, &listing).await {
        Ok(clashes) => clashes,
        Err(e) => return internal(e),
    };
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
    ok(answer)
}

/// A server's metrics report (see [`metrics`]).
async fn metrics_report(State(c): State<Shared>, headers: HeaderMap, Json(report): Json<Value>) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    if !c.metrics.check(&server) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "metrics every minute are enough");
    }
    if report.to_string().len() > metrics::MAX_REPORT || !report["metrics"].is_object() {
        return fail(StatusCode::BAD_REQUEST, "not a metrics report");
    }
    match c.record_metrics(&server, &report).await {
        Ok(()) => ok(json!({})),
        Err(e) => internal(e),
    }
}

/// A launcher's pings to the servers in the directory.
async fn pings(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap, Json(body): Json<Value>) -> Answer {
    let ip = client_ip(peer, &headers);
    if !c.pings.check(&ip.to_string()) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many reports");
    }
    let list = body["pings"].as_array().cloned().unwrap_or_default();
    match c.record_player_pings(ip, &list).await {
        Ok(n) => ok(json!({ "recorded": n })),
        Err(e) => internal(e),
    }
}

/// The directory: servers that are listed and have sent a heartbeat lately,
/// most players online first.
async fn servers(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, headers: HeaderMap) -> Answer {
    if !c.reads.check(&client_ip(peer, &headers).to_string()) {
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
            v["id"] = json!(id);
            v["seen_secs_ago"] = json!(now - last_seen.unwrap_or(now));
            Some(v)
        })
        .collect();
    list.sort_by_key(|v| std::cmp::Reverse(v["players_online"].as_u64().unwrap_or(0)));
    ok(json!({ "servers": list }))
}

async fn changes(State(c): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let list = body["changes"].as_array().cloned().unwrap_or_default();
    if list.len() > MAX_CHANGES {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "too many changes at once");
    }
    if !(0..list.len()).all(|_| c.changes.check(&server)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many changes; slow down");
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
async fn claim_name(State(c): State<Shared>, headers: HeaderMap, Json(req): Json<ClaimRequest>) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
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
