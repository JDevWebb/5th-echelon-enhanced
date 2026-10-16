//! The game: its versions, where it's installed, and whether it's running.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::sys;

/// The game's two executables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GameVersion {
    SplinterCellBlacklistDx9,
    SplinterCellBlacklistDx11,
}

impl GameVersion {
    pub const ALL: [GameVersion; 2] = [GameVersion::SplinterCellBlacklistDx11, GameVersion::SplinterCellBlacklistDx9];

    pub fn executable(self) -> &'static str {
        match self {
            GameVersion::SplinterCellBlacklistDx9 => "Blacklist_game.exe",
            GameVersion::SplinterCellBlacklistDx11 => "Blacklist_DX11_game.exe",
        }
    }

    pub fn full_path(self, game_dir: &Path) -> PathBuf {
        game_dir.join(self.executable())
    }

    pub fn label(self) -> &'static str {
        match self {
            GameVersion::SplinterCellBlacklistDx9 => "DirectX 9",
            GameVersion::SplinterCellBlacklistDx11 => "DirectX 11",
        }
    }
}

/// The versions installed in `game_dir`, DirectX 11 first.
pub fn installed_versions(game_dir: &Path) -> Vec<GameVersion> {
    GameVersion::ALL.into_iter().filter(|v| v.full_path(game_dir).is_file()).collect()
}

/// Whether `dir` is the game's executable folder.
pub fn is_game_dir(dir: &Path) -> bool {
    !installed_versions(dir).is_empty()
}

/// `wanted` if it's installed in `game_dir`, else the other one.
pub fn pick_version(game_dir: &Path, wanted: GameVersion) -> Option<GameVersion> {
    let installed = installed_versions(game_dir);
    installed.contains(&wanted).then_some(wanted).or_else(|| installed.first().copied())
}

/// Game folders on this PC, most likely first: folders already set up for
/// 5th Echelon (they have a `uplay.toml`), then the rest.
///
/// Looks next to the launcher, in the game's registry entry, in every Steam
/// library, in Ubisoft Connect's games folder, and in the usual folders on
/// every fixed drive.
pub fn find_game_dirs() -> Vec<PathBuf> {
    let mut found = Vec::new();
    // The launcher's own folder and its parents: a launcher dropped into the
    // game folder (or its root) always finds that copy first.
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        for dir in exe_dir.ancestors().take(3) {
            found.extend(game_dirs_under(dir, 1));
        }
    }
    for root in sys::install_roots() {
        found.extend(game_dirs_under(&root, 3));
    }
    for common in sys::library_roots() {
        for dir in blacklist_folders(&common) {
            found.extend(game_dirs_under(&dir, 3));
        }
    }
    rank(dedupe(found))
}

/// Steam library folders listed in a `libraryfolders.vdf`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn steam_libraries(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut quoted = line.split('"').skip(1).step_by(2);
            match (quoted.next(), quoted.next()) {
                (Some("path"), Some(path)) => Some(PathBuf::from(path.replace("\\\\", "\\"))),
                _ => None,
            }
        })
        .collect()
}

/// Subfolders of `dir` named like the game ("Splinter Cell Blacklist").
pub(crate) fn blacklist_folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.contains("splinter cell") && name.contains("blacklist")
        })
        .map(|e| e.path())
        .collect()
}

/// Game folders at or below `root`, down to `depth` levels. The usual
/// layouts (`src\SYSTEM`, `SYSTEM`) are checked first.
pub(crate) fn game_dirs_under(root: &Path, depth: usize) -> Vec<PathBuf> {
    for dir in [root.to_path_buf(), root.join("src").join("SYSTEM"), root.join("SYSTEM")] {
        if is_game_dir(&dir) {
            return vec![dir];
        }
    }
    if depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .flat_map(|e| game_dirs_under(&e.path(), depth - 1))
        .collect()
}

/// The game folder in or below `dir` (a folder a player picked: the game's
/// install folder, its `src\SYSTEM`, or something in between).
pub fn game_dir_in(dir: &Path) -> Option<PathBuf> {
    game_dirs_under(dir, 3).into_iter().next()
}

/// Drops duplicates (Windows paths compare case-insensitively).
fn dedupe(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    dirs.into_iter()
        .filter(|d| {
            let key = std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()).to_string_lossy().to_lowercase();
            seen.insert(key)
        })
        .collect()
}

/// Folders already set up for 5th Echelon first; otherwise keeps the order.
fn rank(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.sort_by_key(|d| !hooks_config::get_config_path(d).is_file());
    dirs
}

/// Whether either game executable is running.
pub fn game_running() -> bool {
    sys::process_running(&GameVersion::ALL.map(GameVersion::executable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn steam_library_paths() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}"#;
        assert_eq!(steam_libraries(vdf), [PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"D:\SteamLibrary")]);
    }

    #[test]
    fn finds_game_dirs_in_the_usual_layouts() {
        let root = temp_dir("game-layouts");
        let steam = root.join("common");
        touch(&steam.join("Tom Clancy's Splinter Cell Blacklist/src/SYSTEM/Blacklist_DX11_game.exe"));
        touch(&steam.join("Splinter Cell Blacklist (copy)/deep/er/Blacklist_game.exe"));
        touch(&steam.join("Other Game/Blacklist_game.exe"));

        let folders = blacklist_folders(&steam);
        assert_eq!(folders.len(), 2, "only folders named like the game");
        let mut dirs: Vec<PathBuf> = folders.iter().flat_map(|f| game_dirs_under(f, 3)).collect();
        dirs.sort();
        assert_eq!(dirs.len(), 2);
        assert!(dirs.iter().any(|d| d.ends_with("src/SYSTEM")));
        assert!(dirs.iter().any(|d| d.ends_with("deep/er")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prefers_dx11_and_set_up_folders() {
        let root = temp_dir("game-rank");
        let (a, b) = (root.join("a"), root.join("b"));
        touch(&a.join("Blacklist_game.exe"));
        touch(&b.join("Blacklist_game.exe"));
        touch(&b.join("Blacklist_DX11_game.exe"));
        touch(&b.join("uplay.toml"));

        assert_eq!(installed_versions(&b), [GameVersion::SplinterCellBlacklistDx11, GameVersion::SplinterCellBlacklistDx9]);
        assert_eq!(pick_version(&a, GameVersion::SplinterCellBlacklistDx11), Some(GameVersion::SplinterCellBlacklistDx9));
        assert_eq!(pick_version(&root, GameVersion::SplinterCellBlacklistDx11), None);
        assert_eq!(rank(dedupe(vec![a.clone(), b.clone(), a.clone()])), [b, a]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
