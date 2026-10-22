//! The launcher's join request for the game, next to `uplay.toml`: the friend (account id) the
//! player clicked Join on there, and when (Unix seconds). The launcher is another process, so
//! this is how it tells the game which join it asked for. The launcher writes it with
//! [`contents`]; the game's hook reads it with [`read`] and deletes it, and takes only that
//! friend's invitation without a click (the hooks' `community::requested_join`).

use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

/// The file, in the game's folder (beside `uplay.toml`).
pub const FILE: &str = "5th-echelon-join-request.json";

/// How long a join request (the overlay's or the launcher's) stays good: long enough for a game
/// the launcher started to boot and sign in, short enough that it can't attach to a later,
/// unrelated invitation from the same friend.
pub const GOOD_FOR: Duration = Duration::from_secs(5 * 60);

/// How far ahead of the game's clock a request may be dated (the launcher's clock is the same
/// PC's, so only a little).
const CLOCK_SLACK: i64 = 60;

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    id: String,
    at: i64,
}

/// Why a request file isn't taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not a request at all.
    Unreadable,
    /// Dated ahead (it would outlive its time), or not an account id the game can take.
    Invalid,
    /// Older than [`GOOD_FOR`]; how old.
    Stale(Duration),
}

/// The file's contents for a Join on `friend_id`, clicked at `at` (Unix seconds).
pub fn contents(friend_id: &str, at: i64) -> String {
    serde_json::to_string(&Request { id: friend_id.to_string(), at }).expect("a string and a number")
}

/// The friend a request file asks to join, and how long ago it was written, when it's still
/// good at `now` (Unix seconds).
pub fn read(bytes: &[u8], now: i64) -> Result<(String, Duration), Refused> {
    let request: Request = serde_json::from_slice(bytes).map_err(|_| Refused::Unreadable)?;
    if request.at > now + CLOCK_SLACK || !crate::text::is_plain_id(&request.id) {
        return Err(Refused::Invalid);
    }
    let age = Duration::from_secs(now.saturating_sub(request.at).max(0) as u64);
    if age >= GOOD_FOR {
        return Err(Refused::Stale(age));
    }
    Ok((request.id, age))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn the_launchers_request_is_what_the_game_reads() {
        let file = contents("5f3a9c1e-77b2.x_y", NOW - 30);
        assert_eq!(read(file.as_bytes(), NOW), Ok(("5f3a9c1e-77b2.x_y".to_string(), Duration::from_secs(30))));
    }

    #[test]
    fn a_request_is_good_for_five_minutes() {
        let fresh = contents("friend", NOW);
        assert_eq!(read(fresh.as_bytes(), NOW), Ok(("friend".to_string(), Duration::ZERO)));
        let nearly = contents("friend", NOW - GOOD_FOR.as_secs() as i64 + 1);
        assert!(read(nearly.as_bytes(), NOW).is_ok());
        let stale = contents("friend", NOW - GOOD_FOR.as_secs() as i64);
        assert_eq!(read(stale.as_bytes(), NOW), Err(Refused::Stale(GOOD_FOR)));
    }

    #[test]
    fn one_dated_ahead_is_refused_past_the_clock_slack() {
        let slightly = contents("friend", NOW + CLOCK_SLACK);
        assert_eq!(read(slightly.as_bytes(), NOW), Ok(("friend".to_string(), Duration::ZERO)), "a clock a little behind");
        let ahead = contents("friend", NOW + CLOCK_SLACK + 1);
        assert_eq!(read(ahead.as_bytes(), NOW), Err(Refused::Invalid));
    }

    #[test]
    fn only_a_plain_account_id_is_taken() {
        for id in ["", "a b", "a/b", "名前", "x\n", &"a".repeat(64)] {
            assert_eq!(read(contents(id, NOW).as_bytes(), NOW), Err(Refused::Invalid), "{id:?}");
        }
        assert!(read(contents(&"a".repeat(63), NOW).as_bytes(), NOW).is_ok());
    }

    #[test]
    fn anything_else_is_unreadable() {
        for bytes in [
            &b""[..],
            b"{}",
            b"not json",
            br#"{"id":"friend"}"#,
            br#"{"id":"friend","at":"soon"}"#,
            br#"{"id":7,"at":1}"#,
        ] {
            assert_eq!(read(bytes, NOW), Err(Refused::Unreadable), "{:?}", String::from_utf8_lossy(bytes));
        }
    }
}
