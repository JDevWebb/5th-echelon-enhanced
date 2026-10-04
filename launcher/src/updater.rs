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

    fn url(&self, name: &str) -> anyhow::Result<&str> {
        self.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.as_str())
            .ok_or_else(|| anyhow::anyhow!("release {} has no {name}", self.version))
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

/// Asks GitHub for the latest release.
pub fn latest() -> anyhow::Result<Latest> {
    let body = crate::services::rt().block_on(get(&format!("https://api.github.com/repos/{REPO}/releases/latest"), Duration::from_secs(15), MAX_SMALL))?;
    let r: GhRelease = serde_json::from_slice(&body)?;
    Ok(Latest {
        version: r.tag_name.trim_start_matches('v').to_string(),
        page: r.html_url,
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
    // again under another tag fails here.
    if !signed(&latest.version, &sums, &sig) {
        anyhow::bail!("release {} isn't signed by the project's release key as {}; not installed", latest.version, latest.version);
    }
    let want = setup::update::checksum_for(&sums, name).ok_or_else(|| anyhow::anyhow!("{SUMS_ASSET} doesn't list {name}"))?;
    let data = rt.block_on(get(latest.url(name)?, Duration::from_secs(300), MAX_DOWNLOAD))?;
    let got: [u8; 32] = sha2::Sha256::digest(&data).into();
    if got != want {
        anyhow::bail!("{name} doesn't match its published checksum; not installed");
    }
    let mut tmp = to.as_os_str().to_owned();
    tmp.push(".download");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, &data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, to)?;
    Ok(())
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
    let old = old_path(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old)?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        // Put ourselves back.
        let _ = std::fs::rename(&old, &exe);
        return Err(e.into());
    }
    std::process::Command::new(&exe).spawn()?;
    std::process::exit(0);
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
}
