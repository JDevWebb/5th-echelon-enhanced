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
        Ok(Self { pool, join_token })
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
    async fn apply(&self, server: &str, change: &Value) -> Result<(), String> {
        let text = |k: &str| change[k].as_str().unwrap_or_default().to_string();
        let now = identity::now();
        let result: Result<(), String> = match change["op"].as_str().unwrap_or_default() {
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
                tx.commit().await.map_err(|e| e.to_string())
            }
            "unlink" => sqlx::query("DELETE FROM links WHERE server_id = ? AND global_id = ?")
                .bind(server)
                .bind(text("global_id"))
                .execute(&self.pool)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
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
                self.set_friends(&a, &b, friends, now).await.map_err(|e| e.to_string())
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
                Ok(())
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
            Ok(()) => json!({}),
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

/// The entry for `other` in a relations answer, made on first use.
fn relation<'a>(others: &'a mut BTreeMap<String, Value>, other: &str) -> &'a mut Value {
    others
        .entry(other.to_string())
        .or_insert_with(|| json!({ "other": other, "friends": false, "blocked": false, "blocked_by": false }))
}

#[cfg(test)]
mod tests;
