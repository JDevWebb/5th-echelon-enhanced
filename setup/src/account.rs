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
    /// Makes an account, linked to the player's identity.
    fn register(&self, username: &str, password: &str) -> Result<(), AccountError>;
    /// Signs in with the player's identity key (see `player_identity`) to
    /// whichever account on this server is linked to it, giving it
    /// `new_password`: its name, or None when the identity has no account
    /// here.
    fn identity_login(&self, new_password: &str) -> Result<Option<String>, AccountError>;
}

/// How [`find_account`] found the player's account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The saved account signed in.
    Existing,
    /// The account linked to the player's identity (made on another PC, or
    /// whose password was lost) signed in with their identity key, and got a
    /// new password.
    Recovered,
    /// A new account was made.
    Created,
}

/// A username for a new account from `nick` (the player's chosen name, or
/// their Windows user name): letters A-Z, digits, `_`, `-` and `.` (what
/// servers accept), spaces as `_`, at most 24 characters, "Agent" if nothing
/// is left.
pub fn account_name(nick: &str) -> String {
    let name: String = nick
        .trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('_'),
            c if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') => Some(c),
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

/// A random password: 24 lowercase letters and digits. Players never type
/// it: the launcher keeps it, and their identity replaces it when lost.
pub fn new_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut rng = rand::rng();
    (0..24).map(|_| char::from(ALPHABET[rng.random_range(0..ALPHABET.len())])).collect()
}

/// The player's account on this server, without asking them: the saved one
/// if it still signs in, else the one linked to their identity (with a new
/// password). None when they have none here yet: they choose a name, and
/// [`create_account`] makes it.
///
/// A password is only ever sent to the server it was made for.
pub fn find_account(service: &dyn AccountService, saved: Option<(&str, &str)>) -> Result<Option<(String, String, Outcome)>, AccountError> {
    if let Some((username, password)) = saved.filter(|(u, p)| !u.is_empty() && !p.is_empty()) {
        match service.login(username, password) {
            Ok(()) => return Ok(Some((username.into(), password.into(), Outcome::Existing))),
            // A password that stopped working, or an account that's gone: the identity knows.
            Err(AccountError::WrongPassword | AccountError::NotFound) => {}
            Err(e) => return Err(e),
        }
    }
    let password = new_password();
    Ok(service.identity_login(&password)?.map(|username| (username, password, Outcome::Recovered)))
}

/// Makes an account named `name` (as [`account_name`] cleans it), linked to
/// the player's identity, with a random password. `Taken` when someone has
/// the name: the player chooses another.
pub fn create_account(service: &dyn AccountService, name: &str) -> Result<(String, String), AccountError> {
    let username = account_name(name);
    let password = new_password();
    service.register(&username, &password)?;
    Ok((username, password))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct FakeServer {
        accounts: RefCell<HashMap<String, String>>,
        /// The account linked to the player's identity, if any.
        linked: RefCell<Option<String>>,
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
            *self.linked.borrow_mut() = Some(username.into());
            Ok(())
        }

        fn identity_login(&self, new_password: &str) -> Result<Option<String>, AccountError> {
            let linked = self.linked.borrow().clone();
            if let Some(name) = &linked {
                self.accounts.borrow_mut().insert(name.clone(), new_password.into());
            }
            Ok(linked)
        }
    }

    #[test]
    fn names() {
        assert_eq!(account_name("  Sam Fisher! "), "Sam_Fisher");
        assert_eq!(account_name("???"), "Agent");
        assert_eq!(account_name("Zoë Kiwi"), "Zo_Kiwi", "ASCII only");
        assert_eq!(account_name(&"x".repeat(40)).len(), 24);
        let pw = new_password();
        assert_eq!(pw.len(), 24);
        assert!(pw.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }

    #[test]
    fn a_new_player_has_no_account_until_they_name_one() {
        let server = FakeServer::default();
        assert_eq!(find_account(&server, None), Ok(None));
        let (u, p) = create_account(&server, "Sam Fisher").unwrap();
        assert_eq!(u, "Sam_Fisher");
        assert!(server.login(&u, &p).is_ok());
        // Someone else's name is refused, not changed behind the player's back.
        let other = FakeServer::default();
        other.accounts.borrow_mut().insert("Kiwi".into(), "theirs-password".into());
        assert_eq!(create_account(&other, "Kiwi"), Err(AccountError::Taken));
    }

    #[test]
    fn the_saved_account_or_the_identity_finds_it() {
        let server = FakeServer::default();
        let (u, p) = create_account(&server, "Kiwi").unwrap();
        let (found, _, how) = find_account(&server, Some((&u, &p))).unwrap().unwrap();
        assert_eq!((found.as_str(), how), ("Kiwi", Outcome::Existing));

        // A new PC with the identity imported: found without a name.
        let (found, p2, how) = find_account(&server, None).unwrap().unwrap();
        assert_eq!((found.as_str(), how), ("Kiwi", Outcome::Recovered));
        assert!(server.login("Kiwi", &p2).is_ok(), "the account has the new password");

        // A saved password that stopped working.
        let (_, _, how) = find_account(&server, Some(("Kiwi", "old-password"))).unwrap().unwrap();
        assert_eq!(how, Outcome::Recovered);
    }
}
