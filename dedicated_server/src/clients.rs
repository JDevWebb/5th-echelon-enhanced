//! Which launchers and game clients may play here.
//!
//! The launcher and the game's client DLL say their release when they sign in to the API
//! ("launcher/0.3.156", "game/0.3.156"). One older than `[clients] minimum_version` (by
//! default this server's own release) is refused with a message saying what to do, and so
//! is one that says nothing (every client from before this check). The game's own sign-in
//! (the ticket server's Login/LoginEx) carries no version, so it is let through only for an
//! account a current client signed in to lately, with no outdated one trying since: the
//! client DLL signs in to the API as the game starts, well before the game signs in.

use std::sync::OnceLock;

/// How long a current client's sign-in vouches for the game's (a ticket lasts as long).
pub const VOUCHES_FOR: i64 = 24 * 60 * 60;

static MINIMUM: OnceLock<Option<[u32; 3]>> = OnceLock::new();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse() {
        assert_eq!(parse("0.3.156"), Some([0, 3, 156]));
        assert_eq!(parse("0.3.156-dev"), Some([0, 3, 156]));
        assert_eq!(parse("0.3"), None);
        assert_eq!(parse("0.3.1.2"), None);
        assert_eq!(parse(""), None);
        assert!([0, 3, 99] < [0, 3, 156] && [0, 4, 0] > [0, 3, 156]);
    }
}
