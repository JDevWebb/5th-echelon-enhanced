//! Starting the game. The hook in `uplay_r1_loader.dll` does the rest:
//! it reads `uplay.toml` (and `uplay.override.toml`) from the game folder.

use std::path::Path;
use std::path::PathBuf;
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
    /// Wine started the game in a prefix Proton made, because umu-launcher isn't installed:
    /// the system's Wine can hang there. Say so.
    GameProtonWithoutUmu(Child),
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
/// launch options the player set there); a prefix Proton made outside Steam (`umu-run`,
/// Lutris, Heroic) starts with `umu-run`, in the Proton that made it; other Wine installs
/// start with `wine` in their own prefix.
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
    let prefix = crate::wine::prefix_for(game_dir);
    let start = choose_start(prefix.as_ref(), crate::wine::find_program("umu-run"));
    if matches!(start, LinuxStart::Steam) {
        return start_through_steam().map(|()| Launched::Steam);
    }
    let mut cmd = linux_command(&start, &exe, game_dir, |name| std::env::var_os(name).is_some());
    if crate::wine::cpu_threads() > crate::wine::MAX_THREADS {
        cmd.env("WINE_CPU_TOPOLOGY", crate::wine::cpu_topology(crate::wine::MAX_THREADS));
    }
    let program = if matches!(start, LinuxStart::Umu { .. }) { "umu-run" } else { "wine" };
    let child = cmd
        .spawn()
        .map_err(|e| std::io::Error::new(e.kind(), format!("couldn't run {program} ({e}); start the game from the app you installed it with")))?;
    Ok(match start {
        LinuxStart::Wine { proton: true, .. } => Launched::GameProtonWithoutUmu(child),
        _ => Launched::Game(child),
    })
}

/// How the game starts on Linux.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinuxStart {
    /// Through Steam, which has Proton.
    Steam,
    /// Through `umu-run`: the Proton prefix is run by Proton, as the app that made it did.
    Umu {
        umu_run: PathBuf,
        /// `WINEPREFIX` for umu.
        prefix: PathBuf,
        /// The Proton that made the prefix, if it's still installed.
        proton_dir: Option<PathBuf>,
    },
    /// With the system's `wine`, in `prefix`. `proton`: the prefix is Proton's, but there's
    /// no umu to run it with.
    Wine { prefix: Option<PathBuf>, proton: bool },
}

/// Picks how to start the game in `prefix`: Steam copies through Steam; a Proton prefix
/// outside Steam through `umu_run` (when there is one); anything else with `wine`.
fn choose_start(prefix: Option<&crate::wine::Prefix>, umu_run: Option<PathBuf>) -> LinuxStart {
    let Some(prefix) = prefix else {
        return LinuxStart::Wine { prefix: None, proton: false };
    };
    if prefix.steam {
        return LinuxStart::Steam;
    }
    match (crate::wine::ProtonPrefix::of(prefix), umu_run) {
        (Some(proton), Some(umu_run)) => {
            tracing::info!(
                "the prefix was made by Proton {}; starting the game with umu-run",
                proton.version.as_deref().unwrap_or("(unknown version)")
            );
            LinuxStart::Umu {
                umu_run,
                prefix: proton.compat_dir,
                proton_dir: proton.proton_dir,
            }
        }
        (Some(_), None) => {
            tracing::warn!("the prefix was made by Proton, but umu-run isn't installed; starting the game with the system's wine");
            LinuxStart::Wine {
                prefix: Some(prefix.root.clone()),
                proton: true,
            }
        }
        (None, _) => LinuxStart::Wine {
            prefix: Some(prefix.root.clone()),
            proton: false,
        },
    }
}

/// The command for `start`. `set` says whether the player's environment already has a
/// variable: `PROTONPATH` and `GAMEID` are theirs to choose then.
fn linux_command(start: &LinuxStart, exe: &Path, game_dir: &Path, set: impl Fn(&str) -> bool) -> Command {
    let mut cmd;
    match start {
        LinuxStart::Steam => unreachable!("Steam copies start through Steam"),
        LinuxStart::Umu { umu_run, prefix, proton_dir } => {
            cmd = Command::new(umu_run);
            cmd.env("WINEPREFIX", prefix);
            if !set("GAMEID") {
                // Blacklist's Steam app: umu applies its fixes for the game.
                cmd.env("GAMEID", format!("umu-{}", crate::wine::STEAM_APP_ID));
            }
            if let (false, Some(dir)) = (set("PROTONPATH"), proton_dir) {
                cmd.env("PROTONPATH", dir);
            }
        }
        LinuxStart::Wine { prefix, .. } => {
            cmd = Command::new("wine");
            if let Some(prefix) = prefix {
                cmd.env("WINEPREFIX", prefix);
            }
        }
    }
    cmd.arg(exe).current_dir(game_dir);
    cmd
}

/// Whether the game in `game_dir` would start with the system's Wine in a prefix Proton made:
/// a prefix umu-launcher should run, but isn't installed.
pub fn proton_without_umu(game_dir: &Path) -> bool {
    let prefix = crate::wine::prefix_for(game_dir);
    matches!(choose_start(prefix.as_ref(), crate::wine::find_program("umu-run")), LinuxStart::Wine { proton: true, .. })
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

    fn proton_prefix(root: &Path) -> crate::wine::Prefix {
        std::fs::create_dir_all(root.join("drive_c")).unwrap();
        std::fs::write(root.join("version"), "11.0-100\n").unwrap();
        std::fs::write(root.join("config_info"), "11.0-100\n").unwrap();
        crate::wine::Prefix {
            root: root.to_path_buf(),
            steam: false,
        }
    }

    fn envs(cmd: &Command) -> Vec<(String, String)> {
        let mut envs: Vec<_> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
            .collect();
        envs.sort();
        envs
    }

    #[test]
    fn a_proton_prefix_starts_with_umu_when_it_is_installed() {
        let root = crate::testutil::temp_dir("launch-umu");
        let prefix = proton_prefix(&root.join("pfx"));
        let proton = root.join("Proton 11.0");
        std::fs::create_dir_all(&proton).unwrap();
        std::fs::write(proton.join("proton"), "").unwrap();
        std::fs::write(prefix.root.join("config_info"), format!("11.0-100\n{}/files/share/fonts/\n", proton.display())).unwrap();
        let umu = PathBuf::from("/usr/bin/umu-run");
        let start = choose_start(Some(&prefix), Some(umu.clone()));
        assert_eq!(
            start,
            LinuxStart::Umu {
                umu_run: umu,
                prefix: prefix.root.clone(),
                proton_dir: Some(proton.clone())
            }
        );
        let exe = Path::new("/games/src/SYSTEM/Blacklist_DX11_game.exe");
        let cmd = linux_command(&start, exe, Path::new("/games/src/SYSTEM"), |_| false);
        assert_eq!(cmd.get_program(), "/usr/bin/umu-run");
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), [exe.as_os_str()]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/games/src/SYSTEM")));
        assert_eq!(
            envs(&cmd),
            [
                ("GAMEID".to_string(), "umu-235600".to_string()),
                ("PROTONPATH".to_string(), proton.display().to_string()),
                ("WINEPREFIX".to_string(), prefix.root.display().to_string()),
            ]
        );
        // What the player set themselves stays theirs.
        let cmd = linux_command(&start, exe, Path::new("/games"), |name| name == "PROTONPATH" || name == "GAMEID");
        assert_eq!(envs(&cmd), [("WINEPREFIX".to_string(), prefix.root.display().to_string())]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_proton_prefix_without_umu_falls_back_to_wine_and_says_so() {
        let root = crate::testutil::temp_dir("launch-no-umu");
        let prefix = proton_prefix(&root.join("pfx"));
        let start = choose_start(Some(&prefix), None);
        assert_eq!(
            start,
            LinuxStart::Wine {
                prefix: Some(prefix.root.clone()),
                proton: true
            }
        );
        let cmd = linux_command(&start, Path::new("/g/game.exe"), Path::new("/g"), |_| false);
        assert_eq!(cmd.get_program(), "wine");
        assert_eq!(envs(&cmd), [("WINEPREFIX".to_string(), prefix.root.display().to_string())]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn steam_copies_and_plain_wine_prefixes_keep_their_ways() {
        let root = crate::testutil::temp_dir("launch-plain");
        let umu = Some(PathBuf::from("/usr/bin/umu-run"));
        // Steam: through Steam, with or without umu, and even when Proton's files are there.
        let mut steam = proton_prefix(&root.join("compatdata/235600/pfx"));
        steam.steam = true;
        assert_eq!(choose_start(Some(&steam), umu.clone()), LinuxStart::Steam);
        // A plain Wine prefix: wine, with or without umu, and no Proton warning.
        let plain = crate::wine::Prefix {
            root: root.join("wine"),
            steam: false,
        };
        std::fs::create_dir_all(plain.root.join("drive_c")).unwrap();
        for umu in [umu, None] {
            let start = choose_start(Some(&plain), umu);
            assert_eq!(
                start,
                LinuxStart::Wine {
                    prefix: Some(plain.root.clone()),
                    proton: false
                }
            );
            let cmd = linux_command(&start, Path::new("/g/game.exe"), Path::new("/g"), |_| false);
            assert_eq!(cmd.get_program(), "wine");
            assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["/g/game.exe"]);
        }
        // No prefix found: wine in the default prefix.
        assert_eq!(choose_start(None, None), LinuxStart::Wine { prefix: None, proton: false });
        let cmd = linux_command(&choose_start(None, None), Path::new("/g/game.exe"), Path::new("/g"), |_| false);
        assert!(envs(&cmd).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
