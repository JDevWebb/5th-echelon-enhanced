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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::routing::get;
use axum::routing::post;
use axum::Json;
use axum::Router;
use serde::Deserialize;
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

pub struct Coordinator {
    pool: SqlitePool,
    join_token: String,
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
        let c = Self { pool, join_token };
        c.claim_linked_names().await?;
        Ok(c)
    }

    /// Reserves the names of accounts linked before names were reserved,
    /// oldest link first. Harmless to repeat.
    async fn claim_linked_names(&self) -> sqlx::Result<()> {
        let links: Vec<(String, String, i64)> = sqlx::query_as("SELECT global_id, username, linked_at FROM links ORDER BY linked_at").fetch_all(&self.pool).await?;
        for (global_id, username, at) in links {
            self.claim(&global_id, &username, at).await?;
        }
        Ok(())
    }

    /// Claims `name` for `global_id` unless someone else has it. Returns
    /// whether it's theirs now.
    async fn claim(&self, global_id: &str, name: &str, now: i64) -> sqlx::Result<bool> {
        let key = identity::name_key(name);
        sqlx::query("INSERT OR IGNORE INTO names (name_key, name, global_id, claimed_at) VALUES (?, ?, ?, ?)")
            .bind(&key)
            .bind(name.trim())
            .bind(global_id)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(self.owner(&key).await?.as_deref() == Some(global_id))
    }

    async fn owner(&self, key: &str) -> sqlx::Result<Option<String>> {
        sqlx::query_scalar("SELECT global_id FROM names WHERE name_key = ?").bind(key).fetch_optional(&self.pool).await
    }

    /// Releases `global_id`'s names that none of its accounts use any more
    /// (after a rename or an unlink), past the grace for new accounts.
    async fn release_unused(&self, global_id: &str, now: i64) -> sqlx::Result<()> {
        let used: Vec<String> = sqlx::query_scalar("SELECT username FROM links WHERE global_id = ?").bind(global_id).fetch_all(&self.pool).await?;
        let used: Vec<String> = used.iter().map(|u| identity::name_key(u)).collect();
        let owned: Vec<(String, i64)> = sqlx::query_as("SELECT name_key, claimed_at FROM names WHERE global_id = ?").bind(global_id).fetch_all(&self.pool).await?;
        for (key, claimed_at) in owned {
            if !used.contains(&key) && now - claimed_at >= CLAIM_GRACE_SECS {
                sqlx::query("DELETE FROM names WHERE name_key = ? AND global_id = ?").bind(&key).bind(global_id).execute(&self.pool).await?;
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
                let (global_id, username, signature) = (text("global_id"), text("username"), text("signature"));
                let time = change["time"].as_i64().unwrap_or_default();
                if !identity::is_global_id(&global_id) {
                    return Err("not an identity".into());
                }
                // The server checked the time when the player linked; a change may arrive
                // hours later, after an outage, so only the signature is checked here.
                if !identity::verify(&global_id, &identity::link_message(server, &username, time), &signature) {
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
                self.set_friends(&a, &b, friends, now).await.map(|()| json!({})).map_err(|e| e.to_string())
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
                    self.set_friends(&from, &to, false, now).await.map_err(|e| e.to_string())?;
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
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM blocks WHERE blocked = 1 AND ((from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?))",
        )
        .bind(a)
        .bind(b)
        .bind(b)
        .bind(a)
        .fetch_one(&self.pool)
        .await?
            > 0)
    }

    async fn set_friends(&self, a: &str, b: &str, friends: bool, now: i64) -> sqlx::Result<()> {
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        sqlx::query(
            "INSERT INTO friendships (a, b, friends, updated) VALUES (?, ?, ?, ?)
             ON CONFLICT(a, b) DO UPDATE SET friends = excluded.friends, updated = excluded.updated",
        )
        .bind(a)
        .bind(b)
        .bind(i64::from(friends))
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
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
    ok(json!({ "name": "5th Echelon coordinator", "version": env!("CARGO_PKG_VERSION"), "servers": servers }))
}

#[derive(Deserialize)]
struct JoinRequest {
    token: String,
    server_id: String,
}

async fn join(State(c): State<Shared>, Json(req): Json<JoinRequest>) -> Answer {
    if c.join_token.is_empty() || !same_secret(req.token.trim(), &c.join_token) {
        return fail(StatusCode::FORBIDDEN, "wrong join token");
    }
    let id = req.server_id.trim();
    if id.is_empty() || id.len() > 64 || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        return fail(StatusCode::BAD_REQUEST, "server ids are up to 64 letters, digits and -");
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

async fn heartbeat(State(c): State<Shared>, headers: HeaderMap, Json(listing): Json<Value>) -> Answer {
    let server = match c.server(&headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let text = listing.to_string();
    if text.len() > 4096 {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "listing too large");
    }
    match sqlx::query("UPDATE servers SET listing = ?, last_seen = ? WHERE id = ?")
        .bind(text)
        .bind(identity::now())
        .bind(&server)
        .execute(&c.pool)
        .await
    {
        Ok(_) => ok(json!({})),
        Err(e) => internal(e),
    }
}

/// The directory: servers that are listed and have sent a heartbeat lately,
/// most players online first.
async fn servers(State(c): State<Shared>) -> Answer {
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
    let mut list: Vec<Value> = rows
        .into_iter()
        .filter_map(|(id, listing, last_seen)| {
            let mut v: Value = serde_json::from_str(&listing?).ok()?;
            if !v["listed"].as_bool().unwrap_or(true) {
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
    ok(json!({ "relations": others.into_values().collect::<Vec<_>>() }))
}

#[derive(Deserialize)]
struct ClaimRequest {
    name: String,
    global_id: String,
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
    if !identity::is_global_id(&req.global_id) || !identity::verify(&req.global_id, &identity::link_message(&server, &req.name, req.time), &req.signature) {
        return fail(StatusCode::FORBIDDEN, "the player's signature doesn't match");
    }
    match c.claim(&req.global_id, &req.name, identity::now()).await {
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
        Ok(owner) => ok(json!({ "claimed": owner.is_some(), "global_id": owner })),
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
