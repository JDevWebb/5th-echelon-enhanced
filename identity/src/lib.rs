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
//! Each message names the server by the host the player connected to (as
//! they typed it, never what a server says about itself) and a time, so a
//! signature is good for one server, once, for a few minutes: a server that
//! claims to be another can't use what players sign for it anywhere else.

use ed25519_dalek::Signer as _;
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
    pub fn sign_link(&self, host: &str, username: &str, time: i64) -> String {
        self.sign(&link_message(host, username, time))
    }

    /// Everything a server needs to sign this player in with the key (and
    /// set `new_password`, empty for none).
    pub fn sign_login(&self, host: &str, username: &str, time: i64, new_password: &str) -> String {
        self.sign(&login_message(host, username, time, new_password))
    }
}

/// The form of a server's host name that is signed: trimmed, lower-cased,
/// without a port.
pub fn host_key(host: &str) -> String {
    let host = host.trim().to_lowercase();
    // "name:port" (not an IPv6 address, which has more than one colon).
    match host.rsplit_once(':') {
        Some((name, port)) if !name.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => name.to_string(),
        _ => host,
    }
}

/// Whether a field may go into a signed message: printable ASCII, no
/// separators, so no two messages read the same.
pub fn valid_field(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.bytes().all(|b| b.is_ascii_graphic())
}

/// What a player signs to link `username` on the server they reached as
/// `host` to their identity.
pub fn link_message(host: &str, username: &str, time: i64) -> String {
    format!("5th-echelon/link/v2\n{}\n{username}\n{time}", host_key(host))
}

/// What a player signs to sign in to `username` on `host` with the key, and
/// set `new_password` (a hash of it: the signature then can't be reused to set
/// another).
pub fn login_message(host: &str, username: &str, time: i64, new_password: &str) -> String {
    let password = if new_password.is_empty() {
        String::from("-")
    } else {
        base32_encode(&sha256(new_password.as_bytes()))
    };
    format!("5th-echelon/login/v2\n{}\n{username}\n{time}\n{password}", host_key(host))
}

/// What a player signs to send a suggestion for the roadmap (`POST /v1/suggestions`): when,
/// and a hash of exactly what was sent, so the signature can't be moved to other text.
/// `area` and `title` are one line each.
pub fn suggestion_message(time: i64, area: &str, title: &str, text: &str) -> String {
    let sent = base32_encode(&sha256(format!("{area}\n{title}\n{text}").as_bytes()));
    format!("5th-echelon/suggestion/v1\n{time}\n{sent}")
}

/// What a player signs to read their own suggestions back (`GET /v1/suggestions/mine`).
pub fn suggestions_message(time: i64) -> String {
    format!("5th-echelon/suggestions/v1\n{time}")
}

/// What a player signs to read their own reports back, with the admins' replies
/// (`GET /v1/reports/mine`).
pub fn reports_message(time: i64) -> String {
    format!("5th-echelon/reports/v1\n{time}")
}

/// What the release key signs: a release's version (its tag without the
/// `v`, e.g. `0.4.0`) and its `SHA256SUMS`, as published. With the version
/// signed, a release published again under another tag doesn't verify, so
/// nothing can be made to install an old release as a new one.
///
/// The shell verifiers (`scripts/install-server.sh` and the updater it
/// writes) build the same text with `printf '5th-echelon/release/v2\n%s\n'`
/// followed by the file.
pub fn release_message(version: &str, sums: &str) -> String {
    format!("5th-echelon/release/v2\n{version}\n{sums}")
}

/// Whether `version` can be a release's version: digits first, then only
/// letters, digits, `.` and `-` (no `v`, no separators), at most 32 bytes.
pub fn valid_release_version(version: &str) -> bool {
    (1..=32).contains(&version.len()) && version.starts_with(|c: char| c.is_ascii_digit()) && version.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

/// The public halves of the release keys: a release is installed (by the
/// launcher, the installer and the servers' updater) only when one of them
/// signed its `SHA256SUMS`. The private key stays offline
/// (`scripts/sign-release.sh`); install-server.sh carries the same key.
pub const RELEASE_KEYS: &[&str] = &["GV7WV5QTTXHFGYKWKSHRFWNDEW3N2JZDUS2GQEEQ7Q4HVHMKBCZA"];

/// Whether one of the release keys signed `sums` (a release's `SHA256SUMS`)
/// as release `version` (the tag it was published under, without the `v`).
pub fn release_signed(version: &str, sums: &str, signature: &str) -> bool {
    if !valid_release_version(version) {
        return false;
    }
    let message = release_message(version, sums);
    RELEASE_KEYS.iter().any(|key| verify(key, &message, signature.trim()))
}

fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(data).into()
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
    // Strict: no weak (small-order) keys, and no signatures that aren't canonical.
    !key.is_weak() && key.verify_strict(message.as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig)).is_ok()
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

/// Whether `s` is a global id: 52 upper-case base32 characters (one spelling
/// per key), a valid key that isn't a weak one.
pub fn is_global_id(s: &str) -> bool {
    if s.len() != 52 || s.bytes().any(|b| b.is_ascii_lowercase()) {
        return false;
    }
    let Some(Ok(bytes)) = base32_decode(s).map(<[u8; 32]>::try_from) else {
        return false;
    };
    ed25519_dalek::VerifyingKey::from_bytes(&bytes).is_ok_and(|k| !k.is_weak())
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
    #[test]
    fn a_suggestion_signature_covers_its_text() {
        let me = Identity::generate();
        let message = suggestion_message(1_000, "Launcher", "Chat", "A chat to find players.");
        let signature = me.sign(&message);
        assert!(verify(&me.global_id(), &message, &signature));
        // Other text, another time or another area: the same signature doesn't verify.
        for other in [
            suggestion_message(1_000, "Launcher", "Chat", "A chat to find players!"),
            suggestion_message(1_001, "Launcher", "Chat", "A chat to find players."),
            suggestion_message(1_000, "Overlay", "Chat", "A chat to find players."),
        ] {
            assert!(!verify(&me.global_id(), &other, &signature));
        }
        assert_ne!(suggestions_message(1_000), message);
    }

    use super::*;

    #[test]
    fn release_signatures_bind_the_version() {
        let key = Identity::generate();
        let id = key.global_id();
        let sums = "abc  launcher.exe\n";
        let sig = key.sign(&release_message("0.4.0", sums));
        assert!(verify(&id, &release_message("0.4.0", sums), &sig));
        assert!(!verify(&id, &release_message("0.4.1", sums), &sig), "the same files under another version");
        assert!(!verify(&id, &release_message("0.4.0", "abd  launcher.exe\n"), &sig), "other files");
        assert_eq!(
            release_message("0.4.0", sums),
            "5th-echelon/release/v2\n0.4.0\nabc  launcher.exe\n",
            "what the shell verifiers build"
        );
        for v in ["0.4.0", "1.0.0-rc.1", "10.20.30"] {
            assert!(valid_release_version(v), "{v}");
        }
        for v in ["", "v0.4.0", "0.4.0\n", "0.4.0 x", "-1", &"1".repeat(33)] {
            assert!(!valid_release_version(v), "{v:?}");
        }
        // Not one of RELEASE_KEYS, and an invalid version never verifies.
        assert!(!release_signed("0.4.0", sums, &sig));
        assert!(!release_signed("v0.4.0", sums, &sig));
    }

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
        assert!(!is_global_id(&id.to_lowercase()), "one spelling per key");
        let sig = me.sign_link("server-a", "Kiwi", 1000);
        assert!(verify(&id, &link_message("server-a", "Kiwi", 1000), &sig));
        assert!(verify(&id, &link_message("SERVER-A:80", "Kiwi", 1000), &sig), "the same host however it's written");
        assert!(!verify(&id, &link_message("server-b", "Kiwi", 1000), &sig), "another server");
        assert!(!verify(&id, &link_message("server-a", "Tank", 1000), &sig), "another account");
        assert!(!verify(&id, &link_message("server-a", "Kiwi", 1001), &sig), "another time");
        assert!(!verify(&id, &login_message("server-a", "Kiwi", 1000, ""), &sig), "a link isn't a login");
        let other = Identity::generate();
        assert!(!verify(&other.global_id(), &link_message("server-a", "Kiwi", 1000), &sig));
        assert!(!verify("garbage", "x", &sig));
    }

    #[test]
    fn a_key_login_signs_the_new_password() {
        let me = Identity::generate();
        let sig = me.sign_login("server-a", "Kiwi", 5, "mine-12345");
        assert!(verify(&me.global_id(), &login_message("server-a", "Kiwi", 5, "mine-12345"), &sig));
        assert!(!verify(&me.global_id(), &login_message("server-a", "Kiwi", 5, "attackers"), &sig), "another password");
        assert!(!verify(&me.global_id(), &login_message("server-a", "Kiwi", 5, ""), &sig));
    }

    #[test]
    fn hosts_and_fields() {
        assert_eq!(host_key(" Play.Example.org:80 "), "play.example.org");
        assert_eq!(host_key("203.0.113.5"), "203.0.113.5");
        assert_eq!(host_key("2001:db8::1"), "2001:db8::1", "IPv6 keeps its colons");
        assert!(valid_field("Kiwi") && !valid_field("a\nb") && !valid_field("") && !valid_field("a b"));
    }

    #[test]
    fn secret_restores_the_same_identity() {
        let me = Identity::generate();
        assert_eq!(Identity::from_secret(&me.secret()).global_id(), me.global_id());
        assert_eq!(short("ABCDEFGHIJ"), "ABCD-EFGH");
        assert!(fresh(100, 100 + MAX_CLOCK_SKEW) && !fresh(100, 101 + MAX_CLOCK_SKEW));
    }
}
