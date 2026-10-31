//! The admin UI's replay viewer's recordings (see [`crate::recordings`]).
//!
//! * `POST /api/recordings/<server>/<player>/<id>`: asks the player's server for a recording
//!   its `recording` event names (the server sends it within a pulse or two); with the
//!   account's name, whose own character the recording holds.
//! * `GET /api/recordings/<server>/<id>`: the recording as it was sent (gzip, the `replay`
//!   crate's format, which the page unpacks), or while it isn't here, how the ask is going.

use axum::extract::Path;
use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse as _;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
use axum::Extension;
use axum::Router;
use serde_json::json;

use super::fail;
use super::internal;
use super::ok;
use super::Client;
use super::Shared;

pub(super) fn routes() -> Router<Shared> {
    Router::new()
        .route("/recordings/{server}/{player}/{id}", post(ask))
        .route("/recordings/{server}/{id}", get(fetch))
}

async fn ask(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path((server, player, id)): Path<(String, i64, String)>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !crate::recordings::valid_id(&id) {
        return fail(StatusCode::BAD_REQUEST, "not a recording id");
    }
    match c.recording_known(&server, player, &id).await {
        Ok(true) => {}
        Ok(false) => return fail(StatusCode::NOT_FOUND, "that player's server never told of that recording"),
        Err(e) => return internal(e),
    }
    // The account's name as the network has it: whose own character the recording holds.
    let name = match c.player_name(&server, player).await {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    if c.recording_path(&server, &id).is_some_and(|p| p.exists()) {
        return ok(json!({ "status": "here", "name": name }));
    }
    match c.ask_for_recording(&server, player, &id, &s.username).await {
        Ok(action) => {
            c.audit(&s.username, Some(&client), "recording: asked for", &format!("{id} of #{player} on {server}")).await;
            ok(json!({ "status": "pending", "action": action, "name": name }))
        }
        Err(e) => internal(e),
    }
}

async fn fetch(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path((server, id)): Path<(String, String)>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let Some(path) = c.recording_path(&server, &id) else {
        return fail(StatusCode::NOT_FOUND, "no such recording");
    };
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return (
            [(header::CONTENT_TYPE, "application/octet-stream"), (header::CACHE_CONTROL, "private, max-age=3600")],
            bytes,
        )
            .into_response();
    }
    match c.recording_asked(&server, &id).await {
        Ok(Some((status, message))) => (StatusCode::ACCEPTED, axum::Json(json!({ "status": status, "message": message }))).into_response(),
        Ok(None) => fail(StatusCode::NOT_FOUND, "not asked for yet"),
        Err(e) => internal(e),
    }
}
