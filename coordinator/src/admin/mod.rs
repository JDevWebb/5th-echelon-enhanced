//! The coordinator's admin web UI: the network's metrics, its servers and
//! their updates, and the admins themselves.
//!
//! It has a listener of its own (`--admin-listen`, on this machine only),
//! meant to be reached through Cloudflare and Caddy at its own host name
//! (`--admin-origin`, e.g. https://scbl-metrics.jdevwebb.net); the public API
//! never serves it.
//!
//! Signing in takes a password and a second factor (a TOTP code, a passkey,
//! or a recovery code), or a passkey alone (it verifies the admin with a PIN
//! or biometric). New admins get a one-time setup link from
//! `coordinator admin add`; they choose a password and must add a second
//! factor before anything else. Changes that matter (admins, sign-in
//! restrictions, rollbacks, removing servers, banning, renaming or deleting
//! a player, resetting their password, removing a person's stats, deleting a
//! player's report) want a second factor proved in the last ten minutes.
//!
//! Every request (the page too) must come from an allowed network and
//! country, when those are set. Behind Cloudflare, the address and country
//! are Cloudflare's (`CF-Connecting-IP` and `CF-IPCountry`, passed on by
//! Caddy); otherwise the connection's, located with DB-IP.

pub mod auth;
mod leaderboards;
pub mod live;
mod players;
mod reports;
pub mod webauthn;

use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::OnceLock;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::Request;
use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::Method;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::delete;
use axum::routing::get;
use axum::routing::post;
use axum::Extension;
use axum::Json;
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

use crate::Coordinator;
use crate::Limit;

/// Sessions end after this long without a request, and this long in all.
const IDLE_FOR: i64 = 30 * 60;
const SESSION_FOR: i64 = 12 * 3600;
/// Sensitive changes want a second factor proved this recently.
const RECENT: i64 = 10 * 60;
/// A setup link works this long.
pub const SETUP_FOR: i64 = 24 * 3600;
const CHALLENGE_FOR: i64 = 5 * 60;
/// Failed sign-ins before an account is locked for a while.
const FAILURES_BEFORE_LOCK: i64 = 5;
const TOTP_TRIES: i64 = 5;

/// The admin UI's settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where admins open it, e.g. "https://scbl-metrics.jdevwebb.net":
    /// passkeys are made for this site, and requests must come from it.
    pub origin: String,
    pub rp_id: String,
    /// Trust `X-Admin-Client-IP` and `X-Admin-Country` from the proxy on
    /// this machine (Caddy, passing on Cloudflare's).
    pub behind_proxy: bool,
}

impl Config {
    pub fn new(origin: &str, behind_proxy: bool) -> Result<Self, String> {
        let origin = origin.trim().trim_end_matches('/').to_string();
        let host = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://localhost").map(|_| "localhost"))
            .ok_or("the admin origin is https://<host name> (http only for localhost)")?;
        let rp_id = host.split(':').next().unwrap_or_default().to_string();
        if rp_id.is_empty() || !crate::valid_host(&rp_id) {
            return Err("the admin origin's host isn't a host name".into());
        }
        Ok(Self { origin, rp_id, behind_proxy })
    }

    fn secure(&self) -> bool {
        self.origin.starts_with("https://")
    }

    fn cookie_name(&self) -> &'static str {
        if self.secure() {
            "__Host-fes-admin"
        } else {
            "fes-admin"
        }
    }

    fn site(&self) -> webauthn::Site<'_> {
        webauthn::Site {
            rp_id: &self.rp_id,
            origin: &self.origin,
        }
    }
}

/// Who a request is from.
#[derive(Debug, Clone)]
pub struct Client {
    pub ip: IpAddr,
    pub country: String,
    pub user_agent: String,
}

/// A signed-in admin's session.
#[derive(Debug, Clone)]
struct Session {
    token_hash: String,
    admin_id: i64,
    username: String,
    stage: String,
    verified_at: i64,
}

type Shared = Arc<Coordinator>;

fn limits() -> &'static (Limit, Limit) {
    static L: OnceLock<(Limit, Limit)> = OnceLock::new();
    // Sign-in attempts per address, and API requests per session.
    L.get_or_init(|| (Limit::new(20), Limit::new(600)))
}

fn answer(status: StatusCode, v: Value) -> Response {
    (status, Json(v)).into_response()
}

fn ok(v: Value) -> Response {
    answer(StatusCode::OK, v)
}

fn fail(status: StatusCode, msg: &str) -> Response {
    answer(status, json!({ "error": msg }))
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!("admin: {e}");
    fail(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

fn cfg(c: &Coordinator) -> &Config {
    c.admin.get().expect("the admin UI is configured before it serves")
}

/// The admin UI.
pub fn router(c: Shared) -> Router {
    let api = Router::new()
        .route("/session", get(session_info))
        .route("/login", post(login))
        .route("/login/totp", post(login_totp))
        .route("/login/recovery", post(login_recovery))
        .route("/login/passkey/begin", post(passkey_login_begin))
        .route("/login/passkey/finish", post(passkey_login_finish))
        .route("/logout", post(logout))
        .route("/setup", post(setup))
        .route("/me", get(me))
        .route("/me/password", post(change_password))
        .route("/me/totp/begin", post(totp_begin))
        .route("/me/totp/confirm", post(totp_confirm))
        .route("/me/totp", delete(totp_remove))
        .route("/me/passkeys/begin", post(passkey_add_begin))
        .route("/me/passkeys/finish", post(passkey_add_finish))
        .route("/me/passkeys/{id}", delete(passkey_remove))
        .route("/me/recovery", post(new_recovery_codes))
        .route("/sessions/{id}", delete(end_session))
        .route("/admins", get(admins).post(add_admin))
        .route("/admins/{id}/{action}", post(admin_action))
        .route("/restrictions", get(restrictions).put(set_restrictions))
        .route("/audit", get(audit))
        .route("/overview", get(overview))
        .route("/series", get(series))
        .route("/places", get(places))
        .route("/activity", get(activity))
        .route("/pings", get(pings))
        .route("/labels", axum::routing::put(set_label))
        .route("/updates", get(updates))
        .route("/updates/{action}", post(update_action))
        .route("/servers/{id}", delete(remove_server))
        .route("/servers/{id}/purge-names", post(purge_names))
        .route("/bandwidth", get(bandwidth))
        .route("/players-report", get(players_report))
        .route("/allowances", axum::routing::put(set_allowance))
        .route("/alerts", get(alerts))
        .route("/alerts/webhook", axum::routing::put(set_webhook))
        .route("/alerts/test", post(test_webhook))
        .route("/alerts/reports", axum::routing::put(set_report_alerts))
        .route("/live", get(live::live))
        .merge(players::routes())
        .merge(reports::routes())
        .merge(leaderboards::routes());
    Router::new()
        .route("/", get(page))
        .nest("/api", api)
        .fallback(asset)
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(axum::middleware::from_fn_with_state(Arc::clone(&c), guard))
        .with_state(c)
}

/// Every request: who it's from, whether they may be here at all, that a
/// change comes from this site, and the security headers.
async fn guard(State(c): State<Shared>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Response {
    let config = cfg(&c);
    let headers = req.headers();
    let header_text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().trim().to_string();
    let from_proxy = config.behind_proxy && peer.ip().is_loopback();
    let ip = if from_proxy {
        header_text("x-admin-client-ip").parse().unwrap_or(peer.ip())
    } else {
        peer.ip()
    };
    let mut country = if from_proxy { header_text("x-admin-country").to_uppercase() } else { String::new() };
    // Cloudflare's "XX" (unknown) and "T1" (Tor) aren't countries, and stay what they are:
    // looked up again, an exit node's address could pass a country rule.
    if country.len() != 2 {
        country = c.geo.get().and_then(|g| g.lookup(ip)).map(|p| p.country).unwrap_or_default();
    }
    let client = Client {
        ip,
        country,
        user_agent: header_text("user-agent").chars().take(200).collect(),
    };
    let restrictions = c.restrictions().await.unwrap_or_default();
    if !restrictions.allows(client.ip, &client.country) {
        tracing::warn!("admin: refused {} ({}) by the sign-in restrictions", client.ip, client.country);
        return secure_headers(fail(StatusCode::FORBIDDEN, "The admin UI isn't available from here."));
    }
    // Changes only from this site's own pages: a custom header (other sites can't send it
    // without a preflight this never answers) and, when the browser says, the origin.
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let origin_ok = headers.get(header::ORIGIN).is_none_or(|o| o.to_str().ok() == Some(config.origin.as_str()));
        if !origin_ok || headers.get("x-fes-admin").is_none() {
            return secure_headers(fail(StatusCode::FORBIDDEN, "requests come from the admin UI's own pages"));
        }
    }
    req.extensions_mut().insert(client);
    secure_headers(next.run(req).await)
}

fn secure_headers(mut resp: Response) -> Response {
    let h = resp.headers_mut();
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
        ("x-frame-options", "DENY"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("permissions-policy", "camera=(), microphone=(), geolocation=(), publickey-credentials-get=(self), publickey-credentials-create=(self)"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
        // Browsers ignore it over plain HTTP (only localhost may be), and keep to HTTPS after.
        ("strict-transport-security", "max-age=31536000"),
    ] {
        h.insert(name, HeaderValue::from_static(value));
    }
    // no-transform: Cloudflare in front leaves the pages as they are. Otherwise it injects
    // scripts of its own (Web Analytics, bot detection), which the policy above blocks, with
    // errors in the console.
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store, no-transform"));
    }
    resp
}

/// The admin UI's files: the Vue app built in `coordinator/admin-ui` (see build.rs).
mod ui {
    include!(concat!(env!("OUT_DIR"), "/admin_ui.rs"));
}

/// The paths of the embedded UI's files (for tests).
#[cfg(test)]
pub(crate) fn ui_asset_paths() -> Vec<&'static str> {
    ui::FILES.iter().map(|(p, _, _)| *p).collect()
}

fn ui_file(path: &str) -> Option<(&'static [u8], &'static str)> {
    ui::FILES.iter().find(|(p, _, _)| *p == path).map(|(_, body, kind)| (*body, *kind))
}

async fn page() -> Response {
    match ui_file("/index.html") {
        Some((body, kind)) => ([(header::CONTENT_TYPE, kind)], body).into_response(),
        None => fail(StatusCode::NOT_FOUND, "not found"),
    }
}

async fn asset(req: Request<Body>) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return fail(StatusCode::NOT_FOUND, "not found");
    }
    let path = req.uri().path();
    let Some((body, kind)) = ui_file(path).filter(|_| path != "/index.html") else {
        return fail(StatusCode::NOT_FOUND, "not found");
    };
    let mut resp = ([(header::CONTENT_TYPE, kind)], body).into_response();
    // Built files carry their content's hash in the name: they never change.
    if path.starts_with("/assets/") {
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    }
    resp
}

// Sessions.

impl Coordinator {
    pub async fn restrictions(&self) -> sqlx::Result<auth::Restrictions> {
        let text: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'restrictions'")
            .fetch_optional(&self.pool)
            .await?;
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default())
    }

    pub async fn save_restrictions(&self, r: &auth::Restrictions) -> sqlx::Result<()> {
        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('restrictions', ?)")
            .bind(serde_json::to_string(r).unwrap_or_default())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn setting(&self, key: &str) -> sqlx::Result<Option<String>> {
        sqlx::query_scalar("SELECT value FROM settings WHERE key = ?").bind(key).fetch_optional(&self.pool).await
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> sqlx::Result<()> {
        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(value)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn audit(&self, admin: &str, client: Option<&Client>, event: &str, detail: &str) {
        let (ip, country) = client.map_or((String::from("console"), String::new()), |c| (c.ip.to_string(), c.country.clone()));
        let done = sqlx::query("INSERT INTO audit (at, admin, ip, country, event, detail) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(identity::now())
            .bind(admin)
            .bind(ip)
            .bind(country)
            .bind(event)
            .bind(detail)
            .execute(&self.pool)
            .await;
        match done {
            Ok(r) => self.publish(live::Event::Audit(json!({
                "id": r.last_insert_rowid(), "at": identity::now(), "admin": admin, "ip": client.map_or("console".to_string(), |c| c.ip.to_string()),
                "country": client.map(|c| c.country.clone()).unwrap_or_default(), "event": event, "detail": detail,
            }))),
            Err(e) => tracing::error!("admin: couldn't record {event}: {e}"),
        }
    }

    /// Adds an admin (or, with `reset`, clears an existing one's password,
    /// second factors and sessions) and answers a one-time setup link.
    pub async fn admin_setup_link(&self, username: &str, reset: bool) -> Result<String, String> {
        let username = username.trim();
        if !(2..=32).contains(&username.len()) || !username.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) {
            return Err("admin names are 2 to 32 letters, digits, _ - and .".into());
        }
        let now = identity::now();
        let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM admins WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        let id = match (existing, reset) {
            (Some(_), false) => return Err(format!("{username} is an admin already; reset them to send a new setup link")),
            (None, true) => return Err(format!("{username} isn't an admin")),
            (Some(id), true) => {
                for sql in [
                    "UPDATE admins SET password_hash = NULL, totp_secret = NULL, totp_pending = NULL, failures = 0, locked_until = 0, disabled = 0 WHERE id = ?",
                    "DELETE FROM passkeys WHERE admin_id = ?",
                    "DELETE FROM recovery_codes WHERE admin_id = ?",
                    "DELETE FROM admin_sessions WHERE admin_id = ?",
                    "DELETE FROM setup_tokens WHERE admin_id = ?",
                ] {
                    sqlx::query(sql).bind(id).execute(&self.pool).await.map_err(|e| e.to_string())?;
                }
                id
            }
            (None, false) => sqlx::query_scalar("INSERT INTO admins (username, created_at) VALUES (?, ?) RETURNING id")
                .bind(username)
                .bind(now)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?,
        };
        let token = auth::random(20);
        sqlx::query("INSERT INTO setup_tokens (token_hash, admin_id, expires_at) VALUES (?, ?, ?)")
            .bind(auth::digest(&token))
            .bind(id)
            .bind(now + SETUP_FOR)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        let origin = self.admin.get().map_or("https://<admin host>", |c| c.origin.as_str());
        Ok(format!("{origin}/#setup={token}"))
    }

    /// Admins, for `coordinator admin list`.
    pub async fn admin_list(&self) -> sqlx::Result<Vec<(String, bool, bool, i64)>> {
        sqlx::query_as("SELECT username, disabled, totp_secret IS NOT NULL, (SELECT COUNT(*) FROM passkeys p WHERE p.admin_id = admins.id) FROM admins ORDER BY username")
            .fetch_all(&self.pool)
            .await
    }

    /// Ends expired sessions, challenges and setup links. Run now and then.
    pub async fn sweep_admin(&self) -> sqlx::Result<()> {
        let now = identity::now();
        sqlx::query("DELETE FROM admin_sessions WHERE last_seen < ? OR created_at < ?")
            .bind(now - IDLE_FOR)
            .bind(now - SESSION_FOR)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM webauthn_challenges WHERE expires_at < ?").bind(now).execute(&self.pool).await?;
        sqlx::query("DELETE FROM setup_tokens WHERE expires_at < ?").bind(now).execute(&self.pool).await?;
        Ok(())
    }

    async fn new_session(&self, admin_id: i64, stage: &str, client: &Client, verified: bool, old: Option<&Session>) -> sqlx::Result<String> {
        if let Some(old) = old {
            sqlx::query("DELETE FROM admin_sessions WHERE token_hash = ?")
                .bind(&old.token_hash)
                .execute(&self.pool)
                .await?;
        }
        let token = auth::random(32);
        let now = identity::now();
        sqlx::query("INSERT INTO admin_sessions (token_hash, admin_id, stage, created_at, last_seen, verified_at, ip, country, user_agent) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(auth::digest(&token))
            .bind(admin_id)
            .bind(stage)
            .bind(now)
            .bind(now)
            .bind(if verified { now } else { 0 })
            .bind(client.ip.to_string())
            .bind(&client.country)
            .bind(&client.user_agent)
            .execute(&self.pool)
            .await?;
        if stage == "full" {
            sqlx::query("UPDATE admins SET last_login = ?, failures = 0, locked_until = 0 WHERE id = ?")
                .bind(now)
                .bind(admin_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(token)
    }

    async fn session(&self, headers: &HeaderMap, client: &Client) -> sqlx::Result<Option<Session>> {
        let Some(token) = cookie(headers, cfg(self).cookie_name()) else { return Ok(None) };
        let row: Option<(String, i64, String, String, i64, i64, i64, String, i64)> = sqlx::query_as(
            "SELECT s.token_hash, s.admin_id, a.username, s.stage, s.verified_at, s.created_at, s.last_seen, s.country, a.disabled
               FROM admin_sessions s JOIN admins a ON a.id = s.admin_id WHERE s.token_hash = ?",
        )
        .bind(auth::digest(&token))
        .fetch_optional(&self.pool)
        .await?;
        let Some((token_hash, admin_id, username, stage, verified_at, created_at, last_seen, country, disabled)) = row else {
            return Ok(None);
        };
        let now = identity::now();
        // Expired, a disabled admin, or the session moved to another country: it's over.
        if now - last_seen > IDLE_FOR || now - created_at > SESSION_FOR || disabled != 0 || (!country.is_empty() && country != client.country) {
            sqlx::query("DELETE FROM admin_sessions WHERE token_hash = ?").bind(&token_hash).execute(&self.pool).await?;
            return Ok(None);
        }
        sqlx::query("UPDATE admin_sessions SET last_seen = ? WHERE token_hash = ?")
            .bind(now)
            .bind(&token_hash)
            .execute(&self.pool)
            .await?;
        Ok(Some(Session {
            token_hash,
            admin_id,
            username,
            stage,
            verified_at,
        }))
    }

    /// A session fully signed in, else the answer to give.
    async fn full(&self, headers: &HeaderMap, client: &Client) -> Result<Session, Response> {
        match self.session(headers, client).await {
            Ok(Some(s)) if s.stage == "full" => {
                if !limits().1.check(&s.token_hash) {
                    return Err(fail(StatusCode::TOO_MANY_REQUESTS, "slow down"));
                }
                Ok(s)
            }
            Ok(_) => Err(fail(StatusCode::UNAUTHORIZED, "sign in first")),
            Err(e) => Err(internal(e)),
        }
    }

    /// A full session with a second factor proved lately.
    async fn recent(&self, headers: &HeaderMap, client: &Client) -> Result<Session, Response> {
        let s = self.full(headers, client).await?;
        if identity::now() - s.verified_at > RECENT {
            return Err(answer(StatusCode::FORBIDDEN, json!({ "error": "confirm it's you first", "reverify": true })));
        }
        Ok(s)
    }

    /// The second factors an admin has: (TOTP, passkeys, recovery codes left).
    async fn factors(&self, admin_id: i64) -> sqlx::Result<(bool, i64, i64)> {
        sqlx::query_as(
            "SELECT totp_secret IS NOT NULL,
                    (SELECT COUNT(*) FROM passkeys WHERE admin_id = ?1),
                    (SELECT COUNT(*) FROM recovery_codes WHERE admin_id = ?1 AND used_at IS NULL)
               FROM admins WHERE id = ?1",
        )
        .bind(admin_id)
        .fetch_one(&self.pool)
        .await
    }

    /// A failed sign-in for `admin_id`: after a few, the account waits.
    async fn failed(&self, admin_id: i64, username: &str, client: &Client, what: &str) {
        let now = identity::now();
        let failures: i64 = sqlx::query_scalar("UPDATE admins SET failures = failures + 1 WHERE id = ? RETURNING failures")
            .bind(admin_id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);
        if failures >= FAILURES_BEFORE_LOCK {
            // 15 minutes, doubling with each failure after, at most a day.
            let wait = (15 * 60_i64) << (failures - FAILURES_BEFORE_LOCK).min(6);
            let _ = sqlx::query("UPDATE admins SET locked_until = ? WHERE id = ?")
                .bind(now + wait.min(86_400))
                .bind(admin_id)
                .execute(&self.pool)
                .await;
        }
        self.audit(username, Some(client), "sign-in failed", what).await;
    }

    /// Issues recovery codes (replacing any), and answers them.
    async fn issue_recovery_codes(&self, admin_id: i64) -> sqlx::Result<Vec<String>> {
        sqlx::query("DELETE FROM recovery_codes WHERE admin_id = ?").bind(admin_id).execute(&self.pool).await?;
        let codes = auth::recovery_codes();
        for code in &codes {
            sqlx::query("INSERT INTO recovery_codes (admin_id, code_hash) VALUES (?, ?)")
                .bind(admin_id)
                .bind(auth::digest(code))
                .execute(&self.pool)
                .await?;
        }
        Ok(codes)
    }

    async fn new_challenge(&self, admin_id: Option<i64>, kind: &str) -> sqlx::Result<(String, String)> {
        let id = auth::random(10);
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
        let challenge = webauthn::b64url(&bytes);
        sqlx::query("INSERT INTO webauthn_challenges (id, admin_id, kind, challenge, expires_at) VALUES (?, ?, ?, ?, ?)")
            .bind(&id)
            .bind(admin_id)
            .bind(kind)
            .bind(&challenge)
            .bind(identity::now() + CHALLENGE_FOR)
            .execute(&self.pool)
            .await?;
        Ok((id, challenge))
    }

    /// Takes a challenge (each works once): its admin and value.
    async fn take_challenge(&self, id: &str, kind: &str) -> sqlx::Result<Option<(Option<i64>, String)>> {
        sqlx::query_as("DELETE FROM webauthn_challenges WHERE id = ? AND kind = ? AND expires_at >= ? RETURNING admin_id, challenge")
            .bind(id)
            .bind(kind)
            .bind(identity::now())
            .fetch_optional(&self.pool)
            .await
    }
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

fn with_cookie(c: &Coordinator, token: &str, mut resp: Response) -> Response {
    let config = cfg(c);
    let secure = if config.secure() { "; Secure" } else { "" };
    let value = format!("{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_FOR}{secure}", config.cookie_name());
    if let Ok(v) = HeaderValue::from_str(&value) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    resp
}

fn without_cookie(c: &Coordinator, mut resp: Response) -> Response {
    let config = cfg(c);
    let secure = if config.secure() { "; Secure" } else { "" };
    if let Ok(v) = HeaderValue::from_str(&format!("{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}", config.cookie_name())) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    resp
}

// Signing in.

async fn session_info(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    match c.session(&headers, &client).await {
        Ok(Some(s)) => {
            let (totp, passkeys, recovery) = match c.factors(s.admin_id).await {
                Ok(f) => f,
                Err(e) => return internal(e),
            };
            ok(json!({
                "stage": s.stage, "username": s.username, "totp": totp, "passkeys": passkeys, "recovery_left": recovery,
                "verified_recently": identity::now() - s.verified_at <= RECENT,
                "you": { "ip": client.ip.to_string(), "country": client.country },
            }))
        }
        Ok(None) => answer(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "not signed in", "you": { "ip": client.ip.to_string(), "country": client.country } }),
        ),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

async fn login(State(c): State<Shared>, Extension(client): Extension<Client>, Json(req): Json<LoginRequest>) -> Response {
    if !limits().0.check(&crate::limit_key(client.ip)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many attempts; wait a minute");
    }
    // Anyone can try a name, and failures are kept 400 days: only as much of it as a name can
    // be (admins' names are at most 32 characters).
    let username: String = req.username.trim().chars().filter(|c| !c.is_control()).take(40).collect();
    let row: Option<(i64, Option<String>, i64, i64)> = match sqlx::query_as("SELECT id, password_hash, disabled, locked_until FROM admins WHERE username = ?")
        .bind(&username)
        .fetch_optional(&c.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    let refused = || fail(StatusCode::UNAUTHORIZED, "wrong name or password");
    let Some((id, hash, disabled, locked_until)) = row else {
        // As slow as a real check, so names can't be told from timing.
        let _ = auth::check_password(
            &req.password,
            "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$k8xR2o1fpLwqgMB0ZDzR2X0Hr7nCMuq1H1IMpJzSo4o",
        );
        c.audit(&username, Some(&client), "sign-in failed", "no such admin").await;
        return refused();
    };
    if identity::now() < locked_until {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many failed sign-ins; this account waits a while");
    }
    let valid = hash.as_deref().is_some_and(|h| auth::check_password(&req.password, h));
    if !valid || disabled != 0 {
        c.failed(id, &username, &client, "password").await;
        return refused();
    }
    let (totp, passkeys, _) = match c.factors(id).await {
        Ok(f) => f,
        Err(e) => return internal(e),
    };
    let stage = if totp || passkeys > 0 { "password" } else { "enroll" };
    match c.new_session(id, stage, &client, false, None).await {
        Ok(token) => with_cookie(&c, &token, ok(json!({ "stage": stage, "totp": totp, "passkeys": passkeys }))),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct CodeRequest {
    code: String,
}

/// A TOTP code: finishes a sign-in, or confirms it's still the admin.
async fn login_totp(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<CodeRequest>) -> Response {
    if !limits().0.check(&crate::limit_key(client.ip)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many attempts; wait a minute");
    }
    let s = match c.session(&headers, &client).await {
        Ok(Some(s)) if s.stage == "password" || s.stage == "full" => s,
        Ok(_) => return fail(StatusCode::UNAUTHORIZED, "sign in with your password first"),
        Err(e) => return internal(e),
    };
    let tries: i64 = sqlx::query_scalar("UPDATE admin_sessions SET totp_tries = totp_tries + 1 WHERE token_hash = ? RETURNING totp_tries")
        .bind(&s.token_hash)
        .fetch_one(&c.pool)
        .await
        .unwrap_or(TOTP_TRIES + 1);
    if tries > TOTP_TRIES {
        let _ = sqlx::query("DELETE FROM admin_sessions WHERE token_hash = ?").bind(&s.token_hash).execute(&c.pool).await;
        return without_cookie(&c, fail(StatusCode::UNAUTHORIZED, "too many wrong codes; sign in again"));
    }
    let row: Option<(Option<String>, i64)> = sqlx::query_as("SELECT totp_secret, totp_last_step FROM admins WHERE id = ?")
        .bind(s.admin_id)
        .fetch_optional(&c.pool)
        .await
        .unwrap_or(None);
    let step = row
        .as_ref()
        .and_then(|(secret, last)| auth::totp_check(secret.as_deref()?, &req.code, identity::now(), *last));
    let Some(step) = step else {
        c.failed(s.admin_id, &s.username, &client, "TOTP code").await;
        return fail(StatusCode::UNAUTHORIZED, "that code isn't right");
    };
    let _ = sqlx::query("UPDATE admins SET totp_last_step = ? WHERE id = ?")
        .bind(step)
        .bind(s.admin_id)
        .execute(&c.pool)
        .await;
    second_factor_done(&c, &s, &client, "TOTP").await
}

async fn login_recovery(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<CodeRequest>) -> Response {
    if !limits().0.check(&crate::limit_key(client.ip)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many attempts; wait a minute");
    }
    let s = match c.session(&headers, &client).await {
        Ok(Some(s)) if s.stage == "password" => s,
        Ok(_) => return fail(StatusCode::UNAUTHORIZED, "sign in with your password first"),
        Err(e) => return internal(e),
    };
    let used = sqlx::query("UPDATE recovery_codes SET used_at = ? WHERE admin_id = ? AND code_hash = ? AND used_at IS NULL")
        .bind(identity::now())
        .bind(s.admin_id)
        .bind(auth::digest(&auth::normalise_code(&req.code)))
        .execute(&c.pool)
        .await;
    match used {
        Ok(r) if r.rows_affected() == 1 => second_factor_done(&c, &s, &client, "a recovery code").await,
        Ok(_) => {
            c.failed(s.admin_id, &s.username, &client, "recovery code").await;
            fail(StatusCode::UNAUTHORIZED, "that recovery code isn't right, or was used")
        }
        Err(e) => internal(e),
    }
}

/// A second factor proved: a full session (a new token), or a re-verified one.
async fn second_factor_done(c: &Coordinator, s: &Session, client: &Client, how: &str) -> Response {
    if s.stage == "full" {
        let _ = sqlx::query("UPDATE admin_sessions SET verified_at = ?, totp_tries = 0 WHERE token_hash = ?")
            .bind(identity::now())
            .bind(&s.token_hash)
            .execute(&c.pool)
            .await;
        return ok(json!({ "stage": "full" }));
    }
    match c.new_session(s.admin_id, "full", client, true, Some(s)).await {
        Ok(token) => {
            c.audit(&s.username, Some(client), "signed in", how).await;
            with_cookie(c, &token, ok(json!({ "stage": "full" })))
        }
        Err(e) => internal(e),
    }
}

async fn passkey_login_begin(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if !limits().0.check(&crate::limit_key(client.ip)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many attempts; wait a minute");
    }
    // After a password (or to confirm it's them), the admin's own passkeys; else any (discoverable).
    let session = c.session(&headers, &client).await.ok().flatten();
    let admin_id = session.as_ref().map(|s| s.admin_id);
    let allow: Vec<Value> = match admin_id {
        Some(id) => sqlx::query_scalar::<_, String>("SELECT id FROM passkeys WHERE admin_id = ?")
            .bind(id)
            .fetch_all(&c.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|id| json!({ "type": "public-key", "id": id }))
            .collect(),
        None => vec![],
    };
    match c.new_challenge(admin_id, "login").await {
        Ok((id, challenge)) => ok(json!({
            "challenge_id": id,
            "options": { "challenge": challenge, "rpId": cfg(&c).rp_id, "timeout": 120_000, "userVerification": "required", "allowCredentials": allow }
        })),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct PasskeyFinish {
    challenge_id: String,
    credential: Value,
    #[serde(default)]
    name: String,
}

async fn passkey_login_finish(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<PasskeyFinish>) -> Response {
    if !limits().0.check(&crate::limit_key(client.ip)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many attempts; wait a minute");
    }
    let Ok(Some((for_admin, challenge))) = c.take_challenge(&req.challenge_id, "login").await else {
        return fail(StatusCode::BAD_REQUEST, "that sign-in expired; try again");
    };
    let cred_id = req.credential["id"].as_str().unwrap_or_default().to_string();
    let row: Option<(i64, i64, Vec<u8>, i64, String, i64, i64)> = sqlx::query_as(
        "SELECT p.admin_id, p.alg, p.public_key, p.sign_count, a.username, a.disabled, a.locked_until FROM passkeys p JOIN admins a ON a.id = p.admin_id WHERE p.id = ?",
    )
    .bind(&cred_id)
    .fetch_optional(&c.pool)
    .await
    .unwrap_or(None);
    let Some((admin_id, alg, public_key, sign_count, username, disabled, locked_until)) = row else {
        c.audit("", Some(&client), "sign-in failed", "unknown passkey").await;
        return fail(StatusCode::UNAUTHORIZED, "this passkey isn't registered here");
    };
    if for_admin.is_some_and(|a| a != admin_id) || disabled != 0 || identity::now() < locked_until {
        return fail(StatusCode::UNAUTHORIZED, "this passkey can't sign in now");
    }
    if webauthn::user_handle(&req.credential).is_some_and(|h| h != admin_id.to_string().into_bytes()) {
        return fail(StatusCode::UNAUTHORIZED, "this passkey belongs to someone else");
    }
    let stored = webauthn::Credential {
        id: cred_id.clone(),
        alg,
        public_key,
        sign_count: u32::try_from(sign_count).unwrap_or(0),
    };
    match webauthn::authenticate(&req.credential, &stored, &challenge, &cfg(&c).site()) {
        Ok(count) => {
            let _ = sqlx::query("UPDATE passkeys SET sign_count = ?, last_used = ? WHERE id = ?")
                .bind(i64::from(count))
                .bind(identity::now())
                .bind(&cred_id)
                .execute(&c.pool)
                .await;
            let session = c.session(&headers, &client).await.ok().flatten().filter(|s| s.admin_id == admin_id);
            match session {
                Some(s) => second_factor_done(&c, &s, &client, "a passkey").await,
                None => match c.new_session(admin_id, "full", &client, true, None).await {
                    Ok(token) => {
                        c.audit(&username, Some(&client), "signed in", "a passkey").await;
                        with_cookie(&c, &token, ok(json!({ "stage": "full" })))
                    }
                    Err(e) => internal(e),
                },
            }
        }
        Err(why) => {
            c.failed(admin_id, &username, &client, &format!("passkey: {why}")).await;
            fail(StatusCode::UNAUTHORIZED, &format!("the passkey didn't check out: {why}"))
        }
    }
}

async fn logout(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Ok(Some(s)) = c.session(&headers, &client).await {
        let _ = sqlx::query("DELETE FROM admin_sessions WHERE token_hash = ?").bind(&s.token_hash).execute(&c.pool).await;
        c.audit(&s.username, Some(&client), "signed out", "").await;
    }
    without_cookie(&c, ok(json!({})))
}

#[derive(Deserialize)]
struct SetupRequest {
    token: String,
    password: String,
}

/// A setup link: the admin chooses a password, then must add a second factor.
async fn setup(State(c): State<Shared>, Extension(client): Extension<Client>, Json(req): Json<SetupRequest>) -> Response {
    if !limits().0.check(&crate::limit_key(client.ip)) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too many attempts; wait a minute");
    }
    let row: Option<(i64, String)> =
        sqlx::query_as("DELETE FROM setup_tokens WHERE token_hash = ? AND expires_at >= ? RETURNING admin_id, (SELECT username FROM admins WHERE id = admin_id)")
            .bind(auth::digest(&req.token))
            .bind(identity::now())
            .fetch_optional(&c.pool)
            .await
            .unwrap_or(None);
    let Some((admin_id, username)) = row else {
        return fail(StatusCode::UNAUTHORIZED, "this setup link expired or was used; ask for a new one");
    };
    if let Some(why) = auth::weak_password(&req.password, &username) {
        // The link stays usable for another try.
        let _ = sqlx::query("INSERT INTO setup_tokens (token_hash, admin_id, expires_at) VALUES (?, ?, ?)")
            .bind(auth::digest(&req.token))
            .bind(admin_id)
            .bind(identity::now() + 3600)
            .execute(&c.pool)
            .await;
        return fail(StatusCode::BAD_REQUEST, why);
    }
    let hash = match auth::hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => return internal(e),
    };
    if let Err(e) = sqlx::query("UPDATE admins SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(admin_id)
        .execute(&c.pool)
        .await
    {
        return internal(e);
    }
    c.audit(&username, Some(&client), "set up their account", "").await;
    match c.new_session(admin_id, "enroll", &client, false, None).await {
        Ok(token) => with_cookie(&c, &token, ok(json!({ "stage": "enroll", "username": username }))),
        Err(e) => internal(e),
    }
}

// The admin's own account.

/// A session that may change the admin's own second factors: one adding its
/// first (enrolling), or a full one.
async fn own(c: &Coordinator, headers: &HeaderMap, client: &Client) -> Result<Session, Response> {
    match c.session(headers, client).await {
        Ok(Some(s)) if s.stage == "full" || s.stage == "enroll" => Ok(s),
        Ok(_) => Err(fail(StatusCode::UNAUTHORIZED, "sign in first")),
        Err(e) => Err(internal(e)),
    }
}

async fn me(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let passkeys: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as("SELECT id, name, created_at, last_used FROM passkeys WHERE admin_id = ? ORDER BY created_at")
        .bind(s.admin_id)
        .fetch_all(&c.pool)
        .await
        .unwrap_or_default();
    let sessions: Vec<(String, String, i64, i64, String, String, String)> =
        sqlx::query_as("SELECT token_hash, stage, created_at, last_seen, ip, country, user_agent FROM admin_sessions WHERE admin_id = ? ORDER BY last_seen DESC")
            .bind(s.admin_id)
            .fetch_all(&c.pool)
            .await
            .unwrap_or_default();
    let (totp, _, recovery) = c.factors(s.admin_id).await.unwrap_or_default();
    ok(json!({
        "username": s.username,
        "totp": totp,
        "recovery_left": recovery,
        "passkeys": passkeys.into_iter().map(|(id, name, created, used)| json!({ "id": id, "name": name, "created_at": created, "last_used": used })).collect::<Vec<_>>(),
        "sessions": sessions.into_iter().map(|(hash, stage, created, seen, ip, country, ua)| json!({
            "id": &hash[..16], "current": hash == s.token_hash, "stage": stage, "created_at": created, "last_seen": seen, "ip": ip, "country": country, "user_agent": ua
        })).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
struct PasswordChange {
    current: String,
    new: String,
}

async fn change_password(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<PasswordChange>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM admins WHERE id = ?")
        .bind(s.admin_id)
        .fetch_one(&c.pool)
        .await
        .unwrap_or(None);
    if !hash.as_deref().is_some_and(|h| auth::check_password(&req.current, h)) {
        c.failed(s.admin_id, &s.username, &client, "password change").await;
        return fail(StatusCode::UNAUTHORIZED, "your current password isn't right");
    }
    if let Some(why) = auth::weak_password(&req.new, &s.username) {
        return fail(StatusCode::BAD_REQUEST, why);
    }
    let Ok(hash) = auth::hash_password(&req.new) else { return internal("hashing failed") };
    let _ = sqlx::query("UPDATE admins SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(s.admin_id)
        .execute(&c.pool)
        .await;
    // Other sessions end.
    let _ = sqlx::query("DELETE FROM admin_sessions WHERE admin_id = ? AND token_hash != ?")
        .bind(s.admin_id)
        .bind(&s.token_hash)
        .execute(&c.pool)
        .await;
    c.audit(&s.username, Some(&client), "changed their password", "").await;
    ok(json!({}))
}

async fn totp_begin(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    let s = match own(&c, &headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if s.stage == "full" && identity::now() - s.verified_at > RECENT {
        return answer(StatusCode::FORBIDDEN, json!({ "error": "confirm it's you first", "reverify": true }));
    }
    let secret = auth::random(20);
    let _ = sqlx::query("UPDATE admins SET totp_pending = ? WHERE id = ?")
        .bind(&secret)
        .bind(s.admin_id)
        .execute(&c.pool)
        .await;
    let issuer = format!("5th Echelon ({})", cfg(&c).rp_id);
    let uri = auth::totp_uri(&secret, &s.username, &issuer);
    ok(json!({ "secret": secret, "uri": uri, "qr": auth::qr_svg(&uri) }))
}

async fn totp_confirm(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<CodeRequest>) -> Response {
    let s = match own(&c, &headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let pending: Option<String> = sqlx::query_scalar("SELECT totp_pending FROM admins WHERE id = ?")
        .bind(s.admin_id)
        .fetch_one(&c.pool)
        .await
        .unwrap_or(None);
    let Some(step) = pending.as_deref().and_then(|p| auth::totp_check(p, &req.code, identity::now(), 0)) else {
        return fail(StatusCode::BAD_REQUEST, "that code isn't right; check the time on your phone");
    };
    let _ = sqlx::query("UPDATE admins SET totp_secret = totp_pending, totp_pending = NULL, totp_last_step = ? WHERE id = ?")
        .bind(step)
        .bind(s.admin_id)
        .execute(&c.pool)
        .await;
    c.audit(&s.username, Some(&client), "added an authenticator app", "").await;
    enrolled(&c, &s, &client).await
}

/// A second factor added: a session that was enrolling becomes full, with
/// recovery codes the first time.
async fn enrolled(c: &Coordinator, s: &Session, client: &Client) -> Response {
    let (_, _, recovery_left) = c.factors(s.admin_id).await.unwrap_or_default();
    let codes = if recovery_left == 0 {
        c.issue_recovery_codes(s.admin_id).await.unwrap_or_default()
    } else {
        vec![]
    };
    if s.stage == "enroll" {
        return match c.new_session(s.admin_id, "full", client, true, Some(s)).await {
            Ok(token) => with_cookie(c, &token, ok(json!({ "stage": "full", "recovery_codes": codes }))),
            Err(e) => internal(e),
        };
    }
    ok(json!({ "stage": "full", "recovery_codes": codes }))
}

async fn totp_remove(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (_, passkeys, _) = c.factors(s.admin_id).await.unwrap_or_default();
    if passkeys == 0 {
        return fail(StatusCode::BAD_REQUEST, "add a passkey first: an account always keeps a second factor");
    }
    let _ = sqlx::query("UPDATE admins SET totp_secret = NULL WHERE id = ?").bind(s.admin_id).execute(&c.pool).await;
    c.audit(&s.username, Some(&client), "removed their authenticator app", "").await;
    ok(json!({}))
}

async fn passkey_add_begin(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    let s = match own(&c, &headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if s.stage == "full" && identity::now() - s.verified_at > RECENT {
        return answer(StatusCode::FORBIDDEN, json!({ "error": "confirm it's you first", "reverify": true }));
    }
    let existing: Vec<String> = sqlx::query_scalar("SELECT id FROM passkeys WHERE admin_id = ?")
        .bind(s.admin_id)
        .fetch_all(&c.pool)
        .await
        .unwrap_or_default();
    let config = cfg(&c);
    match c.new_challenge(Some(s.admin_id), "register").await {
        Ok((id, challenge)) => ok(json!({
            "challenge_id": id,
            "options": {
                "challenge": challenge,
                "rp": { "id": config.rp_id, "name": "5th Echelon network admin" },
                "user": { "id": webauthn::b64url(s.admin_id.to_string().as_bytes()), "name": s.username, "displayName": s.username },
                "pubKeyCredParams": [
                    { "type": "public-key", "alg": webauthn::ES256 },
                    { "type": "public-key", "alg": webauthn::EDDSA },
                    { "type": "public-key", "alg": webauthn::RS256 },
                ],
                "timeout": 120_000,
                "attestation": "none",
                "authenticatorSelection": { "residentKey": "required", "requireResidentKey": true, "userVerification": "required" },
                "excludeCredentials": existing.into_iter().map(|id| json!({ "type": "public-key", "id": id })).collect::<Vec<_>>(),
            }
        })),
        Err(e) => internal(e),
    }
}

async fn passkey_add_finish(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<PasskeyFinish>) -> Response {
    let s = match own(&c, &headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Ok(Some((Some(for_admin), challenge))) = c.take_challenge(&req.challenge_id, "register").await else {
        return fail(StatusCode::BAD_REQUEST, "that took too long; try again");
    };
    if for_admin != s.admin_id {
        return fail(StatusCode::BAD_REQUEST, "that passkey request was someone else's");
    }
    let cred = match webauthn::register(&req.credential, &challenge, &cfg(&c).site()) {
        Ok(cred) => cred,
        Err(why) => return fail(StatusCode::BAD_REQUEST, &format!("the passkey couldn't be added: {why}")),
    };
    let name: String = req.name.trim().chars().filter(|ch| !ch.is_control()).take(40).collect();
    let name = if name.is_empty() { String::from("Passkey") } else { name };
    let done = sqlx::query("INSERT INTO passkeys (id, admin_id, name, alg, public_key, sign_count, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&cred.id)
        .bind(s.admin_id)
        .bind(&name)
        .bind(cred.alg)
        .bind(&cred.public_key)
        .bind(i64::from(cred.sign_count))
        .bind(identity::now())
        .execute(&c.pool)
        .await;
    if let Err(e) = done {
        return if e.as_database_error().is_some_and(|d| d.is_unique_violation()) {
            fail(StatusCode::CONFLICT, "that passkey is registered already")
        } else {
            internal(e)
        };
    }
    c.audit(&s.username, Some(&client), "added a passkey", &name).await;
    enrolled(&c, &s, &client).await
}

async fn passkey_remove(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (totp, passkeys, _) = c.factors(s.admin_id).await.unwrap_or_default();
    if !totp && passkeys <= 1 {
        return fail(
            StatusCode::BAD_REQUEST,
            "add an authenticator app or another passkey first: an account always keeps a second factor",
        );
    }
    let name: Option<String> = sqlx::query_scalar("DELETE FROM passkeys WHERE id = ? AND admin_id = ? RETURNING name")
        .bind(&id)
        .bind(s.admin_id)
        .fetch_optional(&c.pool)
        .await
        .unwrap_or(None);
    match name {
        Some(name) => {
            c.audit(&s.username, Some(&client), "removed a passkey", &name).await;
            ok(json!({}))
        }
        None => fail(StatusCode::NOT_FOUND, "no such passkey"),
    }
}

async fn new_recovery_codes(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    match c.issue_recovery_codes(s.admin_id).await {
        Ok(codes) => {
            c.audit(&s.username, Some(&client), "made new recovery codes", "").await;
            ok(json!({ "recovery_codes": codes }))
        }
        Err(e) => internal(e),
    }
}

async fn end_session(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if id.len() != 16 {
        return fail(StatusCode::BAD_REQUEST, "not a session");
    }
    let _ = sqlx::query("DELETE FROM admin_sessions WHERE admin_id = ? AND substr(token_hash, 1, 16) = ?")
        .bind(s.admin_id)
        .bind(&id)
        .execute(&c.pool)
        .await;
    c.audit(&s.username, Some(&client), "ended a session", &id).await;
    ok(json!({}))
}

// Admins and restrictions.

async fn admins(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let rows: Vec<(i64, String, i64, Option<i64>, i64, bool, i64, bool)> = sqlx::query_as(
        "SELECT id, username, created_at, last_login, disabled, totp_secret IS NOT NULL,
                (SELECT COUNT(*) FROM passkeys p WHERE p.admin_id = admins.id), password_hash IS NOT NULL
           FROM admins ORDER BY username",
    )
    .fetch_all(&c.pool)
    .await
    .unwrap_or_default();
    ok(json!({ "admins": rows.into_iter().map(|(id, name, created, last, disabled, totp, passkeys, set_up)| json!({
        "id": id, "username": name, "created_at": created, "last_login": last, "disabled": disabled != 0, "totp": totp, "passkeys": passkeys, "set_up": set_up
    })).collect::<Vec<_>>() }))
}

#[derive(Deserialize)]
struct NewAdmin {
    username: String,
}

async fn add_admin(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<NewAdmin>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    match c.admin_setup_link(&req.username, false).await {
        Ok(link) => {
            c.audit(&s.username, Some(&client), "added an admin", req.username.trim()).await;
            ok(json!({ "link": link, "expires_in": SETUP_FOR }))
        }
        Err(e) => fail(StatusCode::BAD_REQUEST, &e),
    }
}

async fn admin_action(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path((id, action)): Path<(i64, String)>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Ok(Some(username)) = sqlx::query_scalar::<_, String>("SELECT username FROM admins WHERE id = ?")
        .bind(id)
        .fetch_optional(&c.pool)
        .await
    else {
        return fail(StatusCode::NOT_FOUND, "no such admin");
    };
    if id == s.admin_id && action != "reset" {
        return fail(StatusCode::BAD_REQUEST, "you can't do that to yourself");
    }
    let result = match action.as_str() {
        "disable" => {
            let others: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admins WHERE disabled = 0 AND id != ?")
                .bind(id)
                .fetch_one(&c.pool)
                .await
                .unwrap_or(0);
            if others == 0 {
                return fail(StatusCode::BAD_REQUEST, "that's the last admin");
            }
            let _ = sqlx::query("UPDATE admins SET disabled = 1 WHERE id = ?").bind(id).execute(&c.pool).await;
            let _ = sqlx::query("DELETE FROM admin_sessions WHERE admin_id = ?").bind(id).execute(&c.pool).await;
            json!({})
        }
        "enable" => {
            let _ = sqlx::query("UPDATE admins SET disabled = 0, failures = 0, locked_until = 0 WHERE id = ?")
                .bind(id)
                .execute(&c.pool)
                .await;
            json!({})
        }
        "reset" if id != s.admin_id => match c.admin_setup_link(&username, true).await {
            Ok(link) => json!({ "link": link, "expires_in": SETUP_FOR }),
            Err(e) => return fail(StatusCode::BAD_REQUEST, &e),
        },
        "reset" => return fail(StatusCode::BAD_REQUEST, "change your own sign-in under Your account"),
        "remove" => {
            let _ = sqlx::query("DELETE FROM admins WHERE id = ?").bind(id).execute(&c.pool).await;
            json!({})
        }
        _ => return fail(StatusCode::NOT_FOUND, "no such action"),
    };
    c.audit(&s.username, Some(&client), &format!("{action} admin"), &username).await;
    ok(result)
}

async fn restrictions(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    match c.restrictions().await {
        Ok(r) => ok(json!({ "networks": r.networks, "countries": r.countries, "you": { "ip": client.ip.to_string(), "country": client.country } })),
        Err(e) => internal(e),
    }
}

async fn set_restrictions(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<auth::Restrictions>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let r = match req.normalised() {
        Ok(r) => r,
        Err(e) => return fail(StatusCode::BAD_REQUEST, &e),
    };
    // Never lock out the admin making the change.
    if !r.allows(client.ip, &client.country) {
        return fail(
            StatusCode::BAD_REQUEST,
            &format!(
                "those rules would lock you out (you're at {} in {}); include yourself",
                client.ip,
                if client.country.is_empty() { "an unknown country" } else { &client.country }
            ),
        );
    }
    if let Err(e) = c.save_restrictions(&r).await {
        return internal(e);
    }
    c.audit(
        &s.username,
        Some(&client),
        "changed the sign-in restrictions",
        &format!("networks {:?}, countries {:?}", r.networks, r.countries),
    )
    .await;
    ok(json!({ "networks": r.networks, "countries": r.countries }))
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    before: Option<i64>,
}

async fn audit(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(page): Query<Page>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let rows: Vec<(i64, i64, String, String, String, String, String)> =
        sqlx::query_as("SELECT id, at, admin, ip, country, event, detail FROM audit WHERE id < ? ORDER BY id DESC LIMIT 200")
            .bind(page.before.unwrap_or(i64::MAX))
            .fetch_all(&c.pool)
            .await
            .unwrap_or_default();
    ok(json!({ "events": rows.into_iter().map(|(id, at, admin, ip, country, event, detail)| json!({
        "id": id, "at": at, "admin": admin, "ip": ip, "country": country, "event": event, "detail": detail
    })).collect::<Vec<_>>() }))
}

// The network.

#[derive(Deserialize)]
struct Range {
    #[serde(default)]
    range: i64,
}

fn range(r: i64) -> Option<i64> {
    [0, 3600, 6 * 3600, 86_400, 7 * 86_400, 30 * 86_400, 365 * 86_400].contains(&r).then_some(r)
}

async fn overview(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.admin_overview().await.map_or_else(internal, ok)
}

impl Coordinator {
    /// The network at a glance: every server with its listing, latest metrics, update state
    /// and place; the rollout; the day's peak. What the Overview shows, and what live
    /// connections are sent as it changes.
    pub(crate) async fn admin_overview(&self) -> sqlx::Result<Value> {
        let now = identity::now();
        let rollout = self.rollout().await?;
        let latest: std::collections::HashMap<String, Value> = self.latest_metrics().await?.into_iter().collect();
        let rows: Vec<(String, Option<String>, Option<i64>, Option<String>, i64)> =
            sqlx::query_as("SELECT id, listing, last_seen, update_status, joined_at FROM servers ORDER BY id")
                .fetch_all(&self.pool)
                .await?;
        let pings: Vec<(String, Option<f64>)> = sqlx::query_as(
            "SELECT p.server_id, p.ms FROM server_pings p JOIN (SELECT server_id, MAX(at) AS at FROM server_pings GROUP BY server_id) l ON l.server_id = p.server_id AND l.at = p.at",
        )
        .fetch_all(&self.pool)
        .await?;
        let pings: std::collections::HashMap<String, Option<f64>> = pings.into_iter().collect();
        let clashes = self.name_clashes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let mut servers = Vec::new();
        for (id, listing, last_seen, status, joined_at) in rows {
            let l: Value = listing.and_then(|l| serde_json::from_str(&l).ok()).unwrap_or_default();
            let place = server_place(self, l["host"].as_str().unwrap_or_default()).await;
            let version = l["version"].as_str().unwrap_or_default();
            let auto_update = l["auto_update"].as_bool().unwrap_or(false);
            servers.push(json!({
                "id": id,
                "listing": l,
                "last_seen": last_seen,
                "online": last_seen.is_some_and(|t| now - t <= crate::LISTED_FOR.as_secs() as i64),
                "joined_at": joined_at,
                "update": status.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                "delisted": crate::updates::delisted(&rollout, version, auto_update, now),
                "metrics": latest.get(&id),
                "ping_ms": pings.get(&id).copied().flatten(),
                "place": place,
                "name_clashes": clashes.get(&id).cloned().unwrap_or_default(),
            }));
        }
        let peak: Option<f64> = sqlx::query_scalar("SELECT MAX(json_extract(data, '$.max_players')) FROM hourly WHERE hour >= ?")
            .bind(now - 86_400)
            .fetch_one(&self.pool)
            .await?;
        Ok(json!({
            "now": now,
            "coordinator": { "version": env!("FE_RELEASE"), "geo": self.geo.get().is_some_and(|g| g.ready()) },
            "rollout": rollout,
            "servers": servers,
            "peak_24h": peak,
            "alerts": self.open_alerts().await?,
            "open_reports": self.open_reports().await?,
            "attribution": geo::ATTRIBUTION,
        }))
    }
}

/// Where a server is, for the map: its host's address (a public one), located.
async fn server_place(c: &Coordinator, host: &str) -> Option<Value> {
    if host.is_empty() {
        return None;
    }
    let ip = match host.parse::<IpAddr>() {
        Ok(ip) => ip,
        Err(_) => tokio::time::timeout(std::time::Duration::from_secs(2), tokio::net::lookup_host((host, 443)))
            .await
            .ok()?
            .ok()?
            .map(|a| a.ip())
            .find(|ip| crate::public_ip(*ip))?,
    };
    if !crate::public_ip(ip) {
        return None;
    }
    let p = c.geo.get()?.lookup(ip)?;
    Some(json!({ "lat": p.lat, "lon": p.lon, "city": p.city, "country": p.country }))
}

async fn series(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<Range>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let Some(r) = range(q.range).filter(|r| *r > 0) else {
        return fail(StatusCode::BAD_REQUEST, "not a range");
    };
    c.series(r).await.map_or_else(internal, ok)
}

async fn places(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<Range>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let Some(r) = range(q.range) else { return fail(StatusCode::BAD_REQUEST, "not a range") };
    c.places(r).await.map_or_else(internal, ok)
}

async fn activity(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<Range>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let Some(r) = range(q.range) else { return fail(StatusCode::BAD_REQUEST, "not a range") };
    c.activity(r).await.map_or_else(internal, ok)
}

async fn pings(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<Range>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let Some(r) = range(q.range).filter(|r| *r > 0) else {
        return fail(StatusCode::BAD_REQUEST, "not a range");
    };
    c.player_pings(r).await.map_or_else(internal, ok)
}

#[derive(Deserialize)]
struct Label {
    kind: String,
    id: i64,
    name: String,
}

async fn set_label(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(l): Json<Label>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !matches!(l.kind.as_str(), "map" | "game_mode") || l.name.chars().count() > 48 || l.name.chars().any(char::is_control) {
        return fail(StatusCode::BAD_REQUEST, "a map or game_mode, named in up to 48 characters");
    }
    match c.set_label(&l.kind, l.id, &l.name).await {
        Ok(()) => {
            c.audit(&s.username, Some(&client), "named a map or mode", &format!("{} {} = {}", l.kind, l.id, l.name.trim()))
                .await;
            ok(json!({}))
        }
        Err(e) => internal(e),
    }
}

/// A report's period: a day, a week, 30 days or a year.
fn report_range(r: i64) -> Option<i64> {
    [86_400, 7 * 86_400, 30 * 86_400, 365 * 86_400].contains(&r).then_some(r)
}

#[derive(Deserialize)]
struct ReportQuery {
    #[serde(default)]
    range: i64,
    #[serde(default)]
    server: Option<String>,
}

async fn bandwidth(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<ReportQuery>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let Some(range) = report_range(q.range) else {
        return fail(StatusCode::BAD_REQUEST, "not a range");
    };
    c.bandwidth(range, q.server.as_deref().filter(|s| !s.is_empty())).await.map_or_else(internal, ok)
}

async fn players_report(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<ReportQuery>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let Some(range) = report_range(q.range) else {
        return fail(StatusCode::BAD_REQUEST, "not a range");
    };
    c.players_report(range).await.map_or_else(internal, ok)
}

#[derive(Deserialize)]
struct Allowance {
    server: String,
    /// Terabytes a month; 0 for none.
    tb: f64,
}

async fn set_allowance(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(a): Json<Allowance>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !(0.0..=10_000.0).contains(&a.tb) || !a.tb.is_finite() {
        return fail(StatusCode::BAD_REQUEST, "an allowance is 0 (none) to 10,000 TB");
    }
    let known: Option<String> = match sqlx::query_scalar("SELECT id FROM servers WHERE id = ?").bind(&a.server).fetch_optional(&c.pool).await {
        Ok(k) => k,
        Err(e) => return internal(e),
    };
    if known.is_none() {
        return fail(StatusCode::NOT_FOUND, "no such server");
    }
    let key = format!("allowance:{}", a.server);
    let done = if a.tb > 0.0 {
        c.set_setting(&key, &a.tb.to_string()).await
    } else {
        sqlx::query("DELETE FROM settings WHERE key = ?").bind(&key).execute(&c.pool).await.map(drop)
    };
    match done {
        Ok(()) => {
            let what = if a.tb > 0.0 { format!("{} TB a month", a.tb) } else { "none".into() };
            c.audit(&s.username, Some(&client), "set a traffic allowance", &format!("{}: {what}", a.server)).await;
            ok(json!({}))
        }
        Err(e) => internal(e),
    }
}

async fn alerts(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.alerts().await.map_or_else(internal, ok)
}

#[derive(Deserialize)]
struct Webhook {
    url: String,
}

/// Sets (or, empty, removes) the alert webhook. It carries the network's alerts out, so
/// it wants a second factor proved lately.
async fn set_webhook(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(w): Json<Webhook>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let done = if w.url.trim().is_empty() {
        sqlx::query("DELETE FROM settings WHERE key = ?")
            .bind(crate::alerts::WEBHOOK_SETTING)
            .execute(&c.pool)
            .await
            .map(drop)
    } else {
        match crate::alerts::valid_webhook(&w.url) {
            Ok(u) => c.set_setting(crate::alerts::WEBHOOK_SETTING, u.as_str()).await,
            Err(why) => return fail(StatusCode::BAD_REQUEST, &why),
        }
    };
    match done {
        Ok(()) => {
            let host = reqwest::Url::parse(w.url.trim())
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| "none".into());
            c.audit(&s.username, Some(&client), "set the alert webhook", &host).await;
            ok(json!({}))
        }
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct ReportAlerts {
    mode: String,
}

/// Which new player reports go to the webhook: off, problems (rated bad or with problems
/// ticked) or all.
async fn set_report_alerts(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<ReportAlerts>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !crate::reports::ALERT_MODES.contains(&req.mode.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "report alerts are off, problems or all");
    }
    match c.set_setting(crate::reports::ALERTS_SETTING, &req.mode).await {
        Ok(()) => {
            c.audit(&s.username, Some(&client), "set report alerts", &req.mode).await;
            ok(json!({ "report_alerts": req.mode }))
        }
        Err(e) => internal(e),
    }
}

async fn test_webhook(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let url = match c.setting(crate::alerts::WEBHOOK_SETTING).await {
        Ok(Some(u)) => u,
        Ok(None) => return fail(StatusCode::BAD_REQUEST, "no webhook set"),
        Err(e) => return internal(e),
    };
    match crate::alerts::post_webhook(&url, "✅ Test from the SCBL Network admin UI: alerts will arrive here.").await {
        Ok(()) => ok(json!({ "message": "Sent. Check the channel." })),
        Err(why) => fail(StatusCode::BAD_GATEWAY, &why),
    }
}

async fn updates(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let result: sqlx::Result<Value> = async {
        let releases = c.releases().await?;
        Ok(json!({
            "rollout": c.rollout().await?,
            "releases": releases.into_iter().map(|(v, page, published, seen)| json!({ "version": v, "page": page, "published_at": published, "seen_at": seen })).collect::<Vec<_>>(),
            "repo": crate::updates::REPO,
        }))
    }
    .await;
    result.map_or_else(internal, ok)
}

#[derive(Deserialize, Default)]
struct UpdateAction {
    #[serde(default)]
    version: String,
}

async fn update_action(
    State(c): State<Shared>,
    Extension(client): Extension<Client>,
    headers: HeaderMap,
    Path(action): Path<String>,
    body: Option<Json<UpdateAction>>,
) -> Response {
    let sensitive = matches!(action.as_str(), "rollback" | "release" | "promote");
    let s = match if sensitive {
        c.recent(&headers, &client).await
    } else {
        c.full(&headers, &client).await
    } {
        Ok(s) => s,
        Err(r) => return r,
    };
    let version = body.map(|b| b.0.version).unwrap_or_default();
    let result: Result<String, String> = match action.as_str() {
        "pause" => c.set_paused(true).await.map(|()| "paused".into()).map_err(|e| e.to_string()),
        "resume" => c.set_paused(false).await.map(|()| "resumed".into()).map_err(|e| e.to_string()),
        "pin" => c.set_pinned(true).await.map(|()| "pinned".into()).map_err(|e| e.to_string()),
        "unpin" => c.set_pinned(false).await.map(|()| "unpinned".into()).map_err(|e| e.to_string()),
        "promote" => c.promote().await.map(|()| "rolling out to every server".into()).map_err(|e| e.to_string()),
        "halt" => c.halt("halted by an admin").await.map(|()| "halted".into()).map_err(|e| e.to_string()),
        "rollback" => c.roll_back().await.map(|v| format!("rolling back to {v}")),
        "release" => c.roll_out(version.trim()).await.map(|()| format!("rolling out {}", version.trim())),
        "check" => match crate::updates::latest_signed(&crate::http()).await {
            Ok((v, page, published)) => c
                .release_found(&v, &page, &published)
                .await
                .map(|found| match found {
                    crate::updates::Found::Started => format!("{v} is new: rolling it out"),
                    crate::updates::Found::Kept => format!("{v} is the latest signed release"),
                    crate::updates::Found::Held(why) => format!("{v} isn't rolled out on its own ({why})"),
                })
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        },
        _ => return fail(StatusCode::NOT_FOUND, "no such action"),
    };
    match result {
        Ok(msg) => {
            c.audit(&s.username, Some(&client), &format!("updates: {action}"), &msg).await;
            ok(json!({ "message": msg }))
        }
        Err(e) => fail(StatusCode::BAD_REQUEST, &e),
    }
}

async fn remove_server(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    match c.remove_server(&id).await {
        Ok(true) => {
            c.audit(&s.username, Some(&client), "removed a server", &id).await;
            ok(json!({ "message": format!("Removed {id}. Make a new join token if it shouldn't join again.") }))
        }
        Ok(false) => fail(StatusCode::NOT_FOUND, "no such server"),
        Err(e) => internal(e),
    }
}

/// Releases the names a server reserved for identities that never played (see
/// [`Coordinator::purge_unused_names`]).
async fn purge_names(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    match c.purge_unused_names(&id).await {
        Ok(n) => {
            c.audit(&s.username, Some(&client), "released a server's unused names", &format!("{id}: {n} links")).await;
            ok(json!({ "message": format!("Removed {n} unused links of {id}, and the names only they held.") }))
        }
        Err(e) => internal(e),
    }
}
