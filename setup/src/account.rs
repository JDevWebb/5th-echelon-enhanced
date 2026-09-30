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
    /// Signs in to `username` with the player's identity key (see
    /// `player_identity`) and gives the account `new_password`. `NotFound`
    /// when the account isn't linked to this identity, or the server can't.
    fn key_login(&self, _username: &str, _new_password: &str) -> Result<(), AccountError> {
        Err(AccountError::NotFound)
    }
}

/// How [`ensure_account`] got its account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The saved account signed in.
    Existing,
    /// An account of the player's on this server (made on another PC, or
    /// whose password was lost) signed in with their identity key, and got
    /// a new password.
    Recovered,
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

/// Makes sure the player has an account on this server that signs in.
///
/// * `saved`: the account saved for this server. Kept if it signs in; if its
///   password is refused, the identity key may still sign in to it.
/// * `names`: the player's names on other servers, tried with the identity
///   key (their account here from another PC), and used to name a new one.
///
/// Otherwise a new account with a random password, named after the player's
/// usual name or `nick`. A password is only ever sent to the server it was
/// made for: each server gets its own.
pub fn ensure_account(service: &dyn AccountService, saved: Option<(&str, &str)>, names: &[&str], nick: &str) -> Result<(String, String, Outcome), AccountError> {
    let saved = saved.filter(|(u, p)| !u.is_empty() && !p.is_empty());
    if let Some((username, password)) = saved {
        match service.login(username, password) {
            Ok(()) => return Ok((username.into(), password.into(), Outcome::Existing)),
            Err(AccountError::WrongPassword) => {
                let password = new_password();
                return match service.key_login(username, &password) {
                    Ok(()) => Ok((username.into(), password, Outcome::Recovered)),
                    Err(_) => Err(AccountError::WrongPassword),
                };
            }
            // The account is gone from this server: make a new one below.
            Err(AccountError::NotFound) => {}
            Err(e) => return Err(e),
        }
    }
    let mut tried: Vec<String> = Vec::new();
    for name in saved.map(|(u, _)| u).into_iter().chain(names.iter().copied()).chain([nick]) {
        let name = name.trim();
        if name.is_empty() || tried.iter().any(|t| t.eq_ignore_ascii_case(name)) || tried.len() >= 5 {
            continue;
        }
        tried.push(name.to_string());
        let password = new_password();
        if service.key_login(name, &password).is_ok() {
            return Ok((name.to_string(), password, Outcome::Recovered));
        }
    }
    let base = saved.map(|(u, _)| u).or_else(|| names.first().copied()).filter(|n| !n.trim().is_empty()).unwrap_or(nick);
    let (username, password) = create_account(service, base)?;
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
    struct FakeServer {
        accounts: RefCell<HashMap<String, String>>,
        /// Accounts linked to the player's identity.
        linked: Vec<String>,
    }

    impl AccountService for FakeServer {
        fn login(&self, username: &str, password: &str) -> Result<(), AccountError> {
            match self.accounts.borrow().get(username) {
                None => Err(AccountError::NotFound),
                Some(p) if p == password => Ok(()),
                Some(_) => Err(AccountError::WrongPassword),
            }
        }

        fn register(&self, username: &str, password: &str) -> Result<(), AccountError> {
            let mut accounts = self.accounts.borrow_mut();
            if accounts.contains_key(username) {
                return Err(AccountError::Taken);
            }
            accounts.insert(username.into(), password.into());
            Ok(())
        }

        fn key_login(&self, username: &str, new_password: &str) -> Result<(), AccountError> {
            if !self.linked.iter().any(|l| l == username) {
                return Err(AccountError::NotFound);
            }
            self.accounts.borrow_mut().insert(username.into(), new_password.into());
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
    fn keeps_or_creates_and_never_reuses_a_password() {
        let server = FakeServer::default();
        server.register("Kiwi", "kiwi-password").unwrap();

        let (u, _, how) = ensure_account(&server, Some(("Kiwi", "kiwi-password")), &[], "ignored").unwrap();
        assert_eq!((u.as_str(), how), ("Kiwi", Outcome::Existing));

        // The name from another server names the new account, with a password of its own.
        let (u, p, how) = ensure_account(&server, None, &["Tank"], "ignored").unwrap();
        assert_eq!((u.as_str(), how), ("Tank", Outcome::Created));
        assert_ne!(p, "tank-password");
        assert!(server.login("Tank", &p).is_ok());

        // A wrong password is reported, not papered over with a new account.
        assert_eq!(ensure_account(&server, Some(("Kiwi", "wrong")), &[], "Kiwi"), Err(AccountError::WrongPassword));

        // No saved account: a new one, with the next free name.
        let (u, p, how) = ensure_account(&server, None, &[], "Kiwi").unwrap();
        assert_eq!((u.as_str(), how), ("Kiwi2", Outcome::Created));
        assert!(server.login(&u, &p).is_ok());
    }

    #[test]
    fn the_identity_key_recovers_an_account() {
        let server = FakeServer {
            linked: vec!["Kiwi".into()],
            ..FakeServer::default()
        };
        server.register("Kiwi", "made-on-another-pc").unwrap();

        // A new PC with the identity imported, and the player's usual name.
        let (u, p, how) = ensure_account(&server, None, &["Kiwi"], "Someone").unwrap();
        assert_eq!((u.as_str(), how), ("Kiwi", Outcome::Recovered));
        assert!(server.login("Kiwi", &p).is_ok(), "the account has the new password");

        // A saved password that stopped working.
        let (_, _, how) = ensure_account(&server, Some(("Kiwi", "old")), &[], "Kiwi").unwrap();
        assert_eq!(how, Outcome::Recovered);
    }
}
