//! The admin UI's Inbox: everything waiting on an admin, in one list.
//!
//! * `GET /api/inbox`: support conversations whose turn is the admins' (`open`), player
//!   reports not resolved, chat messages reported and not dealt with, crashes of the last week
//!   not marked (or marked fixed and seen again since), and the open alerts; newest first,
//!   each with what the list shows and the key its own page opens it by. The items are dealt
//!   with where they always were (Support, Reports, Moderation, Crashes, Alerts): the Inbox
//!   only gathers them.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Extension;
use axum::Router;
use serde_json::json;
use serde_json::Value;

use super::internal;
use super::ok;
use super::Client;
use super::Shared;
use crate::Coordinator;

/// The most of each kind listed.
const MOST: i64 = 200;

pub(super) fn routes() -> Router<Shared> {
    Router::new().route("/inbox", get(list))
}

async fn list(State(c): State<Shared>, Extension(client): Extension<Client>, headers: HeaderMap) -> Response {
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    c.inbox().await.map_or_else(internal, ok)
}

impl Coordinator {
    pub(crate) async fn inbox(&self) -> sqlx::Result<Value> {
        let mut items: Vec<Value> = Vec::new();

        // Support: the player wrote last, or an admin reopened it.
        let support = self.support_list("open").await?;
        for t in support["threads"].as_array().into_iter().flatten() {
            items.push(json!({
                "kind": "support", "key": t["identity"], "who": t["name"], "title": t["last"],
                "server": t["server"], "at": t["updated_at"], "status": "open", "unread": t["unread"],
            }));
        }

        // Player reports not resolved (game logs sent on their own are `auto`, not here).
        let rows: Vec<(String, String, i64, String, String, Option<String>, String)> =
            sqlx::query_as("SELECT id, server_id, created_at, player_name, comment, rating, problems FROM player_reports WHERE status = 'open' ORDER BY created_at DESC LIMIT ?")
                .bind(MOST)
                .fetch_all(&self.pool)
                .await?;
        for (id, server, at, name, comment, rating, problems) in rows {
            let problems: Vec<String> = serde_json::from_str(&problems).unwrap_or_default();
            items.push(json!({
                "kind": "report", "key": id, "who": name, "title": comment.chars().take(140).collect::<String>(),
                "server": server, "at": at, "status": "open", "rating": rating, "problems": problems,
            }));
        }

        // Chat messages reported.
        for r in self.chat_report_list(identity::now()).await? {
            let m = &r["message"];
            let first = r["reports"].as_array().and_then(|l| l.iter().filter_map(|x| x["at"].as_i64()).min());
            items.push(json!({
                "kind": "chat", "key": m["id"], "who": m["from_name"], "title": m["body"],
                "server": m["server"], "at": first.or_else(|| m["at"].as_i64()), "status": "open",
                "room": m["room"], "reports": r["reports"].as_array().map_or(0, Vec::len),
            }));
        }

        // Crashes of the last week to look at.
        let crashes = self.crashes_view(7).await?;
        for g in crashes["groups"].as_array().into_iter().flatten() {
            let back = g["seen_since_fix"].as_array().is_some_and(|a| !a.is_empty());
            if !g["mark"].is_null() && !back {
                continue;
            }
            items.push(json!({
                "kind": "crash", "key": g["signature"], "who": g["signature"],
                "title": format!("{} players, {} times, {} ended the game", g["players"], g["count"], g["fatal"]),
                "server": "", "at": g["last"], "status": if back { "back" } else { "new" },
                "players": g["players"],
            }));
        }

        // Alerts open now.
        for a in self.open_alerts().await? {
            items.push(json!({
                "kind": "alert", "key": a["id"], "who": a["kind"], "title": a["detail"],
                "server": a["server"], "at": a["started_at"], "status": a["level"],
            }));
        }

        items.sort_by_key(|i| std::cmp::Reverse(i["at"].as_i64().unwrap_or(0)));
        let count = |k: &str| items.iter().filter(|i| i["kind"] == k).count();
        let counts = json!({
            "all": items.len(), "support": count("support"), "report": count("report"),
            "chat": count("chat"), "crash": count("crash"), "alert": count("alert"),
        });
        Ok(json!({ "items": items, "counts": counts }))
    }
}
