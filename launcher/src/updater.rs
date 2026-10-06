//! Updates from this project's GitHub releases. Every download is checked
//! against the `SHA256SUMS` published with the release before it's used,
//! and `SHA256SUMS` must carry the release key's signature
//! (`SHA256SUMS.sig`, made offline with `scripts/sign-release.sh`): someone
//! who can change the release on GitHub still can't ship an update. The
//! signature names the release's version, which must be the tag's, and the
//! launcher only replaces itself with a newer release than it is, so an old
//! release published again under a new tag isn't installed either.
//!
//! The launcher replaces itself by renaming: Windows lets a running exe be
//! renamed, so the new one takes its name, starts, and the old one (kept as
//! `<name>.old`) is deleted on the next start.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use setup::update::Release;
use sha2::Digest as _;

/// Where releases are published.
pub const REPO: &str = "JDevWebb/5th-echelon-enhanced";
pub const RELEASES_PAGE: &str = "https://github.com/JDevWebb/5th-echelon-enhanced/releases";
/// This launcher and the server in a release, for this OS.
#[cfg(target_os = "windows")]
const LAUNCHER_ASSET: &str = "launcher.exe";
#[cfg(not(target_os = "windows"))]
const LAUNCHER_ASSET: &str = "launcher-linux-x86_64";
#[cfg(target_os = "windows")]
pub const SERVER_ASSET: &str = "dedicated_server.exe";
#[cfg(not(target_os = "windows"))]
pub const SERVER_ASSET: &str = "dedicated_server-linux-x86_64";
/// What the server is called next to the launcher.
#[cfg(target_os = "windows")]
pub const SERVER_FILE: &str = "dedicated_server.exe";
#[cfg(not(target_os = "windows"))]
pub const SERVER_FILE: &str = "dedicated_server";
const SUMS_ASSET: &str = "SHA256SUMS";
const SIG_ASSET: &str = "SHA256SUMS.sig";
/// The largest download: a launcher or server binary.
const MAX_DOWNLOAD: usize = 128 * 1024 * 1024;
/// The largest release listing, checksum file or signature.
const MAX_SMALL: usize = 1024 * 1024;

/// This build's release.
pub fn current() -> Release {
    Release::parse(env!("FE_RELEASE")).unwrap_or(Release {
        numbers: (0, 0, 0),
        pre: Some("unknown".into()),
    })
}

/// A development build: it doesn't look for updates on its own.
pub fn is_dev_build() -> bool {
    current().pre.is_some()
}

#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// The latest release.
#[derive(Debug, Clone)]
pub struct Latest {
    pub version: String,
    pub page: String,
    assets: Vec<GhAsset>,
}

impl Latest {
    pub fn newer(&self) -> bool {
        Release::parse(&self.version).is_some_and(|r| r.newer_than(&current()))
    }

    /// Not older than this launcher: what's downloaded beside it (the server) is never
    /// an older release than it, whatever GitHub's listing says is the latest.
    pub fn not_older(&self) -> bool {
        Release::parse(&self.version).is_some_and(|r| !current().newer_than(&r))
    }

    fn url(&self, name: &str) -> anyhow::Result<&str> {
        let url = self
            .assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.as_str())
            .ok_or_else(|| anyhow::anyhow!("release {} has no {name}", self.version))?;
        // What's downloaded is checked against the signed checksums anyway; this keeps the
        // asking itself to GitHub (the listing isn't signed).
        if !from_github(url) {
            anyhow::bail!("release {}'s {name} isn't on GitHub", self.version);
        }
        Ok(url)
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("5th-echelon-launcher/", env!("FE_RELEASE")))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP client")
}

/// Downloads `url`, at most `max` bytes, within `timeout` in all.
async fn get(url: &str, timeout: Duration, max: usize) -> anyhow::Result<Vec<u8>> {
    tokio::time::timeout(timeout, async {
        let mut resp = client().get(url).send().await?.error_for_status()?;
        if resp.content_length().is_some_and(|n| n > max as u64) {
            anyhow::bail!("the download is larger than expected");
        }
        let mut data = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            if data.len() + chunk.len() > max {
                anyhow::bail!("the download is larger than expected");
            }
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    })
    .await
    .map_err(|_| anyhow::anyhow!("GitHub didn't answer in time, or the download stalled"))?
}

/// Whether one of the release keys signed `sums` as release `version`.
fn signed(version: &str, sums: &str, signature: &str) -> bool {
    identity::release_signed(version, sums, signature)
}

/// Whether `data` is release `version`'s `name`: `sums` signed as that version by one of
/// `keys` (the release keys; a test's own in tests), and listing `data`'s checksum.
fn checked(version: &str, sums: &str, signature: &str, name: &str, data: &[u8], keys: &[&str]) -> anyhow::Result<()> {
    let message = identity::release_message(version, sums);
    if !identity::valid_release_version(version) || !keys.iter().any(|k| identity::verify(k, &message, signature.trim())) {
        anyhow::bail!("release {version} isn't signed by the project's release key as {version}; not installed");
    }
    let want = setup::update::checksum_for(sums, name).ok_or_else(|| anyhow::anyhow!("{SUMS_ASSET} doesn't list {name}"))?;
    let got: [u8; 32] = sha2::Sha256::digest(data).into();
    if got != want {
        anyhow::bail!("{name} doesn't match its published checksum; not installed");
    }
    Ok(())
}

/// Writes `data` to `to` through a temporary file beside it, so a failed write leaves
/// nothing behind (and the file is a program, on Linux).
fn install(data: &[u8], to: &Path) -> anyhow::Result<()> {
    let mut tmp = to.as_os_str().to_owned();
    tmp.push(".download");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, to)?;
    Ok(())
}

/// Whether `url` is an HTTPS address on GitHub, where releases and their files are.
fn from_github(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.port().is_none()
            && u.host_str()
                .is_some_and(|h| h == "github.com" || h == "objects.githubusercontent.com" || h == "release-assets.githubusercontent.com")
    })
}

/// Asks GitHub for the latest release.
pub fn latest() -> anyhow::Result<Latest> {
    let body = crate::services::rt().block_on(get(&format!("https://api.github.com/repos/{REPO}/releases/latest"), Duration::from_secs(15), MAX_SMALL))?;
    let r: GhRelease = serde_json::from_slice(&body)?;
    Ok(Latest {
        version: r.tag_name.trim_start_matches('v').to_string(),
        // Opened when the player clicks it, and not signed: only the project's own pages.
        page: if r.html_url.starts_with(&format!("https://github.com/{REPO}/")) {
            r.html_url
        } else {
            RELEASES_PAGE.to_string()
        },
        assets: r.assets,
    })
}

/// Downloads `name` from the release, verified, to `to` (through a
/// temporary file, so a failed download leaves nothing behind).
pub fn download(latest: &Latest, name: &str, to: &Path) -> anyhow::Result<()> {
    let rt = crate::services::rt();
    let sums = String::from_utf8(rt.block_on(get(latest.url(SUMS_ASSET)?, Duration::from_secs(30), MAX_SMALL))?)?;
    let sig = latest
        .url(SIG_ASSET)
        .map_err(|_| anyhow::anyhow!("release {} isn't signed yet; try again later", latest.version))?;
    let sig = String::from_utf8(rt.block_on(get(sig, Duration::from_secs(30), MAX_SMALL))?)?;
    // The tag's version, signed with the files: a signed release published
    // again under another tag fails here (before anything big is downloaded).
    if !signed(&latest.version, &sums, &sig) {
        anyhow::bail!("release {} isn't signed by the project's release key as {}; not installed", latest.version, latest.version);
    }
    let data = rt.block_on(get(latest.url(name)?, Duration::from_secs(300), MAX_DOWNLOAD))?;
    checked(&latest.version, &sums, &sig, name, &data, identity::RELEASE_KEYS)?;
    install(&data, to)
}

fn old_path(exe: &Path) -> PathBuf {
    let mut old = exe.as_os_str().to_owned();
    old.push(".old");
    PathBuf::from(old)
}

/// Replaces this launcher with the release's, starts it, and exits.
pub fn update_self(latest: &Latest) -> anyhow::Result<()> {
    if !latest.newer() {
        anyhow::bail!("release {} isn't newer than this launcher ({}); not installed", latest.version, env!("FE_RELEASE"));
    }
    let exe = std::env::current_exe()?;
    let mut new = exe.as_os_str().to_owned();
    new.push(".new");
    let new = PathBuf::from(new);
    download(latest, LAUNCHER_ASSET, &new)?;
    swap_in(&exe, &new)?;
    std::process::Command::new(&exe).spawn()?;
    std::process::exit(0);
}

/// Puts `new` in `exe`'s place, keeping `exe` as `<exe>.old` (a running exe can be renamed,
/// not overwritten, on Windows); if that fails, `exe` is put back.
fn swap_in(exe: &Path, new: &Path) -> anyhow::Result<()> {
    let old = old_path(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(new, exe) {
        // Put ourselves back.
        let _ = std::fs::rename(&old, exe);
        return Err(e.into());
    }
    Ok(())
}

/// Deletes the previous launcher left behind by an update.
pub fn clean_up() {
    if let Ok(exe) = std::env::current_exe() {
        let old = old_path(&exe);
        if old.exists() {
            // The old process may still be exiting.
            for _ in 0..20 {
                if std::fs::remove_file(&old).is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_release_key_signatures_count() {
        let sums = "abc  launcher.exe\n";
        let other = identity::Identity::generate();
        assert!(!signed("1.0.0", sums, &other.sign(&identity::release_message("1.0.0", sums))), "another key");
        assert!(!signed("1.0.0", sums, "not a signature"));
        assert!(identity::RELEASE_KEYS.iter().all(|k| identity::is_global_id(k)));
    }

    #[test]
    fn a_download_is_installed_only_signed_as_its_release_and_matching_its_checksum() {
        let key = identity::Identity::generate();
        let keys = [key.global_id()];
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let data = b"the new launcher";
        let sums = format!("{}  {LAUNCHER_ASSET}\n", hex(&sha2::Sha256::digest(data)));
        let sig = key.sign(&identity::release_message("1.2.0", &sums));
        assert!(checked("1.2.0", &sums, &sig, LAUNCHER_ASSET, data, &keys).is_ok());
        let refused = |r: anyhow::Result<()>, why: &str| {
            let e = r.expect_err(why).to_string();
            assert!(e.contains("not installed") || e.contains("doesn't list"), "{why}: {e}");
        };
        refused(checked("1.2.0", &sums, &sig, LAUNCHER_ASSET, b"something else", &keys), "another file");
        refused(
            checked("1.3.0", &sums, &sig, LAUNCHER_ASSET, data, &keys),
            "signed as another release (an old one under a new tag)",
        );
        refused(checked("1.2.0", &sums, &sig, "dedicated_server.exe", data, &keys), "a file the sums don't list");
        refused(checked("1.2.0", &sums, "not a signature", LAUNCHER_ASSET, data, &keys), "no signature");
        refused(
            checked("1.2.0", &sums, &sig, LAUNCHER_ASSET, data, identity::RELEASE_KEYS),
            "a key that isn't a release key",
        );
        refused(checked("v1.2.0;", &sums, &sig, LAUNCHER_ASSET, data, &keys), "a version that isn't one");
        // Sums changed after signing.
        let tampered = format!("{sums}{}  extra.dll\n", "0".repeat(64));
        refused(checked("1.2.0", &tampered, &sig, LAUNCHER_ASSET, data, &keys), "sums changed after signing");
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn an_update_takes_the_launchers_place_and_keeps_the_old_one() {
        let dir = std::env::temp_dir().join(format!("fes-launcher-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (exe, new) = (dir.join("launcher.exe"), dir.join("launcher.exe.new"));
        std::fs::write(&exe, "old").unwrap();
        install(b"new", &new).unwrap();
        assert!(!dir.join("launcher.exe.new.download").exists(), "nothing left beside it");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&new).unwrap().permissions().mode() & 0o777, 0o755);
        }
        swap_in(&exe, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(old_path(&exe)).unwrap(), "old", "kept, for clean_up to delete");
        // Nothing to swap in: the launcher is put back as it was.
        assert!(swap_in(&exe, &dir.join("missing")).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
