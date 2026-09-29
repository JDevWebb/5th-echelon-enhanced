//! Secret keys that must survive a restart.
//!
//! The API's login tokens are sealed with a secretbox key. Upstream generated
//! a fresh one on every start, so every launcher and game that was logged in
//! before a restart got "Invalid token" until the player logged in again.
//! The key is now kept in a file next to the database.

use std::fs;
use std::io::ErrorKind;
use std::io::Write;
use std::path::Path;

use sodiumoxide::crypto::secretbox;
use sodiumoxide::crypto::secretbox::Key;

/// File holding the API token key, relative to the working directory (next
/// to `5th-echelon.db`).
pub const API_KEY_FILE: &str = "api.key";
/// File holding the admin API's preshared key.
pub const ADMIN_KEY_FILE: &str = "admin.key";

/// Loads the key stored at `path`, creating it with a random key if the file
/// doesn't exist. A file of the wrong size is an error rather than silently
/// replaced, which would log everyone out again.
pub fn load_or_create(path: &Path) -> eyre::Result<Key> {
    match fs::read(path) {
        Ok(bytes) => Key::from_slice(&bytes).ok_or_else(|| {
            eyre::eyre!(
                "{} is not a valid key ({} bytes, expected {}); delete it to create a new one",
                path.display(),
                bytes.len(),
                secretbox::KEYBYTES
            )
        }),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let key = secretbox::gen_key();
            let tmp = path.with_extension("tmp");
            let mut f = private_file(&tmp)?;
            f.write_all(&key.0)?;
            f.sync_all()?;
            fs::rename(&tmp, path)?;
            Ok(key)
        }
        Err(e) => Err(eyre::eyre!("read {}: {e}", path.display())),
    }
}

/// Writes a secret as text, readable only by the server's user (on Unix).
pub fn write_secret_text(path: &Path, text: &str) -> eyre::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = private_file(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn private_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn private_file(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fe-keys-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn key_survives_a_restart() {
        let dir = temp_dir("restart");
        let path = dir.join(API_KEY_FILE);
        let first = load_or_create(&path).unwrap();
        let second = load_or_create(&path).unwrap();
        assert_eq!(first, second, "a restart must keep the key, or every login token becomes invalid");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn damaged_key_is_an_error_not_a_new_key() {
        let dir = temp_dir("damaged");
        let path = dir.join(API_KEY_FILE);
        fs::write(&path, b"short").unwrap();
        assert!(load_or_create(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"short", "the file must not be replaced");
        fs::remove_dir_all(dir).unwrap();
    }
}
