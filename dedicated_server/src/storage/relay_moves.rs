//! Players moved to the relay for a while on the network they were on (`relay_moves`, 0.5.5's
//! JC2; see `crate::relay_moves`).

use super::run;
use super::Result;
use super::Storage;

impl Storage {
    /// `user_id`, at `ip`, is relayed for `why` until `until` (unix seconds): replaces an earlier
    /// move of theirs.
    pub fn remember_relay_move(&self, user_id: u32, ip: &str, why: &str, until: i64) -> Result<()> {
        run(async {
            sqlx::query("INSERT INTO relay_moves (user_id, ip, why, until) VALUES (?, ?, ?, ?) ON CONFLICT(user_id) DO UPDATE SET ip = excluded.ip, why = excluded.why, until = excluded.until")
                .bind(user_id)
                .bind(ip)
                .bind(why)
                .bind(until)
                .execute(&self.pool)
                .await
        })??;
        Ok(())
    }

    /// `user_id`'s move still in force at `now` (unix seconds): the address it was made at, and why.
    pub fn relay_move(&self, user_id: u32, now: i64) -> Result<Option<(String, String)>> {
        Ok(run(async {
            sqlx::query_as::<_, (String, String)>("SELECT ip, why FROM relay_moves WHERE user_id = ? AND until > ?")
                .bind(user_id)
                .bind(now)
                .fetch_optional(&self.pool)
                .await
        })??)
    }

    /// Forgets `user_id`'s move, and every move that has run out.
    pub fn forget_relay_move(&self, user_id: u32, now: i64) -> Result<()> {
        run(async {
            sqlx::query("DELETE FROM relay_moves WHERE user_id = ? OR until <= ?")
                .bind(user_id)
                .bind(now)
                .execute(&self.pool)
                .await
        })??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::tests::temp_storage;

    #[test]
    fn a_move_lasts_until_it_runs_out() {
        let (storage, dir) = temp_storage("relay-moves");
        storage.remember_relay_move(7, "198.51.100.4", "failed_joins", 1000).unwrap();
        assert_eq!(storage.relay_move(7, 999).unwrap(), Some(("198.51.100.4".into(), "failed_joins".into())));
        assert_eq!(storage.relay_move(7, 1000).unwrap(), None, "run out");
        storage.remember_relay_move(7, "198.51.100.5", "unreachable_peer", 2000).unwrap();
        assert_eq!(storage.relay_move(7, 1500).unwrap(), Some(("198.51.100.5".into(), "unreachable_peer".into())), "the latest");
        storage.forget_relay_move(7, 0).unwrap();
        assert_eq!(storage.relay_move(7, 0).unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
