//! Players' session events from the member servers, for the admin UI's Sessions page: each
//! player's time on a server as a timeline (online, in a party, in a match and with whom),
//! and the problems found in it.
//!
//! * `POST /v1/events` (a member): `{events: [{id, at, last_at, player, name, kind, detail,
//!   count}]}`, up to [`MAX_EVENTS`] a request. The server sends each until it's taken; one
//!   it sends again (a repeat it counted since) replaces the one here.
//! * [`Coordinator::sessions_view`]: the timelines and problems of a time range, with the
//!   alerts of those servers and the times their API didn't answer the coordinator's pings.
//!
//! The server's `session_events.rs` says what each kind means.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// The most events one request may carry, and its largest body.
pub const MAX_EVENTS: usize = 500;
pub const MAX_BODY: usize = 1024 * 1024;
/// Events are kept this long.
pub(crate) const KEEP_FOR: i64 = 30 * 86_400;
/// Events one server may have here: past this, new ones are dropped.
const EVENTS_PER_SERVER: i64 = 2_000_000;
/// The longest range the page shows at once.
const LONGEST_RANGE: i64 = 7 * 86_400;
/// The kinds a server may send.
const KINDS: &[&str] = &[
    "server_start",
    "signin",
    "signin_refused",
    "signout",
    "room",
    "search",
    "join",
    "join_failed",
    "leave",
    "invite",
    "invite_delivered",
    "stats",
    "nat",
    "nat_missing",
    "nat_lost",
    "client_log",
    "relay_drop",
    "request_error",
    "report_refused",
];

/// How long after a relay drop a player leaving their match counts as dropping out, and
/// how close to that a stats write means the match had simply ended.
const DROPOUT_WITHIN: i64 = 180;
const STATS_NEAR: i64 = 180;
/// How long an invitation may take to reach its receiver.
const DELIVERY_WITHIN: i64 = 120;
/// Searches on different servers this close together could have found each other.
const NEAR_MISS_WITHIN: i64 = 300;
/// A public match its host waited in alone at least this long is worth a line.
const LONELY_WAIT: i64 = 60;

/// What the page asks for.
#[derive(Debug, Default, Deserialize)]
pub struct SessionsQuery {
    #[serde(default)]
    pub server: String,
    /// Unix seconds; the last day when left out.
    pub from: Option<i64>,
    pub to: Option<i64>,
}

/// One stored event.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub server: String,
    pub at: i64,
    pub last_at: i64,
    pub player: Option<i64>,
    pub name: String,
    pub kind: String,
    pub detail: Value,
    pub count: i64,
}

impl Event {
    /// Who it's about: the player's id on their server, or their name when the server
    /// didn't know it (a refused sign-in for a name with no account).
    fn who(&self) -> (String, String) {
        (
            self.server.clone(),
            self.player.map_or_else(|| format!("name:{}", self.name.to_lowercase()), |p| p.to_string()),
        )
    }

    /// The room it's about: its id, and when it was made (ids come again once old rooms are
    /// gone from the server, and after a restart).
    fn room(&self) -> Option<(i64, i64)> {
        Some((self.detail["room"].as_i64()?, self.detail["since"].as_i64().unwrap_or(0)))
    }

    fn str(&self, key: &str) -> &str {
        self.detail[key].as_str().unwrap_or_default()
    }
}

/// `text` without control characters or the ones that hide in text, cut to `max` characters.
fn clean(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !crate::hidden_char(*c))
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A detail object as sent, its strings cleaned and kept short.
fn clean_detail(v: &Value) -> Option<Value> {
    let Value::Object(map) = v else { return None };
    let mut out = serde_json::Map::new();
    for (k, v) in map.iter().take(24) {
        let key = clean(k, 32);
        let value = match v {
            Value::String(s) => Value::String(clean(s, 200)),
            Value::Number(_) | Value::Bool(_) | Value::Null => v.clone(),
            _ => continue,
        };
        out.insert(key, value);
    }
    Some(Value::Object(out))
}

/// A mode as people say it.
fn mode_name(mode: &str) -> &'static str {
    match mode {
        "svm" => "Spies vs Mercs",
        "coop" => "co-op",
        _ => "",
    }
}

/// What a search query looked for (as the game's queries were seen to be used).
fn query_name(query: i64) -> &'static str {
    match query {
        8 => "Spies vs Mercs (or an invited room)",
        11 => "co-op",
        _ => "a game",
    }
}

impl Coordinator {
    /// Takes a server's events. Answers how many were kept.
    pub(crate) async fn record_events(&self, server: &str, body: &Value) -> sqlx::Result<usize> {
        let Some(list) = body["events"].as_array() else { return Ok(0) };
        let now = identity::now();
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_events WHERE server_id = ?")
            .bind(server)
            .fetch_one(&self.pool)
            .await?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut kept = 0;
        for e in list.iter().take(MAX_EVENTS) {
            let (Some(id), Some(at)) = (e["id"].as_i64().filter(|n| *n > 0), e["at"].as_i64()) else {
                continue;
            };
            let last_at = e["last_at"].as_i64().unwrap_or(at).max(at);
            if at < now - KEEP_FOR || last_at > now + 300 {
                continue;
            }
            let Some(kind) = e["kind"].as_str().and_then(|k| KINDS.iter().find(|known| **known == k)) else {
                continue;
            };
            let Some(detail) = clean_detail(&e["detail"]) else { continue };
            let player = e["player"].as_i64().filter(|n| (0..=i64::from(u32::MAX)).contains(n));
            let name = clean(e["name"].as_str().unwrap_or_default(), 40);
            let count = e["count"].as_i64().unwrap_or(1).clamp(1, 1_000_000);
            let sql = if stored >= EVENTS_PER_SERVER {
                // Full: only events already here change.
                "UPDATE session_events SET last_at = ?3, count = ?8, detail = ?7 WHERE server_id = ?1 AND id = ?2"
            } else {
                "INSERT INTO session_events (server_id, id, at, last_at, player, name, kind, detail, count) VALUES (?1, ?2, ?9, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (server_id, id) DO UPDATE SET last_at = excluded.last_at, count = excluded.count, detail = excluded.detail"
            };
            sqlx::query(sql)
                .bind(server)
                .bind(id)
                .bind(last_at)
                .bind(player)
                .bind(&name)
                .bind(*kind)
                .bind(detail.to_string())
                .bind(count)
                .bind(at)
                .execute(&mut *tx)
                .await?;
            kept += 1;
        }
        tx.commit().await?;
        Ok(kept)
    }

    /// The Sessions page: the servers, each player's timeline, the problems found, and what
    /// went wrong with the servers themselves, in a range (the last day by default).
    pub(crate) async fn sessions_view(&self, q: &SessionsQuery) -> sqlx::Result<Value> {
        let now = identity::now();
        let to = q.to.unwrap_or(now).min(now);
        let from = q.from.unwrap_or(to - 86_400).max(to - LONGEST_RANGE).min(to);
        let server = q.server.trim();
        let servers: Vec<(String, Option<String>)> = sqlx::query_as("SELECT id, json_extract(listing, '$.name') FROM servers ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        // A little before the range too: what a player was doing when it starts.
        let rows: Vec<(String, i64, i64, Option<i64>, Option<String>, String, String, i64)> = sqlx::query_as(
            "SELECT server_id, at, last_at, player, name, kind, detail, count FROM session_events
             WHERE last_at >= ?1 AND at <= ?2 AND (?3 = '' OR server_id = ?3) ORDER BY at, id",
        )
        .bind(from - 3600)
        .bind(to)
        .bind(server)
        .fetch_all(&self.pool)
        .await?;
        let events: Vec<Event> = rows
            .into_iter()
            .map(|(server, at, last_at, player, name, kind, detail, count)| Event {
                server,
                at,
                last_at,
                player,
                name: name.unwrap_or_default(),
                kind,
                detail: serde_json::from_str(&detail).unwrap_or_else(|_| json!({})),
                count,
            })
            .collect();
        let sessions: Vec<(String, i64, i64, Option<i64>)> = sqlx::query_as(
            "SELECT server_id, player, started, ended FROM play_sessions
             WHERE started <= ?2 AND COALESCE(ended, ?2) >= ?1 AND (?3 = '' OR server_id = ?3) ORDER BY started",
        )
        .bind(from)
        .bind(to)
        .bind(server)
        .fetch_all(&self.pool)
        .await?;
        let names: Vec<(String, i64, String)> = sqlx::query_as(
            "SELECT p.server_id, p.id, p.name FROM players p
             WHERE EXISTS (SELECT 1 FROM play_sessions s WHERE s.server_id = p.server_id AND s.player = p.id AND s.started <= ?2 AND COALESCE(s.ended, ?2) >= ?1)
               AND (?3 = '' OR p.server_id = ?3)",
        )
        .bind(from)
        .bind(to)
        .bind(server)
        .fetch_all(&self.pool)
        .await?;
        let alerts: Vec<(String, String, String, String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT kind, server_id, level, detail, started_at, resolved_at FROM alerts
             WHERE started_at <= ?2 AND COALESCE(resolved_at, ?2) >= ?1 AND (?3 = '' OR server_id = '' OR server_id = ?3) ORDER BY started_at",
        )
        .bind(from)
        .bind(to)
        .bind(server)
        .fetch_all(&self.pool)
        .await?;
        let pings: Vec<(String, i64, Option<f64>)> =
            sqlx::query_as("SELECT server_id, at, ms FROM server_pings WHERE at BETWEEN ?1 AND ?2 AND (?3 = '' OR server_id = ?3) ORDER BY server_id, at")
                .bind(from)
                .bind(to)
                .bind(server)
                .fetch_all(&self.pool)
                .await?;

        let mut outages: Vec<Value> = alerts
            .into_iter()
            .map(|(kind, server, level, detail, started, resolved)| json!({ "server": server, "kind": kind, "level": level, "text": detail, "from": started, "to": resolved }))
            .collect();
        outages.extend(unanswered_pings(&pings));

        let names: HashMap<(String, i64), String> = names.into_iter().map(|(s, id, name)| ((s, id), name)).collect();
        let timelines = timelines(&events, &sessions, &names, from, to);
        let mut problems = problems(&events);
        problems.retain(|p| p["at"].as_i64().is_some_and(|at| (from..=to).contains(&at)));
        // A match the server reported finished with players in it wasn't waited in alone
        // (they may have joined in a way no event shows).
        let played: Vec<(String, i64)> = sqlx::query_as("SELECT server_id, started FROM matches WHERE players >= 2 AND started BETWEEN ?1 AND ?2")
            .bind(from - 3600)
            .bind(to)
            .fetch_all(&self.pool)
            .await?;
        problems.retain(|p| {
            let Some(since) = p["room_since"].as_i64() else { return true };
            !played.iter().any(|(sv, started)| p["server"] == json!(sv) && (started - since).abs() <= 5)
        });
        problems.sort_by_key(|p| std::cmp::Reverse(p["at"].as_i64()));
        let labels: Vec<(String, i64, String)> = sqlx::query_as("SELECT kind, id, name FROM labels").fetch_all(&self.pool).await?;
        Ok(json!({
            "labels": crate::game_names::labels(labels),
            "from": from,
            "to": to,
            "servers": servers.into_iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
            "players": timelines,
            "problems": problems,
            "outages": outages,
        }))
    }

    /// Deletes events past [`KEEP_FOR`] (with the rest of the daily cleanup).
    pub(crate) async fn prune_session_events(&self) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM session_events WHERE last_at < ?")
            .bind(identity::now() - KEEP_FOR)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// Times a server's API didn't answer two pings or more in a row (pings come every minute).
fn unanswered_pings(pings: &[(String, i64, Option<f64>)]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut run: Option<(String, i64, i64, usize)> = None;
    let close = |run: Option<(String, i64, i64, usize)>, out: &mut Vec<Value>| {
        if let Some((server, start, end, n)) = run.filter(|r| r.3 >= 2) {
            out.push(json!({
                "server": server, "kind": "api_unanswered", "level": "warn",
                "text": format!("The server's API didn't answer the coordinator ({n} pings in a row)"),
                "from": start, "to": end,
            }));
        }
    };
    for (server, at, ms) in pings {
        let same = run.as_ref().is_some_and(|r| r.0 == *server);
        if ms.is_none() {
            match &mut run {
                Some(r) if same => {
                    r.2 = *at;
                    r.3 += 1;
                }
                _ => {
                    close(run.take(), &mut out);
                    run = Some((server.clone(), *at, *at, 1));
                }
            }
        } else {
            close(run.take(), &mut out);
        }
    }
    close(run, &mut out);
    out
}

/// A room a player was in, while they were.
#[derive(Debug, Clone)]
struct Stay {
    room: (i64, i64),
    from: i64,
    kind: String,
    mode: String,
    private: bool,
    host: bool,
    /// Whose room it is (empty when the event didn't say).
    host_name: String,
    /// The map and game mode the room said (session attributes 101 and 102), if any.
    map: Option<i64>,
    game_mode: Option<i64>,
    /// How the player's game reached the host's, when they joined: [`NET_KEYS`].
    net: Value,
}

/// What a join says of the network between the guest and the host (the server's
/// `nat_helper::Path::detail`): each one's ping to the server, whether the relay carried
/// their traffic, and if so their round trip through it next to the least a direct one could
/// take.
const NET_KEYS: &[&str] = &["ping_ms", "host_ping_ms", "relayed", "relay_ms", "direct_ms"];

/// A stay that ended (or the range did): who, and how it ended.
struct Closed {
    who: (String, String),
    name: String,
    epoch: i64,
    stay: Stay,
    to: i64,
    /// How it ended: `how` (`left`, `removed` with `by_name`, `abandoned`, `dropped`,
    /// `restarted`, `signed_out`, `server_restart`, `still`) and whether the room `ended`.
    end: Value,
}

/// Each player's timeline: when they were online, the rooms they were in (party or match,
/// whose, which map, with whom and for how long, how they left), and the events worth a mark
/// (searches, stats, refusals, a game that restarted...).
fn timelines(events: &[Event], sessions: &[(String, i64, i64, Option<i64>)], names: &HashMap<(String, i64), String>, from: i64, to: i64) -> Vec<Value> {
    let mut players: BTreeMap<(String, String), Value> = BTreeMap::new();
    let entry = |players: &mut BTreeMap<(String, String), Value>, who: (String, String), name: &str| {
        players
            .entry(who.clone())
            .or_insert_with(|| json!({ "server": who.0, "player": who.1.parse::<i64>().ok(), "name": name, "online": [], "rooms": [], "marks": [] }))
            .clone()
    };
    for (server, player, started, ended) in sessions {
        let who = (server.clone(), player.to_string());
        let name = names.get(&(server.clone(), *player)).cloned().unwrap_or_else(|| format!("#{player}"));
        entry(&mut players, who.clone(), &name);
        let p = players.get_mut(&who).expect("just added");
        p["online"].as_array_mut().expect("an array").push(json!([started.max(&from), ended.map(|e| e.min(to))]));
    }

    let mut epoch: HashMap<&str, i64> = HashMap::new();
    let mut open: HashMap<(String, String), Vec<(Stay, String, i64)>> = HashMap::new();
    let mut closed: Vec<Closed> = Vec::new();
    // When each player's game was last heard from (any event), for a restart's time.
    let mut heard: HashMap<(String, String), i64> = HashMap::new();
    let close_all = |open: &mut HashMap<(String, String), Vec<(Stay, String, i64)>>, closed: &mut Vec<Closed>, who: &(String, String), at: i64, end: Value| {
        for (stay, name, ep) in open.remove(who).unwrap_or_default() {
            closed.push(Closed {
                who: who.clone(),
                name,
                epoch: ep,
                stay,
                to: at,
                end: end.clone(),
            });
        }
    };
    for e in events {
        let who = e.who();
        if e.kind == "server_start" {
            epoch.insert(&e.server, e.at);
            // Everyone on that server is out of every room.
            let on: Vec<_> = open.keys().filter(|w| w.0 == e.server).cloned().collect();
            for w in on {
                close_all(&mut open, &mut closed, &w, e.at, json!({ "how": "server_restart" }));
            }
            continue;
        }
        entry(&mut players, who.clone(), &e.name);
        if let Some(p) = players.get_mut(&who) {
            if !e.name.is_empty() {
                p["name"] = json!(e.name);
            }
        }
        let ep = epoch.get(e.server.as_str()).copied().unwrap_or(0);
        let mut mark: Option<Value> = None;
        match e.kind.as_str() {
            // Signed in with rooms still open: the game before stopped without a word (it
            // restarted, or crashed), and its rooms ended when it was last heard from.
            "signin" if open.get(&who).is_some_and(|o| !o.is_empty()) => {
                let last = heard.get(&who).copied().unwrap_or(e.at).min(e.at);
                let in_match = open.get(&who).is_some_and(|o| o.iter().any(|(s, _, _)| s.kind == "match"));
                close_all(&mut open, &mut closed, &who, last, json!({ "how": "restarted" }));
                mark = Some(json!({
                    "at": e.at, "last_at": e.at, "kind": "restart", "count": 1, "last_heard": last, "in_match": in_match,
                    "text": format!(
                        "Signed in again: the game before stopped without signing out (restarted, or crashed) {}",
                        if e.at - last < 60 { format!("{} s earlier", e.at - last) } else { format!("{} min earlier", (e.at - last) / 60) }
                    ),
                }));
            }
            "room" | "join" => {
                if let Some(room) = e.room() {
                    let stays = open.entry(who.clone()).or_default();
                    if !stays.iter().any(|(s, _, _)| s.room == room) {
                        stays.push((
                            Stay {
                                room,
                                from: e.at,
                                kind: e.str("room_kind").to_string(),
                                mode: e.str("mode").to_string(),
                                private: e.detail["private"].as_bool().unwrap_or(false),
                                host: e.kind == "room",
                                host_name: e.str("host_name").to_string(),
                                map: e.detail["map"].as_i64(),
                                game_mode: e.detail["game_mode"].as_i64(),
                                net: NET_KEYS
                                    .iter()
                                    .filter_map(|k| e.detail.get(*k).filter(|v| v.is_number() || v.is_boolean()).map(|v| ((*k).to_string(), v.clone())))
                                    .collect::<serde_json::Map<_, _>>()
                                    .into(),
                            },
                            e.name.clone(),
                            ep,
                        ));
                    }
                }
            }
            "leave" => {
                if let (Some(room), Some(stays)) = (e.room(), open.get_mut(&who)) {
                    if let Some(i) = stays.iter().position(|(s, _, _)| s.room == room) {
                        let (stay, name, ep) = stays.remove(i);
                        let mut end = serde_json::Map::new();
                        end.insert("how".into(), json!(e.str("how")));
                        for k in ["by_name", "ended"] {
                            if let Some(v) = e.detail.get(k) {
                                end.insert(k.into(), v.clone());
                            }
                        }
                        closed.push(Closed {
                            who: who.clone(),
                            name,
                            epoch: ep,
                            stay,
                            to: e.at,
                            end: end.into(),
                        });
                    }
                }
            }
            "signout" => close_all(&mut open, &mut closed, &who, e.at, json!({ "how": "signed_out", "signout": e.str("how") })),
            _ => {}
        }
        heard.insert(who.clone(), heard.get(&who).copied().unwrap_or(0).max(e.last_at.max(e.at)));
        let mark = mark.or_else(|| {
            (e.last_at >= from && !matches!(e.kind.as_str(), "room" | "leave" | "join" | "signout"))
                .then(|| json!({ "at": e.at.max(from), "last_at": e.last_at, "kind": e.kind, "count": e.count, "text": describe(e), "report": e.detail.get("report") }))
        });
        if let (Some(mark), Some(p)) = (mark, players.get_mut(&who)) {
            if mark["last_at"].as_i64().is_some_and(|t| t >= from) {
                p["marks"].as_array_mut().expect("an array").push(mark);
            }
        }
    }
    // Still in a room at the end of the range.
    let still: Vec<_> = open.keys().cloned().collect();
    for who in still {
        close_all(&mut open, &mut closed, &who, to, json!({ "how": "still" }));
    }

    // With whom: who else was in the same room at the same time, and for how long together.
    for c in &closed {
        if c.to < from {
            continue;
        }
        let mut with: Vec<(String, i64)> = closed
            .iter()
            .filter(|o| o.who != c.who && o.who.0 == c.who.0 && o.epoch == c.epoch && o.stay.room == c.stay.room)
            .map(|o| (o.name.clone(), c.to.min(o.to) - c.stay.from.max(o.stay.from)))
            .filter(|(_, together)| *together > 0)
            .collect();
        with.sort();
        with.dedup_by(|a, b| {
            let same = a.0 == b.0;
            if same {
                b.1 += a.1;
            }
            same
        });
        let stayed = c.to - c.stay.from;
        // Only for someone there for part of it: the 1 s a removed player spent in a match.
        let together: serde_json::Map<String, Value> = with.iter().filter(|(_, t)| *t < stayed).map(|(n, t)| (n.clone(), json!(t))).collect();
        if let Some(p) = players.get_mut(&c.who) {
            p["rooms"].as_array_mut().expect("an array").push(json!({
                "room": c.stay.room.0, "since": c.stay.room.1, "from": c.stay.from.max(from), "to": c.to.min(to), "kind": c.stay.kind,
                "mode": c.stay.mode, "private": c.stay.private, "host": c.stay.host, "host_name": c.stay.host_name,
                "map": c.stay.map, "game_mode": c.stay.game_mode,
                "with": with.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(), "together": together, "net": c.stay.net, "end": c.end,
            }));
        }
    }
    players
        .into_values()
        .map(|mut p| {
            if let Some(rooms) = p["rooms"].as_array_mut() {
                rooms.sort_by_key(|r| r["from"].as_i64());
            }
            p
        })
        .filter(|p| ["online", "rooms", "marks"].iter().any(|k| p[*k].as_array().is_some_and(|a| !a.is_empty())))
        .collect()
}

/// An event, said in a few words.
fn describe(e: &Event) -> String {
    let times = if e.count > 1 { format!(" ({} times)", e.count) } else { String::new() };
    match e.kind.as_str() {
        "signin" => "Signed in".into(),
        "signin_refused" => {
            let why = match e.str("reason") {
                "outdated" => format!("outdated client {}", e.str("client")),
                "no_current_client" => "its game client isn't current (or never signed in to the API)".into(),
                "too_many" => "too many attempts".into(),
                "wrong_password" => "wrong password".into(),
                "banned" => "banned".into(),
                other => other.to_string(),
            };
            format!("Sign-in refused: {}{times}", why.trim())
        }
        "search" => {
            let found = e.detail["found"].as_i64().unwrap_or(0);
            format!(
                "Searched for {}: {}",
                query_name(e.detail["query"].as_i64().unwrap_or(0)),
                if found == 0 { "nothing found".into() } else { format!("{found} found") }
            )
        }
        "join_failed" => match e.detail["host_name"].as_str() {
            Some(host) => format!("Couldn't join {host}'s room ({})", join_error(&e.str("code"))),
            None => format!("Couldn't join a room ({})", join_error(&e.str("code"))),
        },
        "invite" => match e.detail["to_name"].as_str() {
            Some(to) if e.room().is_some() => format!("Invited {to}"),
            Some(to) => format!("Invited {to}, but had no room to invite them into"),
            None => "Invited someone".into(),
        },
        "invite_delivered" => format!("Got {}'s invitation", e.detail["from_name"].as_str().unwrap_or("an")),
        "stats" => format!("Stats written: a mission or match ended{times}"),
        "nat" => {
            if e.detail["relayed"].as_bool().unwrap_or(false) {
                "Online play relayed by the server".into()
            } else {
                "Online play direct".into()
            }
        }
        "client_log" => {
            let times = if e.count > 1 { format!(" ({} times)", e.count) } else { String::new() };
            format!("Game {}: {}{times}", e.str("level"), e.str("message"))
        }
        "nat_lost" => format!(
            "The game stopped registering for online play ({} s since its last check-in) while still signed in: nobody could reach it until it registered again",
            e.detail["probe_secs"]
        ),
        "nat_missing" => format!(
            "In a room, but the game hadn't registered for online play {} s later: nobody could reach it",
            e.detail["after_secs"]
        ),
        "relay_drop" => {
            let pps = |k: &str| e.detail[k].as_f64().map_or_else(|| "?".into(), |v| format!("{v:.0}"));
            let which = if e.str("direction") == "sending" { "from them" } else { "to them" };
            if e.detail["after"].as_f64() == Some(0.0) {
                format!("Relayed traffic {which} stopped ({} packets/s before)", pps("before"))
            } else {
                format!("Relayed traffic {which} fell: {} → {} packets/s", pps("before"), pps("after"))
            }
        }
        "request_error" => format!("A request failed: {} {}{times}", e.str("call"), e.str("error")),
        "log_sent" => format!(
            "Sent its game log, as asked after {} (in Reports, Game logs)",
            match e.str("problem") {
                "join_failed" => "a failed join",
                "nat_lost" => "its registration lapsed",
                "relay_stopped" => "its relayed traffic stopped",
                "restarted" => "it dropped and came back",
                other => other,
            }
        ),
        "report_refused" => format!(
            "A report from the launcher was refused: {} ({} files, {} KB){times}",
            e.str("why"),
            e.detail["files"].as_i64().unwrap_or(0),
            e.detail["bytes"].as_i64().unwrap_or(0) / 1024
        ),
        other => other.to_string(),
    }
}

/// The problems in a run of events (sorted by time), each `{at, server, level (bad, warn,
/// info), player, name, title, text}`.
/// A failed join's code with its name: the game's codes are the CRC-32 of the error's name.
fn join_error(code: &str) -> String {
    let name = match code {
        "0xb08a1a05" => "CONNECTION_FAILED: couldn't reach the host",
        "0x97182b1b" => "LOCKED_SESSION: the match had started",
        "0xeea440ee" => "DATA_VERSION_MISMATCH: different game data (a mod)",
        _ => return code.to_string(),
    };
    format!("{name}, {code}")
}

pub(crate) fn problems(events: &[Event]) -> Vec<Value> {
    let mut out = Vec::new();
    let problem = |e: &Event, level: &str, title: String, text: String| json!({ "at": e.at, "server": e.server, "level": level, "player": e.player, "name": e.name, "title": title, "text": text });
    let by_who = {
        let mut m: HashMap<(String, String), Vec<&Event>> = HashMap::new();
        for e in events {
            m.entry(e.who()).or_default().push(e);
        }
        m
    };

    for e in events {
        match e.kind.as_str() {
            // Refused sign-ins, one problem a player (from their first): worse when they never
            // got in after.
            "signin_refused" => {
                let Some(list) = by_who.get(&e.who()) else { continue };
                let refusals: Vec<&&Event> = list.iter().filter(|o| o.kind == "signin_refused").collect();
                if !refusals.first().is_some_and(|first| std::ptr::eq(**first, e)) {
                    continue;
                }
                let last = refusals.iter().map(|o| o.last_at).max().unwrap_or(e.last_at);
                let got_in = list.iter().any(|o| o.kind == "signin" && o.at >= last);
                let total: i64 = refusals.iter().map(|o| o.count).sum();
                let only_passwords = refusals.iter().all(|o| o.str("reason") == "wrong_password");
                let level = if got_in || only_passwords && total < 5 { "info" } else { "bad" };
                if level == "info" && total < 3 {
                    continue;
                }
                let title = if got_in {
                    format!("{} was refused, then got in", e.name)
                } else {
                    format!("{} couldn't sign in", e.name)
                };
                let mut reasons: Vec<(String, i64)> = Vec::new();
                for o in &refusals {
                    let reason = describe(&Event { count: 1, ..(**o).clone() }).trim_start_matches("Sign-in refused: ").to_string();
                    match reasons.iter_mut().find(|(r, _)| *r == reason) {
                        Some((_, n)) => *n += o.count,
                        None => reasons.push((reason, o.count)),
                    }
                }
                let reasons: Vec<String> = reasons.into_iter().map(|(r, n)| if n > 1 { format!("{r} ({n} times)") } else { r }).collect();
                let mut p = problem(e, level, title, format!("Refused {total} times: {}", reasons.join("; ")));
                p["until"] = json!(last);
                out.push(p);
            }
            "join_failed" => out.push(problem(e, "bad", format!("{} couldn't join a room", e.name), describe(e))),
            // What they wanted to tell the admins didn't arrive.
            "report_refused" => out.push(problem(e, "warn", format!("{}'s report was refused", e.name), describe(e))),
            // Joins with them fail (CONNECTION_FAILED) until a restart registers it.
            "nat_missing" => out.push(problem(e, "bad", format!("{}'s game couldn't be reached", e.name), describe(e))),
            // Joins with them fail (CONNECTION_FAILED) until it registers again.
            "nat_lost" => out.push(problem(e, "bad", format!("{}'s game dropped off the relay", e.name), describe(e))),
            // The game's own errors; its warnings and network events are on its timeline.
            "client_log" if e.str("level") == "error" => out.push(problem(e, "warn", format!("{}'s game reported an error", e.name), describe(e))),
            "request_error" => out.push(problem(
                e,
                "warn",
                format!("A request of {} failed", if e.name.is_empty() { "a player" } else { &e.name }),
                describe(e),
            )),
            "invite" => {
                let to = e.detail["to"].as_i64();
                if e.room().is_none() {
                    out.push(problem(e, "warn", format!("{}'s invitation had no room", e.name), describe(e)));
                    continue;
                }
                let delivered = events.iter().any(|o| {
                    o.kind == "invite_delivered"
                        && o.server == e.server
                        && o.player == to
                        && o.detail["from"].as_i64() == e.player
                        && (e.at..=e.at + DELIVERY_WITHIN).contains(&o.at)
                });
                if !delivered {
                    let to = e.detail["to_name"].as_str().unwrap_or("someone");
                    out.push(problem(
                        e,
                        "warn",
                        format!("{}'s invitation to {to} never arrived", e.name),
                        format!("{to} didn't pick it up within two minutes (offline, or their game had left the online menus)"),
                    ));
                }
            }
            // Traffic stopped, then the player left their match without it ending (no stats).
            "relay_drop" => {
                let Some(list) = by_who.get(&e.who()) else { continue };
                let left = list
                    .iter()
                    .find(|o| o.kind == "leave" && o.str("room_kind") == "match" && (e.at..=e.at + DROPOUT_WITHIN).contains(&o.at));
                let Some(left) = left else { continue };
                let ended_normally = list.iter().any(|o| o.kind == "stats" && (left.at - STATS_NEAR..=left.at + STATS_NEAR).contains(&o.at));
                let earlier_drop = list
                    .iter()
                    .any(|o| o.kind == "relay_drop" && o.at < e.at && (o.at..=o.at + DROPOUT_WITHIN).contains(&left.at));
                if ended_normally || earlier_drop {
                    continue;
                }
                let others: Vec<&str> = events
                    .iter()
                    .filter(|o| o.server == e.server && o.room() == left.room() && o.kind == "leave" && o.who() != e.who() && (left.at..=left.at + DROPOUT_WITHIN).contains(&o.at))
                    .map(|o| o.name.as_str())
                    .collect();
                let mode = mode_name(left.str("mode"));
                out.push(problem(
                    e,
                    "bad",
                    format!("{} dropped out of a {}match", e.name, if mode.is_empty() { String::new() } else { format!("{mode} ") }),
                    format!(
                        "{}; they left the match {} s later with no stats written (it didn't end normally){}",
                        describe(e),
                        left.at - e.at,
                        if others.is_empty() {
                            String::new()
                        } else {
                            format!(". Also left: {}", others.join(", "))
                        }
                    ),
                ));
            }
            // The game went quiet while the player was in a match.
            "signout" if e.str("how") == "timed_out" => {
                let Some(list) = by_who.get(&e.who()) else { continue };
                let mut rooms: HashMap<(i64, i64), &Event> = HashMap::new();
                for o in list.iter().filter(|o| o.at <= e.at) {
                    match (o.kind.as_str(), o.room()) {
                        ("room" | "join", Some(r)) => {
                            rooms.insert(r, o);
                        }
                        ("leave", Some(r)) => {
                            rooms.remove(&r);
                        }
                        ("signout" | "signin", _) if o.at < e.at => rooms.clear(),
                        _ => {}
                    }
                }
                // Only a match with someone else still in it: the game times out on every quit,
                // and a match room alone is just the mission menu.
                let with_others = |m: &Event| {
                    let room = m.room();
                    // Since the room was made (its id comes again after a restart).
                    let made = events
                        .iter()
                        .filter(|o| o.server == e.server && o.kind == "room" && o.room() == room && o.at <= m.at)
                        .map(|o| o.at)
                        .max()
                        .unwrap_or(m.at);
                    events.iter().any(|o| {
                        o.server == e.server
                            && o.who() != e.who()
                            && o.room() == room
                            && matches!(o.kind.as_str(), "room" | "join")
                            && (made..=e.at).contains(&o.at)
                            && !events
                                .iter()
                                .any(|l| l.who() == o.who() && l.kind == "leave" && l.room() == room && (o.at..e.at).contains(&l.at))
                    })
                };
                if let Some(m) = rooms.values().find(|o| o.str("room_kind") == "match" && with_others(o)) {
                    let mode = mode_name(m.str("mode"));
                    out.push(problem(
                        e,
                        "warn",
                        format!(
                            "{}'s game went quiet in a {}match",
                            e.name,
                            if mode.is_empty() { String::new() } else { format!("{mode} ") }
                        ),
                        "Their connection timed out without leaving the match first: a crash, or their connection was lost".into(),
                    ));
                }
            }
            _ => {}
        }
    }

    // Searches on different servers at about the same time, for the same thing, that both
    // found nothing: on one server, they'd have found each other.
    let empty: Vec<&Event> = events.iter().filter(|e| e.kind == "search" && e.detail["found"].as_i64() == Some(0)).collect();
    let mut paired: HashSet<(String, String)> = HashSet::new();
    for (i, a) in empty.iter().enumerate() {
        for b in &empty[i + 1..] {
            if b.at - a.at > NEAR_MISS_WITHIN {
                break;
            }
            if a.server == b.server || a.detail["query"] != b.detail["query"] || a.who() == b.who() {
                continue;
            }
            let key = if a.name < b.name {
                (a.name.clone(), b.name.clone())
            } else {
                (b.name.clone(), a.name.clone())
            };
            if !paired.insert(key) {
                continue;
            }
            let what = query_name(a.detail["query"].as_i64().unwrap_or(0));
            let mut p = problem(
                b,
                "info",
                format!("{} and {} missed each other", a.name, b.name),
                format!("Both searched for {what} within {} s of each other on different servers, and found nobody", b.at - a.at),
            );
            p["servers"] = json!([a.server, b.server]);
            out.push(p);
        }
    }

    // Public matches their host waited in alone, until they gave up.
    for e in events
        .iter()
        .filter(|e| e.kind == "room" && e.str("room_kind") == "match" && !e.detail["private"].as_bool().unwrap_or(false))
    {
        let Some(room) = e.room() else { continue };
        let Some(list) = by_who.get(&e.who()) else { continue };
        let Some(end) = list
            .iter()
            .find(|o| o.kind == "leave" && o.room() == Some(room) && o.at >= e.at && o.detail["ended"].as_bool() == Some(true))
        else {
            continue;
        };
        let joined = events
            .iter()
            .any(|o| o.server == e.server && o.kind == "join" && o.room() == Some(room) && (e.at..=end.at).contains(&o.at));
        // Gone on to another match meanwhile: the room was left behind, not waited in.
        let moved_on = list
            .iter()
            .any(|o| (e.at + 1..end.at).contains(&o.at) && (o.kind == "join" || o.kind == "room" && o.str("room_kind") == "match"));
        if !joined && !moved_on && end.at - e.at >= LONELY_WAIT {
            let mode = mode_name(e.str("mode"));
            let mut p = problem(
                e,
                "info",
                format!("{} waited for players", e.name),
                format!(
                    "Hosted a public {} match for {} min and nobody joined",
                    if mode.is_empty() { "" } else { mode },
                    (end.at - e.at + 30) / 60
                ),
            );
            // For [`Coordinator::sessions_view`] to check against the finished matches.
            p["room_since"] = e.detail["since"].clone();
            out.push(p);
        }
    }

    // A game that signed in again while still in a match: the one before stopped without a
    // word (restarted, or crashed), and the match went on without it (tacit_danger and
    // Ghost_Leader on eu1, 2026-10-06 17:36, when their host's game removed them).
    for list in by_who.values() {
        let mut in_match: Vec<(i64, i64)> = Vec::new();
        for e in list {
            match e.kind.as_str() {
                "room" | "join" if e.str("room_kind") == "match" => in_match.extend(e.room()),
                "leave" => in_match.retain(|r| Some(*r) != e.room()),
                "signout" => in_match.clear(),
                "signin" if !in_match.is_empty() => {
                    let last = list.iter().filter(|o| o.at < e.at).map(|o| o.last_at).max().unwrap_or(e.at);
                    out.push(problem(
                        e,
                        "warn",
                        format!("{}'s game restarted in a match", e.name),
                        format!(
                            "Signed in again {} s after the game was last heard, without leaving the match or signing out: it restarted or crashed",
                            e.at - last
                        ),
                    ));
                    in_match.clear();
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::problems;
    use super::Event;

    fn ev(server: &str, at: i64, player: i64, name: &str, kind: &str, detail: serde_json::Value) -> Event {
        Event {
            server: server.into(),
            at,
            last_at: at,
            player: Some(player),
            name: name.into(),
            kind: kind.into(),
            detail,
            count: 1,
        }
    }

    fn titles(events: &[Event]) -> Vec<String> {
        problems(events).iter().map(|p| p["title"].as_str().unwrap_or_default().to_string()).collect()
    }

    /// Tonight's dropout on NA1, as it happened: traffic fell, the player left the match a
    /// minute later, nobody wrote stats.
    #[test]
    fn a_dropout_after_the_relay_went_quiet_is_a_problem() {
        let coop = json!({ "room": 16, "room_kind": "match", "mode": "coop", "private": true });
        let mut events = vec![
            ev("na", 100, 2, "Viper", "join", coop.clone()),
            ev("na", 1300, 2, "Viper", "relay_drop", json!({ "direction": "sending", "before": 65, "after": 8 })),
            ev(
                "na",
                1356,
                2,
                "Viper",
                "leave",
                json!({ "room": 16, "how": "left", "ended": false, "room_kind": "match", "mode": "coop" }),
            ),
        ];
        assert_eq!(titles(&events), ["Viper dropped out of a co-op match"]);
        // A match that ended (stats written) isn't.
        events.push(ev("na", 1350, 2, "Viper", "stats", json!({})));
        assert!(titles(&events).is_empty());
    }

    /// Tonight's on NA1: a game that signed in and never registered for online play, and
    /// the joins with it that failed to connect, named.
    #[test]
    fn a_game_nobody_could_reach_is_a_problem_and_join_codes_are_named() {
        let events = vec![
            ev("na", 100, 13, "emeraldknight33", "nat_missing", json!({ "after_secs": 92 })),
            ev(
                "na",
                200,
                13,
                "emeraldknight33",
                "join_failed",
                json!({ "room": 8, "code": "0xb08a1a05", "host_name": "SirCooms" }),
            ),
        ];
        let found = problems(&events);
        let lost = problems(&[ev("na", 300, 4, "SirCooms", "nat_lost", json!({ "probe_secs": 95 }))]);
        let logged = problems(&[
            ev(
                "na",
                310,
                4,
                "SirCooms",
                "client_log",
                json!({ "level": "error", "target": "hooks::hooks::nat", "message": "NAT: can't resolve the helper" }),
            ),
            ev(
                "na",
                311,
                4,
                "SirCooms",
                "client_log",
                json!({ "level": "info", "target": "hooks::hooks::nat", "message": "NAT: Storm socket bound" }),
            ),
        ]);
        assert_eq!(logged.len(), 1, "only errors are problems");
        assert_eq!(logged[0]["text"], "Game error: NAT: can't resolve the helper");
        assert_eq!(lost[0]["title"], "SirCooms's game dropped off the relay");
        assert!(lost[0]["text"].as_str().unwrap().contains("95 s"), "{}", lost[0]);
        assert_eq!(found[0]["title"], "emeraldknight33's game couldn't be reached");
        assert!(found[0]["text"].as_str().unwrap().contains("92 s later"), "{}", found[0]);
        assert!(found[1]["text"].as_str().unwrap().contains("CONNECTION_FAILED"), "{}", found[1]);
        assert_eq!(super::join_error("0x12345678"), "0x12345678", "an unknown code as it is");
    }

    #[test]
    fn refusals_failed_joins_and_lost_invitations() {
        let mut refused = ev(
            "eu",
            100,
            7,
            "Renegade",
            "signin_refused",
            json!({ "reason": "outdated", "via": "api", "client": "game/0.4.0" }),
        );
        refused.count = 351;
        let events = vec![
            refused,
            ev(
                "eu",
                200,
                14,
                "Maiorenko",
                "join_failed",
                json!({ "room": 17, "code": "0x97182b1b", "host_name": "VlaBadger" }),
            ),
            ev("na", 100, 5, "Theusma", "invite", json!({ "to": 11, "to_name": "Viper", "room": 16 })),
            ev("na", 400, 5, "Theusma", "invite", json!({ "to": 11, "to_name": "Viper", "room": 16 })),
            ev("na", 410, 11, "Viper", "invite_delivered", json!({ "from": 5, "room": 16 })),
        ];
        assert_eq!(
            titles(&events),
            ["Renegade couldn't sign in", "Maiorenko couldn't join a room", "Theusma's invitation to Viper never arrived"]
        );
    }

    #[test]
    fn players_searching_on_different_servers_missed_each_other() {
        let events = vec![
            ev("eu", 1000, 16, "Gyro", "search", json!({ "query": 8, "found": 0 })),
            ev("na", 1056, 4, "SirCooms", "search", json!({ "query": 8, "found": 0 })),
            ev("na", 1057, 4, "SirCooms", "search", json!({ "query": 8, "found": 0 })),
            ev("na", 1100, 5, "Theusma", "search", json!({ "query": 11, "found": 0 })),
        ];
        assert_eq!(titles(&events), ["Gyro and SirCooms missed each other"]);
    }

    #[test]
    fn a_public_match_nobody_joined() {
        let svm = json!({ "room": 6, "room_kind": "match", "mode": "svm", "private": false });
        let events = vec![
            ev("na", 1000, 4, "SirCooms", "room", svm),
            ev("na", 1150, 4, "SirCooms", "leave", json!({ "room": 6, "how": "abandoned", "ended": true })),
        ];
        assert_eq!(titles(&events), ["SirCooms waited for players"]);
    }

    /// A game timing out is how every quit looks; it counts only in a match with others.
    #[test]
    fn going_quiet_counts_only_in_a_match_with_others() {
        let coop = |room| json!({ "room": room, "room_kind": "match", "mode": "coop", "private": true });
        let alone = vec![
            ev("na", 100, 5, "Theusma", "room", coop(15)),
            ev("na", 900, 5, "Theusma", "signout", json!({ "how": "timed_out" })),
        ];
        assert!(titles(&alone).is_empty());
        let together = vec![
            ev("na", 100, 5, "Theusma", "room", coop(16)),
            ev("na", 130, 11, "Viper", "join", coop(16)),
            ev("na", 900, 5, "Theusma", "signout", json!({ "how": "timed_out" })),
        ];
        assert_eq!(titles(&together), ["Theusma's game went quiet in a co-op match"]);
    }

    #[test]
    fn timelines_show_rooms_with_whom() {
        let coop = |room| json!({ "room": room, "room_kind": "match", "mode": "coop", "private": true });
        let events = vec![
            ev("na", 100, 5, "Theusma", "room", coop(16)),
            ev("na", 130, 11, "Viper", "join", coop(16)),
            ev("na", 1356, 11, "Viper", "leave", json!({ "room": 16 })),
            ev("na", 1500, 5, "Theusma", "signout", json!({ "how": "timed_out" })),
        ];
        let sessions = vec![("na".to_string(), 5, 50, Some(1500)), ("na".to_string(), 11, 120, Some(1400))];
        let t = super::timelines(&events, &sessions, &std::collections::HashMap::new(), 0, 2000);
        assert_eq!(t.len(), 2);
        let theusma = t.iter().find(|p| p["name"] == "Theusma").unwrap();
        assert_eq!(theusma["rooms"][0]["with"], json!(["Viper"]));
        assert_eq!((theusma["rooms"][0]["from"].as_i64(), theusma["rooms"][0]["to"].as_i64()), (Some(100), Some(1500)));
        let viper = t.iter().find(|p| p["name"] == "Viper").unwrap();
        assert_eq!((viper["rooms"][0]["to"].as_i64(), viper["rooms"][0]["host"].as_bool()), (Some(1356), Some(false)));
        assert_eq!(viper["online"][0], json!([120, 1400]));
    }

    /// Oni's match on eu1 (2026-10-06): tacit_danger in it with Renegade for one second,
    /// removed by the host, a restart with rooms still open, and a room still open at the end.
    #[test]
    fn timelines_say_how_each_stay_ended_and_with_whom() {
        let m = |extra: serde_json::Value| {
            let mut v = json!({ "room": 102, "since": 90, "room_kind": "match", "mode": "coop", "host_name": "Oni", "map": 2573003522_u32, "game_mode": 4 });
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            v
        };
        let party = |room: i64| json!({ "room": room, "since": room, "room_kind": "party" });
        let events = vec![
            ev("eu", 90, 1032, "Oni", "room", m(json!({}))),
            ev("eu", 100, 1006, "tacit_danger", "join", m(json!({}))),
            ev("eu", 400, 1007, "Renegade", "join", m(json!({}))),
            ev("eu", 401, 1007, "Renegade", "leave", m(json!({ "how": "removed", "by_name": "Oni" }))),
            ev("eu", 500, 1006, "tacit_danger", "room", party(97)),
            ev(
                "eu",
                2900,
                1006,
                "tacit_danger",
                "relay_drop",
                json!({ "direction": "sending", "before": 30.0, "after": 0.0 }),
            ),
            ev("eu", 2940, 1006, "tacit_danger", "signin", json!({})),
            ev("eu", 2960, 1006, "tacit_danger", "join", m(json!({ "since": 90 }))),
            ev("eu", 2980, 1006, "tacit_danger", "leave", m(json!({ "how": "removed", "by_name": "Oni" }))),
        ];
        let t = super::timelines(&events, &[], &std::collections::HashMap::new(), 0, 3000);
        let tacit = t.iter().find(|p| p["name"] == "tacit_danger").unwrap();
        let rooms = tacit["rooms"].as_array().unwrap();
        // The first stay in the match: with Oni throughout, Renegade for 1 s; ended by the restart.
        let first = &rooms[0];
        assert_eq!((first["from"].as_i64(), first["to"].as_i64()), (Some(100), Some(2900)), "ends when last heard");
        assert_eq!(first["with"], json!(["Oni", "Renegade"]));
        assert_eq!(first["together"], json!({ "Renegade": 1 }));
        assert_eq!(first["end"]["how"], "restarted");
        assert_eq!(
            (first["map"].as_i64(), first["game_mode"].as_i64(), first["host_name"].as_str()),
            (Some(2573003522), Some(4), Some("Oni"))
        );
        // The party opened meanwhile went with the restart too.
        assert_eq!(rooms.iter().filter(|r| r["end"]["how"] == "restarted").count(), 2);
        // Back in, and removed 20 s later.
        let last = rooms.last().unwrap();
        assert_eq!(
            (last["to"].as_i64(), last["end"]["how"].as_str(), last["end"]["by_name"].as_str()),
            (Some(2980), Some("removed"), Some("Oni"))
        );
        let restart = tacit["marks"].as_array().unwrap().iter().find(|m| m["kind"] == "restart").unwrap();
        assert_eq!(
            (restart["at"].as_i64(), restart["last_heard"].as_i64(), restart["in_match"].as_bool()),
            (Some(2940), Some(2900), Some(true))
        );
        let drop = tacit["marks"].as_array().unwrap().iter().find(|m| m["kind"] == "relay_drop").unwrap();
        assert_eq!(drop["text"], "Relayed traffic from them stopped (30 packets/s before)");
        // Oni's match is still open at the end of the range.
        let oni = t.iter().find(|p| p["name"] == "Oni").unwrap();
        assert_eq!(oni["rooms"][0]["end"]["how"], "still");
        assert_eq!(oni["rooms"][0]["to"].as_i64(), Some(3000));
        assert!(titles(&events).contains(&"tacit_danger's game restarted in a match".to_string()), "{:?}", titles(&events));
    }

    /// A refused report is a warning: what the player wanted to tell the admins didn't arrive.
    #[test]
    fn a_refused_report_is_a_warning_saying_why() {
        let mut refused = ev(
            "na",
            100,
            10,
            "NexusXDev25",
            "report_refused",
            json!({ "reason": "invalid", "why": "a file's name isn't one the launcher sends", "files": 5, "bytes": 2_100_000 }),
        );
        refused.count = 2;
        let found = problems(&[refused]);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0]["level"].as_str(), found[0]["title"].as_str()),
            (Some("warn"), Some("NexusXDev25's report was refused"))
        );
        assert_eq!(
            found[0]["text"],
            "A report from the launcher was refused: a file's name isn't one the launcher sends (5 files, 2050 KB) (2 times)"
        );
    }

    /// A join says how the guest's game reached the host's; the room on the timeline keeps
    /// that, and nothing else of the detail.
    #[test]
    fn joined_rooms_keep_how_the_guest_reached_the_host() {
        let mut join = json!({ "room": 16, "room_kind": "match", "mode": "svm", "via": "search" });
        join.as_object_mut().unwrap().extend(
            json!({ "relayed": true, "ping_ms": 38, "host_ping_ms": 41, "relay_ms": 79, "direct_ms": 4, "ip": "198.51.100.7" })
                .as_object()
                .unwrap()
                .clone(),
        );
        let events = vec![
            ev("oc", 100, 5, "Kiwi", "room", json!({ "room": 16, "room_kind": "match", "mode": "svm" })),
            ev("oc", 130, 11, "Tui", "join", join),
            ev("oc", 900, 11, "Tui", "leave", json!({ "room": 16 })),
        ];
        let t = super::timelines(&events, &[], &std::collections::HashMap::new(), 0, 2000);
        let tui = t.iter().find(|p| p["name"] == "Tui").unwrap();
        assert_eq!(
            tui["rooms"][0]["net"],
            json!({ "relayed": true, "ping_ms": 38, "host_ping_ms": 41, "relay_ms": 79, "direct_ms": 4 })
        );
        let kiwi = t.iter().find(|p| p["name"] == "Kiwi").unwrap();
        assert_eq!(kiwi["rooms"][0]["net"], json!({}), "the host's own room says nothing of it");
    }
}
