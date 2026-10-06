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
}

static STATE: Mutex<State> = Mutex::new(State {
    session: None,
    saved: None,
    achievements: BTreeSet::new(),
});

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
