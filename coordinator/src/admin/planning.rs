//! The admin UI's maintenance windows (see [`crate::maintenance`]) and roadmap (see
//! [`crate::roadmap`]):
//!
//! * `GET /api/maintenance`; `POST /api/maintenance` `{servers, network, start, end, note}`
//!   books windows; `DELETE /api/maintenance/<id>` cancels one.
//! * `GET /api/roadmap`: lanes, every item, and how many suggestions are new.
//! * `POST /api/roadmap/items`, `PUT|DELETE /api/roadmap/items/<id>`: items.
//! * `PUT /api/roadmap/lanes/<lane>` `{release}`: the release a lane is for.
//! * `GET /api/suggestions?status=new|all`; `PUT /api/suggestions/<id>` `{status, reply}`;
//!   `POST /api/suggestions/<id>/promote` `{lane}`: players' suggestions.
//!
//! Every change is audited, and the admin UI's live connections hear of it. The roadmap's
//! routes answer only on a coordinator started with `--roadmap` (the community network's).

use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
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
use crate::admin::live::Event;
use crate::maintenance;
use crate::roadmap;

pub(super) fn routes() -> Router<Shared> {
    Router::new()
        .route("/maintenance", get(maintenance_list).post(book))
        .route("/maintenance/{id}", axum::routing::delete(cancel))
        .route("/roadmap", get(roadmap_get))
        .route("/roadmap/items", post(add_item))
        .route("/roadmap/items/{id}", put(edit_item).delete(remove_item))
        .route("/roadmap/lanes/{lane}", put(set_lane))
        .route("/suggestions", get(suggestions))
        .route("/suggestions/{id}", put(answer))
        .route("/suggestions/{id}/promote", post(promote))
}

async fn maintenance_list(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.maintenance_list(identity::now()).await.map_or_else(internal, ok)
}

async fn book(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(b): Json<maintenance::Booking>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let now = identity::now();
    if let Err(why) = b.check(now) {
        return fail(StatusCode::BAD_REQUEST, why);
    }
    match c.book_maintenance(&b, &s.username, now).await {
        Ok(Some(ids)) => {
            let mut whom: Vec<String> = b.servers.clone();
            if b.network {
                whom.push(String::from("the whole network"));
            }
            let note = if b.note.trim().is_empty() { String::new() } else { format!(": {}", b.note.trim()) };
            let detail = format!("{} from {} for {} min{note}", whom.join(", "), maintenance::utc(b.start), (b.end - b.start) / 60);
            c.audit(&s.username, Some(&client), "maintenance: booked", &detail).await;
            c.publish(Event::Maintenance);
            match c.maintenance_list(now).await {
                Ok(list) => {
                    let created: Vec<_> = list["windows"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|w| w["id"].as_i64().is_some_and(|id| ids.contains(&id)))
                        .cloned()
                        .collect();
                    ok(json!({ "windows": created }))
                }
                Err(e) => internal(e),
            }
        }
        Ok(None) => fail(StatusCode::BAD_REQUEST, "a server isn't a member of this network"),
        Err(e) => internal(e),
    }
}

async fn cancel(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    match c.cancel_maintenance(id, identity::now()).await {
        Ok(true) => {
            c.audit(&s.username, Some(&client), "maintenance: cancelled", &format!("window {id}")).await;
            c.publish(Event::Maintenance);
            ok(json!({ "ok": true }))
        }
        Ok(false) => fail(StatusCode::NOT_FOUND, "no such window, or it's cancelled already"),
        Err(e) => internal(e),
    }
}

async fn roadmap_get(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if !c.has_roadmap() {
        return fail(StatusCode::NOT_FOUND, "no roadmap on this network");
    }
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.admin_roadmap().await.map_or_else(internal, ok)
}

async fn add_item(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Json(e): Json<roadmap::ItemEdit>) -> Response {
    if !c.has_roadmap() {
        return fail(StatusCode::NOT_FOUND, "no roadmap on this network");
    }
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(why) = e.check() {
        return fail(StatusCode::BAD_REQUEST, why);
    }
    match c.add_item(&e, &s.username, None, identity::now()).await {
        Ok(item) => {
            c.audit(&s.username, Some(&client), "roadmap: added", &format!("{} ({})", e.title.trim(), e.lane)).await;
            c.publish(Event::Roadmap);
            ok(item)
        }
        Err(e) => internal(e),
    }
}

async fn edit_item(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<i64>, Json(e): Json<roadmap::ItemEdit>) -> Response {
    if !c.has_roadmap() {
        return fail(StatusCode::NOT_FOUND, "no roadmap on this network");
    }
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(why) = e.check() {
        return fail(StatusCode::BAD_REQUEST, why);
    }
    match c.edit_item(id, &e, &s.username, identity::now()).await {
        Ok(Some(item)) => {
            let public = if e.public { "public" } else { "admins only" };
            c.audit(&s.username, Some(&client), "roadmap: changed", &format!("{} ({}, {public})", e.title.trim(), e.lane))
                .await;
            c.publish(Event::Roadmap);
            ok(item)
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such item"),
        Err(e) => internal(e),
    }
}

async fn remove_item(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<i64>) -> Response {
    if !c.has_roadmap() {
        return fail(StatusCode::NOT_FOUND, "no roadmap on this network");
    }
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    match c.remove_item(id).await {
        Ok(Some(title)) => {
            c.audit(&s.username, Some(&client), "roadmap: removed", &title).await;
            c.publish(Event::Roadmap);
            ok(json!({ "ok": true }))
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such item"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct LaneRelease {
    #[serde(default)]
    release: String,
}

async fn set_lane(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(lane): Path<String>, Json(r): Json<LaneRelease>) -> Response {
    if !c.has_roadmap() {
        return fail(StatusCode::NOT_FOUND, "no roadmap on this network");
    }
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !roadmap::valid_lane(&lane) {
        return fail(StatusCode::NOT_FOUND, "no such lane");
    }
    if !crate::valid_text(&r.release, 24) {
        return fail(StatusCode::BAD_REQUEST, "a release is up to 24 printable characters");
    }
    match c.set_lane_release(&lane, &r.release).await {
        Ok(()) => {
            c.audit(&s.username, Some(&client), "roadmap: lane", &format!("{lane}: {}", r.release.trim())).await;
            c.publish(Event::Roadmap);
            ok(json!({ "ok": true }))
        }
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct SuggestionQuery {
    #[serde(default)]
    status: String,
}

async fn suggestions(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Query(q): Query<SuggestionQuery>) -> Response {
    if !c.has_roadmap() {
        return fail(StatusCode::NOT_FOUND, "no roadmap on this network");
    }
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.suggestion_list(q.status == "all").await.map_or_else(internal, ok)
}

#[derive(Deserialize)]
struct Answer {
    status: String,
    #[serde(default)]
    reply: String,
}

async fn answer(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<i64>, Json(a): Json<Answer>) -> Response {
    if !c.has_roadmap() {
        return fail(StatusCode::NOT_FOUND, "no roadmap on this network");
    }
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !roadmap::valid_answer(&a.status, &a.reply) {
        return fail(StatusCode::BAD_REQUEST, "a status of new, planned, done or declined, and a reply of up to 300 characters");
    }
    match c.answer_suggestion(id, &a.status, &a.reply, identity::now()).await {
        Ok(Some(v)) => {
            let title = v["title"].as_str().unwrap_or_default();
            c.audit(&s.username, Some(&client), "suggestion: answered", &format!("{title}: {}", a.status)).await;
            c.publish(Event::Roadmap);
            ok(v)
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such suggestion"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct Promote {
    lane: String,
}

async fn promote(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap, Path(id): Path<i64>, Json(p): Json<Promote>) -> Response {
    if !c.has_roadmap() {
        return fail(StatusCode::NOT_FOUND, "no roadmap on this network");
    }
    let s = match c.full(&headers, &client).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !matches!(p.lane.as_str(), "requested" | "next" | "later") {
        return fail(StatusCode::BAD_REQUEST, "promote to requested, next or later");
    }
    match c.promote_suggestion(id, &p.lane, &s.username, identity::now()).await {
        Ok(Some(item)) => {
            let title = item["title"].as_str().unwrap_or_default();
            c.audit(&s.username, Some(&client), "suggestion: promoted", &format!("{title} ({})", p.lane)).await;
            c.publish(Event::Roadmap);
            ok(item)
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "no such suggestion"),
        Err(e) => internal(e),
    }
}
