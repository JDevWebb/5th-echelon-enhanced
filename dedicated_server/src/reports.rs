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
/// `REFUSED`: the host refused, most often because the room is full at its current lobby
/// size (hosts can raise it; "Maximum number of peers reached."), also while it's leaving or
/// migrating. Seen on co-op rooms still at their 2 seats.
pub const ROOM_FULL: u32 = 0xcf96_4066;
/// `LOCKED_SESSION`: seen when the game reports its own just-closed party, or a match that
/// had started.
pub const LOCKED_SESSION: u32 = 0x9718_2b1b;
/// `INVALID_SESSION_ID`: the room had just been closed.
pub const ROOM_CLOSED: u32 = 0xe92f_bc33;
/// The room going as the player joined: its host `LEAVING`, the session `DISSOLVED`, or its
/// `HOST_GONE` (no new host). The game's codes are the CRC-32 of these names.
const ROOM_GOING: [u32; 3] = [0x33ae_7adc, 0x546d_52c6, 0x59d6_f9cc];
/// `PRIVATE_SESSION`: invite only.
const PRIVATE_SESSION: u32 = 0xbb26_3fc5;
/// The game's own `VERSION_MISMATCH`: the host's answer couldn't be read with this game's data.
const STORM_VERSION_MISMATCH: u32 = 0x9bf4_cf37;

/// How a join the game reported failing went, from its error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinOutcome {
    Full,
    Locked,
    Closed,
    DataMismatch,
    Failed,
}

impl JoinOutcome {
    pub fn of(code: u32) -> Self {
        match code {
            ROOM_FULL => Self::Full,
            LOCKED_SESSION | PRIVATE_SESSION => Self::Locked,
            ROOM_CLOSED => Self::Closed,
            c if ROOM_GOING.contains(&c) => Self::Closed,
            VERSION_MISMATCH | STORM_VERSION_MISMATCH => Self::DataMismatch,
            // CONNECTION_FAILED, CONNECTION_TIMED_OUT, SESSION_JOIN_TIMED_OUT, TICKET_PENDING and
            // the rest: the join itself failed.
            _ => Self::Failed,
        }
    }

    /// As session events name it (`join_failed`'s `outcome`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Locked => "locked",
            Self::Closed => "closed",
            Self::DataMismatch => "data_mismatch",
            Self::Failed => "failed",
        }
    }

    /// Whether it counts as the player's failed join (for the launcher's "how did it go"):
    /// a full lobby is the host's choice of size, and a retry gets in once a seat frees up.
    pub fn counts(self) -> bool {
        self != Self::Full
    }

    /// As the overlay's notice names it.
    pub fn problem(self) -> server_api::notices::JoinProblem {
        use server_api::notices::JoinProblem;
        match self {
            Self::Full => JoinProblem::Full,
            Self::Locked => JoinProblem::Locked,
            Self::Closed => JoinProblem::Closed,
            Self::DataMismatch => JoinProblem::DataMismatch,
            Self::Failed => JoinProblem::Failed,
        }
    }

    /// Whether it's worth the game's log: a full, locked or closed room says all there is.
    pub fn wants_log(self) -> bool {
        matches!(self, Self::DataMismatch | Self::Failed)
    }

    /// From a `join_failed` event's detail: its outcome, or the code's for events noted
    /// before outcomes were.
    pub fn of_detail(detail: &Value) -> Self {
        match detail["outcome"].as_str() {
            Some("full") => Self::Full,
            Some("locked") => Self::Locked,
            Some("closed") => Self::Closed,
            Some("data_mismatch") => Self::DataMismatch,
            Some(_) => Self::Failed,
            None => detail["code"]
                .as_str()
                .and_then(|c| u32::from_str_radix(c.trim_start_matches("0x"), 16).ok())
                .map_or(Self::Failed, Self::of),
        }
    }
}

/// Both sides' game data (attribute 107) in a join refused as a different version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mismatch {
    /// The name of the player whose game refused the join, if known.
    pub host: Option<String>,
    pub mine: Option<u32>,
    pub theirs: Option<u32>,
}

/// A data version as a player reads it: what makes it when known, else hex.
pub(crate) fn data_shown(version: Option<u32>) -> String {
    version.map_or_else(String::new, |v| server_api::data_versions::name(v).map_or_else(|| format!("{v:#010x}"), str::to_string))
}
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
    /// Its number, for the notice that explains it ([`tell`]).
    id: u64,
    at: i64,
    kind: &'static str,
    detail: String,
    mismatch: Option<Mismatch>,
    /// The game's overlay showed the notice explaining it ([`take_notice`]).
    told: bool,
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

fn note(user: u32, kind: &'static str, detail: String, mismatch: Option<Mismatch>) -> u64 {
    static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let now = identity::now();
    let mut all = events().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let list = all.entry(user).or_default();
    while list.len() >= EVENTS_KEPT || list.front().is_some_and(|e| now - e.at > EVENTS_FOR_SECS) {
        list.pop_front();
    }
    list.push_back(Event {
        id,
        at: now,
        kind,
        detail,
        mismatch,
        told: false,
    });
    // Now and then, players gone for a while.
    if all.len() > 2000 {
        all.retain(|_, l| l.back().is_some_and(|e| now - e.at <= EVENTS_FOR_SECS));
    }
    id
}

/// A notice waiting for a player's game ([`tell`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    at: i64,
    title: String,
    text: String,
    /// The events it explains, marked told once the game has it: the same notice again (a
    /// game retrying its join) explains its events too.
    explains: Vec<u64>,
}

/// How long a notice waits for the game: past that, the player has moved on.
const NOTICE_FRESH: i64 = 60;
/// The most notices waiting for one player.
const NOTICES_KEPT: usize = 3;

fn notices() -> &'static Mutex<HashMap<u32, VecDeque<Notice>>> {
    static NOTICES: OnceLock<Mutex<HashMap<u32, VecDeque<Notice>>>> = OnceLock::new();
    NOTICES.get_or_init(Mutex::default)
}

/// Tells `user`'s game, for its overlay, what just went wrong: `explains` is the event it
/// explains (so the launcher needn't ask about it). The same notice again (a game retrying
/// its join) replaces the one waiting.
pub fn tell(user: u32, title: String, text: String, explains: Option<u64>) {
    let now = identity::now();
    let mut all = notices().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let list = all.entry(user).or_default();
    list.retain(|n| now - n.at <= NOTICE_FRESH);
    let mut explains: Vec<u64> = explains.into_iter().collect();
    if let Some(same) = list.iter().position(|n| n.title == title && n.text == text) {
        if let Some(earlier) = list.remove(same) {
            explains.extend(earlier.explains);
        }
    }
    while list.len() >= NOTICES_KEPT {
        list.pop_front();
    }
    list.push_back(Notice { at: now, title, text, explains });
    if all.len() > 2000 {
        all.retain(|_, l| l.back().is_some_and(|n| now - n.at <= NOTICE_FRESH));
    }
}

/// The oldest fresh notice for `user`'s game (title, text), if any; the event it explains is
/// then told.
pub fn take_notice(user: u32) -> Option<(String, String)> {
    let now = identity::now();
    let notice = {
        let mut all = notices().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let list = all.get_mut(&user)?;
        list.retain(|n| now - n.at <= NOTICE_FRESH);
        let notice = list.pop_front();
        if list.is_empty() {
            all.remove(&user);
        }
        notice?
    };
    if !notice.explains.is_empty() {
        let mut all = events().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(list) = all.get_mut(&user) {
            for event in list.iter_mut().filter(|e| notice.explains.contains(&e.id)) {
                event.told = true;
            }
        }
    }
    Some((notice.title, notice.text))
}

/// `user`'s game reported a join failing with `code`: one that counts as theirs (not their
/// game's own churn, see `game_session.rs`, nor a full lobby, [`JoinOutcome::counts`]).
/// `outcome` as the server made it out (a join that failed between games with different data
/// is a mismatch, whatever its code); `mismatch`: both sides' data, for a mismatch.
pub fn join_failed(user: u32, session: u32, code: u32, outcome: JoinOutcome, mismatch: Option<Mismatch>) -> u64 {
    let version = outcome == JoinOutcome::DataMismatch;
    note(
        user,
        if version { "version_mismatch" } else { "failed_join" },
        format!("session {session}, code {code:#010x}"),
        mismatch.filter(|_| version),
    )
}

/// How long one note of lobbies hidden by game data stands for the searches after it: the
/// game searches every few seconds while it looks for a match, which would crowd everything
/// else out of the player's events.
const SEARCH_NOTED_FOR: i64 = 60;

/// `user`'s matchmaking search found nothing, but `lobbies` open lobbies would have answered
/// it but for their game data: `mine`, theirs most often `theirs`.
/// The event's number when noted (not within [`SEARCH_NOTED_FOR`] of the last).
pub fn search_other_data(user: u32, mine: u32, theirs: u32, lobbies: usize) -> Option<u64> {
    let now = identity::now();
    {
        let all = events().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let lately = all
            .get(&user)
            .is_some_and(|l| l.iter().rev().any(|e| e.kind == "search_mismatch" && now - e.at < SEARCH_NOTED_FOR));
        if lately {
            return None;
        }
    }
    Some(note(
        user,
        "search_mismatch",
        format!("{lobbies} lobbies with data {theirs:#010x}, this game's {mine:#010x}"),
        Some(Mismatch {
            host: None,
            mine: Some(mine),
            theirs: Some(theirs),
        }),
    ))
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
    /// The last join refused as a different version, with both sides' data.
    pub mismatch: Option<Mismatch>,
    /// Matchmaking searches that found nothing while lobbies with other game data were open
    /// (each counted once a minute), and the last one's data: theirs, the lobbies'.
    pub searches_other_data: i32,
    pub search_mismatch: Option<Mismatch>,
    /// Of those, the ones the game's overlay explained as they happened ([`take_notice`]):
    /// failed joins other than version mismatches, version mismatches, searches.
    pub joins_explained: i32,
    pub mismatches_explained: i32,
    pub searches_explained: i32,
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
    let told = |kind: &str| i32::try_from(list.iter().filter(|e| e.kind == kind && e.told).count()).unwrap_or(i32::MAX);
    let failed_joins = count("failed_join") + count("version_mismatch");
    let version_mismatches = count("version_mismatch");
    let matches = record.as_ref().map_or(0, |r| i32::try_from(r.matches).unwrap_or(i32::MAX));
    let last_mismatch = |kind: &str| list.iter().rev().filter(|e| e.kind == kind).find_map(|e| e.mismatch.clone());
    let mismatch = last_mismatch("version_mismatch");
    let searches_other_data = count("search_mismatch");
    let search_mismatch = last_mismatch("search_mismatch");
    let json = json!({
        "server_version": crate::community_api::RELEASE,
        "player": record.as_ref().map(|r| json!({ "id": r.id, "name": r.name, "online": r.online, "play_seconds": r.play_seconds, "sessions": r.sessions, "matches": r.matches, "banned": r.banned.is_some() })),
        "session": { "started": started, "ended": if ended == 0 { Value::Null } else { json!(ended) } },
        "relayed": relayed,
        "failed_joins": failed_joins,
        "version_mismatches": version_mismatches,
        "mismatch": mismatch.as_ref().map(|m| json!({
            "host": m.host,
            "mine": m.mine.map(|v| format!("{v:#010x}")),
            "theirs": m.theirs.map(|v| format!("{v:#010x}")),
            "mine_name": m.mine.and_then(server_api::data_versions::name),
            "theirs_name": m.theirs.and_then(server_api::data_versions::name),
        })),
        "searches_other_data": searches_other_data,
        "search_mismatch": search_mismatch.as_ref().map(|m| json!({
            "mine": m.mine.map(|v| format!("{v:#010x}")),
            "theirs": m.theirs.map(|v| format!("{v:#010x}")),
            "mine_name": m.mine.and_then(server_api::data_versions::name),
            "theirs_name": m.theirs.and_then(server_api::data_versions::name),
        })),
        "explained": { "joins": told("failed_join"), "mismatches": told("version_mismatch"), "searches": told("search_mismatch") },
        "events": list.iter().map(|e| json!({ "at": e.at, "kind": e.kind, "detail": e.detail, "told": e.told })).collect::<Vec<_>>(),
    });
    Ok(Summary {
        started,
        ended,
        failed_joins,
        version_mismatches,
        relayed,
        matches,
        mismatch,
        searches_other_data,
        search_mismatch,
        joins_explained: told("failed_join"),
        mismatches_explained: told("version_mismatch"),
        searches_explained: told("search_mismatch"),
        json,
    })
}

impl Summary {
    /// The last mismatch for the launcher (`SessionSummaryResponse`): the host's name and
    /// each side's data as a player reads it; empty where not known.
    pub fn mismatch_shown(&self) -> (String, String, String) {
        self.mismatch
            .as_ref()
            .map_or_else(Default::default, |m| (m.host.clone().unwrap_or_default(), data_shown(m.mine), data_shown(m.theirs)))
    }

    /// The last search's hidden lobbies for the launcher: this game's data and theirs, as a
    /// player reads them; empty where not known.
    pub fn search_mismatch_shown(&self) -> (String, String) {
        self.search_mismatch.as_ref().map_or_else(Default::default, |m| (data_shown(m.mine), data_shown(m.theirs)))
    }
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
        .filter(|(k, _)| ["launcher", "client", "build", "os", "language", "exit_code"].contains(&k.as_str()))
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
    // Checked again with the insert: the check above is only a quick refusal.
    if !storage.queue_report(user, &id, &body.to_string(), !auto, PER_DAY, MAX_OUTBOX).await? {
        return Ok(Err(Refused::TooMany));
    }
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
            client: HashMap::from([
                ("launcher".into(), "0.4.1".into()),
                ("exit_code".into(), "0xC0000005".into()),
                ("secret".into(), "x".into()),
            ]),
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
        assert_eq!(b["client"], json!({ "launcher": "0.4.1", "exit_code": "0xC0000005" }), "only the known client fields");
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
        let mismatch = Mismatch {
            host: Some("Theusma01".into()),
            mine: Some(0xdd55_acd3),
            theirs: Some(0xe90c_0d2d),
        };
        join_failed(424_242, 9, VERSION_MISMATCH, JoinOutcome::DataMismatch, Some(mismatch.clone()));
        join_failed(424_242, 9, 0xb08a_1a05, JoinOutcome::Failed, Some(mismatch.clone()));
        let list = events().lock().unwrap().get(&424_242).cloned().unwrap();
        assert_eq!(list.iter().map(|e| e.kind).collect::<Vec<_>>(), ["version_mismatch", "failed_join"]);
        assert_eq!(list[0].mismatch, Some(mismatch.clone()));
        assert_eq!(list[1].mismatch, None, "only a version mismatch has one");
        let summary = Summary {
            mismatch: Some(mismatch),
            ..Summary::default()
        };
        assert_eq!(summary.mismatch_shown(), ("Theusma01".to_string(), "balaclava mod".to_string(), "unmodified".to_string()));
        let unknown = Summary {
            mismatch: Some(Mismatch {
                host: None,
                mine: Some(0x1234),
                theirs: None,
            }),
            ..Summary::default()
        };
        assert_eq!(unknown.mismatch_shown(), (String::new(), "0x00001234".to_string(), String::new()));
        assert_eq!(Summary::default().mismatch_shown(), Default::default());
    }

    #[test]
    fn a_notice_waits_for_the_game_and_marks_what_it_explains() {
        let user = 424_244;
        assert_eq!(take_notice(user), None);
        let explained = join_failed(user, 9, 0x9718_2b1b, JoinOutcome::Locked, None);
        let retried = join_failed(user, 9, 0x9718_2b1b, JoinOutcome::Locked, None);
        let unexplained = join_failed(user, 10, 0xac2d_801a, JoinOutcome::Failed, None);
        tell(user, "Couldn't join: the session is locked".into(), "a".into(), Some(explained));
        // The game retrying the same join: one notice, not two, explaining both.
        tell(user, "Couldn't join: the session is locked".into(), "a".into(), Some(retried));
        tell(user, "Couldn't join".into(), "b".into(), None);
        let told = |user: u32| {
            events()
                .lock()
                .unwrap()
                .get(&user)
                .map(|l| l.iter().filter(|e| e.told).map(|e| e.id).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        assert!(told(user).is_empty(), "nothing told before the game takes it");
        assert_eq!(take_notice(user), Some(("Couldn't join: the session is locked".into(), "a".into())), "oldest first");
        assert_eq!(told(user), [explained, retried], "the retry is explained too");
        assert_eq!(take_notice(user), Some(("Couldn't join".into(), "b".into())));
        assert_eq!(told(user), [explained, retried], "a notice that explains nothing marks nothing");
        assert!(!told(user).contains(&unexplained));
        assert_eq!(take_notice(user), None);

        // A notice the game didn't take in time is dropped: the player has moved on.
        tell(user, "Old".into(), "c".into(), None);
        notices().lock().unwrap().get_mut(&user).unwrap()[0].at -= NOTICE_FRESH + 1;
        assert_eq!(take_notice(user), None);
        // At most a few wait.
        for i in 0..5 {
            tell(user, format!("n{i}"), String::new(), None);
        }
        assert_eq!(notices().lock().unwrap().get(&user).unwrap().len(), NOTICES_KEPT);
        assert_eq!(take_notice(user).map(|n| n.0), Some("n2".to_string()), "the newest kept");
    }

    #[test]
    fn lobbies_hidden_by_game_data_are_noted_once_a_minute() {
        let user = 424_243;
        search_other_data(user, 0xc37e_d697, 0xe90c_0d2d, 2);
        search_other_data(user, 0xc37e_d697, 0xe90c_0d2d, 2);
        join_failed(user, 9, VERSION_MISMATCH, JoinOutcome::DataMismatch, None);
        let list = events().lock().unwrap().get(&user).cloned().unwrap();
        assert_eq!(
            list.iter().map(|e| e.kind).collect::<Vec<_>>(),
            ["search_mismatch", "version_mismatch"],
            "the game searches every few seconds"
        );
        events().lock().unwrap().get_mut(&user).unwrap()[0].at -= SEARCH_NOTED_FOR;
        search_other_data(user, 0xc37e_d697, 0xe90c_0d2d, 1);
        let list = events().lock().unwrap().get(&user).cloned().unwrap();
        assert_eq!(list.iter().filter(|e| e.kind == "search_mismatch").count(), 2);
        let summary = Summary {
            search_mismatch: list[2].mismatch.clone(),
            ..Summary::default()
        };
        assert_eq!(summary.search_mismatch_shown(), ("0xc37ed697".to_string(), "unmodified".to_string()));
        assert_eq!(Summary::default().search_mismatch_shown(), Default::default());
    }

    #[test]
    fn join_failures_are_classified_by_their_code() {
        assert_eq!(JoinOutcome::of(ROOM_FULL), JoinOutcome::Full);
        assert_eq!(JoinOutcome::of(LOCKED_SESSION), JoinOutcome::Locked);
        assert_eq!(JoinOutcome::of(ROOM_CLOSED), JoinOutcome::Closed);
        assert_eq!(JoinOutcome::of(VERSION_MISMATCH), JoinOutcome::DataMismatch);
        assert_eq!(JoinOutcome::of(0xb08a_1a05), JoinOutcome::Failed);
        assert!(!JoinOutcome::Full.counts() && JoinOutcome::Locked.counts() && JoinOutcome::Failed.counts());
        let logged: Vec<_> = [JoinOutcome::Full, JoinOutcome::Locked, JoinOutcome::Closed, JoinOutcome::DataMismatch, JoinOutcome::Failed]
            .into_iter()
            .filter(|o| o.wants_log())
            .map(JoinOutcome::as_str)
            .collect();
        assert_eq!(logged, ["data_mismatch", "failed"]);
        assert_eq!(JoinOutcome::of_detail(&json!({ "outcome": "full", "code": "0xeea440ee" })), JoinOutcome::Full);
        assert_eq!(JoinOutcome::of_detail(&json!({ "code": "0xcf964066" })), JoinOutcome::Full, "a 0.4.3 server's");
        // The host leaving as they joined (na1, 2026-10-11: LEAVING then TICKET_PENDING on a
        // match breaking up): the room went, not the player's join; a time-out is the join's.
        assert_eq!(JoinOutcome::of(0x33ae_7adc), JoinOutcome::Closed, "LEAVING");
        assert_eq!(JoinOutcome::of(0x59d6_f9cc), JoinOutcome::Closed, "HOST_GONE");
        assert_eq!(JoinOutcome::of(0xbb26_3fc5), JoinOutcome::Locked, "PRIVATE_SESSION");
        assert_eq!(JoinOutcome::of(0x9bf4_cf37), JoinOutcome::DataMismatch, "the game's VERSION_MISMATCH");
        for timed_out in [0xac2d_801a, 0xe4aa_ebcd, 0xca47_3b80] {
            assert_eq!(JoinOutcome::of(timed_out), JoinOutcome::Failed, "{timed_out:#x}");
        }
        assert_eq!(JoinOutcome::of_detail(&json!({})), JoinOutcome::Failed);
    }
}
