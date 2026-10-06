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
//! - `signin_refused`: `reason` (`outdated`, `too_many`, `wrong_password`, `banned`,
//!   `no_current_client`), `via` (`game` or `api`), `client` when known.
//! - `signout`: `how` (`closed`: the game said goodbye; `timed_out`: it went quiet).
//! - `room`: `room` (the session id), made by the player.
//! - `search`: `query` (8: Spies vs Mercs matchmaking or an invitation, 11: co-op), `found`.
//! - `join`: `room`, `via` (`invite`, `party`: their host took them along, `search`); when the
//!   NAT helper knows the guest, how their game reaches the host's (`nat_helper::Path`):
//!   `ping_ms` and `host_ping_ms` (each one's round trip to this server, as their game
//!   measured it), `relayed`, and for a relayed pair `relay_ms` (their round trip through
//!   the relay) and `direct_ms` (the least a direct one could take, from where they are).
//! - `join_failed`: `room`, `code` (the game's error code, hex).
//! - `leave`: `room`, `how` (`left`, `abandoned`, `removed` by the host: `by`), `ended`.
//! - `invite`: `to`, `room` (none: unbound, the receiver finds nothing to join).
//! - `invite_delivered`: `from`, `room`.
//! - `stats`: the game wrote stats (it does at the end of a mission or match).
//! - `nat`: `relayed` (the player's game traffic goes through the server's relay).
//! - `nat_missing`: `after_secs`: the game went online (opened or joined a room) and hadn't
//!   registered with the NAT helper that long after; nobody can reach it.
//! - `nat_lost`: `probe_secs`: the game's registration lapsed (no probe that long) while it
//!   was still signed in; nobody can reach it until it registers again.
//! - `relay_drop`: `direction` (`sending`: the relay stopped getting this player's traffic;
//!   `receiving`: stopped getting traffic for them), `before` and `after` (packets a second).
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

/// Forgets the joins of a room that ended (its id may come again after a restart).
pub fn room_ended(room: u32) {
    queue().lock().unwrap_or_else(std::sync::PoisonError::into_inner).joins.retain(|(_, r), _| *r != room);
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
        let who = f.user_id.map_or(Who::Server, Who::Id);
        let mut detail = json!({ "error": f.error.chars().take(200).collect::<String>() });
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
}
