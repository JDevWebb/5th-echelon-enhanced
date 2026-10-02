//! The admin UI's live connection: a WebSocket (`GET /api/live`) that keeps the
//! dashboards current without reloading anything.
//!
//! Signing in works as for the rest of the API (the session cookie, and every
//! request passes the network and country rules first), plus the browser's
//! `Origin` must be the admin UI's own: a WebSocket isn't bound by the
//! same-origin rule, so another site could otherwise open one with the
//! admin's cookie.
//!
//! What the coordinator sends, as JSON text:
//! * `{"type":"overview","overview":{…},"metrics":bool}`: the network at a
//!   glance (`GET /api/overview`), at once and again within a couple of
//!   seconds of anything changing; `metrics` when a server reported new
//!   numbers, for charts to fetch their latest points.
//! * `{"type":"audit","event":{…}}`: a new audit log entry, as `GET /api/audit`
//!   lists them.
//! * `{"type":"bye","reason":"…"}`: the session ended; the socket closes.
//!
//! Nothing the browser sends is acted on.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::Extension;
use serde_json::json;
use serde_json::Value;

use super::cfg;
use super::fail;
use super::Client;
use crate::Coordinator;

/// Something the live connections should hear about.
#[derive(Debug, Clone)]
pub enum Event {
    /// A server reported its metrics.
    Metrics,
    /// A server's listing or presence changed (its heartbeat).
    Network,
    /// A new audit log entry (every admin action writes one).
    Audit(Value),
}

impl Coordinator {
    /// Tells the live connections (none listening is fine).
    pub(crate) fn publish(&self, event: Event) {
        let _ = self.live.send(event);
    }
}

/// How often a changed overview is sent at most, how often the session is
/// checked again, and how often the connection is pinged (proxies close idle ones).
const COALESCE: Duration = Duration::from_secs(2);
const RECHECK: Duration = Duration::from_secs(30);
const PING: Duration = Duration::from_secs(25);
/// The largest message taken from the browser (it has nothing to say).
const MAX_INCOMING: usize = 4 * 1024;

pub(super) async fn live(
    State(c): State<Arc<Coordinator>>,
    Extension(client): Extension<Client>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    // Who's asking first, whatever the request looks like.
    let origin = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok());
    if origin != Some(cfg(&c).origin.as_str()) {
        return fail(StatusCode::FORBIDDEN, "live updates are for the admin UI's own pages");
    }
    if let Err(r) = c.full(&headers, &client).await {
        return r;
    }
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    ws.max_message_size(MAX_INCOMING)
        .max_frame_size(MAX_INCOMING)
        .on_upgrade(move |socket| serve(c, socket, headers, client))
}

async fn serve(c: Arc<Coordinator>, mut socket: WebSocket, headers: HeaderMap, client: Client) {
    let mut events = c.live.subscribe();
    let mut tick = tokio::time::interval(COALESCE);
    let mut recheck = tokio::time::interval(RECHECK);
    let mut ping = tokio::time::interval(PING);
    // Sent at once: everything is "changed" at the start.
    let (mut dirty, mut metrics) = (true, true);
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(Event::Metrics) => (dirty, metrics) = (true, true),
                Ok(Event::Network) => dirty = true,
                Ok(Event::Audit(entry)) => {
                    dirty = true;
                    if !send(&mut socket, json!({ "type": "audit", "event": entry })).await {
                        return;
                    }
                }
                // Missed some: the next overview covers them.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => (dirty, metrics) = (true, true),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            },
            _ = tick.tick() => {
                if dirty {
                    let overview = match c.admin_overview().await {
                        Ok(o) => o,
                        Err(e) => {
                            tracing::error!("admin: live overview: {e}");
                            continue;
                        }
                    };
                    if !send(&mut socket, json!({ "type": "overview", "overview": overview, "metrics": metrics })).await {
                        return;
                    }
                    (dirty, metrics) = (false, false);
                }
            }
            _ = recheck.tick() => {
                // Signed out, expired, disabled, or moved country: the socket ends with it.
                let still = matches!(c.session(&headers, &client).await, Ok(Some(s)) if s.stage == "full");
                let allowed = c.restrictions().await.map(|r| r.allows(client.ip, &client.country)).unwrap_or(false);
                if !still || !allowed {
                    let _ = send(&mut socket, json!({ "type": "bye", "reason": if still { "not allowed from here" } else { "signed out" } })).await;
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

async fn send(socket: &mut WebSocket, v: Value) -> bool {
    socket.send(Message::Text(v.to_string().into())).await.is_ok()
}
