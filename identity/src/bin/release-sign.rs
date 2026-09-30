//! Signs releases: the launcher only installs an update whose `SHA256SUMS`
//! carries the release key's signature (`SHA256SUMS.sig`).
//!
//! ```text
//! release-sign keygen <key file>             make the release key; prints its public half
//! release-sign sign <key file> <SHA256SUMS>  writes SHA256SUMS.sig next to it
//! release-sign verify <public key> <SHA256SUMS>
//! ```
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

fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, key] if cmd == "keygen" => {
            let id = identity::Identity::generate();
            write_private(Path::new(key), &format!("{}\n", identity::base32_encode(&id.secret()))).map_err(|e| format!("{key}: {e}"))?;
            println!("{}", id.global_id());
            Ok(())
        }
        [cmd, key, sums] if cmd == "sign" => {
            let id = read_key(Path::new(key))?;
            let text = std::fs::read_to_string(sums).map_err(|e| format!("{sums}: {e}"))?;
            let sig = id.sign(&identity::release_message(&text));
            std::fs::write(sig_path(Path::new(sums)), format!("{sig}\n")).map_err(|e| e.to_string())?;
            println!("Signed {sums} with {}", id.global_id());
            Ok(())
        }
        [cmd, public, sums] if cmd == "verify" => {
            let text = std::fs::read_to_string(sums).map_err(|e| format!("{sums}: {e}"))?;
            let sig = std::fs::read_to_string(sig_path(Path::new(sums))).map_err(|e| e.to_string())?;
            if identity::verify(public, &identity::release_message(&text), sig.trim()) {
                println!("Good signature");
                Ok(())
            } else {
                Err("BAD signature".into())
            }
        }
        _ => Err("usage: release-sign keygen <key file> | sign <key file> <SHA256SUMS> | verify <public key> <SHA256SUMS>".into()),
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
