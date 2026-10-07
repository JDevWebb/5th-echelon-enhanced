//! Signs releases: the launcher only installs an update whose `SHA256SUMS`
//! carries the release key's signature (`SHA256SUMS.sig`).
//!
//! ```text
//! release-sign keygen <key file>                       make the release key; prints its public half
//! release-sign sign <key file> <SHA256SUMS> <version>  writes SHA256SUMS.sig next to it
//! release-sign verify <public key> <SHA256SUMS> <version>
//! release-sign sign-image <key file> <IMAGE> <version>   writes IMAGE.sig next to it
//! release-sign verify-image <public key> <IMAGE> <version>
//! ```
//!
//! The signature covers the version (the tag without its `v`, e.g. `0.4.0`)
//! and `SHA256SUMS` ([`identity::release_message`]); an image's, the version and the
//! `IMAGE` file naming the server image's digest ([`identity::image_message`]).
//!
//! Keep the key file off any machine that builds or publishes releases
//! automatically; see `scripts/sign-release.sh`.

use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

fn read_key(path: &Path) -> Result<identity::Identity, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let secret = identity::base32_decode(text.trim()).ok_or("the key file isn't a key")?;
    let secret: [u8; 32] = secret.try_into().map_err(|_| "the key file isn't a key")?;
    Ok(identity::Identity::from_secret(&secret))
}

fn sig_path(sums: &Path) -> PathBuf {
    let mut p = sums.as_os_str().to_owned();
    p.push(".sig");
    PathBuf::from(p)
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(text.as_bytes())
}

/// An `IMAGE` file is one line: an image by its digest, nothing else.
fn image_text(text: &str) -> Result<(), String> {
    let line = text.strip_suffix('\n').unwrap_or_default();
    let ok = !line.contains('\n')
        && line.split_once("@sha256:").is_some_and(|(name, digest)| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_' | b'/' | b':'))
                && digest.len() == 64
                && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
    if ok {
        Ok(())
    } else {
        Err("IMAGE must be one line, <image>@sha256:<digest>".into())
    }
}

fn check_version(version: &str) -> Result<(), String> {
    if identity::valid_release_version(version) {
        Ok(())
    } else {
        Err(format!("{version:?} isn't a release version (the tag without its v, e.g. 0.4.0)"))
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, key] if cmd == "keygen" => {
            let id = identity::Identity::generate();
            write_private(Path::new(key), &format!("{}\n", identity::base32_encode(&id.secret()))).map_err(|e| format!("{key}: {e}"))?;
            println!("{}", id.global_id());
            Ok(())
        }
        [cmd, key, file, version] if cmd == "sign" || cmd == "sign-image" => {
            check_version(version)?;
            let id = read_key(Path::new(key))?;
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            let message = if cmd == "sign" {
                identity::release_message(version, &text)
            } else {
                image_text(&text)?;
                identity::image_message(version, &text)
            };
            let sig = id.sign(&message);
            std::fs::write(sig_path(Path::new(file)), format!("{sig}\n")).map_err(|e| e.to_string())?;
            println!("Signed {file} as release {version} with {}", id.global_id());
            Ok(())
        }
        [cmd, public, sums, version] if cmd == "verify" || cmd == "verify-image" => {
            check_version(version)?;
            let text = std::fs::read_to_string(sums).map_err(|e| format!("{sums}: {e}"))?;
            let sig = std::fs::read_to_string(sig_path(Path::new(sums))).map_err(|e| e.to_string())?;
            let message = if cmd == "verify" { identity::release_message(version, &text) } else { identity::image_message(version, &text) };
            if identity::verify(public, &message, sig.trim()) {
                println!("Good signature");
                Ok(())
            } else {
                Err("BAD signature".into())
            }
        }
        _ => Err(
            "usage: release-sign keygen <key file> | sign <key file> <SHA256SUMS> <version> | verify <public key> <SHA256SUMS> <version> | sign-image <key file> <IMAGE> <version> | verify-image <public key> <IMAGE> <version>".into(),
        ),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
