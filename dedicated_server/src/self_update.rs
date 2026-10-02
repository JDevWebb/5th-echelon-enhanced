//! Installing the releases the coordinator rolls out.
//!
//! The server can't replace itself (it runs unprivileged, its program folder
//! read-only). It writes the version it was asked for to `update-request`
//! in its folder; a root service the installer sets up
//! (`5th-echelon-update.path`) sees the file, downloads that release from
//! GitHub, checks `SHA256SUMS` against the release key's signature and the
//! binaries against `SHA256SUMS`, swaps them in and restarts the server. If
//! the new server doesn't come back healthy, the old one is put back. It
//! records what happened in `/var/lib/5th-echelon-update/update-status.json`
//! (root's: the server only reads it), which the server reports.
//!
//! A coordinator can only ever have a server install a release signed with
//! the release key: no newer than what's on GitHub, and no older than the
//! one installed, except the one it replaced (a rollback).

use std::path::Path;

use serde_json::json;
use serde_json::Value;
use slog::info;
use slog::warn;
use slog::Logger;

pub const REQUEST_FILE: &str = "update-request";
pub const STATUS_FILE: &str = "update-status.json";
/// Where the installer's updater keeps its status; earlier updaters wrote
/// `STATUS_FILE` in the server's folder.
const UPDATER_STATUS: &str = "/var/lib/5th-echelon-update/update-status.json";
/// The unit the installer adds; without it, nothing would act on a request.
const UPDATER_UNIT: &str = "/etc/systemd/system/5th-echelon-update.path";
/// A failed update of one version isn't tried again for this long.
const RETRY_AFTER_SECS: i64 = 6 * 3600;

/// Whether the installer's updater is set up on this machine.
pub fn updater_installed() -> bool {
    cfg!(target_os = "linux") && Path::new(UPDATER_UNIT).exists()
}

/// Whether `version` looks like a release version ("1.4.0", "1.4.0-rc.1").
fn valid_version(version: &str) -> bool {
    (1..=32).contains(&version.len()) && version.starts_with(|c: char| c.is_ascii_digit()) && version.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

fn status_file() -> Value {
    let read = |path: &str| {
        let file = std::fs::File::open(path).ok()?;
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::Read::take(file, 4096), &mut text).ok()?;
        serde_json::from_str(&text).ok()
    };
    read(UPDATER_STATUS).or_else(|| read(STATUS_FILE)).unwrap_or(Value::Null)
}

/// The coordinator asks for `version`: asks the updater for it, unless it's
/// running already, it's off, or it failed lately.
pub fn request(logger: &Logger, version: &str, enabled: bool) {
    let version = version.trim();
    if !enabled || version == crate::community_api::RELEASE || !valid_version(version) || !updater_installed() {
        return;
    }
    if std::fs::read_to_string(REQUEST_FILE).is_ok_and(|v| v.trim() == version) {
        return;
    }
    let status = status_file();
    if status["version"].as_str() == Some(version)
        && matches!(status["state"].as_str(), Some("failed" | "rolled-back"))
        && identity::now() - status["at"].as_i64().unwrap_or(0) < RETRY_AFTER_SECS
    {
        return;
    }
    let tmp = format!("{REQUEST_FILE}.tmp");
    match std::fs::write(&tmp, format!("{version}\n")).and_then(|()| std::fs::rename(&tmp, REQUEST_FILE)) {
        Ok(()) => info!(logger, "Update: the coordinator rolls out {version}; asked the updater for it"),
        Err(e) => warn!(logger, "Update: couldn't ask the updater for {version}: {e}"),
    }
}

/// What to report: whether updates are on, what's asked for, and what the
/// updater last did.
pub fn status(enabled: bool) -> Value {
    json!({
        "auto_update": enabled && updater_installed(),
        "running": crate::community_api::RELEASE,
        "requested": std::fs::read_to_string(REQUEST_FILE).ok().map(|v| v.trim().to_string()),
        "updater": status_file(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        for v in ["1.4.0", "0.3.1-rc.2", "10.0.0"] {
            assert!(valid_version(v), "{v}");
        }
        for v in ["", "v1.0", "1.0;rm -rf", "../1", "1 0", &"1".repeat(40)] {
            assert!(!valid_version(v), "{v:?}");
        }
    }
}
