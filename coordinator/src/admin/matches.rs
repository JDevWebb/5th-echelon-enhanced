//! The admin UI's Matches page (see [`crate::matches`]).
//!
//! * `GET /api/matches?from=&to=&server=&mode=&map=&player=&recordings=&results=&before=`:
//!   matches newest first (a page; `next` is the `before` of the next), each with its players,
//!   how many recordings and results it has.
//! * `GET /api/match-results/<server>/<room>/<since>`: a match's results, each player's line and
//!   where they don't add up.

use axum::extract::Path;
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

pub(super) fn routes() -> Router<Shared> {
    Router::new().route("/matches", get(list)).route("/match-results/{server}/{room}/{since}", get(results))
}

async fn list(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<crate::matches::MatchesQuery>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.matches_view(&q).await.map_or_else(internal, ok)
}

async fn results(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path((server, room, since)): Path<(String, i64, i64)>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.match_results(&server, room, since).await.map_or_else(internal, ok)
}
