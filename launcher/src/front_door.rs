//! Linux (and the Steam Deck): this egui launcher is the program started, and it opens the
//! launcher's Tauri interface (launcher-tauri, beside it) when it can. That one links
//! WebKitGTK 4.1, so without it it can't even start, let alone fall back: it's tried here first
//! (`--probe`: it starts and exits at once, so every library it needs is there), then run, and
//! this one waits. Not in gamescope (gaming mode), where the egui launcher runs full screen, nor
//! when the player chose the classic interface. If the Tauri one fails soon after it starts, or
//! the player switches to the classic interface from it, this one opens its own window.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

/// The exit code the Tauri interface gives when the player switched to the classic one.
pub const SWITCH_TO_CLASSIC: i32 = 75;
/// A Tauri interface failing within this of its start is taken for one that can't run here.
const FAILS_EARLY: Duration = Duration::from_secs(10);
/// How long the probe may take.
#[cfg(not(test))]
const PROBE_FOR: Duration = Duration::from_secs(10);
#[cfg(test)]
const PROBE_FOR: Duration = Duration::from_millis(500);

/// The Tauri interface beside this launcher.
fn tauri_exe() -> Option<PathBuf> {
    crate::updater::tauri_beside(&std::env::current_exe().ok()?).filter(|p| p.is_file())
}

/// Whether `exe` starts here: run with `--probe`, it exits 0 at once.
fn starts(exe: &std::path::Path) -> bool {
    let Ok(mut child) = Command::new(exe)
        .arg("--probe")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let until = Instant::now() + PROBE_FOR;
    while Instant::now() < until {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return false,
        }
    }
    let _ = child.kill();
    false
}

/// Opens the Tauri interface when it can, and waits for it: the code to exit with once it
/// closes, or None to open this launcher's own window (it can't run here, it failed early, or
/// the player switched to the classic interface).
pub fn try_tauri(gamescope: bool) -> Option<i32> {
    if !cfg!(target_os = "linux") || gamescope || crate::prefs::Prefs::classic_interface() || std::env::var_os("FE_CLASSIC").is_some() {
        return None;
    }
    let exe = tauri_exe()?;
    if !starts(&exe) {
        eprintln!("The Tauri interface can't start here (no WebKitGTK 4.1?): the classic one");
        return None;
    }
    let started = Instant::now();
    let status = Command::new(&exe).args(std::env::args_os().skip(1)).status().ok()?;
    match status.code() {
        Some(SWITCH_TO_CLASSIC) => None,
        Some(0) => Some(0),
        code if started.elapsed() < FAILS_EARLY => {
            eprintln!("The Tauri interface stopped soon after starting ({code:?}): the classic one");
            None
        }
        code => Some(code.unwrap_or(1)),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A stand-in for the Tauri interface: a script doing `body` when probed.
    fn script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn only_an_interface_that_starts_and_exits_cleanly_counts() {
        let dir = std::env::temp_dir().join(format!("fes-front-door-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(starts(&script(&dir, "ok", "[ \"$1\" = --probe ] && exit 0; exit 3")), "starts and exits 0");
        assert!(!starts(&script(&dir, "missing-library", "exit 127")), "can't load a library");
        assert!(!starts(&script(&dir, "hangs", "sleep 5")), "doesn't exit in time");
        assert!(!starts(&dir.join("not-there")), "not there");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
