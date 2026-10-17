//! Which client builds a network's servers take: the project's signed releases, and other
//! builds (a developer's, a modified client) only for identities the admins allowed.
//!
//! A client names itself by the SHA-256 of its own file when it signs in. That tells an honest
//! modified build or a development build from a release; it can't stop a client made to send
//! a release's hash, so it's a check for development and casual tampering, not a defence.
//! Servers judge each sign-in from what the heartbeat answer carries ([`Coordinator::client_rules`]):
//! off, report (Sessions notes builds that aren't a release's), or enforce (they're refused).

use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// A release's client files, as its SHA256SUMS names them.
pub const CLIENT_ASSETS: [&str; 3] = ["launcher.exe", "launcher-linux-x86_64", "uplay_r1_loader.dll"];
/// The setting holding the mode.
const MODE_KEY: &str = "client_check";
/// The modes, and the one until an admin chooses.
pub const MODES: [&str; 3] = ["off", "report", "enforce"];
const DEFAULT_MODE: &str = "report";
/// Releases whose builds count: the newest few (players update within days).
const KEEP_RELEASES: i64 = 5;
/// At most this many allowed identities go to servers.
const MAX_ALLOWED: i64 = 2000;

/// The client files and their SHA-256 in a release's SHA256SUMS (`<hex>  <name>` or `<hex> *<name>`).
pub fn client_builds(sums: &str) -> Vec<(&'static str, String)> {
    sums.lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let (sha, name) = (parts.next()?, parts.next()?.trim_start_matches('*'));
            let asset = CLIENT_ASSETS.iter().find(|a| **a == name)?;
            (sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (*asset, sha.to_ascii_lowercase()))
        })
        .collect()
}

impl Coordinator {
    /// Records the client builds of a signed release (its SHA256SUMS checked out).
    pub async fn note_client_builds(&self, version: &str, sums: &str) -> sqlx::Result<()> {
        let now = identity::now();
        for (asset, sha) in client_builds(sums) {
            sqlx::query("INSERT OR IGNORE INTO client_builds (version, asset, sha256, seen_at) VALUES (?, ?, ?, ?)")
                .bind(version)
                .bind(asset)
                .bind(sha)
                .bind(now)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// The mode: off, report or enforce.
    pub async fn client_mode(&self) -> sqlx::Result<String> {
        Ok(self.setting(MODE_KEY).await?.filter(|m| MODES.contains(&m.as_str())).unwrap_or_else(|| DEFAULT_MODE.into()))
    }

    pub async fn set_client_mode(&self, mode: &str) -> sqlx::Result<()> {
        self.set_setting(MODE_KEY, mode).await
    }

    /// The builds of the newest releases recorded, newest first: (version, asset, sha256).
    pub async fn genuine_builds(&self) -> sqlx::Result<Vec<(String, String, String)>> {
        sqlx::query_as(
            "SELECT version, asset, sha256 FROM client_builds
             WHERE version IN (SELECT version FROM client_builds GROUP BY version ORDER BY MAX(seen_at) DESC LIMIT ?)
             ORDER BY seen_at DESC, asset",
        )
        .bind(KEEP_RELEASES)
        .fetch_all(&self.pool)
        .await
    }

    /// The identities allowed other builds: (identity, note, added_by, added_at).
    pub async fn allowed_clients(&self) -> sqlx::Result<Vec<(String, String, String, i64)>> {
        sqlx::query_as("SELECT identity, note, added_by, added_at FROM client_allow ORDER BY added_at DESC LIMIT ?")
            .bind(MAX_ALLOWED)
            .fetch_all(&self.pool)
            .await
    }

    pub async fn allow_client(&self, identity: &str, note: &str, by: &str) -> sqlx::Result<()> {
        sqlx::query("INSERT OR REPLACE INTO client_allow (identity, note, added_by, added_at) VALUES (?, ?, ?, ?)")
            .bind(identity)
            .bind(note)
            .bind(by)
            .bind(identity::now())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Whether it was on the list.
    pub async fn disallow_client(&self, identity: &str) -> sqlx::Result<bool> {
        Ok(sqlx::query("DELETE FROM client_allow WHERE identity = ?")
            .bind(identity)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    /// What a member server's heartbeat answer carries: the mode, the release builds and the
    /// allowed identities. With no release recorded yet, nothing is judged ("off").
    pub async fn client_rules(&self) -> sqlx::Result<Value> {
        let builds = self.genuine_builds().await?;
        let mode = if builds.is_empty() { "off".to_string() } else { self.client_mode().await? };
        Ok(json!({
            "mode": mode,
            "builds": builds.into_iter().map(|(_, _, sha)| sha).collect::<Vec<_>>(),
            "allowed": self.allowed_clients().await?.into_iter().map(|(identity, ..)| identity).collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_files_are_read_from_sha256sums() {
        let a = "a".repeat(64);
        let sums = format!(
            "{a}  launcher.exe\n{}  dedicated_server-linux-x86_64\n{}  *uplay_r1_loader.dll\nnothex  launcher-linux-x86_64\n",
            "b".repeat(64),
            "C".repeat(64)
        );
        assert_eq!(client_builds(&sums), vec![("launcher.exe", a), ("uplay_r1_loader.dll", "c".repeat(64))]);
    }
}
