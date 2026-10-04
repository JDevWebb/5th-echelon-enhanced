//! The admin UI's Sessions page (see [`crate::sessions`]).
//!
//! * `GET /api/sessions?server=&from=&to=`: each player's timeline in the range (the last day
//!   when left out, at most a week), the problems found in it, and the servers' alerts and
//!   unanswered pings.

use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Extension;
use axum::Router;

use super::internal;
use super::ok;
use super::Client;
use super::Shared;
use crate::sessions::SessionsQuery;

pub(super) fn routes() -> Router<Shared> {
    Router::new().route("/sessions", get(sessions))
}

async fn sessions(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<SessionsQuery>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.sessions_view(&q).await.map_or_else(internal, ok)
}
