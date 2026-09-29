//! Accounts: signing in, and one-click accounts for players who don't have
//! one. The launcher talks to the server (gRPC); this is the logic around it.

use rand::Rng as _;

/// What the server said about an account.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
    #[error("wrong password")]
    WrongPassword,
    #[error("no such account on this server")]
    NotFound,
    #[error("that name is taken")]
    Taken,
    #[error("{0}")]
    Other(String),
}

/// The server's account calls.
pub trait AccountService {
    fn login(&self, username: &str, password: &str) -> Result<(), AccountError>;
    fn register(&self, username: &str, password: &str) -> Result<(), AccountError>;
}

/// How [`ensure_account`] got its account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The saved account signed in.
    Existing,
    /// The saved account didn't exist on this server, so it was registered.
    Registered,
    /// A new account was made.
    Created,
}

/// A username for a new account from `nick` (the player's chosen name, or
/// their Windows user name): letters, digits, `_`, `-` and `.`, spaces as
/// `_`, at most 24 characters, "Agent" if nothing is left.
pub fn account_name(nick: &str) -> String {
    let name: String = nick
        .trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('_'),
            c if c.is_alphanumeric() || matches!(c, '_' | '-' | '.') => Some(c),
            _ => None,
        })
        .take(24)
        .collect();
    if name.is_empty() {
        String::from("Agent")
    } else {
        name
    }
}

/// A random password for a one-click account: 24 lowercase letters and digits.
pub fn new_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut rng = rand::rng();
    (0..24).map(|_| char::from(ALPHABET[rng.random_range(0..ALPHABET.len())])).collect()
}

/// Makes sure the player has an account that signs in.
///
/// With `saved` credentials: keep them if they sign in; if this server
/// doesn't know the account, register it (the same name and password, so a
/// player keeps one identity across servers). Otherwise create a new account
/// named after `nick` (`nick`, `nick2`, ... up to `nick20` if taken).
pub fn ensure_account(service: &dyn AccountService, saved: Option<(&str, &str)>, nick: &str) -> Result<(String, String, Outcome), AccountError> {
    if let Some((username, password)) = saved.filter(|(u, p)| !u.is_empty() && !p.is_empty()) {
        match service.login(username, password) {
            Ok(()) => return Ok((username.into(), password.into(), Outcome::Existing)),
            Err(AccountError::NotFound) => {
                service.register(username, password)?;
                return Ok((username.into(), password.into(), Outcome::Registered));
            }
            Err(e) => return Err(e),
        }
    }
    let (username, password) = create_account(service, nick)?;
    Ok((username, password, Outcome::Created))
}

/// Registers a new account named after `nick` with a random password.
pub fn create_account(service: &dyn AccountService, nick: &str) -> Result<(String, String), AccountError> {
    let base = account_name(nick);
    let password = new_password();
    for n in 1..=20 {
        let username = if n == 1 { base.clone() } else { format!("{base}{n}") };
        match service.register(&username, &password) {
            Ok(()) => return Ok((username, password)),
            Err(AccountError::Taken) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(AccountError::Taken)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct FakeServer(RefCell<HashMap<String, String>>);

    impl AccountService for FakeServer {
        fn login(&self, username: &str, password: &str) -> Result<(), AccountError> {
            match self.0.borrow().get(username) {
                None => Err(AccountError::NotFound),
                Some(p) if p == password => Ok(()),
                Some(_) => Err(AccountError::WrongPassword),
            }
        }

        fn register(&self, username: &str, password: &str) -> Result<(), AccountError> {
            let mut accounts = self.0.borrow_mut();
            if accounts.contains_key(username) {
                return Err(AccountError::Taken);
            }
            accounts.insert(username.into(), password.into());
            Ok(())
        }
    }

    #[test]
    fn names() {
        assert_eq!(account_name("  Sam Fisher! "), "Sam_Fisher");
        assert_eq!(account_name("???"), "Agent");
        assert_eq!(account_name(&"x".repeat(40)).len(), 24);
        let pw = new_password();
        assert_eq!(pw.len(), 24);
        assert!(pw.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }

    #[test]
    fn keeps_registers_or_creates() {
        let server = FakeServer::default();
        server.register("Kiwi", "kiwi-password").unwrap();

        let (u, _, how) = ensure_account(&server, Some(("Kiwi", "kiwi-password")), "ignored").unwrap();
        assert_eq!((u.as_str(), how), ("Kiwi", Outcome::Existing));

        // A saved account from another server is registered here as is.
        let (u, p, how) = ensure_account(&server, Some(("Tank", "tank-password")), "ignored").unwrap();
        assert_eq!((u.as_str(), p.as_str(), how), ("Tank", "tank-password", Outcome::Registered));

        // A wrong password is reported, not papered over with a new account.
        assert_eq!(ensure_account(&server, Some(("Kiwi", "wrong")), "Kiwi"), Err(AccountError::WrongPassword));

        // No saved account: a new one, with the next free name.
        let (u, p, how) = ensure_account(&server, None, "Kiwi").unwrap();
        assert_eq!((u.as_str(), how), ("Kiwi2", Outcome::Created));
        assert!(server.login(&u, &p).is_ok());
    }
}
