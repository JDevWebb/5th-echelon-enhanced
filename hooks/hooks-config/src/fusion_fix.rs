//! ThirteenAG's Fusion Fix, when it's installed beside the game.
//!
//! Its `DisableNegativeMouseAcceleration` removes the game's mouse acceleration its own way: it
//! NOPs the four `movaps` that apply the look-delta clamp, so the clamp's bounds are never used.
//! With it on, our wider bounds (`DisableMouseAccel`, the hooks' `mouse_fix`) change nothing,
//! and the camera has no limit on an input spike. The launcher and the hooks both ask here.
//!
//! An ASI loader loads `SplinterCellBlacklist.FusionFix.asi` from the game's folder or its
//! `scripts\` or `plugins\` folder, and Fusion Fix reads the `.ini` of the same name beside it.
//! A missing key counts as on (Fusion Fix's own default).

use std::path::Path;
use std::path::PathBuf;

const NAME: &str = "SplinterCellBlacklist.FusionFix";
const SECTION: &str = "MAIN";
const KEY: &str = "DisableNegativeMouseAcceleration";

/// The installed Fusion Fix's `.asi`, if there is one in the folders an ASI loader looks in.
pub fn installed(game_dir: &Path) -> Option<PathBuf> {
    [game_dir.to_path_buf(), game_dir.join("scripts"), game_dir.join("plugins")]
        .into_iter()
        .map(|dir| dir.join(format!("{NAME}.asi")))
        .find(|asi| asi.is_file())
}

/// Whether an installed Fusion Fix removes the mouse acceleration itself (its
/// `DisableNegativeMouseAcceleration` isn't 0). No `.ini` beside the `.asi` is Fusion Fix's
/// default: on.
pub fn removes_mouse_accel(game_dir: &Path) -> bool {
    installed(game_dir).is_some_and(|asi| std::fs::read_to_string(asi.with_extension("ini")).map_or(true, |ini| ini_removes_mouse_accel(&ini)))
}

/// The `.ini`'s `[MAIN] DisableNegativeMouseAcceleration`: on unless it reads as 0. Fusion Fix
/// reads the number at the start of the value, so a comment after it (`= 0 // ...`) is fine.
fn ini_removes_mouse_accel(ini: &str) -> bool {
    let mut in_section = false;
    for line in ini.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.split_once(']')).map(|(name, _)| name) {
            in_section = name.trim().eq_ignore_ascii_case(SECTION);
        } else if let Some((key, value)) = line.split_once('=').filter(|_| in_section) {
            if key.trim().eq_ignore_ascii_case(KEY) {
                let value = value.trim_start();
                let end = value.find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '+')).unwrap_or(value.len());
                return value[..end].parse::<i64>().map_or(true, |n| n != 0);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ini_says_whether_it_removes_the_acceleration() {
        assert!(ini_removes_mouse_accel("[MAIN]\r\nSkipIntro = 1\r\nDisableNegativeMouseAcceleration = 1\r\n"));
        assert!(!ini_removes_mouse_accel("[MAIN]\nDisableNegativeMouseAcceleration = 0   // off\n"));
        assert!(!ini_removes_mouse_accel("[main]\ndisablenegativemouseacceleration=0\n"));
        // Missing, or only in another section: Fusion Fix's default, on.
        assert!(ini_removes_mouse_accel("[MAIN]\nSkipIntro = 1\n"));
        assert!(ini_removes_mouse_accel("[OTHER]\nDisableNegativeMouseAcceleration = 0\n"));
        // Not a number: Fusion Fix falls back to its default.
        assert!(ini_removes_mouse_accel("[MAIN]\nDisableNegativeMouseAcceleration = off\n"));
    }

    #[test]
    fn found_where_an_asi_loader_looks_and_read_beside_it() {
        let dir = std::env::temp_dir().join(format!("fusion-fix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        assert!(!removes_mouse_accel(&dir), "not installed");
        // A stray .ini alone isn't Fusion Fix.
        std::fs::write(dir.join(format!("{NAME}.ini")), "[MAIN]\nDisableNegativeMouseAcceleration = 1\n").unwrap();
        assert!(installed(&dir).is_none());
        // The .asi without an .ini: its default, on.
        std::fs::write(dir.join("scripts").join(format!("{NAME}.asi")), b"").unwrap();
        assert!(removes_mouse_accel(&dir));
        // Switched off in the .ini beside it: ours can do it.
        std::fs::write(dir.join("scripts").join(format!("{NAME}.ini")), "[MAIN]\nDisableNegativeMouseAcceleration = 0\n").unwrap();
        assert!(!removes_mouse_accel(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
