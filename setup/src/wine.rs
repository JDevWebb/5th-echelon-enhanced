//! Running the game under Wine or Proton (Linux, Steam Deck).
//!
//! The game and its hook run inside a Wine prefix: a folder with its own
//! `drive_c`. For Steam, that's Proton's `steamapps/compatdata/<app>/pfx`,
//! which Proton creates the first time the game runs. The hook keeps its
//! saves in the prefix's AppData, so the launcher has to find the prefix
//! from the game's folder.

use std::path::Path;
use std::path::PathBuf;

/// Splinter Cell: Blacklist on Steam.
pub const STEAM_APP_ID: u32 = 235_600;

/// Under Wine, the game can freeze at start on CPUs with more threads than
/// this; `WINE_CPU_TOPOLOGY` makes it see fewer (found upstream, #98).
pub const MAX_THREADS: usize = 16;

/// A Wine prefix the game is installed in, or will run in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefix {
    /// The prefix: the folder that has (or will have) `drive_c`.
    pub root: PathBuf,
    /// Steam's Proton prefix, created by Proton on the game's first run.
    pub steam: bool,
}

impl Prefix {
    /// Whether the prefix exists yet. Proton creates Steam's on the first
    /// run; it must not be made by hand before that.
    pub fn ready(&self) -> bool {
        self.root.join("drive_c").is_dir()
    }

    /// The Windows user's roaming AppData in this prefix (`%APPDATA%` for the
    /// game), where the hook keeps its saves.
    pub fn roaming_dir(&self) -> PathBuf {
        let users = self.root.join("drive_c").join("users");
        let user = if self.steam {
            String::from("steamuser")
        } else {
            std::env::var("USER")
                .ok()
                .filter(|u| users.join(u).is_dir())
                .or_else(|| {
                    std::fs::read_dir(&users)
                        .ok()?
                        .flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .find(|n| n != "Public" && n != "steamuser")
                })
                .unwrap_or_else(|| String::from("steamuser"))
        };
        users.join(user).join("AppData").join("Roaming")
    }
}

/// The prefix the game in `game_dir` runs in: Proton's for a Steam library,
/// or the Wine prefix whose `drive_c` holds the game (Lutris, Heroic, plain
/// Wine).
pub fn prefix_for(game_dir: &Path) -> Option<Prefix> {
    for dir in game_dir.ancestors() {
        match dir.file_name().and_then(|n| n.to_str()) {
            Some("steamapps") => {
                return Some(Prefix {
                    root: steam_prefix(dir, crate::sys::steam_libraries),
                    steam: true,
                })
            }
            Some("drive_c") => {
                return Some(Prefix {
                    root: dir.parent()?.to_path_buf(),
                    steam: false,
                })
            }
            _ => {}
        }
    }
    None
}

/// Proton's prefix for the game in a library's `steamapps`.
fn proton_prefix(steamapps: &Path) -> PathBuf {
    steamapps.join("compatdata").join(STEAM_APP_ID.to_string()).join("pfx")
}

/// Proton's prefix for a game in `steamapps`: usually in the same library,
/// but a game moved to another library (an SD card, say) leaves its prefix
/// behind, so the other `libraries` are looked in too. Where it would be in
/// the game's own library when there's none yet.
fn steam_prefix(steamapps: &Path, libraries: impl FnOnce() -> Vec<PathBuf>) -> PathBuf {
    let own = proton_prefix(steamapps);
    if own.join("drive_c").is_dir() {
        return own;
    }
    libraries()
        .iter()
        .map(|l| proton_prefix(&l.join("steamapps")))
        .find(|p| p.join("drive_c").is_dir())
        .unwrap_or(own)
}

/// A Wine prefix that Proton made outside Steam (`umu-run`, Lutris or Heroic with a
/// Proton runner): Proton keeps its own files beside `drive_c` (or in the folder above a
/// `pfx` folder), and the prefix is meant to be run by Proton, not by the system's Wine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtonPrefix {
    /// The folder with Proton's own files (`version`, `config_info`): what umu takes as
    /// `WINEPREFIX`. The prefix itself, or the folder above its `pfx`.
    pub compat_dir: PathBuf,
    /// The Proton version that made the prefix (its `version` file, "11.0-100").
    pub version: Option<String>,
    /// The Proton that made the prefix, if it's still installed (from `config_info`).
    pub proton_dir: Option<PathBuf>,
}

impl ProtonPrefix {
    /// Whether `dir` has the files Proton leaves in a prefix it made.
    fn files_in(dir: &Path) -> bool {
        dir.join("version").is_file() && ["config_info", "pfx.lock", "creation_sync_guard"].iter().any(|f| dir.join(f).is_file())
    }

    /// The Proton prefix `prefix` is, or `None` for a Steam prefix (Steam starts it) or a
    /// plain Wine prefix.
    pub fn of(prefix: &Prefix) -> Option<Self> {
        if prefix.steam {
            return None;
        }
        let above = prefix.root.file_name().is_some_and(|n| n == "pfx").then(|| prefix.root.parent()).flatten();
        let compat_dir = [Some(prefix.root.as_path()), above].into_iter().flatten().find(|d| Self::files_in(d))?;
        let version = std::fs::read_to_string(compat_dir.join("version"))
            .ok()
            .and_then(|v| v.lines().next().map(|l| l.trim().to_string()))
            .filter(|v| !v.is_empty());
        Some(Self {
            compat_dir: compat_dir.to_path_buf(),
            version,
            proton_dir: proton_dir_in(&compat_dir.join("config_info")),
        })
    }
}

/// The Proton that made a prefix, from its `config_info`: a line that points into
/// `<Proton>/files/` (the fonts folder, say). Only if that Proton is still there.
fn proton_dir_in(config_info: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config_info).ok()?;
    text.lines()
        .filter_map(|l| l.trim().split_once("/files/").map(|(dir, _)| PathBuf::from(dir)))
        .find(|dir| dir.is_absolute() && dir.join("proton").is_file())
}

/// Where the program `name` is, in `PATH` or the user's `~/.local/bin` (a launcher started
/// from a menu may not have that in its `PATH`).
pub fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let local = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin"));
    std::env::split_paths(&path).chain(local).map(|d| d.join(name)).find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Why the game's save folder can't be found, for the player.
pub fn no_save_folder(game_dir: &Path) -> String {
    match prefix_for(game_dir) {
        Some(p) if p.steam && !p.ready() => format!(
            "Proton hasn't set the game up yet: there's nothing at {}. Start the game from Steam once, quit it, then come back.",
            p.root.display()
        ),
        Some(p) if !p.ready() => format!("The game's Wine prefix has no drive_c: {}.", p.root.display()),
        None if !cfg!(target_os = "windows") => "The game's folder isn't in a Steam library or a Wine prefix, so where it keeps its save can't be worked out.".to_string(),
        _ => "The save folder can't be found.".to_string(),
    }
}

/// This PC's CPU threads.
pub fn cpu_threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// A `WINE_CPU_TOPOLOGY` value showing the game `n` threads.
pub fn cpu_topology(n: usize) -> String {
    let cpus: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    format!("{n}:{}", cpus.join(","))
}

/// The Steam launch options that keep the game from freezing on this CPU,
/// if it needs any.
pub fn steam_launch_options() -> Option<String> {
    (cpu_threads() > MAX_THREADS).then(|| format!("WINE_CPU_TOPOLOGY={} %command%", cpu_topology(MAX_THREADS)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn steam_games_use_protons_prefix() {
        let game = Path::new("/home/deck/.local/share/Steam/steamapps/common/Tom Clancy's Splinter Cell Blacklist/src/SYSTEM");
        let p = prefix_for(game).unwrap();
        assert!(p.steam);
        assert_eq!(p.root, Path::new("/home/deck/.local/share/Steam/steamapps/compatdata/235600/pfx"));
        assert_eq!(p.roaming_dir(), p.root.join("drive_c/users/steamuser/AppData/Roaming"));
        assert!(!p.ready(), "Proton hasn't run the game here");
    }

    #[test]
    fn other_wine_prefixes_hold_the_game() {
        let root = temp_dir("wine-prefix");
        let game = root.join("drive_c/Program Files (x86)/Ubisoft/Ubisoft Game Launcher/games/Splinter Cell Blacklist/src/SYSTEM");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(root.join("drive_c/users/Public")).unwrap();
        std::fs::create_dir_all(root.join("drive_c/users/player")).unwrap();
        let p = prefix_for(&game).unwrap();
        assert!(!p.steam && p.ready());
        assert_eq!(p.root, root);
        assert!(p.roaming_dir().ends_with("AppData/Roaming"));
        assert!(!p.roaming_dir().to_string_lossy().contains("Public"));
        assert_eq!(prefix_for(Path::new("/opt/games/blacklist")), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_prefix_left_in_another_library_is_found() {
        let root = temp_dir("wine-moved");
        let sd = root.join("sdcard/steamapps");
        let internal = root.join("internal");
        let left = proton_prefix(&internal.join("steamapps"));
        // None anywhere yet: where Proton will make it, beside the game.
        assert_eq!(steam_prefix(&sd, || vec![internal.clone()]), proton_prefix(&sd));
        std::fs::create_dir_all(left.join("drive_c")).unwrap();
        assert_eq!(steam_prefix(&sd, || vec![internal.clone()]), left);
        // One beside the game wins.
        std::fs::create_dir_all(proton_prefix(&sd).join("drive_c")).unwrap();
        assert_eq!(steam_prefix(&sd, || vec![internal]), proton_prefix(&sd));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_prefix_proton_made_is_told_by_its_files() {
        let root = temp_dir("wine-proton");
        let proton = root.join("Proton 11.0");
        std::fs::create_dir_all(proton.join("files/share/fonts")).unwrap();
        std::fs::write(proton.join("proton"), "").unwrap();
        let config = format!("11.0-100\n{}/files/share/fonts/\n{}/files/lib/\n\n0\n", proton.display(), proton.display());
        let prefix = |dir: &Path| Prefix {
            root: dir.to_path_buf(),
            steam: false,
        };

        // umu-run: Proton's files and drive_c in one folder.
        let umu = root.join("umu");
        std::fs::create_dir_all(umu.join("drive_c")).unwrap();
        assert_eq!(ProtonPrefix::of(&prefix(&umu)), None, "no Proton files yet: plain Wine");
        std::fs::write(umu.join("version"), "11.0-100\n").unwrap();
        assert_eq!(ProtonPrefix::of(&prefix(&umu)), None, "a version file alone isn't enough");
        std::fs::write(umu.join("config_info"), &config).unwrap();
        let found = ProtonPrefix::of(&prefix(&umu)).unwrap();
        assert_eq!(found.compat_dir, umu);
        assert_eq!(found.version.as_deref(), Some("11.0-100"));
        assert_eq!(found.proton_dir, Some(proton.clone()));

        // Lutris or Heroic: the files are above `pfx`.
        let heroic = root.join("heroic");
        std::fs::create_dir_all(heroic.join("pfx/drive_c")).unwrap();
        std::fs::write(heroic.join("version"), "GE-Proton9-1\n").unwrap();
        std::fs::write(heroic.join("pfx.lock"), "").unwrap();
        let found = ProtonPrefix::of(&prefix(&heroic.join("pfx"))).unwrap();
        assert_eq!(found.compat_dir, heroic);
        assert_eq!(found.version.as_deref(), Some("GE-Proton9-1"));
        assert_eq!(found.proton_dir, None, "no config_info");
        // The folder above a prefix that isn't called pfx doesn't count.
        let other = root.join("other");
        std::fs::create_dir_all(other.join("wineprefix/drive_c")).unwrap();
        std::fs::write(other.join("version"), "1").unwrap();
        std::fs::write(other.join("pfx.lock"), "").unwrap();
        assert_eq!(ProtonPrefix::of(&prefix(&other.join("wineprefix"))), None);

        // A Proton that has been removed isn't named.
        std::fs::remove_file(proton.join("proton")).unwrap();
        assert_eq!(ProtonPrefix::of(&prefix(&umu)).unwrap().proton_dir, None);

        // Steam's own prefix is Steam's business.
        let steam = Prefix { root: umu.clone(), steam: true };
        assert_eq!(ProtonPrefix::of(&steam), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn topology() {
        assert_eq!(cpu_topology(4), "4:0,1,2,3");
    }
}
