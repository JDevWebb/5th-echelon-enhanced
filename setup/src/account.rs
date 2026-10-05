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
    /// The server refuses this launcher's version; what it says to do.
    #[error("{0}")]
    Outdated(String),
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

/// What became of a player whose identity has no account on a server yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewAccount {
    Created { username: String, password: String },
    /// The player chooses a name: `suggested` to start from, and whether it was taken there.
    NeedsName { suggested: String, taken: bool },
}

/// Makes the account for a player with none on a server: with the name they chose, or on a
/// community server (`automatic`) with their usual name, without asking (the coordinator holds
/// it for their identity, so it's free there unless an account from before names were held
/// has it). A name taken there, or no name to use, is for the player to choose.
pub fn new_account(service: &dyn AccountService, chosen: Option<&str>, usual: Option<&str>, automatic: bool) -> Result<NewAccount, AccountError> {
    let usual = usual.map(str::trim).filter(|u| !u.is_empty());
    let name = match (chosen, usual) {
        (Some(chosen), _) => chosen,
        (None, Some(usual)) if automatic => usual,
        (None, usual) => {
            return Ok(NewAccount::NeedsName {
                suggested: usual.unwrap_or_default().to_string(),
                taken: false,
            })
        }
    };
    match create_account(service, name) {
        Ok((username, password)) => Ok(NewAccount::Created { username, password }),
        Err(AccountError::Taken) => Ok(NewAccount::NeedsName {
            suggested: account_name(name),
            taken: true,
        }),
        Err(e) => Err(e),
    }
}

/// Names to offer when `name` is taken: with the player's country (`Kiwi_NZ`) when it's
/// known, then numbered (`Kiwi2`, `Kiwi3`…), each a valid account name.
pub fn name_suggestions(name: &str, country: Option<&str>) -> Vec<String> {
    let base: String = account_name(name).chars().take(21).collect();
    let country = country.map(str::trim).filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_alphabetic())).map(str::to_ascii_uppercase);
    let mut names: Vec<String> = country.iter().map(|c| format!("{base}_{c}")).collect();
    names.extend((2..=6).map(|n| format!("{base}{n}")));
    names
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
    fn a_community_server_gets_the_usual_name_without_asking() {
        let server = FakeServer::default();
        let made = new_account(&server, None, Some("Kiwi"), true).unwrap();
        assert!(matches!(&made, NewAccount::Created { username, .. } if username == "Kiwi"));
        if let NewAccount::Created { username, password } = made {
            assert!(server.login(&username, &password).is_ok());
        }
    }

    #[test]
    fn a_taken_name_or_none_asks_the_player() {
        // An account from before names were held has it.
        let server = FakeServer::default();
        server.accounts.borrow_mut().insert("Kiwi".into(), "theirs-password".into());
        assert_eq!(
            new_account(&server, None, Some("Kiwi"), true),
            Ok(NewAccount::NeedsName { suggested: "Kiwi".into(), taken: true })
        );
        // The name the player chose, taken since they checked it.
        assert_eq!(
            new_account(&server, Some("Kiwi"), None, false),
            Ok(NewAccount::NeedsName { suggested: "Kiwi".into(), taken: true })
        );
        // No usual name yet, or a server of their own: asked, with the usual name to start from.
        assert_eq!(new_account(&server, None, None, true), Ok(NewAccount::NeedsName { suggested: String::new(), taken: false }));
        assert_eq!(
            new_account(&server, None, Some("Kiwi"), false),
            Ok(NewAccount::NeedsName { suggested: "Kiwi".into(), taken: false })
        );
        // The name they chose, free: made.
        assert!(matches!(new_account(&server, Some("Kiwi_NZ"), Some("Kiwi"), false), Ok(NewAccount::Created { .. })));
    }

    #[test]
    fn suggestions_are_valid_names() {
        assert_eq!(name_suggestions("Kiwi", Some("nz"))[..3], ["Kiwi_NZ", "Kiwi2", "Kiwi3"]);
        assert_eq!(name_suggestions("Kiwi", None)[0], "Kiwi2");
        assert_eq!(name_suggestions("Kiwi", Some("Aotearoa"))[0], "Kiwi2", "not a country code");
        let long = name_suggestions(&"x".repeat(40), Some("NZ"));
        assert!(long.iter().all(|n| n.len() <= 24 && account_name(n) == *n));
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
