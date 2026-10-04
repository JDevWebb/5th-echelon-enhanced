//! The admin UI's leaderboards: the global ones (see [`crate::stats`]), and removing a
//! person's stats (a cheater's, say).
//!
//! * `GET /api/leaderboards?leaderboard=&context=&count=`: every leaderboard and the lists
//!   that aren't empty, and with a leaderboard and context, its top `count` (up to 100).
//! * `DELETE /api/stats/<global id>`: removes everything kept for that person. Wants a second
//!   factor proved lately; audited.

use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::delete;
use axum::routing::get;
use axum::Extension;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use super::fail;
use super::internal;
use super::ok;
use super::Client;
use super::Shared;
use crate::stats;

pub(super) fn routes() -> Router<Shared> {
    Router::new().route("/leaderboards", get(leaderboards)).route("/stats/{global_id}", delete(remove))
}

#[derive(Deserialize)]
struct Pick {
    leaderboard: Option<u32>,
    context: Option<u32>,
    count: Option<usize>,
}

async fn leaderboards(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<Pick>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let pick = match (q.leaderboard, q.context) {
        (Some(id), Some(context)) => match stat_boards::leaderboard(id) {
            Some(l) if stat_boards::board(l.board).is_some_and(|b| b.has_context(context)) => Some((l, context)),
            _ => return fail(StatusCode::BAD_REQUEST, "no such leaderboard and context"),
        },
        (None, None) => None,
        _ => return fail(StatusCode::BAD_REQUEST, "a leaderboard and a context, or neither"),
    };
    let count = q.count.unwrap_or(stats::MAX_COUNT);
    if !(1..=stats::MAX_COUNT).contains(&count) {
        return fail(StatusCode::BAD_REQUEST, "count is 1 to 100");
    }
    c.admin_leaderboard(pick, count).await.map_or_else(internal, ok)
}

async fn remove(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(global_id): Path<String>) -> Response {
    let s = match c.recent(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !stats::valid_global_id(&global_id) {
        return fail(StatusCode::BAD_REQUEST, "not a global id");
    }
    match c.remove_stats(&global_id).await {
        Ok((0, None)) => fail(StatusCode::NOT_FOUND, "no stats kept for them"),
        Ok((removed, name)) => {
            let who = name.map_or_else(|| global_id.clone(), |n| format!("{n} ({global_id})"));
            c.audit(&s.username, Some(&client), "stats: removed", &format!("{who}: {removed} stats")).await;
            ok(json!({ "removed": removed }))
        }
        Err(e) => internal(e),
    }
}
