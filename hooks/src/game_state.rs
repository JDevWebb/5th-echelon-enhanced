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
}

static STATE: Mutex<State> = Mutex::new(State {
    session: None,
    saved: None,
    achievements: BTreeSet::new(),
    online: None,
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
    info!("Saved the game ({} KB): a checkpoint, or a mission's end", bytes / 1024);
}

/// The game's sessions as [`Online`] says; written when it changes.
pub fn online(now: Online) {
    let mut st = state();
    if st.online == Some(now) {
        return;
    }
    let first = st.online.is_none();
    st.online = Some(now);
    drop(st);
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
