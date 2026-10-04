//! What needs the operating system: where games are installed, network
//! adapters, running processes and known folders. Real on Windows; small
//! stand-ins elsewhere so the rest can be built and tested anywhere.

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
pub use self::win::*;

#[cfg(not(target_os = "windows"))]
mod other {
    use std::net::IpAddr;
    use std::path::Path;
    use std::path::PathBuf;

    /// Folders the game's installers record (Windows registry only).
    pub fn install_roots() -> Vec<PathBuf> {
        Vec::new()
    }

    fn home() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    /// Steam installs: native, Flatpak and Snap.
    fn steam_dirs() -> Vec<PathBuf> {
        let Some(home) = home() else { return Vec::new() };
        let mut dirs: Vec<PathBuf> = [
            ".local/share/Steam",
            ".steam/steam",
            ".steam/root",
            ".var/app/com.valvesoftware.Steam/.local/share/Steam",
            "snap/steam/common/.local/share/Steam",
        ]
        .iter()
        .map(|d| home.join(d))
        .filter(|d| d.join("steamapps").is_dir())
        .collect();
        // .steam/steam is usually a link to .local/share/Steam.
        let mut seen = std::collections::HashSet::new();
        dirs.retain(|d| seen.insert(std::fs::canonicalize(d).unwrap_or_else(|_| d.clone())));
        dirs
    }

    /// Every Steam library (the folders holding `steamapps`): each Steam
    /// install's own and those its `libraryfolders.vdf` lists, an SD card too.
    pub fn steam_libraries() -> Vec<PathBuf> {
        let mut libraries = Vec::new();
        for steam in steam_dirs() {
            let vdf = std::fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
            libraries.extend(crate::game::steam_libraries(&vdf));
            libraries.push(steam);
        }
        libraries
    }

    /// Where Ubisoft Connect keeps games inside a Wine prefix.
    fn ubisoft_games(prefix: &Path) -> [PathBuf; 2] {
        let drive_c = prefix.join("drive_c");
        [
            drive_c.join("Program Files (x86)/Ubisoft/Ubisoft Game Launcher/games"),
            drive_c.join("Program Files/Ubisoft/Ubisoft Game Launcher/games"),
        ]
    }

    /// Folders that hold one folder per installed game: every Steam library
    /// (an SD card too, when Steam lists it), and Ubisoft Connect's games in
    /// the usual Wine prefixes (Lutris, Heroic, ~/.wine).
    pub fn library_roots() -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = steam_libraries().into_iter().map(|l| l.join("steamapps/common")).collect();
        if let Some(home) = home() {
            roots.extend(ubisoft_games(&home.join(".wine")));
            for parent in ["Games", "Games/Heroic/Prefixes", "Games/Heroic/Prefixes/default"] {
                if let Ok(entries) = std::fs::read_dir(home.join(parent)) {
                    for prefix in entries.flatten() {
                        roots.extend(ubisoft_games(&prefix.path()));
                    }
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        roots.retain(|r| r.is_dir() && seen.insert(std::fs::canonicalize(r).unwrap_or_else(|_| r.clone())));
        roots
    }

    /// Whether a process runs one of `names` (the game's .exe, under Wine).
    #[cfg(target_os = "linux")]
    pub fn process_running(names: &[&str]) -> bool {
        let Ok(procs) = std::fs::read_dir("/proc") else { return false };
        procs.flatten().any(|p| {
            let cmdline = std::fs::read(p.path().join("cmdline")).unwrap_or_default();
            let cmdline = String::from_utf8_lossy(&cmdline).to_lowercase();
            // The first argument is the program; Wine shows the Windows path.
            let program = cmdline.split('\0').next().unwrap_or_default();
            names.iter().any(|n| program.ends_with(&n.to_lowercase()))
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn process_running(_names: &[&str]) -> bool {
        false
    }

    /// Where this user's application settings go: `$XDG_CONFIG_HOME` or
    /// `~/.config` (`%APPDATA%` on Windows).
    pub fn roaming_app_data() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home().map(|h| h.join(".config")))
    }

    /// The game's `%APPDATA%`: inside the Wine prefix it runs in.
    pub fn game_roaming_dir(game_dir: &Path) -> Option<PathBuf> {
        crate::wine::prefix_for(game_dir).filter(crate::wine::Prefix::ready).map(|p| p.roaming_dir())
    }

    /// Ubisoft Connect's install folder (Windows registry only).
    pub fn ubisoft_launcher_dir() -> Option<PathBuf> {
        None
    }

    #[cfg(unix)]
    pub fn adapters() -> Vec<(String, IpAddr)> {
        use std::collections::BTreeMap;
        use std::net::Ipv4Addr;

        let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
        if unsafe { libc::getifaddrs(&mut addrs) } != 0 {
            return Vec::new();
        }
        let mut found = BTreeMap::new();
        let mut current = unsafe { addrs.as_ref() };
        while let Some(ifaddr) = current {
            let family = unsafe { ifaddr.ifa_addr.as_ref() }.map(|a| i32::from(a.sa_family));
            if family == Some(libc::AF_INET) {
                let name = unsafe { std::ffi::CStr::from_ptr(ifaddr.ifa_name) }.to_string_lossy().into_owned();
                let sin = unsafe { &*ifaddr.ifa_addr.cast::<libc::sockaddr_in>() };
                found.insert(name, IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))));
            }
            current = unsafe { ifaddr.ifa_next.as_ref() };
        }
        unsafe { libc::freeifaddrs(addrs) };
        found.into_iter().collect()
    }

    #[cfg(not(unix))]
    pub fn adapters() -> Vec<(String, IpAddr)> {
        Vec::new()
    }
}
#[cfg(not(target_os = "windows"))]
pub use other::*;
