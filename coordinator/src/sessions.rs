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
    "relay_drop",
    "request_error",
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
        let mut tx = self.pool.begin().await?;
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
        Ok(json!({
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
}

/// Each player's timeline: when they were online, the rooms they were in (party or match,
/// whose, with whom), and the events worth a mark (searches, stats, refusals...).
fn timelines(events: &[Event], sessions: &[(String, i64, i64, Option<i64>)], names: &HashMap<(String, i64), String>, from: i64, to: i64) -> Vec<Value> {
    // Who was in each room (per server, and per server start: ids come again after one).
    let mut epoch: HashMap<&str, i64> = HashMap::new();
    let mut members: HashMap<(String, i64, (i64, i64)), HashSet<String>> = HashMap::new();
    for e in events {
        if e.kind == "server_start" {
            epoch.insert(&e.server, e.at);
        }
        if let (Some(room), "room" | "join") = (e.room(), e.kind.as_str()) {
            let ep = epoch.get(e.server.as_str()).copied().unwrap_or(0);
            members.entry((e.server.clone(), ep, room)).or_default().insert(e.name.clone());
        }
    }

    let mut players: BTreeMap<(String, String), Value> = BTreeMap::new();
    let mut open: HashMap<(String, String), Vec<Stay>> = HashMap::new();
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
    let close = |players: &mut BTreeMap<(String, String), Value>,
                 who: &(String, String),
                 stay: Stay,
                 until: i64,
                 ep: i64,
                 members: &HashMap<(String, i64, (i64, i64)), HashSet<String>>,
                 me: &str| {
        if until < from {
            return;
        }
        let mut with: Vec<String> = members
            .get(&(who.0.clone(), ep, stay.room))
            .map(|m| m.iter().filter(|n| n.as_str() != me).cloned().collect())
            .unwrap_or_default();
        with.sort();
        if let Some(p) = players.get_mut(who) {
            p["rooms"].as_array_mut().expect("an array").push(json!({
                "room": stay.room.0, "from": stay.from.max(from), "to": until.min(to), "kind": stay.kind,
                "mode": stay.mode, "private": stay.private, "host": stay.host, "with": with,
            }));
        }
    };
    for e in events {
        let who = e.who();
        if e.kind == "server_start" {
            let before = epoch.insert(&e.server, e.at).unwrap_or(0);
            // Everyone on that server is out of every room.
            for (w, stays) in &mut open {
                if w.0 == e.server {
                    let me = players.get(w).and_then(|p| p["name"].as_str().map(str::to_string)).unwrap_or_default();
                    for stay in stays.drain(..) {
                        close(&mut players, w, stay, e.at, before, &members, &me);
                    }
                }
            }
            continue;
        }
        let ep = epoch.get(e.server.as_str()).copied().unwrap_or(0);
        if e.player.is_none() && e.name.is_empty() {
            continue;
        }
        entry(&mut players, who.clone(), &e.name);
        if let Some(p) = players.get_mut(&who) {
            if !e.name.is_empty() {
                p["name"] = json!(e.name);
            }
        }
        match e.kind.as_str() {
            "room" | "join" => {
                if let Some(room) = e.room() {
                    let stays = open.entry(who.clone()).or_default();
                    if !stays.iter().any(|s| s.room == room) {
                        stays.push(Stay {
                            room,
                            from: e.at,
                            kind: e.str("room_kind").to_string(),
                            mode: e.str("mode").to_string(),
                            private: e.detail["private"].as_bool().unwrap_or(false),
                            host: e.kind == "room",
                        });
                    }
                }
            }
            "leave" => {
                if let (Some(room), Some(stays)) = (e.room(), open.get_mut(&who)) {
                    if let Some(i) = stays.iter().position(|s| s.room == room) {
                        let stay = stays.remove(i);
                        close(&mut players, &who, stay, e.at, ep, &members, &e.name);
                    }
                }
            }
            "signout" => {
                for stay in open.remove(&who).unwrap_or_default() {
                    close(&mut players, &who, stay, e.at, ep, &members, &e.name);
                }
            }
            _ => {}
        }
        if e.last_at >= from && e.kind != "room" && e.kind != "leave" && e.kind != "join" && e.kind != "signout" {
            if let Some(p) = players.get_mut(&who) {
                p["marks"].as_array_mut().expect("an array").push(json!({
                    "at": e.at.max(from), "last_at": e.last_at, "kind": e.kind, "count": e.count, "text": describe(e),
                }));
            }
        }
    }
    // Still in a room at the end of the range.
    for (who, stays) in open {
        let ep = epoch.get(who.0.as_str()).copied().unwrap_or(0);
        let me = players.get(&who).and_then(|p| p["name"].as_str().map(str::to_string)).unwrap_or_default();
        for stay in stays {
            close(&mut players, &who, stay, to, ep, &members, &me);
        }
    }
    players
        .into_values()
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
        "nat_missing" => format!(
            "Signed in, but the game hadn't registered for online play {} s later: nobody could reach it",
            e.detail["after_secs"]
        ),
        "relay_drop" => format!(
            "Relayed traffic {} fell from {} to {} packets/s",
            if e.str("direction") == "sending" { "from them" } else { "to them" },
            e.detail["before"],
            e.detail["after"]
        ),
        "request_error" => format!("A request failed: {} {}{times}", e.str("call"), e.str("error")),
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
            // Joins with them fail (CONNECTION_FAILED) until a restart registers it.
            "nat_missing" => out.push(problem(e, "bad", format!("{}'s game couldn't be reached", e.name), describe(e))),
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
}
