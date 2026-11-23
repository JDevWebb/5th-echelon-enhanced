//! Every match, kept for good (0.5.6's RP4 and RP5), for the admin UI's Matches page: a match
//! browser for finding a past match to review, and each match's result.
//!
//! Filled as the servers' events come in ([`note`], from `record_events`): a match is a server's
//! room of kind `match` and when it was made (`since`), as the Match page names it. Its events
//! (`room`, `join`, `leave`, `match_state`, `stats`, all carrying the room) give its map, mode,
//! host, players, how each left, whether it got under way, and when it ended (its host's leave
//! with `ended`). A player's game writes its stats at the end of a match or mission
//! (`stats`, with the room since 0.5.6): those values are the player's result. A match keeps
//! its rows after its events (30 days) and recordings (30 days) are gone.
//!
//! Every number here is as a player's game gave it, so it can be doctored; the Match page sets
//! the players' results side by side (one side's wins are the other's losses).

use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// The result stats a `stats` event carries, summed over a player's writes for one match.
pub const RESULT_STATS: [&str; 14] = [
    "kills", "deaths", "assists", "shots", "hits", "headshots", "from_above", "mark_execute", "completed", "wins", "ties", "losses", "time", "score",
];
/// Matches on a page of the list.
const PAGE: i64 = 50;

/// Notes event `kind` (`times` more of it than before) for the matches it names.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn note(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    server: &str,
    at: i64,
    last_at: i64,
    player: Option<i64>,
    name: &str,
    kind: &str,
    d: &Value,
    times: i64,
) -> sqlx::Result<()> {
    let rooms: Vec<i64> = match kind {
        "match_state" => d["rooms"].as_array().map(|r| r.iter().filter_map(Value::as_i64).collect()).unwrap_or_default(),
        _ => d["room"].as_i64().into_iter().collect(),
    };
    if rooms.is_empty() {
        return Ok(());
    }
    for room in rooms {
        // The match's `since`: the event's own, else the room's latest match before it.
        let since = match d["since"].as_i64() {
            Some(s) => Some(s),
            None => {
                sqlx::query_scalar("SELECT since FROM played_matches WHERE server_id = ? AND room = ? AND since <= ? ORDER BY since DESC LIMIT 1")
                    .bind(server)
                    .bind(room)
                    .bind(at)
                    .fetch_optional(&mut **tx)
                    .await?
            }
        };
        let Some(since) = since else { continue };
        let is_match = d["room_kind"].as_str().is_none_or(|k| k == "match");
        if !is_match {
            continue;
        }
        if d["room_kind"] == "match" {
            sqlx::query(
                "INSERT INTO played_matches (server_id, room, since, first_at, last_at, mode, game_mode, map, host, host_name, private)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT (server_id, room, since) DO UPDATE SET
                   first_at = MIN(first_at, excluded.first_at), last_at = MAX(last_at, excluded.last_at),
                   mode = CASE WHEN excluded.mode <> '' THEN excluded.mode ELSE mode END,
                   game_mode = COALESCE(excluded.game_mode, game_mode), map = COALESCE(excluded.map, map),
                   host = COALESCE(excluded.host, host),
                   host_name = CASE WHEN excluded.host_name <> '' THEN excluded.host_name ELSE host_name END,
                   private = excluded.private",
            )
            .bind(server)
            .bind(room)
            .bind(since)
            .bind(at)
            .bind(last_at)
            .bind(d["mode"].as_str().unwrap_or_default())
            .bind(d["game_mode"].as_i64())
            .bind(d["map"].as_i64())
            .bind(d["host"].as_i64())
            .bind(d["host_name"].as_str().unwrap_or_default())
            .bind(d["private"] == true)
            .execute(&mut **tx)
            .await?;
        } else {
            // An event that names only the room (match_state, stats): the match must be known.
            let known = sqlx::query("UPDATE played_matches SET last_at = MAX(last_at, ?4) WHERE server_id = ?1 AND room = ?2 AND since = ?3")
                .bind(server)
                .bind(room)
                .bind(since)
                .bind(last_at)
                .execute(&mut **tx)
                .await?;
            if known.rows_affected() == 0 {
                continue;
            }
        }
        if kind == "match_state" && d["under_way"] == true {
            sqlx::query("UPDATE played_matches SET under_way = 1 WHERE server_id = ? AND room = ? AND since = ?")
                .bind(server)
                .bind(room)
                .bind(since)
                .execute(&mut **tx)
                .await?;
        }
        if kind == "round_result" && d["host"] == true {
            add_round(tx, server, room, since, d, at).await?;
        }
        if kind == "leave" && d["ended"] == true {
            sqlx::query("UPDATE played_matches SET ended_at = MAX(COALESCE(ended_at, 0), ?4) WHERE server_id = ?1 AND room = ?2 AND since = ?3")
                .bind(server)
                .bind(room)
                .bind(since)
                .bind(at)
                .execute(&mut **tx)
                .await?;
        }
        let Some(player) = player else { continue };
        sqlx::query(
            "INSERT INTO played_players (server_id, room, since, player, name, first_at, last_at, how) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (server_id, room, since, player) DO UPDATE SET
               name = CASE WHEN excluded.name <> '' THEN excluded.name ELSE name END,
               first_at = MIN(first_at, excluded.first_at), last_at = MAX(last_at, excluded.last_at),
               how = COALESCE(excluded.how, how)",
        )
        .bind(server)
        .bind(room)
        .bind(since)
        .bind(player)
        .bind(name)
        .bind(at)
        .bind(last_at)
        .bind((kind == "leave").then(|| d["how"].as_str().unwrap_or("left").to_string()))
        .execute(&mut **tx)
        .await?;
        if kind == "stats" && times > 0 {
            add_result(tx, server, room, since, player, name, d, times, last_at).await?;
        }
    }
    Ok(())
}

/// Keeps a round's result (the host's word) with its match, by its number (a round said again
/// replaces it); at most 64 rounds a match.
async fn add_round(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, server: &str, room: i64, since: i64, d: &Value, at: i64) -> sqlx::Result<()> {
    let before: Option<String> = sqlx::query_scalar("SELECT rounds FROM played_matches WHERE server_id = ? AND room = ? AND since = ?")
        .bind(server)
        .bind(room)
        .bind(since)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(before) = before else { return Ok(()) };
    let mut rounds: Vec<Value> = serde_json::from_str(&before).unwrap_or_default();
    let mut round = serde_json::Map::new();
    for k in ["round", "winner_team", "winner_player", "rounds", "hacks", "score"] {
        if let Some(v) = d[k].as_i64() {
            round.insert(k.into(), json!(v));
        }
    }
    round.insert("at".into(), json!(at));
    let number = d["round"].as_i64();
    rounds.retain(|r| number.is_none() || r["round"].as_i64() != number);
    if rounds.len() < 64 {
        rounds.push(Value::Object(round));
    }
    rounds.sort_by_key(|r| r["at"].as_i64());
    sqlx::query("UPDATE played_matches SET rounds = ? WHERE server_id = ? AND room = ? AND since = ?")
        .bind(Value::Array(rounds).to_string())
        .bind(server)
        .bind(room)
        .bind(since)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Adds a stats write (`times` of it) to a player's result for a match.
#[allow(clippy::too_many_arguments)]
async fn add_result(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    server: &str,
    room: i64,
    since: i64,
    player: i64,
    name: &str,
    d: &Value,
    times: i64,
    at: i64,
) -> sqlx::Result<()> {
    let before: Option<String> = sqlx::query_scalar("SELECT result FROM match_results WHERE server_id = ? AND room = ? AND since = ? AND player = ?")
        .bind(server)
        .bind(room)
        .bind(since)
        .bind(player)
        .fetch_optional(&mut **tx)
        .await?;
    let mut result: serde_json::Map<String, Value> = before.and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default();
    #[allow(clippy::cast_precision_loss)]
    let times_f = times as f64;
    for stat in RESULT_STATS {
        if let Some(v) = d[stat].as_f64().filter(|v| v.is_finite()) {
            let sum = result.get(stat).and_then(Value::as_f64).unwrap_or(0.0) + v * times_f;
            result.insert(stat.into(), json!((sum * 100.0).round() / 100.0));
        }
    }
    sqlx::query(
        "INSERT INTO match_results (server_id, room, since, player, name, writes, result, last_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (server_id, room, since, player) DO UPDATE SET writes = writes + excluded.writes, result = excluded.result,
           last_at = MAX(last_at, excluded.last_at), name = CASE WHEN excluded.name <> '' THEN excluded.name ELSE name END",
    )
    .bind(server)
    .bind(room)
    .bind(since)
    .bind(player)
    .bind(name)
    .bind(times)
    .bind(Value::Object(result).to_string())
    .bind(at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// What the Matches page asks for.
#[derive(Debug, Default, serde::Deserialize)]
pub struct MatchesQuery {
    /// Unix seconds; the last week when left out.
    pub from: Option<i64>,
    pub to: Option<i64>,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub mode: String,
    pub map: Option<i64>,
    /// Part of a player's name, or their account number on the server.
    #[serde(default)]
    pub player: String,
    /// Only matches with recordings, or with results.
    #[serde(default)]
    pub recordings: bool,
    #[serde(default)]
    pub results: bool,
    /// Matches started before this (the next page).
    pub before: Option<i64>,
}

impl Coordinator {
    /// The Matches page: matches newest first, with their players, results and recordings.
    pub async fn matches_view(&self, q: &MatchesQuery) -> sqlx::Result<Value> {
        let now = identity::now();
        let to = q.to.unwrap_or(now).min(q.before.unwrap_or(i64::MAX));
        let from = q.from.unwrap_or(now - 7 * 86_400);
        let player = q.player.trim().to_lowercase();
        let player_id: Option<i64> = player.parse().ok();
        let rows: Vec<(String, i64, i64, i64, i64, Option<i64>, String, Option<i64>, Option<i64>, String, bool, bool)> = sqlx::query_as(
            "SELECT m.server_id, m.room, m.since, m.first_at, m.last_at, m.ended_at, m.mode, m.game_mode, m.map, m.host_name, m.private, m.under_way
               FROM played_matches m
              WHERE m.since >= ?1 AND m.since < ?2 AND (?3 = '' OR m.server_id = ?3) AND (?4 = '' OR m.mode = ?4) AND (?5 IS NULL OR m.map = ?5)
                AND (?6 = '' OR EXISTS (SELECT 1 FROM played_players p WHERE p.server_id = m.server_id AND p.room = m.room AND p.since = m.since
                                         AND (lower(p.name) LIKE '%' || ?6 || '%' OR p.player = ?7)))
                AND (?8 = 0 OR EXISTS (SELECT 1 FROM match_results r WHERE r.server_id = m.server_id AND r.room = m.room AND r.since = m.since))
              ORDER BY m.since DESC LIMIT ?9",
        )
        .bind(from)
        .bind(to)
        .bind(q.server.trim())
        .bind(q.mode.trim())
        .bind(q.map)
        .bind(&player)
        .bind(player_id)
        .bind(q.results)
        .bind(PAGE * 3)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for (server, room, since, first_at, last_at, ended_at, mode, game_mode, map, host_name, private, under_way) in rows {
            if out.len() as i64 >= PAGE {
                break;
            }
            let players: Vec<(i64, String, Option<String>)> =
                sqlx::query_as("SELECT player, name, how FROM played_players WHERE server_id = ? AND room = ? AND since = ? ORDER BY first_at")
                    .bind(&server)
                    .bind(room)
                    .bind(since)
                    .fetch_all(&self.pool)
                    .await?;
            let end = ended_at.unwrap_or(last_at);
            // Recordings: a player's sent within 15 minutes of the match's end (the events'
            // 30 days, as R2 keeps them).
            let recordings: i64 = sqlx::query_scalar(
                "SELECT COUNT(DISTINCT json_extract(e.detail, '$.id')) FROM session_events e
                  WHERE e.server_id = ?1 AND e.kind = 'recording' AND e.at BETWEEN ?2 AND ?3
                    AND e.player IN (SELECT player FROM played_players WHERE server_id = ?1 AND room = ?4 AND since = ?5)",
            )
            .bind(&server)
            .bind(first_at)
            .bind(end + 900)
            .bind(room)
            .bind(since)
            .fetch_one(&self.pool)
            .await?;
            if q.recordings && recordings == 0 {
                continue;
            }
            let results: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM match_results WHERE server_id = ? AND room = ? AND since = ?")
                .bind(&server)
                .bind(room)
                .bind(since)
                .fetch_one(&self.pool)
                .await?;
            out.push(json!({
                "server": server, "room": room, "since": since, "first_at": first_at, "last_at": last_at, "ended_at": ended_at,
                "mode": mode, "game_mode": game_mode, "map": map, "host_name": host_name, "private": private, "under_way": under_way,
                "players": players.iter().map(|(id, name, how)| json!({ "player": id, "name": name, "how": how })).collect::<Vec<_>>(),
                "recordings": recordings, "results": results,
            }));
        }
        let next = out.last().and_then(|m| m["since"].as_i64()).filter(|_| out.len() as i64 >= PAGE);
        let labels: Vec<(String, i64, String)> = sqlx::query_as("SELECT kind, id, name FROM labels").fetch_all(&self.pool).await?;
        let servers: Vec<(String, Option<String>)> = sqlx::query_as("SELECT id, json_extract(listing, '$.name') FROM servers ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(json!({
            "from": from, "to": to, "matches": out, "next": next,
            "labels": crate::game_names::labels(labels),
            "servers": servers.into_iter().map(|(id, name)| json!({ "id": id, "name": name.unwrap_or_default() })).collect::<Vec<_>>(),
        }))
    }

    /// A match's results: each player's line (their summed writes), and the checks between them.
    pub async fn match_results(&self, server: &str, room: i64, since: i64) -> sqlx::Result<Value> {
        let rows: Vec<(i64, String, i64, String)> =
            sqlx::query_as("SELECT player, name, writes, result FROM match_results WHERE server_id = ? AND room = ? AND since = ? ORDER BY player")
                .bind(server)
                .bind(room)
                .bind(since)
                .fetch_all(&self.pool)
                .await?;
        let lines: Vec<Value> = rows
            .into_iter()
            .map(|(player, name, writes, result)| json!({ "player": player, "name": name, "writes": writes, "result": serde_json::from_str::<Value>(&result).unwrap_or_default() }))
            .collect();
        let rounds: Option<String> = sqlx::query_scalar("SELECT rounds FROM played_matches WHERE server_id = ? AND room = ? AND since = ?")
            .bind(server)
            .bind(room)
            .bind(since)
            .fetch_optional(&self.pool)
            .await?;
        let rounds: Value = rounds.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_else(|| json!([]));
        Ok(json!({ "lines": lines, "checks": checks(&lines), "rounds": rounds }))
    }
}

/// Where the players' results don't add up (report only; each comes from a player's game):
/// more kills than deaths across the match (each kill is someone's death; suicides and the
/// world make deaths more, never kills), and more wins than the other players' losses.
pub fn checks(lines: &[Value]) -> Vec<String> {
    let sum = |stat: &str| lines.iter().filter_map(|l| l["result"][stat].as_f64()).sum::<f64>();
    let mut out = Vec::new();
    if lines.len() >= 2 {
        let (kills, deaths) = (sum("kills"), sum("deaths"));
        if kills > deaths + 1.0 {
            out.push(format!("{kills:.0} kills but {deaths:.0} deaths across the players"));
        }
        let (wins, losses) = (sum("wins"), sum("losses"));
        if wins > 0.0 && losses > 0.0 && (wins - losses).abs() > lines.len() as f64 {
            out.push(format!("{wins:.0} wins against {losses:.0} losses"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_that_dont_add_up_are_said() {
        let line = |k: f64, d: f64| json!({ "result": { "kills": k, "deaths": d } });
        assert!(checks(&[line(3.0, 2.0), line(2.0, 3.0)]).is_empty());
        assert_eq!(checks(&[line(30.0, 2.0), line(2.0, 3.0)]), vec!["32 kills but 5 deaths across the players"]);
        assert!(checks(&[line(30.0, 0.0)]).is_empty(), "one player: nothing to set it against");
    }
}
