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
    let f = fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)?;
    #[cfg(windows)]
    restrict_to_owner(path);
    Ok(f)
}

/// Windows: lets only this user, the file's owner and SYSTEM read `path`
/// (no inherited access for other users of the PC). The owner and SYSTEM by
/// SID, so any Windows language works; this user too, so a first run as
/// administrator doesn't lock out a later normal one. Best effort: without
/// icacls (Wine), the file keeps its folder's permissions.
#[cfg(windows)]
fn restrict_to_owner(path: &std::path::Path) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let user = std::env::var("USERDOMAIN")
        .ok()
        .zip(std::env::var("USERNAME").ok())
        .map(|(domain, user)| format!("{domain}\\{user}:F"));
    // With this user named, then (a name icacls doesn't know, e.g. a service's) without.
    for with_user in [user, None] {
        let mut command = std::process::Command::new("icacls");
        command.arg(path).args(["/inheritance:r", "/grant:r", "*S-1-3-4:F", "/grant:r", "*S-1-5-18:F"]);
        if let Some(user) = &with_user {
            command.args(["/grant:r", user]);
        }
        let done = command
            .arg("/q")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .is_ok_and(|s| s.success());
        if done {
            return;
        }
    }
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
