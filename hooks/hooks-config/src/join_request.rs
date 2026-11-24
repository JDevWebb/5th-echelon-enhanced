//! The launcher's join request for the game, next to `uplay.toml`: the friend (account id) the
//! player clicked Join on there, and when (Unix seconds). The launcher is another process, so
//! this is how it tells the game which join it asked for. The launcher writes it with
//! [`contents`] (or [`write`]); the game's hook reads it with [`read`] and deletes it, and takes
//! only that friend's invitation without a click (the hooks' `community::requested_join`).
//!
//! A request the server answered without an invitation (`JoinNowResponse.direct`: into a public
//! match) says so (`direct`): the hook then starts the join itself, raising the game's
//! "invitation accepted" event for the friend, since no invitation will come.

use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

/// The file, in the game's folder (beside `uplay.toml`).
pub const FILE: &str = "5th-echelon-join-request.json";

/// How long a join request (the overlay's or the launcher's) stays good: long enough for a game
/// the launcher started to boot and sign in, short enough that it can't attach to a later,
/// unrelated invitation from the same friend.
pub const GOOD_FOR: Duration = Duration::from_secs(5 * 60);
/// A direct request (no invitation comes) is good for less: the server keeps the match for the
/// game's search only this long (the dedicated server's `join_now::FOR`), and a join started
/// after that finds nothing and leaves the player in a party of their own.
pub const DIRECT_GOOD_FOR: Duration = Duration::from_secs(4 * 60);

/// How far ahead of the game's clock a request may be dated (the launcher's clock is the same
/// PC's, so only a little).
const CLOCK_SLACK: i64 = 60;

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    id: String,
    at: i64,
    /// The server queued no invitation: the game starts the join itself. Absent in a request
    /// from an older launcher.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    direct: bool,
}

/// A join request that's still good: whom to join, how long ago it was asked for, and whether
/// the game is to start it itself (no invitation comes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub id: String,
    pub age: Duration,
    pub direct: bool,
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

/// The file's contents for a Join on `friend_id`, clicked at `at` (Unix seconds); `direct` when
/// the server queued no invitation for it.
pub fn contents(friend_id: &str, at: i64, direct: bool) -> String {
    serde_json::to_string(&Request {
        id: friend_id.to_string(),
        at,
        direct,
    })
    .expect("a string, a number and a flag")
}

/// Writes the request ([`contents`]) into the game's folder `game_dir`, whole or not at all: the
/// game reads it while it runs, and a half-written one would be dropped as unreadable.
pub fn write(game_dir: &std::path::Path, friend_id: &str, at: i64, direct: bool) -> std::io::Result<()> {
    let path = game_dir.join(FILE);
    let part = game_dir.join(format!("{FILE}.part"));
    std::fs::write(&part, contents(friend_id, at, direct))?;
    std::fs::rename(&part, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

/// The friend a request file asks to join, how long ago it was written, and whether it's
/// direct, when it's still good at `now` (Unix seconds).
pub fn read(bytes: &[u8], now: i64) -> Result<Asked, Refused> {
    let request: Request = serde_json::from_slice(bytes).map_err(|_| Refused::Unreadable)?;
    if request.at > now + CLOCK_SLACK || !crate::text::is_plain_id(&request.id) {
        return Err(Refused::Invalid);
    }
    let age = Duration::from_secs(now.saturating_sub(request.at).max(0) as u64);
    if age >= if request.direct { DIRECT_GOOD_FOR } else { GOOD_FOR } {
        return Err(Refused::Stale(age));
    }
    Ok(Asked {
        id: request.id,
        age,
        direct: request.direct,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn asked(id: &str, age: u64) -> Result<Asked, Refused> {
        Ok(Asked {
            id: id.to_string(),
            age: Duration::from_secs(age),
            direct: false,
        })
    }

    #[test]
    fn the_launchers_request_is_what_the_game_reads() {
        let file = contents("5f3a9c1e-77b2.x_y", NOW - 30, false);
        assert_eq!(read(file.as_bytes(), NOW), asked("5f3a9c1e-77b2.x_y", 30));
    }

    #[test]
    fn a_request_is_good_for_five_minutes() {
        let fresh = contents("friend", NOW, false);
        assert_eq!(read(fresh.as_bytes(), NOW), asked("friend", 0));
        let nearly = contents("friend", NOW - GOOD_FOR.as_secs() as i64 + 1, false);
        assert!(read(nearly.as_bytes(), NOW).is_ok());
        let stale = contents("friend", NOW - GOOD_FOR.as_secs() as i64, false);
        assert_eq!(read(stale.as_bytes(), NOW), Err(Refused::Stale(GOOD_FOR)));
    }

    #[test]
    fn one_dated_ahead_is_refused_past_the_clock_slack() {
        let slightly = contents("friend", NOW + CLOCK_SLACK, false);
        assert_eq!(read(slightly.as_bytes(), NOW), asked("friend", 0), "a clock a little behind");
        let ahead = contents("friend", NOW + CLOCK_SLACK + 1, false);
        assert_eq!(read(ahead.as_bytes(), NOW), Err(Refused::Invalid));
    }

    #[test]
    fn only_a_plain_account_id_is_taken() {
        for id in ["", "a b", "a/b", "名前", "x\n", &"a".repeat(64)] {
            assert_eq!(read(contents(id, NOW, false).as_bytes(), NOW), Err(Refused::Invalid), "{id:?}");
        }
        assert!(read(contents(&"a".repeat(63), NOW, false).as_bytes(), NOW).is_ok());
    }

    #[test]
    fn a_direct_request_says_so_and_an_older_one_isnt() {
        let direct = contents("friend", NOW, true);
        assert_eq!(read(direct.as_bytes(), NOW).map(|a| a.direct), Ok(true));
        assert_eq!(contents("friend", NOW, false), format!(r#"{{"id":"friend","at":{NOW}}}"#), "as an older launcher wrote it");
        assert_eq!(read(format!(r#"{{"id":"friend","at":{NOW}}}"#).as_bytes(), NOW).map(|a| a.direct), Ok(false));
    }

    #[test]
    fn a_direct_request_lapses_with_the_servers_hold() {
        let nearly = contents("friend", NOW - DIRECT_GOOD_FOR.as_secs() as i64 + 1, true);
        assert!(read(nearly.as_bytes(), NOW).is_ok());
        let lapsed = contents("friend", NOW - DIRECT_GOOD_FOR.as_secs() as i64, true);
        assert_eq!(read(lapsed.as_bytes(), NOW), Err(Refused::Stale(DIRECT_GOOD_FOR)));
        let invited = contents("friend", NOW - DIRECT_GOOD_FOR.as_secs() as i64, false);
        assert!(read(invited.as_bytes(), NOW).is_ok(), "an invitation's request keeps its five minutes");
    }

    #[test]
    fn a_request_is_written_whole() {
        let dir = std::env::temp_dir().join(format!("join-request-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, "friend", NOW, false).unwrap();
        write(&dir, "friend", NOW, true).unwrap();
        let bytes = std::fs::read(dir.join(FILE)).unwrap();
        assert_eq!(read(&bytes, NOW).map(|a| a.direct), Ok(true), "the second replaced the first");
        assert!(!dir.join(format!("{FILE}.part")).exists(), "nothing left beside it");
        let _ = std::fs::remove_dir_all(dir);
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
