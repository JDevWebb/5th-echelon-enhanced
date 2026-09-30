//! A player's identity across servers.
//!
//! The launcher makes an Ed25519 key once and keeps it. Its public half, in
//! base32, is the player's global id. Signing proves the player holds it:
//!
//! * linking an account on a server to the identity ([`link_message`]), so
//!   friends follow the player to other servers that share a coordinator;
//! * signing in to an account without its password ([`login_message`]), on a
//!   new PC or when the password is lost.
//!
//! Each message names the server (its id from `/api/info`) and a time, so a
//! signature is good for one server, once, for a few minutes.

use ed25519_dalek::Signer as _;
use ed25519_dalek::Verifier as _;
use rand::RngCore as _;

/// How far a signed time may be from the server's clock, in seconds.
pub const MAX_CLOCK_SKEW: i64 = 5 * 60;

/// The player's key.
pub struct Identity {
    key: ed25519_dalek::SigningKey,
}

impl Identity {
    /// A new random identity.
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::rng().fill_bytes(&mut seed);
        Self::from_secret(&seed)
    }

    /// The identity with this secret (from [`Identity::secret`]).
    pub fn from_secret(secret: &[u8; 32]) -> Self {
        Self {
            key: ed25519_dalek::SigningKey::from_bytes(secret),
        }
    }

    /// The secret, for saving. Whoever has it *is* this player.
    pub fn secret(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    /// The public key in base32: the player's global id.
    pub fn global_id(&self) -> String {
        base32_encode(self.key.verifying_key().as_bytes())
    }

    /// Signs `message`; the signature in base32.
    pub fn sign(&self, message: &str) -> String {
        base32_encode(&self.key.sign(message.as_bytes()).to_bytes())
    }

    /// Everything a server needs to link an account to this identity.
    pub fn sign_link(&self, server_id: &str, username: &str, time: i64) -> String {
        self.sign(&link_message(server_id, username, time))
    }

    /// Everything a server needs to sign this player in with the key.
    pub fn sign_login(&self, server_id: &str, username: &str, time: i64) -> String {
        self.sign(&login_message(server_id, username, time))
    }
}

/// What a player signs to link `username` on `server_id` to their identity.
pub fn link_message(server_id: &str, username: &str, time: i64) -> String {
    format!("5th-echelon/link/v1\n{server_id}\n{username}\n{time}")
}

/// What a player signs to sign in to `username` on `server_id` with the key.
pub fn login_message(server_id: &str, username: &str, time: i64) -> String {
    format!("5th-echelon/login/v1\n{server_id}\n{username}\n{time}")
}

/// Whether `signature` (base32) is `global_id`'s signature of `message`.
pub fn verify(global_id: &str, message: &str, signature: &str) -> bool {
    let (Some(key), Some(sig)) = (base32_decode(global_id), base32_decode(signature)) else {
        return false;
    };
    let (Ok(key), Ok(sig)) = (<[u8; 32]>::try_from(key.as_slice()), <[u8; 64]>::try_from(sig.as_slice())) else {
        return false;
    };
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&key) else {
        return false;
    };
    key.verify(message.as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig)).is_ok()
}

/// The form of a player name that decides whether two names are the same:
/// "Kiwi" and "kiwi" are one name, on a server and across servers that
/// share friends.
pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Whether a signed `time` is close enough to `now` (both Unix seconds).
pub fn fresh(time: i64, now: i64) -> bool {
    (now - time).abs() <= MAX_CLOCK_SKEW
}

/// Whether `s` looks like a global id (52 base32 characters, 32 bytes).
pub fn is_global_id(s: &str) -> bool {
    s.len() == 52 && base32_decode(s).is_some_and(|b| b.len() == 32)
}

/// A short form of a global id for people to compare, e.g. "K7QF-2M9D".
pub fn short(global_id: &str) -> String {
    let head: String = global_id.chars().take(8).collect();
    match head.len() {
        8 => format!("{}-{}", &head[..4], &head[4..]),
        _ => head,
    }
}

/// The current time in Unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, without padding.
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Decodes [`base32_encode`]'s output (either case); None if it isn't.
pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let value = ALPHABET.iter().position(|&a| a == c.to_ascii_uppercase())?;
        buffer = (buffer << 5) | u32::try_from(value).ok()?;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    // Leftover bits must be padding (zero), or the text wasn't ours.
    ((buffer & ((1 << bits) - 1)) == 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_round_trips() {
        for len in 0..40 {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base32_decode(&base32_encode(&data)).unwrap(), data);
        }
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert!(base32_decode("not base32!").is_none());
    }

    #[test]
    fn signatures_bind_server_name_and_time() {
        let me = Identity::generate();
        let id = me.global_id();
        assert!(is_global_id(&id));
        let sig = me.sign_link("server-a", "Kiwi", 1000);
        assert!(verify(&id, &link_message("server-a", "Kiwi", 1000), &sig));
        assert!(!verify(&id, &link_message("server-b", "Kiwi", 1000), &sig), "another server");
        assert!(!verify(&id, &link_message("server-a", "Tank", 1000), &sig), "another account");
        assert!(!verify(&id, &link_message("server-a", "Kiwi", 1001), &sig), "another time");
        assert!(!verify(&id, &login_message("server-a", "Kiwi", 1000), &sig), "a link isn't a login");
        let other = Identity::generate();
        assert!(!verify(&other.global_id(), &link_message("server-a", "Kiwi", 1000), &sig));
        assert!(!verify("garbage", "x", &sig));
    }

    #[test]
    fn secret_restores_the_same_identity() {
        let me = Identity::generate();
        assert_eq!(Identity::from_secret(&me.secret()).global_id(), me.global_id());
        assert_eq!(short("ABCDEFGHIJ"), "ABCD-EFGH");
        assert!(fresh(100, 100 + MAX_CLOCK_SKEW) && !fresh(100, 101 + MAX_CLOCK_SKEW));
    }
}
