//! The XP each player's game says its profile has (0.5.6's N2): the rank a player shows comes from
//! their local save, so it can't be stopped without server-held progression; noted once a day
//! (`profile`, `xp`) to set against what their stats imply, and flagged once the data shows how
//! far apart the two run for honest players.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::PoisonError;

static NOTED_ON: Mutex<Option<HashMap<u32, i64>>> = Mutex::new(None);

/// Whether `user`'s XP isn't noted yet on the day of `now` (Unix seconds, UTC); from now on it is.
pub fn first_today(user: u32, now: i64) -> bool {
    let day = now.div_euclid(86_400);
    let mut noted = NOTED_ON.lock().unwrap_or_else(PoisonError::into_inner);
    let map = noted.get_or_insert_with(HashMap::new);
    if map.len() > 200_000 {
        map.retain(|_, d| *d == day);
    }
    map.insert(user, day) != Some(day)
}

#[cfg(test)]
mod tests {
    #[test]
    fn once_a_day() {
        let day = 20_000 * 86_400;
        assert!(super::first_today(5, day));
        assert!(!super::first_today(5, day + 100));
        assert!(super::first_today(5, day + 86_400));
    }
}
