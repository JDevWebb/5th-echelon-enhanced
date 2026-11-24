//! The admin UI's Crashes page (the 0.5.0 crash work, item 4): every player's crashes and
//! freezes as their games sent them at their next start (`crash` session events), grouped by
//! where they happened, with how many players each hit, how often it ended the game, which
//! releases, the latest one's details (registers, the code there, callers, what the game was
//! doing) and the reports sent around it; and what the add-on's crash fixes did in players'
//! games (`crash_fixes` events): how often each kept a game going, and the builds where one
//! didn't go in.
//!
//! A report a player sent themselves (logs included, their choice) adds the crashes in its game
//! logs too (0.5.6's CT): crashes are sent at the next start only with diagnostics on, and a
//! report was often the only sign of them (Tea_bag's round-end crashes, 21-22 Nov). They're
//! `crash` events with the report's id in them, at negative ids (a server's own are positive),
//! and one the game sent as well, or another report carried (a log goes again as
//! `bl-tracing.prev.log`), counts once.
//!
//! An admin marks a group: `fixed` (in a release: crashes from that release or later show it
//! came back), `known` (nothing to do, an audio driver's say) or `watching`. A new crash, not
//! marked, that hits [`ALERT_PLAYERS`] players in a day is an alert, as is a fixed one seen
//! again since its fix.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// What a group can be marked.
pub const STATUSES: &[&str] = &["fixed", "known", "watching"];
/// The longest the page looks back (events are kept 30 days).
const LONGEST_DAYS: i64 = 30;
/// A new crash this many players hit in a day is an alert.
pub(crate) const ALERT_PLAYERS: usize = 3;
/// A report sent this long after a crash by the same player is linked to it.
const REPORT_WITHIN: i64 = 3 * 3600;
/// The most players named, and reports linked, per group.
const MOST_NAMED: usize = 12;
/// The most crashes taken from one report's logs.
const MOST_FROM_A_REPORT: usize = 10;
/// A crash from a report and one the game sent this close together, at the same place, by the
/// same player, are one.
const SAME_CRASH_WITHIN: i64 = 120;
/// The setting that says the reports already here were read for crashes (once, on the first
/// start with CT).
const FROM_REPORTS_DONE: &str = "crashes_from_reports";

/// Whether a report's file is a game log the add-on writes.
pub(crate) fn is_game_log(name: &str) -> bool {
    matches!(name, "bl-tracing.log" | "bl-tracing.prev.log")
}

/// The event id of crash `n` from report `report`: negative, never a server's own, and the same
/// each time, so a report read twice adds nothing.
fn report_crash_id(report: &str, n: usize) -> i64 {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(format!("{report}/{n}").as_bytes());
    let v = i64::from_be_bytes([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]]) & i64::MAX;
    -v.max(1)
}

/// The crashes in a report's game logs, as `crash` events' details (the fields the game server
/// gives one the game sent, and the report's id), oldest first, at most [`MOST_FROM_A_REPORT`]
/// (the latest).
pub(crate) fn crashes_in_logs(logs: &[String], report: &str, created_at: i64) -> Vec<(i64, Value)> {
    let short = |t: &str, max: usize| t.chars().filter(|c| !c.is_control()).take(max).collect::<String>();
    let parts = |text: &str, by: &str| text.split(by).map(str::trim).filter(|p| !p.is_empty()).take(16).map(|p| short(p, 190)).collect::<Vec<_>>();
    let mut found: Vec<hooks_config::crash_log::Crash> = logs.iter().flat_map(|l| hooks_config::crash_log::crashes(l)).filter(|c| valid_place(&c.place)).collect();
    found.sort_by_key(|c| c.at);
    found.dedup_by(|a, b| a.at == b.at && a.place == b.place && a.kind == b.kind);
    let skip = found.len().saturating_sub(MOST_FROM_A_REPORT);
    found
        .into_iter()
        .skip(skip)
        .map(|c| {
            let release = c.release.clone().unwrap_or_default();
            (
                c.at,
                json!({
                    "kind": c.kind.as_str(),
                    "at": c.at,
                    "code": short(&c.code, 120),
                    "place": short(&c.place, 120),
                    "stack": parts(&c.stack, ", "),
                    "registers": parts(&c.registers, "; "),
                    "callers": parts(&c.callers, ", "),
                    "doing": short(&c.doing, 190),
                    "fatal": c.fatal,
                    "release": short(&release, 32),
                    "release_from": if release.is_empty() { "" } else { "log" },
                    "reported_after_s": (created_at - c.at).max(0),
                    "report": report,
                }),
            )
        })
        .collect()
}

/// A release as `0.5.0` or `0.5.0-dev` (a dev build comes before its release).
fn release_key(r: &str) -> Option<(u32, u32, u32, bool)> {
    let (v, dev) = match r.split_once('-') {
        Some((v, _)) => (v, true),
        None => (r, false),
    };
    let mut parts = v.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??, !dev))
}

/// Whether `release` is `fixed_in` or later.
fn since_fix(release: &str, fixed_in: &str) -> bool {
    match (release_key(release), release_key(fixed_in)) {
        (Some(r), Some(f)) => r >= f,
        _ => false,
    }
}

struct Mark {
    status: String,
    fixed_in: String,
    note: String,
    by: String,
    at: i64,
}

#[derive(Default)]
struct Group {
    kind: String,
    count: i64,
    fatal: i64,
    first: i64,
    last: i64,
    players: BTreeSet<(String, String)>,
    names: BTreeSet<String>,
    releases: BTreeSet<String>,
    /// The players seen on each release.
    by_release: BTreeMap<String, BTreeSet<(String, String)>>,
    servers: BTreeSet<String>,
    /// The latest one's details.
    latest: Value,
    /// How many came from reports players sent, not from their games (CT).
    from_reports: i64,
    /// (server, player, at) of each, for the reports around them.
    hits: Vec<(String, i64, i64)>,
}

/// A list of strings as one (a string as it is).
fn joined(v: &Value, by: &str) -> String {
    match v {
        Value::Array(items) => items.iter().filter_map(|i| i.as_str()).collect::<Vec<_>>().join(by),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// A place as the game server lets it through: `module+0xhex`, or a bare `0xhex`. Others
/// (from a server that doesn't check) are left out.
fn valid_place(place: &str) -> bool {
    let hex = |h: &str| (1..=8).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit());
    match place.split_once("+0x") {
        Some((module, offset)) => (1..=64).contains(&module.len()) && module.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')) && hex(offset),
        None => place.strip_prefix("0x").is_some_and(hex),
    }
}

/// A fixed crash seen again by this many players is back (one could be anyone's say-so).
const BACK_PLAYERS: usize = 2;

/// One event's signature: where (and "freeze at" for a freeze).
fn signature(d: &Value) -> String {
    let place = d["place"].as_str().unwrap_or("?");
    if d["kind"] == "freeze" {
        format!("freeze at {place}")
    } else {
        place.to_string()
    }
}

impl Coordinator {
    async fn crash_marks(&self) -> sqlx::Result<HashMap<String, Mark>> {
        let rows: Vec<(String, String, String, String, String, i64)> = sqlx::query_as("SELECT signature, status, fixed_in, note, marked_by, marked_at FROM crash_marks")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|(sig, status, fixed_in, note, by, at)| (sig, Mark { status, fixed_in, note, by, at }))
            .collect())
    }

    /// The crash events since `since`, grouped by signature.
    async fn crash_groups(&self, since: i64) -> sqlx::Result<BTreeMap<String, Group>> {
        let rows: Vec<(String, Option<i64>, Option<String>, i64, i64, String)> =
            sqlx::query_as("SELECT server_id, player, name, at, count, detail FROM session_events WHERE kind = 'crash' AND at >= ? ORDER BY at")
                .bind(since)
                .fetch_all(&self.pool)
                .await?;
        let rows: Vec<(String, Option<i64>, Option<String>, i64, i64, Value)> = rows
            .into_iter()
            .filter_map(|(server, player, name, at, count, detail)| Some((server, player, name, at, count, serde_json::from_str::<Value>(&detail).ok()?)))
            .collect();
        // When each player's game sent a crash at each place (the crash's own time), for the
        // ones a report carried as well.
        let mut sent: HashMap<(String, i64, String), Vec<i64>> = HashMap::new();
        for (server, player, _, at, _, d) in &rows {
            if let (Some(p), None) = (player, d.get("report")) {
                sent.entry((server.clone(), *p, signature(d))).or_default().push(d["at"].as_i64().unwrap_or(*at));
            }
        }
        let mut from_reports_seen: BTreeSet<(String, Option<i64>, String, i64)> = BTreeSet::new();
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for (server, player, name, at, count, d) in rows {
            if !d["place"].as_str().is_some_and(valid_place) {
                continue;
            }
            let from_report = d.get("report").is_some();
            if from_report {
                let crash_at = d["at"].as_i64().unwrap_or(at);
                let game_sent = player.is_some_and(|p| {
                    sent.get(&(server.clone(), p, signature(&d)))
                        .is_some_and(|times| times.iter().any(|t| (t - crash_at).abs() <= SAME_CRASH_WITHIN))
                });
                if game_sent || !from_reports_seen.insert((server.clone(), player, signature(&d), crash_at)) {
                    continue;
                }
            }
            let g = groups.entry(signature(&d)).or_default();
            g.kind = d["kind"].as_str().unwrap_or("exception").to_string();
            g.count += count;
            if from_report {
                g.from_reports += count;
            }
            if d["fatal"] == true {
                g.fatal += count;
            }
            if g.first == 0 || at < g.first {
                g.first = at;
            }
            g.last = g.last.max(at);
            let who = player.map_or_else(|| name.clone().unwrap_or_default(), |p| p.to_string());
            g.players.insert((server.clone(), who.clone()));
            if let Some(n) = name.as_ref().filter(|n| !n.is_empty()) {
                g.names.insert(n.clone());
            }
            if let Some(r) = d["release"].as_str().filter(|r| !r.is_empty()) {
                g.releases.insert(r.to_string());
                g.by_release.entry(r.to_string()).or_default().insert((server.clone(), who));
            }
            g.servers.insert(server.clone());
            if let Some(p) = player {
                g.hits.push((server.clone(), p, at));
            }
            g.latest = json!({
                "at": at,
                "server": server,
                "name": name,
                "code": d["code"],
                "stack": joined(&d["stack"], ", "),
                "registers": joined(&d["registers"], "; "),
                "callers": joined(&d["callers"], ", "),
                "doing": d["doing"],
                "release": d["release"],
                "fatal": d["fatal"],
                "report": d["report"],
            });
        }
        Ok(groups)
    }

    /// The crashes in the game logs of report `report` (sent by `player`, `name`, on `server`),
    /// noted as `crash` events (CT). Answers how many were new.
    pub(crate) async fn crashes_from_report(&self, server: &str, report: &str, player: i64, name: &str, created_at: i64, logs: Vec<String>) -> sqlx::Result<u64> {
        let id = report.to_string();
        let found = tokio::task::spawn_blocking(move || crashes_in_logs(&logs, &id, created_at)).await.unwrap_or_default();
        let oldest = identity::now() - LONGEST_DAYS * 86_400;
        let mut added = 0;
        for (n, (at, detail)) in found.into_iter().enumerate() {
            if at < oldest {
                continue;
            }
            added += sqlx::query("INSERT OR IGNORE INTO session_events (server_id, id, at, last_at, player, name, kind, detail, count) VALUES (?, ?, ?, ?, ?, ?, 'crash', ?, 1)")
                .bind(server)
                .bind(report_crash_id(report, n))
                .bind(at)
                .bind(at)
                .bind(player)
                .bind(name)
                .bind(detail.to_string())
                .execute(&self.pool)
                .await?
                .rows_affected();
        }
        Ok(added)
    }

    /// The game logs of a stored report, from disk.
    async fn report_logs(&self, report: &str) -> Vec<String> {
        let mut logs = Vec::new();
        for name in ["bl-tracing.log", "bl-tracing.prev.log"] {
            if let Ok(Some(Some(text))) = self.report_file(report, name).await {
                logs.push(String::from_utf8_lossy(&text).into_owned());
            }
        }
        logs
    }

    /// Once: the crashes in the reports already here (the last 30 days'), for the first start
    /// with CT.
    pub async fn crashes_from_stored_reports(&self) -> sqlx::Result<()> {
        let done: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(FROM_REPORTS_DONE)
            .fetch_optional(&self.pool)
            .await?;
        if done.is_some() {
            return Ok(());
        }
        let since = identity::now() - LONGEST_DAYS * 86_400;
        let reports: Vec<(String, String, i64, String, i64)> = sqlx::query_as(
            "SELECT id, server_id, player_id, player_name, created_at FROM player_reports WHERE created_at >= ? AND id IN (SELECT report_id FROM player_report_files WHERE name LIKE 'bl-tracing%')",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        let mut added = 0;
        for (id, server, player, name, created_at) in &reports {
            let logs = self.report_logs(id).await;
            if !logs.is_empty() {
                added += self.crashes_from_report(server, id, *player, name, *created_at, logs).await?;
            }
        }
        tracing::info!("crashes from the reports already here: {added} from {} reports", reports.len());
        self.set_setting(FROM_REPORTS_DONE, "1").await
    }

    /// The Crashes page: the groups of the last `days` days, the crash fixes' counts, and the
    /// marks.
    pub(crate) async fn crashes_view(&self, days: i64) -> sqlx::Result<Value> {
        let days = days.clamp(1, LONGEST_DAYS);
        let since = identity::now() - days * 86_400;
        let groups = self.crash_groups(since).await?;
        let marks = self.crash_marks().await?;
        let servers: HashMap<String, String> = sqlx::query_as::<_, (String, Option<String>)>("SELECT id, json_extract(listing, '$.name') FROM servers")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|(id, name)| (id.clone(), name.unwrap_or(id)))
            .collect();
        let mut out = Vec::new();
        let (mut total, mut fatal) = (0, 0);
        let mut everyone = BTreeSet::new();
        for (sig, g) in &groups {
            total += g.count;
            fatal += g.fatal;
            everyone.extend(g.players.iter().cloned());
            // The reports these players sent soon after.
            let mut reports = Vec::new();
            for (server, player, at) in g.hits.iter().rev() {
                if reports.len() >= MOST_NAMED {
                    break;
                }
                let found: Vec<(String,)> =
                    sqlx::query_as("SELECT id FROM player_reports WHERE server_id = ? AND player_id = ? AND created_at BETWEEN ? AND ? ORDER BY created_at LIMIT 2")
                        .bind(server)
                        .bind(player)
                        .bind(at)
                        .bind(at + REPORT_WITHIN)
                        .fetch_all(&self.pool)
                        .await?;
                for (id,) in found {
                    if !reports.contains(&id) {
                        reports.push(id);
                    }
                }
            }
            let mark = marks.get(sig);
            let again: Vec<&String> = mark
                .filter(|m| m.status == "fixed")
                .map(|m| g.releases.iter().filter(|r| since_fix(r, &m.fixed_in)).collect())
                .unwrap_or_default();
            out.push(json!({
                "signature": sig,
                "kind": g.kind,
                "count": g.count,
                "fatal": g.fatal,
                "from_reports": g.from_reports,
                "players": g.players.len(),
                "names": g.names.iter().take(MOST_NAMED).collect::<Vec<_>>(),
                "first": g.first,
                "last": g.last,
                "releases": g.releases,
                "servers": g.servers.iter().map(|s| servers.get(s).cloned().unwrap_or_else(|| s.clone())).collect::<Vec<_>>(),
                "latest": g.latest,
                "reports": reports,
                "mark": mark.map(|m| json!({ "status": m.status, "fixed_in": m.fixed_in, "note": m.note, "by": m.by, "at": m.at })),
                "seen_since_fix": again,
            }));
        }
        out.sort_by(|a, b| (b["players"].as_u64(), b["count"].as_i64(), b["last"].as_i64()).cmp(&(a["players"].as_u64(), a["count"].as_i64(), a["last"].as_i64())));
        // Marks for crashes not seen in the range, so the list of what's known stays whole.
        let unseen: Vec<Value> = marks
            .iter()
            .filter(|(sig, _)| !groups.contains_key(*sig))
            .map(|(sig, m)| json!({ "signature": sig, "status": m.status, "fixed_in": m.fixed_in, "note": m.note, "by": m.by, "at": m.at }))
            .collect();
        // The crash events are a sample: only games with diagnostics on send theirs, and those
        // send what the crash fixes did on each start (`crash_fixes`).
        let players_in = |kind: &'static str| {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(DISTINCT server_id || ':' || player) FROM session_events WHERE kind = ? AND at >= ? AND player IS NOT NULL")
                .bind(kind)
                .bind(since)
                .fetch_one(&self.pool)
        };
        let signed_in = players_in("signin").await?;
        let sending = players_in("crash_fixes").await?.min(signed_in);
        Ok(json!({
            "since": since,
            "days": days,
            "crashes": total,
            "fatal": fatal,
            "players": everyone.len(),
            "diagnostics": { "sending": sending, "signed_in": signed_in },
            "groups": out,
            "fixes": self.fix_counts(since).await?,
            "marked_unseen": unseen,
        }))
    }

    /// What the crash fixes did since `since`: per fix, the times it kept a game going and the
    /// players whose games it did; per fix and build, the runs it went in and didn't.
    async fn fix_counts(&self, since: i64) -> sqlx::Result<Value> {
        let rows: Vec<(String, Option<i64>, i64, String)> = sqlx::query_as("SELECT server_id, player, count, detail FROM session_events WHERE kind = 'crash_fixes' AND at >= ?")
            .bind(since)
            .fetch_all(&self.pool)
            .await?;
        #[derive(Default)]
        struct Fix {
            saves: i64,
            players: BTreeSet<(String, i64)>,
            runs: i64,
            missing: BTreeMap<String, i64>,
        }
        let mut fixes: BTreeMap<String, Fix> = BTreeMap::new();
        for (server, player, count, detail) in rows {
            let Ok(d) = serde_json::from_str::<Value>(&detail) else { continue };
            let build = format!("{} {}", d["exe"].as_str().unwrap_or("?"), d["release"].as_str().unwrap_or("?"));
            let names = |key: &str| {
                d[key]
                    .as_array()
                    .map(|l| l.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>())
                    .unwrap_or_default()
            };
            // "perception-fix:2": that fix kept the game going twice since the last count.
            for s in names("saves") {
                let Some((name, n)) = s.rsplit_once(':') else { continue };
                let saves = n.parse::<i64>().unwrap_or(0) * count;
                if saves > 0 {
                    let x = fixes.entry(name.to_string()).or_default();
                    x.saves += saves;
                    if let Some(p) = player {
                        x.players.insert((server.clone(), p));
                    }
                }
            }
            if d["first"] == true {
                for name in names("installed") {
                    fixes.entry(name).or_default().runs += count;
                }
                for name in names("missing") {
                    let x = fixes.entry(name).or_default();
                    x.runs += count;
                    *x.missing.entry(build.clone()).or_default() += count;
                }
            }
        }
        Ok(Value::Array(
            fixes
                .into_iter()
                .map(|(name, f)| json!({ "name": name, "saves": f.saves, "players": f.players.len(), "runs": f.runs, "missing": f.missing }))
                .collect(),
        ))
    }

    /// Marks a group (`status` empty clears it).
    pub(crate) async fn mark_crash(&self, signature: &str, status: &str, fixed_in: &str, note: &str, by: &str) -> sqlx::Result<()> {
        if status.is_empty() {
            sqlx::query("DELETE FROM crash_marks WHERE signature = ?").bind(signature).execute(&self.pool).await?;
        } else {
            sqlx::query(
                "INSERT INTO crash_marks (signature, status, fixed_in, note, marked_by, marked_at) VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT (signature) DO UPDATE SET status = excluded.status, fixed_in = excluded.fixed_in, note = excluded.note, marked_by = excluded.marked_by, marked_at = excluded.marked_at",
            )
            .bind(signature)
            .bind(status)
            .bind(fixed_in)
            .bind(note)
            .bind(by)
            .bind(identity::now())
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// The alerts' crash conditions: a crash not marked that hit [`ALERT_PLAYERS`] players in
    /// the last day, or a fixed one seen again from its fix's release on. Keyed by signature.
    pub(crate) async fn crash_conditions(&self) -> sqlx::Result<Vec<(String, String, &'static str, String)>> {
        let groups = self.crash_groups(identity::now() - 86_400).await?;
        let marks = self.crash_marks().await?;
        let mut found = Vec::new();
        for (sig, g) in groups {
            let what = if g.kind == "freeze" { "freeze" } else { "crash" };
            // Where it crashed, as the players' games said: shown as code in the webhook's
            // channel, so on one line, cut, and with no backtick to end the code early.
            let at: String = sig.split_whitespace().collect::<Vec<_>>().join(" ").replace('`', "'").chars().take(120).collect();
            match marks.get(&sig) {
                None if g.players.len() >= ALERT_PLAYERS => found.push((
                    "crash".to_string(),
                    sig.clone(),
                    "warn",
                    format!(
                        "A new {what} at `{at}` hit {} players in the last day ({} times, {} ended the game)",
                        g.players.len(),
                        g.count,
                        g.fatal
                    ),
                )),
                Some(m) if m.status == "fixed" => {
                    let again: Vec<&String> = g.releases.iter().filter(|r| since_fix(r, &m.fixed_in)).collect();
                    let players: BTreeSet<&(String, String)> = again.iter().filter_map(|r| g.by_release.get(*r)).flatten().collect();
                    if players.len() >= BACK_PLAYERS {
                        found.push((
                            "crash".to_string(),
                            sig.clone(),
                            "bad",
                            format!(
                                "The {what} at `{at}`, fixed in {}, came back: {} times in the last day on {}",
                                m.fixed_in,
                                g.count,
                                again.iter().take(8).map(|r| crate::reports::chat_safe(r, 32)).collect::<Vec<_>>().join(", ")
                            ),
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_compare_with_dev_builds_before_their_release() {
        assert!(since_fix("0.5.0", "0.5.0"));
        assert!(since_fix("0.5.1", "0.5.0"));
        assert!(since_fix("0.10.0", "0.5.0"));
        assert!(!since_fix("0.5.0-dev", "0.5.0"));
        assert!(!since_fix("0.4.7", "0.5.0"));
        assert!(!since_fix("", "0.5.0"));
        assert!(!since_fix("0.5.0", "soon"));
    }

    #[test]
    fn places_are_a_module_and_offset() {
        assert!(valid_place("Blacklist_DX11_game.exe+0x6f205a"));
        assert!(valid_place("0x1e41f20"));
        assert!(!valid_place("**look** [here](https://x.example)"));
        assert!(!valid_place("x.exe+0xzz"));
    }

    #[test]
    fn a_freeze_is_its_own_group() {
        assert_eq!(signature(&json!({"kind": "exception", "place": "x.exe+0x1"})), "x.exe+0x1");
        assert_eq!(signature(&json!({"kind": "freeze", "place": "x.exe+0x1"})), "freeze at x.exe+0x1");
    }
}
