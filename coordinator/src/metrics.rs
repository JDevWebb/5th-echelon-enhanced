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
/// The servers' live points and the live events.
const LIVE_FOR: i64 = 24 * 3600;
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
    pub failed_joins: f64,
    pub matches_started: f64,
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
        self.failed_joins += o.failed_joins;
        self.matches_started += o.matches_started;
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
            &mut self.failed_joins,
            &mut self.matches_started,
            &mut self.load,
        ] {
            *v *= f;
        }
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// How much a counter grew. When it went back, the server (or machine) restarted and
/// counts from zero again: all of it is new.
fn grew(now: &Value, before: &Value) -> f64 {
    let (a, b) = (num(now), num(before));
    if a >= b {
        a - b
    } else {
        a
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
            p.failed_joins = grew(&c["failed_joins"], &c0["failed_joins"]) * per_minute;
            p.matches_started = grew(&c["matches_started"], &c0["matches_started"]) * per_minute;
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
            'failed_logins', json_extract(data, '$.counters.failed_logins'), 'registrations', json_extract(data, '$.counters.registrations'),
            'failed_joins', json_extract(data, '$.counters.failed_joins'), 'matches_started', json_extract(data, '$.counters.matches_started')))
      FROM samples WHERE server_id = ? AND at >= ? ORDER BY at";

/// The hour (Unix seconds at its start) `t` falls in.
fn hour_of(t: i64) -> i64 {
    t - t.rem_euclid(3600)
}

/// The UTC day (Unix seconds at its start) `t` falls in.
pub(crate) fn day_of(t: i64) -> i64 {
    t - t.rem_euclid(86_400)
}

/// The most anonymised player ids taken from one report.
const MAX_ACTIVE: usize = 5000;

/// (year, month 1-12, day 1-31) of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Days since 1970-01-01 of a civil date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The UTC month `t` falls in: its start, and the next month's.
pub(crate) fn month_of(t: i64) -> (i64, i64) {
    let (y, m, _) = civil(t.div_euclid(86_400));
    let start = days_from_civil(y, m, 1) * 86_400;
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (start, days_from_civil(ny, nm, 1) * 86_400)
}

/// The value at quantile `q` (0..=1) of `v` (sorted here); 0 for none.
fn quantile(mut v: Vec<f64>, q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// One hour of one server's traffic, from its rollup (or the hour so far).
#[derive(Debug, Clone, Default)]
pub(crate) struct HourTraffic {
    server: String,
    pub hour: i64,
    rx: f64,
    tx: f64,
    relayed: f64,
    /// The fastest (in + out) bytes a second between two samples.
    peak: f64,
    /// (in + out) bytes a second, for each five minutes.
    rate5: Vec<f64>,
    max_players: f64,
    logins: f64,
    failed_logins: f64,
    registrations: f64,
    /// Joins that failed, and matches started (counters newer servers send).
    pub failed_joins: f64,
    pub matches_started: f64,
}

impl HourTraffic {
    /// From a rollup; hours rolled up before byte totals were kept are estimated from
    /// their average rates.
    fn from_summary(server: String, hour: i64, h: &Value) -> Self {
        let avg = &h["avg"];
        let exact = h["rx_bytes"].is_number();
        let minutes = num(&h["samples"]).max(1.0);
        let rate = num(&avg["rx"]) + num(&avg["tx"]);
        Self {
            server,
            hour,
            rx: if exact { num(&h["rx_bytes"]) } else { num(&avg["rx"]) * 3600.0 },
            tx: if exact { num(&h["tx_bytes"]) } else { num(&avg["tx"]) * 3600.0 },
            relayed: if exact { num(&h["relayed_bytes"]) } else { num(&avg["relayed"]) * 3600.0 },
            peak: if exact { num(&h["peak_bps"]) } else { rate },
            rate5: match h["rate5"].as_array() {
                Some(r) => r.iter().map(num).collect(),
                None => vec![rate; 12],
            },
            max_players: num(&h["max_players"]),
            logins: if exact { num(&h["logins"]) } else { num(&avg["logins"]) * minutes },
            failed_logins: if exact { num(&h["failed_logins"]) } else { num(&avg["failed_logins"]) * minutes },
            registrations: if exact { num(&h["registrations"]) } else { num(&avg["registrations"]) * minutes },
            failed_joins: num(&h["failed_joins"]),
            matches_started: num(&h["matches_started"]),
        }
    }
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
        self.record_active(server, now, &report["metrics"]["active"]).await?;
        self.record_matches(server, &report["metrics"]["matches"]).await
    }

    /// Counts a minute for each player online (their anonymised ids, see the server's
    /// `metrics`), and notes the day each was first seen.
    async fn record_active(&self, server: &str, now: i64, active: &Value) -> sqlx::Result<()> {
        let ids: Vec<&str> = active
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|id| id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()))
            .take(MAX_ACTIVE)
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let day = day_of(now);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for id in ids {
            sqlx::query("INSERT INTO daily_players (server_id, day, player, minutes) VALUES (?, ?, ?, 1) ON CONFLICT DO UPDATE SET minutes = minutes + 1")
                .bind(server)
                .bind(day)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT OR IGNORE INTO first_seen (server_id, player, day) VALUES (?, ?, ?)")
                .bind(server)
                .bind(id)
                .bind(day)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
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
            ("DELETE FROM daily_players WHERE day < ?", HOURLY_FOR),
            ("DELETE FROM alerts WHERE resolved_at < ?", HOURLY_FOR),
            ("DELETE FROM play_sessions WHERE COALESCE(ended, started) < ?", crate::players::KEEP_FOR),
            ("DELETE FROM matches WHERE ended < ?", crate::players::KEEP_FOR),
            ("DELETE FROM player_actions WHERE created_at < ?", crate::players::KEEP_FOR),
            ("DELETE FROM pulses WHERE at < ?", LIVE_FOR),
            ("DELETE FROM live_feed WHERE at < ?", LIVE_FOR),
        ] {
            sqlx::query(sql).bind(now - keep).execute(&self.pool).await?;
        }
        self.prune_reports().await?;
        self.prune_session_events().await?;
        self.prune_content().await?;
        self.prune_support().await?;
        self.expire_actions().await
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
        let labels = crate::game_names::labels(labels);
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

    /// Every server's traffic by the hour from `from` on (`server`: one only): the rollups,
    /// and the hour so far summed up from its samples.
    pub(crate) async fn traffic_since(&self, from: i64, server: Option<&str>) -> sqlx::Result<Vec<HourTraffic>> {
        let rows: Vec<(String, i64, String)> = sqlx::query_as("SELECT server_id, hour, data FROM hourly WHERE hour >= ? AND (?2 IS NULL OR server_id = ?2) ORDER BY hour")
            .bind(hour_of(from))
            .bind(server)
            .fetch_all(&self.pool)
            .await?;
        let mut hours: Vec<HourTraffic> = rows
            .into_iter()
            .map(|(s, hour, data)| HourTraffic::from_summary(s, hour, &serde_json::from_str(&data).unwrap_or_default()))
            .collect();
        // Hours not rolled up yet (this one, and any the hourly job hasn't reached).
        let done: HashMap<String, i64> = hours.iter().fold(HashMap::new(), |mut m, h| {
            let e = m.entry(h.server.clone()).or_insert(h.hour);
            *e = (*e).max(h.hour);
            m
        });
        let now = identity::now();
        let servers: Vec<String> = sqlx::query_scalar("SELECT DISTINCT server_id FROM samples WHERE at >= ? AND (?2 IS NULL OR server_id = ?2)")
            .bind(hour_of(now) - 3 * 3600)
            .bind(server)
            .fetch_all(&self.pool)
            .await?;
        for s in servers {
            let start = done.get(&s).map_or(hour_of(from).max(hour_of(now) - 3 * 3600), |h| h + 3600);
            let rows: Vec<(i64, String)> = sqlx::query_as(CHARTED).bind(&s).bind(start - 600).fetch_all(&self.pool).await?;
            let mut hour = start;
            while hour <= hour_of(now) {
                let inside: Vec<(i64, String)> = rows.iter().filter(|(t, _)| *t >= hour - 600 && *t < hour + 3600).cloned().collect();
                if let Some(h) = summarise(hour, &inside) {
                    hours.push(HourTraffic::from_summary(s.clone(), hour, &h));
                }
                hour += 3600;
            }
        }
        hours.retain(|h| h.hour >= hour_of(from));
        Ok(hours)
    }

    /// The bandwidth report over the last `range` seconds (`server`: one only): data in, out
    /// and relayed per period, the totals, the peak and 95th percentile, a row per day (or
    /// month, over a year), and each server's month against its allowance.
    pub async fn bandwidth(&self, range: i64, server: Option<&str>) -> sqlx::Result<Value> {
        let now = identity::now();
        let from = now - range;
        let hours = self.traffic_since(from, server).await?;
        // Where each hour goes: chart buckets, and table rows.
        let year = range > 40 * 86_400;
        let step = match range {
            r if r <= 86_400 => 3600,
            r if r <= 7 * 86_400 => 6 * 3600,
            _ => 86_400,
        };
        // Days start at midnight UTC; shorter periods count from the start of the range.
        let start = if step == 86_400 { day_of(from) } else { hour_of(from) };
        let bucket_of = |h: i64| if year { month_of(h).0 } else { start + (h - start).div_euclid(step) * step };
        let row_of = |h: i64| if year { month_of(h).0 } else { day_of(h) };
        // Every period in the range, so quiet ones still show on the chart.
        let mut buckets: BTreeMap<i64, [f64; 3]> = BTreeMap::new();
        let mut t = bucket_of(from);
        while t <= now {
            buckets.insert(t, [0.0; 3]);
            t = if year { month_of(t).1 } else { t + step };
        }
        let mut rows: BTreeMap<i64, Value> = BTreeMap::new();
        let mut by_server: BTreeMap<String, [f64; 3]> = BTreeMap::new();
        // Per hour, every server together: for peaks and the 95th percentile.
        let mut net_hours: BTreeMap<i64, (f64, f64, Vec<f64>)> = BTreeMap::new();
        let (mut rx, mut tx, mut relayed) = (0.0, 0.0, 0.0);
        let mut peak = (0.0_f64, 0_i64, String::new());
        for h in &hours {
            rx += h.rx;
            tx += h.tx;
            relayed += h.relayed;
            let b = buckets.entry(bucket_of(h.hour)).or_default();
            b[0] += h.rx;
            b[1] += h.tx;
            b[2] += h.relayed;
            let s = by_server.entry(h.server.clone()).or_default();
            s[0] += h.rx;
            s[1] += h.tx;
            s[2] += h.relayed;
            if h.peak > peak.0 {
                peak = (h.peak, h.hour, h.server.clone());
            }
            let e = net_hours.entry(h.hour).or_insert_with(|| (0.0, 0.0, vec![0.0; 12]));
            e.0 += h.peak;
            e.1 += h.max_players;
            for (i, r) in h.rate5.iter().enumerate().take(12) {
                e.2[i] += r;
            }
            let row = rows
                .entry(row_of(h.hour))
                .or_insert_with(|| json!({ "t": row_of(h.hour), "rx": 0.0, "tx": 0.0, "relayed": 0.0 }));
            for (k, v) in [("rx", h.rx), ("tx", h.tx), ("relayed", h.relayed)] {
                row[k] = json!(num(&row[k]) + v);
            }
        }
        let mut row_rates: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
        let mut all_rates = Vec::new();
        for (hour, (peak_sum, players, rates)) in &net_hours {
            let row = rows.entry(row_of(*hour)).or_default();
            row["peak_bps"] = json!(num(&row["peak_bps"]).max(*peak_sum));
            row["peak_players"] = json!(num(&row["peak_players"]).max(*players));
            let rates: Vec<f64> = rates.iter().copied().filter(|r| *r > 0.0).collect();
            row_rates.entry(row_of(*hour)).or_default().extend(&rates);
            all_rates.extend(rates);
        }
        for (t, rates) in row_rates {
            if let Some(row) = rows.get_mut(&t) {
                row["p95_bps"] = json!(quantile(rates, 0.95));
            }
        }
        // This month, per server, against its allowance.
        let (month_start, month_end) = month_of(now);
        let month = self.traffic_since(month_start, None).await?;
        let mut used: BTreeMap<String, f64> = BTreeMap::new();
        for h in month.iter().filter(|h| h.hour >= month_start) {
            *used.entry(h.server.clone()).or_default() += h.rx + h.tx;
        }
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM servers ORDER BY id").fetch_all(&self.pool).await?;
        let elapsed = ((now - month_start) as f64 / (month_end - month_start) as f64).max(1.0 / 720.0);
        let mut allowances = Vec::new();
        for id in ids {
            let tb: Option<f64> = self.setting(&format!("allowance:{id}")).await?.and_then(|v| v.parse().ok()).filter(|v: &f64| *v > 0.0);
            let u = used.get(&id).copied().unwrap_or(0.0);
            allowances.push(json!({ "server": id, "used": u, "projected": u / elapsed, "allowance": tb.map(|tb| tb * 1e12) }));
        }
        Ok(json!({
            "from": from,
            "to": now,
            "step": if year { "month" } else if step == 86_400 { "day" } else if step == 3600 { "hour" } else { "6 hours" },
            "rows_are": if year { "months" } else { "days" },
            "buckets": buckets.into_iter().map(|(t, [rx, tx, relayed])| json!({ "t": t, "rx": rx, "tx": tx, "relayed": relayed })).collect::<Vec<_>>(),
            "totals": { "rx": rx, "tx": tx, "relayed": relayed },
            "peak": { "bps": peak.0, "t": peak.1, "server": peak.2 },
            "p95_bps": quantile(all_rates, 0.95),
            "by_server": by_server.into_iter().map(|(s, [rx, tx, relayed])| json!({ "server": s, "rx": rx, "tx": tx, "relayed": relayed })).collect::<Vec<_>>(),
            "rows": rows.into_values().rev().collect::<Vec<_>>(),
            "month": { "start": month_start, "end": month_end },
            "allowances": allowances,
        }))
    }

    /// The players report over the last `range` seconds: players (counted per server), new
    /// and returning ones, time played, when they play, sign-ins, and pings by city.
    ///
    /// Players and time played each day come from play sessions for servers that send them
    /// (from the first whole day they did), else from the anonymised ids sent each minute (a
    /// minute for each sample a player was in). New and returning players come from the ids.
    pub async fn players_report(&self, range: i64) -> sqlx::Result<Value> {
        let now = identity::now();
        let from = day_of(now - range + 86_400);
        let before = from - (day_of(now) + 86_400 - from);
        let one = |sql: &'static str, a: i64, b: i64| sqlx::query_scalar::<_, i64>(sql).bind(a).bind(b).fetch_one(&self.pool);
        let earlier = one(
            "SELECT COUNT(*) FROM (SELECT DISTINCT server_id, player FROM daily_players WHERE day >= ? AND day < ?)",
            before,
            from,
        )
        .await?;
        let returning = one(
            "SELECT COUNT(*) FROM (SELECT DISTINCT a.server_id, a.player FROM daily_players a JOIN daily_players b
               ON b.server_id = a.server_id AND b.player = a.player AND b.day >= ?2 AND b.day < ?1 WHERE a.day >= ?1)",
            from,
            before,
        )
        .await?;
        let new_players = one("SELECT COUNT(*) FROM first_seen WHERE day >= ? AND day < ?", from, i64::MAX).await?;
        let since: Option<i64> = sqlx::query_scalar("SELECT MIN(day) FROM first_seen").fetch_one(&self.pool).await?;
        let sessions = self.session_days(from).await?;
        let uses_sessions = |server: &str, day: i64| sessions.since.get(server).is_some_and(|s| day >= *s);
        // Per day: players and minutes, each server from its sessions or its ids.
        let mut days: BTreeMap<i64, (i64, f64)> = BTreeMap::new();
        let ids: Vec<(String, i64, i64, i64)> = sqlx::query_as("SELECT server_id, day, COUNT(*), SUM(minutes) FROM daily_players WHERE day >= ? GROUP BY server_id, day")
            .bind(from)
            .fetch_all(&self.pool)
            .await?;
        for (server, day, players, minutes) in ids {
            if !uses_sessions(&server, day) {
                let e = days.entry(day).or_default();
                e.0 += players;
                e.1 += minutes as f64;
            }
        }
        for ((server, day), secs) in &sessions.seconds {
            if uses_sessions(server, *day) {
                let e = days.entry(*day).or_default();
                e.0 += sessions.players.get(&(server.clone(), *day)).copied().unwrap_or(0);
                e.1 += *secs as f64 / 60.0;
            }
        }
        // Players in the period, per server: by their ids or their sessions, whichever saw
        // more (the two can't be matched up, and sessions may cover only part of it).
        let by_ids: HashMap<String, i64> = sqlx::query_as("SELECT server_id, COUNT(DISTINCT player) FROM daily_players WHERE day >= ? GROUP BY server_id")
            .bind(from)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .collect();
        let mut players: i64 = by_ids.iter().filter(|(s, _)| !sessions.distinct.contains_key(*s)).map(|(_, n)| n).sum();
        for (server, seen) in &sessions.distinct {
            players += (seen.len() as i64).max(by_ids.get(server).copied().unwrap_or(0));
        }
        let new_by_day: HashMap<i64, i64> = sqlx::query_as::<_, (i64, i64)>("SELECT day, COUNT(*) FROM first_seen WHERE day >= ? GROUP BY day")
            .bind(from)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .collect();
        let player_days: i64 = days.values().map(|d| d.0).sum();
        let minutes: f64 = days.values().map(|d| d.1).sum();
        // Hour by hour (every server together): players on average, and the most at once.
        let hours = self.traffic_since(now - range, None).await?;
        let mut by_hour: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
        // Every day in the range, so quiet ones still show on the charts.
        let every_day: Vec<i64> = (0..).map(|i| from + i * 86_400).take_while(|d| *d <= day_of(now)).collect();
        let mut signins: BTreeMap<i64, [f64; 4]> = every_day.iter().map(|d| (*d, [0.0; 4])).collect();
        for h in &hours {
            let e = by_hour.entry(h.hour).or_default();
            e.1 += h.max_players;
            let s = signins.entry(day_of(h.hour)).or_default();
            s[0] += h.logins;
            s[1] += h.failed_logins;
            s[2] += h.registrations;
            s[3] += h.failed_joins;
        }
        let avg_players: Vec<(i64, f64)> = sqlx::query_as("SELECT hour, SUM(CAST(json_extract(data, '$.avg.players') AS REAL)) FROM hourly WHERE hour >= ? GROUP BY hour")
            .bind(hour_of(now - range))
            .fetch_all(&self.pool)
            .await?;
        for (hour, avg) in avg_players {
            by_hour.entry(hour).or_default().0 = avg;
        }
        let peak = by_hour.values().map(|v| v.1).fold(sessions.peak as f64, f64::max);
        // Players' pings by city and server: the median of what their launchers measured.
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as("SELECT server_id, country, city, ms FROM player_pings WHERE at >= ?")
            .bind(now - range)
            .fetch_all(&self.pool)
            .await?;
        let mut cities: BTreeMap<(String, String), BTreeMap<String, Vec<f64>>> = BTreeMap::new();
        for (server, country, city, ms) in rows {
            cities.entry((country, city)).or_default().entry(server).or_default().push(ms as f64);
        }
        let mut cities: Vec<Value> = cities
            .into_iter()
            .map(|((country, city), servers)| {
                let reports: usize = servers.values().map(Vec::len).sum();
                let pings: BTreeMap<String, f64> = servers.into_iter().map(|(s, ms)| (s, quantile(ms, 0.5))).collect();
                json!({ "country": country, "city": city, "reports": reports, "median": pings })
            })
            .collect();
        cities.sort_by(|a, b| num(&b["reports"]).total_cmp(&num(&a["reports"])));
        cities.truncate(20);
        Ok(json!({
            "from": from,
            "to": now,
            "tracking_since": since,
            "totals": {
                "players": players,
                "daily_average": if days.is_empty() { 0.0 } else { player_days as f64 / days.values().filter(|d| d.0 > 0).count().max(1) as f64 },
                "new": new_players,
                "returning": returning,
                "earlier": earlier,
                "minutes_per_player_day": if player_days > 0 { minutes / player_days as f64 } else { 0.0 },
                "peak": peak,
            },
            "days": every_day
                .iter()
                .map(|day| {
                    let (players, minutes) = days.get(day).copied().unwrap_or_default();
                    json!({ "t": day, "players": players, "minutes": minutes.round() as i64, "new": new_by_day.get(day).copied().unwrap_or(0) })
                })
                .collect::<Vec<_>>(),
            // Servers whose time played comes from play sessions, and from which day.
            "sessions_from": sessions.since,
            "hours": by_hour.into_iter().map(|(t, (avg, max))| json!({ "t": t, "avg": avg, "max": max })).collect::<Vec<_>>(),
            "signins": signins.into_iter().map(|(t, [ok, failed, new, failed_joins])| json!({ "t": t, "ok": ok, "failed": failed, "new": new, "failed_joins": failed_joins })).collect::<Vec<_>>(),
            "cities": cities,
        }))
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
    // Bytes and counts over the hour, the fastest rate, and (bytes, seconds) per five minutes.
    let (mut rx, mut tx, mut relayed, mut peak) = (0.0, 0.0, 0.0, 0.0_f64);
    let (mut logins, mut failed_logins, mut registrations) = (0.0, 0.0, 0.0);
    let (mut failed_joins, mut matches_started) = (0.0, 0.0);
    let mut slots = [(0.0_f64, 0.0_f64); 12];
    for (i, (t, m)) in samples.iter().enumerate() {
        if *t < hour {
            continue;
        }
        let prev = i.checked_sub(1).map(|j| (samples[j].0, &samples[j].1));
        if let Some((t0, before)) = prev.filter(|(t0, _)| (1..=600).contains(&(*t - *t0))) {
            let secs = (*t - t0) as f64;
            let (sys, sys0, c, c0) = (&m["system"], &before["system"], &m["counters"], &before["counters"]);
            let (r, x) = (grew(&sys["net_rx_bytes"], &sys0["net_rx_bytes"]), grew(&sys["net_tx_bytes"], &sys0["net_tx_bytes"]));
            rx += r;
            tx += x;
            relayed += grew(&c["relayed_bytes"], &c0["relayed_bytes"]);
            logins += grew(&c["game_logins"], &c0["game_logins"]);
            failed_logins += grew(&c["failed_logins"], &c0["failed_logins"]);
            registrations += grew(&c["registrations"], &c0["registrations"]);
            failed_joins += grew(&c["failed_joins"], &c0["failed_joins"]);
            matches_started += grew(&c["matches_started"], &c0["matches_started"]);
            peak = peak.max((r + x) / secs);
            let slot = usize::try_from((*t - hour) / 300).unwrap_or(0).min(11);
            slots[slot].0 += r + x;
            slots[slot].1 += secs;
        }
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
        "rx_bytes": rx,
        "tx_bytes": tx,
        "relayed_bytes": relayed,
        "peak_bps": peak,
        "rate5": slots.iter().map(|(b, s)| if *s > 0.0 { b / s } else { 0.0 }).collect::<Vec<_>>(),
        "logins": logins,
        "failed_logins": failed_logins,
        "registrations": registrations,
        "failed_joins": failed_joins,
        "matches_started": matches_started,
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
    fn months_and_days() {
        // 2026-10-03 12:00 UTC.
        let t = 1_791_028_800;
        let (start, end) = month_of(t);
        assert_eq!(civil(start / 86_400), (2026, 10, 1));
        assert_eq!(civil(end / 86_400), (2026, 11, 1));
        assert_eq!(month_of(days_from_civil(2026, 12, 31) * 86_400).1, days_from_civil(2027, 1, 1) * 86_400);
        assert_eq!(day_of(t), days_from_civil(2026, 10, 3) * 86_400);
        assert_eq!(quantile(vec![5.0, 1.0, 3.0, 2.0, 4.0], 0.5), 3.0);
    }

    #[test]
    fn hourly_rollups_keep_byte_totals() {
        let rows = vec![(3540, sample(2, 0, 0)), (3600, sample(4, 6000, 2)), (3660, sample(4, 12_000, 2))];
        let h = summarise(3600, &rows).unwrap();
        assert_eq!(h["rx_bytes"], 12_000.0);
        assert_eq!(h["logins"], 2.0);
        assert_eq!(h["peak_bps"], 100.0);
        assert_eq!(h["rate5"][0], 100.0);
        let t = HourTraffic::from_summary("a".into(), 3600, &h);
        assert_eq!(t.rx, 12_000.0);
        // An hour rolled up before byte totals: estimated from its average rate.
        let old = json!({ "samples": 60, "avg": { "rx": 10.0, "tx": 5.0, "logins": 0.5 } });
        let t = HourTraffic::from_summary("a".into(), 3600, &old);
        assert_eq!((t.rx, t.tx, t.logins, t.rate5.len()), (36_000.0, 18_000.0, 30.0, 12));
    }

    #[test]
    fn a_restart_counts_what_came_since() {
        // A counter back at 3 (the server restarted): 3 happened since, not 0.
        assert_eq!(grew(&json!(3), &json!(100)), 3.0);
        assert_eq!(grew(&json!(105), &json!(100)), 5.0);
        let rows = vec![(3540, sample(1, 0, 10)), (3600, sample(1, 0, 12)), (3660, sample(1, 0, 4))];
        let h = summarise(3600, &rows).unwrap();
        assert_eq!(h["logins"], 6.0, "2 before the restart, 4 after");
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
