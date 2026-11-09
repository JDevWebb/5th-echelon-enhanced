//! The admin UI's Crashes page (see [`crate::crashes`]).
//!
//! * `GET /api/crashes?days=`: the crashes and freezes of the last `days` days (7 when left
//!   out, at most 30) grouped by where, the crash fixes' counts, and the marks.
//! * `PUT /api/crashes/mark` `{signature, status, fixed_in?, note?}`: marks a group fixed,
//!   known or watching (`status` empty clears it).

use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::routing::put;
use axum::Extension;
use axum::Json;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use super::fail;
use super::internal;
use super::ok;
use super::Client;
use super::Shared;

pub(super) fn routes() -> Router<Shared> {
    Router::new().route("/crashes", get(list)).route("/crashes/mark", put(mark))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    days: Option<i64>,
}

async fn list(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.crashes_view(q.days.unwrap_or(7)).await.map_or_else(internal, ok)
}

#[derive(Deserialize)]
struct Mark {
    signature: String,
    status: String,
    #[serde(default)]
    fixed_in: String,
    #[serde(default)]
    note: String,
}

async fn mark(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(req): Json<Mark>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let signature = req.signature.trim();
    let printable = |t: &str, max: usize| t.chars().count() <= max && !t.chars().any(|ch| ch.is_control() || crate::hidden_char(ch));
    if signature.is_empty() || !printable(signature, 160) {
        return fail(StatusCode::BAD_REQUEST, "a crash's signature is up to 160 printable characters");
    }
    if !req.status.is_empty() && !crate::crashes::STATUSES.contains(&req.status.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "a crash is marked fixed, known or watching");
    }
    let fixed_in = req.fixed_in.trim();
    if req.status == "fixed" && (fixed_in.is_empty() || !printable(fixed_in, 32)) {
        return fail(StatusCode::BAD_REQUEST, "say the release that fixes it (0.5.0)");
    }
    let note = req.note.trim();
    if !printable(note, 300) {
        return fail(StatusCode::BAD_REQUEST, "a note is up to 300 printable characters");
    }
    if let Err(e) = c.mark_crash(signature, &req.status, fixed_in, note, &s.username).await {
        return internal(e);
    }
    let what = if req.status.is_empty() {
        "unmarked".to_string()
    } else {
        format!("marked {}", req.status)
    };
    c.audit(
        &s.username,
        Some(&client),
        &format!("crash: {what}"),
        &format!("{signature}{}", if fixed_in.is_empty() { String::new() } else { format!(" in {fixed_in}") }),
    )
    .await;
    ok(json!({ "ok": true }))
}
