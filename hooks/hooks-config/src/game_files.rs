//! The game's files that change play, hashed (the 0.4.9 plan's D0, brought into 0.4.8; report
//! only), for the game's sign-in to carry to its server, so the Sessions page can show a player
//! whose files differ from everyone else's. Shared by the launcher, which hashes them before
//! Play is ready, and the add-on, which reads them again as the game starts (through the cache,
//! so in a few ms; a game started without the launcher hashes them itself).
//!
//! * Only the files that matter ([`CORE`]): both executables, the game's DLLs, Ubisoft's DLL
//!   (kept as `uplay_r1_loader.orig.dll`: a crack's emulator or stripped copy differs), and
//!   the script and data packages. A few hundred MB, never the whole install.
//! * Other files that can change the game ([`is_other`]: loose `.ini` overrides, extra DLLs,
//!   ASI mods and packages) are told by name only.
//! * Hashed once: [`CACHE`] keeps each file's size, modified time and hash, so a later look
//!   only reads the folder.
//!
//! Evidence, not proof: the add-on is open source, so a rebuilt one can say anything.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest as _;
use tracing::warn;

/// The files hashed, under the game's SYSTEM folder (the add-on's own). The ones missing from
/// a copy (`steam_api.dll` on a Ubisoft Connect copy) are left out.
pub const CORE: &[&str] = &[
    "Blacklist_game.exe",
    "Blacklist_DX11_game.exe",
    "SC6.dll",
    "uplay_r1_loader.orig.dll",
    "steam_api.dll",
    "dynamicflash.umd",
    "dynamicwin.umd",
    "UMDs/Blacklist.umd",
    "UMDs/Blacklist.ass",
    "UMDs/Blacklist.asstrm",
    "UMDs/lobby.umd",
    "UMDs/lobby.ass",
    "UMDs/menu.umd",
    "UMDs/menu.ass",
];

/// The cache, in the game's SYSTEM folder.
pub const CACHE: &str = "5th-echelon-file-hashes.json";
/// The most other files named.
const MOST_OTHERS: usize = 16;
/// A file bigger than this isn't hashed (none of [`CORE`] comes near it).
const LARGEST: u64 = 1 << 30;

/// One hashed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hashed {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// What the sign-in carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    pub hashed: Vec<Hashed>,
    pub others: Vec<String>,
}

/// One cached hash: the file as it was when hashed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Cached {
    size: u64,
    modified: u64,
    sha256: String,
}

/// The files in `system` (the game's SYSTEM folder), hashed (through the cache) and named.
pub fn look(system: &Path) -> Files {
    let cache_path = system.join(CACHE);
    let cache: BTreeMap<String, Cached> = std::fs::read(&cache_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut kept = BTreeMap::new();
    let mut hashed = Vec::new();
    for name in CORE {
        let path = system.join(name);
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if !meta.is_file() || meta.len() > LARGEST {
            continue;
        }
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let sha256 = match cache.get(*name).filter(|c| c.size == meta.len() && c.modified == modified) {
            Some(c) => c.sha256.clone(),
            None => match hash(&path) {
                Ok(h) => h,
                Err(e) => {
                    warn!("Couldn't hash {name}: {e}");
                    continue;
                }
            },
        };
        kept.insert(
            name.to_string(),
            Cached {
                size: meta.len(),
                modified,
                sha256: sha256.clone(),
            },
        );
        hashed.push(Hashed {
            name: name.to_string(),
            size: meta.len(),
            sha256,
        });
    }
    if kept != cache {
        if let Err(e) = serde_json::to_vec(&kept).map_err(std::io::Error::other).and_then(|b| std::fs::write(&cache_path, b)) {
            warn!("Couldn't save the game files' hashes: {e}");
        }
    }
    let mut others: Vec<String> = ["", "UMDs/"]
        .iter()
        .flat_map(|sub| {
            std::fs::read_dir(system.join(sub))
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(move |e| format!("{sub}{}", e.file_name().to_string_lossy()))
        })
        .filter(|name| is_other(name))
        .collect();
    others.sort_by_key(|n| n.to_ascii_lowercase());
    others.truncate(MOST_OTHERS);
    Files { hashed, others }
}

/// Whether `name` (under SYSTEM) is a file that can change the game and isn't one of [`CORE`],
/// the player's settings, the add-on's own or the game's language packs.
pub fn is_other(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let changes_the_game = [".exe", ".dll", ".asi", ".ini", ".u", ".umd", ".upk", ".ass", ".asstrm"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    let known = CORE.iter().any(|c| c.eq_ignore_ascii_case(name))
        || lower == "blacklist.ini"
        || lower == "uplay_r1_loader.dll"
        || (lower.starts_with("manifest_") && lower.ends_with(".ini"))
        || (lower.starts_with("loc-") && lower.ends_with(".umd"));
    changes_the_game && !known
}

/// The SHA-256 of the file at `path`, as hex.
fn hash(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut sha = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
    }
    Ok(format!("{:x}", sha.finalize()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fes-file-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("UMDs")).unwrap();
        dir
    }

    /// The core files are hashed (the missing ones left out), the others named, and the
    /// player's settings, language packs and the add-on's own DLL are neither.
    #[test]
    fn hashes_the_core_and_names_the_others() {
        let dir = folder("look");
        std::fs::write(dir.join("Blacklist_DX11_game.exe"), b"abc").unwrap();
        std::fs::write(dir.join("UMDs/lobby.umd"), b"").unwrap();
        for other in [
            "PEC.ini",
            "dinput8.dll",
            "UMDs/extra.umd",
            "Blacklist.ini",
            "loc-fra.umd",
            "uplay_r1_loader.dll",
            "Manifest_1.ini",
            "readme.txt",
        ] {
            std::fs::write(dir.join(other), b"x").unwrap();
        }
        let files = look(&dir);
        assert_eq!(
            files.hashed,
            vec![
                Hashed {
                    name: "Blacklist_DX11_game.exe".into(),
                    size: 3,
                    sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into()
                },
                Hashed {
                    name: "UMDs/lobby.umd".into(),
                    size: 0,
                    sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into()
                },
            ]
        );
        assert_eq!(files.others, vec!["dinput8.dll", "PEC.ini", "UMDs/extra.umd"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file unchanged since it was hashed is taken from the cache, without reading it; one
    /// changed since (its size or time) is hashed again.
    #[test]
    fn an_unchanged_file_comes_from_the_cache() {
        let dir = folder("cache");
        let exe = dir.join("Blacklist_game.exe");
        std::fs::write(&exe, b"abc").unwrap();
        let first = look(&dir);
        // The cache says otherwise for the same size and time: it's believed, so not read.
        let mut cache: BTreeMap<String, Cached> = serde_json::from_slice(&std::fs::read(dir.join(CACHE)).unwrap()).unwrap();
        cache.get_mut("Blacklist_game.exe").unwrap().sha256 = "cached".into();
        std::fs::write(dir.join(CACHE), serde_json::to_vec(&cache).unwrap()).unwrap();
        assert_eq!(look(&dir).hashed[0].sha256, "cached");
        std::fs::write(&exe, b"abcd").unwrap();
        let changed = look(&dir);
        assert_ne!(changed.hashed[0].sha256, "cached");
        assert_ne!(changed.hashed[0].sha256, first.hashed[0].sha256);
        assert_eq!(changed.hashed[0].size, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
