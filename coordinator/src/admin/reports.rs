//! The admin UI's player reports (see [`crate::reports`]):
//!
//! * `GET /api/reports?status=open|resolved|all&server=&problem=&q=&page=`: a page of reports.
//! * `GET /api/reports/<id>`: one report, with its summary, the server's log lines and the
//!   player's other reports.
//! * `GET /api/reports/<id>/files/<name>`: a file, decompressed, as a plain-text download.
//! * `POST /api/reports/<id>` `{status, note?, reply?}`: resolves or reopens it; `reply` is
//!   for the player, who reads it in their launcher.
//! * `DELETE /api/reports/<id>`: deletes it with its files (a second factor proved lately).

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
use crate::admin::live::Event;
use crate::reports;

pub(super) fn routes() -> Router<Shared> {
    Router::new()
        .route("/reports", get(list))
        .route("/reports/{id}", get(detail).post(set_status).delete(remove))
        .route("/reports/{id}/files/{name}", get(file))
}

async fn list(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<reports::ListQuery>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.report_list(&q).await.map_or_else(internal, ok)
}

async fn detail(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    match c.report_detail(&id).await {
        Ok(Some(v)) => ok(v),
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such report"),
        Err(e) => internal(e),
    }
}

/// A file of a report, as text to save: never shown as a page.
async fn file(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path((id, name)): Path<(String, String)>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    match c.report_file(&id, &name).await {
        Ok(Some(Some(text))) => {
            let mut resp = text.into_response();
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
            // The id and name are checked (hex, and letters, digits, . _ -): nothing to escape.
            if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{}-{name}\"", id.to_ascii_lowercase())) {
                h.insert(header::CONTENT_DISPOSITION, v);
            }
            resp
        }
        Ok(Some(None)) => fail(StatusCode::GONE, "this file was removed to keep the reports' storage under its cap"),
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such file"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct StatusChange {
    status: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    reply: Option<String>,
}

async fn set_status(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<String>, Json(req): Json<StatusChange>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    // A game log (`auto`) stays one: a note or reply is saved with that status.
    if !["open", "resolved", "auto"].contains(&req.status.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "a report is open or resolved");
    }
    let note = req.note.as_deref().map(str::trim);
    if note.is_some_and(|n| n.chars().count() > reports::MAX_NOTE || n.chars().any(|ch| (ch.is_control() && ch != '\n') || crate::hidden_char(ch))) {
        return fail(StatusCode::BAD_REQUEST, "a note is up to 500 printable characters");
    }
    let reply = req.reply.as_deref().map(str::trim);
    if reply.is_some_and(|r| r.chars().count() > reports::MAX_REPLY || r.chars().any(|ch| (ch.is_control() && ch != '\n') || crate::hidden_char(ch))) {
        return fail(StatusCode::BAD_REQUEST, "a reply is up to 1000 printable characters");
    }
    match c.set_report_status(&id, &req.status, note, reply, &s.username).await {
        Ok(Some((before, row))) => {
            let who = row["player"]["name"].as_str().unwrap_or_default();
            let server = row["server"].as_str().unwrap_or_default();
            let what = match (before == req.status, req.status.as_str()) {
                (true, _) => "noted",
                (false, "resolved") => "resolved",
                (false, _) => "reopened",
            };
            let note = note.filter(|n| !n.is_empty()).map(|n| format!(": {n}")).unwrap_or_default();
            let what = if reply.is_some_and(|r| !r.is_empty()) {
                format!("{what}, replied")
            } else {
                what.to_string()
            };
            c.audit(
                &s.username,
                Some(&client),
                &format!("report: {what}"),
                &format!("{} from {who} on {server}{note}", row["id"].as_str().unwrap_or_default()),
            )
            .await;
            c.publish(Event::Report);
            ok(row)
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such report"),
        Err(e) => internal(e),
    }
}

/// Deletes a report and the player's logs with it: wants a second factor proved lately.
async fn remove(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    match c.delete_report(&id).await {
        Ok(Some((who, server))) => {
            c.audit(
                &s.username,
                Some(&client),
                "report: deleted",
                &format!("{} from {who} on {server}", id.to_ascii_lowercase()),
            )
            .await;
            c.publish(Event::Report);
            ok(json!({}))
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such report"),
        Err(e) => internal(e),
    }
}
