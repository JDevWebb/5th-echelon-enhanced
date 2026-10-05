//! Player management from the coordinator's admin UI: the actions an admin takes on a
//! player (ban, kick, a new password, rename, delete), which reach this server with the
//! coordinator's answer to its pulse (`federation.rs`), and which players changed since
//! the roster last went out.

use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::Mutex;
use std::sync::OnceLock;

use serde::Deserialize;
use serde::Serialize;

use crate::storage::Ban;
use crate::storage::Storage;

/// What a banned player is told when signing in.
pub fn banned_message(ban: &Ban) -> String {
    let how_long = match ban.until {
        None => "for good".to_string(),
        Some(until) => {
            let left = until - identity::now();
            let days = (left + 86_399) / 86_400;
            if days > 1 {
                format!("for {days} more days")
            } else {
                "for less than a day more".to_string()
            }
        }
    };
    if ban.reason.is_empty() {
        format!("This account is banned on this server {how_long}.")
    } else {
        format!("This account is banned on this server {how_long}: {}", ban.reason)
    }
}

/// Players whose roster entry changed since it last went to the coordinator.
fn changed_set() -> &'static Mutex<HashSet<u32>> {
    static CHANGED: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    CHANGED.get_or_init(Mutex::default)
}

/// `user_id`'s roster entry changed (they signed in or out): it goes with the roster, sent
/// within seconds ([`crate::federation::roster_soon`]).
pub fn changed(user_id: u32) {
    changed_set().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(user_id);
    crate::federation::roster_soon();
}

/// The players changed since the last call.
pub fn take_changed() -> HashSet<u32> {
    std::mem::take(&mut *changed_set().lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Puts players back for the next roster when sending it failed.
pub fn changed_again(users: impl IntoIterator<Item = u32>) {
    changed_set().lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend(users);
}

static SIGN_OUTS: OnceLock<Mutex<mpsc::Sender<u32>>> = OnceLock::new();

/// The game service's end of the sign-out queue (`Server::sign_outs`); taken once.
pub fn sign_outs() -> mpsc::Receiver<u32> {
    let (sender, receiver) = mpsc::channel();
    let _ = SIGN_OUTS.set(Mutex::new(sender));
    receiver
}

/// Closes `user_id`'s game connections (within a second).
pub fn sign_out(user_id: u32) {
    if let Some(sender) = SIGN_OUTS.get() {
        let _ = sender.lock().unwrap_or_else(std::sync::PoisonError::into_inner).send(user_id);
    }
}

/// An action an admin took in the coordinator's admin UI.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Action {
    pub id: u64,
    pub kind: String,
    pub player: u32,
    #[serde(default)]
    pub reason: Option<String>,
    /// For a ban: Unix seconds; none for good.
    #[serde(default)]
    pub until: Option<i64>,
    /// For a rename.
    #[serde(default)]
    pub name: Option<String>,
}

/// What came of an action, for the admin.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Outcome {
    pub ok: bool,
    pub message: String,
    /// The new password, for a reset: shown to the admin once.
    pub password: Option<String>,
}

impl Outcome {
    fn done(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            password: None,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            password: None,
        }
    }
}

/// The most of a ban reason kept.
const MAX_REASON: usize = 200;

/// A password for a reset: 16 letters and digits, easy to read out.
fn new_password() -> String {
    use rand::Rng as _;
    const CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    (0..16).map(|_| CHARS[rng.random_range(0..CHARS.len())] as char).collect()
}

/// Carries out an admin's action.
pub async fn perform(logger: &slog::Logger, storage: &Storage, action: &Action) -> Outcome {
    let user = action.player;
    let Ok(Some(person)) = storage.find_person(user).await else {
        return Outcome::failed("No such player on this server");
    };
    if !storage.is_player_account(user).await.unwrap_or(false) {
        return Outcome::failed("That's one of the server's own accounts");
    }
    let name = person.username.clone();
    let outcome = match action.kind.as_str() {
        "kick" => {
            sign_out(user);
            Outcome::done(format!("Signed {name} out"))
        }
        "ban" => {
            let reason: String = action
                .reason
                .as_deref()
                .unwrap_or_default()
                .trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(MAX_REASON)
                .collect();
            // A ban that ended before it got here (a late action) isn't made permanent.
            if action.until.is_some_and(|u| u <= identity::now()) {
                return Outcome::failed(format!("The ban on {name} had already ended"));
            }
            match storage.ban(user, &reason, action.until).await {
                Ok(()) => {
                    // Signed out everywhere: the game now, the launcher's tokens too.
                    let _ = storage.new_token_epoch(user).await;
                    sign_out(user);
                    Outcome::done(format!("Banned {name}"))
                }
                Err(e) => Outcome::failed(format!("Couldn't ban {name}: {e}")),
            }
        }
        "unban" => match storage.unban(user).await {
            Ok(true) => Outcome::done(format!("{name} can sign in again")),
            Ok(false) => Outcome::done(format!("{name} wasn't banned")),
            Err(e) => Outcome::failed(format!("Couldn't unban {name}: {e}")),
        },
        "reset_password" => {
            let password = new_password();
            match storage.set_password(user, &password).await {
                Ok(()) => Outcome {
                    ok: true,
                    message: format!("{name} has a new password"),
                    password: Some(password),
                },
                Err(e) => Outcome::failed(format!("Couldn't set a new password: {e}")),
            }
        }
        "rename" => {
            let new_name = action.name.as_deref().unwrap_or_default().trim().to_string();
            if let Err(why) = crate::api::check_username(&new_name) {
                return Outcome::failed(why);
            }
            let clash = storage.find_person_by_name(&new_name).await.ok().flatten().is_some_and(|p| p.id != user)
                || storage.find_person_by_ubi_id(&new_name).await.ok().flatten().is_some_and(|p| p.id != user);
            if clash {
                return Outcome::failed("Another player here has that name");
            }
            match storage.rename_user(user, &new_name).await {
                Ok(Ok(())) => {
                    // The game shows the name it signed in with: sign them out to show the new one.
                    sign_out(user);
                    Outcome::done(format!("{name} is now {new_name}"))
                }
                Ok(Err(_)) => Outcome::failed("Another player here has that name"),
                Err(e) => Outcome::failed(format!("Couldn't rename {name}: {e}")),
            }
        }
        "delete" => {
            sign_out(user);
            let global_id = person.global_id.clone();
            match storage.delete_user_async(user).await {
                Ok(()) => {
                    if let Some(global_id) = global_id {
                        crate::federation::record(logger, storage, crate::federation::Change::Unlink { global_id }).await;
                    }
                    Outcome::done(format!("Deleted {name}"))
                }
                Err(e) => Outcome::failed(format!("Couldn't delete {name}: {e}")),
            }
        }
        other => Outcome::failed(format!("This server doesn't know the action {other:?}; update it")),
    };
    if outcome.ok {
        warn!(logger, "Admin action {} on {name} ({user}): {}", action.kind, outcome.message);
        changed(user);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ban_messages_say_how_long() {
        let now = identity::now();
        let ban = |reason: &str, until| Ban {
            reason: reason.into(),
            until,
            at: now,
        };
        assert_eq!(banned_message(&ban("cheating", None)), "This account is banned on this server for good: cheating");
        assert_eq!(banned_message(&ban("", Some(now + 3 * 86_400))), "This account is banned on this server for 3 more days.");
        assert!(banned_message(&ban("", Some(now + 60))).contains("less than a day"));
    }

    #[test]
    fn new_passwords_are_long_and_plain() {
        let p = new_password();
        assert_eq!(p.len(), 16);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric() && !"0O1lI".contains(c)));
        assert_ne!(p, new_password());
    }

    #[test]
    fn admins_actions_are_carried_out_here() {
        use serde_json::json;

        use crate::storage::run;
        use crate::storage::tests::temp_storage;
        let (s, dir) = temp_storage("admin-actions");
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        let user = |name: &str, ubi: Option<&str>| {
            s.register_user(name, "password1", ubi).unwrap();
            s.find_user_id_by_name(name).unwrap().unwrap()
        };
        let (kiwi, tank, own) = (user("Kiwi", Some("Kiwi")), user("Tank", Some("Tank")), user("Bot", None));
        let act = |json: serde_json::Value| -> Outcome {
            let action: Action = serde_json::from_value(json).unwrap();
            run(perform(&logger, &s, &action)).unwrap()
        };
        let now = identity::now();

        // Bans: for a while, or for good; one that ended before it got here isn't made one.
        let o = act(json!({ "id": 1, "kind": "ban", "player": kiwi, "reason": "cheating\u{7}", "until": now + 3600 }));
        assert!(o.ok && s.banned(kiwi).unwrap(), "{}", o.message);
        let o = act(json!({ "id": 2, "kind": "ban", "player": tank, "reason": "", "until": now - 1 }));
        assert!(!o.ok && !s.banned(tank).unwrap(), "{}", o.message);
        let o = act(json!({ "id": 3, "kind": "unban", "player": kiwi }));
        assert!(o.ok && !s.banned(kiwi).unwrap(), "{}", o.message);
        assert_eq!(act(json!({ "id": 4, "kind": "unban", "player": kiwi })).message, "Kiwi wasn't banned");

        // A new password: the old one stops working, the new one (shown once) works.
        let o = act(json!({ "id": 5, "kind": "reset_password", "player": kiwi }));
        let password = o.password.clone().expect("the new password");
        assert!(o.ok && s.login_user("Kiwi", "password1").unwrap().is_err(), "{}", o.message);
        assert_eq!(s.login_user("Kiwi", &password).unwrap().ok(), Some(kiwi));

        // Renames: to a free, valid name only.
        assert!(!act(json!({ "id": 6, "kind": "rename", "player": kiwi, "name": "tank" })).ok, "another player's name");
        assert!(
            !act(json!({ "id": 7, "kind": "rename", "player": kiwi, "name": "no spaces allowed" })).ok,
            "an invalid name"
        );
        let o = act(json!({ "id": 8, "kind": "rename", "player": kiwi, "name": "Kiwi2" }));
        assert!(o.ok, "{}", o.message);
        assert_eq!(s.find_user_id_by_name("Kiwi2").unwrap(), Some(kiwi));

        // The server's own accounts, and players it doesn't have, are left alone.
        assert_eq!(
            act(json!({ "id": 9, "kind": "ban", "player": own, "reason": "" })).message,
            "That's one of the server's own accounts"
        );
        assert!(!s.banned(own).unwrap());
        assert_eq!(act(json!({ "id": 10, "kind": "kick", "player": 999_999 })).message, "No such player on this server");
        assert!(!act(json!({ "id": 11, "kind": "explode", "player": tank })).ok, "an action there isn't");

        // Deleting: the account is gone.
        let o = act(json!({ "id": 12, "kind": "delete", "player": tank }));
        assert!(o.ok, "{}", o.message);
        assert_eq!(s.find_user_id_by_name("Tank").unwrap(), None);
        assert!(run(s.find_person(tank)).unwrap().unwrap().is_none());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn actions_read_as_the_coordinator_sends_them() {
        let a: Action = serde_json::from_str(r#"{"id":17,"kind":"ban","player":1007,"reason":"cheating","until":null,"name":null}"#).unwrap();
        assert_eq!((a.id, a.kind.as_str(), a.player, a.until), (17, "ban", 1007, None));
        let a: Action = serde_json::from_str(r#"{"id":18,"kind":"kick","player":1007}"#).unwrap();
        assert_eq!(a.reason, None);
    }
}
