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
    rank(dedupe(found.iter().map(|d| tidy_dir(d)).collect()))
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

/// The launch options set for `app` in a Steam `localconfig.vdf` (a player's own settings:
/// `"apps" { "235600" { "LaunchOptions" "-offline" } }`), if any.
pub fn steam_launch_options(vdf: &str, app: u32) -> Option<String> {
    let app = app.to_string();
    // Quoted strings (backslash escapes) and braces, in order.
    let mut tokens = Vec::new();
    let mut chars = vdf.chars();
    while let Some(c) = chars.next() {
        match c {
            '{' | '}' => tokens.push(c.to_string()),
            '"' => {
                let mut text = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => text.extend(chars.next()),
                        '"' => break,
                        c => text.push(c),
                    }
                }
                tokens.push(format!("\"{text}"));
            }
            _ => {}
        }
    }
    let mut path: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "}" => {
                path.pop();
                i += 1;
            }
            "{" => i += 1,
            key => {
                let key = key.trim_start_matches('"');
                match tokens.get(i + 1).map(String::as_str) {
                    Some("{") => {
                        path.push(key.to_ascii_lowercase());
                        i += 2;
                    }
                    Some(value) if value.starts_with('"') => {
                        let in_app = path.len() >= 2 && path[path.len() - 1] == app && path[path.len() - 2] == "apps";
                        if in_app && key.eq_ignore_ascii_case("LaunchOptions") {
                            return Some(value.trim_start_matches('"').to_string());
                        }
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
        }
    }
    None
}

/// The switch in launch options that keeps the game offline (`-offline`, `-offline_mode`).
pub fn offline_switch(options: &str) -> Option<&str> {
    options
        .split_whitespace()
        .map(|a| a.trim_matches('"'))
        .find(|a| a.eq_ignore_ascii_case("-offline") || a.eq_ignore_ascii_case("-offline_mode"))
}

/// The game's launch options in this PC's Steam, for the player signed in to it last (R6:
/// only theirs, not every account's that ever signed in there), when they keep the game
/// offline (Wikowikor's: `-offline -offline_mode`).
pub fn steam_offline_options() -> Option<String> {
    for steam in crate::sys::steam_installs() {
        let Some(account) = std::fs::read_to_string(steam.join("config").join("loginusers.vdf"))
            .ok()
            .and_then(|v| most_recent_account(&v))
        else {
            continue;
        };
        let Ok(vdf) = std::fs::read_to_string(steam.join("userdata").join(account.to_string()).join("config").join("localconfig.vdf")) else {
            continue;
        };
        if let Some(options) = steam_launch_options(&vdf, crate::wine::STEAM_APP_ID).filter(|o| offline_switch(o).is_some()) {
            return Some(options);
        }
    }
    None
}

/// The account id (its `userdata` folder) of the user Steam's `loginusers.vdf` marks
/// `MostRecent`: their SteamID64 less the individual-account base.
fn most_recent_account(vdf: &str) -> Option<u64> {
    const BASE: u64 = 76_561_197_960_265_728;
    let mut current: Option<u64> = None;
    for line in vdf.lines() {
        let parts: Vec<&str> = line.split('"').filter(|p| !p.trim().is_empty()).collect();
        match parts.as_slice() {
            [id] if id.len() == 17 && id.bytes().all(|b| b.is_ascii_digit()) => current = id.parse().ok(),
            [key, value] if key.eq_ignore_ascii_case("MostRecent") && *value == "1" => {
                return current.and_then(|id| id.checked_sub(BASE));
            }
            _ => {}
        }
    }
    None
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
/// A folder as Windows writes it, '\' throughout: one saved with '/' in it ("D:/Splinter Cell
/// Blacklist\src\SYSTEM", from an older launcher or a typed path) shows and compares the same as
/// the one found. Elsewhere as it is.
pub fn tidy_dir(dir: &Path) -> PathBuf {
    if cfg!(target_os = "windows") {
        PathBuf::from(dir.to_string_lossy().replace('/', "\\"))
    } else {
        dir.to_path_buf()
    }
}

/// Whether two paths are the same folder: as the file system resolves them where it can, else
/// tidied ([`tidy_dir`]), without a trailing separator, and on Windows in any case.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    let key = |p: &Path| {
        let tidy = tidy_dir(p);
        let full = std::fs::canonicalize(&tidy).unwrap_or(tidy);
        let text = full.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
        if cfg!(target_os = "windows") {
            text.to_lowercase()
        } else {
            text
        }
    };
    key(a) == key(b)
}

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

    #[test]
    fn the_same_folder_written_another_way() {
        let dir = std::env::temp_dir();
        let with_slash = PathBuf::from(format!("{}/", dir.display()));
        assert!(same_dir(&dir, &with_slash), "a trailing separator");
        assert!(!same_dir(&dir, &dir.join("elsewhere")));
        if cfg!(target_os = "windows") {
            assert_eq!(
                tidy_dir(Path::new("D:/Splinter Cell Blacklist\\src\\SYSTEM")),
                PathBuf::from("D:\\Splinter Cell Blacklist\\src\\SYSTEM")
            );
            assert!(same_dir(Path::new("D:/Games\\SYSTEM"), Path::new("d:\\games\\system")));
        } else {
            assert_eq!(tidy_dir(Path::new("/games/a")), PathBuf::from("/games/a"), "as it is elsewhere");
        }
    }

    #[test]
    fn only_the_last_steam_user_is_read() {
        let vdf = r#""users"
{
	"76561197960265729"
	{
		"AccountName"		"someone"
		"MostRecent"		"0"
	}
	"76561197960265740"
	{
		"AccountName"		"player"
		"MostRecent"		"1"
	}
}"#;
        assert_eq!(most_recent_account(vdf), Some(12));
        assert_eq!(most_recent_account("\"users\"\n{\n}"), None);
    }

    #[test]
    fn steam_launch_options_are_read_for_the_game_only() {
        let vdf = r#""UserLocalConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"apps"
				{
					"220"
					{
						"LaunchOptions"		"-novid"
					}
					"235600"
					{
						"LastPlayed"		"1792619695"
						"LaunchOptions"		"-uplay_steam_mode -offline -offline_mode"
					}
				}
			}
		}
	}
}"#;
        let options = steam_launch_options(vdf, 235_600);
        assert_eq!(options.as_deref(), Some("-uplay_steam_mode -offline -offline_mode"));
        assert_eq!(offline_switch(options.as_deref().unwrap()), Some("-offline"));
        assert_eq!(steam_launch_options(vdf, 1), None);
        assert_eq!(
            steam_launch_options(r#""apps" { "235600" { "LaunchOptions" "say \"hi\"" } }"#, 235_600).as_deref(),
            Some("say \"hi\"")
        );
        assert_eq!(offline_switch("-windowed -offlinex"), None);
    }
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
