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
    Ok {
        xp: u32,
    },
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

/// Whether `data` is a save the game wrote: [`split`]'s XML profile (upstream's generated
/// save), or the game's own binary one, which has the same frame (a 6-byte header starting
/// with 1, then the length of the rest) without the XML. Only the XML one can be read and
/// raised to rank 5; the binary one is kept as it is.
fn is_save(data: &[u8]) -> bool {
    split(data).is_some()
        || (data.first() == Some(&1)
            && data
                .get(6..10)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes)
                .is_some_and(|n| n as usize == data.len() - 10 && n > 0))
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

/// Ubisoft Connect's folders that may hold `savegames`: the one the registry
/// names, its default place, any folder above the game that has `savegames`
/// (a game installed through Ubisoft Connect), and Ubisoft Connect inside the
/// game's Wine or Proton prefix.
fn ubisoft_roots(game_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = crate::sys::ubisoft_launcher_dir().into_iter().collect();
    if cfg!(windows) {
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher"));
    }
    if let Some(game_dir) = game_dir {
        roots.extend(game_dir.ancestors().skip(1).filter(|d| d.join("savegames").is_dir()).map(Path::to_path_buf));
        if let Some(prefix) = crate::wine::prefix_for(game_dir) {
            roots.push(prefix.root.join("drive_c").join("Program Files (x86)").join("Ubisoft").join("Ubisoft Game Launcher"));
        }
    }
    let mut seen = Vec::new();
    roots.retain(|r| {
        let new = !seen.contains(r);
        seen.push(r.clone());
        new
    });
    roots
}

/// Blacklist saves in these Ubisoft Connect folders, newest first: slot 1
/// (`1.save`) of any account and any game id (the game runs as 449 or 91,
/// depending on the edition), only the files that are Blacklist saves.
pub fn ubisoft_saves_in(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = roots
        .iter()
        .filter_map(|root| std::fs::read_dir(root.join("savegames")).ok())
        .flat_map(|accounts| accounts.flatten())
        .filter_map(|account| std::fs::read_dir(account.path()).ok())
        .flat_map(|games| games.flatten())
        .map(|game| game.path().join("1.save"))
        .filter(|p| std::fs::read(p).is_ok_and(|data| ubisoft_payload(&data).is_some()))
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.dedup_by(|a, b| a.1 == b.1);
    found.into_iter().map(|(_, p)| p).collect()
}

/// The newest Blacklist save Ubisoft Connect keeps on this PC (or in the
/// game's prefix), if any.
pub fn find_ubisoft_save(game_dir: Option<&Path>) -> Option<PathBuf> {
    ubisoft_saves_in(&ubisoft_roots(game_dir)).into_iter().next()
}

/// The save inside a Ubisoft Connect save file: its own metadata comes first
/// (a u32 LE length), then the same layout as ours.
fn ubisoft_payload(data: &[u8]) -> Option<&[u8]> {
    let meta = u32::from_le_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let payload = data.get(4usize.checked_add(meta)?..)?;
    is_save(payload).then_some(payload)
}

/// Imports a Ubisoft Connect save, backing up the save at `to` first.
pub fn import_ubisoft(from: &Path, to: &Path) -> std::io::Result<Option<PathBuf>> {
    let data = std::fs::read(from)?;
    let payload = ubisoft_payload(&data).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "not a Blacklist save"))?;
    let backup = if to.exists() { Some(backup(to)?) } else { None };
    write(to, payload)?;
    Ok(backup)
}

/// Imports any Blacklist save file: one of ours (a `.sav`, or one of the
/// launcher's `.bak` backups) or Ubisoft Connect's (`1.save`). The save at
/// `to` is backed up first; a file that isn't a Blacklist save changes nothing.
pub fn import_file(from: &Path, to: &Path) -> std::io::Result<Option<PathBuf>> {
    let data = std::fs::read(from)?;
    let save = if is_save(&data) {
        &data[..]
    } else {
        ubisoft_payload(&data).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "that file isn't a Blacklist save"))?
    };
    if std::fs::canonicalize(from).ok() == std::fs::canonicalize(to).ok() && to.exists() {
        return Ok(None);
    }
    let backup = if to.exists() { Some(backup(to)?) } else { None };
    write(to, save)?;
    Ok(backup)
}

/// What [`prepare`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// The save was already there, at rank 5 or above (or in a layout left alone).
    Ready,
    /// Ubisoft Connect's save, imported from this file; `raised` if it was below rank 5.
    Imported { from: PathBuf, raised: bool },
    /// No save anywhere: a new rank 5 one.
    Created,
    /// The save was below rank 5, and is raised (backed up first).
    Raised,
}

impl Prepared {
    /// For the setup log and the Status card.
    pub fn describe(&self) -> Option<&'static str> {
        match self {
            Prepared::Ready => None,
            Prepared::Imported { raised: false, .. } => Some("Imported your Ubisoft Connect save."),
            Prepared::Imported { raised: true, .. } => Some("Imported your Ubisoft Connect save and raised it to rank 5."),
            Prepared::Created => Some("Created a rank 5 save."),
            Prepared::Raised => Some("Raised your save to rank 5 (the old one is backed up)."),
        }
    }
}

/// Gets the save at `path` ready to play: with none there, the player's own
/// from Ubisoft Connect when there is one (rather than a blank profile), else
/// a new rank 5 save; and below rank 5, raised.
pub fn prepare(path: &Path, game_dir: Option<&Path>) -> std::io::Result<Prepared> {
    prepare_from(path, find_ubisoft_save(game_dir))
}

fn prepare_from(path: &Path, ubisoft: Option<PathBuf>) -> std::io::Result<Prepared> {
    match check(path) {
        SaveState::Missing => match ubisoft {
            Some(from) => {
                import_ubisoft(&from, path)?;
                let raised = check(path).below_rank5() && raise_to_rank5(path)?.is_some();
                Ok(Prepared::Imported { from, raised })
            }
            None => create_rank5(path).map(|_| Prepared::Created),
        },
        s if s.below_rank5() => raise_to_rank5(path).map(|_| Prepared::Raised),
        _ => Ok(Prepared::Ready),
    }
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

    fn ubisoft_file(xp: u32) -> Vec<u8> {
        let base = String::from_utf8(BASE_SAVE.to_vec())
            .unwrap()
            .replacen("<m_iXP> 6600</m_iXP>", &format!("<m_iXP>{xp}</m_iXP>"), 1);
        let mut ubisoft = 3u32.to_le_bytes().to_vec();
        ubisoft.extend(b"abc");
        ubisoft.extend(assemble(&NEW_HEADER, base.as_bytes()));
        ubisoft
    }

    #[test]
    fn finds_the_newest_ubisoft_save_under_any_account_and_game_id() {
        let dir = temp_dir("save-find");
        let root = dir.join("Ubisoft Game Launcher");
        let put = |rel: &str, data: &[u8]| {
            let p = root.join("savegames").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, data).unwrap();
            p
        };
        let old = put("account-a/449/1.save", &ubisoft_file(100));
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let newer = put("account-b/91/1.save", &ubisoft_file(200));
        put("account-b/1234/1.save", b"another game's save");
        put("account-a/449/2.save", &ubisoft_file(300));
        assert_eq!(ubisoft_saves_in(&[root.clone(), dir.join("missing")]), [newer, old]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_games_binary_saves_are_saves() {
        // As Ubisoft Connect keeps one: its metadata, then the game's binary save.
        let body: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
        let mut game = vec![1, 0, 0, 0, 0, 0];
        game.extend((body.len() as u32).to_le_bytes());
        game.extend(&body);
        let mut ubisoft = 0x224u32.to_le_bytes().to_vec();
        ubisoft.extend(vec![0u8; 0x224]);
        ubisoft.extend(&game);
        assert_eq!(ubisoft_payload(&ubisoft), Some(&game[..]));
        assert!(is_save(&game));
        let dir = temp_dir("binary-save");
        let (from, to) = (dir.join("1.save"), dir.join("Saves").join("00000001.sav"));
        std::fs::write(&from, &ubisoft).unwrap();
        import_file(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), game);
        // Kept as it is: it can't be read for its rank.
        assert_eq!(check(&to), SaveState::Unreadable);
        assert_eq!(prepare_from(&to, None).unwrap(), Prepared::Ready);
        // Not anything with a 1 in front.
        let mut wrong = game.clone();
        wrong.push(0);
        assert!(!is_save(&wrong) && !is_save(b"\x01garbage"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prepare_imports_before_making_a_blank_save() {
        let dir = temp_dir("save-prepare");
        let path = dir.join("Saves").join("00000001.sav");
        std::fs::write(dir.join("1.save"), ubisoft_file(50_000)).unwrap();
        assert_eq!(
            prepare_from(&path, Some(dir.join("1.save"))).unwrap(),
            Prepared::Imported {
                from: dir.join("1.save"),
                raised: false
            }
        );
        assert_eq!(check(&path), SaveState::Ok { xp: 50_000 }, "the player's own progress, not rank 5");
        assert_eq!(prepare_from(&path, Some(dir.join("1.save"))).unwrap(), Prepared::Ready, "a save there is never replaced");

        std::fs::remove_dir_all(dir.join("Saves")).unwrap();
        std::fs::write(dir.join("1.save"), ubisoft_file(10)).unwrap();
        assert_eq!(
            prepare_from(&path, Some(dir.join("1.save"))).unwrap(),
            Prepared::Imported {
                from: dir.join("1.save"),
                raised: true
            }
        );
        assert_eq!(check(&path), SaveState::Ok { xp: RANK5_XP });

        std::fs::remove_dir_all(dir.join("Saves")).unwrap();
        assert_eq!(prepare_from(&path, None).unwrap(), Prepared::Created);
        assert_eq!(check(&path), SaveState::Ok { xp: RANK5_XP });
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn imports_save_files_of_either_kind() {
        let dir = temp_dir("save-file");
        let to = dir.join("Saves").join("00000001.sav");
        create_rank5(&to).unwrap();
        // One of the launcher's backups (our layout).
        let ours = dir.join("old.sav.123.bak");
        let base = String::from_utf8(BASE_SAVE.to_vec()).unwrap().replacen("<m_iXP> 6600</m_iXP>", "<m_iXP>9000</m_iXP>", 1);
        std::fs::write(&ours, assemble(&NEW_HEADER, base.as_bytes())).unwrap();
        let backup = import_file(&ours, &to).unwrap().expect("the save there was backed up");
        assert_eq!(check(&to), SaveState::Ok { xp: 9000 });
        assert_eq!(check(&backup), SaveState::Ok { xp: RANK5_XP });
        // Ubisoft Connect's.
        std::fs::write(dir.join("1.save"), ubisoft_file(12_345)).unwrap();
        import_file(&dir.join("1.save"), &to).unwrap();
        assert_eq!(check(&to), SaveState::Ok { xp: 12_345 });
        // Anything else changes nothing.
        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
        assert!(import_file(&dir.join("notes.txt"), &to).is_err());
        assert_eq!(check(&to), SaveState::Ok { xp: 12_345 });
        // The save itself: nothing to do.
        assert_eq!(import_file(&to, &to).unwrap(), None);
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
