//! Everything the launcher does to get a player into a 5th Echelon game,
//! without its UI: find the game, install the client, keep the settings,
//! set up an account, pick the network adapter, make a save, check that it
//! all works, and start the game.
//!
//! Most of it is plain file and socket work, tested on any host. The parts
//! that need Windows (registry, adapters, processes) live in `sys` and have
//! small stand-ins elsewhere.

pub mod account;
pub mod config;
pub mod diagnose;
pub mod directory;
pub mod feedback;
pub mod game;
pub mod install;
pub mod key_art;
pub mod launch;
pub mod net;
pub mod overrides;
pub mod player_identity;
pub mod save;
pub mod server_info;
pub mod update;
pub mod wine;
mod sys;

/// The server's gRPC API port (accounts, friends, invites, admin).
pub const API_PORT: u16 = 50051;
/// The server's online-config and community-API port.
pub const CONFIG_PORT: u16 = 80;
/// The game's Quazal (PRUDP) port on the server.
pub const QUAZAL_PORT: u16 = 21126;
/// The server's NAT helper port.
pub const NAT_PORT: u16 = 21128;

/// The launcher's own folder: `%APPDATA%\5th-Echelon` on Windows,
/// `~/.config/5th-Echelon` on Linux. (The game's saves are in the game's own
/// `%APPDATA%`; see `save::save_path`.)
pub fn app_data_dir() -> Option<std::path::PathBuf> {
    sys::roaming_app_data().map(|d| d.join("5th-Echelon"))
}

/// Writes `data` to `path` through a temporary file and a rename, so a crash
/// never leaves half a file.
pub(crate) fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Like [`write_atomic`], for secrets: on Linux only the user can read the
/// file (0600).
pub(crate) fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut f = options.open(&tmp)?;
        std::io::Write::write_all(&mut f, data)?;
    }
    #[cfg(unix)]
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    /// A fresh, empty folder for one test.
    pub fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fe-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
