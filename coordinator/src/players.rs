//! Players: each member server's accounts and play sessions, the matches played, and what
//! admins ask a server to do to a player. For the admin UI's player list and reports.
//!
//! * `POST /v1/players` (a member): its players and play sessions, at start (the whole
//!   roster, `full`), then the changes every five minutes, and the whole roster again every
//!   six hours. Up to [`MAX_PLAYERS`] players and [`MAX_SESSIONS`] sessions a request.
//! * Finished matches come with each minute's metrics (see [`Coordinator::record_matches`]).
//! * The pulse's answer (every ten seconds) carries the server's pending actions, oldest
//!   first; the server answers each with `POST /v1/actions/<id>`. One unanswered for an hour
//!   has expired.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use serde_json::json;
use serde_json::Value;
use sqlx::Row as _;

use crate::metrics::day_of;
use crate::Coordinator;

/// The most players and sessions one request may carry.
pub const MAX_PLAYERS: usize = 2000;
pub const MAX_SESSIONS: usize = 5000;
/// The largest players request (2000 players with long names and ban reasons fit).
pub const MAX_BODY: usize = 4 * 1024 * 1024;
/// Players one server may have here, and sessions: past these, only rows already here change.
const PLAYERS_PER_SERVER: usize = 100_000;
const SESSIONS_PER_SERVER: i64 = 2_000_000;
/// Sessions and matches are kept this long.
pub(crate) const KEEP_FOR: i64 = 400 * 86_400;
/// A session longer than this is cut to it (a server that never closed one).
const LONGEST_SESSION: i64 = 7 * 86_400;
/// A match longer than this isn't one.
const LONGEST_MATCH: i64 = 86_400;
/// The most finished matches taken from one metrics report (sent each minute), and kept
/// for one server.
const MAX_MATCHES: usize = 100;
const MATCHES_PER_SERVER: i64 = 2_000_000;
/// An action unanswered this long has expired; a reset password nobody read this long after
/// it came is blanked.
pub(crate) const ACTION_EXPIRES: i64 = 3600;
const PASSWORD_KEPT: i64 = 3600;
/// Actions sent with one pulse.
const ACTIONS_PER_PULSE: i64 = 20;
/// Rows a page of the player list has.
pub const PAGE: i64 = 50;

/// What an admin can ask a server to do to a player.
pub const KINDS: [&str; 6] = ["ban", "unban", "kick", "reset_password", "rename", "delete"];

/// Whether an action wants a second factor proved lately (as rollbacks do).
pub fn sensitive(kind: &str) -> bool {
    matches!(kind, "ban" | "delete" | "rename" | "reset_password")
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

/// Whether `name` may be a player's new name: 3-24 letters, digits, _ - and .
pub fn valid_new_name(name: &str) -> bool {
    (3..=24).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// One player as a server reported them, cleaned up.
struct Reported {
    id: i64,
    name: String,
    identity: Option<String>,
    created_at: Option<i64>,
    last_seen: Option<i64>,
    online: bool,
    play_seconds: i64,
    sessions: i64,
    matches: i64,
    /// (reason, until, at).
    ban: Option<(String, Option<i64>, i64)>,
}

impl Reported {
    /// None without a usable id: everything else is cleaned or capped, so one odd field
    /// doesn't lose the player (or, in a full roster, delete them).
    fn from(v: &Value, now: i64) -> Option<Self> {
        let id = v["id"].as_i64().filter(|id| (0..=i64::from(u32::MAX)).contains(id))?;
        let name = clean(v["name"].as_str().unwrap_or_default(), 64);
        let time = |t: &Value| t.as_i64().filter(|t| (1..=now + 86_400).contains(t));
        let count = |k: &str, max: i64| v[k].as_i64().unwrap_or(0).clamp(0, max);
        let b = &v["banned"];
        Some(Self {
            id,
            name: if name.is_empty() { format!("#{id}") } else { name },
            identity: v["identity"]
                .as_str()
                .map(str::trim)
                .filter(|s| (1..=128).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric()))
                .map(str::to_string),
            created_at: time(&v["created_at"]),
            last_seen: time(&v["last_seen"]),
            online: v["online"].as_bool().unwrap_or(false),
            // A hundred years, a million sessions a year.
            play_seconds: count("play_seconds", 100 * 365 * 86_400),
            sessions: count("sessions", 100_000_000),
            matches: count("matches", 100_000_000),
            ban: b.is_object().then(|| {
                (
                    clean(b["reason"].as_str().unwrap_or_default(), 200),
                    b["until"].as_i64().filter(|t| *t > 0),
                    time(&b["at"]).unwrap_or(now),
                )
            }),
        })
    }
}

/// One play session as a server reported it, checked: (id, player, started, ended).
fn session(v: &Value, now: i64) -> Option<(i64, i64, i64, Option<i64>)> {
    let id = v["id"].as_i64().filter(|id| *id >= 0)?;
    let player = v["player"].as_i64().filter(|p| (0..=i64::from(u32::MAX)).contains(p))?;
    let started = v["start"].as_i64().filter(|t| (now - KEEP_FOR..=now + 3600).contains(t))?;
    let ended = match &v["end"] {
        Value::Null => None,
        e => Some(e.as_i64().filter(|t| *t >= started && *t <= now + 3600)?.min(started + LONGEST_SESSION)),
    };
    Some((id, player, started, ended))
}

/// Seconds of each UTC day from `from` on that `[start, end)` covers.
fn split_by_day(start: i64, end: i64, from: i64, mut add: impl FnMut(i64, i64)) {
    let mut t = start.max(from);
    while t < end {
        let next = (day_of(t) + 86_400).min(end);
        add(day_of(t), next - t);
        t = next;
    }
}

/// The list's columns: a player, whether they're online now (and their server sent a
/// heartbeat in the last 3 minutes), their play in the 7 days to ?1 (from sessions), and the other servers their
/// identity has an account on. ?1 is now.
const PLAYER_COLUMNS: &str = "p.server_id, p.id, p.name, p.identity, p.created_at, p.last_seen,
       (p.online = 1 AND COALESCE(s.last_seen, 0) >= ?1 - 180) AS online_now,
       p.play_seconds, p.sessions, p.matches, p.banned_reason, p.banned_until, p.banned_at,
       (SELECT COALESCE(SUM(MIN(COALESCE(x.ended, MIN(?1, MAX(x.started, COALESCE(s.last_seen, 0) + 60))), ?1) - MAX(x.started, ?1 - 604800)), 0)
          FROM play_sessions x WHERE x.server_id = p.server_id AND x.player = p.id AND x.started < ?1
           AND COALESCE(x.ended, ?1) > ?1 - 604800) AS week_seconds,
       (SELECT group_concat(o.server_id) FROM players o JOIN links l ON l.global_id = o.identity AND l.server_id = o.server_id
         WHERE o.identity = p.identity AND o.server_id != p.server_id) AS also_on";

fn player_json(r: &sqlx::sqlite::SqliteRow, now: i64) -> Value {
    let banned_at: Option<i64> = r.get("banned_at");
    let banned_until: Option<i64> = r.get("banned_until");
    let also: Option<String> = r.get("also_on");
    let mut also: Vec<String> = also.map(|a| a.split(',').map(str::to_string).collect()).unwrap_or_default();
    also.sort();
    also.dedup();
    json!({
        "server": r.get::<String, _>("server_id"),
        "id": r.get::<i64, _>("id"),
        "name": r.get::<String, _>("name"),
        "identity": r.get::<Option<String>, _>("identity"),
        "created_at": r.get::<Option<i64>, _>("created_at"),
        "last_seen": r.get::<Option<i64>, _>("last_seen"),
        "online": r.get::<bool, _>("online_now"),
        "play_seconds": r.get::<i64, _>("play_seconds"),
        "week_seconds": r.get::<i64, _>("week_seconds").max(0),
        "sessions": r.get::<i64, _>("sessions"),
        "matches": r.get::<i64, _>("matches"),
        // A ban that ran out isn't one any more.
        "banned": banned_at.filter(|_| banned_until.is_none_or(|u| u > now)).map(|at| json!({
            "reason": r.get::<Option<String>, _>("banned_reason").unwrap_or_default(), "until": banned_until, "at": at,
        })),
        "also_on": also,
    })
}

/// A player list query: search, filters, order and page.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub online: String,
    #[serde(default)]
    pub banned: String,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub page: i64,
}

fn yes(v: &str) -> bool {
    matches!(v, "1" | "true" | "yes")
}

impl Coordinator {
    /// Takes a server's players and play sessions (`POST /v1/players`). With `full`, the list
    /// is the whole roster: its players not in it were deleted there. Answers what was taken.
    pub(crate) async fn record_players(&self, server: &str, body: &Value) -> sqlx::Result<Value> {
        let now = identity::now();
        let full = body["full"].as_bool().unwrap_or(false);
        let empty = Vec::new();
        let players = body["players"].as_array().unwrap_or(&empty);
        let sessions = body["sessions"].as_array().unwrap_or(&empty);
        let mut tx = self.pool.begin().await?;
        let mut known: HashSet<i64> = sqlx::query_scalar("SELECT id FROM players WHERE server_id = ?")
            .bind(server)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
        let (mut taken, mut skipped) = (0, 0);
        let mut listed = HashSet::new();
        for v in players {
            let Some(p) = Reported::from(v, now) else {
                skipped += 1;
                continue;
            };
            listed.insert(p.id);
            if !known.contains(&p.id) && known.len() >= PLAYERS_PER_SERVER {
                skipped += 1;
                continue;
            }
            let (reason, until, at) = match p.ban {
                Some((reason, until, at)) => (Some(reason), until, Some(at)),
                None => (None, None, None),
            };
            sqlx::query(
                "INSERT INTO players (server_id, id, name, identity, created_at, last_seen, online, play_seconds, sessions, matches,
                                      banned_reason, banned_until, banned_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(server_id, id) DO UPDATE SET name = excluded.name, identity = excluded.identity, created_at = excluded.created_at,
                    last_seen = excluded.last_seen, online = excluded.online, play_seconds = excluded.play_seconds, sessions = excluded.sessions,
                    matches = excluded.matches, banned_reason = excluded.banned_reason, banned_until = excluded.banned_until,
                    banned_at = excluded.banned_at, updated_at = excluded.updated_at",
            )
            .bind(server)
            .bind(p.id)
            .bind(&p.name)
            .bind(&p.identity)
            .bind(p.created_at)
            .bind(p.last_seen)
            .bind(p.online)
            .bind(p.play_seconds)
            .bind(p.sessions)
            .bind(p.matches)
            .bind(reason)
            .bind(until)
            .bind(at)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            known.insert(p.id);
            taken += 1;
        }
        // The whole roster: whoever isn't on it was deleted on the server. Their sessions stay,
        // for time played.
        let mut removed = 0;
        if full {
            for id in known.iter().filter(|id| !listed.contains(*id)) {
                sqlx::query("DELETE FROM players WHERE server_id = ? AND id = ?")
                    .bind(server)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                removed += 1;
            }
            known.retain(|id| listed.contains(id));
        }
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM play_sessions WHERE server_id = ?")
            .bind(server)
            .fetch_one(&mut *tx)
            .await?;
        // Past the limit, only sessions already here change (a server can't fill the disk).
        let insert = if stored < SESSIONS_PER_SERVER {
            "INSERT INTO play_sessions (server_id, id, player, started, ended) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(server_id, id) DO UPDATE SET player = excluded.player, started = excluded.started, ended = excluded.ended"
        } else {
            "UPDATE play_sessions SET player = ?3, started = ?4, ended = ?5 WHERE server_id = ?1 AND id = ?2"
        };
        let mut session_count = 0;
        for v in sessions {
            // Only the server's own players' sessions.
            let Some((id, player, started, ended)) = session(v, now).filter(|s| known.contains(&s.1)) else {
                skipped += 1;
                continue;
            };
            sqlx::query(insert).bind(server).bind(id).bind(player).bind(started).bind(ended).execute(&mut *tx).await?;
            session_count += 1;
        }
        tx.commit().await?;
        Ok(json!({ "ok": true, "players": taken, "sessions": session_count, "removed": removed, "skipped": skipped }))
    }

    /// Stores the finished matches in a metrics report (`metrics.matches`; older servers send
    /// none). The same match sent again is one row.
    pub(crate) async fn record_matches(&self, server: &str, matches: &Value) -> sqlx::Result<()> {
        let now = identity::now();
        let Some(list) = matches.as_array().filter(|l| !l.is_empty()) else {
            return Ok(());
        };
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM matches WHERE server_id = ?")
            .bind(server)
            .fetch_one(&self.pool)
            .await?;
        if stored >= MATCHES_PER_SERVER {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for m in list.iter().take(MAX_MATCHES) {
            let mode = match m["mode"].as_str() {
                Some(mode @ ("svm" | "coop")) => mode,
                _ => "other",
            };
            let (Some(started), Some(ended)) = (m["started"].as_i64(), m["ended"].as_i64()) else {
                continue;
            };
            if started <= 0 || ended < started || ended > now + 3600 || ended - started > LONGEST_MATCH {
                continue;
            }
            let id = |v: &Value| v.as_i64().filter(|n| (0..=i64::from(u32::MAX)).contains(n)).unwrap_or(0);
            sqlx::query("INSERT OR IGNORE INTO matches (server_id, mode, map, game_mode, started, ended, players, private) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(server)
                .bind(mode)
                .bind(id(&m["map"]))
                .bind(id(&m["game_mode"]))
                .bind(started)
                .bind(ended)
                .bind(m["players"].as_i64().unwrap_or(0).clamp(0, 256))
                .bind(m["private"].as_bool().unwrap_or(false))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    /// Queues an action for `player` on `server`. Answers its id.
    pub(crate) async fn queue_action(&self, server: &str, player: i64, kind: &str, args: &Value, by: &str) -> sqlx::Result<i64> {
        sqlx::query_scalar("INSERT INTO player_actions (server_id, player, kind, args, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?) RETURNING id")
            .bind(server)
            .bind(player)
            .bind(kind)
            .bind(args.to_string())
            .bind(by)
            .bind(identity::now())
            .fetch_one(&self.pool)
            .await
    }

    /// Gives up on actions unanswered for an hour, and blanks reset passwords nobody read.
    pub(crate) async fn expire_actions(&self) -> sqlx::Result<()> {
        let now = identity::now();
        sqlx::query("UPDATE player_actions SET status = 'expired', message = 'The server didn''t answer within an hour.', done_at = ? WHERE status = 'pending' AND created_at < ?")
            .bind(now)
            .bind(now - ACTION_EXPIRES)
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE player_actions SET password = NULL WHERE password IS NOT NULL AND done_at < ?")
            .bind(now - PASSWORD_KEPT)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The actions `server` has to carry out, oldest first, as its pulse answer lists them.
    pub(crate) async fn pending_actions(&self, server: &str) -> sqlx::Result<Vec<Value>> {
        self.expire_actions().await?;
        let rows: Vec<(i64, i64, String, String)> =
            sqlx::query_as("SELECT id, player, kind, args FROM player_actions WHERE server_id = ? AND status = 'pending' ORDER BY id LIMIT ?")
                .bind(server)
                .bind(ACTIONS_PER_PULSE)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(id, player, kind, args)| {
                let a: Value = serde_json::from_str(&args).unwrap_or_default();
                json!({ "id": id, "kind": kind, "player": player, "reason": a["reason"].as_str().unwrap_or_default(), "until": a["until"], "name": a["name"] })
            })
            .collect())
    }

    /// What came of an action, from the server it was for (`POST /v1/actions/<id>`). False
    /// when it's no action of that server's. A late answer to an expired action still counts:
    /// it's what happened.
    pub(crate) async fn action_result(&self, server: &str, id: i64, result: &Value) -> sqlx::Result<bool> {
        let row: Option<(i64, String, String, String)> = sqlx::query_as("SELECT player, kind, args, status FROM player_actions WHERE id = ? AND server_id = ?")
            .bind(id)
            .bind(server)
            .fetch_optional(&self.pool)
            .await?;
        let Some((player, kind, args, status)) = row else { return Ok(false) };
        if status == "done" || status == "failed" {
            return Ok(true);
        }
        let now = identity::now();
        let done = result["ok"].as_bool().unwrap_or(false);
        let message = clean(result["message"].as_str().unwrap_or_default(), 300);
        let password = result["password"]
            .as_str()
            .filter(|_| done && kind == "reset_password")
            .map(|p| clean(p, 64))
            .filter(|p| !p.is_empty());
        sqlx::query("UPDATE player_actions SET status = ?, message = ?, password = ?, done_at = ? WHERE id = ?")
            .bind(if done { "done" } else { "failed" })
            .bind(&message)
            .bind(password)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        if done {
            self.apply_done(server, player, &kind, &serde_json::from_str(&args).unwrap_or_default(), now).await?;
        }
        Ok(true)
    }

    /// Shows a finished action in the player list at once, before the server's next report.
    async fn apply_done(&self, server: &str, player: i64, kind: &str, args: &Value, now: i64) -> sqlx::Result<()> {
        match kind {
            "ban" => {
                sqlx::query("UPDATE players SET banned_reason = ?, banned_until = ?, banned_at = ?, online = 0 WHERE server_id = ? AND id = ?")
                    .bind(args["reason"].as_str().unwrap_or_default())
                    .bind(args["until"].as_i64())
                    .bind(now)
                    .bind(server)
                    .bind(player)
                    .execute(&self.pool)
                    .await?;
            }
            "unban" => {
                sqlx::query("UPDATE players SET banned_reason = NULL, banned_until = NULL, banned_at = NULL WHERE server_id = ? AND id = ?")
                    .bind(server)
                    .bind(player)
                    .execute(&self.pool)
                    .await?;
            }
            "kick" => {
                sqlx::query("UPDATE players SET online = 0 WHERE server_id = ? AND id = ?")
                    .bind(server)
                    .bind(player)
                    .execute(&self.pool)
                    .await?;
            }
            "delete" => {
                sqlx::query("DELETE FROM players WHERE server_id = ? AND id = ?")
                    .bind(server)
                    .bind(player)
                    .execute(&self.pool)
                    .await?;
            }
            "rename" => {
                let Some(name) = args["name"].as_str() else { return Ok(()) };
                let row: Option<(String, Option<String>)> = sqlx::query_as("SELECT name, identity FROM players WHERE server_id = ? AND id = ?")
                    .bind(server)
                    .bind(player)
                    .fetch_optional(&self.pool)
                    .await?;
                sqlx::query("UPDATE players SET name = ?, online = 0 WHERE server_id = ? AND id = ?")
                    .bind(name)
                    .bind(server)
                    .bind(player)
                    .execute(&self.pool)
                    .await?;
                if let Some((old, identity)) = row {
                    self.rename_link(server, &old, identity.as_deref(), name, now).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The identity linked to a player on `server`: their link by identity, or by their name
    /// there.
    async fn linked_identity(&self, server: &str, name: &str, identity: Option<&str>) -> sqlx::Result<Option<String>> {
        sqlx::query_scalar("SELECT global_id FROM links WHERE server_id = ? AND (global_id = ? OR username = ?) ORDER BY global_id = ? DESC LIMIT 1")
            .bind(server)
            .bind(identity)
            .bind(name)
            .bind(identity)
            .fetch_optional(&self.pool)
            .await
    }

    /// A rename an admin had a server make: the server can't sign for the player, so the link
    /// (what friends on other servers see) and the name reserved across the network follow
    /// here. The admin's action is the authority; the old name is released.
    async fn rename_link(&self, server: &str, old: &str, identity: Option<&str>, new: &str, now: i64) -> sqlx::Result<()> {
        let Some(global_id) = self.linked_identity(server, old, identity).await? else {
            return Ok(());
        };
        sqlx::query("UPDATE links SET username = ? WHERE server_id = ? AND global_id = ?")
            .bind(new)
            .bind(server)
            .bind(&global_id)
            .execute(&self.pool)
            .await?;
        if !self.claim_for(&global_id, new, now, Some(server)).await? {
            tracing::warn!("server {server}: renamed {old} to {new}, a name another player holds");
        }
        // Released at once: the grace for new accounts doesn't apply to a name being replaced.
        let used: Vec<String> = sqlx::query_scalar("SELECT username FROM links WHERE global_id = ?")
            .bind(&global_id)
            .fetch_all(&self.pool)
            .await?;
        if !used.iter().any(|u| identity::name_key(u) == identity::name_key(old)) {
            sqlx::query("DELETE FROM names WHERE name_key = ? AND global_id = ?")
                .bind(identity::name_key(old))
                .bind(&global_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Why `player` on `server` may not be renamed `name`, if they may not: another identity
    /// holds the name across the network.
    pub(crate) async fn rename_refused(&self, server: &str, player: i64, name: &str) -> sqlx::Result<Option<&'static str>> {
        let row: Option<(String, Option<String>)> = sqlx::query_as("SELECT name, identity FROM players WHERE server_id = ? AND id = ?")
            .bind(server)
            .bind(player)
            .fetch_optional(&self.pool)
            .await?;
        let Some((old, identity)) = row else { return Ok(Some("no such player")) };
        let mine = self.linked_identity(server, &old, identity.as_deref()).await?;
        Ok(match self.owner(&identity::name_key(name)).await? {
            Some(owner) if Some(&owner) != mine.as_ref() => Some("that name belongs to another player on the network"),
            _ => None,
        })
    }

    /// An action, for the admin UI. The reset password it brought is shown once, to the admin
    /// who asked for it, and then forgotten.
    pub(crate) async fn read_action(&self, id: i64, admin: &str) -> sqlx::Result<Option<Value>> {
        self.expire_actions().await?;
        let row: Option<(String, i64, String, String, String, i64, String, Option<String>, Option<i64>)> =
            sqlx::query_as("SELECT server_id, player, kind, args, created_by, created_at, status, message, done_at FROM player_actions WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let Some((server, player, kind, args, by, at, status, message, done_at)) = row else {
            return Ok(None);
        };
        let mut password: Option<String> = None;
        if by == admin {
            let kept: Option<String> = sqlx::query_scalar("SELECT password FROM player_actions WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
            if let Some(kept) = kept {
                // Whoever blanks it shows it: two reads at once can't both.
                let blanked = sqlx::query("UPDATE player_actions SET password = NULL WHERE id = ? AND password = ?")
                    .bind(id)
                    .bind(&kept)
                    .execute(&self.pool)
                    .await?;
                password = (blanked.rows_affected() == 1).then_some(kept);
            }
        }
        Ok(Some(json!({
            "id": id, "server": server, "player": player, "kind": kind, "args": serde_json::from_str::<Value>(&args).unwrap_or_default(),
            "created_by": by, "created_at": at, "status": status, "message": message, "done_at": done_at, "password": password,
        })))
    }

    /// A page of players across the servers, for the admin UI: one row a person. Accounts on
    /// several servers are one row when they're the same identity, linked on each server (as
    /// "also on"); the row opens the account last seen. A search, server or filter that
    /// matches any of a person's accounts shows the person.
    pub async fn player_list(&self, q: &ListQuery) -> sqlx::Result<Value> {
        let now = identity::now();
        let search = q.q.trim();
        let like = format!("%{}%", search.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let order = match q.sort.as_str() {
            "play_time" => "play_seconds DESC",
            "name" => "a.name COLLATE NOCASE",
            "created" => "created_at DESC NULLS LAST",
            _ => "online_now DESC, last_seen DESC NULLS LAST",
        };
        let accounts = format!(
            "WITH acc AS (
               SELECT {PLAYER_COLUMNS},
                      CASE WHEN p.identity IS NOT NULL AND EXISTS (SELECT 1 FROM links l WHERE l.global_id = p.identity AND l.server_id = p.server_id)
                           THEN 'i:' || p.identity ELSE 's:' || p.server_id || ':' || p.id END AS person,
                      (p.banned_at IS NOT NULL AND (p.banned_until IS NULL OR p.banned_until > ?1)) AS banned_now
                 FROM players p LEFT JOIN servers s ON s.id = p.server_id),
             hit AS (SELECT DISTINCT person FROM acc
                      WHERE (?2 = '' OR name LIKE ?3 ESCAPE '\\' OR identity = ?2 OR CAST(id AS TEXT) = ?2)
                        AND (?4 = '' OR server_id = ?4)
                        AND (?5 = 0 OR online_now)
                        AND (?6 = 0 OR banned_now))"
        );
        let page = q.page.clamp(0, 100_000);
        // The account a row opens: the one last seen.
        let rows = sqlx::query(&format!(
            "{accounts}
             SELECT g.*, a.server_id, a.id, a.name, a.identity FROM
               (SELECT person, MAX(last_seen) AS last_seen, MAX(online_now) AS online_now, MIN(created_at) AS created_at,
                       SUM(play_seconds) AS play_seconds, SUM(MAX(week_seconds, 0)) AS week_seconds, SUM(sessions) AS sessions,
                       SUM(matches) AS matches, SUM(banned_now) AS banned_accounts, COUNT(*) AS accounts,
                       group_concat(server_id, ',') AS servers
                  FROM acc WHERE person IN (SELECT person FROM hit) GROUP BY person) g
               JOIN (SELECT person, server_id, id, name, identity,
                            ROW_NUMBER() OVER (PARTITION BY person ORDER BY COALESCE(last_seen, 0) DESC, server_id, id) AS rn
                       FROM acc) a ON a.person = g.person AND a.rn = 1
              ORDER BY {order}, g.person LIMIT ?7 OFFSET ?8"
        ))
        .bind(now)
        .bind(search)
        .bind(&like)
        .bind(q.server.trim())
        .bind(yes(&q.online))
        .bind(yes(&q.banned))
        .bind(PAGE)
        .bind(page * PAGE)
        .fetch_all(&self.pool)
        .await?;
        let total: i64 = sqlx::query_scalar(&format!("{accounts} SELECT COUNT(*) FROM hit"))
            .bind(now)
            .bind(search)
            .bind(&like)
            .bind(q.server.trim())
            .bind(yes(&q.online))
            .bind(yes(&q.banned))
            .fetch_one(&self.pool)
            .await?;
        let players: Vec<Value> = rows
            .iter()
            .map(|r| {
                let server: String = r.get("server_id");
                let mut servers: Vec<String> = r.get::<String, _>("servers").split(',').map(str::to_string).collect();
                servers.sort();
                servers.dedup();
                let also_on: Vec<&String> = servers.iter().filter(|s| **s != server).collect();
                json!({
                    "server": server,
                    "id": r.get::<i64, _>("id"),
                    "name": r.get::<String, _>("name"),
                    "identity": r.get::<Option<String>, _>("identity"),
                    "servers": servers,
                    "also_on": also_on,
                    "accounts": r.get::<i64, _>("accounts"),
                    "created_at": r.get::<Option<i64>, _>("created_at"),
                    "last_seen": r.get::<Option<i64>, _>("last_seen"),
                    "online": r.get::<bool, _>("online_now"),
                    "play_seconds": r.get::<i64, _>("play_seconds"),
                    "week_seconds": r.get::<i64, _>("week_seconds"),
                    "sessions": r.get::<i64, _>("sessions"),
                    "matches": r.get::<i64, _>("matches"),
                    "banned_accounts": r.get::<i64, _>("banned_accounts"),
                })
            })
            .collect();
        Ok(json!({ "players": players, "total": total, "page": page, "per_page": PAGE }))
    }

    /// One player: their last 50 sessions, time played each day of the last 30, their other
    /// accounts (the same identity) and what admins asked their server to do.
    pub async fn player_detail(&self, server: &str, id: i64) -> sqlx::Result<Option<Value>> {
        let now = identity::now();
        let row = sqlx::query(&format!(
            "SELECT {PLAYER_COLUMNS} FROM players p LEFT JOIN servers s ON s.id = p.server_id WHERE p.server_id = ?2 AND p.id = ?3"
        ))
        .bind(now)
        .bind(server)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let player = player_json(&row, now);
        let server_seen: Option<i64> = sqlx::query_scalar("SELECT last_seen FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
        // A session still open counts until the server was last heard from.
        let open_until = |started: i64| now.min(started.max(server_seen.unwrap_or(0) + 60));
        let sessions: Vec<(i64, i64, Option<i64>)> =
            sqlx::query_as("SELECT id, started, ended FROM play_sessions WHERE server_id = ? AND player = ? ORDER BY started DESC LIMIT 50")
                .bind(server)
                .bind(id)
                .fetch_all(&self.pool)
                .await?;
        let from = day_of(now) - 29 * 86_400;
        let month: Vec<(i64, Option<i64>)> =
            sqlx::query_as("SELECT started, ended FROM play_sessions WHERE server_id = ? AND player = ? AND started < ? AND COALESCE(ended, ?) >= ?")
                .bind(server)
                .bind(id)
                .bind(now)
                .bind(now)
                .bind(from)
                .fetch_all(&self.pool)
                .await?;
        let mut days: BTreeMap<i64, i64> = (0..30).map(|i| (from + i * 86_400, 0)).collect();
        for (started, ended) in month {
            split_by_day(started, ended.unwrap_or_else(|| open_until(started)), from, |day, secs| {
                *days.entry(day).or_default() += secs;
            });
        }
        let others = match player["identity"].as_str() {
            Some(identity) => sqlx::query(&format!(
                "SELECT {PLAYER_COLUMNS} FROM players p LEFT JOIN servers s ON s.id = p.server_id
                  WHERE p.identity = ?2 AND NOT (p.server_id = ?3 AND p.id = ?4)
                    AND EXISTS (SELECT 1 FROM links l WHERE l.global_id = p.identity AND l.server_id = p.server_id)
                  ORDER BY p.server_id, p.id LIMIT 50"
            ))
            .bind(now)
            .bind(identity)
            .bind(server)
            .bind(id)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| player_json(r, now))
            .collect(),
            None => vec![],
        };
        let actions: Vec<(i64, String, String, String, i64, String, Option<String>, Option<i64>)> = sqlx::query_as(
            "SELECT id, kind, args, created_by, created_at, status, message, done_at FROM player_actions WHERE server_id = ? AND player = ? ORDER BY id DESC LIMIT 50",
        )
        .bind(server)
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(json!({
            "player": player,
            "sessions": sessions.into_iter().map(|(sid, started, ended)| json!({
                "id": sid, "start": started, "end": ended, "seconds": ended.unwrap_or_else(|| open_until(started)) - started,
            })).collect::<Vec<_>>(),
            "days": days.into_iter().map(|(t, secs)| json!({ "t": t, "seconds": secs })).collect::<Vec<_>>(),
            "others": others,
            "actions": actions.into_iter().map(|(aid, kind, args, by, at, status, message, done)| json!({
                "id": aid, "kind": kind, "args": serde_json::from_str::<Value>(&args).unwrap_or_default(), "created_by": by,
                "created_at": at, "status": status, "message": message, "done_at": done,
            })).collect::<Vec<_>>(),
        })))
    }

    /// The accounts sharing `player`'s identity (theirs too), as (server, id); just theirs
    /// without one.
    ///
    /// Only identities linked where each account is: a server says which identity its
    /// players have, and one mustn't have an admin's action reach another's players.
    pub(crate) async fn accounts_of(&self, server: &str, player: i64) -> sqlx::Result<Vec<(String, i64)>> {
        let identity: Option<String> = sqlx::query_scalar(
            "SELECT p.identity FROM players p JOIN links l ON l.global_id = p.identity AND l.server_id = p.server_id
              WHERE p.server_id = ? AND p.id = ?",
        )
        .bind(server)
        .bind(player)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        match identity {
            Some(identity) => {
                sqlx::query_as(
                    "SELECT p.server_id, p.id FROM players p JOIN links l ON l.global_id = p.identity AND l.server_id = p.server_id
                      WHERE p.identity = ? ORDER BY p.server_id, p.id LIMIT 100",
                )
                .bind(identity)
                .fetch_all(&self.pool)
                .await
            }
            None => Ok(vec![(server.to_string(), player)]),
        }
    }

    /// A player's name on a server, if the coordinator has them.
    pub(crate) async fn player_name(&self, server: &str, player: i64) -> sqlx::Result<Option<String>> {
        sqlx::query_scalar("SELECT name FROM players WHERE server_id = ? AND id = ?")
            .bind(server)
            .bind(player)
            .fetch_optional(&self.pool)
            .await
    }

    /// Play time from sessions for the players report: for each server sending sessions,
    /// from the first whole day it did on, the players and seconds played each day from
    /// `from`, the distinct players in all, and the most playing at once (every such server
    /// together).
    pub(crate) async fn session_days(&self, from: i64) -> sqlx::Result<SessionDays> {
        let now = identity::now();
        let firsts: Vec<(String, i64)> = sqlx::query_as("SELECT server_id, MIN(started) FROM play_sessions GROUP BY server_id")
            .fetch_all(&self.pool)
            .await?;
        let seen: HashMap<String, i64> = sqlx::query_as::<_, (String, Option<i64>)>("SELECT id, last_seen FROM servers")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|(id, t)| (id, t.unwrap_or(0)))
            .collect();
        let mut out = SessionDays {
            // Days a server's sessions cover in full: from the day after its first.
            since: firsts.into_iter().map(|(s, first)| (s, day_of(first) + 86_400)).collect(),
            ..SessionDays::default()
        };
        let rows: Vec<(String, i64, i64, Option<i64>)> =
            sqlx::query_as("SELECT server_id, player, started, ended FROM play_sessions WHERE started < ? AND COALESCE(ended, ?) >= ?")
                .bind(now)
                .bind(now)
                .bind(from)
                .fetch_all(&self.pool)
                .await?;
        let mut players: HashMap<(String, i64), HashSet<i64>> = HashMap::new();
        let mut edges: Vec<(i64, i32)> = Vec::new();
        for (server, player, started, ended) in rows {
            let since = out.since.get(&server).copied().unwrap_or(i64::MAX);
            let end = ended.unwrap_or_else(|| now.min(started.max(seen.get(&server).copied().unwrap_or(0) + 60)));
            let start = started.max(from);
            if end <= start {
                continue;
            }
            edges.push((start, 1));
            edges.push((end, -1));
            split_by_day(started, end, from.max(since), |day, secs| {
                *out.seconds.entry((server.clone(), day)).or_default() += secs;
                players.entry((server.clone(), day)).or_default().insert(player);
            });
            if end > from.max(since) {
                out.distinct.entry(server.clone()).or_default().insert(player);
            }
        }
        out.players = players.into_iter().map(|(k, v)| (k, v.len() as i64)).collect();
        // Ends before starts at the same second: back to back isn't two at once.
        edges.sort_unstable();
        let mut at_once = 0;
        for (_, d) in edges {
            at_once += d;
            out.peak = out.peak.max(i64::from(at_once));
        }
        Ok(out)
    }

    /// The matches report over the last `days` days: matches per day by mode, how long they
    /// last and how many play, by mode, map and game mode, private and public.
    pub async fn matches_report(&self, days: i64) -> sqlx::Result<Value> {
        let now = identity::now();
        let from = day_of(now) - (days - 1) * 86_400;
        let rows: Vec<(String, i64, i64, i64, i64, i64, bool)> =
            sqlx::query_as("SELECT mode, map, game_mode, started, ended, players, private FROM matches WHERE ended >= ? ORDER BY ended")
                .bind(from)
                .fetch_all(&self.pool)
                .await?;
        #[derive(Default)]
        struct Sum {
            matches: i64,
            seconds: i64,
            players: i64,
        }
        impl Sum {
            fn add(&mut self, secs: i64, players: i64) {
                self.matches += 1;
                self.seconds += secs;
                self.players += players;
            }
            fn json(&self) -> Value {
                let n = self.matches.max(1) as f64;
                json!({ "matches": self.matches, "avg_seconds": self.seconds as f64 / n, "avg_players": self.players as f64 / n })
            }
        }
        let every_day: Vec<i64> = (0..days).map(|i| from + i * 86_400).collect();
        let mut by_day: BTreeMap<i64, BTreeMap<String, i64>> = every_day.iter().map(|d| (*d, BTreeMap::new())).collect();
        let (mut total, mut private, mut public) = (Sum::default(), Sum::default(), Sum::default());
        let mut by_mode: BTreeMap<String, Sum> = BTreeMap::new();
        let mut by_map: BTreeMap<(String, i64), Sum> = BTreeMap::new();
        let mut by_game_mode: BTreeMap<(String, i64), Sum> = BTreeMap::new();
        for (mode, map, game_mode, started, ended, players, is_private) in rows {
            let secs = ended - started;
            *by_day.entry(day_of(ended)).or_default().entry(mode.clone()).or_default() += 1;
            total.add(secs, players);
            (if is_private { &mut private } else { &mut public }).add(secs, players);
            by_mode.entry(mode.clone()).or_default().add(secs, players);
            by_map.entry((mode.clone(), map)).or_default().add(secs, players);
            by_game_mode.entry((mode, game_mode)).or_default().add(secs, players);
        }
        // Matches started each day, and joins that failed, from the servers' counters.
        let mut counted: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
        for h in self.traffic_since(from, None).await? {
            let e = counted.entry(day_of(h.hour)).or_default();
            e.0 += h.matches_started;
            e.1 += h.failed_joins;
        }
        let ranked = |m: BTreeMap<(String, i64), Sum>, key: &str| {
            let mut list: Vec<(String, i64, Sum)> = m.into_iter().map(|((mode, id), s)| (mode, id, s)).collect();
            list.sort_by(|a, b| b.2.matches.cmp(&a.2.matches));
            list.truncate(25);
            list.into_iter()
                .map(|(mode, id, s)| {
                    let mut v = s.json();
                    v["mode"] = json!(mode);
                    v[key] = json!(id);
                    v
                })
                .collect::<Vec<_>>()
        };
        let labels: Vec<(String, i64, String)> = sqlx::query_as("SELECT kind, id, name FROM labels").fetch_all(&self.pool).await?;
        Ok(json!({
            "from": from,
            "to": now,
            "totals": total.json(),
            "private": private.json(),
            "public": public.json(),
            "days": by_day.into_iter().map(|(t, modes)| {
                let (started, failed_joins) = counted.get(&t).copied().unwrap_or_default();
                json!({ "t": t, "svm": modes.get("svm").copied().unwrap_or(0), "coop": modes.get("coop").copied().unwrap_or(0),
                        "other": modes.get("other").copied().unwrap_or(0), "started": started, "failed_joins": failed_joins })
            }).collect::<Vec<_>>(),
            "by_mode": by_mode.into_iter().map(|(mode, s)| { let mut v = s.json(); v["mode"] = json!(mode); v }).collect::<Vec<_>>(),
            "maps": ranked(by_map, "map"),
            "game_modes": ranked(by_game_mode, "game_mode"),
            "labels": crate::game_names::labels(labels),
        }))
    }
}

/// Play from sessions, for the players report (see [`Coordinator::session_days`]).
#[derive(Debug, Default)]
pub(crate) struct SessionDays {
    /// Each server sending sessions, and the first day they cover in full.
    pub since: HashMap<String, i64>,
    /// (server, day): seconds played, and players who played.
    pub seconds: HashMap<(String, i64), i64>,
    pub players: HashMap<(String, i64), i64>,
    /// Each server's players in the days its sessions cover.
    pub distinct: HashMap<String, HashSet<i64>>,
    /// The most playing at once, every server sending sessions together.
    pub peak: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_are_cleaned_not_dropped() {
        let now = 1_000_000;
        let p = Reported::from(
            &json!({ "id": 7, "name": "Ex\u{202e}o\u{7}", "identity": "ab-12", "created_at": now * 9, "play_seconds": -5, "banned": { "reason": "x".repeat(300) } }),
            now,
        )
        .unwrap();
        assert_eq!((p.name.as_str(), p.identity, p.created_at, p.play_seconds), ("Exo", None, None, 0));
        assert_eq!(p.ban.unwrap().0.len(), 200);
        assert_eq!(Reported::from(&json!({ "id": 3 }), now).unwrap().name, "#3");
        assert!(Reported::from(&json!({ "id": -1, "name": "A" }), now).is_none());
        assert!(Reported::from(&json!({ "name": "A" }), now).is_none());
    }

    #[test]
    fn sessions_are_checked_and_split_by_day() {
        let now = 500 * 86_400;
        assert_eq!(session(&json!({ "id": 1, "player": 2, "start": now - 60, "end": null }), now), Some((1, 2, now - 60, None)));
        assert!(session(&json!({ "id": 1, "player": 2, "start": now, "end": now - 1 }), now).is_none());
        assert!(session(&json!({ "id": 1, "player": 2, "start": now + 86_400 }), now).is_none());
        assert_eq!(session(&json!({ "id": 1, "player": 2, "start": 5, "end": 9 }), now), None, "older than kept");
        let mut days = Vec::new();
        split_by_day(86_400 - 100, 86_400 + 50, 0, |d, s| days.push((d, s)));
        assert_eq!(days, [(0, 100), (86_400, 50)]);
        days.clear();
        split_by_day(86_400 - 100, 86_400 + 50, 86_400, |d, s| days.push((d, s)));
        assert_eq!(days, [(86_400, 50)]);
    }

    #[test]
    fn new_names_follow_the_rules() {
        for good in ["Exo", "kiwi_2", "a.b-c", &"x".repeat(24)] {
            assert!(valid_new_name(good), "{good}");
        }
        for bad in ["ab", "has space", "ünï", &"x".repeat(25), ""] {
            assert!(!valid_new_name(bad), "{bad}");
        }
    }
}
