//! Relay passes (global matchmaking, G1 item 5). Players sign in to the network's matchmaking
//! server, but their games may use a relay near them on another machine, which can't check the
//! matchmaking server's own tickets (its `nat.key` stays with it). So the matchmaking server
//! also signs a pass at sign-in, Ed25519 over the player's name and the day, with a key kept
//! in [`KEY_FILE`]; a relay that trusts that key (`[role] ticket_key`, its global-id form, which
//! the matchmaking server logs at start) answers a pass with its own ticket for the name
//! (nat_proto's TicketPresent and TicketGrant), and the game's probes carry that as usual.
//!
//! A pass is good for the day it was signed and the next six, as tickets are. Checking one costs
//! a signature check, so a helper checks at most [`CHECKS_PER_ADDRESS`] from one address in
//! [`ADDRESS_WINDOW`], and [`CHECKS_PER_SECOND`] in all; the game shows its pass again at its
//! next probe, so one not checked is only late.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

/// The matchmaking server's signing key, next to the database.
pub const KEY_FILE: &str = "relay-pass.key";
/// Days a pass works: the day it was signed and the next six.
const PASS_DAYS: u32 = 7;
/// The most passes a helper checks a second, from everyone (a backstop).
const CHECKS_PER_SECOND: u64 = 1000;
/// The most passes checked from one address in [`ADDRESS_WINDOW`].
const CHECKS_PER_ADDRESS: u32 = 5;
const ADDRESS_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);
/// The most addresses kept for that.
const MAX_ADDRESSES: usize = 10_000;

static ISSUER: OnceLock<identity::Identity> = OnceLock::new();
static ACCEPTED: OnceLock<Vec<String>> = OnceLock::new();

/// What a pass signs: the name and day, and from 0.5.0 the game's session key (a pass for a
/// key is only good with it: nat_proto's SessionKey).
fn message(name: &str, day: u32, key: Option<&nat_proto::SessionKey>) -> String {
    match key {
        None => format!("fes-relay-pass-v1\n{}\n{day}", identity::name_key(name)),
        Some(k) => format!("fes-relay-pass-v2\n{}\n{day}\n{}", identity::name_key(name), identity::base32_encode(k)),
    }
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
pub fn issue(name: &str, key: Option<&nat_proto::SessionKey>) -> Option<Vec<u8>> {
    let issuer = ISSUER.get()?;
    let day = today();
    let signature = identity::base32_decode(&issuer.sign(&message(name, day, key)))?;
    let mut pass = day.to_be_bytes().to_vec();
    pass.extend_from_slice(&signature);
    Some(pass)
}

/// Whether `signature` is a pass for `name` signed on `day` with a key this helper takes, and
/// still good today.
pub fn valid(name: &str, day: u32, signature: &[u8; 64], from: std::net::IpAddr, key: Option<&nat_proto::SessionKey>) -> bool {
    may_check_from(from, std::time::Instant::now()) && valid_on(name, day, signature, today(), key)
}

/// Whether `from` may have one more pass checked now ([`CHECKS_PER_ADDRESS`]).
fn may_check_from(from: std::net::IpAddr, now: std::time::Instant) -> bool {
    type Seen = std::collections::HashMap<std::net::IpAddr, (std::time::Instant, u32)>;
    static SEEN: std::sync::LazyLock<std::sync::Mutex<Seen>> = std::sync::LazyLock::new(Default::default);
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Full (forged sources can fill it: a pass needs no round trip): it starts again rather
    // than refusing every new address; CHECKS_PER_SECOND still caps the work.
    if seen.len() >= MAX_ADDRESSES && !seen.contains_key(&from) {
        seen.retain(|_, (at, _)| now.duration_since(*at) < ADDRESS_WINDOW);
        if seen.len() >= MAX_ADDRESSES {
            seen.clear();
        }
    }
    let entry = seen.entry(from).or_insert((now, 0));
    if now.duration_since(entry.0) >= ADDRESS_WINDOW {
        *entry = (now, 0);
    }
    entry.1 += 1;
    entry.1 <= CHECKS_PER_ADDRESS
}

fn valid_on(name: &str, day: u32, signature: &[u8; 64], today: u32, key: Option<&nat_proto::SessionKey>) -> bool {
    let Some(keys) = ACCEPTED.get().filter(|k| !k.is_empty()) else { return false };
    if name.is_empty() || day > today || today - day >= PASS_DAYS || !may_check() {
        return false;
    }
    let message = message(name, day, key);
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
        let sign = |who: &identity::Identity, name: &str, day: u32, key: Option<&nat_proto::SessionKey>| -> [u8; 64] {
            identity::base32_decode(&who.sign(&message(name, day, key))).unwrap().try_into().unwrap()
        };
        let day = 20_400;
        let pass = sign(&matchmaking, "Kiwi", day, None);
        assert!(valid_on("Kiwi", day, &pass, day, None));
        assert!(valid_on("kiwi", day, &pass, day + 6, None), "the same name, any case, the seventh day");
        assert!(!valid_on("Kiwi", day, &pass, day + 7, None), "a week old");
        assert!(!valid_on("Kiwi", day, &pass, day - 1, None), "dated ahead");
        assert!(!valid_on("Tank", day, &pass, day, None), "another name");
        assert!(!valid_on("Kiwi", day + 1, &pass, day + 1, None), "another day than signed");
        assert!(!valid_on("Kiwi", day, &sign(&stranger, "Kiwi", day, None), day, None), "a key not trusted");
        // A pass for a session key holds with that key only, and never as a keyless one.
        let (k1, k2) = ([1u8; 32], [2u8; 32]);
        let keyed = sign(&matchmaking, "Kiwi", day, Some(&k1));
        assert!(valid_on("Kiwi", day, &keyed, day, Some(&k1)));
        assert!(!valid_on("Kiwi", day, &keyed, day, Some(&k2)), "another key");
        assert!(!valid_on("Kiwi", day, &keyed, day, None), "without its key");
        assert!(!valid_on("Kiwi", day, &pass, day, Some(&k1)), "a keyless pass with a key");
    }

    #[test]
    fn one_address_gets_a_few_checks_at_a_time() {
        let t0 = std::time::Instant::now();
        let (a, b): (std::net::IpAddr, std::net::IpAddr) = ("198.51.100.4".parse().unwrap(), "198.51.100.5".parse().unwrap());
        for _ in 0..CHECKS_PER_ADDRESS {
            assert!(may_check_from(a, t0));
        }
        assert!(!may_check_from(a, t0), "one too many from one address");
        assert!(may_check_from(b, t0), "another address isn't held up");
        assert!(may_check_from(a, t0 + ADDRESS_WINDOW), "the next window");
    }
}
