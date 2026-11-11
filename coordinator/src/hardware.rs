//! Bans that hold for the PC (the 0.5.0 plan's F10). Servers send each player's hardware ids,
//! hashed by their game or launcher (the hwid crate), with the players report; they're keyed
//! again here with the coordinator's own secret ([`Coordinator::hardware_secret`]) and kept
//! (`player_hardware`). A player whose PC shares [`TO_BAN`] ids or more with a banned player's
//! is banned too, for as long as that ban runs: their identity across the network, as an
//! admin's ban is (F2), and the account itself. One id in common is flagged for the admins, as
//! is a new account on a virtual machine.
//!
//! The clients salt the hashes per server (the server's id, else its host name; hwid's
//! `fes-hw-v2`), so a PC's ids match only between accounts on the same server: one matchmaking
//! server for a whole network (the community network's) matches every player, but a network of
//! several game servers no longer matches one PC across them, and no server can link a player
//! to another server's accounts by their PC.
//!
//! An id seen on [`SHARED_BY`] players or more who aren't banned is never matched on: an OEM's
//! default, a cloned PC or a VM image. The clients already leave out placeholders.
//!
//! What a member server says of its own players (a ban, an identity, ids) is never enough to
//! ban anyone elsewhere (security review, 2026-10-31): only a ban an admin made here counts
//! ([`admin_banned`]; a ban made for a PC never leads to another), only the ids the banned
//! player had before that ban count, and only an identity the player linked with their own
//! signature (`links`) is banned across the network; otherwise only the account is.

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
const KINDS: [&str; 6] = ["machine-guid", "smbios-uuid", "board-serial", "disk-serial", "linux-machine-id", "network"];

/// Whether a player row is banned now: their own ban, or their identity's across the network.
const BANNED: &str = "((p.banned_at IS NOT NULL AND (p.banned_until IS NULL OR p.banned_until > ?now))
     OR EXISTS (SELECT 1 FROM identity_bans ib WHERE ib.identity = p.identity AND (ib.until IS NULL OR ib.until > ?now)))";

/// Player row `b`'s identity, when the account it names linked it with its own signature
/// (`links`, by name: a server can report any identity on any row).
const B_LINKED: &str = "JOIN links l ON l.global_id = ib.identity AND l.server_id = b.server_id AND lower(l.username) = lower(b.name)";

/// Whether player row `b` is under a ban an admin made here and still running: their linked
/// identity's network ban, or a ban an admin queued for the account, carried out by its
/// server, with no unban queued after it and not yet ended (by the action's own end). Never a
/// ban made for a PC, nor what a server only says of its players.
fn admin_banned() -> String {
    format!(
        "(EXISTS (SELECT 1 FROM identity_bans ib {B_LINKED} WHERE ib.identity = b.identity AND ib.by != 'hardware' AND (ib.until IS NULL OR ib.until > ?3))
          OR EXISTS ({LAST_ADMIN_BAN} AND (json_extract(a.args, '$.until') IS NULL OR json_extract(a.args, '$.until') > ?3)))"
    )
}

/// The latest ban an admin queued for account `b` that its server carried out, unless an
/// unban was queued after it.
const LAST_ADMIN_BAN: &str = "SELECT 1 FROM player_actions a WHERE a.server_id = b.server_id AND a.player = b.id AND a.kind = 'ban'
       AND a.created_by != 'hardware' AND a.status = 'done'
       AND NOT EXISTS (SELECT 1 FROM player_actions u WHERE u.server_id = a.server_id AND u.player = a.player AND u.kind = 'unban' AND u.id > a.id)";

/// When that admin ban was made: the banned player's ids first seen after it don't count.
fn admin_banned_at() -> String {
    format!(
        "COALESCE(
          (SELECT ib.at FROM identity_bans ib {B_LINKED} WHERE ib.identity = b.identity AND ib.by != 'hardware'),
          (SELECT MIN(a.created_at) FROM player_actions a WHERE a.server_id = b.server_id AND a.player = b.id AND a.kind = 'ban'
            AND a.created_by != 'hardware' AND a.status = 'done'))"
    )
}

/// That ban's reason and end, from the admin's ban itself (never the server's report).
fn admin_ban_terms() -> String {
    format!(
        "CASE WHEN EXISTS (SELECT 1 FROM identity_bans ib {B_LINKED} WHERE ib.identity = b.identity AND ib.by != 'hardware' AND (ib.until IS NULL OR ib.until > ?3))
              THEN (SELECT ib.reason FROM identity_bans ib WHERE ib.identity = b.identity)
              ELSE (SELECT json_extract(a.args, '$.reason') FROM player_actions a WHERE a.server_id = b.server_id AND a.player = b.id AND a.kind = 'ban'
                     AND a.created_by != 'hardware' AND a.status = 'done' ORDER BY a.id DESC LIMIT 1) END,
         CASE WHEN EXISTS (SELECT 1 FROM identity_bans ib {B_LINKED} WHERE ib.identity = b.identity AND ib.by != 'hardware' AND (ib.until IS NULL OR ib.until > ?3))
              THEN (SELECT ib.until FROM identity_bans ib WHERE ib.identity = b.identity)
              ELSE (SELECT json_extract(a.args, '$.until') FROM player_actions a WHERE a.server_id = b.server_id AND a.player = b.id AND a.kind = 'ban'
                     AND a.created_by != 'hardware' AND a.status = 'done' ORDER BY a.id DESC LIMIT 1) END"
    )
}

/// A new account this long after a ban an admin made is compared with the banned player.
const EVASION_WINDOW: i64 = 30 * 86_400;
/// Friends in common (and at least half of the new account's) that flag it.
const FRIENDS_IN_COMMON: i64 = 3;
/// Play by hour of the week this alike (cosine) is said beside another sign.
const PLAY_TIMES_ALIKE: f64 = 0.8;
/// Each account played at least this long (seconds) for its hours to be compared.
const ENOUGH_PLAY: f64 = 5.0 * 3600.0;

/// How alike two hour-of-the-week profiles are (cosine), when each has [`ENOUGH_PLAY`].
fn cosine(a: &[f64; 168], b: &[f64; 168]) -> Option<f64> {
    let (sa, sb): (f64, f64) = (a.iter().sum(), b.iter().sum());
    if sa < ENOUGH_PLAY || sb < ENOUGH_PLAY {
        return None;
    }
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f64; 168]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    Some(dot / (norm(a) * norm(b)))
}

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
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    // The client's ids, then the network's (which a server always sends): neither crowds out the other.
    let (network, mut ids): (Vec<_>, Vec<_>) = ids.into_iter().partition(|(kind, _)| kind == "network");
    ids.truncate(MOST_IDS);
    ids.extend(network.into_iter().take(1));
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
            if self.check_hardware(server, id, now).await? {
                banned += 1;
            } else {
                self.check_evasion(server, id, now).await?;
            }
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
        let Some((reported_identity, already, vm, created_at)) = me else { return Ok(false) };
        if already {
            return Ok(false);
        }
        if vm.is_some() && created_at.is_some_and(|c| c > now - NEW_ACCOUNT) {
            self.note_match(server, player, ("", 0), 0, "vm", now).await?;
        }
        // Only an identity this account linked with its own signature: a server can report
        // any identity on any row.
        let identity: Option<String> = sqlx::query_scalar(
            "SELECT p.identity FROM players p JOIN links l ON l.global_id = p.identity AND l.server_id = p.server_id AND lower(l.username) = lower(p.name)
              WHERE p.server_id = ? AND p.id = ?",
        )
        .bind(server)
        .bind(player)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        // The banned player whose PC shares the most of this one's ids (theirs from before
        // the ban), leaving out the ids that many who aren't banned share, and the player's
        // own identity's other accounts.
        let best: Option<(String, i64, i64, Option<String>, Option<i64>, Option<String>)> = sqlx::query_as(&format!(
            "SELECT b.server_id, b.id, COUNT(DISTINCT mine.hash) AS n, {terms}, b.identity
               FROM player_hardware mine
               JOIN player_hardware theirs ON theirs.hash = mine.hash AND NOT (theirs.server_id = mine.server_id AND theirs.player = mine.player)
               JOIN players b ON b.server_id = theirs.server_id AND b.id = theirs.player
              WHERE mine.server_id = ?1 AND mine.player = ?2
                AND (b.identity IS NULL OR ?4 IS NULL OR b.identity != ?4)
                AND mine.kind != 'network' AND {banned}
                AND theirs.first_seen <= {banned_at}
                AND (SELECT COUNT(DISTINCT COALESCE(o.identity, o.server_id || '/' || o.id))
                       FROM player_hardware x JOIN players o ON o.server_id = x.server_id AND o.id = x.player
                      WHERE x.hash = mine.hash AND NOT {}) < ?5
              GROUP BY b.server_id, b.id
              ORDER BY n DESC LIMIT 1",
            BANNED.replace("p.", "o.").replace("?now", "?3"),
            terms = admin_ban_terms(),
            banned = admin_banned(),
            banned_at = admin_banned_at(),
        ))
        .bind(server)
        .bind(player)
        .bind(now)
        .bind(identity.as_ref().or(reported_identity.as_ref()))
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
        self.note_match_saying(server, player, banned, matched, outcome, None, now).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn note_match_saying(&self, server: &str, player: i64, banned: (&str, i64), matched: i64, outcome: &str, detail: Option<&str>, now: i64) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO hardware_matches (server_id, player, banned_server, banned_player, matched, outcome, detail, at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(server_id, player, outcome, banned_server, banned_player) DO UPDATE SET matched = excluded.matched, detail = excluded.detail",
        )
        .bind(server)
        .bind(player)
        .bind(banned.0)
        .bind(banned.1)
        .bind(matched)
        .bind(outcome)
        .bind(detail)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Flags `player` on `server` for the admins when they look like a player an admin banned
    /// lately, back under a new name: an account made within [`EVASION_WINDOW`] after that ban,
    /// signing in from the same network (an IPv4 /24 or IPv6 /48), or with friends mostly
    /// the banned player's; alike play times (by hour of the week) are said beside, never
    /// enough alone. Never a ban: networks are shared (homes, carriers), friends too.
    async fn check_evasion(&self, server: &str, player: i64, now: i64) -> sqlx::Result<()> {
        let me: Option<(Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT p.created_at, (SELECT l.global_id FROM links l WHERE l.global_id = p.identity AND l.server_id = p.server_id AND lower(l.username) = lower(p.name))
               FROM players p WHERE p.server_id = ? AND p.id = ?",
        )
        .bind(server)
        .bind(player)
        .fetch_optional(&self.pool)
        .await?;
        let Some((Some(created), identity)) = me else { return Ok(()) };
        if created < now - EVASION_WINDOW {
            return Ok(());
        }
        let candidates: Vec<(String, i64, Option<String>, i64, Option<String>)> = sqlx::query_as(&format!(
            "SELECT b.server_id, b.id, b.identity, {at} AS banned_at, b.name FROM players b
              WHERE {banned} AND {at} BETWEEN ?4 - ?5 AND ?4
                AND NOT (b.server_id = ?1 AND b.id = ?2) AND (b.identity IS NULL OR ?6 IS NULL OR b.identity != ?6)
              LIMIT 50",
            at = admin_banned_at(),
            banned = admin_banned(),
        ))
        .bind(server)
        .bind(player)
        .bind(now)
        .bind(created)
        .bind(EVASION_WINDOW)
        .bind(&identity)
        .fetch_all(&self.pool)
        .await?;
        for (their_server, their_id, their_identity, banned_at, _) in candidates {
            let network: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM player_hardware mine JOIN player_hardware theirs ON theirs.hash = mine.hash
                  WHERE mine.server_id = ? AND mine.player = ? AND mine.kind = 'network'
                    AND theirs.server_id = ? AND theirs.player = ? AND theirs.kind = 'network' AND theirs.first_seen <= ?",
            )
            .bind(server)
            .bind(player)
            .bind(&their_server)
            .bind(their_id)
            .bind(banned_at)
            .fetch_one(&self.pool)
            .await?;
            let (common, mine) = match (&identity, &their_identity) {
                (Some(me), Some(them)) => self.friends_in_common(me, them).await?,
                _ => (0, 0),
            };
            let friends = common >= FRIENDS_IN_COMMON && common * 2 >= mine;
            if !network && !friends {
                continue;
            }
            let alike = self.play_times_alike(server, player, (&their_server, their_id), banned_at, now).await?;
            let mut said = Vec::new();
            if network {
                said.push(String::from("signs in from the same network"));
            }
            if friends {
                said.push(format!("{common} of their {mine} friends are the banned player's"));
            }
            if let Some(alike) = alike.filter(|a| *a >= PLAY_TIMES_ALIKE) {
                said.push(format!("plays at the same hours ({:.0}% alike)", alike * 100.0));
            }
            let signs = i64::try_from(said.len()).unwrap_or(0);
            self.note_match_saying(server, player, (&their_server, their_id), signs, "evasion", Some(&said.join("; ")), now)
                .await?;
        }
        Ok(())
    }

    /// How many friends two identities have in common, and how many the first has.
    async fn friends_in_common(&self, me: &str, them: &str) -> sqlx::Result<(i64, i64)> {
        const FRIENDS_OF: &str = "SELECT b AS f FROM friendships WHERE a = ?X AND friends = 1 UNION SELECT a FROM friendships WHERE b = ?X AND friends = 1";
        let mine: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM ({})", FRIENDS_OF.replace("?X", "?1")))
            .bind(me)
            .fetch_one(&self.pool)
            .await?;
        let common: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM ({} INTERSECT {})",
            FRIENDS_OF.replace("?X", "?1"),
            FRIENDS_OF.replace("?X", "?2")
        ))
        .bind(me)
        .bind(them)
        .fetch_one(&self.pool)
        .await?;
        Ok((common, mine))
    }

    /// How alike two accounts' play times are, by hour of the week (cosine, 0 to 1): the new
    /// account's since it was made, the banned one's in the [`EVASION_WINDOW`] before its ban.
    /// None when either has played less than [`ENOUGH_PLAY`].
    async fn play_times_alike(&self, server: &str, player: i64, banned: (&str, i64), banned_at: i64, now: i64) -> sqlx::Result<Option<f64>> {
        let hours = |sessions: Vec<(i64, Option<i64>)>, end: i64| {
            let mut bins = [0f64; 168];
            for (start, stop) in sessions {
                let mut t = start;
                let stop = stop.unwrap_or(end).min(end);
                while t < stop {
                    let next = (t / 3600 + 1) * 3600;
                    // 1970-01-01 was a Thursday: hours of the week from there.
                    let bin = usize::try_from((t / 3600 + 72) % 168).unwrap_or(0);
                    #[allow(clippy::cast_precision_loss)]
                    {
                        bins[bin] += (next.min(stop) - t) as f64;
                    }
                    t = next;
                }
            }
            bins
        };
        let sessions = |server: String, player: i64, from: i64, to: i64| async move {
            sqlx::query_as::<_, (i64, Option<i64>)>("SELECT started, ended FROM play_sessions WHERE server_id = ? AND player = ? AND started >= ? AND started < ? LIMIT 5000")
                .bind(server)
                .bind(player)
                .bind(from)
                .bind(to)
                .fetch_all(&self.pool)
                .await
        };
        let mine = hours(sessions(server.to_string(), player, now - EVASION_WINDOW, now).await?, now);
        let theirs = hours(sessions(banned.0.to_string(), banned.1, banned_at - EVASION_WINDOW, banned_at).await?, banned_at);
        Ok(cosine(&mine, &theirs))
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
        let matches: Vec<(String, i64, i64, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT m.banned_server, m.banned_player, m.matched, m.outcome, m.at, b.name, m.detail FROM hardware_matches m
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
            "matches": matches.into_iter().map(|(s, p, n, outcome, at, name, detail)| json!({
                "server": s, "player": p, "name": name, "matched": n, "outcome": outcome, "at": at, "detail": detail,
            })).collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::cosine;

    #[test]
    fn play_times_are_alike_by_hour_of_the_week() {
        let mut evenings = [0f64; 168];
        let mut mornings = [0f64; 168];
        for day in 0..7 {
            evenings[day * 24 + 20] = 3600.0;
            evenings[day * 24 + 21] = 3600.0;
            mornings[day * 24 + 8] = 3600.0;
        }
        assert!(cosine(&evenings, &evenings).unwrap() > 0.99);
        assert!(cosine(&evenings, &mornings).unwrap() < 0.01);
        assert_eq!(cosine(&evenings, &[0f64; 168]), None, "too little play to compare");
    }
}
