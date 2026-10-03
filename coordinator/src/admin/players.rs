//! The admin UI's players: the list across servers, one player's detail, the actions an
//! admin can have a player's server carry out, and the matches report.
//!
//! * `GET /api/players?q=&server=&online=&banned=&sort=&page=`: a page of players.
//! * `GET /api/players/<server>/<id>`: one player, with sessions, play by day, other
//!   accounts and action history.
//! * `POST /api/players/<server>/<id>/actions` `{kind, reason?, until?, name?, all_servers?}`:
//!   queues an action (for every account sharing the identity, with `all_servers`). Bans,
//!   deletes, renames and password resets want a second factor proved lately.
//! * `GET /api/actions/<id>`: how an action went; a reset password once, to who asked.
//! * `GET /api/matches-report?days=`: matches per day by mode, lengths, players, maps.

use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
use axum::Extension;
use axum::Json;
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

use super::fail;
use super::internal;
use super::ok;
use super::Client;
use super::Shared;
use crate::players;

pub(super) fn routes() -> Router<Shared> {
    Router::new()
        .route("/players", get(list))
        .route("/players/{server}/{id}", get(detail))
        .route("/players/{server}/{id}/actions", post(act))
        .route("/actions/{id}", get(action))
        .route("/matches-report", get(matches_report))
}

async fn list(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<players::ListQuery>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.player_list(&q).await.map_or_else(internal, ok)
}

async fn detail(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path((server, id)): Path<(String, i64)>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    match c.player_detail(&server, id).await {
        Ok(Some(v)) => ok(v),
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such player"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct ActionRequest {
    kind: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    until: Option<i64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    all_servers: bool,
}

/// The longest ban that isn't for good.
const LONGEST_BAN: i64 = 10 * 365 * 86_400;

async fn act(
    State(c): State<Shared>,
    Extension(client): Extension<Client>,
    headers: HeaderMap,
    Path((server, id)): Path<(String, i64)>,
    Json(req): Json<ActionRequest>,
) -> Response {
    let kind = req.kind.as_str();
    if !players::KINDS.contains(&kind) {
        return fail(StatusCode::BAD_REQUEST, "not an action: ban, unban, kick, reset_password, rename or delete");
    }
    let s = match if players::sensitive(kind) {
        c.recent(&headers, &client).await
    } else {
        c.full(&headers, &client).await
    } {
        Ok(s) => s,
        Err(r) => return r,
    };
    let reason = req.reason.trim();
    if reason.chars().count() > 200 || reason.chars().any(|ch| ch.is_control() || crate::hidden_char(ch)) {
        return fail(StatusCode::BAD_REQUEST, "a reason is up to 200 printable characters");
    }
    let now = identity::now();
    let until = match (kind, req.until) {
        ("ban", Some(t)) if t > now && t - now <= LONGEST_BAN => Some(t),
        ("ban", Some(_)) => return fail(StatusCode::BAD_REQUEST, "a ban ends in the future, within ten years (or never)"),
        _ => None,
    };
    let name = match (kind, req.name.as_deref().map(str::trim)) {
        ("rename", Some(n)) if players::valid_new_name(n) => Some(n.to_string()),
        ("rename", _) => return fail(StatusCode::BAD_REQUEST, "a name is 3 to 24 letters, digits, _ - and ."),
        _ => None,
    };
    let Ok(Some(player_name)) = c.player_name(&server, id).await else {
        return fail(StatusCode::NOT_FOUND, "no such player");
    };
    let accounts = if req.all_servers {
        match c.accounts_of(&server, id).await {
            Ok(a) => a,
            Err(e) => return internal(e),
        }
    } else {
        vec![(server.clone(), id)]
    };
    if let Some(name) = &name {
        for (srv, player) in &accounts {
            match c.rename_refused(srv, *player, name).await {
                Ok(None) => {}
                Ok(Some(why)) => return fail(StatusCode::CONFLICT, why),
                Err(e) => return internal(e),
            }
        }
    }
    let args = json!({ "reason": reason, "until": until, "name": name });
    let mut queued = Vec::new();
    for (srv, player) in &accounts {
        match c.queue_action(srv, *player, kind, &args, &s.username).await {
            Ok(action) => queued.push(json!({ "id": action, "server": srv, "player": player, "kind": kind, "status": "pending" })),
            Err(e) => return internal(e),
        }
    }
    let detail = format!(
        "{player_name} (#{id} on {server}){}{}{}{}",
        if accounts.len() > 1 {
            format!(" and {} more accounts", accounts.len() - 1)
        } else {
            String::new()
        },
        name.as_deref().map(|n| format!(" to {n}")).unwrap_or_default(),
        if kind == "ban" {
            until.map_or(" for good".to_string(), |u| format!(" for {} days", ((u - now) as f64 / 86_400.0).round()))
        } else {
            String::new()
        },
        if reason.is_empty() { String::new() } else { format!(": {reason}") },
    );
    c.audit(&s.username, Some(&client), &format!("player: {kind}"), &detail).await;
    ok(json!({ "actions": queued }))
}

async fn action(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    match c.read_action(id, &s.username).await {
        Ok(Some(v)) => {
            if v["password"].is_string() {
                let detail = format!("#{} on {}", v["player"], v["server"].as_str().unwrap_or_default());
                c.audit(&s.username, Some(&client), "player: saw a reset password", &detail).await;
            }
            ok(v)
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such action"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct Days {
    #[serde(default)]
    days: i64,
}

async fn matches_report(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<Days>) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    if !(1..=400).contains(&q.days) {
        return fail(StatusCode::BAD_REQUEST, "days is 1 to 400");
    }
    c.matches_report(q.days).await.map_or_else(internal, |v: Value| ok(v))
}
