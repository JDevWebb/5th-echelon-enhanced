//! Global stats: the stat writes waiting for the coordinator (`stats_outbox`), and what
//! the coordinator last said about the network's leaderboards and players' stats
//! (`global_leaderboards`, `global_ranks`, `global_stats`), kept so the game is answered
//! without waiting on it.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

use super::run;
use super::Result;
use super::Storage;

/// A stat write on its way to the coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct OutboxWrite {
    pub id: i64,
    pub global_id: String,
    pub name: String,
    pub board: u32,
    pub context: u32,
    pub stat: u32,
    pub value: f64,
}

/// A place on a global leaderboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalPlace {
    pub global_id: String,
    #[serde(default)]
    pub name: String,
    pub rank: u32,
    pub value: f64,
    /// Their stats on the leaderboard's board and context.
    #[serde(default)]
    pub stats: Vec<(u32, f64)>,
}

/// A global leaderboard's top places.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GlobalList {
    pub leaderboard: u32,
    pub context: u32,
    pub total: u32,
    pub top: Vec<GlobalPlace>,
}

/// A player's place on one global leaderboard.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GlobalRank {
    pub leaderboard: u32,
    pub context: u32,
    #[serde(flatten)]
    pub place: GlobalPlace,
}

/// One stored stat of a player across the network.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GlobalStat {
    pub global_id: String,
    pub board: u32,
    pub context: u32,
    pub stat: u32,
    pub value: f64,
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

impl Storage {
    /// This database's stats epoch: with the outbox's ids, it tells the coordinator which
    /// writes it already has.
    pub async fn stats_epoch(&self) -> Result<String> {
        Ok(sqlx::query_scalar("SELECT epoch FROM stats_epoch LIMIT 1").fetch_one(&self.pool).await?)
    }

    /// The oldest `limit` writes waiting for the coordinator.
    pub async fn stats_outbox_peek(&self, limit: u32) -> Result<Vec<OutboxWrite>> {
        Ok(
            sqlx::query_as("SELECT id, global_id, name, board, context, stat, value FROM stats_outbox ORDER BY id LIMIT ?")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// The coordinator has the writes up to `last_id`.
    pub async fn stats_outbox_remove_upto(&self, last_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM stats_outbox WHERE id <= ?").bind(last_id).execute(&self.pool).await?;
        Ok(())
    }

    /// Replaces the global leaderboards' top places with the coordinator's.
    pub async fn replace_global_leaderboards(&self, lists: &[GlobalList]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM global_leaderboards").execute(&mut *tx).await?;
        for list in lists {
            sqlx::query("INSERT OR REPLACE INTO global_leaderboards (leaderboard, context, total, top) VALUES (?, ?, ?, ?)")
                .bind(list.leaderboard)
                .bind(list.context)
                .bind(list.total)
                .bind(serde_json::to_string(&list.top)?)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Replaces what's known of `players`' places with the coordinator's `ranks`.
    pub async fn replace_global_ranks(&self, players: &[String], ranks: &[GlobalRank]) -> Result<()> {
        if players.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        let sql = format!("DELETE FROM global_ranks WHERE global_id IN ({})", placeholders(players.len()));
        let mut delete = sqlx::query(&sql);
        for p in players {
            delete = delete.bind(p);
        }
        delete.execute(&mut *tx).await?;
        for r in ranks.iter().filter(|r| players.contains(&r.place.global_id)) {
            sqlx::query("INSERT OR REPLACE INTO global_ranks (global_id, leaderboard, context, name, rank, value, stats) VALUES (?, ?, ?, ?, ?, ?, ?)")
                .bind(&r.place.global_id)
                .bind(r.leaderboard)
                .bind(r.context)
                .bind(&r.place.name)
                .bind(r.place.rank)
                .bind(r.place.value)
                .bind(serde_json::to_string(&r.place.stats)?)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Replaces what's known of `players`' stats across the network with the
    /// coordinator's, except for players with writes it doesn't have yet (their stats
    /// here are ahead of its).
    pub async fn replace_global_stats(&self, players: &[String], stats: &[GlobalStat]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for p in players {
            let waiting: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stats_outbox WHERE global_id = ?")
                .bind(p)
                .fetch_one(&mut *tx)
                .await?;
            if waiting > 0 {
                continue;
            }
            sqlx::query("DELETE FROM global_stats WHERE global_id = ?").bind(p).execute(&mut *tx).await?;
            for s in stats.iter().filter(|s| &s.global_id == p) {
                sqlx::query("INSERT OR REPLACE INTO global_stats (global_id, board, context, stat, value) VALUES (?, ?, ?, ?, ?)")
                    .bind(p)
                    .bind(s.board)
                    .bind(s.context)
                    .bind(s.stat)
                    .bind(s.value)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Whether the coordinator's leaderboards are here: then the game gets those.
    pub fn has_global_leaderboards(&self) -> Result<bool> {
        let n: i64 = run(sqlx::query_scalar("SELECT COUNT(*) FROM global_leaderboards").fetch_one(&self.pool))??;
        Ok(n > 0)
    }

    /// A global leaderboard's player count and top places, if the coordinator sent it.
    pub fn global_top(&self, leaderboard: u32, context: u32) -> Result<Option<(u32, Vec<GlobalPlace>)>> {
        let row: Option<(u32, String)> = run(sqlx::query_as("SELECT total, top FROM global_leaderboards WHERE leaderboard = ? AND context = ?")
            .bind(leaderboard)
            .bind(context)
            .fetch_optional(&self.pool))??;
        Ok(row.map(|(total, top)| (total, serde_json::from_str(&top).unwrap_or_default())))
    }

    /// The places of `players` (global ids) on a global leaderboard, best first.
    pub fn global_ranks_of(&self, players: &[String], leaderboard: u32, context: u32) -> Result<Vec<GlobalPlace>> {
        if players.is_empty() {
            return Ok(vec![]);
        }
        let sql = format!(
            "SELECT global_id, name, rank, value, stats FROM global_ranks WHERE leaderboard = ? AND context = ? AND global_id IN ({}) ORDER BY rank",
            placeholders(players.len())
        );
        let mut query = sqlx::query_as::<_, (String, String, u32, f64, String)>(&sql).bind(leaderboard).bind(context);
        for p in players {
            query = query.bind(p);
        }
        Ok(run(query.fetch_all(&self.pool))??
            .into_iter()
            .map(|(global_id, name, rank, value, stats)| GlobalPlace {
                global_id,
                name,
                rank,
                value,
                stats: serde_json::from_str(&stats).unwrap_or_default(),
            })
            .collect())
    }

    /// `player`'s stats across the network on a board and context, if the coordinator
    /// sent their stats (`None`: it hasn't; the stats here are all there is).
    pub fn global_stats_of(&self, player: &str, board: u32, context: u32) -> Result<Option<Vec<(u32, f64)>>> {
        run(async {
            let known: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM global_stats WHERE global_id = ?")
                .bind(player)
                .fetch_one(&self.pool)
                .await?;
            if known == 0 {
                return Ok(None);
            }
            let rows: Vec<(u32, f64)> = sqlx::query_as("SELECT stat, value FROM global_stats WHERE global_id = ? AND board = ? AND context = ? ORDER BY stat")
                .bind(player)
                .bind(board)
                .bind(context)
                .fetch_all(&self.pool)
                .await?;
            Ok::<_, sqlx::Error>(Some(rows))
        })?
        .map_err(Into::into)
    }

    /// The global ids and names of accounts here, by account id.
    pub fn global_ids_of(&self, players: &[u32]) -> Result<HashMap<u32, (String, String)>> {
        if players.is_empty() {
            return Ok(HashMap::new());
        }
        let sql = format!(
            "SELECT id, global_id, username FROM users WHERE global_id IS NOT NULL AND id IN ({})",
            placeholders(players.len())
        );
        let mut query = sqlx::query_as::<_, (u32, String, String)>(&sql);
        for p in players {
            query = query.bind(p);
        }
        Ok(run(query.fetch_all(&self.pool))??.into_iter().map(|(id, g, name)| (id, (g, name))).collect())
    }

    /// The accounts here of people with these global ids.
    pub fn accounts_of(&self, global_ids: &[String]) -> Result<HashMap<String, u32>> {
        if global_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let sql = format!("SELECT global_id, id FROM users WHERE global_id IN ({})", placeholders(global_ids.len()));
        let mut query = sqlx::query_as::<_, (String, u32)>(&sql);
        for g in global_ids {
            query = query.bind(g);
        }
        Ok(run(query.fetch_all(&self.pool))??.into_iter().collect())
    }

    /// Global ids worth keeping places and stats for: the players online here and their
    /// friends here.
    pub async fn global_ids_to_follow(&self, limit: u32) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT u.global_id FROM users u
             WHERE u.global_id IS NOT NULL AND (u.is_online = 1 OR u.id IN (
                 SELECT r.other_id FROM relationships r JOIN users o ON o.id = r.user_id WHERE o.is_online = 1 AND r.kind = 'friend'))
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// A player's global id and the friends here they have one for.
    pub async fn global_ids_around(&self, user_id: u32, limit: u32) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT u.global_id FROM users u
             WHERE u.global_id IS NOT NULL AND (u.id = ? OR u.id IN (SELECT r.other_id FROM relationships r WHERE r.user_id = ? AND r.kind = 'friend'))
             LIMIT ?",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::run;
    use crate::storage::tests::temp_storage;
    use crate::storage::StatWrite;

    fn player(storage: &Storage, name: &str, global_id: Option<&str>) -> u32 {
        storage.register_user(name, "pw", Some(&format!("{name}-UBI"))).unwrap();
        let id = storage.find_user_id_by_name(name).unwrap().unwrap();
        if let Some(g) = global_id {
            run(sqlx::query("UPDATE users SET global_id = ? WHERE id = ?").bind(g).bind(id).execute(&storage.pool))
                .unwrap()
                .unwrap();
        }
        id
    }

    fn kills(value: f64) -> StatWrite {
        StatWrite {
            board: 17,
            context: 1,
            stat: 100,
            value,
        }
    }

    #[test]
    fn writes_wait_for_the_coordinator_and_keep_the_network_stats_current() {
        let (storage, dir) = temp_storage("global-stats");
        let linked = player(&storage, "Linked", Some("GLOBALA"));
        let local = player(&storage, "Local", None);

        storage.write_stats(linked, &[kills(3.0)]).unwrap();
        storage.write_stats(local, &[kills(9.0)]).unwrap();
        let waiting = run(storage.stats_outbox_peek(10)).unwrap().unwrap();
        assert_eq!(waiting.len(), 1, "only a player with an identity goes to the coordinator");
        assert_eq!((waiting[0].global_id.as_str(), waiting[0].name.as_str(), waiting[0].value), ("GLOBALA", "Linked", 3.0));
        assert_eq!(run(storage.stats_epoch()).unwrap().unwrap().len(), 16);

        // Until the coordinator has the write, its older figures don't replace the ones here.
        let from_coordinator = vec![GlobalStat {
            global_id: "GLOBALA".into(),
            board: 17,
            context: 1,
            stat: 100,
            value: 40.0,
        }];
        run(storage.replace_global_stats(&["GLOBALA".into()], &from_coordinator)).unwrap().unwrap();
        assert_eq!(storage.global_stats_of("GLOBALA", 17, 1).unwrap(), None);
        run(storage.stats_outbox_remove_upto(waiting[0].id)).unwrap().unwrap();
        run(storage.replace_global_stats(&["GLOBALA".into()], &from_coordinator)).unwrap().unwrap();
        assert_eq!(storage.global_stats_of("GLOBALA", 17, 1).unwrap(), Some(vec![(100, 40.0)]));
        // A new write lands on the network's figure too.
        storage.write_stats(linked, &[kills(2.0)]).unwrap();
        assert_eq!(storage.global_stats_of("GLOBALA", 17, 1).unwrap(), Some(vec![(100, 42.0)]));
        assert_eq!(storage.global_stats_of("GLOBALA", 17, 2).unwrap(), Some(vec![]), "known, nothing there");
        assert_eq!(storage.global_stats_of("NOBODY", 17, 1).unwrap(), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn leaderboards_and_places_from_the_coordinator() {
        let (storage, dir) = temp_storage("global-boards");
        let me = player(&storage, "Me", Some("GLOBALME"));
        assert!(!storage.has_global_leaderboards().unwrap());
        let place = |g: &str, rank, value| GlobalPlace {
            global_id: g.into(),
            name: g.to_lowercase(),
            rank,
            value,
            stats: vec![(100, value)],
        };
        let lists = vec![GlobalList {
            leaderboard: 10,
            context: 1,
            total: 250,
            top: vec![place("TOP1", 1, 90.0), place("TOP2", 2, 80.0)],
        }];
        run(storage.replace_global_leaderboards(&lists)).unwrap().unwrap();
        assert!(storage.has_global_leaderboards().unwrap());
        let (total, top) = storage.global_top(10, 1).unwrap().unwrap();
        assert_eq!((total, top.len(), top[0].stats.clone()), (250, 2, vec![(100, 90.0)]));
        assert_eq!(storage.global_top(10, 2).unwrap(), None);

        let ranks = vec![GlobalRank {
            leaderboard: 10,
            context: 1,
            place: place("GLOBALME", 120, 3.0),
        }];
        run(storage.replace_global_ranks(&["GLOBALME".into()], &ranks)).unwrap().unwrap();
        assert_eq!(storage.global_ranks_of(&["GLOBALME".into()], 10, 1).unwrap()[0].rank, 120);
        assert_eq!(storage.global_ids_of(&[me]).unwrap()[&me], ("GLOBALME".to_string(), "Me".to_string()));
        assert_eq!(storage.accounts_of(&["GLOBALME".into(), "TOP1".into()]).unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_coordinators_answers_read() {
        let rank: GlobalRank = serde_json::from_str(r#"{"global_id":"G","name":"Exo","leaderboard":7,"context":1,"rank":3,"value":12.0,"stats":[[122,12.0]]}"#).unwrap();
        assert_eq!((rank.leaderboard, rank.place.rank, rank.place.stats.clone()), (7, 3, vec![(122, 12.0)]));
        let list: GlobalList = serde_json::from_str(r#"{"leaderboard":7,"context":1,"total":1,"top":[{"global_id":"G","name":"Exo","rank":1,"value":12.0,"stats":[]}]}"#).unwrap();
        assert_eq!(list.top[0].name, "Exo");
    }
}
