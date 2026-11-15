//! Starting the launcher with the system (0.5.2's S: Glacio1's request on the roadmap):
//! minimised, or in the tray (Windows) with no window until its icon is clicked. Off by default;
//! Settings › Display turns it on and picks which. The system's own setting is the truth (the
//! player can remove it in Windows' Startup apps), so nothing is kept in the prefs: how it starts
//! is the argument written there.
//!
//! - Windows: the current user's `Run` key, `"<launcher.exe>" --minimised` (or `--tray`).
//! - Linux: an XDG autostart entry, `~/.config/autostart/5th-echelon-launcher.desktop`.

/// The argument the launcher starts with from the system: its window starts minimised.
pub const MINIMISED: &str = "--minimised";
/// Or: no window, its icon in the tray (Windows), until the icon is clicked.
pub const TRAY: &str = "--tray";

/// Whether this system can start the launcher in the tray (Windows).
pub const TRAY_HERE: bool = cfg!(target_os = "windows");

/// The Run key's value name, and the autostart entry's.
#[cfg(any(target_os = "windows", test))]
const NAME: &str = "5th Echelon Launcher";

/// How the launcher starts with the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// Its window minimised, in the taskbar.
    Minimised,
    /// No window: its icon in the tray (Windows; elsewhere, minimised).
    Tray,
}

impl Start {
    fn arg(self) -> &'static str {
        match self {
            Self::Minimised => MINIMISED,
            Self::Tray => TRAY,
        }
    }

    /// From what the system runs (the Run key's command, or the entry's Exec line); one written
    /// before 0.5.2's choice is minimised.
    fn of(command: &str) -> Self {
        if command.split_whitespace().any(|a| a == TRAY) {
            Self::Tray
        } else {
            Self::Minimised
        }
    }
}

/// What the system runs: this launcher, minimised or in the tray.
pub fn command(exe: &std::path::Path, how: Start) -> String {
    format!("\"{}\" {}", exe.display(), how.arg())
}

/// Whether the launcher starts with the system now.
pub fn is_on() -> bool {
    imp::command().is_some()
}

/// How it starts with the system, when it does.
pub fn how() -> Option<Start> {
    imp::command().map(|c| Start::of(&c))
}

/// Starts the launcher with the system (minimised or in the tray), or not.
pub fn set(on: bool, how: Start) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("where the launcher is: {e}"))?;
    imp::set(on, &exe, how)
}

/// Whether this run was started by the system minimised ([`MINIMISED`], or [`TRAY`] where
/// there's no tray).
pub fn started_minimised() -> bool {
    std::env::args().any(|a| a == MINIMISED) || (!TRAY_HERE && std::env::args().any(|a| a == TRAY))
}

/// Whether this run was started by the system in the tray ([`TRAY`], on Windows).
pub fn started_in_tray() -> bool {
    TRAY_HERE && std::env::args().any(|a| a == TRAY)
}

#[cfg(target_os = "windows")]
mod imp {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// The Run key's command for the launcher, if it's there.
    pub fn command() -> Option<String> {
        let (key, value) = (wide(RUN), wide(super::NAME));
        let read = |data: Option<*mut core::ffi::c_void>, size: &mut u32| unsafe {
            Registry::RegGetValueW(
                Registry::HKEY_CURRENT_USER,
                PCWSTR::from_raw(key.as_ptr()),
                PCWSTR::from_raw(value.as_ptr()),
                Registry::RRF_RT_REG_SZ,
                None,
                data,
                Some(size),
            )
        };
        let mut size = 0u32;
        read(None, &mut size).ok().ok()?;
        let mut buf = vec![0u16; (size as usize).div_ceil(2).max(1)];
        read(Some(buf.as_mut_ptr().cast()), &mut size).ok().ok()?;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    pub fn set(on: bool, exe: &std::path::Path, how: super::Start) -> Result<(), String> {
        let (key, value) = (wide(RUN), wide(super::NAME));
        let err = if on {
            let data = wide(&super::command(exe, how));
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

    /// The autostart entry's Exec line, if the entry is there.
    pub fn command() -> Option<String> {
        command_at(&entry()?)
    }

    pub(super) fn command_at(path: &std::path::Path) -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;
        Some(text.lines().find_map(|l| l.strip_prefix("Exec=")).unwrap_or_default().to_string())
    }

    pub fn set(on: bool, exe: &std::path::Path, how: super::Start) -> Result<(), String> {
        set_at(&entry().ok_or("no home folder")?, on, exe, how)
    }

    pub(super) fn set_at(path: &std::path::Path, on: bool, exe: &std::path::Path, how: super::Start) -> Result<(), String> {
        let path = path.to_path_buf();
        if on {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, super::desktop_entry(exe, how)).map_err(|e| e.to_string())
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
fn desktop_entry(exe: &std::path::Path, how: Start) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=5th Echelon Launcher\nComment=Splinter Cell: Blacklist online (5th Echelon)\nExec={}\nX-GNOME-Autostart-enabled=true\n",
        command(exe, how)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_starts_this_launcher_minimised_or_in_the_tray() {
        let exe = std::path::Path::new("/opt/5ee/launcher");
        assert_eq!(command(exe, Start::Minimised), "\"/opt/5ee/launcher\" --minimised");
        assert_eq!(command(exe, Start::Tray), "\"/opt/5ee/launcher\" --tray");
        let entry = desktop_entry(exe, Start::Minimised);
        assert!(entry.contains("Exec=\"/opt/5ee/launcher\" --minimised") && entry.starts_with("[Desktop Entry]"));
        assert_eq!(NAME, "5th Echelon Launcher");
        // Read back from what the system runs.
        assert_eq!(Start::of(&command(exe, Start::Tray)), Start::Tray);
        assert_eq!(Start::of(&command(exe, Start::Minimised)), Start::Minimised);
        assert_eq!(Start::of("\"C:\\Games\\launcher.exe\" --minimised"), Start::Minimised, "written before the choice");
        assert_eq!(Start::of("\"C:\\a--tray\\launcher.exe\" --minimised"), Start::Minimised, "only an argument of its own");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn the_autostart_entry_comes_and_goes() {
        let dir = std::env::temp_dir().join(format!("fe-autostart-{}", std::process::id()));
        let path = dir.join("autostart").join("5th-echelon-launcher.desktop");
        let exe = std::path::Path::new("/opt/5ee/launcher");
        imp::set_at(&path, true, exe, Start::Minimised).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("--minimised"));
        imp::set_at(&path, true, exe, Start::Tray).unwrap();
        assert_eq!(imp::command_at(&path).map(|c| Start::of(&c)), Some(Start::Tray));
        imp::set_at(&path, false, exe, Start::Minimised).unwrap();
        assert!(!path.exists());
        assert_eq!(imp::command_at(&path), None);
        imp::set_at(&path, false, exe, Start::Minimised).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
