//! Installing the client: our `uplay_r1_loader.dll` in the game folder, with
//! the game's own DLL kept as `uplay_r1_loader.orig.dll` (the hook forwards
//! to it).

use std::io;
use std::path::Path;

/// The DLL the game loads, which we replace.
pub const DLL_NAME: &str = "uplay_r1_loader.dll";
/// The game's own DLL, kept for the hook to forward calls to.
pub const ORIG_DLL_NAME: &str = "uplay_r1_loader.orig.dll";

/// Our DLL's product name contains this (UTF-16 in its version resource).
const HOOK_MARKER: &str = "for 5th Echelon";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    /// No `uplay_r1_loader.dll`: not a game folder, or a broken install.
    NoGameDll,
    /// The game's own DLL; not installed yet.
    NotInstalled,
    /// A 5th Echelon DLL that isn't this launcher's (older, newer, or upstream's).
    Different,
    /// This launcher's DLL.
    Installed,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{DLL_NAME} is missing from the game folder. Verify the game's files in Steam or Ubisoft Connect, then try again.")]
    NoGameDll,
    #[error("The game's own {DLL_NAME} was replaced before and no copy of it was kept. Verify the game's files in Steam or Ubisoft Connect, then try again.")]
    OriginalLost,
    #[error("The game is running. Close Splinter Cell: Blacklist and try again.")]
    GameRunning,
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Whether `data` is one of our DLLs (upstream's or ours).
pub fn is_5th_echelon_dll(data: &[u8]) -> bool {
    let marker: Vec<u8> = HOOK_MARKER.encode_utf16().flat_map(u16::to_le_bytes).collect();
    data.windows(marker.len()).any(|w| w == marker)
}

/// What's installed in `game_dir`, compared with `bundled` (the DLL this
/// launcher carries).
pub fn client_state(game_dir: &Path, bundled: &[u8]) -> ClientState {
    let Ok(installed) = std::fs::read(game_dir.join(DLL_NAME)) else {
        return ClientState::NoGameDll;
    };
    if installed == bundled {
        ClientState::Installed
    } else if is_5th_echelon_dll(&installed) {
        ClientState::Different
    } else {
        ClientState::NotInstalled
    }
}

/// Installs `bundled` as the game's `uplay_r1_loader.dll`, keeping the
/// game's own DLL first. Does nothing if it's already installed.
pub fn install(game_dir: &Path, bundled: &[u8]) -> Result<(), InstallError> {
    let dll = game_dir.join(DLL_NAME);
    let orig = game_dir.join(ORIG_DLL_NAME);
    let installed = match std::fs::read(&dll) {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(InstallError::NoGameDll),
        Err(e) => return Err(e.into()),
    };
    if installed == bundled {
        return Ok(());
    }
    // The game's own DLL, kept once. A copy cut short (a full disk, a crash while writing)
    // isn't one: kept again while the game's own is still there to keep.
    let kept = std::fs::read(&orig).ok().filter(|o| looks_like_dll(o));
    if kept.is_none() {
        if is_5th_echelon_dll(&installed) {
            return Err(InstallError::OriginalLost);
        }
        // Written whole, then renamed into place: never a half-written copy under its name.
        let tmp = game_dir.join(format!("{ORIG_DLL_NAME}.tmp"));
        {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&installed)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &orig).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
    }
    let new = game_dir.join(format!("{DLL_NAME}.new"));
    std::fs::write(&new, bundled)?;
    std::fs::rename(&new, &dll).map_err(|e| {
        let _ = std::fs::remove_file(&new);
        if in_use(&e) {
            InstallError::GameRunning
        } else {
            e.into()
        }
    })
}

/// Whether `data` could be a Windows DLL: the DOS header, and the PE header where it says.
fn looks_like_dll(data: &[u8]) -> bool {
    let pe = data.get(0x3c..0x40).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    data.starts_with(b"MZ") && pe.is_some_and(|at| data.get(at..at.saturating_add(4)) == Some(b"PE\0\0"))
}

/// Puts the game's own DLL back (uninstalls the client).
pub fn uninstall(game_dir: &Path) -> Result<(), InstallError> {
    let orig = game_dir.join(ORIG_DLL_NAME);
    if !orig.exists() {
        return Ok(());
    }
    std::fs::copy(&orig, game_dir.join(DLL_NAME)).map_err(|e| if in_use(&e) { InstallError::GameRunning } else { e.into() })?;
    std::fs::remove_file(orig)?;
    Ok(())
}

/// A file that can't be replaced because a running program has it open.
fn in_use(e: &io::Error) -> bool {
    // ERROR_SHARING_VIOLATION (32) and ERROR_LOCK_VIOLATION (33).
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32 | 33)) && cfg!(windows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    /// The game's own DLL, as far as the checks look: a DOS header pointing at a PE one.
    fn theirs() -> Vec<u8> {
        let mut data = b"MZ".to_vec();
        data.resize(0x3c, 0);
        data.extend(64u32.to_le_bytes());
        data.extend(b"PE\0\0 ubisoft");
        data
    }

    fn ours(tag: &str) -> Vec<u8> {
        let mut data = format!("MZ {tag} ").into_bytes();
        data.extend("UPlay R1 Loader for 5th Echelon Enhanced".encode_utf16().flat_map(u16::to_le_bytes));
        data
    }

    #[test]
    fn installs_and_keeps_the_original_once() {
        let dir = temp_dir("install");
        std::fs::write(dir.join(DLL_NAME), theirs()).unwrap();
        assert_eq!(client_state(&dir, &ours("v1")), ClientState::NotInstalled);

        install(&dir, &ours("v1")).unwrap();
        assert_eq!(client_state(&dir, &ours("v1")), ClientState::Installed);
        assert_eq!(std::fs::read(dir.join(ORIG_DLL_NAME)).unwrap(), theirs());

        // An update replaces ours and leaves the original alone.
        assert_eq!(client_state(&dir, &ours("v2")), ClientState::Different);
        install(&dir, &ours("v2")).unwrap();
        assert_eq!(std::fs::read(dir.join(DLL_NAME)).unwrap(), ours("v2"));
        assert_eq!(std::fs::read(dir.join(ORIG_DLL_NAME)).unwrap(), theirs());

        uninstall(&dir).unwrap();
        assert_eq!(std::fs::read(dir.join(DLL_NAME)).unwrap(), theirs());
        assert!(!dir.join(ORIG_DLL_NAME).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_backup_cut_short_is_kept_again() {
        let dir = temp_dir("install-cut");
        std::fs::write(dir.join(DLL_NAME), theirs()).unwrap();
        std::fs::write(dir.join(ORIG_DLL_NAME), b"MZ").unwrap();
        install(&dir, &ours("v1")).unwrap();
        assert_eq!(std::fs::read(dir.join(ORIG_DLL_NAME)).unwrap(), theirs());
        assert!(!dir.join(format!("{ORIG_DLL_NAME}.tmp")).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_when_the_original_is_gone_or_missing() {
        let dir = temp_dir("install-lost");
        assert!(matches!(install(&dir, &ours("v1")), Err(InstallError::NoGameDll)));
        assert_eq!(client_state(&dir, &ours("v1")), ClientState::NoGameDll);
        // Someone else's 5th Echelon DLL and no backup: the game's DLL is lost.
        std::fs::write(dir.join(DLL_NAME), ours("upstream")).unwrap();
        assert!(matches!(install(&dir, &ours("v1")), Err(InstallError::OriginalLost)));
        assert_eq!(std::fs::read(dir.join(DLL_NAME)).unwrap(), ours("upstream"), "left alone");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
