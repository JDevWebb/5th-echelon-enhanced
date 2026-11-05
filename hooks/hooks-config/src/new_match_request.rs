//! The launcher's "New match" for the game, next to `uplay.toml`: when it was asked (Unix
//! seconds). The launcher asks the server first (the player's searches find nothing for a while,
//! `search_steer`), starts the game with the quick start, and writes this; the game's quick start
//! reads it with [`read`], deletes it, and once in the online lobby starts matchmaking by itself
//! (Quick match and the mode, or Co-op's Find teammate), so it hosts a new public match without
//! joining an open one and without a click. The mode is the playlist the player chose (the id of
//! its entry in the game's mode list, `game_names::PLAYLISTS`), or the one the game has focused.

use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

/// The file, in the game's folder (beside `uplay.toml`).
pub const FILE: &str = "5th-echelon-new-match.json";

/// How long a request stays good: as long as the server's ask holds (`search_steer::FOR`), so a
/// game that boots slowly still hosts, and a later start of the game doesn't.
pub const GOOD_FOR: Duration = Duration::from_secs(5 * 60);

/// How far ahead of the game's clock a request may be dated (the same PC's clock).
const CLOCK_SLACK: i64 = 60;

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    playlist: Option<u32>,
}

/// A request that's still good.
#[derive(Debug, PartialEq, Eq)]
pub struct Accepted {
    /// How long ago it was asked.
    pub age: Duration,
    /// The Spies vs Mercs mode chosen (its playlist), if one was.
    pub playlist: Option<u32>,
}

/// Why a request file isn't taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not a request at all.
    Unreadable,
    /// Dated ahead (it would outlive its time).
    Invalid,
    /// Older than [`GOOD_FOR`]; how old.
    Stale(Duration),
}

/// The file's contents for a New match asked at `at` (Unix seconds), in `playlist`'s mode.
pub fn contents(at: i64, playlist: Option<u32>) -> String {
    serde_json::to_string(&Request { at, playlist }).expect("numbers")
}

/// The request, when it's still good at `now` (Unix seconds).
pub fn read(bytes: &[u8], now: i64) -> Result<Accepted, Refused> {
    let request: Request = serde_json::from_slice(bytes).map_err(|_| Refused::Unreadable)?;
    if request.at > now + CLOCK_SLACK {
        return Err(Refused::Invalid);
    }
    #[allow(clippy::cast_sign_loss)]
    let age = Duration::from_secs(now.saturating_sub(request.at).max(0) as u64);
    if age >= GOOD_FOR {
        return Err(Refused::Stale(age));
    }
    Ok(Accepted { age, playlist: request.playlist })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn the_launchers_request_is_what_the_game_reads() {
        let got = read(contents(NOW - 20, Some(4_008_726_265)).as_bytes(), NOW);
        assert_eq!(
            got,
            Ok(Accepted {
                age: Duration::from_secs(20),
                playlist: Some(4_008_726_265)
            })
        );
        // Co-op, or the game's own choice: no playlist.
        assert_eq!(read(contents(NOW, None).as_bytes(), NOW).map(|a| a.playlist), Ok(None));
        assert_eq!(read(br#"{"at":1800000000}"#, NOW).map(|a| a.playlist), Ok(None));
    }

    #[test]
    fn a_request_is_good_for_five_minutes_and_not_from_the_future() {
        #[allow(clippy::cast_possible_wrap)]
        let good = GOOD_FOR.as_secs() as i64;
        assert!(read(contents(NOW - good + 1, None).as_bytes(), NOW).is_ok());
        assert_eq!(read(contents(NOW - good, None).as_bytes(), NOW), Err(Refused::Stale(GOOD_FOR)));
        assert_eq!(read(contents(NOW + CLOCK_SLACK + 1, None).as_bytes(), NOW), Err(Refused::Invalid));
        assert_eq!(read(b"not json", NOW), Err(Refused::Unreadable));
    }
}
