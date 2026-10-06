//! The admin UI's Support page (see [`crate::support`]), on the community network's
//! coordinator only (with the roadmap):
//!
//! * `GET /api/support?status=active|open|waiting|resolved|all`: the conversations, newest
//!   first, with what's unread for the admins.
//! * `GET /api/support/<identity>`: one conversation (marked read), with the player's accounts.
//! * `POST /api/support/<identity>` `{text}`: an answer, which the player reads in their
//!   launcher (and is told of in the overlay); the conversation then waits on them.
//! * `PUT /api/support/<identity>/status` `{status}`: open, waiting or resolved.
//! * `GET /api/support/<identity>/files/<message>/<name>`: a file, unpacked, to save.

use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use axum::routing::put;
use axum::Extension;
use axum::Json;
use axum::Router;
use serde::Deserialize;

use super::fail;
use super::internal;
use super::ok;
use super::Client;
use super::Shared;
use crate::admin::live::Event;
use crate::support;

pub(super) fn routes() -> Router<Shared> {
    Router::new()
        .route("/support", get(list))
        .route("/support/{identity}", get(thread).post(answer))
        .route("/support/{identity}/status", put(set_status))
        .route("/support/{identity}/files/{message}/{name}", get(file))
}

fn no_support() -> Response {
    fail(StatusCode::NOT_FOUND, "support is on the community network's coordinator only")
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    status: String,
}

async fn list(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    if !c.has_roadmap() {
        return no_support();
    }
    let status = if q.status.is_empty() { "active" } else { q.status.as_str() };
    if !["active", "all"].contains(&status) && !support::STATUSES.contains(&status) {
        return fail(StatusCode::BAD_REQUEST, "status is active, open, waiting, resolved or all");
    }
    c.support_list(status).await.map_or_else(internal, ok)
}

async fn thread(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(identity): Path<String>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    if !c.has_roadmap() {
        return no_support();
    }
    match c.support_thread(&identity).await {
        Ok(Some((v, read_now))) => {
            // Read now: the rail's count goes down for every admin (only then: their pages
            // reload on it, and reading again mustn't set them off again).
            if read_now {
                c.publish(Event::Support);
            }
            ok(v)
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such conversation"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct Answer {
    text: String,
}

async fn answer(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(identity): Path<String>, Json(req): Json<Answer>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !c.has_roadmap() {
        return no_support();
    }
    let text = req.text.trim();
    if text.is_empty() || !crate::reports::printable(text, support::MAX_TEXT, true) {
        return fail(StatusCode::BAD_REQUEST, "an answer is 1-2000 printable characters");
    }
    match c.answer_support(&identity, &s.username, text, identity::now()).await {
        Ok(Some(_)) => {
            c.audit(&s.username, Some(&client), "support: answered", &identity::short(&identity)).await;
            c.publish(Event::Support);
            c.support_thread(&identity).await.map_or_else(internal, |t| ok(t.map(|(v, _)| v).unwrap_or_default()))
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such conversation"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct StatusChange {
    status: String,
}

async fn set_status(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(identity): Path<String>, Json(req): Json<StatusChange>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !c.has_roadmap() {
        return no_support();
    }
    if !support::STATUSES.contains(&req.status.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "a conversation is open, waiting or resolved");
    }
    match c.set_support_status(&identity, &req.status).await {
        Ok(true) => {
            c.audit(&s.username, Some(&client), &format!("support: marked {}", req.status), &identity::short(&identity))
                .await;
            c.publish(Event::Support);
            ok(serde_json::json!({ "status": req.status }))
        }
        Ok(false) => fail(StatusCode::NOT_FOUND, "no such conversation"),
        Err(e) => internal(e),
    }
}

/// A file a player sent, as text to save: never shown as a page.
async fn file(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path((identity, message, name)): Path<(String, i64, String)>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    if !c.has_roadmap() {
        return no_support();
    }
    if !crate::reports::valid_file_name(&name) {
        return fail(StatusCode::NOT_FOUND, "no such file");
    }
    // The message must be in this conversation: the identity in the address isn't decoration.
    let in_thread: Option<i64> = match sqlx::query_scalar("SELECT id FROM support_messages WHERE id = ? AND identity = ?")
        .bind(message)
        .bind(&identity)
        .fetch_optional(&c.pool)
        .await
    {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    if in_thread.is_none() {
        return fail(StatusCode::NOT_FOUND, "no such file");
    }
    match c.support_file(message, &name).await {
        Ok(Some(data)) => {
            let mut resp = data.into_response();
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
            // The name is checked (letters, digits, . _ -): nothing to escape.
            if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"support-{message}-{name}\"")) {
                h.insert(header::CONTENT_DISPOSITION, v);
            }
            resp
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such file"),
        Err(e) => internal(e),
    }
}
