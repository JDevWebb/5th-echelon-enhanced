//! Alerts for the network's operators: checked every minute, raised when a condition
//! starts and resolved when it ends, shown live in the admin UI and (optionally) posted to
//! a webhook (Discord's or Slack's incoming webhooks take the same message).
//!
//! What's watched:
//! * a server that stopped sending heartbeats (seen in the last week, silent 3 minutes);
//! * a server's CPU over 90% for its last five minutes, memory over 90%, disk under 10% free;
//! * bursts of refused sign-ins (30 or more in ten minutes on a server);
//! * a server's month of traffic against its allowance: 80% used, all of it used, or on
//!   course to go over;
//! * a server whose update failed or was rolled back, and a halted rollout.

use std::collections::BTreeMap;
use std::collections::HashMap;

use serde_json::json;
use serde_json::Value;

use crate::admin::live::Event;
use crate::Coordinator;

/// How long a server may stay silent before it's offline (heartbeats come every 30 seconds).
const OFFLINE_AFTER: i64 = 180;
/// Servers silent longer than this are gone, not down: no alert.
const FORGET_AFTER: i64 = 7 * 86_400;
const CPU_HIGH: f64 = 90.0;
const MEMORY_HIGH: f64 = 0.9;
const DISK_LOW: f64 = 0.1;
const FAILED_SIGNINS: f64 = 30.0;
/// The setting with the webhook's address.
pub const WEBHOOK_SETTING: &str = "alert_webhook";

/// A condition that holds now: (kind, server, level, what to say).
type Condition = (String, String, &'static str, String);

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

impl Coordinator {
    /// What's wrong now.
    async fn conditions(&self) -> sqlx::Result<Vec<Condition>> {
        let now = identity::now();
        let mut found = Vec::new();
        let servers: Vec<(String, Option<String>, Option<i64>, Option<String>)> =
            sqlx::query_as("SELECT id, listing, last_seen, update_status FROM servers").fetch_all(&self.pool).await?;
        let name = |listing: &Option<String>, id: &str| {
            listing
                .as_deref()
                .and_then(|l| serde_json::from_str::<Value>(l).ok())
                .and_then(|l| l["name"].as_str().map(str::to_string))
                .unwrap_or_else(|| id.to_string())
        };
        let mut names = HashMap::new();
        for (id, listing, last_seen, status) in &servers {
            let n = name(listing, id);
            names.insert(id.clone(), n.clone());
            if let Some(seen) = last_seen.filter(|t| now - t > OFFLINE_AFTER && now - t < FORGET_AFTER) {
                found.push(("offline".into(), id.clone(), "bad", format!("{n} has sent no heartbeat for {} min", (now - seen) / 60)));
            }
            let updater = status
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .map(|s| s["updater"].clone())
                .unwrap_or_default();
            if let Some(state) = updater["state"].as_str().filter(|s| *s == "failed" || *s == "rolled-back") {
                let version = updater["version"].as_str().unwrap_or("?");
                found.push((
                    "update".into(),
                    id.clone(),
                    "warn",
                    format!("{n}'s update to {version} {}", if state == "failed" { "failed" } else { "was rolled back" }),
                ));
            }
        }
        // The last five minutes of each server's samples: load, and refused sign-ins.
        let rows: Vec<(String, i64, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>)> = sqlx::query_as(
            "SELECT server_id, at, CAST(json_extract(data, '$.system.cpu_percent') AS REAL), CAST(json_extract(data, '$.system.mem_total') AS REAL),
                    CAST(json_extract(data, '$.system.mem_available') AS REAL), CAST(json_extract(data, '$.system.disk_total') AS REAL), CAST(json_extract(data, '$.system.disk_free') AS REAL)
               FROM samples WHERE at >= ? ORDER BY at",
        )
        .bind(now - 300)
        .fetch_all(&self.pool)
        .await?;
        let mut by_server: BTreeMap<String, Vec<(f64, f64, f64, f64, f64)>> = BTreeMap::new();
        for (s, _, cpu, mt, ma, dt, df) in rows {
            by_server
                .entry(s)
                .or_default()
                .push((cpu.unwrap_or(0.0), mt.unwrap_or(0.0), ma.unwrap_or(0.0), dt.unwrap_or(0.0), df.unwrap_or(0.0)));
        }
        for (id, samples) in &by_server {
            let n = names.get(id).cloned().unwrap_or_else(|| id.clone());
            // CPU high in every sample of the five minutes (at least three of them).
            if samples.len() >= 3 && samples.iter().all(|s| s.0 > CPU_HIGH) {
                let avg = samples.iter().map(|s| s.0).sum::<f64>() / samples.len() as f64;
                found.push(("cpu".into(), id.clone(), "warn", format!("{n}'s CPU has been at {avg:.0}% for 5 min")));
            }
            let last = samples[samples.len() - 1];
            if last.1 > 0.0 && (last.1 - last.2) / last.1 > MEMORY_HIGH {
                found.push((
                    "memory".into(),
                    id.clone(),
                    "warn",
                    format!("{n} is using {:.0}% of its memory", (last.1 - last.2) / last.1 * 100.0),
                ));
            }
            if last.3 > 0.0 && last.4 / last.3 < DISK_LOW {
                found.push(("disk".into(), id.clone(), "warn", format!("{n} has {:.0}% of its disk free", last.4 / last.3 * 100.0)));
            }
        }
        let failed: Vec<(String, Option<f64>, Option<f64>)> = sqlx::query_as(
            "SELECT server_id, MIN(CAST(json_extract(data, '$.counters.failed_logins') AS REAL)), MAX(CAST(json_extract(data, '$.counters.failed_logins') AS REAL))
               FROM samples WHERE at >= ? GROUP BY server_id",
        )
        .bind(now - 600)
        .fetch_all(&self.pool)
        .await?;
        for (id, lo, hi) in failed {
            let n = hi.unwrap_or(0.0) - lo.unwrap_or(0.0);
            if n >= FAILED_SIGNINS {
                let name = names.get(&id).cloned().unwrap_or_else(|| id.clone());
                found.push(("signins".into(), id, "warn", format!("{n:.0} sign-ins refused on {name} in 10 min")));
            }
        }
        // Each server's month against its allowance.
        let report = self.bandwidth(86_400, None).await?;
        for a in report["allowances"].as_array().into_iter().flatten() {
            let Some(allowance) = a["allowance"].as_f64() else { continue };
            let id = a["server"].as_str().unwrap_or_default().to_string();
            let n = names.get(&id).cloned().unwrap_or_else(|| id.clone());
            let (used, projected) = (num(&a["used"]) / allowance, num(&a["projected"]) / allowance);
            if used >= 1.0 {
                found.push((
                    "allowance".into(),
                    id,
                    "bad",
                    format!("{n} has used all of this month's traffic allowance ({:.0}%)", used * 100.0),
                ));
            } else if used >= 0.8 {
                found.push((
                    "allowance".into(),
                    id,
                    "warn",
                    format!("{n} has used {:.0}% of this month's traffic allowance", used * 100.0),
                ));
            } else if projected > 1.0 && used >= 0.25 {
                found.push((
                    "allowance".into(),
                    id,
                    "warn",
                    format!("{n} is on course to use {:.0}% of this month's traffic allowance", projected * 100.0),
                ));
            }
        }
        let rollout = self.rollout().await?;
        if rollout.stage == "halted" {
            let note = if rollout.note.is_empty() { String::new() } else { format!(": {}", rollout.note) };
            found.push((
                "rollout".into(),
                String::new(),
                "bad",
                format!("The rollout of {} is halted{note}", rollout.target.as_deref().unwrap_or("?")),
            ));
        }
        Ok(found)
    }

    /// Raises alerts for new conditions and resolves those that ended. Run every minute.
    pub async fn check_alerts(&self) -> sqlx::Result<()> {
        let now = identity::now();
        let found = self.conditions().await?;
        let open: Vec<(i64, String, String, String, String, i64)> = sqlx::query_as("SELECT id, kind, server_id, level, detail, started_at FROM alerts WHERE resolved_at IS NULL")
            .fetch_all(&self.pool)
            .await?;
        let mut open_by: HashMap<(String, String), (i64, String, String, i64)> = open
            .into_iter()
            .map(|(id, kind, server, level, detail, started)| ((kind, server), (id, level, detail, started)))
            .collect();
        for (kind, server, level, detail) in found {
            match open_by.remove(&(kind.clone(), server.clone())) {
                // Still on: its words and level follow the numbers.
                Some((id, old_level, old_detail, started)) => {
                    if old_level != level || old_detail != detail {
                        sqlx::query("UPDATE alerts SET level = ?, detail = ? WHERE id = ?")
                            .bind(level)
                            .bind(&detail)
                            .bind(id)
                            .execute(&self.pool)
                            .await?;
                        self.publish(Event::Alert(alert_json(id, &kind, &server, level, &detail, started, None)));
                    }
                }
                None => {
                    let id: i64 = sqlx::query_scalar("INSERT INTO alerts (kind, server_id, level, detail, started_at) VALUES (?, ?, ?, ?, ?) RETURNING id")
                        .bind(&kind)
                        .bind(&server)
                        .bind(level)
                        .bind(&detail)
                        .bind(now)
                        .fetch_one(&self.pool)
                        .await?;
                    tracing::warn!("alert: {detail}");
                    self.publish(Event::Alert(alert_json(id, &kind, &server, level, &detail, now, None)));
                    self.notify(&format!("{} {detail}", if level == "bad" { "🔴" } else { "🟠" })).await;
                }
            }
        }
        for ((kind, server), (id, level, detail, started)) in open_by {
            sqlx::query("UPDATE alerts SET resolved_at = ? WHERE id = ?").bind(now).bind(id).execute(&self.pool).await?;
            tracing::info!("alert resolved: {detail}");
            self.publish(Event::Alert(alert_json(id, &kind, &server, &level, &detail, started, Some(now))));
            self.notify(&format!("🟢 Resolved: {detail}")).await;
        }
        Ok(())
    }

    /// Open alerts, and the last hundred resolved.
    pub async fn alerts(&self) -> sqlx::Result<Value> {
        let rows = |sql: &'static str| sqlx::query_as::<_, (i64, String, String, String, String, i64, Option<i64>)>(sql).fetch_all(&self.pool);
        let to_json = |r: Vec<(i64, String, String, String, String, i64, Option<i64>)>| {
            r.into_iter()
                .map(|(id, kind, server, level, detail, started, resolved)| alert_json(id, &kind, &server, &level, &detail, started, resolved))
                .collect::<Vec<_>>()
        };
        let active = to_json(rows("SELECT id, kind, server_id, level, detail, started_at, resolved_at FROM alerts WHERE resolved_at IS NULL ORDER BY started_at DESC").await?);
        let recent = to_json(
            rows("SELECT id, kind, server_id, level, detail, started_at, resolved_at FROM alerts WHERE resolved_at IS NOT NULL ORDER BY resolved_at DESC LIMIT 100").await?,
        );
        let webhook = self
            .setting(WEBHOOK_SETTING)
            .await?
            .and_then(|u| reqwest::Url::parse(&u).ok())
            .and_then(|u| u.host_str().map(str::to_string));
        Ok(json!({ "active": active, "recent": recent, "webhook_host": webhook, "report_alerts": self.report_alert_mode().await? }))
    }

    /// The open alerts, for the overview.
    pub(crate) async fn open_alerts(&self) -> sqlx::Result<Vec<Value>> {
        Ok(self.alerts().await?["active"].as_array().cloned().unwrap_or_default())
    }

    /// Posts `text` to the alert webhook, if one is set (in the background: a slow
    /// webhook mustn't hold the checks).
    pub(crate) async fn notify(&self, text: &str) {
        let Ok(Some(url)) = self.setting(WEBHOOK_SETTING).await else { return };
        let text = text.to_string();
        tokio::spawn(async move {
            if let Err(e) = post_webhook(&url, &text).await {
                tracing::warn!("alert webhook: {e}");
            }
        });
    }
}

fn alert_json(id: i64, kind: &str, server: &str, level: &str, detail: &str, started: i64, resolved: Option<i64>) -> Value {
    json!({ "id": id, "kind": kind, "server": server, "level": level, "detail": detail, "started_at": started, "resolved_at": resolved })
}

/// Whether `url` may be a webhook: https, a host name or address, nothing private.
pub fn valid_webhook(url: &str) -> Result<reqwest::Url, String> {
    let u = reqwest::Url::parse(url.trim()).map_err(|_| "that isn't a web address".to_string())?;
    if u.scheme() != "https" || u.host_str().is_none_or(str::is_empty) || !u.username().is_empty() || u.password().is_some() {
        return Err("the webhook is an https:// address".into());
    }
    if u.as_str().len() > 500 {
        return Err("that address is too long".into());
    }
    Ok(u)
}

/// Posts a message the way Discord (`content`) and Slack (`text`) webhooks take it. Only to
/// a public address: the coordinator mustn't be made to knock on its own network.
pub async fn post_webhook(url: &str, text: &str) -> Result<(), String> {
    let u = valid_webhook(url)?;
    let host = u.host_str().unwrap_or_default().to_string();
    let port = u.port_or_known_default().unwrap_or(443);
    let addrs: Vec<std::net::SocketAddr> = tokio::time::timeout(std::time::Duration::from_secs(5), tokio::net::lookup_host((host.as_str(), port)))
        .await
        .map_err(|_| "the webhook's name didn't resolve in time".to_string())?
        .map_err(|e| format!("the webhook's name doesn't resolve: {e}"))?
        .collect();
    if addrs.is_empty() || !addrs.iter().all(|a| crate::public_ip(a.ip())) {
        return Err("the webhook isn't on a public address".into());
    }
    let resp = crate::http()
        .post(u)
        .timeout(std::time::Duration::from_secs(10))
        .json(&json!({ "content": text, "text": text }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("the webhook answered {}", resp.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhooks_are_https_and_plain() {
        assert!(valid_webhook("https://discord.com/api/webhooks/1/abc").is_ok());
        assert!(valid_webhook("http://discord.com/api/webhooks/1/abc").is_err());
        assert!(valid_webhook("https://user:pw@example.com/hook").is_err());
        assert!(valid_webhook("not a url").is_err());
    }
}
