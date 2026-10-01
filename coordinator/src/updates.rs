//! Rolling out releases to every member server.
//!
//! The coordinator looks at GitHub every few minutes. A release whose
//! `SHA256SUMS` carries the release key's signature becomes the target, and
//! is rolled out in stages:
//!
//! 1. **canary**: one server (the quietest that installs updates) is told to
//!    update in its heartbeat answer;
//! 2. **verifying**: once it runs the release, it must keep reporting in for
//!    [`HEALTHY_FOR`];
//! 3. **rolling**: every other server is told, each when it has no players
//!    on, or [`QUIET_WAIT`] after the stage began whatever;
//! 4. **done**, when they all run it.
//!
//! A canary that doesn't come back, or whose updater rolled back, halts the
//! rollout. Admins can pause, pin (no new releases on their own), promote,
//! roll back to the release before, or roll out an older recorded release.
//!
//! Servers check the release's signature themselves before installing it
//! (see dedicated_server's `self_update`): the coordinator only chooses which
//! signed release, and when.
//!
//! Membership is conditional on keeping up: a server that doesn't install
//! updates, or still runs an older release [`DELIST_AFTER`] after a rollout
//! finished, drops out of the directory until it catches up.

use std::cmp::Ordering;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::Coordinator;

pub const REPO: &str = "JDevWebb/5th-echelon-enhanced";
/// The asset every release must carry for servers.
const SERVER_ASSET: &str = "dedicated_server-linux-x86_64";
/// How often GitHub is asked for a new release.
pub const CHECK_EVERY: Duration = Duration::from_secs(10 * 60);
/// The canary must keep reporting in on the new release this long.
const HEALTHY_FOR: i64 = 10 * 60;
/// A canary that hasn't updated after this long halts the rollout.
const CANARY_TIMEOUT: i64 = 3 * 3600;
/// Servers with players on are updated anyway this long into a stage.
const QUIET_WAIT: i64 = 2 * 3600;
/// Seen this recently, a server counts as up.
const FRESH: i64 = 180;
/// A server still behind this long after a rollout finished is delisted.
pub const DELIST_AFTER: i64 = 24 * 3600;
/// The largest release listing or checksum file.
const MAX_SMALL: usize = 1024 * 1024;

/// A release number: 1.4.0, or 1.4.0-rc.1 (before 1.4.0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    numbers: (u64, u64, u64),
    pre: Option<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let (core, pre) = text.split_once('-').map_or((text, None), |(c, p)| (c, Some(p.to_string())));
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let numbers = (parts.next()??, parts.next()??, parts.next()??);
        if parts.next().is_some() || pre.as_deref().is_some_and(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')) {
            return None;
        }
        Some(Self { numbers, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers.cmp(&other.numbers).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            // Part by part, numbers as numbers (rc.2 before rc.10), as the updater's sort -V.
            (Some(a), Some(b)) => {
                let key = |p: &str| p.parse::<u64>().map_or((1, 0, p.to_string()), |n| (0, n, String::new()));
                a.split('.').map(key).cmp(b.split('.').map(key))
            }
        })
    }
}

/// Whether `a` is a newer release than `b` (anything beats an unparsable one).
pub fn newer(a: &str, b: &str) -> bool {
    match (Version::parse(a), Version::parse(b)) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        _ => false,
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, Default)]
pub struct Rollout {
    pub target: Option<String>,
    pub previous: Option<String>,
    pub stage: String,
    pub canary: Option<String>,
    pub stage_since: i64,
    pub paused: bool,
    pub pinned: bool,
    pub note: String,
}

/// A member server as the rollout sees it.
#[derive(Debug, Clone)]
struct Member {
    id: String,
    version: String,
    auto_update: bool,
    players_online: u64,
    last_seen: i64,
    updater_state: String,
    updater_version: String,
}

impl Coordinator {
    pub async fn rollout(&self) -> sqlx::Result<Rollout> {
        sqlx::query_as("SELECT target, previous, stage, canary, stage_since, paused, pinned, note FROM rollout WHERE id = 1")
            .fetch_one(&self.pool)
            .await
    }

    async fn save_rollout(&self, r: &Rollout) -> sqlx::Result<()> {
        sqlx::query("UPDATE rollout SET target = ?, previous = ?, stage = ?, canary = ?, stage_since = ?, paused = ?, pinned = ?, note = ? WHERE id = 1")
            .bind(&r.target)
            .bind(&r.previous)
            .bind(&r.stage)
            .bind(&r.canary)
            .bind(r.stage_since)
            .bind(r.paused)
            .bind(r.pinned)
            .bind(&r.note)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn members(&self) -> sqlx::Result<Vec<Member>> {
        let rows: Vec<(String, Option<String>, Option<i64>, Option<String>)> =
            sqlx::query_as("SELECT id, listing, last_seen, update_status FROM servers").fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|(id, listing, last_seen, status)| {
                let l: Value = listing.and_then(|l| serde_json::from_str(&l).ok()).unwrap_or_default();
                let s: Value = status.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
                Member {
                    id,
                    version: l["version"].as_str().unwrap_or_default().to_string(),
                    auto_update: l["auto_update"].as_bool().unwrap_or(false),
                    players_online: l["players_online"].as_u64().unwrap_or(0),
                    last_seen: last_seen.unwrap_or(0),
                    updater_state: s["updater"]["state"].as_str().unwrap_or_default().to_string(),
                    updater_version: s["updater"]["version"].as_str().unwrap_or_default().to_string(),
                }
            })
            .collect())
    }

    /// Records a signed release, and rolls it out when it's newer than the
    /// target and the rollout isn't pinned.
    pub async fn release_found(&self, version: &str, page: &str, published_at: &str) -> sqlx::Result<bool> {
        sqlx::query("INSERT OR IGNORE INTO releases (version, page, published_at, seen_at) VALUES (?, ?, ?, ?)")
            .bind(version)
            .bind(page)
            .bind(published_at)
            .bind(identity::now())
            .execute(&self.pool)
            .await?;
        let r = self.rollout().await?;
        if r.pinned || r.target.as_deref().is_some_and(|t| !newer(version, t)) {
            return Ok(false);
        }
        self.start_rollout(version, "a new signed release").await?;
        Ok(true)
    }

    /// Rolls out `version` from the start (a canary first).
    pub async fn start_rollout(&self, version: &str, why: &str) -> sqlx::Result<()> {
        let mut r = self.rollout().await?;
        // What the network ran before: the old target, or else what most servers run.
        let previous = match r.target.clone() {
            Some(t) if t != version => Some(t),
            Some(_) => r.previous.clone(),
            None => {
                let members = self.members().await?;
                let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
                for m in members.iter().filter(|m| !m.version.is_empty()) {
                    *counts.entry(m.version.clone()).or_default() += 1;
                }
                counts.into_iter().max_by_key(|(_, n)| *n).map(|(v, _)| v)
            }
        };
        tracing::info!("Rollout: {version} ({why})");
        r.target = Some(version.to_string());
        r.previous = previous;
        r.stage = "canary".into();
        r.canary = None;
        r.stage_since = identity::now();
        r.paused = false;
        r.note = why.to_string();
        self.save_rollout(&r).await?;
        self.tick_rollout().await
    }

    /// Moves the rollout along. Run every 30 seconds.
    pub async fn tick_rollout(&self) -> sqlx::Result<()> {
        let mut r = self.rollout().await?;
        let Some(target) = r.target.clone() else { return Ok(()) };
        if r.paused {
            return Ok(());
        }
        let now = identity::now();
        let members = self.members().await?;
        let fresh: Vec<&Member> = members.iter().filter(|m| now - m.last_seen <= FRESH).collect();
        let before = (r.stage.clone(), r.canary.clone());
        match r.stage.as_str() {
            "canary" => {
                let canary = r.canary.as_ref().and_then(|c| fresh.iter().find(|m| &m.id == c)).copied();
                match canary {
                    None => {
                        // The quietest server that installs updates and isn't on the target yet.
                        let pick = fresh
                            .iter()
                            .filter(|m| m.auto_update && m.version != target)
                            .min_by_key(|m| (m.players_online, m.id.clone()));
                        match pick {
                            Some(m) => {
                                r.canary = Some(m.id.clone());
                                r.stage_since = now;
                            }
                            // Nobody left to try it on: every server runs it already, or none update.
                            None => {
                                r.stage = "rolling".into();
                                r.stage_since = now;
                            }
                        }
                    }
                    Some(m) if m.version == target => {
                        r.stage = "verifying".into();
                        r.stage_since = now;
                    }
                    Some(m) if m.updater_version == target && matches!(m.updater_state.as_str(), "failed" | "rolled-back") => {
                        r.stage = "halted".into();
                        r.note = format!("{} couldn't install {target} ({}); rollout halted", m.id, m.updater_state);
                        r.stage_since = now;
                    }
                    Some(m) if now - r.stage_since > CANARY_TIMEOUT => {
                        r.stage = "halted".into();
                        r.note = format!("{} hasn't installed {target} after {} hours; rollout halted", m.id, CANARY_TIMEOUT / 3600);
                        r.stage_since = now;
                    }
                    Some(_) => {}
                }
            }
            "verifying" => {
                let canary = r.canary.as_ref().and_then(|c| members.iter().find(|m| &m.id == c));
                match canary {
                    Some(m) if m.version == target && now - m.last_seen <= FRESH => {
                        if now - r.stage_since >= HEALTHY_FOR {
                            r.stage = "rolling".into();
                            r.stage_since = now;
                            r.note = format!("{} runs {target} fine; rolling out to the rest", m.id);
                        }
                    }
                    _ => {
                        r.stage = "halted".into();
                        r.note = format!("the canary {} stopped reporting in on {target}; rollout halted", r.canary.clone().unwrap_or_default());
                        r.stage_since = now;
                    }
                }
            }
            "rolling" => {
                if fresh.iter().all(|m| m.version == target || !m.auto_update) {
                    r.stage = "done".into();
                    r.stage_since = now;
                    r.note = format!("every server that installs updates runs {target}");
                }
            }
            _ => {}
        }
        if (r.stage.clone(), r.canary.clone()) != before {
            tracing::info!("Rollout of {target}: {} ({})", r.stage, r.note);
            self.save_rollout(&r).await?;
        }
        // The coordinator itself follows once the canary proved the release.
        if matches!(r.stage.as_str(), "rolling" | "done") {
            self.request_own_update(&target);
        }
        Ok(())
    }

    /// The release a server should install now, for its heartbeat answer.
    pub(crate) async fn update_for(&self, server: &str, version: &str, players_online: u64) -> sqlx::Result<Option<String>> {
        let r = self.rollout().await?;
        let Some(target) = r.target else { return Ok(None) };
        if r.paused || version == target {
            return Ok(None);
        }
        let now = identity::now();
        let asked = match r.stage.as_str() {
            "canary" => r.canary.as_deref() == Some(server),
            "rolling" | "done" => players_online == 0 || now - r.stage_since >= QUIET_WAIT,
            _ => false,
        };
        Ok(asked.then_some(target))
    }

    /// Asks this machine's updater (if the installer set one up next to the
    /// coordinator) for `version`, as servers do.
    fn request_own_update(&self, version: &str) {
        let Some(dir) = self.data_dir.get() else { return };
        if version == env!("FE_RELEASE") || !std::path::Path::new("/etc/systemd/system/5th-echelon-update.path").exists() {
            return;
        }
        let file = dir.join("update-request");
        if std::fs::read_to_string(&file).is_ok_and(|v| v.trim() == version) {
            return;
        }
        let tmp = dir.join("update-request.tmp");
        if std::fs::write(&tmp, format!("{version}\n")).and_then(|()| std::fs::rename(&tmp, &file)).is_ok() {
            tracing::info!("Rollout: asked this machine's updater for {version}");
        }
    }

    // Admin actions.

    pub async fn set_paused(&self, paused: bool) -> sqlx::Result<()> {
        sqlx::query("UPDATE rollout SET paused = ? WHERE id = 1").bind(paused).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn set_pinned(&self, pinned: bool) -> sqlx::Result<()> {
        sqlx::query("UPDATE rollout SET pinned = ? WHERE id = 1").bind(pinned).execute(&self.pool).await?;
        Ok(())
    }

    /// Skips the canary: every server updates now.
    pub async fn promote(&self) -> sqlx::Result<()> {
        sqlx::query("UPDATE rollout SET stage = 'rolling', stage_since = ?, note = 'promoted by an admin' WHERE id = 1 AND target IS NOT NULL")
            .bind(identity::now())
            .execute(&self.pool)
            .await?;
        self.tick_rollout().await
    }

    /// Stops asking servers to update (they keep what they run).
    pub async fn halt(&self, why: &str) -> sqlx::Result<()> {
        sqlx::query("UPDATE rollout SET stage = 'halted', stage_since = ?, note = ? WHERE id = 1")
            .bind(identity::now())
            .bind(why)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Goes back to the release before, on every server at once (their
    /// updaters keep it, so it needs no download), and pins it.
    pub async fn roll_back(&self) -> Result<String, String> {
        let r = self.rollout().await.map_err(|e| e.to_string())?;
        let previous = r.previous.clone().ok_or("there's no release before this one to go back to")?;
        let target = r.target.clone().unwrap_or_default();
        let back = Rollout {
            target: Some(previous.clone()),
            previous: Some(target.clone()),
            stage: "rolling".into(),
            canary: None,
            stage_since: identity::now() - QUIET_WAIT,
            paused: false,
            pinned: true,
            note: format!("rolled back from {target} by an admin"),
        };
        self.save_rollout(&back).await.map_err(|e| e.to_string())?;
        Ok(previous)
    }

    /// Rolls out a release recorded earlier (a signed one), with a canary.
    pub async fn roll_out(&self, version: &str) -> Result<(), String> {
        let known: Option<String> = sqlx::query_scalar("SELECT version FROM releases WHERE version = ?")
            .bind(version)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        if known.is_none() {
            return Err(format!("{version} isn't a signed release the coordinator has seen"));
        }
        self.start_rollout(version, "started by an admin").await.map_err(|e| e.to_string())?;
        self.set_pinned(true).await.map_err(|e| e.to_string())
    }

    pub async fn releases(&self) -> sqlx::Result<Vec<(String, String, String, i64)>> {
        let mut list: Vec<(String, String, String, i64)> = sqlx::query_as("SELECT version, page, published_at, seen_at FROM releases").fetch_all(&self.pool).await?;
        list.sort_by(|a, b| Version::parse(&b.0).cmp(&Version::parse(&a.0)));
        Ok(list)
    }
}

/// Why the directory leaves a server out, if it does: it doesn't install
/// updates, or it's still behind long after a rollout finished.
pub fn delisted(r: &Rollout, version: &str, auto_update: bool, now: i64) -> Option<String> {
    let target = r.target.as_deref()?;
    if version == target {
        return None;
    }
    if !auto_update {
        return Some(format!("runs {version} and doesn't install updates (the network runs {target})"));
    }
    (r.stage == "done" && now - r.stage_since > DELIST_AFTER).then(|| format!("still runs {version}, a day after the network moved to {target}"))
}

/// Asks GitHub for the latest release and checks its signature. Answers
/// (version, page, published) for a signed release with a server build.
pub async fn latest_signed(http: &reqwest::Client) -> Result<(String, String, String), String> {
    let get = |url: String| async move {
        let resp = http.get(&url).send().await.and_then(reqwest::Response::error_for_status).map_err(|e| e.to_string())?;
        if resp.content_length().is_some_and(|n| n > MAX_SMALL as u64) {
            return Err("too large".to_string());
        }
        let body = resp.bytes().await.map_err(|e| e.to_string())?;
        if body.len() > MAX_SMALL {
            return Err("too large".to_string());
        }
        Ok::<_, String>(body.to_vec())
    };
    let body = get(format!("https://api.github.com/repos/{REPO}/releases/latest")).await?;
    let release: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let version = release["tag_name"].as_str().unwrap_or_default().trim_start_matches('v').to_string();
    if Version::parse(&version).is_none() {
        return Err(format!("{version:?} isn't a release number"));
    }
    let asset = |name: &str| {
        release["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|a| a["name"] == name))
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("release {version} has no {name}"))
    };
    let sums = String::from_utf8(get(asset("SHA256SUMS")?).await?).map_err(|e| e.to_string())?;
    let signature = String::from_utf8(get(asset("SHA256SUMS.sig")?).await?).map_err(|e| e.to_string())?;
    if !identity::release_signed(&sums, &signature) {
        return Err(format!("release {version} isn't signed with the release key"));
    }
    if !sums.lines().any(|l| l.split_whitespace().nth(1).map(|n| n.trim_start_matches('*')) == Some(SERVER_ASSET)) {
        return Err(format!("release {version} has no {SERVER_ASSET}"));
    }
    Ok((
        version,
        release["html_url"].as_str().unwrap_or_default().to_string(),
        release["published_at"].as_str().unwrap_or_default().to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order() {
        assert!(newer("1.4.0", "1.3.9"));
        assert!(newer("1.4.0", "1.4.0-rc.2"));
        assert!(newer("1.4.0-rc.2", "1.4.0-rc.1"));
        assert!(newer("1.4.0-rc.10", "1.4.0-rc.2"));
        assert!(newer("0.3.1", "0.3.0-dev"));
        assert!(!newer("1.4.0", "1.4.0"));
        assert!(!newer("junk", "1.0.0"));
        assert!(newer("1.0.0", ""));
        assert!(Version::parse("1.2").is_none());
        assert!(Version::parse("1.2.3-").is_none());
        assert!(Version::parse("1.2.3-a;b").is_none());
    }

    #[test]
    fn delisting() {
        let r = Rollout {
            target: Some("1.4.0".into()),
            stage: "done".into(),
            stage_since: 1000,
            ..Rollout::default()
        };
        assert_eq!(delisted(&r, "1.4.0", false, 1000), None, "on the target");
        assert!(delisted(&r, "1.3.0", false, 1000).is_some(), "no updates");
        assert_eq!(delisted(&r, "1.3.0", true, 1000 + DELIST_AFTER), None, "still in the grace");
        assert!(delisted(&r, "1.3.0", true, 1001 + DELIST_AFTER).is_some());
        assert_eq!(delisted(&Rollout::default(), "1.3.0", false, 0), None, "nothing rolled out yet");
    }
}
