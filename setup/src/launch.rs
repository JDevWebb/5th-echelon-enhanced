//! Starting the game. The hook in `uplay_r1_loader.dll` does the rest:
//! it reads `uplay.toml` (and `uplay.override.toml`) from the game folder.

use std::path::Path;
use std::process::Child;
use std::process::Command;

use crate::game::GameVersion;

/// How the game was started.
pub enum Launched {
    /// The game's own process (Windows, or Wine started directly).
    Game(Child),
    /// Through Steam, which starts the game on its own; watch for the game's
    /// process instead ([`crate::game::game_running`]).
    Steam,
    /// Asked to start through Steam, Steam couldn't be started: the game's own process
    /// instead, and why.
    GameNotSteam(Child, String),
    /// As administrator, because the game is set to run that way (its
    /// Compatibility setting or manifest) and a plain start was refused.
    /// Watched like [`Launched::Steam`], by the game's process.
    Elevated,
}

/// Whether the game in `game_dir` is a Steam copy: in a Steam library (a `steamapps` folder)
/// that has the game's app manifest.
pub fn steam_copy(game_dir: &Path) -> bool {
    game_dir
        .ancestors()
        .find(|d| d.file_name().is_some_and(|n| n.eq_ignore_ascii_case("steamapps")))
        .is_some_and(|steamapps| steamapps.join(format!("appmanifest_{}.acf", crate::wine::STEAM_APP_ID)).is_file())
}

/// Windows' ERROR_ELEVATION_REQUIRED: the program must run as administrator.
const ERROR_ELEVATION_REQUIRED: i32 = 740;

/// Starts `version` of the game from `game_dir`.
///
/// On Windows (and a Windows launcher under Wine) that's the executable
/// itself, or with `through_steam` for a Steam copy, Steam (for Steam Input:
/// Steam applies it only to games it starts; it then picks DirectX 9 or 11
/// itself). On Linux, Steam games start through Steam (with Proton and the
/// launch options the player set there); other Wine installs start with
/// `wine` in their own prefix.
pub fn launch(game_dir: &Path, version: GameVersion, through_steam: bool) -> std::io::Result<Launched> {
    let exe = version.full_path(game_dir);
    if !exe.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} isn't in {}", version.executable(), game_dir.display()),
        ));
    }
    if cfg!(target_os = "windows") {
        let steam_failed = if through_steam && steam_copy(game_dir) && !hooks_config::running_under_wine() {
            match start_through_steam_on_windows() {
                Ok(()) => return Ok(Launched::Steam),
                Err(e) => Some(e.to_string()),
            }
        } else {
            None
        };
        let mut cmd = Command::new(&exe);
        cmd.current_dir(game_dir);
        // A Windows launcher running under Wine: keep the game from freezing
        // on CPUs with many threads, as a Linux launcher would.
        if hooks_config::running_under_wine() && crate::wine::cpu_threads() > crate::wine::MAX_THREADS {
            cmd.env("WINE_CPU_TOPOLOGY", crate::wine::cpu_topology(crate::wine::MAX_THREADS));
        }
        return match cmd.spawn() {
            Ok(child) => Ok(match steam_failed {
                Some(why) => Launched::GameNotSteam(child, why),
                None => Launched::Game(child),
            }),
            Err(e) if e.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED) => start_elevated(&exe, game_dir),
            Err(e) => Err(e),
        };
    }
    match crate::wine::prefix_for(game_dir) {
        Some(prefix) if prefix.steam => start_through_steam().map(|()| Launched::Steam),
        prefix => {
            let mut cmd = Command::new("wine");
            cmd.arg(&exe).current_dir(game_dir);
            if let Some(prefix) = prefix {
                cmd.env("WINEPREFIX", &prefix.root);
            }
            if crate::wine::cpu_threads() > crate::wine::MAX_THREADS {
                cmd.env("WINE_CPU_TOPOLOGY", crate::wine::cpu_topology(crate::wine::MAX_THREADS));
            }
            cmd.spawn()
                .map(Launched::Game)
                .map_err(|e| std::io::Error::new(e.kind(), format!("couldn't run wine ({e}); start the game from the app you installed it with")))
        }
    }
}

/// Asks Steam on Windows to start the game: its own program (`-applaunch`), else a `steam://`
/// link through the shell.
fn start_through_steam_on_windows() -> std::io::Result<()> {
    let app = crate::wine::STEAM_APP_ID.to_string();
    let detach = |mut child: Child| {
        // Steam may be the process started here, and runs on after the game.
        std::thread::spawn(move || child.wait());
    };
    if let Some(steam) = crate::sys::steam_exe() {
        if let Ok(child) = Command::new(steam).args(["-applaunch", &app]).spawn() {
            detach(child);
            return Ok(());
        }
    }
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", "start", "", &format!("steam://rungameid/{app}")]);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        // No console window flashing up.
        cmd.creation_flags(0x0800_0000);
    }
    match cmd.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(_) | Err(_) => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Steam isn't installed or didn't answer")),
    }
}

/// Starts the game as administrator, with Windows asking the player (UAC).
#[cfg(target_os = "windows")]
fn start_elevated(exe: &Path, game_dir: &Path) -> std::io::Result<Launched> {
    tracing::info!("{} needs to run as administrator; starting it that way", exe.display());
    crate::sys::start_elevated(exe, game_dir).map(|()| Launched::Elevated)
}

#[cfg(not(target_os = "windows"))]
fn start_elevated(_exe: &Path, _game_dir: &Path) -> std::io::Result<Launched> {
    Err(std::io::Error::from_raw_os_error(ERROR_ELEVATION_REQUIRED))
}

/// Asks Steam to start the game: its own `steam` command, Flatpak Steam, or
/// a `steam://` link for whatever handles it.
fn start_through_steam() -> std::io::Result<()> {
    let app = crate::wine::STEAM_APP_ID.to_string();
    let attempts: [(&str, Vec<String>); 3] = [
        ("steam", vec!["-applaunch".into(), app.clone()]),
        ("flatpak", vec!["run".into(), "com.valvesoftware.Steam".into(), "-applaunch".into(), app.clone()]),
        ("xdg-open", vec![format!("steam://rungameid/{app}")]),
    ];
    let flatpak_steam = std::env::var_os("HOME").is_some_and(|h| Path::new(&h).join(".var/app/com.valvesoftware.Steam").is_dir());
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "Steam isn't installed");
    for (program, args) in attempts {
        if program == "flatpak" && !flatpak_steam {
            continue;
        }
        match Command::new(program).args(&args).spawn() {
            Ok(mut child) => {
                // Steam may run on after the game; don't leave a zombie.
                std::thread::spawn(move || child.wait());
                return Ok(());
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_steam_copy_is_one_in_a_library_with_the_games_manifest() {
        let root = std::env::temp_dir().join(format!("fe-steam-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("SteamLibrary/steamapps/common/Tom Clancy's Splinter Cell Blacklist");
        std::fs::create_dir_all(&game).unwrap();
        assert!(!steam_copy(&game), "a library without the game's manifest");
        std::fs::write(root.join("SteamLibrary/steamapps/appmanifest_235600.acf"), "\"AppState\" {}").unwrap();
        assert!(steam_copy(&game));
        let ubisoft = root.join("Ubisoft/games/Splinter Cell Blacklist");
        std::fs::create_dir_all(&ubisoft).unwrap();
        assert!(!steam_copy(&ubisoft), "not in a Steam library");
        let _ = std::fs::remove_dir_all(&root);
    }
}
