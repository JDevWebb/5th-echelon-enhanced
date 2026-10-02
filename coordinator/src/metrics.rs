//! The network's metrics, for the admin UI: what each member server reports
//! every minute (`POST /v1/metrics`), rolled up per hour; the coordinator's
//! own round trips to each server; and players' pings to the servers, as
//! their launchers measured them (`POST /v1/pings`, located by city here,
//! the address not kept).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;
use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// Raw samples are kept this long; hourly rollups for [`HOURLY_FOR`].
const RAW_FOR: i64 = 8 * 24 * 3600;
const HOURLY_FOR: i64 = 400 * 24 * 3600;
const PLAYER_PINGS_FOR: i64 = 90 * 24 * 3600;
/// The most points a chart gets.
const MAX_POINTS: usize = 360;
/// The largest metrics report.
pub const MAX_REPORT: usize = 128 * 1024;

/// One sample's numbers, as charted. Rates are per second over the time
/// since the sample before; counts per minute.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Point {
    pub t: i64,
    pub players: f64,
    pub in_match: f64,
    pub cpu: f64,
    pub process_cpu: f64,
    pub mem_used: f64,
    pub mem_total: f64,
    pub rss: f64,
    pub rx: f64,
    pub tx: f64,
    pub relayed: f64,
    pub logins: f64,
    pub failed_logins: f64,
    pub registrations: f64,
    pub load: f64,
}

impl Point {
    fn add(&mut self, o: &Point) {
        self.players += o.players;
        self.in_match += o.in_match;
        self.cpu += o.cpu;
        self.process_cpu += o.process_cpu;
        self.mem_used += o.mem_used;
        self.mem_total += o.mem_total;
        self.rss += o.rss;
        self.rx += o.rx;
        self.tx += o.tx;
        self.relayed += o.relayed;
        self.logins += o.logins;
        self.failed_logins += o.failed_logins;
        self.registrations += o.registrations;
        self.load += o.load;
    }

    fn scale(&mut self, f: f64) {
        for v in [
            &mut self.players,
            &mut self.in_match,
            &mut self.cpu,
            &mut self.process_cpu,
            &mut self.mem_used,
            &mut self.mem_total,
            &mut self.rss,
            &mut self.rx,
            &mut self.tx,
            &mut self.relayed,
            &mut self.logins,
            &mut self.failed_logins,
            &mut self.registrations,
            &mut self.load,
        ] {
            *v *= f;
        }
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// How much a counter grew (0 when it went back: a restart).
fn grew(now: &Value, before: &Value) -> f64 {
    let (a, b) = (num(now), num(before));
    if a >= b {
        a - b
    } else {
        0.0
    }
}

/// The chart point for sample `m` at `t`, `prev` the sample before (for rates).
fn point(t: i64, m: &Value, prev: Option<(i64, &Value)>) -> Point {
    let s = &m["system"];
    let mut p = Point {
        t,
        players: num(&m["players"]["online"]),
        in_match: num(&m["players"]["in_match"]),
        cpu: num(&s["cpu_percent"]),
        process_cpu: num(&s["process_cpu_percent"]),
        mem_used: (num(&s["mem_total"]) - num(&s["mem_available"])).max(0.0),
        mem_total: num(&s["mem_total"]),
        rss: num(&s["process_rss"]),
        load: num(&s["load"][0]),
        ..Point::default()
    };
    if let Some((t0, before)) = prev {
        let secs = (t - t0) as f64;
        if secs > 0.0 && secs <= 600.0 {
            let (s0, c, c0) = (&before["system"], &m["counters"], &before["counters"]);
            p.rx = grew(&s["net_rx_bytes"], &s0["net_rx_bytes"]) / secs;
            p.tx = grew(&s["net_tx_bytes"], &s0["net_tx_bytes"]) / secs;
            p.relayed = grew(&c["relayed_bytes"], &c0["relayed_bytes"]) / secs;
            let per_minute = 60.0 / secs;
            p.logins = grew(&c["game_logins"], &c0["game_logins"]) * per_minute;
            p.failed_logins = grew(&c["failed_logins"], &c0["failed_logins"]) * per_minute;
            p.registrations = grew(&c["registrations"], &c0["registrations"]) * per_minute;
        }
    }
    p
}

/// Averages `points` into at most [`MAX_POINTS`] buckets of `step` seconds.
fn bucket(points: Vec<Point>, from: i64, step: i64) -> Vec<Point> {
    let mut buckets: BTreeMap<i64, (Point, f64)> = BTreeMap::new();
    for p in points {
        let key = from + (p.t - from).div_euclid(step) * step;
        let e = buckets.entry(key).or_insert_with(|| (Point { t: key, ..Point::default() }, 0.0));
        e.0.add(&p);
        e.1 += 1.0;
    }
    buckets
        .into_values()
        .map(|(mut p, n)| {
            let t = p.t;
            p.scale(1.0 / n);
            p.t = t;
            p
        })
        .collect()
}

/// A server's samples since a time, cut down in SQL to the numbers [`point`] reads.
const CHARTED: &str = "SELECT at, json_object(
        'players', json_object('online', json_extract(data, '$.players.online'), 'in_match', json_extract(data, '$.players.in_match')),
        'system', json_object(
            'cpu_percent', json_extract(data, '$.system.cpu_percent'), 'process_cpu_percent', json_extract(data, '$.system.process_cpu_percent'),
            'mem_total', json_extract(data, '$.system.mem_total'), 'mem_available', json_extract(data, '$.system.mem_available'),
            'process_rss', json_extract(data, '$.system.process_rss'), 'load', json_array(json_extract(data, '$.system.load[0]')),
            'net_rx_bytes', json_extract(data, '$.system.net_rx_bytes'), 'net_tx_bytes', json_extract(data, '$.system.net_tx_bytes')),
        'counters', json_object(
            'relayed_bytes', json_extract(data, '$.counters.relayed_bytes'), 'game_logins', json_extract(data, '$.counters.game_logins'),
            'failed_logins', json_extract(data, '$.counters.failed_logins'), 'registrations', json_extract(data, '$.counters.registrations')))
      FROM samples WHERE server_id = ? AND at >= ? ORDER BY at";

/// The hour (Unix seconds at its start) `t` falls in.
fn hour_of(t: i64) -> i64 {
    t - t.rem_euclid(3600)
}

impl Coordinator {
    /// Stores a server's metrics report.
    pub(crate) async fn record_metrics(&self, server: &str, report: &Value) -> sqlx::Result<()> {
        let now = identity::now();
        sqlx::query("INSERT OR REPLACE INTO samples (server_id, at, data) VALUES (?, ?, ?)")
            .bind(server)
            .bind(now)
            .bind(report["metrics"].to_string())
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE servers SET update_status = ? WHERE id = ?")
            .bind(report["update"].to_string())
            .bind(server)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Rolls finished hours up, and drops what's past keeping. Run hourly.
    pub async fn roll_up(&self) -> sqlx::Result<()> {
        let now = identity::now();
        let this_hour = hour_of(now);
        let done: Vec<(String, i64)> = sqlx::query_as("SELECT server_id, MAX(hour) FROM hourly GROUP BY server_id").fetch_all(&self.pool).await?;
        let done: HashMap<String, i64> = done.into_iter().collect();
        let servers: Vec<(String, i64)> = sqlx::query_as("SELECT server_id, MIN(at) FROM samples GROUP BY server_id").fetch_all(&self.pool).await?;
        for (server, first) in servers {
            let start = done.get(&server).map_or(hour_of(first), |h| h + 3600);
            let mut hour = start;
            while hour < this_hour {
                let rows: Vec<(i64, String)> = sqlx::query_as("SELECT at, data FROM samples WHERE server_id = ? AND at >= ? AND at < ? ORDER BY at")
                    .bind(&server)
                    .bind(hour - 600)
                    .bind(hour + 3600)
                    .fetch_all(&self.pool)
                    .await?;
                if let Some(summary) = summarise(hour, &rows) {
                    sqlx::query("INSERT OR REPLACE INTO hourly (server_id, hour, data) VALUES (?, ?, ?)")
                        .bind(&server)
                        .bind(hour)
                        .bind(summary.to_string())
                        .execute(&self.pool)
                        .await?;
                }
                hour += 3600;
            }
        }
        for (sql, keep) in [
            ("DELETE FROM samples WHERE at < ?", RAW_FOR),
            ("DELETE FROM server_pings WHERE at < ?", RAW_FOR),
            ("DELETE FROM hourly WHERE hour < ?", HOURLY_FOR),
            ("DELETE FROM player_pings WHERE at < ?", PLAYER_PINGS_FOR),
            ("DELETE FROM audit WHERE at < ?", HOURLY_FOR),
        ] {
            sqlx::query(sql).bind(now - keep).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// Measures the round trip to each listed server (a TCP connection to
    /// its API port). Run every minute.
    pub async fn ping_servers(&self) -> sqlx::Result<()> {
        let since = identity::now() - 180;
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, listing FROM servers WHERE last_seen >= ? AND listing IS NOT NULL")
            .bind(since)
            .fetch_all(&self.pool)
            .await?;
        let mut tasks = Vec::new();
        for (id, listing) in rows {
            let l: Value = serde_json::from_str(&listing).unwrap_or_default();
            let host = l["host"].as_str().unwrap_or_default().to_string();
            let port = l["ports"]["api_tls"]
                .as_u64()
                .or_else(|| l["ports"]["api"].as_u64())
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(80);
            if host.is_empty() {
                continue;
            }
            tasks.push(tokio::spawn(async move {
                // Only to a public address: a member can't have the coordinator knock on its
                // own network, or this machine's.
                let resolved = tokio::time::timeout(Duration::from_secs(3), tokio::net::lookup_host((host.as_str(), port))).await;
                let Some(target) = resolved.ok().and_then(Result::ok).and_then(|mut a| a.find(|a| crate::public_ip(a.ip()))) else {
                    return (id, None);
                };
                let start = Instant::now();
                let ms = match tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(target)).await {
                    Ok(Ok(_)) => Some(start.elapsed().as_secs_f64() * 1000.0),
                    _ => None,
                };
                (id, ms)
            }));
        }
        let at = identity::now();
        for task in tasks {
            if let Ok((id, ms)) = task.await {
                sqlx::query("INSERT OR REPLACE INTO server_pings (server_id, at, ms) VALUES (?, ?, ?)")
                    .bind(id)
                    .bind(at)
                    .bind(ms)
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// Records a launcher's pings to the servers, from `ip` (located, then
    /// forgotten).
    pub(crate) async fn record_player_pings(&self, ip: std::net::IpAddr, pings: &[Value]) -> sqlx::Result<usize> {
        let place = self.geo.get().and_then(|g| g.lookup(ip)).unwrap_or_default();
        let now = identity::now();
        let mut n = 0;
        for p in pings.iter().take(32) {
            let (Some(server), Some(ms)) = (p["server"].as_str(), p["ms"].as_u64()) else { continue };
            if !(1..=5000).contains(&ms) {
                continue;
            }
            let done = sqlx::query("INSERT INTO player_pings (server_id, at, country, city, ms) SELECT id, ?, ?, ?, ? FROM servers WHERE id = ?")
                .bind(now)
                .bind(&place.country)
                .bind(&place.city)
                .bind(i64::try_from(ms).unwrap_or(0))
                .bind(server)
                .execute(&self.pool)
                .await?;
            n += usize::try_from(done.rows_affected()).unwrap_or(0);
        }
        Ok(n)
    }

    /// Chart points for each server over the last `range` seconds, and the
    /// coordinator's pings to them.
    pub async fn series(&self, range: i64) -> sqlx::Result<Value> {
        let now = identity::now();
        let from = now - range;
        let step = (range / MAX_POINTS as i64).max(60);
        let mut by_server: BTreeMap<String, Vec<Point>> = BTreeMap::new();
        if range <= RAW_FOR {
            // A server at a time, and only the numbers charted (a report can be 128 KB, and
            // there are a week of them).
            let servers: Vec<String> = sqlx::query_scalar("SELECT DISTINCT server_id FROM samples WHERE at >= ?")
                .bind(from - 600)
                .fetch_all(&self.pool)
                .await?;
            for server in servers {
                let rows: Vec<(i64, String)> = sqlx::query_as(CHARTED).bind(&server).bind(from - 600).fetch_all(&self.pool).await?;
                let mut prev: Option<(i64, Value)> = None;
                let mut points = Vec::new();
                for (at, data) in rows {
                    let m: Value = serde_json::from_str(&data).unwrap_or_default();
                    let p = point(at, &m, prev.as_ref().map(|(t, v)| (*t, v)));
                    if at >= from {
                        points.push(p);
                    }
                    prev = Some((at, m));
                }
                by_server.insert(server, points);
            }
        } else {
            let rows: Vec<(String, i64, String)> = sqlx::query_as("SELECT server_id, hour, data FROM hourly WHERE hour >= ? ORDER BY server_id, hour")
                .bind(from)
                .fetch_all(&self.pool)
                .await?;
            for (server, hour, data) in rows {
                let h: Value = serde_json::from_str(&data).unwrap_or_default();
                let mut p: Point = serde_json::from_value(h["avg"].clone()).unwrap_or_default();
                p.t = hour;
                by_server.entry(server).or_default().push(p);
            }
        }
        let points: BTreeMap<String, Vec<Point>> = by_server.into_iter().map(|(s, pts)| (s, bucket(pts, from, step))).collect();
        let pings: Vec<(String, i64, Option<f64>)> = sqlx::query_as("SELECT server_id, at, ms FROM server_pings WHERE at >= ? ORDER BY at")
            .bind(from)
            .fetch_all(&self.pool)
            .await?;
        let mut ping_buckets: BTreeMap<String, BTreeMap<i64, (f64, f64, u32)>> = BTreeMap::new();
        for (server, at, ms) in pings {
            let key = from + (at - from).div_euclid(step) * step;
            let e = ping_buckets.entry(server).or_default().entry(key).or_insert((0.0, 0.0, 0));
            match ms {
                Some(ms) => {
                    e.0 += ms;
                    e.1 += 1.0;
                }
                None => e.2 += 1,
            }
        }
        let pings: BTreeMap<String, Vec<Value>> = ping_buckets
            .into_iter()
            .map(|(s, b)| {
                let list = b
                    .into_iter()
                    .map(|(t, (sum, n, lost))| json!({ "t": t, "ms": if n > 0.0 { Some(sum / n) } else { None }, "lost": lost }))
                    .collect();
                (s, list)
            })
            .collect();
        Ok(json!({ "from": from, "to": now, "step": step, "points": points, "pings": pings }))
    }

    /// Players per city: online now (range 0), or player-minutes over the
    /// last `range` seconds. Each with its servers.
    pub async fn places(&self, range: i64) -> sqlx::Result<Value> {
        let mut cities: BTreeMap<(String, String, String), Value> = BTreeMap::new();
        let mut add = |server: &str, p: &Value, amount: f64| {
            let key = (
                p["country"].as_str().unwrap_or_default().to_string(),
                p["region"].as_str().unwrap_or_default().to_string(),
                p["city"].as_str().unwrap_or_default().to_string(),
            );
            let e = cities.entry(key).or_insert_with(|| {
                json!({
                    "country": p["country"], "country_name": p["country_name"], "region": p["region"], "city": p["city"],
                    "lat": p["lat"], "lon": p["lon"], "amount": 0.0, "servers": {}
                })
            });
            e["amount"] = json!(num(&e["amount"]) + amount);
            let by = num(&e["servers"][server]);
            e["servers"][server] = json!(by + amount);
        };
        if range == 0 {
            for (server, m) in self.latest_metrics().await? {
                for p in m["places"].as_array().into_iter().flatten() {
                    add(&server, p, num(&p["players"]));
                }
            }
        } else {
            let rows: Vec<(String, String)> = sqlx::query_as("SELECT server_id, data FROM hourly WHERE hour >= ?")
                .bind(identity::now() - range)
                .fetch_all(&self.pool)
                .await?;
            for (server, data) in rows {
                let h: Value = serde_json::from_str(&data).unwrap_or_default();
                for p in h["places"].as_array().into_iter().flatten() {
                    add(&server, p, num(&p["player_minutes"]));
                }
            }
        }
        let mut list: Vec<Value> = cities.into_values().collect();
        list.sort_by(|a, b| num(&b["amount"]).total_cmp(&num(&a["amount"])));
        Ok(json!({ "unit": if range == 0 { "players" } else { "player-minutes" }, "places": list, "attribution": geo::ATTRIBUTION }))
    }

    /// What's being played: now (range 0), or player-minutes over the last
    /// `range` seconds, per mode, room, map and game mode.
    pub async fn activity(&self, range: i64) -> sqlx::Result<Value> {
        let mut by: BTreeMap<String, Value> = BTreeMap::new();
        let mut add = |a: &Value, players: f64, sessions: f64| {
            let key = format!("{}|{}|{}|{}", a["mode"], a["room"], a["map"], a["game_mode"]);
            let e = by
                .entry(key)
                .or_insert_with(|| json!({ "mode": a["mode"], "room": a["room"], "map": a["map"], "game_mode": a["game_mode"], "players": 0.0, "sessions": 0.0 }));
            e["players"] = json!(num(&e["players"]) + players);
            e["sessions"] = json!(num(&e["sessions"]) + sessions);
        };
        if range == 0 {
            for (_, m) in self.latest_metrics().await? {
                for a in m["activity"].as_array().into_iter().flatten() {
                    add(a, num(&a["players"]), num(&a["sessions"]));
                }
            }
        } else {
            let rows: Vec<(String,)> = sqlx::query_as("SELECT data FROM hourly WHERE hour >= ?")
                .bind(identity::now() - range)
                .fetch_all(&self.pool)
                .await?;
            for (data,) in rows {
                let h: Value = serde_json::from_str(&data).unwrap_or_default();
                for a in h["activity"].as_array().into_iter().flatten() {
                    add(a, num(&a["player_minutes"]), num(&a["session_minutes"]));
                }
            }
        }
        let mut list: Vec<Value> = by.into_values().collect();
        list.sort_by(|a, b| num(&b["players"]).total_cmp(&num(&a["players"])));
        let labels: Vec<(String, i64, String)> = sqlx::query_as("SELECT kind, id, name FROM labels").fetch_all(&self.pool).await?;
        let labels: Vec<Value> = labels.into_iter().map(|(kind, id, name)| json!({ "kind": kind, "id": id, "name": name })).collect();
        Ok(json!({ "unit": if range == 0 { "now" } else { "minutes" }, "activity": list, "labels": labels }))
    }

    /// Players' pings to each server over the last `range` seconds, by
    /// country: the median and how many reports.
    pub async fn player_pings(&self, range: i64) -> sqlx::Result<Value> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as("SELECT server_id, country, ms FROM player_pings WHERE at >= ?")
            .bind(identity::now() - range)
            .fetch_all(&self.pool)
            .await?;
        let mut groups: BTreeMap<(String, String), Vec<i64>> = BTreeMap::new();
        for (server, country, ms) in rows {
            groups.entry((server, country)).or_default().push(ms);
        }
        let list: Vec<Value> = groups
            .into_iter()
            .map(|((server, country), mut ms)| {
                ms.sort_unstable();
                let pct = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
                json!({ "server": server, "country": country, "reports": ms.len(), "median": pct(0.5), "p90": pct(0.9) })
            })
            .collect();
        Ok(json!({ "pings": list }))
    }

    /// Each server's latest metrics, from the last few minutes.
    pub async fn latest_metrics(&self) -> sqlx::Result<Vec<(String, Value)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT s.server_id, s.data FROM samples s
               JOIN (SELECT server_id, MAX(at) AS at FROM samples WHERE at >= ? GROUP BY server_id) l
                 ON l.server_id = s.server_id AND l.at = s.at",
        )
        .bind(identity::now() - 300)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(s, d)| (s, serde_json::from_str(&d).unwrap_or_default())).collect())
    }

    /// Names for map and mode ids.
    pub async fn set_label(&self, kind: &str, id: i64, name: &str) -> sqlx::Result<()> {
        if name.trim().is_empty() {
            sqlx::query("DELETE FROM labels WHERE kind = ? AND id = ?").bind(kind).bind(id).execute(&self.pool).await?;
        } else {
            sqlx::query("INSERT OR REPLACE INTO labels (kind, id, name) VALUES (?, ?, ?)")
                .bind(kind)
                .bind(id)
                .bind(name.trim())
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
}

/// One hour of a server's samples (`rows`, from shortly before the hour, so
/// the first rate in it has a sample before): averages for the charts, and
/// player-minutes per city and per activity.
fn summarise(hour: i64, rows: &[(i64, String)]) -> Option<Value> {
    let samples: Vec<(i64, Value)> = rows.iter().map(|(t, d)| (*t, serde_json::from_str(d).unwrap_or_default())).collect();
    let mut points = Vec::new();
    let mut places: BTreeMap<String, Value> = BTreeMap::new();
    let mut activity: BTreeMap<String, Value> = BTreeMap::new();
    let mut max_players: f64 = 0.0;
    let mut version = String::new();
    for (i, (t, m)) in samples.iter().enumerate() {
        if *t < hour {
            continue;
        }
        let prev = i.checked_sub(1).map(|j| (samples[j].0, &samples[j].1));
        // Each sample stands for the minutes since the one before (a minute at most... or five, after a gap).
        let minutes = prev.map_or(1.0, |(t0, _)| ((t - t0) as f64 / 60.0).clamp(0.0, 5.0));
        let p = point(*t, m, prev);
        max_players = max_players.max(p.players);
        points.push(p);
        version = m["version"].as_str().unwrap_or_default().to_string();
        for pl in m["places"].as_array().into_iter().flatten() {
            let key = format!("{}|{}|{}", pl["country"], pl["region"], pl["city"]);
            let e = places.entry(key).or_insert_with(|| {
                json!({ "country": pl["country"], "country_name": pl["country_name"], "region": pl["region"], "city": pl["city"],
                        "lat": pl["lat"], "lon": pl["lon"], "player_minutes": 0.0 })
            });
            e["player_minutes"] = json!(num(&e["player_minutes"]) + num(&pl["players"]) * minutes);
        }
        for a in m["activity"].as_array().into_iter().flatten() {
            let key = format!("{}|{}|{}|{}", a["mode"], a["room"], a["map"], a["game_mode"]);
            let e = activity
                .entry(key)
                .or_insert_with(|| json!({ "mode": a["mode"], "room": a["room"], "map": a["map"], "game_mode": a["game_mode"], "player_minutes": 0.0, "session_minutes": 0.0 }));
            e["player_minutes"] = json!(num(&e["player_minutes"]) + num(&a["players"]) * minutes);
            e["session_minutes"] = json!(num(&e["session_minutes"]) + num(&a["sessions"]) * minutes);
        }
    }
    if points.is_empty() {
        return None;
    }
    let n = points.len();
    let avg = bucket(points, hour, 3600).pop()?;
    Some(json!({
        "samples": n,
        "avg": avg,
        "max_players": max_players,
        "version": version,
        "places": places.into_values().collect::<Vec<_>>(),
        "activity": activity.into_values().collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(players: u64, rx: u64, logins: u64) -> String {
        json!({
            "version": "1.0.0",
            "players": { "online": players, "in_match": 0 },
            "places": [{ "country": "NZ", "country_name": "New Zealand", "region": "", "city": "Auckland", "lat": -36.8, "lon": 174.7, "players": players }],
            "activity": [{ "mode": "svm", "room": "match", "map": 3, "game_mode": 1, "sessions": 1, "players": players }],
            "counters": { "game_logins": logins },
            "system": { "net_rx_bytes": rx, "mem_total": 100, "mem_available": 40 },
        })
        .to_string()
    }

    #[test]
    fn rates_and_rollups() {
        let rows = vec![(3540, sample(2, 0, 0)), (3600, sample(4, 6000, 2)), (3660, sample(4, 12_000, 2)), (3720, sample(0, 0, 3))];
        let h = summarise(3600, &rows).unwrap();
        assert_eq!(h["samples"], 3, "the sample before the hour only gives the first rate");
        assert_eq!(h["max_players"], 4.0);
        // rx: 100/s, 100/s, then a restart (counter back to 0): 0.
        assert!((num(&h["avg"]["rx"]) - 200.0 / 3.0).abs() < 1e-9);
        assert!((num(&h["avg"]["mem_used"]) - 60.0).abs() < 1e-9);
        // 4 + 4 + 0 players, a minute each.
        assert_eq!(h["places"][0]["player_minutes"], 8.0);
        assert_eq!(h["activity"][0]["session_minutes"], 3.0);
        assert!(summarise(7200, &rows).is_none());
    }

    #[test]
    fn buckets_average() {
        let pts = (0..10)
            .map(|i| Point {
                t: i * 60,
                players: f64::from(i as u8),
                ..Point::default()
            })
            .collect();
        let b = bucket(pts, 0, 300);
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].t, b[0].players), (0, 2.0));
        assert_eq!((b[1].t, b[1].players), (300, 7.0));
    }
}
