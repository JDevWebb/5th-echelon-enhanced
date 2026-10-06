//! Which launchers and game clients may play here.
//!
//! The launcher and the game's client DLL say their release when they sign in to the API
//! ("launcher/0.3.156", "game/0.3.156"). One older than `[clients] minimum_version` (by
//! default this server's own release) is refused with a message saying what to do, and so
//! is one that says nothing (every client from before this check). The game's own sign-in
//! (the ticket server's Login/LoginEx) carries no version, so it is let through only for an
//! account a current client signed in to lately, with no outdated one trying since: the
//! client DLL signs in to the API as the game starts, well before the game signs in.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use sha2::Digest as _;
use sha2::Sha256;

/// How long a current client's sign-in vouches for the game's (a ticket lasts as long).
pub const VOUCHES_FOR: i64 = 24 * 60 * 60;

static MINIMUM: OnceLock<Option<[u32; 3]>> = OnceLock::new();

/// How long an outdated client that proved its password is refused again without the
/// password being checked (an old client retrying every second costs a hash each time).
const REFUSED_FOR: Duration = Duration::from_secs(60);
/// Outdated sign-ins that proved their password lately, by a hash of the name, client and
/// password (only the same three match): when, and the account.
static REFUSED: Mutex<Option<HashMap<[u8; 32], (Instant, u32)>>> = Mutex::new(None);

/// Unix seconds now.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// major.minor.patch of a release ("0.3.156", "0.3.156-dev").
pub fn parse(version: &str) -> Option<[u32; 3]> {
    let mut parts = version.split('-').next()?.split('.').map(|p| p.parse().ok());
    let v = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(v)
}

fn show([major, minor, patch]: [u32; 3]) -> String {
    format!("{major}.{minor}.{patch}")
}

/// Sets the oldest client allowed from `[clients] minimum_version`; returns it, or a
/// complaint about the setting (the server's own release is used then).
pub fn configure(cfg: &crate::config::ClientsConfig) -> Result<Option<String>, String> {
    let own = parse(crate::community_api::RELEASE);
    let (minimum, complaint) = match cfg.minimum_version.as_deref().map(str::trim) {
        None | Some("") => (own, None),
        Some(off) if off.eq_ignore_ascii_case("off") => (None, None),
        Some(v) => match parse(v) {
            Some(v) => (Some(v), None),
            None => (
                own,
                Some(format!(
                    "[clients] minimum_version {v:?} isn't major.minor.patch or \"off\"; using {}",
                    crate::community_api::RELEASE
                )),
            ),
        },
    };
    let _ = MINIMUM.set(minimum);
    complaint.map_or(Ok(minimum.map(show)), Err)
}

/// The oldest client allowed, if there's a limit.
pub fn minimum() -> Option<String> {
    MINIMUM.get().copied().flatten().map(show)
}

/// Whether `client` ("kind/version", as a client signs in with) may sign in; when not, what
/// to tell the player.
pub fn check(client: &str) -> Result<(), String> {
    let Some(minimum) = MINIMUM.get().copied().flatten() else {
        return Ok(());
    };
    let (kind, version) = client.split_once('/').unwrap_or(("", client));
    match parse(version) {
        Some(v) if v >= minimum => Ok(()),
        Some(v) => Err(refusal(kind, &format!("version {}", show(v)), minimum)),
        None => Err(refusal(kind, "an older version", minimum)),
    }
}

fn refusal(kind: &str, what: &str, minimum: [u32; 3]) -> String {
    let fix = if kind == "game" {
        "Start the game from the launcher to update it"
    } else {
        "Update the launcher (it updates itself when it starts, or download the latest release)"
    };
    format!("This server needs 5th Echelon {} or newer, and this is {what}. {fix}.", show(minimum))
}

fn refusal_key(username: &str, client: &str, password: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    for part in [username, client, password] {
        h.update((part.len() as u64).to_le_bytes());
        h.update(part.as_bytes());
    }
    h.finalize().into()
}

/// Whether this outdated client signed in with this name and password within the last
/// minute and was refused: if so, what to tell it, without checking the password again.
pub fn refused_lately(username: &str, client: &str, password: &str) -> Option<String> {
    let why = check(client).err()?;
    let key = refusal_key(username, client, password);
    let mut refused = REFUSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = refused.get_or_insert_with(HashMap::new);
    map.retain(|_, (at, _)| at.elapsed() < REFUSED_FOR);
    map.contains_key(&key).then_some(why)
}

/// An outdated client proved the account's password and was refused (and noted as such).
pub fn refused(user_id: u32, username: &str, client: &str, password: &str) {
    let mut refused = REFUSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = refused.get_or_insert_with(HashMap::new);
    // Bounded: one per account and client at most, gone after a minute.
    if map.len() < 10_000 {
        map.insert(refusal_key(username, client, password), (Instant::now(), user_id));
    }
}

/// A current client signed in to the account: an outdated one trying after it must be
/// checked (and noted) again, or the game's sign-in would go by the current one.
pub fn admitted(user_id: u32) {
    let mut refused = REFUSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(map) = refused.as_mut() {
        map.retain(|_, (_, user)| *user != user_id);
    }
}

/// Whether a refusal of `who` is worth a line in the log: once an hour each. A game or
/// launcher not yet updated retries every few seconds (Renegade's 0.4.1 game: 494 times in
/// half an hour, eu1, 2026-10-06), and each was a warning; the Sessions page still counts them.
pub fn worth_saying(who: &str) -> bool {
    static SAID: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
    let mut said = SAID.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = said.get_or_insert_with(HashMap::new);
    map.retain(|_, at| at.elapsed() < SAY_REFUSALS_EVERY);
    let key = who.to_lowercase();
    if map.contains_key(&key) || map.len() >= 10_000 {
        return false;
    }
    map.insert(key, Instant::now());
    true
}

const SAY_REFUSALS_EVERY: Duration = Duration::from_secs(3600);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_said_once_an_hour() {
        assert!(worth_saying("Clients-Test-Renegade"));
        assert!(!worth_saying("clients-test-renegade"), "the same player, any case");
        assert!(worth_saying("Clients-Test-Other"));
    }

    #[test]
    fn versions_parse() {
        assert_eq!(parse("0.3.156"), Some([0, 3, 156]));
        assert_eq!(parse("0.3.156-dev"), Some([0, 3, 156]));
        assert_eq!(parse("0.3"), None);
        assert_eq!(parse("0.3.1.2"), None);
        assert_eq!(parse(""), None);
        assert!([0, 3, 99] < [0, 3, 156] && [0, 4, 0] > [0, 3, 156]);
    }

    #[test]
    fn outdated_refusals_are_remembered_for_the_same_credentials() {
        let _ = MINIMUM.set(Some([0, 4, 1]));
        assert_eq!(refused_lately("ana", "launcher/0.4.0", "secret1"), None);
        refused(7, "ana", "launcher/0.4.0", "secret1");
        assert!(refused_lately("ana", "launcher/0.4.0", "secret1").is_some());
        // Another password, client or name goes through the full check.
        assert_eq!(refused_lately("ana", "launcher/0.4.0", "secret2"), None);
        assert_eq!(refused_lately("ana", "game/0.4.0", "secret1"), None);
        assert_eq!(refused_lately("bob", "launcher/0.4.0", "secret1"), None);
        // A current client: never refused from memory.
        assert_eq!(refused_lately("ana", "launcher/0.4.1", "secret1"), None);
        // A current client signing in to the account clears it.
        admitted(7);
        assert_eq!(refused_lately("ana", "launcher/0.4.0", "secret1"), None);
    }
}
