//! The Play screen's checklist, for both interfaces: which checks show, how ready the player
//! is, and the fixes the launcher can make itself.

use std::path::Path;

use setup::diagnose::Check;
use setup::diagnose::Fix;
use setup::diagnose::Status;

use crate::flow::Support;
use crate::prefs::Game;

/// The checks that matter, shown when nothing's wrong (all of them on "All checks").
pub const SUMMARY: [&str; 4] = ["client", "account", "network", "save"];

/// The checks to show: all of them when asked; else the problems, or the few that matter
/// when there are none.
pub fn shown(checks: &[Check], all: bool) -> Vec<&Check> {
    let issues = checks.iter().any(|c| c.status != Status::Ok);
    checks
        .iter()
        .filter(|c| all || if issues { c.status != Status::Ok } else { SUMMARY.contains(&c.id) })
        .collect()
}

/// Whether "Fix everything" has something to do: a failed check with a fix.
pub fn fixable(checks: &[Check]) -> bool {
    checks.iter().any(|c| c.status == Status::Fail && c.fix.is_some())
}

/// The fix's button.
pub fn fix_label(fix: Fix) -> &'static str {
    match fix {
        Fix::FindGame => "Find",
        Fix::InstallClient => "Install",
        Fix::ChooseServer => "Choose",
        Fix::SetUpAccount => "Connect",
        Fix::AutoAdapter => "Choose automatically",
        Fix::CreateSave => "Set up a save",
        Fix::RaiseSave => "Raise to rank 5",
        Fix::UpdateLauncher => "Update",
    }
}

/// What the activity bar says while a fix the launcher makes itself runs; None for the ones
/// the interface takes over (finding the game, the server, updating).
pub fn fix_title(fix: Fix) -> Option<&'static str> {
    match fix {
        Fix::InstallClient => Some("Installing the 5th Echelon client"),
        Fix::AutoAdapter => Some("Choosing the network adapter automatically"),
        Fix::CreateSave | Fix::RaiseSave => Some("Fixing your save"),
        Fix::FindGame | Fix::ChooseServer | Fix::SetUpAccount | Fix::UpdateLauncher => None,
    }
}

/// Makes a fix the launcher makes itself (on a task's thread): what it did, or why not. None
/// for the ones [`fix_title`] leaves to the interface.
pub fn run_fix(fix: Fix, game_dir: &Path) -> Option<Result<String, String>> {
    match fix {
        Fix::InstallClient => Some(crate::flow::install_client(game_dir, crate::dll_utils::bundled())),
        Fix::AutoAdapter => Some(crate::flow::auto_adapter(game_dir)),
        Fix::CreateSave | Fix::RaiseSave => Some(crate::flow::fix_save(game_dir)),
        Fix::FindGame | Fix::ChooseServer | Fix::SetUpAccount | Fix::UpdateLauncher => None,
    }
}

/// The game version's line under the checks, when it isn't supported or couldn't be told:
/// its status, title and why (its first line).
pub fn support_line(support: Option<&Support>) -> Option<(Status, &'static str, String)> {
    let (status, title, why) = match support? {
        Support::Supported => return None,
        Support::Unsupported(why) => (Status::Fail, "This game version isn't supported yet", why),
        Support::Unknown(why) => (Status::Warn, "Couldn't check the game version", why),
    };
    Some((status, title, why.lines().next().unwrap_or_default().to_string()))
}

/// The game executable the hooks are looked for in: the one the game starts.
pub fn game_version(game: &Game) -> setup::game::GameVersion {
    setup::game::pick_version(&game.dir, game.cfg.default_game).unwrap_or(setup::game::GameVersion::SplinterCellBlacklistDx11)
}

/// Looks for the hooks' addresses in an unsupported game executable (on a task's thread).
pub fn identify(game_dir: &Path, version: setup::game::GameVersion) -> Result<String, String> {
    crate::flow::identify(game_dir, version)
        .map(|()| "The game version is now supported.".to_string())
        .map_err(|e| format!("Couldn't identify it: {e}"))
}

/// How ready the player is, beside Play: its tone (None while checking) and its line.
pub fn readiness(checks: &[Check], support: Option<&Support>, running: bool) -> (Option<Status>, String) {
    if running {
        return (Some(Status::Ok), "Splinter Cell: Blacklist is running".into());
    }
    if checks.is_empty() {
        return (None, "Checking…".into());
    }
    let fails = checks.iter().filter(|c| c.status == Status::Fail).count() + usize::from(matches!(support, Some(Support::Unsupported(_))));
    let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
    match (fails, warns) {
        (0, 0) => (Some(Status::Ok), "Ready to play · everything checked".into()),
        (0, 1) => (Some(Status::Warn), "Ready to play · 1 note".into()),
        (0, n) => (Some(Status::Warn), format!("Ready to play · {n} notes")),
        (1, _) => (Some(Status::Fail), "1 thing to fix".into()),
        (n, _) => (Some(Status::Fail), format!("{n} things to fix")),
    }
}
