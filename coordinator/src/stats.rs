//! Stats and leaderboards across the network: one entry per person (their global id), however
//! many servers they play on.
//!
//! * `POST /v1/stats` (a member) `{ epoch, writes: [{ id, global_id, name, board, context,
//!   stat, value }] }`: the stat writes the game sent its server, in order. Each server numbers
//!   them (ids ascending, per `epoch`, a random 16 hex digits per server database); a write is
//!   applied once, added up the way its board says ([`stat_boards`]), and the answer's
//!   `last_id` tells the server how far it may forget. Invalid writes, and writes for people
//!   not linked on that server, are skipped (and counted as done). Up to [`MAX_WRITES`] a
//!   request.
//! * `GET /v1/leaderboards?count=` (a member): the top of every leaderboard in every context,
//!   with each person's stats on that board (the servers answer the game from it). Made at
//!   most once a minute, and again after stats changed.
//! * `POST /v1/leaderboards/players` `{ ids }` (a member): where those people are on every
//!   list, and every list's size.
//! * `POST /v1/stats/players` `{ ids }` (a member): everything kept for those people.
//!
//! Ratio stats are worked out when read, never stored. The stats are kept for good (an admin
//! may remove a person's, see the admin UI's Leaderboards).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde_json::json;
use serde_json::Value;
use sqlx::Row as _;
use stat_boards::Aggregation;
use stat_boards::Leaderboard;

use crate::Coordinator;

/// The most writes one request may carry, and people one lookup may ask about.
pub const MAX_WRITES: usize = 1000;
pub const MAX_IDS: usize = 200;
/// The most places of a leaderboard answered.
pub const MAX_COUNT: usize = 100;
/// The largest stats request (1000 writes with long names fit).
pub const MAX_BODY: usize = 1024 * 1024;
/// The largest value a write may carry.
const LARGEST: f64 = 1e12;
/// How long the leaderboards answer is kept.
const CACHE_FOR: Duration = Duration::from_secs(60);
/// ...and at most this long once new stats came.
const STALE_FOR: Duration = Duration::from_secs(10);

/// Whether `s` may be a person's global id here: 1 to 128 letters and digits.
pub fn valid_global_id(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Whether `s` is a server's stats epoch: 16 hex digits.
pub fn valid_epoch(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A name to show for a person: 1 to 64 printable characters.
fn valid_display_name(s: &str) -> bool {
    !s.is_empty() && crate::valid_text(s, 64)
}

#[derive(Deserialize)]
struct Write {
    global_id: String,
    #[serde(default)]
    name: String,
    board: u32,
    context: u32,
    stat: u32,
    value: f64,
}

/// A write, if it's one to apply: a known board, context and stat that isn't a ratio, a
/// sensible value, a person's id.
fn checked(w: &Value) -> Option<(Write, Aggregation)> {
    let w: Write = serde_json::from_value(w.clone()).ok()?;
    let board = stat_boards::board(w.board).filter(|b| b.has_context(w.context))?;
    let aggregation = board.aggregation(w.stat).filter(|a| !matches!(a, Aggregation::Ratio(..)))?;
    (w.value.is_finite() && w.value.abs() <= LARGEST && valid_global_id(&w.global_id)).then_some((w, aggregation))
}

/// One person's place on a list.
struct Ranked {
    global_id: String,
    name: String,
    rank: i64,
    value: f64,
}

/// The rows a leaderboard ranks, as SQL conditions on `s` (`?1` board, `?2` stat): those with
/// its stat and, for the lower-is-better ones (best times), a time that was set (more than
/// 0 and less than the "not yet" value).
fn ranked_rows(l: &Leaderboard) -> String {
    let unset = stat_boards::default_value(l.stat);
    let mut sql = String::from("s.board = ?1 AND s.stat = ?2");
    if !l.descending {
        sql.push_str(" AND s.value > 0");
        if unset != 0.0 {
            sql.push_str(&format!(" AND s.value < {unset}"));
        }
    }
    sql
}

/// Everyone a leaderboard ranks in context `?3`, best first: between equal values, whoever
/// got there first.
fn ranking(l: &Leaderboard) -> String {
    let order = if l.descending { "DESC" } else { "ASC" };
    format!(
        "SELECT s.global_id AS global_id, COALESCE(n.name, '') AS name, s.value AS value,
                ROW_NUMBER() OVER (ORDER BY s.value {order}, s.updated_at, s.global_id) AS rank
           FROM global_stats s LEFT JOIN global_names n ON n.global_id = s.global_id
          WHERE {} AND s.context = ?3",
        ranked_rows(l)
    )
}

/// `?first, ?first+1, …`: `n` numbered parameters.
fn params_from(first: usize, n: usize) -> String {
    (first..first + n).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ")
}

/// `ids` checked, without repeats.
fn checked_ids(ids: &[Value]) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.iter()
        .filter_map(Value::as_str)
        .filter(|id| valid_global_id(id) && seen.insert(id.to_string()))
        .map(str::to_string)
        .collect()
}

/// The leaderboards answer, kept a minute (see [`Coordinator::leaderboards`]).
#[derive(Default)]
pub(crate) struct Cache {
    made: Option<(Instant, Value)>,
    /// New stats came since it was made.
    stale: bool,
}

impl Coordinator {
    /// Applies a server's stat writes of `epoch` that come after the last applied (all in one
    /// transaction), and answers the last applied id now.
    pub async fn apply_stats(&self, server: &str, epoch: &str, writes: &[Value]) -> sqlx::Result<i64> {
        let now = identity::now();
        let mut tx = self.pool.begin().await?;
        // A write first, so the transaction holds the database's write lock from the start.
        sqlx::query("INSERT INTO stat_sequences (server_id, epoch, last_id, updated_at) VALUES (?, ?, 0, ?) ON CONFLICT DO NOTHING")
            .bind(server)
            .bind(epoch)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let mut last: i64 = sqlx::query_scalar("SELECT last_id FROM stat_sequences WHERE server_id = ? AND epoch = ?")
            .bind(server)
            .bind(epoch)
            .fetch_one(&mut *tx)
            .await?;
        let mut writes: Vec<(i64, &Value)> = writes.iter().filter_map(|w| Some((w["id"].as_i64()?, w))).collect();
        writes.sort_by_key(|(id, _)| *id);
        let mut names: BTreeMap<String, String> = BTreeMap::new();
        // Only for people linked on the server that sends them: a server can't make up
        // anyone else's stats.
        let mut linked: BTreeMap<String, bool> = BTreeMap::new();
        let mut changed = false;
        for (id, w) in writes {
            if id <= last {
                continue;
            }
            last = id;
            let Some((w, aggregation)) = checked(w) else {
                continue;
            };
            if !linked.contains_key(&w.global_id) {
                let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM links WHERE global_id = ? AND server_id = ?")
                    .bind(&w.global_id)
                    .bind(server)
                    .fetch_one(&mut *tx)
                    .await?;
                linked.insert(w.global_id.clone(), n > 0);
            }
            if !linked[&w.global_id] {
                continue;
            }
            let stored: Option<f64> = sqlx::query_scalar("SELECT value FROM global_stats WHERE global_id = ? AND board = ? AND context = ? AND stat = ?")
                .bind(&w.global_id)
                .bind(w.board)
                .bind(w.context)
                .bind(w.stat)
                .fetch_optional(&mut *tx)
                .await?;
            let value = stat_boards::aggregate(aggregation, stored, w.value);
            if value.is_finite() && stored != Some(value) {
                changed = true;
                sqlx::query(
                    "INSERT INTO global_stats (global_id, board, context, stat, value, updated_at) VALUES (?, ?, ?, ?, ?, ?)
                     ON CONFLICT (global_id, board, context, stat) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                )
                .bind(&w.global_id)
                .bind(w.board)
                .bind(w.context)
                .bind(w.stat)
                .bind(value)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
            let name = w.name.trim();
            if valid_display_name(name) {
                names.insert(w.global_id, name.to_string());
            }
        }
        for (global_id, name) in names {
            sqlx::query(
                "INSERT INTO global_names (global_id, name, updated_at) VALUES (?, ?, ?)
                 ON CONFLICT (global_id) DO UPDATE SET name = excluded.name, updated_at = excluded.updated_at WHERE name != excluded.name",
            )
            .bind(global_id)
            .bind(name)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE stat_sequences SET last_id = ?, updated_at = ? WHERE server_id = ? AND epoch = ?")
            .bind(last)
            .bind(now)
            .bind(server)
            .bind(epoch)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        // New figures: the leaderboards are made again soon, but not for every write while
        // people play.
        if changed {
            self.stats_cache.lock().await.stale = true;
        }
        Ok(last)
    }

    /// The top `count` of a leaderboard in `context`.
    async fn top(&self, l: &Leaderboard, context: u32, count: usize) -> sqlx::Result<Vec<Ranked>> {
        let rows = sqlx::query(&format!("{} ORDER BY rank LIMIT ?4", ranking(l)))
            .bind(l.board)
            .bind(l.stat)
            .bind(context)
            .bind(count as i64)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(ranked).collect())
    }

    /// How many a leaderboard ranks, per context (the empty ones left out).
    async fn totals(&self, l: &Leaderboard) -> sqlx::Result<BTreeMap<u32, i64>> {
        let rows: Vec<(u32, i64)> = sqlx::query_as(&format!("SELECT s.context, COUNT(*) FROM global_stats s WHERE {} GROUP BY s.context", ranked_rows(l)))
            .bind(l.board)
            .bind(l.stat)
            .fetch_all(&self.pool)
            .await?;
        let board = stat_boards::board(l.board);
        Ok(rows.into_iter().filter(|(c, _)| board.is_some_and(|b| b.has_context(*c))).collect())
    }

    /// The stats `ids` have on `board` in `context` (ratios aren't stored), per person.
    async fn board_stats(&self, board: u32, context: u32, ids: &[String]) -> sqlx::Result<HashMap<String, Vec<Value>>> {
        let mut out: HashMap<String, Vec<Value>> = HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let sql = format!(
            "SELECT global_id, stat, value FROM global_stats WHERE board = ?1 AND context = ?2 AND global_id IN ({}) ORDER BY global_id, stat",
            params_from(3, ids.len())
        );
        let mut q = sqlx::query_as::<_, (String, u32, f64)>(&sql).bind(board).bind(context);
        for id in ids {
            q = q.bind(id);
        }
        for (id, stat, value) in q.fetch_all(&self.pool).await? {
            out.entry(id).or_default().push(json!([stat, value]));
        }
        Ok(out)
    }

    /// One list: a leaderboard's top `count` in `context` with their stats on its board.
    async fn list(&self, l: &Leaderboard, context: u32, total: i64, count: usize) -> sqlx::Result<Value> {
        let top = self.top(l, context, count).await?;
        let ids: Vec<String> = top.iter().map(|r| r.global_id.clone()).collect();
        let mut stats = self.board_stats(l.board, context, &ids).await?;
        let top: Vec<Value> = top
            .into_iter()
            .map(|r| {
                let s = stats.remove(&r.global_id).unwrap_or_default();
                json!({ "global_id": r.global_id, "name": r.name, "rank": r.rank, "value": r.value, "stats": s })
            })
            .collect();
        Ok(json!({ "leaderboard": l.id, "context": context, "total": total, "top": top }))
    }

    /// Every leaderboard's top `count` in every context, the empty ones left out. Made at most
    /// once a minute.
    pub async fn leaderboards(&self, count: usize) -> sqlx::Result<Value> {
        let count = count.clamp(1, MAX_COUNT);
        let mut cache = self.stats_cache.lock().await;
        let keep_for = if cache.stale { STALE_FOR } else { CACHE_FOR };
        let fresh = cache.made.as_ref().filter(|(at, _)| at.elapsed() < keep_for).map(|(_, v)| v.clone());
        let all = match fresh {
            Some(v) => v,
            None => {
                let mut lists = Vec::new();
                for l in stat_boards::LEADERBOARDS {
                    for (context, total) in self.totals(l).await? {
                        lists.push(self.list(l, context, total, MAX_COUNT).await?);
                    }
                }
                let v = json!({ "lists": lists });
                cache.made = Some((Instant::now(), v.clone()));
                cache.stale = false;
                v
            }
        };
        drop(cache);
        let mut v = all;
        if count < MAX_COUNT {
            for list in v["lists"].as_array_mut().into_iter().flatten() {
                if let Some(top) = list["top"].as_array_mut() {
                    top.truncate(count);
                }
            }
        }
        Ok(v)
    }

    /// Forgets the leaderboards answer (after an admin removed someone's stats).
    pub(crate) async fn forget_leaderboards(&self) {
        self.stats_cache.lock().await.made = None;
    }

    /// Where `ids` are on every list they're on, with their stats on its board, and how many
    /// every list that isn't empty ranks.
    pub async fn player_ranks(&self, ids: &[Value]) -> sqlx::Result<Value> {
        let ids = checked_ids(ids);
        let mut ranks = Vec::new();
        let mut totals = Vec::new();
        // Which boards, contexts and stats they have at all: only those lists are ranked.
        let mut held: HashSet<(u32, u32, u32)> = HashSet::new();
        let mut stats: HashMap<(String, u32, u32), Vec<Value>> = HashMap::new();
        if !ids.is_empty() {
            let sql = format!(
                "SELECT global_id, board, context, stat, value FROM global_stats WHERE global_id IN ({}) ORDER BY global_id, board, context, stat",
                params_from(1, ids.len())
            );
            let mut q = sqlx::query_as::<_, (String, u32, u32, u32, f64)>(&sql);
            for id in &ids {
                q = q.bind(id);
            }
            for (id, board, context, stat, value) in q.fetch_all(&self.pool).await? {
                if stat_boards::LEADERBOARDS.iter().any(|l| l.board == board) {
                    held.insert((board, context, stat));
                    stats.entry((id, board, context)).or_default().push(json!([stat, value]));
                }
            }
        }
        for l in stat_boards::LEADERBOARDS {
            for (context, total) in self.totals(l).await? {
                totals.push(json!({ "leaderboard": l.id, "context": context, "total": total }));
                if !held.contains(&(l.board, context, l.stat)) {
                    continue;
                }
                let sql = format!("SELECT * FROM ({}) WHERE global_id IN ({}) ORDER BY rank", ranking(l), params_from(4, ids.len()));
                let mut q = sqlx::query(&sql).bind(l.board).bind(l.stat).bind(context);
                for id in &ids {
                    q = q.bind(id);
                }
                for r in q.fetch_all(&self.pool).await?.iter().map(ranked) {
                    let s = stats.get(&(r.global_id.clone(), l.board, context)).cloned().unwrap_or_default();
                    ranks.push(json!({
                        "global_id": r.global_id, "name": r.name, "leaderboard": l.id, "context": context,
                        "rank": r.rank, "value": r.value, "stats": s,
                    }));
                }
            }
        }
        Ok(json!({ "ranks": ranks, "totals": totals }))
    }

    /// Everything kept for `ids`.
    pub async fn player_stats(&self, ids: &[Value]) -> sqlx::Result<Value> {
        let ids = checked_ids(ids);
        if ids.is_empty() {
            return Ok(json!({ "stats": [] }));
        }
        let sql = format!(
            "SELECT global_id, board, context, stat, value FROM global_stats WHERE global_id IN ({}) ORDER BY global_id, board, context, stat",
            params_from(1, ids.len())
        );
        let mut q = sqlx::query_as::<_, (String, u32, u32, u32, f64)>(&sql);
        for id in &ids {
            q = q.bind(id);
        }
        let stats: Vec<Value> = q
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|(global_id, board, context, stat, value)| json!({ "global_id": global_id, "board": board, "context": context, "stat": stat, "value": value }))
            .collect();
        Ok(json!({ "stats": stats }))
    }

    /// For the admin UI: every list that isn't empty and how many it ranks, and with a
    /// leaderboard and context, its top `count`.
    pub async fn admin_leaderboard(&self, pick: Option<(&Leaderboard, u32)>, count: usize) -> sqlx::Result<Value> {
        let mut lists = Vec::new();
        let mut chosen = Value::Null;
        for l in stat_boards::LEADERBOARDS {
            for (context, total) in self.totals(l).await? {
                lists.push(json!({ "leaderboard": l.id, "context": context, "total": total }));
                if pick.is_some_and(|(p, c)| p.id == l.id && c == context) {
                    chosen = self.list(l, context, total, count.clamp(1, MAX_COUNT)).await?;
                }
            }
        }
        if chosen.is_null() {
            if let Some((l, context)) = pick {
                chosen = json!({ "leaderboard": l.id, "context": context, "total": 0, "top": [] });
            }
        }
        let leaderboards: Vec<Value> = stat_boards::LEADERBOARDS
            .iter()
            .map(|l| {
                let contexts = stat_boards::board(l.board).map(stat_boards::Board::context_ids).unwrap_or_default();
                json!({ "id": l.id, "board": l.board, "stat": l.stat, "descending": l.descending, "contexts": contexts })
            })
            .collect();
        Ok(json!({ "leaderboards": leaderboards, "lists": lists, "list": chosen }))
    }

    /// Removes everything kept for `global_id` (and the name shown for them): answers how
    /// many stats went, and the name they had.
    pub async fn remove_stats(&self, global_id: &str) -> sqlx::Result<(u64, Option<String>)> {
        let mut tx = self.pool.begin().await?;
        let name: Option<String> = sqlx::query_scalar("DELETE FROM global_names WHERE global_id = ? RETURNING name")
            .bind(global_id)
            .fetch_optional(&mut *tx)
            .await?;
        let done = sqlx::query("DELETE FROM global_stats WHERE global_id = ?").bind(global_id).execute(&mut *tx).await?;
        tx.commit().await?;
        self.forget_leaderboards().await;
        Ok((done.rows_affected(), name))
    }
}

fn ranked(row: &sqlx::sqlite::SqliteRow) -> Ranked {
    Ranked {
        global_id: row.get("global_id"),
        name: row.get("name"),
        rank: row.get("rank"),
        value: row.get("value"),
    }
}
