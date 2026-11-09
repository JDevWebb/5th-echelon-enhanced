//! What the game is doing, in a few lines: the game session it's in, its saves (a
//! checkpoint, or a mission's end) and achievements, for the server's Sessions page with the
//! game's other diagnostics (`hooks::game_state`, see `hooks_config::diagnostics`). A freeze
//! then reads as "last checkpoint at 06:36:50, nothing after" without the player's log
//! (PlaySkill's co-op freeze on eu1, 2026-10-06, was only found in the attached one).
//!
//! Only changes are written: the game repeats its session calls many times a second, writes
//! two save slots at each checkpoint, and asks for an achievement again and again.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use tracing::info;

/// Saves written within this of the last one are the same checkpoint.
const SAVE_EVERY: Duration = Duration::from_secs(15);

struct State {
    session: Option<(u32, bool)>,
    saved: Option<Instant>,
    achievements: BTreeSet<usize>,
    online: Option<Online>,
    /// The game is getting into a match: its lobby session locked, not in a match yet.
    joining: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    session: None,
    saved: None,
    achievements: BTreeSet::new(),
    online: None,
    joining: false,
});

/// The game's own view of its online sessions (read by `migration_fix`'s tick): whether its
/// match and lobby sessions are there and locked (`byte [session+0x5de]`, what makes a host
/// turn a joiner away with "locked"), and whether it counts itself in a match.
///
/// For finding out what changes when a Spies vs Mercs round starts: the lock is set on the
/// lobby session as the player goes into a match (the pre-round lobby too, where joins still
/// work), and the in-match check holds there as well, so neither is "under way" as far as
/// anyone has read the game (notes: session-locked.md). Its changes, next to the server's
/// joins and refusals, show which one is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Online {
    pub match_session: Option<bool>,
    pub lobby_session: Option<bool>,
    pub in_match: bool,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The game says which session it's in (`None`: none).
pub fn session(id: Option<u32>, invite_only: bool) {
    let mut st = state();
    let now = id.map(|id| (id, invite_only));
    if st.session == now {
        return;
    }
    match (st.session, now) {
        (_, Some((id, invite_only))) => {
            info!("In game session {id}{}", if invite_only { " (invite only)" } else { "" });
        }
        (Some((id, _)), None) => info!("Left game session {id}"),
        (None, None) => {}
    }
    st.session = now;
}

/// The game saved (`bytes` of it).
pub fn saved(bytes: usize) {
    let mut st = state();
    if st.saved.is_some_and(|t| t.elapsed() < SAVE_EVERY) {
        return;
    }
    st.saved = Some(Instant::now());
    drop(st);
    crate::crash::saved();
    info!("Saved the game ({} KB): a checkpoint, or a mission's end", bytes / 1024);
}

/// What the game is doing, in a few words, for a crash's details (crash.rs keeps it for its
/// handler): online or not, in a match, its session, the match's mode, state and map.
pub fn summary() -> String {
    let st = state();
    let mut parts: Vec<String> = Vec::new();
    match st.online {
        Some(o) if o.in_match => parts.push("in a match".into()),
        Some(o) if o.lobby_session.is_some() || o.match_session.is_some() => parts.push("online, in a lobby".into()),
        Some(_) => parts.push("online, no session".into()),
        None => parts.push("not online yet".into()),
    }
    if let Some((id, invite_only)) = st.session {
        parts.push(format!("session {id}{}", if invite_only { " (invite only)" } else { "" }));
    }
    drop(st);
    let (round, under_way, mode) = crate::round_state::current();
    if !mode.is_empty() {
        parts.push(format!("mode {mode}"));
    }
    if !round.is_empty() {
        parts.push(format!("match state {round}{}", if under_way { " (under way)" } else { "" }));
    }
    let map = crate::round_state::server_map();
    if !map.is_empty() {
        parts.push(format!("map {map}"));
    }
    parts.join(", ")
}

/// The game's sessions as [`Online`] says; written when it changes.
pub fn online(now: Online) {
    let mut st = state();
    if st.online == Some(now) {
        return;
    }
    let first = st.online.is_none();
    st.online = Some(now);
    // A join the game gave up on: its lobby session locked to get into a match, then unlocked
    // (back to its own lobby) without its ever being in it. The game doesn't always report
    // that (a connection that never came up), and the server kept the player in the room.
    let gave_up = if now.in_match {
        st.joining = false;
        false
    } else if now.lobby_session == Some(true) {
        st.joining = true;
        false
    } else {
        std::mem::take(&mut st.joining)
    };
    drop(st);
    if gave_up {
        info!("Online sessions: its lobby unlocked without its getting into a match (a join given up, or a party joined); telling the server, which checks for a match it was sent to");
        std::thread::spawn(crate::api::join_never_got_in);
    }
    if first
        && now
            == (Online {
                match_session: None,
                lobby_session: None,
                in_match: false,
            })
    {
        return;
    }
    let shown = |s: Option<bool>| match s {
        None => "none",
        Some(false) => "open",
        Some(true) => "locked",
    };
    info!(
        "Online sessions: match {}, lobby {}, {}",
        shown(now.match_session),
        shown(now.lobby_session),
        if now.in_match { "in a match" } else { "not in a match" }
    );
}

/// The game earned achievement `id` (once each).
pub fn achievement(id: usize) {
    if state().achievements.insert(id) {
        info!("Achievement {id:#x} earned");
    }
}

/// The overlay's panel opened or closed (with the key that did it).
pub fn overlay(open: bool, key: &str) {
    info!("Overlay {} ({key})", if open { "opened" } else { "closed" });
}
