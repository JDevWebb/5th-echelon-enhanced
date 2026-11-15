//! Asking once whether the game may send its diagnostics to the server (`send_diagnostics`;
//! the hooks DLL sends them, see `hooks_config::diagnostics`). Asked before the game first
//! runs with them: until the player answers, nothing is sent (the DLL's default is off).

use setup::feedback::Private;

use crate::activity::Activities as Notices;
use crate::prefs::Prefs;

/// Lines of the player's own last game shown as the example.
const EXAMPLE_LINES: usize = 12;

pub const TITLE: &str = "Help us fix connection problems";
pub const WHAT: &str = "5th Echelon now sends the server you play on a short diagnostic log while you play: the game's warnings, errors and network messages, and what it's doing (a few lines a minute at most). When something goes wrong (a join that fails, your connection dropping), the server can also ask for the 15 minutes of the game's log before it, at most once an hour; never the whole log. If the game crashes or freezes, it says where in the game's code when it next starts. It lets the server's admins see why a join failed or a connection dropped on your side, not just theirs.";
pub const PRIVATE: &str = "Before anything leaves your PC, your PC's name, your Windows account name, your user folder and your internet address are removed. It goes only to the server you're playing on and is kept for at most 30 days.";
pub const LATER: &str = "You can change this any time in Settings › Feedback.";

/// The question about diagnostics (launcher-slint asks it).
pub struct Ask;

impl Ask {
    /// Whether the player still has to answer (an install another tool manages isn't asked:
    /// it stays off).
    pub fn pending(game: Option<&crate::prefs::Game>) -> bool {
        game.is_some_and(|g| g.managed.is_none()) && !Prefs::diagnostics_asked()
    }
}

/// The player's answer: kept in the game's settings, noted as asked, and confirmed.
pub fn answer_with(game: &mut crate::prefs::Game, notices: &mut Notices, send: bool) {
    game.update(notices, |c| c.hook_config.send_diagnostics = send);
    Prefs::set_diagnostics_asked();
    notices.info(if send {
        "Thanks: the game will send its diagnostics. Settings › Feedback turns them off."
    } else {
        "The game won't send diagnostics. Settings › Feedback turns them on."
    });
}

/// The last lines of the player's own log that would have been sent, private parts hidden.
pub fn example(game: &crate::prefs::Game) -> Vec<String> {
    let mut private = Private::of_this_pc(Vec::new());
    if let Some(profile) = game.cfg.current_profile() {
        private = private.showing(&profile.user.username);
    }
    ["bl-tracing.log", "bl-tracing.prev.log"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(game.dir.join(f)).ok())
        .map(|log| hooks_config::diagnostics::example(&log, EXAMPLE_LINES))
        .find(|lines| !lines.is_empty())
        .unwrap_or_default()
        .into_iter()
        .map(|l| setup::feedback::redact(&l, &private))
        .collect()
}
