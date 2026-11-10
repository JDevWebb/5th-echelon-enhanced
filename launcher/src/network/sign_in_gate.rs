//! When the launcher may sign in again after a sign-in went wrong, so it never keeps asking a
//! server that has already said no (an outdated launcher on a loop, a wrong password every
//! 30 s): what kind of failure it was decides.
//!
//! - **Can't reach the server** (no connection, a timeout, the server unavailable): tried again,
//!   waiting longer each time ([`CONNECTION_WAITS`]), from the first success on as before.
//! - **This launcher is refused** (outdated, or a build the network doesn't take): not tried
//!   again on that server until the launcher changes (it restarts to update).
//! - **Wrong name or password**: [`WRONG_TRIES`] tries, then none for [`WRONG_WAIT`]; a changed
//!   password is tried at once (the hold is for the password that was refused).
//! - **Banned**: none for [`BANNED_WAIT`].
//! - **The server is busy**: none for [`BUSY_WAIT`].

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// Waits after each consecutive failure to connect (the last repeats).
const CONNECTION_WAITS: [u64; 6] = [5, 15, 30, 60, 120, 300];
const WRONG_TRIES: u32 = 3;
const WRONG_WAIT: Duration = Duration::from_secs(30 * 60);
const BANNED_WAIT: Duration = Duration::from_secs(30 * 60);
const BUSY_WAIT: Duration = Duration::from_secs(60);

/// What a sign-in's failure was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Connection,
    Refused(String),
    Wrong,
    Banned(String),
    Busy,
    /// Anything else: no hold.
    Other,
}

impl Failure {
    pub fn of(e: &super::Error) -> Self {
        use super::Error;
        match e {
            Error::ConnectionFailed | Error::TimedOut => Self::Connection,
            Error::InvalidPassword | Error::UserNotFound => Self::Wrong,
            Error::Rpc(status) => Self::of_status(status),
            _ => Self::Other,
        }
    }

    pub fn of_status(status: &tonic::Status) -> Self {
        use tonic::Code;
        match status.code() {
            Code::Unavailable | Code::DeadlineExceeded | Code::Cancelled | Code::Unknown => Self::Connection,
            Code::Unauthenticated | Code::NotFound => Self::Wrong,
            Code::FailedPrecondition => Self::Refused(status.message().to_string()),
            Code::PermissionDenied => Self::Banned(status.message().to_string()),
            Code::ResourceExhausted => Self::Busy,
            _ => Self::Other,
        }
    }
}

#[derive(Default)]
struct Held {
    /// Consecutive failures to connect, and until when the next waits.
    connection: (u32, Option<Instant>),
    /// Wrong passwords in a row (for this password), and until when none is tried.
    wrong: (u32, Option<Instant>),
    /// Refused for good (this run), and why.
    refused: Option<String>,
    banned: Option<(Instant, String)>,
    busy: Option<Instant>,
}

/// Holds by server, and by server, account and password (hashed).
fn held() -> std::sync::MutexGuard<'static, HashMap<String, Held>> {
    static HELD: std::sync::LazyLock<Mutex<HashMap<String, Held>>> = std::sync::LazyLock::new(Mutex::default);
    HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The key for `account` with `secret` on `server`: never the secret itself.
fn account_key(server: &str, account: &str, secret: &str) -> String {
    format!("{server}\n{}\n{}", account.to_lowercase(), super::secret_fingerprint(secret))
}

/// Why a sign-in to `server` as `account` with `secret` may not be tried now, if it may not.
pub fn why_not(server: &str, account: &str, secret: &str) -> Option<String> {
    why_not_at(server, account, secret, Instant::now())
}

fn why_not_at(server: &str, account: &str, secret: &str, now: Instant) -> Option<String> {
    let all = held();
    let left = |until: Instant| until.saturating_duration_since(now);
    if let Some(h) = all.get(server) {
        if let Some(why) = &h.refused {
            return Some(format!("{why} (not signing in again until the launcher is updated)"));
        }
        if let Some(until) = h.busy.filter(|u| *u > now) {
            return Some(format!("The server is busy; signing in again in {} s", left(until).as_secs().max(1)));
        }
        if let (n, Some(until)) = h.connection {
            if until > now {
                return Some(format!("Couldn't reach the server ({n} times); trying again in {} s", left(until).as_secs().max(1)));
            }
        }
    }
    if let Some(h) = all.get(&account_key(server, account, secret)) {
        if let Some((until, why)) = h.banned.as_ref().filter(|(u, _)| *u > now) {
            return Some(format!("{why} (not signing in again for {} min)", left(*until).as_secs().div_ceil(60)));
        }
        if let (_, Some(until)) = h.wrong {
            if until > now {
                return Some(format!(
                    "The server refused this account's password {WRONG_TRIES} times; not trying it again for {} min (check it in Settings › Servers and accounts)",
                    left(until).as_secs().div_ceil(60)
                ));
            }
        }
    }
    None
}

/// How a sign-in went: `None` signed in, else its failure.
pub fn record(server: &str, account: &str, secret: &str, failure: Option<&Failure>) {
    record_at(server, account, secret, failure, Instant::now());
}

fn record_at(server: &str, account: &str, secret: &str, failure: Option<&Failure>, now: Instant) {
    let mut all = held();
    if all.len() > 1000 {
        all.clear();
    }
    let key = account_key(server, account, secret);
    match failure {
        None => {
            all.remove(&key);
            if let Some(h) = all.get_mut(server) {
                (h.connection, h.busy) = ((0, None), None);
            }
        }
        Some(Failure::Connection) => {
            let h = all.entry(server.to_string()).or_default();
            h.connection.0 += 1;
            let wait = CONNECTION_WAITS[(h.connection.0 as usize - 1).min(CONNECTION_WAITS.len() - 1)];
            h.connection.1 = Some(now + Duration::from_secs(wait));
        }
        Some(Failure::Refused(why)) => all.entry(server.to_string()).or_default().refused = Some(why.clone()),
        Some(Failure::Busy) => all.entry(server.to_string()).or_default().busy = Some(now + BUSY_WAIT),
        Some(Failure::Banned(why)) => all.entry(key).or_default().banned = Some((now + BANNED_WAIT, why.clone())),
        Some(Failure::Wrong) => {
            let h = all.entry(key).or_default();
            h.wrong.0 += 1;
            if h.wrong.0 >= WRONG_TRIES {
                h.wrong = (0, Some(now + WRONG_WAIT));
            }
        }
        Some(Failure::Other) => {}
    }
}

/// Runs `sign_in` unless a hold says not to (then [`super::Error::Held`]), and notes how it went.
pub async fn gated<T>(server: &str, account: &str, secret: &str, sign_in: impl std::future::Future<Output = Result<T, super::Error>>) -> Result<T, super::Error> {
    if let Some(why) = why_not(server, account, secret) {
        return Err(super::Error::Held(why));
    }
    let result = sign_in.await;
    match &result {
        Ok(_) => record(server, account, secret, None),
        Err(e) => record(server, account, secret, Some(&Failure::of(e))),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_failure_holds_as_long_as_it_should() {
        let t = Instant::now();
        let s = "https://holds.test";
        // Three wrong passwords, then 30 minutes off; another password is tried at once.
        for _ in 0..2 {
            record_at(s, "Kiwi", "bad", Some(&Failure::Wrong), t);
            assert_eq!(why_not_at(s, "Kiwi", "bad", t), None);
        }
        record_at(s, "Kiwi", "bad", Some(&Failure::Wrong), t);
        assert!(why_not_at(s, "Kiwi", "bad", t).unwrap().contains("30 min"));
        assert!(why_not_at(s, "Kiwi", "bad", t + Duration::from_secs(29 * 60)).is_some());
        assert_eq!(why_not_at(s, "Kiwi", "bad", t + Duration::from_secs(31 * 60)), None);
        assert_eq!(why_not_at(s, "Kiwi", "fixed", t), None, "a new password");
        // Can't connect: 5 s, then 15 s.
        record_at(s, "Tank", "pw", Some(&Failure::Connection), t);
        assert!(why_not_at(s, "Tank", "pw", t + Duration::from_secs(4)).is_some());
        assert_eq!(why_not_at(s, "Tank", "pw", t + Duration::from_secs(6)), None);
        record_at(s, "Tank", "pw", Some(&Failure::Connection), t + Duration::from_secs(6));
        assert!(why_not_at(s, "Tank", "pw", t + Duration::from_secs(20)).is_some());
        record_at(s, "Tank", "pw", None, t + Duration::from_secs(22));
        assert_eq!(why_not_at(s, "Tank", "pw", t + Duration::from_secs(22)), None, "connected: as before");
        // Outdated: never again on that server, whoever signs in.
        let o = "https://outdated.test";
        record_at(o, "Kiwi", "pw", Some(&Failure::Refused("Update the launcher".into())), t);
        assert!(why_not_at(o, "Nova", "other", t + Duration::from_secs(86_400)).unwrap().starts_with("Update the launcher"));
        assert_eq!(why_not_at("https://elsewhere.test", "Kiwi", "pw", t), None);
    }

    #[test]
    fn failures_are_told_apart() {
        assert_eq!(Failure::of_status(&tonic::Status::unavailable("x")), Failure::Connection);
        assert_eq!(Failure::of_status(&tonic::Status::unauthenticated("x")), Failure::Wrong);
        assert_eq!(Failure::of_status(&tonic::Status::failed_precondition("old")), Failure::Refused("old".into()));
        assert_eq!(Failure::of_status(&tonic::Status::permission_denied("banned")), Failure::Banned("banned".into()));
        assert_eq!(Failure::of_status(&tonic::Status::resource_exhausted("busy")), Failure::Busy);
        assert_eq!(Failure::of(&crate::network::Error::ConnectionFailed), Failure::Connection);
    }
}
