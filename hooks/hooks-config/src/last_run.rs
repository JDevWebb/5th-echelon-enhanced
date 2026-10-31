//! The game's last start, as the add-on saw it (`bl-last-run.txt` in the game folder): which
//! executable ran, so which DirectX, and whether Steam started it. The launcher shows it beside
//! its Renderer setting, and says why when it isn't what was chosen (the 0.4.8 plan's P3).

use std::path::Path;
use std::time::SystemTime;

/// In the game folder, beside the executables.
pub const FILE: &str = "bl-last-run.txt";

/// Set by the launcher on the games it starts itself: one Steam starts (or restarts) hasn't it.
pub const LAUNCHER_ENV: &str = "FES_STARTED_BY_LAUNCHER";

/// The DirectX 9 executable's name; the other is DirectX 11.
const DX9_EXE: &str = "Blacklist_game.exe";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastRun {
    /// The executable's file name.
    pub exe: String,
    /// Steam started it (its own choice of executable, whatever the launcher's).
    pub steam: bool,
    /// The launcher started it itself (its Renderer setting picked the executable).
    pub launcher: bool,
    /// When (the file's time).
    pub at: Option<SystemTime>,
}

impl LastRun {
    pub fn dx9(&self) -> bool {
        self.exe.eq_ignore_ascii_case(DX9_EXE)
    }
}

/// Written once a start through Ubisoft Connect signed in to a server: the copy starts that
/// way, so the launcher offers starting without it ([`crate::Config::without_ubisoft_connect`]).
pub const UBISOFT_CONNECT_FILE: &str = "bl-ubisoft-connect.txt";

/// Notes that a start through Ubisoft Connect worked (once; later ones leave it).
pub fn note_ubisoft_connect(game_dir: &Path) {
    let path = game_dir.join(UBISOFT_CONNECT_FILE);
    if !path.exists() {
        let _ = std::fs::write(&path, "This copy of the game started through Ubisoft Connect and signed in.\n");
    }
}

/// Whether a start through Ubisoft Connect worked in `game_dir`.
pub fn ubisoft_connect_worked(game_dir: &Path) -> bool {
    game_dir.join(UBISOFT_CONNECT_FILE).is_file()
}

/// The file's text.
pub fn write(exe: &str, steam: bool, launcher: bool) -> String {
    format!("exe={exe}\nsteam={steam}\nlauncher={launcher}\n")
}

pub fn parse(text: &str) -> Option<LastRun> {
    let mut exe = None;
    let mut steam = false;
    let mut launcher = false;
    for line in text.lines() {
        match line.split_once('=') {
            Some(("exe", v)) if !v.trim().is_empty() => exe = Some(v.trim().to_string()),
            Some(("steam", v)) => steam = v.trim() == "true",
            Some(("launcher", v)) => launcher = v.trim() == "true",
            _ => {}
        }
    }
    Some(LastRun {
        exe: exe?,
        steam,
        launcher,
        at: None,
    })
}

/// The last start in `game_dir`, if the add-on noted one.
pub fn read(game_dir: &Path) -> Option<LastRun> {
    let path = game_dir.join(FILE);
    let mut last = parse(&std::fs::read_to_string(&path).ok()?)?;
    last.at = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    Some(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let last = parse(&write("Blacklist_game.exe", true, false)).unwrap();
        assert!(last.dx9() && last.steam && !last.launcher);
        let last = parse(&write("Blacklist_DX11_game.exe", false, true)).unwrap();
        assert!(!last.dx9() && !last.steam && last.launcher);
        assert_eq!(parse("steam=true\n"), None);
    }
}
