//! Another server's accounts merged into this one's database (global matchmaking's move, the
//! 0.5.0 plan's G7): every player account of the other server, by its identity. One already
//! here (the same person on both servers) keeps its account here and gains the other's
//! friends, blocks, bans, content and stats; one that isn't is added, with its name, password
//! hash and identity, under a new id (and a new account id where this server has its old one).
//!
//! Friends, requests and blocks move both ways, a block over a friendship over a request; a ban
//! keeps the longer; uploaded content the newer; per-server stats add up as their board says
//! (stat_boards). What lasts only a session (invitations, rooms, friend events) and play
//! history (sent to the coordinator already) stay behind. Run on a stopped server, once per
//! other database; nothing is changed unless all of it goes in.

use std::collections::HashMap;
use std::path::Path;

use sqlx::Row as _;

use super::Result;
use super::Storage;

/// What a merge did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Merged {
    /// Accounts added, and accounts of people already here that took the other's in.
    pub added: u32,
    pub joined: u32,
    /// Accounts added under another name (theirs was another person's here).
    pub renamed: Vec<(String, String)>,
    pub relationships: u32,
    pub bans: u32,
    pub content: u32,
    pub stats: u32,
}

/// How strongly a relationship holds: what's kept when both servers had one.
fn rank(kind: &str) -> u8 {
    match kind {
        "block" => 3,
        "friend" => 2,
        _ => 1,
    }
}

impl Storage {
    /// Merges the player accounts of the server database at `from` into this one.
    pub async fn merge_accounts(&self, from: &Path) -> Result<Merged> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("ATTACH DATABASE ? AS src")
            .bind(from.to_string_lossy().into_owned())
            .execute(&mut *conn)
            .await?;
        let result = merge_attached(&mut conn).await;
        let _ = sqlx::query("DETACH DATABASE src").execute(&mut *conn).await;
        result
    }
}

async fn merge_attached(conn: &mut sqlx::SqliteConnection) -> Result<Merged> {
    use sqlx::Connection as _;
    // The same schema: a database a release behind or ahead is migrated by its own server first.
    let mine: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM main._sqlx_migrations WHERE success")
        .fetch_one(&mut *conn)
        .await?;
    let theirs: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM src._sqlx_migrations WHERE success")
        .fetch_one(&mut *conn)
        .await?;
    if mine != theirs {
        return Err(eyre::eyre!(
            "the other database's schema ({theirs:?}) isn't this one's ({mine:?}): start its server on this release once first"
        ));
    }
    // What it hadn't sent the coordinator yet would be lost.
    for outbox in ["stats_outbox", "report_outbox", "federation_outbox"] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM src.{outbox}")).fetch_one(&mut *conn).await?;
        if n > 0 {
            return Err(eyre::eyre!(
                "the other server hasn't sent everything to the coordinator yet ({n} in {outbox}): let it run until it has, then stop it"
            ));
        }
    }
    let mut tx = conn.begin().await?;
    let mut done = Merged::default();
    // The columns both have, but the id, whether it's online and what names it (set apart).
    let columns: Vec<String> =
        sqlx::query("SELECT name FROM pragma_table_info('users', 'src') WHERE name NOT IN ('id', 'is_online', 'username', 'name_key', 'ubi_id', 'name_conflict')")
            .fetch_all(&mut *tx)
            .await?
            .iter()
            .map(|r| r.get::<String, _>(0))
            .collect();
    // Players only: an account a game signs in to (the server's own sample accounts left out).
    let players = sqlx::query(
        "SELECT id, username, ubi_id, global_id, COALESCE(matches_played, 0), last_login, created_at, name_not_allowed FROM src.users
         WHERE ubi_id IS NOT NULL AND (password IS NOT NULL OR password_hash IS NOT NULL)",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut ids: HashMap<i64, i64> = HashMap::new();
    for row in &players {
        let (src_id, name, ubi_id, global_id): (i64, String, String, Option<String>) = (row.get(0), row.get(1), row.get(2), row.get(3));
        let here: Option<i64> = match &global_id {
            Some(g) => sqlx::query_scalar("SELECT id FROM main.users WHERE global_id = ?").bind(g).fetch_optional(&mut *tx).await?,
            None => None,
        };
        if let Some(dst) = here {
            // The same person: their account here takes the other's in.
            sqlx::query(
                "UPDATE main.users SET matches_played = COALESCE(matches_played, 0) + ?,
                   last_login = MAX(COALESCE(last_login, ''), COALESCE(?, '')),
                   created_at = MIN(created_at, COALESCE(?, created_at)),
                   name_not_allowed = MAX(name_not_allowed, ?)
                 WHERE id = ?",
            )
            .bind(row.get::<i64, _>(4))
            .bind(row.get::<Option<String>, _>(5))
            .bind(row.get::<Option<String>, _>(6))
            .bind(row.get::<i64, _>(7))
            .bind(dst)
            .execute(&mut *tx)
            .await?;
            ids.insert(src_id, dst);
            done.joined += 1;
            continue;
        }
        // Someone new here: their name, unless another person has it here.
        let mut given = name.clone();
        let mut n = 2;
        while sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM main.users WHERE username = ? OR name_key = ?")
            .bind(&given)
            .bind(super::name_key(&given))
            .fetch_one(&mut *tx)
            .await?
            > 0
        {
            given = format!("{name}{n}");
            n += 1;
        }
        let account = if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM main.users WHERE ubi_id = ?")
            .bind(&ubi_id)
            .fetch_one(&mut *tx)
            .await?
            > 0
        {
            let mut bytes = [0u8; 4];
            rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
            format!("{given}.{}", identity::base32_encode(&bytes).to_lowercase())
        } else {
            ubi_id.clone()
        };
        let list = columns.join(", ");
        let sql = format!("INSERT INTO main.users (username, name_key, ubi_id, name_conflict, {list}) SELECT ?, ?, ?, ?, {list} FROM src.users WHERE id = ?");
        sqlx::query(&sql)
            .bind(&given)
            .bind(super::name_key(&given))
            .bind(&account)
            .bind(i64::from(given != name))
            .bind(src_id)
            .execute(&mut *tx)
            .await?;
        let dst: i64 = sqlx::query_scalar("SELECT last_insert_rowid()").fetch_one(&mut *tx).await?;
        if given != name {
            done.renamed.push((name, given));
        }
        ids.insert(src_id, dst);
        done.added += 1;
    }
    // Friends, requests and blocks, both ends moved.
    let rels = sqlx::query("SELECT user_id, other_id, kind, created_at FROM src.relationships").fetch_all(&mut *tx).await?;
    for r in rels {
        let (Some(u), Some(o)) = (ids.get(&r.get::<i64, _>(0)), ids.get(&r.get::<i64, _>(1))) else {
            continue;
        };
        if u == o {
            continue;
        }
        let kind: String = r.get(2);
        let existing: Option<String> = sqlx::query_scalar("SELECT kind FROM main.relationships WHERE user_id = ? AND other_id = ?")
            .bind(u)
            .bind(o)
            .fetch_optional(&mut *tx)
            .await?;
        if existing.as_deref().is_some_and(|e| rank(e) >= rank(&kind)) {
            continue;
        }
        sqlx::query(
            "INSERT INTO main.relationships (user_id, other_id, kind, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (user_id, other_id) DO UPDATE SET kind = excluded.kind",
        )
        .bind(u)
        .bind(o)
        .bind(&kind)
        .bind(r.get::<String, _>(3))
        .execute(&mut *tx)
        .await?;
        done.relationships += 1;
    }
    // Bans: the longer stands (none ending is longest).
    let bans = sqlx::query("SELECT user_id, reason, until, created_at FROM src.bans").fetch_all(&mut *tx).await?;
    for b in bans {
        let Some(u) = ids.get(&b.get::<i64, _>(0)) else { continue };
        sqlx::query(
            "INSERT INTO main.bans (user_id, reason, until, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (user_id) DO UPDATE SET
               until = CASE WHEN main.bans.until IS NULL OR excluded.until IS NULL THEN NULL ELSE MAX(main.bans.until, excluded.until) END,
               reason = CASE WHEN excluded.until IS NULL OR (main.bans.until IS NOT NULL AND excluded.until > main.bans.until) THEN excluded.reason ELSE main.bans.reason END",
        )
        .bind(u)
        .bind(b.get::<String, _>(1))
        .bind(b.get::<Option<i64>, _>(2))
        .bind(b.get::<i64, _>(3))
        .execute(&mut *tx)
        .await?;
        done.bans += 1;
    }
    // Uploaded content: the newer.
    let content = sqlx::query("SELECT user_id, type_id, size, gzip, updated_at FROM src.player_content")
        .fetch_all(&mut *tx)
        .await?;
    for c in content {
        let Some(u) = ids.get(&c.get::<i64, _>(0)) else { continue };
        sqlx::query(
            "INSERT INTO main.player_content (user_id, type_id, size, gzip, updated_at, sent) VALUES (?, ?, ?, ?, ?, 0)
             ON CONFLICT (user_id, type_id) DO UPDATE SET size = excluded.size, gzip = excluded.gzip, updated_at = excluded.updated_at, sent = 0
             WHERE excluded.updated_at > main.player_content.updated_at",
        )
        .bind(u)
        .bind(c.get::<i64, _>(1))
        .bind(c.get::<i64, _>(2))
        .bind(c.get::<Vec<u8>, _>(3))
        .bind(c.get::<i64, _>(4))
        .execute(&mut *tx)
        .await?;
        done.content += 1;
    }
    // Per-server stats, added up as each board says.
    let stats = sqlx::query("SELECT user_id, board_id, context_id, stat_id, value, updated_at FROM src.player_stats")
        .fetch_all(&mut *tx)
        .await?;
    for s in stats {
        let Some(u) = ids.get(&s.get::<i64, _>(0)) else { continue };
        let (board, context, stat, value, at): (i64, i64, i64, f64, String) = (s.get(1), s.get(2), s.get(3), s.get(4), s.get(5));
        let here: Option<(f64, String)> = sqlx::query_as("SELECT value, updated_at FROM main.player_stats WHERE user_id = ? AND board_id = ? AND context_id = ? AND stat_id = ?")
            .bind(u)
            .bind(board)
            .bind(context)
            .bind(stat)
            .fetch_optional(&mut *tx)
            .await?;
        let aggregation = u32::try_from(board)
            .ok()
            .and_then(stat_boards::board)
            .and_then(|b| b.aggregation(u32::try_from(stat).ok()?));
        let (value, at) = match (here, aggregation) {
            (None, _) => (value, at),
            (Some((mine, my_at)), Some(stat_boards::Aggregation::Add)) => (mine + value, my_at.max(at)),
            (Some((mine, my_at)), Some(stat_boards::Aggregation::Maximum)) => (mine.max(value), my_at.max(at)),
            (Some((mine, my_at)), Some(stat_boards::Aggregation::Minimum)) => (mine.min(value), my_at.max(at)),
            // Overwrite (and anything else): the newer.
            (Some((mine, my_at)), _) => {
                if at > my_at {
                    (value, at)
                } else {
                    (mine, my_at)
                }
            }
        };
        sqlx::query(
            "INSERT INTO main.player_stats (user_id, board_id, context_id, stat_id, value, updated_at) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (user_id, board_id, context_id, stat_id) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(u)
        .bind(board)
        .bind(context)
        .bind(stat)
        .bind(value)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        done.stats += 1;
    }
    tx.commit().await?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::super::run;
    use super::super::tests::temp_storage;
    use super::*;

    fn player(storage: &Storage, name: &str, identity: Option<&str>) -> i64 {
        storage.register_user(name, "pw", Some(name)).unwrap();
        let id = i64::from(storage.find_user_id_by_name(name).unwrap().unwrap());
        if let Some(g) = identity {
            run(sqlx::query("UPDATE users SET global_id = ?, matches_played = 3 WHERE id = ?")
                .bind(g)
                .bind(id)
                .execute(&storage.pool))
            .unwrap()
            .unwrap();
        }
        id
    }

    fn exec(storage: &Storage, sql: &str) {
        run(sqlx::query(sql).execute(&storage.pool)).unwrap().unwrap();
    }

    fn one<T: Send + Unpin + for<'r> sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>>(storage: &Storage, sql: &str) -> T {
        run(sqlx::query_scalar::<_, T>(sql).fetch_one(&storage.pool)).unwrap().unwrap()
    }

    /// The same person on both servers keeps one account here with both servers' friends and
    /// matches; someone new is added under their name (a number added when another person has
    /// it here); a block outranks a friendship; the longer ban and the newer content stand;
    /// stats add up as their board says; a database with unsent changes, or another schema, is
    /// refused untouched.
    #[test]
    fn another_servers_accounts_merge_in_by_identity() {
        let (mm, mm_dir) = temp_storage("merge-into");
        let (eu, eu_dir) = temp_storage("merge-from");
        let kiwi_mm = player(&mm, "Kiwi", Some("IDKIWI"));
        let other_tank = player(&mm, "Tank", Some("IDTANKMM"));
        let kiwi = player(&eu, "Kiwi", Some("IDKIWI"));
        let tank = player(&eu, "Tank", Some("IDTANKEU"));
        let raven = player(&eu, "Raven", Some("IDRAVEN"));
        // Kiwi and Raven friends on eu1; Raven blocked Tank there, and was friends with them here
        // first... (a block outranks); Raven banned for a day, then for good.
        exec(
            &eu,
            &format!("INSERT INTO relationships (user_id, other_id, kind) VALUES ({kiwi}, {raven}, 'friend'), ({raven}, {kiwi}, 'friend'), ({raven}, {tank}, 'block')"),
        );
        exec(&eu, &format!("INSERT INTO bans (user_id, reason, until, created_at) VALUES ({raven}, 'griefing', NULL, 1)"));
        exec(&eu, &format!("INSERT INTO player_stats (user_id, board_id, context_id, stat_id, value, updated_at) VALUES ({kiwi}, 10, 228, 100, 5, '2026-11-01'), ({kiwi}, 10, 228, 216, 4, '2026-11-01')"));
        exec(&mm, &format!("INSERT INTO player_stats (user_id, board_id, context_id, stat_id, value, updated_at) VALUES ({kiwi_mm}, 10, 228, 100, 7, '2026-11-02'), ({kiwi_mm}, 10, 228, 216, 9, '2026-11-02')"));
        exec(
            &eu,
            &format!("INSERT INTO player_content (user_id, type_id, size, gzip, updated_at) VALUES ({kiwi}, 1, 3, x'010203', 200)"),
        );
        exec(
            &mm,
            &format!("INSERT INTO player_content (user_id, type_id, size, gzip, updated_at) VALUES ({kiwi_mm}, 1, 1, x'09', 100)"),
        );

        let done = run(mm.merge_accounts(&eu_dir.join("test.db"))).unwrap().unwrap();
        assert_eq!((done.added, done.joined), (2, 1), "Kiwi joined; Tank (another person) and Raven added");
        assert_eq!(done.renamed, vec![("Tank".to_string(), "Tank2".to_string())]);
        let id_of = |name: &str| one::<i64>(&mm, &format!("SELECT id FROM users WHERE username = '{name}'"));
        let (raven_mm, tank2) = (id_of("Raven"), id_of("Tank2"));
        assert_ne!(tank2, other_tank);
        assert_eq!(
            one::<i64>(&mm, &format!("SELECT matches_played FROM users WHERE id = {kiwi_mm}")),
            6,
            "both servers' matches"
        );
        assert_eq!(one::<String>(&mm, &format!("SELECT global_id FROM users WHERE id = {raven_mm}")), "IDRAVEN");
        assert_eq!(
            one::<String>(&mm, &format!("SELECT kind FROM relationships WHERE user_id = {kiwi_mm} AND other_id = {raven_mm}")),
            "friend"
        );
        assert_eq!(
            one::<String>(&mm, &format!("SELECT kind FROM relationships WHERE user_id = {raven_mm} AND other_id = {tank2}")),
            "block"
        );
        assert_eq!(
            one::<Option<i64>>(&mm, &format!("SELECT until FROM bans WHERE user_id = {raven_mm}")),
            None,
            "banned for good"
        );
        assert_eq!(
            one::<f64>(&mm, &format!("SELECT value FROM player_stats WHERE user_id = {kiwi_mm} AND stat_id = 100")),
            12.0,
            "kills add up"
        );
        assert_eq!(
            one::<f64>(&mm, &format!("SELECT value FROM player_stats WHERE user_id = {kiwi_mm} AND stat_id = 216")),
            9.0,
            "the longest killstreak is the larger"
        );
        assert_eq!(
            one::<i64>(&mm, &format!("SELECT size FROM player_content WHERE user_id = {kiwi_mm}")),
            3,
            "the newer content"
        );
        assert!(
            one::<String>(&mm, &format!("SELECT password_hash FROM users WHERE id = {raven_mm}")).starts_with("$argon2"),
            "their password goes with them"
        );

        // Unsent changes: refused, nothing changed.
        exec(&eu, "INSERT INTO federation_outbox (body) VALUES ('{}')");
        let before = one::<i64>(&mm, "SELECT COUNT(*) FROM users");
        assert!(run(mm.merge_accounts(&eu_dir.join("test.db")))
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("federation_outbox"));
        assert_eq!(one::<i64>(&mm, "SELECT COUNT(*) FROM users"), before);
        for d in [mm_dir, eu_dir] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
