//! Starting the game from the Play button, for both interfaces: what has to be answered or set
//! first, the settings the game reads as it starts, then the start itself.

use setup::launch::Launched;

use crate::activity::Activities as Notices;
use crate::prefs::Game;
use crate::prefs::PlayMode;
use crate::prefs::Prefs;

/// Why the game wasn't started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Something to do first (a question to answer): a notice, not an error.
    First(String),
    Error(String),
}

/// Starts the game, `mode` saying whether straight into an online lobby. `name_not_allowed`:
/// the server said the player's name breaks its rules (it would keep the game out).
pub fn start(game: &mut Game, notices: &mut Notices, mode: PlayMode, name_not_allowed: bool) -> Result<Launched, Refused> {
    // The question about diagnostics is answered first (diagnostics.rs).
    if crate::diagnostics::Ask::pending(Some(game)) {
        return Err(Refused::First("First, say whether the game may send its diagnostics.".into()));
    }
    // And the new game options (new_options.rs).
    if !crate::new_options::pending(Some(game)).is_empty() {
        return Err(Refused::First("First, choose the new game options.".into()));
    }
    if setup::game::game_running() {
        return Err(Refused::Error("Splinter Cell: Blacklist is already running.".into()));
    }
    if name_not_allowed {
        return Err(Refused::Error(
            "Your name breaks this network's rules. Choose another first: Settings › Servers and accounts › Rename.".into(),
        ));
    }
    // The hook reads uplay.toml when the game starts: make sure it has the current profile.
    if game.managed.is_none() {
        if let Some(profile) = game.cfg.current_profile().cloned() {
            game.update(notices, |c| c.apply_profile(&profile));
        }
        // Straight into a lobby, or a plain start (which clears a lobby the launcher set before,
        // never one set by hand). The game reads it on every start, through Steam too.
        let set_before = Prefs::quick_start_set();
        let mut set = set_before;
        game.update(notices, |c| set = mode.apply(c, set_before));
        Prefs::set_quick_start_set(set);
    }
    let Some(version) = setup::game::pick_version(&game.dir, game.cfg.default_game) else {
        return Err(Refused::Error("The game's executable is missing.".into()));
    };
    setup::launch::launch(&game.dir, version, Prefs::start_through_steam()).map_err(|e| {
        tracing::warn!("couldn't start the game: {e}");
        Refused::Error(format!("Couldn't start the game: {e}"))
    })
}
