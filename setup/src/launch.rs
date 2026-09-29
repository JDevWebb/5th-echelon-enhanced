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
}

/// Starts `version` of the game from `game_dir`.
///
/// On Windows (and a Windows launcher under Wine) that's the executable
/// itself. On Linux, Steam games start through Steam (with Proton and the
/// launch options the player set there); other Wine installs start with
/// `wine` in their own prefix.
pub fn launch(game_dir: &Path, version: GameVersion) -> std::io::Result<Launched> {
    let exe = version.full_path(game_dir);
    if !exe.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} isn't in {}", version.executable(), game_dir.display())));
    }
    if cfg!(target_os = "windows") {
        let mut cmd = Command::new(exe);
        cmd.current_dir(game_dir);
        // A Windows launcher running under Wine: keep the game from freezing
        // on CPUs with many threads, as a Linux launcher would.
        if hooks_config::running_under_wine() && crate::wine::cpu_threads() > crate::wine::MAX_THREADS {
            cmd.env("WINE_CPU_TOPOLOGY", crate::wine::cpu_topology(crate::wine::MAX_THREADS));
        }
        return cmd.spawn().map(Launched::Game);
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
            cmd.spawn().map(Launched::Game).map_err(|e| {
                std::io::Error::new(e.kind(), format!("couldn't run wine ({e}); start the game from the app you installed it with"))
            })
        }
    }
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
