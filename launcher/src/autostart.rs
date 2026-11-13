//! Starting the launcher with the system, minimised (0.5.2's S: Glacio1's request on the
//! roadmap). Off by default; Settings › Display turns it on. The system's own setting is the
//! truth (the player can remove it in Windows' Startup apps), so nothing is kept in the prefs.
//!
//! - Windows: the current user's `Run` key, `"<launcher.exe>" --minimised`.
//! - Linux: an XDG autostart entry, `~/.config/autostart/5th-echelon-launcher.desktop`.

/// The argument the launcher starts with from the system: its window starts minimised.
pub const MINIMISED: &str = "--minimised";

/// The Run key's value name, and the autostart entry's.
#[cfg(any(target_os = "windows", test))]
const NAME: &str = "5th Echelon Launcher";

/// What the system runs: this launcher, minimised.
pub fn command(exe: &std::path::Path) -> String {
    format!("\"{}\" {MINIMISED}", exe.display())
}

/// Whether the launcher starts with the system now.
pub fn is_on() -> bool {
    imp::is_on()
}

/// Starts the launcher with the system (minimised), or not.
pub fn set(on: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("where the launcher is: {e}"))?;
    imp::set(on, &exe)
}

/// Whether this run was started by the system ([`MINIMISED`]).
pub fn started_minimised() -> bool {
    std::env::args().any(|a| a == MINIMISED)
}

#[cfg(target_os = "windows")]
mod imp {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn is_on() -> bool {
        let (key, value) = (wide(RUN), wide(super::NAME));
        let mut size = 0u32;
        let err = unsafe {
            Registry::RegGetValueW(
                Registry::HKEY_CURRENT_USER,
                PCWSTR::from_raw(key.as_ptr()),
                PCWSTR::from_raw(value.as_ptr()),
                Registry::RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        err.is_ok()
    }

    pub fn set(on: bool, exe: &std::path::Path) -> Result<(), String> {
        let (key, value) = (wide(RUN), wide(super::NAME));
        let err = if on {
            let data = wide(&super::command(exe));
            unsafe {
                Registry::RegSetKeyValueW(
                    Registry::HKEY_CURRENT_USER,
                    PCWSTR::from_raw(key.as_ptr()),
                    PCWSTR::from_raw(value.as_ptr()),
                    Registry::REG_SZ.0,
                    Some(data.as_ptr().cast()),
                    u32::try_from(data.len() * 2).unwrap_or(u32::MAX),
                )
            }
        } else {
            let err = unsafe { Registry::RegDeleteKeyValueW(Registry::HKEY_CURRENT_USER, PCWSTR::from_raw(key.as_ptr()), PCWSTR::from_raw(value.as_ptr())) };
            // Already gone is fine.
            if err == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            err
        };
        err.ok().map_err(|e| format!("Windows refused it: {e}"))
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::path::PathBuf;

    fn entry() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(config.join("autostart").join("5th-echelon-launcher.desktop"))
    }

    pub fn is_on() -> bool {
        entry().is_some_and(|p| p.exists())
    }

    pub fn set(on: bool, exe: &std::path::Path) -> Result<(), String> {
        set_at(&entry().ok_or("no home folder")?, on, exe)
    }

    pub(super) fn set_at(path: &std::path::Path, on: bool, exe: &std::path::Path) -> Result<(), String> {
        let path = path.to_path_buf();
        if on {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, super::desktop_entry(exe)).map_err(|e| e.to_string())
        } else {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        }
    }
}

/// The XDG autostart entry for `exe`.
#[cfg(any(not(target_os = "windows"), test))]
fn desktop_entry(exe: &std::path::Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=5th Echelon Launcher\nComment=Splinter Cell: Blacklist online (5th Echelon)\nExec={}\nX-GNOME-Autostart-enabled=true\n",
        command(exe)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_starts_this_launcher_minimised() {
        let exe = std::path::Path::new("/opt/5ee/launcher");
        assert_eq!(command(exe), "\"/opt/5ee/launcher\" --minimised");
        let entry = desktop_entry(exe);
        assert!(entry.contains("Exec=\"/opt/5ee/launcher\" --minimised") && entry.starts_with("[Desktop Entry]"));
        assert_eq!(NAME, "5th Echelon Launcher");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn the_autostart_entry_comes_and_goes() {
        let dir = std::env::temp_dir().join(format!("fe-autostart-{}", std::process::id()));
        let path = dir.join("autostart").join("5th-echelon-launcher.desktop");
        let exe = std::path::Path::new("/opt/5ee/launcher");
        imp::set_at(&path, true, exe).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("--minimised"));
        imp::set_at(&path, false, exe).unwrap();
        assert!(!path.exists());
        imp::set_at(&path, false, exe).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
