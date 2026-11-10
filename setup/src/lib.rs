//! Everything the launcher does to get a player into a 5th Echelon game,
//! without its UI: find the game, install the client, keep the settings,
//! set up an account, pick the network adapter, make a save, check that it
//! all works, and start the game.
//!
//! Most of it is plain file and socket work, tested on any host. The parts
//! that need Windows (registry, adapters, processes) live in `sys` and have
//! small stand-ins elsewhere.

pub mod account;
pub mod clock;
pub mod config;
pub mod diagnose;
pub mod directory;
pub mod feedback;
pub mod game;
pub mod gamma;
pub mod install;
pub mod key_art;
pub mod launch;
pub mod net;
pub mod overrides;
pub mod player_identity;
pub mod roadmap;
pub mod save;
pub mod server_info;
pub mod support;
mod sys;
pub mod update;
pub mod vkbasalt;
pub mod wine;

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
/// file (0600), from its first byte. A `.tmp` left from before (another
/// mode, another owner's link) is removed first, so the file is always made
/// new here.
pub fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut f = options.open(&tmp)?;
        std::io::Write::write_all(&mut f, data)?;
    }
    #[cfg(unix)]
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    std::fs::rename(&tmp, path)
}

#[cfg(all(test, unix))]
mod private_tests {
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_private_file_is_the_users_alone_whatever_was_left_before() {
        let dir = super::testutil::temp_dir("write-private");
        let path = dir.join("secret.toml");
        // A stale .tmp anyone could read, from an earlier run.
        std::fs::write(dir.join("secret.toml.tmp"), "old").unwrap();
        std::fs::set_permissions(dir.join("secret.toml.tmp"), std::fs::Permissions::from_mode(0o644)).unwrap();
        super::write_private(&path, b"password").unwrap();
        assert_eq!((std::fs::read(&path).unwrap(), mode(&path)), (b"password".to_vec(), 0o600));
        assert!(!dir.join("secret.toml.tmp").exists());
        // Written over: still the user's alone.
        super::write_private(&path, b"new").unwrap();
        assert_eq!((std::fs::read(&path).unwrap(), mode(&path)), (b"new".to_vec(), 0o600));
    }

    #[test]
    fn a_settings_backup_is_the_users_alone() {
        let dir = super::testutil::temp_dir("backup-private");
        let path = dir.join("uplay.toml");
        // Upstream's settings (no format), readable by anyone: started over, kept as a backup.
        std::fs::write(
            &path,
            "DefaultGame = \"SplinterCellBlacklistDx9\"\nApiServer = \"http://192.168.1.10:50051\"\nConfigServer = \"192.168.1.10\"\n[User]\nUsername = \"Nexus\"\nPassword = \"pw12345678\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loaded = crate::config::Config::load(&dir);
        let kept = loaded.started_over().expect("started over");
        assert_eq!(mode(kept), 0o600, "{}", kept.display());
        assert!(std::fs::read_to_string(kept).unwrap().contains("pw12345678"), "the old file, as it was");
        // An unreadable one is kept as .broken, the user's alone too.
        std::fs::write(&path, "password = = \"b\"").unwrap();
        let _ = crate::config::Config::load(&dir);
        assert_eq!(mode(&dir.join("uplay.toml.broken")), 0o600);
    }
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
