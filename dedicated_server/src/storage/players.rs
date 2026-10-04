//! What an admin sees of players: play sessions (`play_sessions`), bans (`bans`) and the
//! matches each played (`match_players`), and the roster sent to the coordinator.

use serde::Serialize;

use super::run;
use super::Result;
use super::Storage;

/// SQL for the current time in Unix seconds.
const NOW: &str = "CAST(strftime('%s', 'now') AS INTEGER)";

/// A sign-in this soon after the last sign-out carries on that session (a reconnect, a
/// restarted game).
pub const RESUME_WITHIN_SECS: i64 = 120;

/// A ban on a player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Ban {
    pub reason: String,
    /// Unix seconds; `None` for good.
    pub until: Option<i64>,
    pub at: i64,
}

/// A player as the coordinator's admin UI lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlayerRecord {
    pub id: u32,
    pub name: String,
    /// Their identity's public key (the global id the heartbeat already lists for online
    /// players): the same person on every server.
    pub identity: Option<String>,
    pub created_at: i64,
    pub last_seen: Option<i64>,
    pub online: bool,
    pub play_seconds: i64,
    pub sessions: u32,
    pub matches: u32,
    pub banned: Option<Ban>,
}

/// A play session as sent to the coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct PlaySession {
    pub id: i64,
    pub player: u32,
    pub start: i64,
    /// `None` while still playing.
    pub end: Option<i64>,
}

/// A finished match, as sent to the coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedMatch {
    pub attributes: String,
    pub started: i64,
    pub ended: i64,
    pub players: u32,
}

impl Storage {
    /// `user_id` signed in to the game: a new play session, or the last one carried on
    /// when it ended moments ago. True for a new one.
    pub fn start_play(&self, user_id: u32) -> Result<bool> {
        run(async {
            let mut tx = self.pool.begin().await?;
            let open: Option<i64> = sqlx::query_scalar("SELECT id FROM play_sessions WHERE user_id = ? AND ended_at IS NULL ORDER BY id DESC LIMIT 1")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
            if open.is_some() {
                // Still open: another connection of the same game.
                tx.commit().await?;
                return Ok(false);
            }
            let resumed = sqlx::query(&format!(
                "UPDATE play_sessions SET ended_at = NULL, seen_at = {NOW}, changed_at = {NOW}
                 WHERE id = (SELECT id FROM play_sessions WHERE user_id = ? ORDER BY id DESC LIMIT 1) AND ended_at >= {NOW} - ?"
            ))
            .bind(user_id)
            .bind(RESUME_WITHIN_SECS)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                > 0;
            if !resumed {
                sqlx::query(&format!(
                    "INSERT INTO play_sessions (user_id, started_at, seen_at, changed_at) VALUES (?, {NOW}, {NOW}, {NOW})"
                ))
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            Ok::<_, sqlx::Error>(!resumed)
        })?
        .map_err(Into::into)
    }

    /// `user_id`'s game connection closed: their play session ends now.
    pub fn end_play(&self, user_id: u32) -> Result<()> {
        run(sqlx::query(&format!(
            "UPDATE play_sessions SET ended_at = {NOW}, seen_at = {NOW}, changed_at = {NOW} WHERE user_id = ? AND ended_at IS NULL"
        ))
        .bind(user_id)
        .execute(&self.pool))??;
        Ok(())
    }

    /// `user_id`'s last play session: when it started, and ended (0 while still going).
    pub fn last_play_session(&self, user_id: u32) -> Result<Option<(i64, i64)>> {
        let row: Option<(i64, Option<i64>)> = run(sqlx::query_as("SELECT started_at, ended_at FROM play_sessions WHERE user_id = ? ORDER BY id DESC LIMIT 1")
            .bind(user_id)
            .fetch_optional(&self.pool))??;
        Ok(row.map(|(start, end)| (start, end.unwrap_or(0))))
    }

    /// How many reports `user_id` sent in the last day.
    pub async fn reports_today(&self, user_id: u32) -> Result<i64> {
        Ok(sqlx::query_scalar(&format!("SELECT COUNT(*) FROM report_log WHERE user_id = ? AND at > {NOW} - 86400"))
            .bind(user_id)
            .fetch_one(&self.pool)
            .await?)
    }

    /// Queues `user_id`'s report for the coordinator, and notes they sent one.
    pub async fn queue_report(&self, user_id: u32, id: &str, body: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(&format!("INSERT INTO report_outbox (id, body, created_at) VALUES (?, ?, {NOW})"))
            .bind(id)
            .bind(body)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("INSERT INTO report_log (user_id, at) VALUES (?, {NOW})"))
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("DELETE FROM report_log WHERE at < {NOW} - 86400")).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The oldest report waiting for the coordinator; ones waiting over a week are dropped.
    pub async fn next_report(&self) -> Result<Option<(String, String)>> {
        sqlx::query(&format!("DELETE FROM report_outbox WHERE created_at < {NOW} - 7 * 86400"))
            .execute(&self.pool)
            .await?;
        Ok(sqlx::query_as("SELECT id, body FROM report_outbox ORDER BY created_at, id LIMIT 1")
            .fetch_optional(&self.pool)
            .await?)
    }

    /// The coordinator has the report.
    pub async fn report_sent(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM report_outbox WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }

    /// Every minute: the open play sessions are still going.
    pub fn touch_play(&self) -> Result<()> {
        run(sqlx::query(&format!("UPDATE play_sessions SET seen_at = {NOW} WHERE ended_at IS NULL")).execute(&self.pool))??;
        Ok(())
    }

    /// At start: sessions the server never saw end (it stopped) ended when last seen.
    pub fn close_stale_play(&self) -> Result<()> {
        run(sqlx::query(&format!("UPDATE play_sessions SET ended_at = seen_at, changed_at = {NOW} WHERE ended_at IS NULL")).execute(&self.pool))??;
        Ok(())
    }

    /// Play sessions changed at or after `since` (Unix seconds), oldest change first: up
    /// to `limit` of them, after the first `skip`.
    pub fn play_sessions_changed_since(&self, since: i64, limit: u32, skip: u32) -> Result<Vec<PlaySession>> {
        Ok(run(sqlx::query_as(
            "SELECT id, user_id AS player, started_at AS start, ended_at AS end FROM play_sessions WHERE changed_at >= ? ORDER BY changed_at, id LIMIT ? OFFSET ?",
        )
        .bind(since)
        .bind(limit)
        .bind(skip)
        .fetch_all(&self.pool))??)
    }

    /// The server's clock, in Unix seconds, as SQLite has it.
    pub fn now(&self) -> Result<i64> {
        Ok(run(sqlx::query_scalar(&format!("SELECT {NOW}")).fetch_one(&self.pool))??)
    }

    /// Players for the coordinator: all of them (`only` None), or those listed. The
    /// server's own accounts are left out.
    pub fn player_records(&self, only: Option<&[u32]>) -> Result<Vec<PlayerRecord>> {
        if only.is_some_and(<[u32]>::is_empty) {
            return Ok(vec![]);
        }
        let filter = only.map_or_else(String::new, |ids| format!("AND u.id IN ({})", vec!["?"; ids.len()].join(",")));
        let sql = format!(
            "SELECT u.id, u.username, u.global_id,
                    COALESCE(CAST(strftime('%s', u.created_at) AS INTEGER), 0),
                    COALESCE((SELECT MAX(COALESCE(p.ended_at, p.seen_at)) FROM play_sessions p WHERE p.user_id = u.id),
                             CAST(strftime('%s', u.last_login) AS INTEGER)),
                    COALESCE(u.is_online, 0),
                    COALESCE((SELECT SUM(COALESCE(p.ended_at, p.seen_at) - p.started_at) FROM play_sessions p WHERE p.user_id = u.id), 0),
                    (SELECT COUNT(*) FROM play_sessions p WHERE p.user_id = u.id),
                    u.matches_played,
                    b.reason, b.until, b.created_at
             FROM users u LEFT JOIN bans b ON b.user_id = u.id
             WHERE u.ubi_id IS NOT NULL AND u.ubi_id != '' {filter}
             ORDER BY u.id"
        );
        type Row = (u32, String, Option<String>, i64, Option<i64>, bool, i64, u32, u32, Option<String>, Option<i64>, Option<i64>);
        let mut query = sqlx::query_as::<_, Row>(&sql);
        for id in only.unwrap_or_default() {
            query = query.bind(id);
        }
        let rows = run(query.fetch_all(&self.pool))??;
        Ok(rows
            .into_iter()
            .map(
                |(id, name, global_id, created_at, last_seen, online, play_seconds, sessions, matches, reason, until, at)| PlayerRecord {
                    id,
                    name,
                    identity: global_id,
                    created_at,
                    last_seen,
                    online,
                    play_seconds: play_seconds.max(0),
                    sessions,
                    matches,
                    banned: at.map(|at| Ban {
                        reason: reason.unwrap_or_default(),
                        until,
                        at,
                    }),
                },
            )
            .collect())
    }

    /// Bans `user_id` until `until` (Unix seconds; for good when `None`), replacing any ban.
    pub async fn ban(&self, user_id: u32, reason: &str, until: Option<i64>) -> Result<()> {
        sqlx::query(&format!(
            "INSERT INTO bans (user_id, reason, until, created_at) VALUES (?, ?, ?, {NOW})
             ON CONFLICT (user_id) DO UPDATE SET reason = excluded.reason, until = excluded.until, created_at = excluded.created_at"
        ))
        .bind(user_id)
        .bind(reason)
        .bind(until)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lifts a ban. True when there was one.
    pub async fn unban(&self, user_id: u32) -> Result<bool> {
        Ok(sqlx::query("DELETE FROM bans WHERE user_id = ?").bind(user_id).execute(&self.pool).await?.rows_affected() > 0)
    }

    /// The ban keeping `user_id` out now, if any; one that ran out is lifted.
    pub async fn active_ban(&self, user_id: u32) -> Result<Option<Ban>> {
        sqlx::query(&format!("DELETE FROM bans WHERE user_id = ? AND until IS NOT NULL AND until <= {NOW}"))
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(sqlx::query_as("SELECT reason, until, created_at AS at FROM bans WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// `user_id` is in match `game_id` (a session of room kind 0). Counts the match for
    /// each player once a second one is in it. True when that made it a match.
    pub fn note_match_player(&self, game_id: u32, user_id: u32) -> Result<bool> {
        run(async {
            let mut tx = self.pool.begin().await?;
            let is_match: Option<String> = sqlx::query_scalar("SELECT attributes FROM game_sessions WHERE id = ? AND destroyed_at IS NULL")
                .bind(game_id)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
            if !is_match.as_deref().is_some_and(|a| crate::game_session::match_mode(a).is_some()) {
                return Ok(false);
            }
            let added = sqlx::query("INSERT OR IGNORE INTO match_players (game_id, user_id) VALUES (?, ?)")
                .bind(game_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                > 0;
            if !added {
                return Ok(false);
            }
            let players: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM match_players WHERE game_id = ?")
                .bind(game_id)
                .fetch_one(&mut *tx)
                .await?;
            sqlx::query("UPDATE game_sessions SET peak_players = ? WHERE id = ?")
                .bind(players)
                .bind(game_id)
                .execute(&mut *tx)
                .await?;
            let counted = match players {
                2 => "UPDATE users SET matches_played = matches_played + 1 WHERE id IN (SELECT user_id FROM match_players WHERE game_id = ?)",
                n if n > 2 => "UPDATE users SET matches_played = matches_played + 1 WHERE id = ?",
                _ => "",
            };
            if !counted.is_empty() {
                let id = if players == 2 { game_id } else { user_id };
                sqlx::query(counted).bind(id).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            Ok::<_, sqlx::Error>(players == 2)
        })?
        .map_err(Into::into)
    }

    /// Matches that ended with at least two players and haven't been reported, marked
    /// reported.
    pub async fn take_finished_matches_async(&self) -> Result<Vec<FinishedMatch>> {
        let mut tx = self.pool.begin().await?;
        let rows: Vec<(Option<String>, Option<i64>, Option<i64>, u32)> = sqlx::query_as(
            "SELECT attributes, CAST(strftime('%s', created_at) AS INTEGER), CAST(strftime('%s', destroyed_at) AS INTEGER), peak_players
             FROM game_sessions WHERE destroyed_at IS NOT NULL AND reported = 0 AND peak_players >= 2",
        )
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query("UPDATE game_sessions SET reported = 1 WHERE destroyed_at IS NOT NULL AND reported = 0")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|(attributes, started, ended, players)| FinishedMatch {
                attributes: attributes.unwrap_or_default(),
                started: started.unwrap_or_default(),
                ended: ended.unwrap_or_default(),
                players,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::tests::temp_storage;

    fn player(storage: &crate::storage::Storage, name: &str) -> u32 {
        storage.register_user(name, "pw", Some(&format!("{name}-UBI"))).unwrap();
        storage.find_user_id_by_name(name).unwrap().unwrap()
    }

    #[test]
    fn play_sessions_and_bans() {
        let (storage, dir) = temp_storage("players");
        let a = player(&storage, "PlayA");

        assert!(storage.start_play(a).unwrap(), "a new session");
        assert!(!storage.start_play(a).unwrap(), "another connection of the same game");
        storage.end_play(a).unwrap();
        assert!(!storage.start_play(a).unwrap(), "back within two minutes: the same session");
        let sessions = storage.play_sessions_changed_since(0, 10, 0).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].end, None);

        // A restart ends it where it was last seen.
        storage.close_stale_play().unwrap();
        assert!(storage.play_sessions_changed_since(0, 10, 0).unwrap()[0].end.is_some());

        let record = storage.player_records(Some(&[a])).unwrap().pop().unwrap();
        assert_eq!((record.name.as_str(), record.sessions, record.banned.clone()), ("PlayA", 1, None));
        assert!(record.last_seen.is_some());
        assert!(
            storage.player_records(None).unwrap().iter().all(|p| p.id != 1 && p.id != 105),
            "not the server's own accounts"
        );

        use crate::storage::run;
        run(storage.ban(a, "cheating", None)).unwrap().unwrap();
        let ban = run(storage.active_ban(a)).unwrap().unwrap().unwrap();
        assert_eq!(ban.reason, "cheating");
        assert!(storage.player_records(Some(&[a])).unwrap()[0].banned.is_some());
        assert!(matches!(storage.login_user("PlayA", "pw").unwrap(), Err(crate::storage::LoginError::Banned(_))));
        assert!(run(storage.unban(a)).unwrap().unwrap());
        assert!(storage.login_user("PlayA", "pw").unwrap().is_ok());
        // A ban that ran out is lifted.
        run(storage.ban(a, "a day", Some(1))).unwrap().unwrap();
        assert_eq!(run(storage.active_ban(a)).unwrap().unwrap(), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_public_sessions_open_their_host_to_probes() {
        let (storage, dir) = temp_storage("public-host");
        let (host, other) = (player(&storage, "HostP"), player(&storage, "OtherP"));
        let coop = "113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 2;4 => 0;101 => 3578398534;102 => 3;103 => 0;105 => 2;112 => 2";
        let private = "113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 0;4 => 2;101 => 3578398534;102 => 3;103 => 0;105 => 2;112 => 2";

        let private_room = storage.create_game_session(host, 1, private.into()).unwrap();
        storage.add_participants(1, private_room, vec![], vec![host]).unwrap();
        assert!(!storage.hosts_public_session(host).unwrap(), "private seats only");

        let public = storage.create_game_session(host, 1, coop.into()).unwrap();
        assert!(!storage.hosts_public_session(host).unwrap(), "not in it yet");
        storage.add_participants(1, public, vec![], vec![host]).unwrap();
        assert!(storage.hosts_public_session(host).unwrap());
        assert!(!storage.hosts_public_session(other).unwrap());

        // Announced invite-only: not public, whatever its seats.
        crate::storage::run(storage.set_advertised_session_async(host, Some(public), true, &[])).unwrap().unwrap();
        assert!(!storage.hosts_public_session(host).unwrap());
        crate::storage::run(storage.set_advertised_session_async(host, Some(public), false, &[])).unwrap().unwrap();
        assert!(storage.hosts_public_session(host).unwrap());
        storage.delete_game_session(host, 1, public).unwrap();
        assert!(!storage.hosts_public_session(host).unwrap(), "ended");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn matches_count_once_a_second_player_is_in() {
        let (storage, dir) = temp_storage("matches");
        let (a, b, c) = (player(&storage, "MatchA"), player(&storage, "MatchB"), player(&storage, "MatchC"));
        let svm = "113 => 0;109 => 0;110 => 0;106 => 3564829;107 => 3909881133;108 => 0;3 => 4;4 => 0;101 => 72621668;102 => 8;103 => 2165463540;105 => 0;112 => 2";
        let game = storage.create_game_session(a, 1, svm.into()).unwrap();
        let lobby = storage.create_game_session(a, 1, "113 => 1;3 => 8;4 => 0".into()).unwrap();

        assert!(!storage.note_match_player(lobby, b).unwrap(), "a lobby isn't a match");
        assert!(!storage.note_match_player(game, a).unwrap());
        assert!(storage.note_match_player(game, b).unwrap(), "two players: a match");
        assert!(!storage.note_match_player(game, b).unwrap(), "once");
        storage.note_match_player(game, c).unwrap();
        let matches = |id| storage.player_records(Some(&[id])).unwrap()[0].matches;
        assert_eq!((matches(a), matches(b), matches(c)), (1, 1, 1));

        assert!(crate::storage::run(storage.take_finished_matches_async()).unwrap().unwrap().is_empty(), "still going");
        storage.delete_game_session(a, 1, game).unwrap();
        let finished = crate::storage::run(storage.take_finished_matches_async()).unwrap().unwrap();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].players, 3);
        assert!(crate::storage::run(storage.take_finished_matches_async()).unwrap().unwrap().is_empty(), "reported once");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
