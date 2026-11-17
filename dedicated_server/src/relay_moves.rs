//! A move to the relay that lasts (0.5.5's JC2).
//!
//! J2 moves a direct player whose joins keep failing to the relay, for their session. The next
//! session started direct again and failed the same way: on ca1, 10-17 Nov 2026, Gbane started 13
//! sessions direct and lost 26 joins in one of them before anything moved him. So a move is kept
//! for [`LASTS`], with the address the player was at: signing in again from that address (their
//! network), they start relayed; from another, as their connection says.
//!
//! On the matchmaking server. A player at its own helper is relayed at registration
//! ([`crate::nat_helper`] asks [`lasting`]); one at another relay is relayed through that relay
//! when its report names them direct ([`remote_direct`]).

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::Duration;

use slog::info;
use slog::Logger;

use crate::storage::Storage;

/// How long a move lasts on the network it was made on.
pub const LASTS: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The signed-in players with a move in force, by name key: the address it was made at, and why.
fn moves() -> std::sync::MutexGuard<'static, HashMap<String, (Ipv4Addr, &'static str)>> {
    static MOVES: std::sync::LazyLock<Mutex<HashMap<String, (Ipv4Addr, &'static str)>>> = std::sync::LazyLock::new(Mutex::default);
    MOVES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The reasons a move is kept for, as `&'static str` (the helper's `why`).
fn why_of(why: &str) -> Option<&'static str> {
    ["failed_joins", "unreachable_peer"].into_iter().find(|w| *w == why)
}

fn now_secs() -> i64 {
    identity::now()
}

/// `user_id` (`name`), at `ip`, was moved to the relay for `why`: kept for [`LASTS`].
pub fn moved(storage: &Storage, logger: &Logger, user_id: u32, name: &str, ip: Ipv4Addr, why: &'static str) {
    let until = now_secs().saturating_add(i64::try_from(LASTS.as_secs()).unwrap_or(i64::MAX));
    if let Err(e) = storage.remember_relay_move(user_id, &ip.to_string(), why, until) {
        slog::warn!(logger, "Keeping {name}'s move to the relay failed: {e}");
        return;
    }
    let mut all = moves();
    if all.len() < 100_000 {
        all.insert(identity::name_key(name), (ip, why));
    }
}

/// `user_id` (`name`) signed in: a move of theirs still in force is applied from their next
/// registration, if it comes from the address the move was made at.
pub fn signed_in(storage: &Storage, logger: &Logger, user_id: u32, name: &str) {
    let found = match storage.relay_move(user_id, now_secs()) {
        Ok(found) => found,
        Err(e) => {
            slog::warn!(logger, "Looking up {name}'s move to the relay failed: {e}");
            return;
        }
    };
    let key = identity::name_key(name);
    let Some((ip, why)) = found.and_then(|(ip, why)| Some((ip.parse::<Ipv4Addr>().ok()?, why_of(&why)?))) else {
        moves().remove(&key);
        // None in force: theirs (and any other that ran out) goes.
        if let Err(e) = storage.forget_relay_move(user_id, now_secs()) {
            slog::warn!(logger, "Forgetting moves to the relay that ran out failed: {e}");
        }
        return;
    };
    info!(logger, "{name} was moved to the relay at {ip} lately ({why}): relayed again from there");
    let mut all = moves();
    if all.len() < 100_000 {
        all.insert(key, (ip, why));
    }
}

/// `name` signed out.
pub fn signed_out(name: &str) {
    moves().remove(&identity::name_key(name));
}

/// Why `key` (a name key) registering from `ip` is to be relayed, if a lasting move says so.
pub fn lasting(key: &str, ip: Ipv4Addr) -> Option<&'static str> {
    moves().get(key).filter(|(at, _)| *at == ip).map(|(_, why)| *why)
}

/// Of the players another relay reports (name, address, relayed), the direct ones a lasting move
/// says to relay: the caller tells their relay ([`crate::relays::relay_remote`]).
pub fn remote_direct<'a>(regs: impl IntoIterator<Item = (&'a str, Ipv4Addr, bool)>) -> Vec<String> {
    let all = moves();
    regs.into_iter()
        .filter(|(name, ip, relayed)| !relayed && all.get(&identity::name_key(name)).is_some_and(|(at, _)| at == ip))
        .map(|(name, ..)| name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_move_applies_on_the_network_it_was_made_on() {
        let (storage, dir) = crate::storage::tests::temp_storage("relay-moves-mod");
        let logger = Logger::root(slog::Discard, slog::o!());
        let home: Ipv4Addr = "198.51.100.4".parse().unwrap();
        let away: Ipv4Addr = "203.0.113.9".parse().unwrap();
        moved(&storage, &logger, 41, "MovedOnce", home, "failed_joins");
        signed_out("MovedOnce");
        assert_eq!(lasting(&identity::name_key("MovedOnce"), home), None, "signed out");
        signed_in(&storage, &logger, 41, "MovedOnce");
        let key = identity::name_key("MovedOnce");
        assert_eq!(lasting(&key, home), Some("failed_joins"), "signed in again at home");
        assert_eq!(lasting(&key, away), None, "another network");
        assert_eq!(remote_direct([("MovedOnce", home, false), ("MovedOnce", away, false)]), vec!["MovedOnce".to_string()]);
        assert!(remote_direct([("MovedOnce", home, true)]).is_empty(), "relayed already");
        storage.remember_relay_move(41, "198.51.100.4", "failed_joins", 1).unwrap();
        signed_in(&storage, &logger, 41, "MovedOnce");
        assert_eq!(lasting(&key, home), None, "ran out");
        assert_eq!(storage.relay_move(41, 0).unwrap(), None, "and forgotten");
        let _ = std::fs::remove_dir_all(dir);
    }
}
