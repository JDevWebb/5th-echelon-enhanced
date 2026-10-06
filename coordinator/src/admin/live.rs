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
//! * `{"type":"pulses","pulses":{server:[point…]},"feed":[…]}` at once: each
//!   server's live numbers of the last half hour, and the recent events.
//! * `{"type":"pulse","server":id,"point":{…}}`: a server's live numbers, every
//!   ten seconds (players, in a match, matches, lobbies, bytes a second in, out
//!   and relayed).
//! * `{"type":"event","event":{…}}`: something that happened, made from the
//!   pulses: players signing in, new accounts, failed sign-ins, matches starting
//!   and ending. Counts only.
//! * `{"type":"alert","alert":{…}}`: an alert raised or resolved (see `alerts`).
//! * `{"type":"report"}`: a player's report came, or an admin changed one (see `reports`);
//!   the overview that follows has the open reports' count.
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
    /// A server's live numbers: (server, point).
    Pulse(String, Value),
    /// Something that happened, made from the pulses.
    Feed(Value),
    /// An alert raised or resolved.
    Alert(Value),
    /// A player's report came, or an admin changed one.
    Report,
    /// A maintenance window was booked or cancelled.
    Maintenance,
    /// The roadmap or a suggestion changed (or a player sent one).
    Roadmap,
}

/// The live points kept per server: half an hour of them.
const KEEP_POINTS: usize = 180;
/// The recent events kept.
const KEEP_FEED: usize = 50;

/// A server's last pulse, and its points of the last half hour.
#[derive(Debug, Default)]
pub struct Pulses {
    pub(crate) last: Option<(i64, Value)>,
    pub(crate) points: std::collections::VecDeque<Value>,
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// How much a counter grew since `before`. When it went back (the server restarted), all
/// of it is new.
fn grew(now: &Value, before: &Value) -> Option<f64> {
    let (a, b) = (num(now), num(before));
    Some(if a >= b { a - b } else { a })
}

fn plural(n: f64, one: &str, many: &str) -> String {
    format!("{n} {}", if (n - 1.0).abs() < f64::EPSILON { one } else { many })
}

/// Rooms kept from one pulse at most.
const MAX_GAMES: usize = 100;

/// A pulse's list of the rooms open now, checked as [`online_list`] is: at most
/// [`MAX_GAMES`], each with only the fields the Live page shows.
fn games_list(v: &Value) -> Value {
    let text = |v: &Value| -> Value { json!(v.as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(MAX_FIELD).collect::<String>()) };
    let id = |v: &Value| v.as_u64().filter(|n| *n <= u64::from(u32::MAX));
    let games = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|g| g.is_object())
        .take(MAX_GAMES)
        .map(|g| {
            json!({
                "id": id(&g["id"]).unwrap_or(0),
                "host": text(&g["host"]),
                "players": g["players"].as_array().into_iter().flatten().take(16).map(text).collect::<Vec<_>>(),
                "room_kind": text(&g["room_kind"]),
                "mode": text(&g["mode"]),
                "private": g["private"].as_bool().unwrap_or(false),
                "map": id(&g["map"]),
                "game_mode": id(&g["game_mode"]),
                "since": g["since"].as_i64(),
            })
        })
        .collect();
    Value::Array(games)
}

/// Online players kept from one pulse at most, and the longest text kept of each field.
const MAX_ONLINE: usize = 300;
const MAX_FIELD: usize = 64;

/// A pulse's list of who's online, checked: at most [`MAX_ONLINE`] players, each with only
/// the fields the map shows, texts cut to [`MAX_FIELD`] characters, numbers in range.
fn online_list(v: &Value) -> Value {
    let text = |v: &Value| -> Value { json!(v.as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(MAX_FIELD).collect::<String>()) };
    let players = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p.is_object())
        .take(MAX_ONLINE)
        .map(|p| {
            let coord = |v: &Value, max: f64| json!(v.as_f64().filter(|x| x.is_finite() && x.abs() <= max).unwrap_or(0.0));
            json!({
                "id": p["id"].as_u64().unwrap_or(0),
                "name": text(&p["name"]),
                "country": text(&p["country"]),
                "country_name": text(&p["country_name"]),
                "region": text(&p["region"]),
                "city": text(&p["city"]),
                "lat": coord(&p["lat"], 90.0),
                "lon": coord(&p["lon"], 180.0),
                "status": text(&p["status"]),
                "mode": text(&p["mode"]),
                "with": p["with"].as_array().into_iter().flatten().take(16).map(text).collect::<Vec<_>>(),
                "since": p["since"].as_i64(),
                "network": text(&p["network"]),
                "ping_ms": p["ping_ms"].as_u64().filter(|ms| *ms < 10_000),
            })
        })
        .collect::<Vec<_>>();
    Value::Array(players)
}

/// The live point a pulse makes, given the one before it (for rates), and what
/// happened in between.
fn point_from(at: i64, p: &Value, last: Option<&(i64, Value)>) -> (Value, Vec<(&'static str, &'static str, String)>) {
    let mut point = json!({
        "t": at,
        "players": num(&p["players"]["online"]),
        "in_match": num(&p["players"]["in_match"]),
        "matches": num(&p["matches"]),
        "lobbies": num(&p["lobbies"]),
        "rx": 0.0, "tx": 0.0, "relayed": 0.0,
    });
    let mut events = Vec::new();
    if let Some((t0, before)) = last.filter(|(t0, _)| (1..=60).contains(&(at - *t0))) {
        let secs = (at - t0) as f64;
        let (c, c0) = (&p["counters"], &before["counters"]);
        for (key, field) in [("rx", "net_rx_bytes"), ("tx", "net_tx_bytes")] {
            point[key] = json!(grew(&p[field], &before[field]).unwrap_or(0.0) / secs);
        }
        point["relayed"] = json!(grew(&c["relayed_bytes"], &c0["relayed_bytes"]).unwrap_or(0.0) / secs);
        if let Some(n) = grew(&c["game_logins"], &c0["game_logins"]).filter(|n| *n > 0.0) {
            events.push(("signin", "info", format!("{} signed in", plural(n, "player", "players"))));
        }
        if let Some(n) = grew(&c["registrations"], &c0["registrations"]).filter(|n| *n > 0.0) {
            events.push(("account", "info", format!("{} made", plural(n, "new account", "new accounts"))));
        }
        if let Some(n) = grew(&c["failed_logins"], &c0["failed_logins"]).filter(|n| *n > 0.0) {
            events.push(("failed", "warn", format!("{} refused", plural(n, "sign-in", "sign-ins"))));
        }
        let (m, m0) = (num(&p["matches"]), num(&before["matches"]));
        if m > m0 {
            events.push(("match", "info", format!("{} started", plural(m - m0, "match", "matches"))));
        } else if m < m0 {
            events.push(("match", "muted", format!("{} ended", plural(m0 - m, "match", "matches"))));
        }
    }
    (point, events)
}

impl Coordinator {
    /// Takes a server's pulse: its live point, and events from what changed since the last.
    /// Both are stored too (a day), so a restart of the coordinator keeps the live view.
    pub(crate) async fn record_pulse(&self, server: &str, p: &Value) -> sqlx::Result<()> {
        let at = identity::now();
        let (point, events) = {
            let mut pulses = self.pulses.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = pulses.entry(server.to_string()).or_default();
            let (point, events) = point_from(at, p, entry.last.as_ref());
            // The raw pulse isn't kept: only its numbers and a checked, capped list of who's
            // online (a server could send anything up to the request's limit).
            let mut kept = p.clone();
            kept["online"] = online_list(&p["online"]);
            kept["games"] = games_list(&p["games"]);
            entry.last = Some((at, kept));
            entry.points.push_back(point.clone());
            while entry.points.len() > KEEP_POINTS {
                entry.points.pop_front();
            }
            (point, events)
        };
        let events: Vec<Value> = events
            .into_iter()
            .map(|(kind, level, text)| json!({ "t": at, "server": server, "kind": kind, "level": level, "text": text }))
            .collect();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("INSERT OR REPLACE INTO pulses (server_id, at, point) VALUES (?, ?, ?)")
            .bind(server)
            .bind(at)
            .bind(point.to_string())
            .execute(&mut *tx)
            .await?;
        for event in &events {
            sqlx::query("INSERT INTO live_feed (at, event) VALUES (?, ?)")
                .bind(at)
                .bind(event.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.publish(Event::Pulse(server.to_string(), point));
        for event in events {
            {
                let mut feed = self.feed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                feed.push_front(event.clone());
                feed.truncate(KEEP_FEED);
            }
            self.publish(Event::Feed(event));
        }
        Ok(())
    }

    /// Who's online now on each server that pulsed in the last half minute, as its last
    /// pulse listed them (kept in memory only, never stored): `[{server, id, name, city, ...}]`.
    pub(crate) fn online_now(&self) -> Value {
        let now = identity::now();
        let pulses = self.pulses.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = Vec::new();
        for (server, entry) in pulses.iter() {
            let Some((at, p)) = entry.last.as_ref().filter(|(at, _)| now - at <= 30) else {
                continue;
            };
            for player in p["online"].as_array().into_iter().flatten() {
                let mut player = player.clone();
                player["server"] = json!(server);
                player["seen"] = json!(at);
                out.push(player);
            }
        }
        Value::Array(out)
    }

    /// The rooms open now on each server that pulsed in the last half minute, as its last
    /// pulse listed them (in memory only): `[{server, id, host, players, map, ...}]`.
    pub(crate) fn games_now(&self) -> Value {
        let now = identity::now();
        let pulses = self.pulses.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = Vec::new();
        for (server, entry) in pulses.iter() {
            let Some((_, p)) = entry.last.as_ref().filter(|(at, _)| now - at <= 30) else {
                continue;
            };
            for game in p["games"].as_array().into_iter().flatten() {
                let mut game = game.clone();
                game["server"] = json!(server);
                out.push(game);
            }
        }
        Value::Array(out)
    }

    /// Loads the stored live points and events of the last half hour (at start).
    pub(crate) async fn load_live(&self) -> sqlx::Result<()> {
        let since = identity::now() - 1800;
        let points: Vec<(String, String)> = sqlx::query_as("SELECT server_id, point FROM pulses WHERE at >= ? ORDER BY at")
            .bind(since)
            .fetch_all(&self.pool)
            .await?;
        let events: Vec<(String,)> = sqlx::query_as("SELECT event FROM live_feed WHERE at >= ? ORDER BY id DESC LIMIT ?")
            .bind(since)
            .bind(KEEP_FEED as i64)
            .fetch_all(&self.pool)
            .await?;
        {
            let mut pulses = self.pulses.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (server, point) in points {
                let Ok(point) = serde_json::from_str::<Value>(&point) else { continue };
                let entry = pulses.entry(server).or_default();
                entry.points.push_back(point);
                while entry.points.len() > KEEP_POINTS {
                    entry.points.pop_front();
                }
            }
        }
        let mut feed = self.feed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        feed.extend(events.into_iter().filter_map(|(e,)| serde_json::from_str(&e).ok()));
        Ok(())
    }

    /// The live points of the last half hour per server, and the recent events.
    fn live_history(&self) -> Value {
        let since = identity::now() - 1800;
        let pulses = self.pulses.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let points: serde_json::Map<String, Value> = pulses
            .iter()
            .map(|(s, p)| {
                (
                    s.clone(),
                    Value::from(p.points.iter().filter(|v| num(&v["t"]) >= since as f64).cloned().collect::<Vec<_>>()),
                )
            })
            .collect();
        drop(pulses);
        let feed: Vec<Value> = self.feed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().cloned().collect();
        json!({ "type": "pulses", "pulses": points, "feed": feed })
    }
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
    // Sent at once: everything is "changed" at the start, and the live history.
    let (mut dirty, mut metrics) = (true, true);
    if !send(&mut socket, c.live_history()).await {
        return;
    }
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
                Ok(Event::Pulse(server, point)) => {
                    if !send(&mut socket, json!({ "type": "pulse", "server": server, "point": point })).await {
                        return;
                    }
                }
                Ok(Event::Feed(event)) => {
                    if !send(&mut socket, json!({ "type": "event", "event": event })).await {
                        return;
                    }
                }
                Ok(Event::Alert(alert)) => {
                    dirty = true;
                    if !send(&mut socket, json!({ "type": "alert", "alert": alert })).await {
                        return;
                    }
                }
                Ok(Event::Report) => {
                    dirty = true;
                    if !send(&mut socket, json!({ "type": "report" })).await {
                        return;
                    }
                }
                Ok(Event::Maintenance) => {
                    if !send(&mut socket, json!({ "type": "maintenance" })).await {
                        return;
                    }
                }
                Ok(Event::Roadmap) => {
                    if !send(&mut socket, json!({ "type": "roadmap" })).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_games_are_checked_and_capped() {
        let one = json!({ "id": 102, "host": "Oni", "players": ["Oni", "tacit_danger", "Ghost_Leader"], "room_kind": "match", "mode": "coop",
            "private": false, "map": 2573003522_u32, "game_mode": 4, "since": 1791305206, "extra": "dropped" });
        let mut many = vec![one.clone(); MAX_GAMES + 5];
        many.push(json!("not a game"));
        let kept = games_list(&json!(many));
        assert_eq!(kept.as_array().unwrap().len(), MAX_GAMES);
        let g = &kept[0];
        assert_eq!((g["host"].as_str(), g["map"].as_u64(), g["game_mode"].as_u64()), (Some("Oni"), Some(2573003522), Some(4)));
        assert_eq!(g["players"], json!(["Oni", "tacit_danger", "Ghost_Leader"]));
        assert!(g.get("extra").is_none());
        let odd = games_list(&json!([{ "id": -1, "host": "x\u{7}y", "map": 1u64 << 40, "players": "nope" }]));
        assert_eq!((odd[0]["id"].as_u64(), odd[0]["host"].as_str(), odd[0]["map"].as_u64()), (Some(0), Some("xy"), None));
        assert_eq!(odd[0]["players"], json!([]));
    }

    #[test]
    fn pulses_make_rates_and_events() {
        let p0 =
            json!({ "players": { "online": 2 }, "matches": 1, "net_rx_bytes": 1000, "net_tx_bytes": 0, "counters": { "game_logins": 5, "failed_logins": 0, "relayed_bytes": 0 } });
        let (first, events) = point_from(100, &p0, None);
        assert!(events.is_empty() && first["rx"] == 0.0, "nothing to compare the first with");
        let p1 = json!({ "players": { "online": 3 }, "matches": 2, "net_rx_bytes": 3000, "net_tx_bytes": 500, "counters": { "game_logins": 6, "failed_logins": 3, "relayed_bytes": 100 } });
        let (point, events) = point_from(110, &p1, Some(&(100, p0.clone())));
        assert_eq!(
            (point["rx"].as_f64(), point["tx"].as_f64(), point["relayed"].as_f64()),
            (Some(200.0), Some(50.0), Some(10.0))
        );
        let texts: Vec<&str> = events.iter().map(|e| e.2.as_str()).collect();
        assert_eq!(texts, ["1 player signed in", "3 sign-ins refused", "1 match started"]);
        // A restart (counters back to zero) makes no events, and no rates from a stale pulse.
        let restarted = json!({ "players": { "online": 0 }, "matches": 2, "net_rx_bytes": 10, "counters": { "game_logins": 0 } });
        assert!(point_from(120, &restarted, Some(&(110, p1.clone()))).1.is_empty());
        assert_eq!(point_from(500, &p1, Some(&(100, p0))).0["rx"], 0.0);
    }
}
