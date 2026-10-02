//! The player's identity across servers (see the `identity` crate): made
//! once, kept in the launcher's folder, and moved to another PC by exporting
//! it.
//!
//! On Windows the key is encrypted for the Windows user (DPAPI); on Linux the
//! file is readable by the user only. Whoever has the key can sign in to
//! every account linked to it, so the export is a secret too.

use std::path::Path;
use std::path::PathBuf;

pub use identity::Identity;

/// The file in the launcher's folder.
pub const FILE: &str = "identity.key";
/// What an exported identity starts with.
const EXPORT_PREFIX: &str = "5th-echelon-identity:v1:";

/// Where the key is kept.
pub fn path() -> Option<PathBuf> {
    crate::app_data_dir().map(|d| d.join(FILE))
}

/// The player's identity, made the first time.
pub fn load_or_create() -> anyhow::Result<Identity> {
    let path = path().ok_or_else(|| anyhow::anyhow!("no folder for the launcher's settings"))?;
    load_or_create_at(&path)
}

pub fn load_or_create_at(path: &Path) -> anyhow::Result<Identity> {
    if let Some(identity) = load_at(path)? {
        return Ok(identity);
    }
    let identity = Identity::generate();
    save_at(path, &identity)?;
    Ok(identity)
}

/// The saved identity, if there is one.
pub fn load() -> anyhow::Result<Option<Identity>> {
    match path() {
        Some(path) => load_at(&path),
        None => Ok(None),
    }
}

fn load_at(path: &Path) -> anyhow::Result<Option<Identity>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let text = text.trim();
    let secret = match text.strip_prefix("dpapi:") {
        Some(blob) => hooks_config::protect::unprotect(blob)
            .ok_or_else(|| anyhow::anyhow!("{} is encrypted for another Windows user or PC; import your identity again", path.display()))?,
        None => text.to_string(),
    };
    Ok(Some(parse_secret(&secret)?))
}

/// Saves `identity` as this PC's (replacing any other).
pub fn save(identity: &Identity) -> anyhow::Result<()> {
    let path = path().ok_or_else(|| anyhow::anyhow!("no folder for the launcher's settings"))?;
    save_at(&path, identity)
}

fn save_at(path: &Path, identity: &Identity) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let secret = identity::base32_encode(&identity.secret());
    let text = match hooks_config::protect::protect(&secret) {
        Some(blob) => format!("dpapi:{blob}\n"),
        None => format!("{secret}\n"),
    };
    crate::write_private(path, text.as_bytes())?;
    Ok(())
}

/// Saves `identity` as this PC's, first keeping a different one it replaces
/// as `identity.<its short id>.key` beside it (encrypted as it was), so an
/// import over the wrong PC's identity can be undone. Returns that file.
pub fn replace(identity: &Identity) -> anyhow::Result<Option<PathBuf>> {
    let path = path().ok_or_else(|| anyhow::anyhow!("no folder for the launcher's settings"))?;
    let kept = keep_replaced_at(&path, identity)?;
    save_at(&path, identity)?;
    Ok(kept)
}

fn keep_replaced_at(path: &Path, identity: &Identity) -> anyhow::Result<Option<PathBuf>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    // One this PC can't read (another Windows user's) is kept as well, under a plain name.
    let name = match load_at(path) {
        Ok(Some(old)) if old.global_id() == identity.global_id() => return Ok(None),
        Ok(Some(old)) => format!("identity.{}.key", identity::short(&old.global_id())),
        _ => "identity.unreadable.key".to_string(),
    };
    let kept = path.with_file_name(name);
    crate::write_private(&kept, &bytes)?;
    Ok(Some(kept))
}

/// The identity as text to keep somewhere safe or move to another PC.
pub fn export(identity: &Identity) -> String {
    format!("{EXPORT_PREFIX}{}", identity::base32_encode(&identity.secret()))
}

/// An identity from [`export`]'s text.
pub fn import(text: &str) -> anyhow::Result<Identity> {
    let secret = text
        .trim()
        .strip_prefix(EXPORT_PREFIX)
        .ok_or_else(|| anyhow::anyhow!("that isn't an exported 5th Echelon identity"))?;
    parse_secret(secret)
}

fn parse_secret(text: &str) -> anyhow::Result<Identity> {
    let bytes = identity::base32_decode(text.trim()).ok_or_else(|| anyhow::anyhow!("the identity is damaged"))?;
    let secret: [u8; 32] = bytes.try_into().map_err(|_| anyhow::anyhow!("the identity is damaged"))?;
    Ok(Identity::from_secret(&secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_identity_is_kept_beside() {
        let dir = crate::testutil::temp_dir("identity-replace");
        let path = dir.join(FILE);
        assert_eq!(keep_replaced_at(&path, &Identity::generate()).unwrap(), None, "nothing to keep yet");
        let old = load_or_create_at(&path).unwrap();
        assert_eq!(keep_replaced_at(&path, &old).unwrap(), None, "the same identity again");
        let kept = keep_replaced_at(&path, &Identity::generate()).unwrap().expect("a different one keeps the old");
        assert_eq!(load_at(&kept).unwrap().unwrap().global_id(), old.global_id());
    }

    #[test]
    fn made_once_then_kept() {
        let dir = crate::testutil::temp_dir("identity");
        let path = dir.join(FILE);
        let first = load_or_create_at(&path).unwrap();
        let again = load_or_create_at(&path).unwrap();
        assert_eq!(first.global_id(), again.global_id());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "readable by the user only");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_and_import() {
        let me = Identity::generate();
        let text = export(&me);
        assert_eq!(import(&format!("  {text}\n")).unwrap().global_id(), me.global_id());
        assert!(import("hello").is_err());
        assert!(import("5th-echelon-identity:v1:AAAA").is_err());
    }
}
