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
const OUTBOX_BATCH: u32 = 50;

/// One change for the coordinator, as stored in the outbox and sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    /// A player linked their account here to their identity (with their
    /// signature, which the coordinator checks too).
    Link { global_id: String, username: String, time: i64, signature: String },
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
}

/// What the directory shows about this server.
#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub name: String,
    pub region: String,
    pub listed: bool,
    pub host: String,
    pub ports: PublicPorts,
    pub version: String,
    pub players_online: u32,
    pub players_total: u32,
    pub friends_mode: FriendsMode,
}

struct State {
    server_id: String,
    enabled: bool,
    wake: tokio::sync::Notify,
    pulls: Mutex<HashSet<u32>>,
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
    });
}

/// This server's id; "local" until [`init`] (tests).
pub fn server_id() -> &'static str {
    STATE.get().map_or("local", |s| s.server_id.as_str())
}

fn enabled() -> bool {
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

/// Asks for `user`'s friends from other servers soon.
pub fn pull_soon(user: u32) {
    if let Some(state) = STATE.get().filter(|s| s.enabled) {
        state.pulls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(user);
        state.wake.notify_one();
    }
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
            info!(logger, "Federation: {user} and {} ({}) now {:?}", other.username, other.id, storage.relation(user, other.id).await?);
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
    let http = match reqwest::Client::builder().timeout(Duration::from_secs(10)).build() {
        Ok(c) => c,
        Err(e) => return crit!(logger, "Federation: no HTTP client: {e}"),
    };
    info!(logger, "Federation: coordinator {base}, server id {}", state.server_id);
    let mut secret: Option<String> = None;
    let mut last_heartbeat: Option<Instant> = None;
    let mut last_online_pull: Option<Instant> = None;
    loop {
        if secret.is_none() {
            secret = match credentials(&base) {
                Some(saved) => Some(saved),
                None => join(&logger, &http, &base, &cfg, &state.server_id).await,
            };
        }
        if let Some(secret) = secret.as_deref() {
            let client = Coordinator {
                http: &http,
                base: &base,
                secret,
            };
            if last_heartbeat.is_none_or(|t| t.elapsed() >= HEARTBEAT_EVERY) {
                let mut listing = listing();
                if let Ok((online, total)) = storage.player_counts().await {
                    (listing.players_online, listing.players_total) = (online, total);
                }
                match client.post("/v1/heartbeat", &listing).await {
                    Ok(_) => last_heartbeat = Some(Instant::now()),
                    Err(e) => warn!(logger, "Federation: heartbeat failed: {e}"),
                }
            }
            if let Err(e) = flush(&logger, &storage, &client).await {
                warn!(logger, "Federation: sending changes failed (will retry): {e}");
            } else {
                if last_online_pull.is_none_or(|t| t.elapsed() >= PULL_ONLINE_EVERY) {
                    last_online_pull = Some(Instant::now());
                    if let Ok(online) = storage.online_linked().await {
                        state.pulls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend(online.into_iter().map(|p| p.id));
                    }
                }
                let users: Vec<u32> = state.pulls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain().collect();
                for user in users {
                    if let Err(e) = pull(&logger, &storage, &client, user).await {
                        warn!(logger, "Federation: pulling friends of {user} failed: {e}");
                    }
                }
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), state.wake.notified()).await;
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
        let value = resp.json::<serde_json::Value>().await.unwrap_or_default();
        if !status.is_success() {
            return Err(eyre::eyre!("{status}: {}", value["error"].as_str().unwrap_or_default()));
        }
        Ok(value)
    }

    async fn get(&self, path: &str) -> eyre::Result<serde_json::Value> {
        let resp = self.http.get(format!("{}{path}", self.base)).bearer_auth(self.secret).send().await?;
        let status = resp.status();
        let value = resp.json::<serde_json::Value>().await.unwrap_or_default();
        if !status.is_success() {
            return Err(eyre::eyre!("{status}: {}", value["error"].as_str().unwrap_or_default()));
        }
        Ok(value)
    }
}

/// The saved credentials, if they're for this coordinator.
fn credentials(base: &str) -> Option<String> {
    let text = std::fs::read_to_string(CREDENTIALS_FILE).ok()?;
    let (url, secret) = text.trim().split_once('\n')?;
    (url.trim() == base).then(|| secret.trim().to_string())
}

/// Joins the coordinator with the join token and saves the credentials.
async fn join(logger: &Logger, http: &reqwest::Client, base: &str, cfg: &FederationConfig, server_id: &str) -> Option<String> {
    if cfg.join_token.trim().is_empty() {
        error!(logger, "Federation: not joined yet, and [federation] join_token is empty; ask the coordinator's operator for one");
        return None;
    }
    let body = serde_json::json!({ "token": cfg.join_token.trim(), "server_id": server_id });
    let result = async {
        let resp = http.post(format!("{base}/v1/join")).json(&body).send().await?;
        let status = resp.status();
        let value = resp.json::<serde_json::Value>().await.unwrap_or_default();
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
            error!(logger, "Federation: joining {base} failed: {e}");
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
            if let Some(err) = results.get(i).and_then(|r| r["error"].as_str()) {
                warn!(logger, "Federation: the coordinator refused {body}: {err}");
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
    apply(logger, storage, user, &relations).await
}

#[cfg(test)]
mod tests {
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
        block_on(apply(&log, &s, kiwi, &[pulled(&tank_gid, true, false, false), pulled("GID-NotHere", true, false, false)])).unwrap().unwrap();
        assert_eq!(block_on(s.relation(kiwi, tank)).unwrap().unwrap(), Relation::Friend);

        // A block elsewhere hides them here too, and wins over friendship.
        block_on(apply(&log, &s, kiwi, &[pulled(&pest_gid, true, false, true)])).unwrap().unwrap();
        assert_eq!(block_on(s.relation(kiwi, pest)).unwrap().unwrap(), Relation::BlockedBy);

        // Unfriended and unblocked elsewhere.
        block_on(apply(&log, &s, kiwi, &[pulled(&tank_gid, false, false, false), pulled(&pest_gid, false, false, false)])).unwrap().unwrap();
        assert_eq!(block_on(s.relation(kiwi, tank)).unwrap().unwrap(), Relation::None);
        assert_eq!(block_on(s.relation(kiwi, pest)).unwrap().unwrap(), Relation::None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn changes_encode_as_the_coordinator_expects() {
        let c = Change::Block {
            from: "A".into(),
            to: "B".into(),
            blocked: true,
        };
        assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!({ "op": "block", "from": "A", "to": "B", "blocked": true }));
    }
}
