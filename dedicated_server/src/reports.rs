//! Players' feedback and problem reports. After a game session the launcher asks this server
//! what it saw of it ([`summary`]: failed joins, the relay, matches), and when something went
//! wrong (or now and then) asks the player how it went. Their answer comes back with their
//! logs if they agreed, redacted on their PC ([`accept`]); this server adds what it logged
//! about them (`recent_log.rs`) and its summary, and passes the report on to its coordinator
//! (`report_outbox`, sent by `federation.rs`), where the network's admins read it.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::OnceLock;

use serde_json::json;
use serde_json::Value;

use crate::storage::Storage;

/// What the game reports when the host refuses a join as another version of the game
/// (`DATA_VERSION_MISMATCH`).
pub const VERSION_MISMATCH: u32 = 0xeea4_40ee;
/// The problems a player can tick.
pub const PROBLEMS: [&str; 7] = ["join", "lag", "crash", "connection", "version", "signin", "other"];
/// The most files, of each and of all of them (compressed), and the longest comment.
const MAX_FILES: usize = 8;
const MAX_FILE: u64 = 4 * 1024 * 1024;
const MAX_FILES_GZIP: usize = 6 * 1024 * 1024;
const MAX_COMMENT: usize = 2000;
/// Reports a player may send a day.
const PER_DAY: i64 = 5;
/// Reports waiting for the coordinator, in bytes, past which new ones are refused (the
/// coordinator away for long, or someone filling the queue).
const MAX_OUTBOX: i64 = 200 * 1024 * 1024;
/// The most of the server's log sent with a report: a long session's worth (the
/// coordinator takes up to a megabyte).
const MAX_SERVER_LOG: usize = 256 * 1024;
/// What the server keeps of each player's recent events.
const EVENTS_KEPT: usize = 50;
const EVENTS_FOR_SECS: i64 = 6 * 3600;

/// Something that happened to a player, for their next report.
#[derive(Debug, Clone)]
struct Event {
    at: i64,
    kind: &'static str,
    detail: String,
}

fn events() -> &'static Mutex<HashMap<u32, VecDeque<Event>>> {
    static EVENTS: OnceLock<Mutex<HashMap<u32, VecDeque<Event>>>> = OnceLock::new();
    EVENTS.get_or_init(Mutex::default)
}

/// Whether each player (by name, lowercase) was relayed when the NAT helper last saw them.
fn relayed_names() -> &'static Mutex<HashMap<String, (i64, bool)>> {
    static RELAYED: OnceLock<Mutex<HashMap<String, (i64, bool)>>> = OnceLock::new();
    RELAYED.get_or_init(Mutex::default)
}

fn note(user: u32, kind: &'static str, detail: String) {
    let now = identity::now();
    let mut all = events().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let list = all.entry(user).or_default();
    while list.len() >= EVENTS_KEPT || list.front().is_some_and(|e| now - e.at > EVENTS_FOR_SECS) {
        list.pop_front();
    }
    list.push_back(Event { at: now, kind, detail });
    // Now and then, players gone for a while.
    if all.len() > 2000 {
        all.retain(|_, l| l.back().is_some_and(|e| now - e.at <= EVENTS_FOR_SECS));
    }
}

/// `user`'s game reported a join failing with `code`.
pub fn join_failed(user: u32, session: u32, code: u32) {
    note(
        user,
        if code == VERSION_MISMATCH { "version_mismatch" } else { "failed_join" },
        format!("session {session}, code {code:#010x}"),
    );
}

/// The NAT helper registered `name`, relayed or not.
pub fn nat_registered(name: &str, relayed: bool) {
    let mut all = relayed_names().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if all.len() > 5000 {
        all.clear();
    }
    all.insert(name.to_lowercase(), (identity::now(), relayed));
}

/// What the server saw of a player's last game session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub started: i64,
    pub ended: i64,
    pub failed_joins: i32,
    pub version_mismatches: i32,
    pub relayed: bool,
    pub matches: i32,
    pub json: Value,
}

/// `user`'s last play session and what happened in it.
pub fn summary(storage: &Storage, user: u32) -> eyre::Result<Summary> {
    let record = storage.player_records(Some(&[user]))?.pop();
    let (started, ended) = storage.last_play_session(user)?.unwrap_or_default();
    // From a little before the session: a refused join can come before the game connects.
    let since = if started > 0 { started - 600 } else { identity::now() - EVENTS_FOR_SECS };
    let list: Vec<Event> = events()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&user)
        .map(|l| l.iter().filter(|e| e.at >= since).cloned().collect())
        .unwrap_or_default();
    let name = record.as_ref().map(|r| r.name.clone()).unwrap_or_default();
    let relayed = relayed_names()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&name.to_lowercase())
        .filter(|(at, _)| *at >= since)
        .is_some_and(|(_, r)| *r);
    let count = |kind: &str| i32::try_from(list.iter().filter(|e| e.kind == kind).count()).unwrap_or(i32::MAX);
    let failed_joins = count("failed_join") + count("version_mismatch");
    let version_mismatches = count("version_mismatch");
    let matches = record.as_ref().map_or(0, |r| i32::try_from(r.matches).unwrap_or(i32::MAX));
    let json = json!({
        "server_version": crate::community_api::RELEASE,
        "player": record.as_ref().map(|r| json!({ "id": r.id, "name": r.name, "online": r.online, "play_seconds": r.play_seconds, "sessions": r.sessions, "matches": r.matches, "banned": r.banned.is_some() })),
        "session": { "started": started, "ended": if ended == 0 { Value::Null } else { json!(ended) } },
        "relayed": relayed,
        "failed_joins": failed_joins,
        "version_mismatches": version_mismatches,
        "events": list.iter().map(|e| json!({ "at": e.at, "kind": e.kind, "detail": e.detail })).collect::<Vec<_>>(),
    });
    Ok(Summary {
        started,
        ended,
        failed_joins,
        version_mismatches,
        relayed,
        matches,
        json,
    })
}

/// A report as the launcher sent it.
pub struct Incoming {
    pub rating: String,
    pub problems: Vec<String>,
    pub comment: String,
    pub triggers: Vec<String>,
    pub client: HashMap<String, String>,
    /// Name, gzip, uncompressed size.
    pub files: Vec<(String, Vec<u8>, u64)>,
}

/// Why a report was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    TooMany,
    Invalid(&'static str),
}

fn printable(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect()
}

fn valid_file_name(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// Checks a report and turns it into what the coordinator takes (`POST /v1/reports`).
pub fn body(report: Incoming, player: Value, summary: &Value, server_log: String) -> Result<(String, Value), Refused> {
    if report.files.len() > MAX_FILES {
        return Err(Refused::Invalid("too many files"));
    }
    if report.files.iter().map(|(_, gz, _)| gz.len()).sum::<usize>() > MAX_FILES_GZIP {
        return Err(Refused::Invalid("the files are too big"));
    }
    let mut files = Vec::new();
    for (name, gzip, size) in &report.files {
        if !valid_file_name(name) || *size > MAX_FILE || gzip.len() < 18 || gzip[..2] != [0x1f, 0x8b] {
            return Err(Refused::Invalid("a file isn't a log the launcher sends"));
        }
        files.push(json!({ "name": name, "gzip_base64": sodiumoxide::base64::encode(gzip, sodiumoxide::base64::Variant::Original), "size": size }));
    }
    let rating = match report.rating.as_str() {
        "good" | "bad" => json!(report.rating),
        _ => Value::Null,
    };
    let problems: Vec<&str> = PROBLEMS.iter().copied().filter(|p| report.problems.iter().any(|q| q == p)).collect();
    let triggers: Vec<String> = report.triggers.iter().take(16).map(|t| printable(t, 40)).filter(|t| !t.is_empty()).collect();
    let client: serde_json::Map<String, Value> = report
        .client
        .iter()
        .filter(|(k, _)| ["launcher", "client", "build", "os", "language"].contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), json!(printable(v, 64))))
        .collect();
    let id: String = {
        let mut bytes = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    };
    let body = json!({
        "id": id,
        "created_at": identity::now(),
        "player": player,
        "rating": rating,
        "problems": problems,
        "comment": printable(report.comment.trim(), MAX_COMMENT),
        "triggers": triggers,
        "client": client,
        "summary": summary,
        "files": files,
        "server_log": server_log,
    });
    Ok((id, body))
}

/// Takes `user`'s report, adds this server's side and queues it for the coordinator.
/// Answers the report's id.
/// `auto`: the game's log the server asked for, which doesn't count towards the player's
/// own reports a day (full_logs.rs limits those).
pub async fn accept(storage: &Storage, user: u32, peer: Option<std::net::IpAddr>, report: Incoming, auto: bool) -> eyre::Result<Result<String, Refused>> {
    if (!auto && storage.reports_today(user).await? >= PER_DAY) || storage.report_outbox_bytes().await? > MAX_OUTBOX {
        return Ok(Err(Refused::TooMany));
    }
    let person = storage.find_person(user).await?.ok_or_else(|| eyre::eyre!("no such player"))?;
    let summary = {
        let storage_ref = storage;
        // `summary` reads with the storage's blocking calls.
        tokio::task::block_in_place(|| summary(storage_ref, user))?
    };
    let since = if summary.started > 0 { summary.started - 600 } else { identity::now() - 3600 };
    let addresses: Vec<String> = peer.into_iter().chain(crate::metrics::address_of(user)).map(|ip| ip.to_canonical().to_string()).collect();
    let server_log = crate::recent_log::about(user, &person.username, &addresses, since, MAX_SERVER_LOG);
    let player = json!({ "id": user, "name": person.username, "identity": person.global_id });
    let (id, body) = match body(report, player, &summary.json, server_log) {
        Ok(b) => b,
        Err(refused) => return Ok(Err(refused)),
    };
    storage.queue_report(user, &id, &body.to_string(), !auto).await?;
    Ok(Ok(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gzip(text: &str) -> Vec<u8> {
        use std::io::Write as _;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(text.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    fn report(files: Vec<(String, Vec<u8>, u64)>) -> Incoming {
        Incoming {
            rating: "bad".into(),
            problems: vec!["join".into(), "nonsense".into(), "lag".into()],
            comment: "couldn't join\\u{7}my friend".into(),
            triggers: vec!["failed_join".into()],
            client: HashMap::from([("launcher".into(), "0.4.1".into()), ("secret".into(), "x".into())]),
            files,
        }
    }

    #[test]
    fn reports_are_checked_and_shaped_for_the_coordinator() {
        let log = gzip("2026 INFO attaching");
        let (id, b) = body(report(vec![("bl-tracing.log".into(), log, 20)]), json!({ "id": 7 }), &json!({}), "line".into()).unwrap();
        assert_eq!(id.len(), 32);
        assert_eq!(b["id"], json!(id));
        assert_eq!(b["problems"], json!(["join", "lag"]), "only the known problems");
        assert_eq!(b["client"], json!({ "launcher": "0.4.1" }), "only the known client fields");
        assert_eq!(b["rating"], json!("bad"));
        assert_eq!(b["files"][0]["name"], json!("bl-tracing.log"));
        assert!(b["files"][0]["gzip_base64"].as_str().unwrap().starts_with("H4sI"), "gzip, base64");

        let bad_name = report(vec![("../x".into(), gzip("x"), 1)]);
        assert_eq!(
            body(bad_name, json!({}), &json!({}), String::new()).unwrap_err(),
            Refused::Invalid("a file isn't a log the launcher sends")
        );
        let not_gzip = report(vec![("a.log".into(), b"plain text, not gzip at all".to_vec(), 1)]);
        assert!(body(not_gzip, json!({}), &json!({}), String::new()).is_err());
        let too_many = report((0..9).map(|i| (format!("f{i}.log"), gzip("x"), 1)).collect());
        assert_eq!(body(too_many, json!({}), &json!({}), String::new()).unwrap_err(), Refused::Invalid("too many files"));
    }

    #[test]
    fn a_version_mismatch_is_told_apart() {
        join_failed(424_242, 9, VERSION_MISMATCH);
        join_failed(424_242, 9, 0xb08a_1a05);
        let list = events().lock().unwrap().get(&424_242).cloned().unwrap();
        assert_eq!(list.iter().map(|e| e.kind).collect::<Vec<_>>(), ["version_mismatch", "failed_join"]);
    }
}
