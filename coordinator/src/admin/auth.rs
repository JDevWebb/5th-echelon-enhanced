//! The admin UI's sign-in pieces: passwords (Argon2id), TOTP codes
//! (RFC 6238), recovery codes, session tokens, and where admins may sign in
//! from (networks and countries).

use std::net::IpAddr;

use hmac::Mac as _;
use sha2::Digest as _;

/// Passwords are at least this long.
pub const MIN_PASSWORD: usize = 12;
const MAX_PASSWORD: usize = 256;

pub fn hash_password(password: &str) -> Result<String, String> {
    use argon2::password_hash::PasswordHasher as _;
    let salt = argon2::password_hash::SaltString::try_from_rng(&mut argon2::password_hash::rand_core::OsRng).map_err(|e| e.to_string())?;
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), salt.as_salt())
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

pub fn check_password(password: &str, hash: &str) -> bool {
    use argon2::password_hash::PasswordVerifier as _;
    argon2::password_hash::PasswordHash::new(hash).is_ok_and(|h| argon2::Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// Why a new password isn't good enough, if it isn't.
pub fn weak_password(password: &str, username: &str) -> Option<&'static str> {
    if password.chars().count() < MIN_PASSWORD {
        return Some("passwords are at least 12 characters");
    }
    if password.len() > MAX_PASSWORD {
        return Some("that password is too long");
    }
    let lower = password.to_lowercase();
    if lower.contains(&username.to_lowercase()) || ["password", "123456", "qwerty", "letmein", "admin", "echelon"].iter().any(|w| lower.contains(w)) {
        return Some("that password is too easy to guess");
    }
    let distinct: std::collections::HashSet<char> = password.chars().collect();
    if distinct.len() < 6 {
        return Some("that password repeats too few characters");
    }
    None
}

/// Random bytes, base32: tokens, secrets, codes.
pub fn random(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut buf);
    identity::base32_encode(&buf)
}

/// The stored form of a token or code (they're random: no salt needed).
pub fn digest(secret: &str) -> String {
    identity::base32_encode(&sha2::Sha256::digest(secret.trim().as_bytes()))
}

/// Recovery codes: ten, each `xxxx-xxxx-xxxx` (60 random bits).
pub fn recovery_codes() -> Vec<String> {
    (0..10)
        .map(|_| {
            let s = random(8).to_lowercase();
            format!("{}-{}-{}", &s[0..4], &s[4..8], &s[8..12])
        })
        .collect()
}

/// A recovery code as typed: lowercase, without spaces or dashes, dashed again.
pub fn normalise_code(code: &str) -> String {
    let s: String = code.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_lowercase();
    if s.len() == 12 {
        format!("{}-{}-{}", &s[0..4], &s[4..8], &s[8..12])
    } else {
        s
    }
}

/// TOTP: 6 digits, 30-second steps, HMAC-SHA1 (what every authenticator app does).
pub const TOTP_STEP: i64 = 30;

pub fn totp_at(secret: &[u8], step: i64) -> u32 {
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(secret).expect("any key length");
    mac.update(&step.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let offset = usize::from(h[19] & 0xf);
    let code = u32::from_be_bytes([h[offset] & 0x7f, h[offset + 1], h[offset + 2], h[offset + 3]]);
    code % 1_000_000
}

/// The time step `code` is valid for (this one, or one either side for
/// clock drift), if it's later than `last_used`.
pub fn totp_check(secret_b32: &str, code: &str, now: i64, last_used: i64) -> Option<i64> {
    let secret = identity::base32_decode(secret_b32)?;
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    if code.len() != 6 {
        return None;
    }
    let code: u32 = code.parse().ok()?;
    let step = now.div_euclid(TOTP_STEP);
    [step - 1, step, step + 1].into_iter().find(|s| *s > last_used && totp_at(&secret, *s) == code)
}

/// The `otpauth://` link authenticator apps read (from a QR code).
pub fn totp_uri(secret_b32: &str, username: &str, issuer: &str) -> String {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| if b.is_ascii_alphanumeric() { char::from(b).to_string() } else { format!("%{b:02X}") })
            .collect::<String>()
    };
    format!(
        "otpauth://totp/{}:{}?secret={secret_b32}&issuer={}&algorithm=SHA1&digits=6&period=30",
        enc(issuer),
        enc(username),
        enc(issuer)
    )
}

/// The link as a QR code, in SVG.
pub fn qr_svg(text: &str) -> String {
    qrcode::QrCode::new(text.as_bytes())
        .map(|c| c.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).quiet_zone(true).build())
        .unwrap_or_default()
}

/// An address range: "203.0.113.0/24", "2001:db8::/48", or one address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    net: IpAddr,
    prefix: u8,
}

impl Network {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (addr, prefix) = s.split_once('/').map_or((s, None), |(a, p)| (a, Some(p)));
        let net: IpAddr = addr.parse().ok()?;
        let max = if net.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.parse().ok().filter(|p| *p <= max)?,
            None => max,
        };
        Some(Self { net, prefix })
    }

    pub fn contains(self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 => v4,
        };
        let bits = |ip: IpAddr| -> u128 {
            match ip {
                IpAddr::V4(v4) => u128::from(u32::from(v4)) << 96,
                IpAddr::V6(v6) => u128::from(v6),
            }
        };
        if self.net.is_ipv4() != ip.is_ipv4() {
            return false;
        }
        let len = u32::from(self.prefix);
        let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
        bits(self.net) & mask == bits(ip) & mask
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.net, self.prefix)
    }
}

/// Where admins may sign in from. Empty lists allow anywhere.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Restrictions {
    /// Address ranges, e.g. "203.0.113.7/32".
    #[serde(default)]
    pub networks: Vec<String>,
    /// ISO country codes, e.g. "NZ" (from Cloudflare, or DB-IP).
    #[serde(default)]
    pub countries: Vec<String>,
}

impl Restrictions {
    /// Cleans the lists up; an error names what isn't valid.
    pub fn normalised(&self) -> Result<Self, String> {
        let mut networks = Vec::new();
        for n in &self.networks {
            if n.trim().is_empty() {
                continue;
            }
            networks.push(Network::parse(n).ok_or_else(|| format!("{n:?} isn't an address or range"))?.to_string());
        }
        let mut countries = Vec::new();
        for c in &self.countries {
            let c = c.trim().to_uppercase();
            if c.is_empty() {
                continue;
            }
            if c.len() != 2 || !c.bytes().all(|b| b.is_ascii_uppercase()) {
                return Err(format!("{c:?} isn't a two-letter country code"));
            }
            countries.push(c);
        }
        networks.sort();
        networks.dedup();
        countries.sort();
        countries.dedup();
        Ok(Self { networks, countries })
    }

    /// Whether a request from `ip` in `country` may reach the admin UI. An
    /// unknown country passes only when countries aren't restricted.
    pub fn allows(&self, ip: IpAddr, country: &str) -> bool {
        let network_ok = self.networks.is_empty() || self.networks.iter().filter_map(|n| Network::parse(n)).any(|n| n.contains(ip));
        let country_ok = self.countries.is_empty() || self.countries.iter().any(|c| c.eq_ignore_ascii_case(country));
        network_ok && country_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_matches_rfc_6238() {
        // RFC 6238's SHA1 test key, at 59 s and 1111111109 s (8-digit values 94287082, 07081804).
        let key = b"12345678901234567890";
        assert_eq!(totp_at(key, 59 / 30), 287_082);
        assert_eq!(totp_at(key, 1_111_111_109 / 30), 81_804);
        let secret = identity::base32_encode(key);
        let now = 1_111_111_109;
        let step = totp_check(&secret, "081 804", now, 0).unwrap();
        assert_eq!(totp_check(&secret, "081804", now, step), None, "a code works once");
        assert_eq!(totp_check(&secret, "000000", now, 0), None);
        assert!(totp_uri(&secret, "jason webb", "5th Echelon").starts_with("otpauth://totp/5th%20Echelon:jason%20webb?secret="));
        assert!(qr_svg("otpauth://totp/x").contains("<svg"));
    }

    #[test]
    fn passwords() {
        let h = hash_password("correct horse battery").unwrap();
        assert!(check_password("correct horse battery", &h));
        assert!(!check_password("correct horse batterz", &h));
        assert!(weak_password("short", "kiwi").is_some());
        assert!(weak_password("kiwi-is-the-best-1", "kiwi").is_some());
        assert!(weak_password("aaaaaaaaaaaaaaaa", "x").is_some());
        assert!(weak_password("night vision goggles 4", "jason").is_none());
    }

    #[test]
    fn codes() {
        let codes = recovery_codes();
        assert_eq!(codes.len(), 10);
        assert!(codes.iter().all(|c| c.len() == 14 && normalise_code(&c.replace('-', " ").to_uppercase()) == *c));
    }

    #[test]
    fn restrictions() {
        let r = Restrictions {
            networks: vec!["203.0.113.0/24".into(), " 2001:db8::/32".into(), "".into()],
            countries: vec!["nz".into(), "AU".into()],
        }
        .normalised()
        .unwrap();
        assert_eq!(r.networks, vec!["2001:db8::/32", "203.0.113.0/24"]);
        assert!(r.allows("203.0.113.9".parse().unwrap(), "NZ"));
        assert!(r.allows("::ffff:203.0.113.9".parse().unwrap(), "AU"));
        assert!(!r.allows("198.51.100.1".parse().unwrap(), "NZ"), "outside the networks");
        assert!(!r.allows("203.0.113.9".parse().unwrap(), "US"), "another country");
        assert!(!r.allows("203.0.113.9".parse().unwrap(), ""), "an unknown country");
        assert!(Restrictions::default().allows("198.51.100.1".parse().unwrap(), ""));
        assert!(Restrictions {
            networks: vec!["300.1.1.1".into()],
            ..Restrictions::default()
        }
        .normalised()
        .is_err());
        assert!(Restrictions {
            countries: vec!["NZL".into()],
            ..Restrictions::default()
        }
        .normalised()
        .is_err());
    }
}
