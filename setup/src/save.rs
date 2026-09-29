//! Save games. 5th Echelon keeps its own save instead of Ubisoft's cloud
//! one. Co-op and Spies vs Mercs are locked below rank 5, and a new profile
//! starts at rank 1, so a new player gets a rank 5 save.
//!
//! The file: a header (a new save's is `01 00 00 00 00 00`), the length of
//! what follows as a u32 LE, `masW`, then the profile as XML.

use std::path::Path;
use std::path::PathBuf;

use hooks_config::SaveDir;

/// Rank 5's experience (upstream's generated save).
pub const RANK5_XP: u32 = 6600;

/// Upstream's rank 5 profile.
const BASE_SAVE: &[u8] = include_bytes!("../data/base_savegame.xml");
const NEW_HEADER: [u8; 6] = [1, 0, 0, 0, 0, 0];
const MAGIC: &[u8] = b"masW";
const XP_OPEN: &str = "<m_iXP>";
const XP_CLOSE: &str = "</m_iXP>";

/// The save file (slot 1) the hook uses, per the `[Save]` setting.
pub fn save_path(save: &hooks_config::Save, game_dir: &Path) -> Option<PathBuf> {
    let dir = match &save.save_dir {
        SaveDir::Roaming => crate::sys::game_roaming_dir(game_dir)?.join("5th-Echelon").join("Saves"),
        SaveDir::InstallLocation => game_dir.join("5th-Echelon-Saves"),
        SaveDir::Custom(dir) => PathBuf::from(dir),
    };
    Some(dir.join(format!("{:08}.sav", 1)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveState {
    Missing,
    /// Not in the layout we know: left alone.
    Unreadable,
    Ok { xp: u32 },
}

impl SaveState {
    pub fn below_rank5(&self) -> bool {
        matches!(self, SaveState::Ok { xp } if *xp < RANK5_XP)
    }
}

pub fn check(path: &Path) -> SaveState {
    match std::fs::read(path) {
        Err(_) => SaveState::Missing,
        Ok(data) => match split(&data).and_then(|(_, xml)| xp(xml)) {
            Some(xp) => SaveState::Ok { xp },
            None => SaveState::Unreadable,
        },
    }
}

/// Header and XML of a save, if it's in the known layout.
fn split(data: &[u8]) -> Option<(&[u8], &[u8])> {
    if data.first() != Some(&1) {
        return None;
    }
    let at = data.windows(MAGIC.len()).take(64).position(|w| w == MAGIC)?;
    let length = u32::from_le_bytes(data.get(at.checked_sub(4)?..at)?.try_into().ok()?) as usize;
    (length == data.len() - at).then(|| (&data[..at - 4], &data[at + MAGIC.len()..]))
}

fn xp(xml: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(xml).ok()?;
    let start = text.find(XP_OPEN)? + XP_OPEN.len();
    let end = start + text[start..].find(XP_CLOSE)?;
    text[start..end].trim().parse().ok()
}

fn assemble(header: &[u8], xml: &[u8]) -> Vec<u8> {
    let mut out = header.to_vec();
    out.extend(((xml.len() + MAGIC.len()) as u32).to_le_bytes());
    out.extend(MAGIC);
    out.extend(xml);
    out
}

/// Writes the save and the `.meta` file next to it the game expects.
fn write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    crate::write_atomic(path, data)?;
    let meta = path.with_extension("meta");
    if !meta.exists() {
        std::fs::write(meta, b"sc6_save.sav")?;
    }
    Ok(())
}

/// Copies the save aside as `<name>.<timestamp>.bak`.
pub fn backup(path: &Path) -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{stamp}.bak"));
    let backup = PathBuf::from(name);
    std::fs::copy(path, &backup)?;
    Ok(backup)
}

/// A new rank 5 save at `path`, backing up any save already there.
pub fn create_rank5(path: &Path) -> std::io::Result<Option<PathBuf>> {
    let backup = if path.exists() { Some(backup(path)?) } else { None };
    write(path, &assemble(&NEW_HEADER, BASE_SAVE))?;
    Ok(backup)
}

/// Raises the save's experience to rank 5 if it's lower, keeping everything
/// else (backed up first). Returns the backup if it changed anything.
pub fn raise_to_rank5(path: &Path) -> std::io::Result<Option<PathBuf>> {
    let data = std::fs::read(path)?;
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not a save in the known layout");
    let (header, xml) = split(&data).ok_or_else(bad)?;
    let text = std::str::from_utf8(xml).map_err(|_| bad())?;
    if xp(xml).ok_or_else(bad)? >= RANK5_XP {
        return Ok(None);
    }
    let start = text.find(XP_OPEN).ok_or_else(bad)? + XP_OPEN.len();
    let end = start + text[start..].find(XP_CLOSE).ok_or_else(bad)?;
    let raised = format!("{}{RANK5_XP}{}", &text[..start], &text[end..]);
    let backup = backup(path)?;
    write(path, &assemble(header, raised.as_bytes()))?;
    Ok(Some(backup))
}

/// The save Ubisoft Connect keeps for Blacklist (game id 449), if any.
pub fn find_ubisoft_save() -> Option<PathBuf> {
    let saves = crate::sys::ubisoft_launcher_dir()?.join("savegames");
    std::fs::read_dir(saves).ok()?.flatten().map(|user| user.path().join("449").join("1.save")).find(|p| p.is_file())
}

/// Imports a Ubisoft Connect save: its own metadata comes first (a u32 LE
/// length), then the same layout as ours.
pub fn import_ubisoft(from: &Path, to: &Path) -> std::io::Result<Option<PathBuf>> {
    let data = std::fs::read(from)?;
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not a Blacklist save");
    let meta = u32::from_le_bytes(data.get(..4).ok_or_else(bad)?.try_into().unwrap()) as usize;
    let payload = data.get(4 + meta..).ok_or_else(bad)?;
    split(payload).ok_or_else(bad)?;
    let backup = if to.exists() { Some(backup(to)?) } else { None };
    write(to, payload)?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn creates_checks_and_raises() {
        let dir = temp_dir("save");
        let path = dir.join("Saves").join("00000001.sav");
        assert_eq!(check(&path), SaveState::Missing);
        assert_eq!(create_rank5(&path).unwrap(), None);
        assert_eq!(check(&path), SaveState::Ok { xp: RANK5_XP });
        assert_eq!(std::fs::read(path.with_extension("meta")).unwrap(), b"sc6_save.sav");

        // A rank 1 save (as the game makes) is raised, keeping its header.
        let base = String::from_utf8(BASE_SAVE.to_vec()).unwrap();
        assert!(base.contains("<m_iXP> 6600</m_iXP>"));
        let rank1 = base.replacen("<m_iXP> 6600</m_iXP>", "<m_iXP>0</m_iXP>", 1);
        std::fs::write(&path, assemble(&[1, 9, 9, 9, 9, 9], rank1.as_bytes())).unwrap();
        assert!(check(&path).below_rank5());
        let backup = raise_to_rank5(&path).unwrap().expect("changed");
        assert_eq!(check(&path), SaveState::Ok { xp: RANK5_XP });
        assert_eq!(&std::fs::read(&path).unwrap()[..6], &[1, 9, 9, 9, 9, 9]);
        assert_eq!(check(&backup), SaveState::Ok { xp: 0 });
        assert_eq!(raise_to_rank5(&path).unwrap(), None, "already rank 5");

        std::fs::write(&path, b"garbage").unwrap();
        assert_eq!(check(&path), SaveState::Unreadable);
        assert!(raise_to_rank5(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn imports_ubisoft_saves() {
        let dir = temp_dir("save-import");
        let ours = assemble(&NEW_HEADER, BASE_SAVE);
        let mut ubisoft = 3u32.to_le_bytes().to_vec();
        ubisoft.extend(b"abc");
        ubisoft.extend(&ours);
        std::fs::write(dir.join("1.save"), &ubisoft).unwrap();
        let to = dir.join("out").join("00000001.sav");
        import_ubisoft(&dir.join("1.save"), &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), ours);
        std::fs::write(dir.join("bad.save"), b"\x01\x00\x00\x00x").unwrap();
        assert!(import_ubisoft(&dir.join("bad.save"), &to).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
