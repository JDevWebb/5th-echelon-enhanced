//! What happened in each player's time on the server, for the admin UI's Sessions page:
//! sign-ins and their refusals, rooms made, searches and what they found, joins, failed
//! joins, leaving, invitations, stat writes (a mission or match played to its end), the
//! relay losing a player's traffic, and requests the server failed to answer.
//!
//! The game services note them here as they happen ([`note`]), without waiting on the
//! database: [`run`] saves them every couple of seconds, adds what the room was (from its
//! attributes) and the names of the players they mention, and keeps a repeat of the same
//! thing (an outdated client retrying its sign-in every few seconds) as one event with a
//! count. The federation sends the saved ones to the coordinator (`POST /v1/events`) and
//! marks them sent only once it took them, so an outage of the coordinator loses none.
//!
//! Kinds, and what their detail holds:
//! - `server_start`: `version`.
//! - `signin`: the game signed in.
//! - `signin_refused`: `reason` (`outdated`, `too_many`: too many failed sign-ins,
//!   `too_many_signins`: too many sign-ins from the address, failed or not, `wrong_password`,
//!   `banned`, `no_current_client`, `name_not_allowed`), `via` (`game` or `api`), `client` when known.
//! - `signout`: `how` (`closed`: the game said goodbye, or its connection went after it said
//!   it was closing; `timed_out`: it went quiet; `closing`: the game said it's quitting, as
//!   0.4.5 games do (Misc.Closing), ahead of its connection going: see [`closing`]).
//! - `room`: `room` (the session id), made by the player.
//! - `search`: `query` (8: Spies vs Mercs matchmaking or an invitation, 11: co-op), `found`.
//! - `join`: `room`, `via` (`invite`, `party`: their host took them along, `search`); when the
//!   NAT helper knows the guest, how their game reaches the host's (`nat_helper::Path`):
//!   `ping_ms` and `host_ping_ms` (each one's round trip to this server, as their game
//!   measured it), `relayed`, and for a relayed pair `relay_ms` (their round trip through
//!   the relay) and `direct_ms` (the least a direct one could take, from where they are).
//! - `join_failed`: `room`, `code` (the game's error code, hex), `outcome` (`full`: the
//!   lobby is full at its current size; `locked`; `closed`: the room had just been closed;
//!   `data_mismatch`: different game data; `failed`: anything else). `own`: true when the
//!   room is the player's own or had ended, the game's own churn rather than a failed join.
//!   `reported_room`: what the game said, when that was session 0 or its own split party
//!   and `room` is the one the server had just given the player (join_targets.rs). For
//!   `data_mismatch`: `data` (`mine`, `theirs`: each side's attribute 107, hex, either may be
//!   absent) and `mine_name`, `theirs_name` (`server_api::data_versions`, null if unknown).
//! - `leave`: `room`, `how` (`left`, `abandoned`, `removed` by the host: `by`; `dropped`: an
//!   earlier game of theirs went without a word; `closed`: it went after saying it was
//!   closing), `ended`.
//! - `invite`: `to`, `room` (none: unbound, the receiver finds nothing to join).
//! - `invite_delivered`: `from`, `room`.
//! - `stats`: the game wrote stats (it does at the end of a mission or match).
//! - `nat`: `relayed` (the player's game traffic goes through the server's relay).
//! - `nat_missing`: `after_secs`: the game went online (opened or joined a room) and hadn't
//!   registered with the NAT helper that long after; nobody can reach it.
//! - `nat_lost`: `probe_secs`: the game's registration lapsed (no probe that long) while it
//!   was still signed in; nobody can reach it until it registers again.
//! - `relay_drop`: `direction` (`sending`: the relay stopped getting this player's traffic;
//!   `receiving`: stopped getting traffic for them), `before` and `after` (packets a second),
//!   `player_quiet` (their game and launcher said nothing to the server over that time
//!   either: their own connection stalled; 0.4.4 servers).
//! - `login_stalled`: the game kept sending but never acknowledged the server's reliable
//!   packets (its replies aren't reaching the PC, so it can't finish signing in): `packets`
//!   given up on in this run, `since` (when the run began). At most once in 10 minutes.
//! - `request_error`: `call` (Protocol.Method), `error`.
//! - `report_refused`: the player's report (the launcher's "how did it go") wasn't taken:
//!   `reason` (`too_many` today, or `invalid`), `why` (what the player was told), `files`
//!   and `bytes` (compressed) it carried.
//! - `client_log`: `level` (`error`, `warn`, `info`), `target`, `message`: a line
//!   of the game's own log (its warnings, errors and network events), sent by the game
//!   (Misc.ClientLog) unless the player turned it off.
//!
//! Every `room` gets `since` (when it was made: ids come again once old rooms are gone),
//! `room_kind` (`party` or `match`), `mode` (`coop`, `svm`), `private`, `map`, `game_mode` and
//! `host` when saved; every player id mentioned (`host`, `to`, `from`,
//! `by`) gets a `<key>_name`.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use serde_json::json;
use serde_json::Value;
use slog::Logger;

use crate::storage::NewSessionEvent;
use crate::storage::Storage;

/// Events waiting to be saved at most (a flood past this is dropped and counted).
const MAX_QUEUED: usize = 20_000;
/// How often the queue is saved.
const SAVE_EVERY: Duration = Duration::from_secs(2);
/// How often old events are deleted.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// A join noted again for the same player and room within this long is the same join (the
/// game may both add itself and send `JoinSession`).
const SAME_JOIN: Duration = Duration::from_secs(600);
/// A player's lost packets are noted at most this often (`login_stalled`), and a run of them
/// ends after this long without one.
const STALL_NOTE_EVERY: Duration = Duration::from_secs(600);
const STALL_RUN_GAP: Duration = Duration::from_secs(600);

/// Who an event is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// A player by account id.
    Id(u32),
    /// A player by name (the relay and the API know them so). Dropped when no account has it.
    Name(String),
    /// Nobody: the server itself.
    Server,
}

#[derive(Default)]
struct Queue {
    events: VecDeque<NewSessionEvent>,
    dropped: u64,
    joins: HashMap<(u32, u32), Instant>,
    stalls: HashMap<u32, Stall>,
}

/// A player's run of reliable packets their game never acknowledged (see [`login_stalled`]).
struct Stall {
    /// When the run began (unix seconds).
    since: i64,
    /// Packets given up on in it.
    packets: u64,
    last: Instant,
    /// When it was last noted.
    noted: Option<Instant>,
}

fn queue() -> &'static Mutex<Queue> {
    static QUEUE: OnceLock<Mutex<Queue>> = OnceLock::new();
    QUEUE.get_or_init(Mutex::default)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs().try_into().unwrap_or(i64::MAX))
}

/// Notes an event; [`run`] saves it in a moment.
pub fn note(who: Who, kind: &'static str, detail: Value) {
    // Worth the game's whole log, for a player who agreed (full_logs.rs).
    if let Some(problem) = crate::full_logs::problem_of(kind, &detail) {
        let (id, name) = match &who {
            Who::Id(id) => (Some(*id), None),
            Who::Name(name) => (None, Some(name.as_str())),
            _ => (None, None),
        };
        crate::full_logs::noted(id, name, problem, now());
    }
    let event = NewSessionEvent {
        at: now(),
        user_id: match who {
            Who::Id(id) => Some(id),
            _ => None,
        },
        name: match who {
            Who::Name(name) => Some(name),
            _ => None,
        },
        kind,
        detail,
    };
    let mut q = queue().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if q.events.len() >= MAX_QUEUED {
        q.dropped += 1;
        return;
    }
    q.events.push_back(event);
}

/// Notes that `user` joined `room`, once however many ways the game says so.
pub fn joined(user: u32, room: u32, via: &'static str) {
    {
        let mut q = queue().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        if q.joins.len() > 10_000 {
            q.joins.retain(|_, at| now.duration_since(*at) < SAME_JOIN);
        }
        if q.joins.get(&(user, room)).is_some_and(|at| now.duration_since(*at) < SAME_JOIN) {
            return;
        }
        q.joins.insert((user, room), now);
    }
    note(Who::Id(user), "join", json!({ "room": room, "via": via }));
}

/// Forgets `user`'s joins: their game signed in again (a restart, or a crash), and its join
/// of the room the game before was in is a join of its own, to note.
pub fn forget_joins(user: u32) {
    queue().lock().unwrap_or_else(std::sync::PoisonError::into_inner).joins.retain(|(u, _), _| *u != user);
}

/// How long after a game said it's closing its connection going is the quit, not a drop: a
/// quitting game sends no goodbye, so its connection goes when it's been quiet a minute
/// (quazal's session timeout), and the relay notices its traffic stopping within seconds.
/// One a player restarting at once leaves behind goes then too (a game that exited at
/// 20:31:29 was taken for a restart at 20:32:23).
pub const CLOSING_COVERS: Duration = Duration::from_secs(120);

/// Games that said they're closing (Misc.Closing), by account id and by name (lower case):
/// when.
#[derive(Default)]
struct Closing {
    ids: HashMap<u32, Instant>,
    names: HashMap<String, Instant>,
}

fn closings() -> std::sync::MutexGuard<'static, Closing> {
    static CLOSING: OnceLock<Mutex<Closing>> = OnceLock::new();
    CLOSING.get_or_init(Mutex::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `user`'s game (`name`, when known) said it's closing normally: for [`CLOSING_COVERS`],
/// its connection going is the quit ([`closed_lately`]), not a drop or a restart. Noted as a
/// `signout` (`closing`) once, however often the game says it.
pub fn closing(user: u32, name: Option<&str>) {
    let fresh = {
        let mut c = closings();
        let now = Instant::now();
        if c.ids.len() > 10_000 {
            c.ids.retain(|_, at| now.duration_since(*at) < CLOSING_COVERS);
            c.names.retain(|_, at| now.duration_since(*at) < CLOSING_COVERS);
        }
        let fresh = !c.ids.get(&user).is_some_and(|at| now.duration_since(*at) < CLOSING_COVERS);
        c.ids.insert(user, now);
        if let Some(name) = name {
            c.names.insert(name.to_lowercase(), now);
        }
        fresh
    };
    if fresh {
        note(Who::Id(user), "signout", json!({ "how": "closing" }));
    }
}

/// Whether `who`'s game said it was closing within [`CLOSING_COVERS`]: its connection going
/// (or its traffic stopping) now is the quit. Never for games before 0.4.5, which don't say.
pub fn closed_lately(who: &Who) -> bool {
    let c = closings();
    let at = match who {
        Who::Id(id) => c.ids.get(id),
        Who::Name(name) => c.names.get(&name.to_lowercase()),
        Who::Server => None,
    };
    at.is_some_and(|at| at.elapsed() < CLOSING_COVERS)
}

/// Forgets the joins of a room that ended (its id may come again after a restart).
pub fn room_ended(room: u32) {
    queue().lock().unwrap_or_else(std::sync::PoisonError::into_inner).joins.retain(|(_, r), _| *r != room);
}

/// Notes that `user`'s game kept sending but never acknowledged `packets` of the server's
/// reliable packets (quazal gave up on them): its replies aren't reaching their PC, so the
/// game can't finish signing in. At most once per player in [`STALL_NOTE_EVERY`], with the
/// run's total so far.
pub fn login_stalled(user: u32, packets: usize) {
    let detail = {
        let mut q = queue().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        stalled(&mut q.stalls, user, packets, Instant::now(), now())
    };
    if let Some(detail) = detail {
        note(Who::Id(user), "login_stalled", detail);
    }
}

/// Adds `packets` to `user`'s run of lost packets, and gives the event's detail when it's
/// time to note it.
fn stalled(stalls: &mut HashMap<u32, Stall>, user: u32, packets: usize, at: Instant, unix: i64) -> Option<Value> {
    stalls.retain(|_, s| at.duration_since(s.last) < STALL_RUN_GAP);
    let s = stalls.entry(user).or_insert(Stall {
        since: unix,
        packets: 0,
        last: at,
        noted: None,
    });
    s.packets += packets as u64;
    s.last = at;
    if s.noted.is_some_and(|n| at.duration_since(n) < STALL_NOTE_EVERY) {
        return None;
    }
    s.noted = Some(at);
    Some(json!({ "packets": s.packets, "since": s.since }))
}

/// The events waiting, taken.
fn take() -> (Vec<NewSessionEvent>, u64) {
    let mut q = queue().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    (q.events.drain(..).collect(), std::mem::take(&mut q.dropped))
}

/// Events that couldn't be saved, back at the front of the queue (unless it's full).
fn put_back(events: Vec<NewSessionEvent>) {
    let mut q = queue().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for event in events.into_iter().rev() {
        if q.events.len() >= MAX_QUEUED {
            q.dropped += 1;
            continue;
        }
        q.events.push_front(event);
    }
}

/// What a room is, from its attributes, as event detail.
pub fn room_detail(attributes: &str) -> Value {
    use crate::game_session::attribute_value;
    let mut detail = json!({
        "room_kind": if attribute_value(attributes, 113) == Some(0) { "match" } else { "party" },
        // Private seats only (as `metrics::Match`).
        "private": attribute_value(attributes, 3) == Some(0) && attribute_value(attributes, 4).is_some_and(|n| n > 0),
    });
    if let Some(mode) = crate::game_session::match_mode(attributes) {
        detail["mode"] = json!(mode);
    }
    for (key, id) in [("map", 101), ("game_mode", 102)] {
        if let Some(value) = attribute_value(attributes, id) {
            detail[key] = json!(value);
        }
    }
    detail
}

/// Saves what was noted every couple of seconds, and deletes old events now and then.
pub async fn run(logger: Logger, storage: std::sync::Arc<Storage>) {
    let mut last_prune: Option<Instant> = None;
    loop {
        tokio::time::sleep(SAVE_EVERY).await;
        let (events, dropped) = take();
        if dropped > 0 {
            slog::warn!(logger, "Session events: {dropped} dropped (too many at once)");
        }
        if !events.is_empty() {
            if let Err(e) = storage.save_session_events_async(events.clone()).await {
                // The database busy, say: they go again next time (the transaction saved none).
                slog::warn!(logger, "Session events: saving failed (will retry): {e:#}");
                put_back(events);
            }
        }
        if last_prune.is_none_or(|t| t.elapsed() >= PRUNE_EVERY) {
            last_prune = Some(Instant::now());
            if let Err(e) = storage.prune_session_events_async().await {
                slog::warn!(logger, "Session events: deleting old ones failed: {e:#}");
            }
        }
    }
}

/// The reason a refused sign-in gives, for `signin_refused`.
pub fn refused(who: Who, reason: &str, via: &str, client: Option<&str>) {
    let mut detail = json!({ "reason": reason, "via": via });
    if let Some(client) = client.filter(|c| !c.is_empty()) {
        detail["client"] = json!(client.chars().take(64).collect::<String>());
    }
    note(who, "signin_refused", detail);
}

/// Hands the request failures quazal reports to [`note`].
pub fn watch_request_failures() {
    quazal::rmc::failures::set_hook(|f| {
        if !crate::rate_limit::request_errors().check(f.user_id.unwrap_or(0)) {
            return;
        }
        let who = f.user_id.map_or(Who::Server, Who::Id);
        let mut error: String = f.error.chars().take(200).collect();
        // Not signed in: anyone's packets, whose numbers ("12 bytes missing") would make every
        // event a new one; without them, repeats are counted as one.
        if f.user_id.is_none() {
            error = error.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
        }
        let mut detail = json!({ "error": error });
        if let Some(call) = f.call {
            detail["call"] = json!(call);
        } else if let (Some(p), Some(m)) = (f.protocol, f.method) {
            detail["call"] = json!(format!("{p}.{m}"));
        }
        note(who, "request_error", detail);
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn rooms_are_described_by_their_attributes() {
        let coop_private = "113 => 0;3 => 0;4 => 2;101 => 3578398534;102 => 3;103 => 0;105 => 2";
        let d = super::room_detail(coop_private);
        assert_eq!(
            (d["room_kind"].as_str(), d["mode"].as_str(), d["private"].as_bool()),
            (Some("match"), Some("coop"), Some(true))
        );
        assert_eq!(d["map"], 3_578_398_534u32);
        let party = super::room_detail("113 => 1;3 => 8;4 => 0");
        assert_eq!((party["room_kind"].as_str(), party.get("mode")), (Some("party"), None));
    }

    /// Lost packets are noted once in ten minutes, with the run's total; a quiet spell as long
    /// starts a new run.
    #[test]
    fn a_stalled_login_is_noted_once_in_ten_minutes() {
        use std::time::Duration;
        use std::time::Instant;
        let mut stalls = std::collections::HashMap::new();
        let t = Instant::now();
        let at = |secs: u64| t + Duration::from_secs(secs);
        let first = super::stalled(&mut stalls, 1009, 1, at(0), 1000).unwrap();
        assert_eq!(first, serde_json::json!({ "packets": 1, "since": 1000 }));
        // Every four or five minutes, for a while: one note in ten minutes.
        assert_eq!(super::stalled(&mut stalls, 1009, 1, at(270), 1270), None);
        assert_eq!(super::stalled(&mut stalls, 2000, 2, at(300), 1300).unwrap()["packets"], 2, "another player's own");
        assert_eq!(super::stalled(&mut stalls, 1009, 2, at(540), 1540), None);
        let again = super::stalled(&mut stalls, 1009, 1, at(810), 1810).unwrap();
        assert_eq!(again, serde_json::json!({ "packets": 5, "since": 1000 }));
        // Fine for ten minutes, then again: a new run.
        let later = super::stalled(&mut stalls, 1009, 1, at(1500), 2500).unwrap();
        assert_eq!(later, serde_json::json!({ "packets": 1, "since": 2500 }));
        assert!(!stalls.contains_key(&2000), "old runs are forgotten");
    }
}
