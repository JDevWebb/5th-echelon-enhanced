//! Player stats (the `player_stats` table): what the game writes, added up as each
//! board says ([`crate::stat_boards`]), and leaderboards ranked from it.

use super::packed_date;
use super::run;
use super::Result;
use super::Storage;
use crate::stat_boards;
use crate::stat_boards::Aggregation;

/// One stat the game wrote, already checked against its board ([`stat_boards`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatWrite {
    pub board: u32,
    pub context: u32,
    pub stat: u32,
    pub value: f64,
}

/// A player's stats on one board and context.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredStats {
    pub user_id: u32,
    pub name: String,
    /// When they last changed, packed as Quazal dates are.
    pub submitted: u64,
    /// As stored, ratio stats left out.
    pub stats: Vec<(u32, f64)>,
}

/// A player's place on a leaderboard.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub user_id: u32,
    pub name: String,
    /// From 1.
    pub rank: u32,
    pub value: f64,
}

impl Storage {
    /// Adds the game's writes to `user_id`'s stats, each as its board says. Ratio stats
    /// aren't stored; they are worked out when read.
    pub fn write_stats(&self, user_id: u32, writes: &[StatWrite]) -> Result<()> {
        run(async {
            let mut transaction = self.pool.begin().await?;
            for w in writes {
                let Some(aggregation) = stat_boards::board(w.board).and_then(|b| b.aggregation(w.stat)) else {
                    continue;
                };
                if matches!(aggregation, Aggregation::Ratio(..)) {
                    continue;
                }
                let stored: Option<f64> = sqlx::query_scalar("SELECT value FROM player_stats WHERE user_id = ? AND board_id = ? AND context_id = ? AND stat_id = ?")
                    .bind(user_id)
                    .bind(w.board)
                    .bind(w.context)
                    .bind(w.stat)
                    .fetch_optional(&mut *transaction)
                    .await?;
                let value = stat_boards::aggregate(aggregation, stored, w.value);
                sqlx::query(
                    "INSERT INTO player_stats (user_id, board_id, context_id, stat_id, value) VALUES (?, ?, ?, ?, ?)
                     ON CONFLICT (user_id, board_id, context_id, stat_id) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP",
                )
                .bind(user_id)
                .bind(w.board)
                .bind(w.context)
                .bind(w.stat)
                .bind(value)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
            Ok::<_, sqlx::Error>(())
        })??;
        Ok(())
    }

    /// The stats of each of `players` that has any on `board` and `context`, in the
    /// order asked for.
    pub fn stats_of(&self, players: &[u32], board: u32, context: u32) -> Result<Vec<StoredStats>> {
        if players.is_empty() {
            return Ok(vec![]);
        }
        let sql = format!(
            "SELECT s.user_id, u.username, s.stat_id, s.value, {submitted}
             FROM player_stats s JOIN users u ON u.id = s.user_id
             WHERE s.board_id = ? AND s.context_id = ? AND s.user_id IN ({players})
             ORDER BY s.user_id, s.stat_id",
            submitted = packed_date("s.updated_at"),
            players = vec!["?"; players.len()].join(","),
        );
        let mut query = sqlx::query_as::<_, (u32, String, u32, f64, i64)>(&sql).bind(board).bind(context);
        for p in players {
            query = query.bind(p);
        }
        let rows = run(query.fetch_all(&self.pool))??;
        let mut found: Vec<StoredStats> = Vec::new();
        for (user_id, name, stat, value, submitted) in rows {
            let submitted = u64::try_from(submitted).unwrap_or_default();
            match found.last_mut() {
                Some(s) if s.user_id == user_id => {
                    s.stats.push((stat, value));
                    s.submitted = s.submitted.max(submitted);
                }
                _ => found.push(StoredStats {
                    user_id,
                    name,
                    submitted,
                    stats: vec![(stat, value)],
                }),
            }
        }
        let mut ordered = Vec::with_capacity(found.len());
        for p in players {
            if let Some(i) = found.iter().position(|s| s.user_id == *p) {
                ordered.push(found.swap_remove(i));
            }
        }
        Ok(ordered)
    }

    /// The SQL ranking everyone with the leaderboard's stat: best first, and between
    /// equals whoever got there first.
    fn ranking(leaderboard: &stat_boards::Leaderboard) -> String {
        let order = if leaderboard.descending { "DESC" } else { "ASC" };
        format!(
            "SELECT s.user_id, u.username, ROW_NUMBER() OVER (ORDER BY s.value {order}, s.updated_at, s.user_id) AS rank, s.value
             FROM player_stats s JOIN users u ON u.id = s.user_id
             WHERE s.board_id = ? AND s.context_id = ? AND s.stat_id = ?"
        )
    }

    /// How many players a leaderboard ranks.
    pub fn leaderboard_size(&self, leaderboard: &stat_boards::Leaderboard, context: u32) -> Result<u32> {
        let count: i64 = run(
            sqlx::query_scalar("SELECT COUNT(*) FROM player_stats WHERE board_id = ? AND context_id = ? AND stat_id = ?")
                .bind(leaderboard.board)
                .bind(context)
                .bind(leaderboard.stat)
                .fetch_one(&self.pool),
        )??;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// Up to `count` places of a leaderboard from rank `first` (1 is the top).
    pub fn leaderboard_from(&self, leaderboard: &stat_boards::Leaderboard, context: u32, first: u32, count: u32) -> Result<Vec<Ranked>> {
        let sql = format!("SELECT * FROM ({}) WHERE rank >= ? ORDER BY rank LIMIT ?", Self::ranking(leaderboard));
        let rows: Vec<(u32, String, u32, f64)> = run(sqlx::query_as(&sql)
            .bind(leaderboard.board)
            .bind(context)
            .bind(leaderboard.stat)
            .bind(first.max(1))
            .bind(count)
            .fetch_all(&self.pool))??;
        Ok(rows.into_iter().map(ranked).collect())
    }

    /// The places of `players` on a leaderboard, best first; players it doesn't rank are
    /// left out.
    pub fn leaderboard_of(&self, leaderboard: &stat_boards::Leaderboard, context: u32, players: &[u32]) -> Result<Vec<Ranked>> {
        if players.is_empty() {
            return Ok(vec![]);
        }
        let sql = format!(
            "SELECT * FROM ({}) WHERE user_id IN ({}) ORDER BY rank",
            Self::ranking(leaderboard),
            vec!["?"; players.len()].join(",")
        );
        let mut query = sqlx::query_as::<_, (u32, String, u32, f64)>(&sql)
            .bind(leaderboard.board)
            .bind(context)
            .bind(leaderboard.stat);
        for p in players {
            query = query.bind(p);
        }
        Ok(run(query.fetch_all(&self.pool))??.into_iter().map(ranked).collect())
    }

    /// Up to `count` places of a leaderboard around `player`'s; the top of it when they
    /// aren't on it.
    pub fn leaderboard_around(&self, leaderboard: &stat_boards::Leaderboard, context: u32, player: u32, count: u32) -> Result<Vec<Ranked>> {
        let rank = self.leaderboard_of(leaderboard, context, &[player])?.first().map_or(1, |r| r.rank);
        self.leaderboard_from(leaderboard, context, rank.saturating_sub(count / 2).max(1), count)
    }
}

fn ranked((user_id, name, rank, value): (u32, String, u32, f64)) -> Ranked {
    Ranked { user_id, name, rank, value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::temp_storage;

    fn write(board: u32, context: u32, stat: u32, value: f64) -> StatWrite {
        StatWrite { board, context, stat, value }
    }

    #[test]
    fn stats_add_up_and_rank() {
        let (storage, dir) = temp_storage("stats");
        let player = |name: &str| {
            storage.register_user(name, "pw", None).unwrap();
            storage.find_user_id_by_name(name).unwrap().unwrap()
        };
        let (a, b, c) = (player("StatsA"), player("StatsB"), player("StatsC"));

        // Ladder 1: kills add up, a ratio stat isn't stored.
        storage.write_stats(a, &[write(17, 1, 100, 3.0), write(10, 228, 102, 9.0)]).unwrap();
        storage.write_stats(a, &[write(17, 1, 100, 4.0)]).unwrap();
        storage.write_stats(b, &[write(17, 1, 100, 10.0)]).unwrap();
        storage.write_stats(c, &[write(17, 1, 100, 1.0), write(17, 2, 100, 50.0)]).unwrap();
        // A best time is overwritten, and lower is better.
        storage.write_stats(a, &[write(23, 100, 154, 300.0)]).unwrap();
        storage.write_stats(a, &[write(23, 100, 154, 280.0)]).unwrap();
        storage.write_stats(b, &[write(23, 100, 154, 250.0)]).unwrap();

        let stats = storage.stats_of(&[b, a, 9999], 17, 1).unwrap();
        assert_eq!(stats.iter().map(|s| s.user_id).collect::<Vec<_>>(), [b, a], "in the order asked, those with stats");
        assert_eq!(stats[1].stats, [(100, 7.0)]);
        assert_eq!(stats[1].name, "StatsA");
        assert!(stats[1].submitted >> 26 >= 2024, "a packed date");
        assert!(storage.stats_of(&[a], 10, 228).unwrap().is_empty(), "ratios aren't stored");

        let kills = stat_boards::leaderboard(10).unwrap();
        assert_eq!(storage.leaderboard_size(kills, 1).unwrap(), 3);
        let top = storage.leaderboard_from(kills, 1, 1, 10).unwrap();
        assert_eq!(
            top.iter().map(|r| (r.user_id, r.rank, r.value)).collect::<Vec<_>>(),
            [(b, 1, 10.0), (a, 2, 7.0), (c, 3, 1.0)]
        );
        assert_eq!(storage.leaderboard_from(kills, 1, 2, 1).unwrap()[0].user_id, a);
        assert_eq!(storage.leaderboard_of(kills, 1, &[c, a]).unwrap().iter().map(|r| r.rank).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(storage.leaderboard_around(kills, 1, c, 2).unwrap().iter().map(|r| r.rank).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(storage.leaderboard_size(kills, 2).unwrap(), 1, "each ladder on its own");

        let best_time = stat_boards::leaderboard(2).unwrap();
        let times = storage.leaderboard_from(best_time, 100, 1, 10).unwrap();
        assert_eq!(times.iter().map(|r| (r.user_id, r.value)).collect::<Vec<_>>(), [(b, 250.0), (a, 280.0)]);

        std::fs::remove_dir_all(dir).unwrap();
    }
}
