//! The game's picture brightness. `Blacklist.ini`'s `m_fVideoGamma` (the in-game brightness
//! slider's value) is clamped to `[m_fMinVideoGamma, m_fMaxVideoGamma]`, and the shipped cap
//! (0.6) leaves the picture dark under Wine/Proton. The launcher's "Brighten the picture"
//! option lifts that cap and sets a brighter value by editing `Blacklist.ini` — statically,
//! only while the game isn't running (it reads the file at start and never writes it back) —
//! and puts the originals back when the option is turned off.

use std::io;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

const SECTION: &str = "GENERAL";
const VIDEO: &str = "m_fVideoGamma";
const MAX: &str = "m_fMaxVideoGamma";
/// The cap is lifted to the game's own no-key default (1.0) so the slider's whole range applies.
const OPEN_CAP: f32 = 1.0;

/// The widest and tightest the brightness slider goes, in hundredths of `m_fVideoGamma`.
pub const MIN_BRIGHTNESS: u8 = 60;
pub const MAX_BRIGHTNESS: u8 = 100;
/// The default brightness when the option is first turned on (the value confirmed by testing).
pub const DEFAULT_BRIGHTNESS: u8 = 75;

/// The values the two keys had before the option changed them, to put back when it's turned
/// off. `None` means the key wasn't in the file, so restoring removes it again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Backup {
    pub video: Option<String>,
    pub max: Option<String>,
}

/// `Blacklist.ini` in the game's folder.
pub fn blacklist_ini(game_dir: &Path) -> PathBuf {
    game_dir.join("Blacklist.ini")
}

/// Turns the brightness up: sets `m_fVideoGamma` to `brightness` (hundredths) and lifts the
/// cap, saving the originals into `backup` the first time. Call with the game stopped.
pub fn enable(game_dir: &Path, brightness: u8, backup: &mut Option<Backup>) -> io::Result<()> {
    let path = blacklist_ini(game_dir);
    let text = std::fs::read_to_string(&path)?;
    if backup.is_none() {
        *backup = Some(Backup {
            video: read_key(&text, SECTION, VIDEO),
            max: read_key(&text, SECTION, MAX),
        });
    }
    let value = f32::from(brightness.clamp(MIN_BRIGHTNESS, MAX_BRIGHTNESS)) / 100.0;
    let out = set_key(&text, SECTION, VIDEO, &fmt(value));
    let out = set_key(&out, SECTION, MAX, &fmt(OPEN_CAP));
    std::fs::write(&path, out)
}

/// Puts the original values back and clears the backup. Call with the game stopped. Does
/// nothing if the option was never on (no backup).
pub fn disable(game_dir: &Path, backup: &mut Option<Backup>) -> io::Result<()> {
    let Some(b) = backup.take() else { return Ok(()) };
    let path = blacklist_ini(game_dir);
    let text = std::fs::read_to_string(&path)?;
    let out = restore_key(&text, SECTION, VIDEO, b.video.as_deref());
    let out = restore_key(&out, SECTION, MAX, b.max.as_deref());
    std::fs::write(&path, out)
}

/// `0.75`, `1.00` — a plain decimal the game parses, matching the file's own style.
fn fmt(v: f32) -> String {
    format!("{v:.2}")
}

/// The value of `key` under `[section]`, if the key is there.
fn read_key(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r');
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            in_section = t[1..t.len() - 1].eq_ignore_ascii_case(section);
        } else if in_section {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// Sets `key=value` under `[section]`: replaces the line if it's there, else inserts it at the
/// end of the section (adding the section if the file has none). The file's line endings are
/// kept.
fn set_key(text: &str, section: &str, key: &str, value: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let new_line = format!("{key}={value}");
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut seen_section = false;
    let mut done = false;
    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r');
        let t = line.trim();
        let header = t.starts_with('[') && t.ends_with(']');
        if header {
            // Leaving the target section without having written: put the key at its end.
            if in_section && !done {
                out.push(new_line.clone());
                done = true;
            }
            in_section = t[1..t.len() - 1].eq_ignore_ascii_case(section);
            seen_section |= in_section;
        } else if in_section && !done {
            if let Some((k, _)) = t.split_once('=') {
                if k.trim() == key {
                    out.push(new_line.clone());
                    done = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    if in_section && !done {
        out.push(new_line.clone());
        done = true;
    }
    if !seen_section {
        out.push(format!("[{section}]"));
        out.push(new_line);
    } else if !done {
        // Section existed but the loop ended mid-file oddly; append to be safe.
        out.push(new_line);
    }
    out.join(nl)
}

/// Removes `key` from `[section]`, if it's there.
fn remove_key(text: &str, section: &str, key: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r');
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            in_section = t[1..t.len() - 1].eq_ignore_ascii_case(section);
        } else if in_section {
            if let Some((k, _)) = t.split_once('=') {
                if k.trim() == key {
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    out.join(nl)
}

/// Puts `key` back to `original`, or removes it if it wasn't there before.
fn restore_key(text: &str, section: &str, key: &str, original: Option<&str>) -> String {
    match original {
        Some(v) => set_key(text, section, key, v),
        None => remove_key(text, section, key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INI: &str = "[GENERAL]\r\nm_fVideoGamma=0.30\r\nm_fMinVideoGamma=-0.2\r\nm_fMaxVideoGamma=0.6\r\n[OTHER]\r\nx=1\r\n";

    #[test]
    fn reads_and_sets_keys_in_the_right_section() {
        assert_eq!(read_key(INI, "GENERAL", "m_fMaxVideoGamma").as_deref(), Some("0.6"));
        assert_eq!(read_key(INI, "OTHER", "x").as_deref(), Some("1"));
        let out = set_key(INI, "GENERAL", "m_fVideoGamma", "0.75");
        assert!(out.contains("m_fVideoGamma=0.75"));
        assert!(out.contains("\r\n"), "CRLF kept");
        assert!(out.contains("[OTHER]\r\nx=1"), "other section untouched");
    }

    #[test]
    fn inserts_a_missing_key_into_its_section() {
        let bare = "[GENERAL]\nm_fMinVideoGamma=-0.2\n[OTHER]\ny=2\n";
        let out = set_key(bare, "GENERAL", "m_fMaxVideoGamma", "1.00");
        // Added under [GENERAL], before [OTHER].
        let gen = out.find("[GENERAL]").unwrap();
        let oth = out.find("[OTHER]").unwrap();
        let at = out.find("m_fMaxVideoGamma=1.00").unwrap();
        assert!(gen < at && at < oth, "{out}");
    }

    #[test]
    fn enable_then_disable_round_trips() {
        let dir = crate::testutil::temp_dir("gamma");
        let path = blacklist_ini(&dir);
        std::fs::write(&path, INI).unwrap();
        let mut backup = None;
        enable(&dir, 80, &mut backup).unwrap();
        let on = std::fs::read_to_string(&path).unwrap();
        assert!(on.contains("m_fVideoGamma=0.80"));
        assert!(on.contains("m_fMaxVideoGamma=1.00"));
        assert_eq!(backup.as_ref().unwrap().max.as_deref(), Some("0.6"));
        disable(&dir, &mut backup).unwrap();
        let off = std::fs::read_to_string(&path).unwrap();
        assert!(off.contains("m_fVideoGamma=0.30"), "original restored: {off}");
        assert!(off.contains("m_fMaxVideoGamma=0.6"));
        assert!(backup.is_none());
    }

    #[test]
    fn disable_removes_a_key_that_wasnt_there() {
        let dir = crate::testutil::temp_dir("gamma-absent");
        let path = blacklist_ini(&dir);
        std::fs::write(&path, "[GENERAL]\r\nm_fVideoGamma=0.30\r\n").unwrap();
        let mut backup = None;
        enable(&dir, 75, &mut backup).unwrap();
        assert_eq!(backup.as_ref().unwrap().max, None, "cap wasn't in the file");
        disable(&dir, &mut backup).unwrap();
        let off = std::fs::read_to_string(&path).unwrap();
        assert!(!off.contains("m_fMaxVideoGamma"), "cap key removed again: {off}");
        assert!(off.contains("m_fVideoGamma=0.30"));
    }
}
