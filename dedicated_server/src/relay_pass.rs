//! Relay passes (global matchmaking, G1 item 5). Players sign in to the network's matchmaking
//! server, but their games may use a relay near them on another machine, which can't check the
//! matchmaking server's own tickets (its `nat.key` stays with it). So the matchmaking server
//! also signs a pass at sign-in, Ed25519 over the player's name and the day, with a key kept
//! in [`KEY_FILE`]; a relay that trusts that key (`[role] ticket_key`, its global-id form, which
//! the matchmaking server logs at start) answers a pass with its own ticket for the name
//! (nat_proto's TicketPresent and TicketGrant), and the game's probes carry that as usual.
//!
//! A pass is good for the day it was signed and the next six, as tickets are. Checking one costs
//! a signature check, so a helper checks at most [`CHECKS_PER_SECOND`].

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

/// The matchmaking server's signing key, next to the database.
pub const KEY_FILE: &str = "relay-pass.key";
/// Days a pass works: the day it was signed and the next six.
const PASS_DAYS: u32 = 7;
/// The most passes a helper checks a second; the rest wait for the game's next try.
const CHECKS_PER_SECOND: u64 = 200;

static ISSUER: OnceLock<identity::Identity> = OnceLock::new();
static ACCEPTED: OnceLock<Vec<String>> = OnceLock::new();

/// What a pass signs.
fn message(name: &str, day: u32) -> String {
    format!("fes-relay-pass-v1\n{}\n{day}", identity::name_key(name))
}

fn today() -> u32 {
    u32::try_from(identity::now().max(0) / 86_400).unwrap_or(u32::MAX)
}

/// Signs passes from now on, with the key in [`KEY_FILE`] (made on first use): the network's
/// matchmaking server. Its key, in the form relays are told to trust.
pub fn start_issuing() -> eyre::Result<String> {
    let key = crate::keys::load_or_create(std::path::Path::new(KEY_FILE))?;
    let issuer = identity::Identity::from_secret(&key.0);
    let id = issuer.global_id();
    let _ = ISSUER.set(issuer);
    Ok(id)
}

/// The keys whose passes this helper takes: the matchmaking server's own (when this is it) and
/// the one its operator set (`[role] ticket_key`). Called once at start.
pub fn accept(configured: Option<&str>) {
    let mut keys: Vec<String> = ISSUER.get().map(identity::Identity::global_id).into_iter().collect();
    keys.extend(configured.map(str::trim).filter(|k| identity::is_global_id(k)).map(String::from));
    let _ = ACCEPTED.set(keys);
}

/// `name`'s pass for today, for the sign-in answer (4 bytes of day, 64 of signature); None
/// when this server doesn't sign them.
pub fn issue(name: &str) -> Option<Vec<u8>> {
    let issuer = ISSUER.get()?;
    let day = today();
    let signature = identity::base32_decode(&issuer.sign(&message(name, day)))?;
    let mut pass = day.to_be_bytes().to_vec();
    pass.extend_from_slice(&signature);
    Some(pass)
}

/// Whether `signature` is a pass for `name` signed on `day` with a key this helper takes, and
/// still good today.
pub fn valid(name: &str, day: u32, signature: &[u8; 64]) -> bool {
    valid_on(name, day, signature, today())
}

fn valid_on(name: &str, day: u32, signature: &[u8; 64], today: u32) -> bool {
    let Some(keys) = ACCEPTED.get().filter(|k| !k.is_empty()) else { return false };
    if name.is_empty() || day > today || today - day >= PASS_DAYS || !may_check() {
        return false;
    }
    let message = message(name, day);
    let signature = identity::base32_encode(signature);
    keys.iter().any(|key| identity::verify(key, &message, &signature))
}

/// Whether one more check fits this second's [`CHECKS_PER_SECOND`].
fn may_check() -> bool {
    // The second in the high bits, the count in the low 20.
    static WINDOW: AtomicU64 = AtomicU64::new(0);
    let second = u64::try_from(identity::now().max(0)).unwrap_or(0);
    let mut current = WINDOW.load(Ordering::Relaxed);
    loop {
        let (at, count) = (current >> 20, current & 0xf_ffff);
        let next = if at == second {
            if count >= CHECKS_PER_SECOND {
                return false;
            }
            current + 1
        } else {
            (second << 20) | 1
        };
        match WINDOW.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(seen) => current = seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pass signed by a trusted key holds for its name, from its day for a week; another
    /// name, a later day, an old pass or an untrusted key's doesn't.
    #[test]
    fn a_pass_holds_for_its_name_and_week_and_trusted_key() {
        let matchmaking = identity::Identity::from_secret(&[7; 32]);
        let stranger = identity::Identity::from_secret(&[8; 32]);
        let _ = ACCEPTED.set(vec![matchmaking.global_id()]);
        let sign = |who: &identity::Identity, name: &str, day: u32| -> [u8; 64] { identity::base32_decode(&who.sign(&message(name, day))).unwrap().try_into().unwrap() };
        let day = 20_400;
        let pass = sign(&matchmaking, "Kiwi", day);
        assert!(valid_on("Kiwi", day, &pass, day));
        assert!(valid_on("kiwi", day, &pass, day + 6), "the same name, any case, the seventh day");
        assert!(!valid_on("Kiwi", day, &pass, day + 7), "a week old");
        assert!(!valid_on("Kiwi", day, &pass, day - 1), "dated ahead");
        assert!(!valid_on("Tank", day, &pass, day), "another name");
        assert!(!valid_on("Kiwi", day + 1, &pass, day + 1), "another day than signed");
        assert!(!valid_on("Kiwi", day, &sign(&stranger, "Kiwi", day), day), "a key not trusted");
    }
}
