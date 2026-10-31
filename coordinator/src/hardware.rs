//! Bans that hold for the PC (the 0.4.8 plan's F10). Servers send each player's hardware ids,
//! hashed by their game or launcher (the hwid crate), with the players report; they're keyed
//! again here with the coordinator's own secret ([`Coordinator::hardware_secret`]) and kept
//! (`player_hardware`). A player whose PC shares [`TO_BAN`] ids or more with a banned player's
//! is banned too, for as long as that ban runs: their identity across the network, as an
//! admin's ban is (F2), and the account itself. One id in common is flagged for the admins, as
//! is a new account on a virtual machine.
//!
//! An id seen on [`SHARED_BY`] players or more who aren't banned is never matched on: an OEM's
//! default, a cloned PC or a VM image. The clients already leave out placeholders.

use serde_json::json;
use serde_json::Value;

use crate::Coordinator;

/// Ids in common with a banned player's PC that ban the player.
pub const TO_BAN: i64 = 2;
/// An id held by this many players who aren't banned (by identity, else account) is shared,
/// and never matched on.
pub const SHARED_BY: i64 = 5;
/// The most ids taken for a player in one report (the clients send at most 12).
const MOST_IDS: usize = 12;
/// Ids not seen for this long go, unless their player is banned.
const KEPT_FOR: i64 = 180 * 86_400;
/// An account this new on a virtual machine is flagged.
const NEW_ACCOUNT: i64 = 7 * 86_400;
const KINDS: [&str; 5] = ["machine-guid", "smbios-uuid", "board-serial", "disk-serial", "linux-machine-id"];

/// Whether a player row is banned now: their own ban, or their identity's across the network.
const BANNED: &str = "((p.banned_at IS NOT NULL AND (p.banned_until IS NULL OR p.banned_until > ?now))
     OR EXISTS (SELECT 1 FROM identity_bans ib WHERE ib.identity = p.identity AND (ib.until IS NULL OR ib.until > ?now)))";

/// The well-formed ids in a reported player (`hardware`: [{kind, hash}]).
fn reported_ids(v: &Value) -> Vec<(String, String)> {
    let mut ids: Vec<(String, String)> = v["hardware"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    let (kind, hash) = (i["kind"].as_str()?, i["hash"].as_str()?);
                    (KINDS.contains(&kind) && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (kind.to_string(), hash.to_ascii_lowercase()))
                })
                .take(MOST_IDS)
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids
}

impl Coordinator {
    /// The secret the clients' hashes are keyed with here, made on first use and kept: the
    /// database alone doesn't tell which PC an id was.
    async fn hardware_secret(&self) -> sqlx::Result<[u8; 32]> {
        let fresh: [u8; 32] = rand::random();
        sqlx::query("INSERT OR IGNORE INTO settings (key, value) VALUES ('hardware_secret', ?)")
            .bind(identity::base32_encode(&fresh))
            .execute(&self.pool)
            .await?;
        let kept: String = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'hardware_secret'").fetch_one(&self.pool).await?;
        Ok(identity::base32_decode(&kept).and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok()).unwrap_or(fresh))
    }

    /// Keeps the hardware ids (and virtual machines) of a players report's players, then bans
    /// or flags those whose PC is a banned player's. Answers how many it banned.
    pub(crate) async fn record_hardware(&self, server: &str, players: &[Value]) -> sqlx::Result<usize> {
        use hmac::Mac as _;
        let now = identity::now();
        let secret = self.hardware_secret().await?;
        let mut checked = Vec::new();
        for v in players {
            let Some(id) = v["id"].as_i64() else { continue };
            let ids = reported_ids(v);
            let vm: Option<String> = v["vm"].as_str().map(|s| s.chars().filter(|c| !c.is_control()).take(32).collect());
            if ids.is_empty() && vm.is_none() {
                continue;
            }
            // Only the server's own players (the report took them first).
            let known: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM players WHERE server_id = ? AND id = ?")
                .bind(server)
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
            if !known {
                continue;
            }
            for (kind, hash) in &ids {
                let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&secret).expect("any key length");
                mac.update(kind.as_bytes());
                mac.update(b":");
                mac.update(hash.as_bytes());
                let keyed: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
                sqlx::query(
                    "INSERT INTO player_hardware (server_id, player, kind, hash, first_seen, last_seen) VALUES (?, ?, ?, ?, ?, ?)
                     ON CONFLICT(server_id, player, kind, hash) DO UPDATE SET last_seen = excluded.last_seen",
                )
                .bind(server)
                .bind(id)
                .bind(kind)
                .bind(keyed)
                .bind(now)
                .bind(now)
                .execute(&self.pool)
                .await?;
            }
            sqlx::query("UPDATE players SET vm = ? WHERE server_id = ? AND id = ?")
                .bind(&vm)
                .bind(server)
                .bind(id)
                .execute(&self.pool)
                .await?;
            checked.push(id);
        }
        let mut banned = 0;
        for id in checked {
            banned += usize::from(self.check_hardware(server, id, now).await?);
        }
        // Ids not seen for long go, unless their player is banned (a ban keeps its PC's).
        sqlx::query(&format!(
            "DELETE FROM player_hardware WHERE last_seen < ?1 AND NOT EXISTS (
               SELECT 1 FROM players p WHERE p.server_id = player_hardware.server_id AND p.id = player_hardware.player AND {})",
            BANNED.replace("?now", "?2")
        ))
        .bind(now - KEPT_FOR)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(banned)
    }

    /// Bans `player` on `server` if their PC shares [`TO_BAN`] ids with a banned player's,
    /// flags them if it shares one, or if they're new on a virtual machine. Whether it banned.
    async fn check_hardware(&self, server: &str, player: i64, now: i64) -> sqlx::Result<bool> {
        let me: Option<(Option<String>, bool, Option<String>, Option<i64>)> = sqlx::query_as(&format!(
            "SELECT p.identity, {} AS banned, p.vm, p.created_at FROM players p WHERE p.server_id = ?1 AND p.id = ?2",
            BANNED.replace("?now", "?3")
        ))
        .bind(server)
        .bind(player)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        let Some((identity, already, vm, created_at)) = me else { return Ok(false) };
        if already {
            return Ok(false);
        }
        if vm.is_some() && created_at.is_some_and(|c| c > now - NEW_ACCOUNT) {
            self.note_match(server, player, ("", 0), 0, "vm", now).await?;
        }
        // The banned player whose PC shares the most of this one's ids, leaving out the ids
        // that many who aren't banned share, and the player's own identity's other accounts.
        let best: Option<(String, i64, i64, Option<String>, Option<i64>, Option<String>)> = sqlx::query_as(&format!(
            "SELECT b.server_id, b.id, COUNT(DISTINCT mine.hash) AS n,
                    COALESCE((SELECT ib.reason FROM identity_bans ib WHERE ib.identity = b.identity), b.banned_reason),
                    CASE WHEN EXISTS (SELECT 1 FROM identity_bans ib WHERE ib.identity = b.identity AND (ib.until IS NULL OR ib.until > ?3))
                         THEN (SELECT ib.until FROM identity_bans ib WHERE ib.identity = b.identity) ELSE b.banned_until END,
                    b.identity
               FROM player_hardware mine
               JOIN player_hardware theirs ON theirs.hash = mine.hash AND NOT (theirs.server_id = mine.server_id AND theirs.player = mine.player)
               JOIN players b ON b.server_id = theirs.server_id AND b.id = theirs.player
              WHERE mine.server_id = ?1 AND mine.player = ?2
                AND (b.identity IS NULL OR ?4 IS NULL OR b.identity != ?4)
                AND {}
                AND (SELECT COUNT(DISTINCT COALESCE(o.identity, o.server_id || '/' || o.id))
                       FROM player_hardware x JOIN players o ON o.server_id = x.server_id AND o.id = x.player
                      WHERE x.hash = mine.hash AND NOT {}) < ?5
              GROUP BY b.server_id, b.id
              ORDER BY n DESC LIMIT 1",
            BANNED.replace("p.", "b.").replace("?now", "?3"),
            BANNED.replace("p.", "o.").replace("?now", "?3"),
        ))
        .bind(server)
        .bind(player)
        .bind(now)
        .bind(&identity)
        .bind(SHARED_BY)
        .fetch_optional(&self.pool)
        .await?;
        let Some((their_server, their_id, matched, reason, until, _)) = best else {
            return Ok(false);
        };
        if matched < TO_BAN {
            self.note_match(server, player, (&their_server, their_id), matched, "flag", now).await?;
            return Ok(false);
        }
        let reason = format!("This PC is banned with another account: {}", reason.unwrap_or_default().trim())
            .trim_end_matches([':', ' '])
            .to_string();
        if let Some(identity) = &identity {
            self.set_identity_ban(identity, Some((&reason, until)), "hardware").await?;
        }
        let pending: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM player_actions WHERE server_id = ? AND player = ? AND kind = 'ban' AND created_at > ?")
            .bind(server)
            .bind(player)
            .bind(now - crate::players::ACTION_EXPIRES)
            .fetch_one(&self.pool)
            .await?;
        if !pending {
            self.queue_action(server, player, "ban", &json!({ "reason": reason, "until": until }), "hardware").await?;
        }
        self.note_match(server, player, (&their_server, their_id), matched, "ban", now).await?;
        tracing::info!("{server}/{player}: banned, the PC of banned {their_server}/{their_id} ({matched} hardware ids in common)");
        Ok(true)
    }

    async fn note_match(&self, server: &str, player: i64, banned: (&str, i64), matched: i64, outcome: &str, now: i64) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO hardware_matches (server_id, player, banned_server, banned_player, matched, outcome, at) VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(server_id, player, outcome, banned_server, banned_player) DO UPDATE SET matched = excluded.matched",
        )
        .bind(server)
        .bind(player)
        .bind(banned.0)
        .bind(banned.1)
        .bind(matched)
        .bind(outcome)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// What the player page says of a player's PC: how many ids, which kinds, a virtual
    /// machine, and what it shares with banned players (never the hashes).
    pub(crate) async fn hardware_of(&self, server: &str, player: i64) -> sqlx::Result<Value> {
        let kinds: Vec<(String, i64, i64)> =
            sqlx::query_as("SELECT kind, COUNT(*), MAX(last_seen) FROM player_hardware WHERE server_id = ? AND player = ? GROUP BY kind ORDER BY kind")
                .bind(server)
                .bind(player)
                .fetch_all(&self.pool)
                .await?;
        let vm: Option<String> = sqlx::query_scalar("SELECT vm FROM players WHERE server_id = ? AND id = ?")
            .bind(server)
            .bind(player)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
        let matches: Vec<(String, i64, i64, String, i64, Option<String>)> = sqlx::query_as(
            "SELECT m.banned_server, m.banned_player, m.matched, m.outcome, m.at, b.name FROM hardware_matches m
               LEFT JOIN players b ON b.server_id = m.banned_server AND b.id = m.banned_player
              WHERE m.server_id = ? AND m.player = ? ORDER BY m.at DESC LIMIT 20",
        )
        .bind(server)
        .bind(player)
        .fetch_all(&self.pool)
        .await?;
        Ok(json!({
            "kinds": kinds.into_iter().map(|(kind, count, seen)| json!({ "kind": kind, "count": count, "last_seen": seen })).collect::<Vec<_>>(),
            "vm": vm,
            "matches": matches.into_iter().map(|(s, p, n, outcome, at, name)| json!({
                "server": s, "player": p, "name": name, "matched": n, "outcome": outcome, "at": at,
            })).collect::<Vec<_>>(),
        }))
    }
}
