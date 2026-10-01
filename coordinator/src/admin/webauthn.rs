//! Passkeys (WebAuthn) for admins: the relying party's half.
//!
//! Registration keeps the credential's id and public key (attestation isn't
//! checked: "none" is asked for, and any authenticator is fine). Signing in
//! checks the client data (type, challenge, origin), the authenticator data
//! (this site's id hash, user present and verified, the counter), and the
//! signature over both. ES256, EdDSA and RS256 keys, verified with `ring`.
//!
//! User verification is required, so a passkey on its own is two factors:
//! something the admin has and their PIN or biometric.

use ciborium::Value as Cbor;
use ring::signature;
use serde_json::Value;
use sha2::Digest as _;

pub const ES256: i64 = -7;
pub const EDDSA: i64 = -8;
pub const RS256: i64 = -257;

const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_AT: u8 = 0x40;

/// A registered passkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// The credential id, base64url.
    pub id: String,
    pub alg: i64,
    /// ES256: the uncompressed point (0x04 x y); EdDSA: the 32-byte key;
    /// RS256: the modulus's length (2 bytes, big-endian), the modulus, the exponent.
    pub public_key: Vec<u8>,
    pub sign_count: u32,
}

pub fn b64url(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

pub fn from_b64url(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|_| "not base64url".to_string())
}

/// What the site expects: its id (the host name) and origin.
pub struct Site<'a> {
    pub rp_id: &'a str,
    pub origin: &'a str,
}

/// Checks the client data JSON: its type, the challenge, the origin.
fn check_client_data(raw: &[u8], kind: &str, challenge: &str, site: &Site) -> Result<(), String> {
    let data: Value = serde_json::from_slice(raw).map_err(|_| "the client data isn't JSON")?;
    if data["type"] != kind {
        return Err(format!("not a {kind}"));
    }
    if data["challenge"].as_str().map(|c| c.trim_end_matches('=')) != Some(challenge) {
        return Err("the challenge doesn't match".into());
    }
    if data["origin"] != site.origin {
        return Err(format!("made for {}, not {}", data["origin"], site.origin));
    }
    if data["crossOrigin"].as_bool() == Some(true) {
        return Err("made in a frame of another site".into());
    }
    Ok(())
}

/// The fixed start of authenticator data: the site id hash, flags, counter.
fn check_auth_data(auth: &[u8], site: &Site) -> Result<(u8, u32), String> {
    if auth.len() < 37 {
        return Err("the authenticator data is too short".into());
    }
    let rp_hash: [u8; 32] = sha2::Sha256::digest(site.rp_id.as_bytes()).into();
    if auth[..32] != rp_hash {
        return Err("made for another site".into());
    }
    let flags = auth[32];
    if flags & FLAG_UP == 0 || flags & FLAG_UV == 0 {
        return Err("the authenticator didn't verify the user (PIN or biometric)".into());
    }
    Ok((flags, u32::from_be_bytes([auth[33], auth[34], auth[35], auth[36]])))
}

fn cbor_get<'a>(map: &'a [(Cbor, Cbor)], key: i64) -> Option<&'a Cbor> {
    map.iter().find(|(k, _)| k.as_integer().and_then(|i| i64::try_from(i).ok()) == Some(key)).map(|(_, v)| v)
}

fn cbor_bytes(v: Option<&Cbor>) -> Result<&[u8], String> {
    v.and_then(Cbor::as_bytes).map(Vec::as_slice).ok_or_else(|| "a key part is missing".to_string())
}

/// The public key from a COSE key.
fn cose_key(key: &Cbor) -> Result<(i64, Vec<u8>), String> {
    let map = key.as_map().ok_or("the key isn't a COSE key")?;
    let int = |k| cbor_get(map, k).and_then(Cbor::as_integer).and_then(|i| i64::try_from(i).ok());
    let alg = int(3).ok_or("the key has no algorithm")?;
    match (int(1), alg) {
        // EC2, P-256.
        (Some(2), ES256) => {
            if int(-1) != Some(1) {
                return Err("only P-256 is accepted".into());
            }
            let (x, y) = (cbor_bytes(cbor_get(map, -2))?, cbor_bytes(cbor_get(map, -3))?);
            if x.len() != 32 || y.len() != 32 {
                return Err("not a P-256 point".into());
            }
            Ok((ES256, [&[4u8][..], x, y].concat()))
        }
        // OKP, Ed25519.
        (Some(1), EDDSA) => {
            if int(-1) != Some(6) {
                return Err("only Ed25519 is accepted".into());
            }
            let x = cbor_bytes(cbor_get(map, -2))?;
            if x.len() != 32 {
                return Err("not an Ed25519 key".into());
            }
            Ok((EDDSA, x.to_vec()))
        }
        // RSA.
        (Some(3), RS256) => {
            let (n, e) = (cbor_bytes(cbor_get(map, -1))?, cbor_bytes(cbor_get(map, -2))?);
            if n.len() < 256 || n.len() > 1024 || e.is_empty() || e.len() > 8 {
                return Err("not an RSA key of 2048 bits or more".into());
            }
            let len = u16::try_from(n.len()).map_err(|_| "too long")?;
            Ok((RS256, [&len.to_be_bytes()[..], n, e].concat()))
        }
        _ => Err(format!("the key type isn't accepted (algorithm {alg})")),
    }
}

/// Checks a registration (`navigator.credentials.create`'s answer, as the
/// UI sends it: base64url fields) for `challenge`, and answers the passkey.
pub fn register(credential: &Value, challenge: &str, site: &Site) -> Result<Credential, String> {
    let response = &credential["response"];
    let field = |name: &str| from_b64url(response[name].as_str().unwrap_or_default());
    let client_data = field("clientDataJSON")?;
    check_client_data(&client_data, "webauthn.create", challenge, site)?;
    let attestation: Cbor = ciborium::from_reader(field("attestationObject")?.as_slice()).map_err(|_| "the attestation isn't CBOR")?;
    let map = attestation.as_map().ok_or("the attestation isn't a map")?;
    let auth = map
        .iter()
        .find(|(k, _)| k.as_text() == Some("authData"))
        .and_then(|(_, v)| v.as_bytes())
        .ok_or("the attestation has no authenticator data")?;
    let (flags, sign_count) = check_auth_data(auth, site)?;
    if flags & FLAG_AT == 0 {
        return Err("no credential in the authenticator data".into());
    }
    // After the fixed part: AAGUID (16), the id's length (2), the id, the COSE key.
    let rest = auth.get(37 + 16..).ok_or("the credential data is cut short")?;
    if rest.len() < 2 {
        return Err("the credential data is cut short".into());
    }
    let id_len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
    if id_len == 0 || id_len > 1023 {
        return Err("the credential id's length is wrong".into());
    }
    let id = rest.get(2..2 + id_len).ok_or("the credential id is cut short")?;
    let mut key_bytes = rest.get(2 + id_len..).ok_or("no public key")?;
    let key: Cbor = ciborium::from_reader(&mut key_bytes).map_err(|_| "the public key isn't CBOR")?;
    let (alg, public_key) = cose_key(&key)?;
    let id = b64url(id);
    if credential["id"].as_str().is_some_and(|given| given != id) {
        return Err("the credential ids don't agree".into());
    }
    Ok(Credential { id, alg, public_key, sign_count })
}

/// Checks a sign-in (`navigator.credentials.get`'s answer) with `stored`
/// for `challenge`. Answers the new counter. `user_handle` is the admin the
/// passkey was made for, when the authenticator says.
pub fn authenticate(credential: &Value, stored: &Credential, challenge: &str, site: &Site) -> Result<u32, String> {
    let response = &credential["response"];
    let field = |name: &str| from_b64url(response[name].as_str().unwrap_or_default());
    let client_data = field("clientDataJSON")?;
    check_client_data(&client_data, "webauthn.get", challenge, site)?;
    let auth = field("authenticatorData")?;
    let (_, count) = check_auth_data(&auth, site)?;
    // A counter that didn't go up means a copied key (when the authenticator counts at all).
    if (count != 0 || stored.sign_count != 0) && count <= stored.sign_count {
        return Err("the passkey's counter went back: it may have been copied".into());
    }
    let signed = [auth.as_slice(), &sha2::Sha256::digest(&client_data)].concat();
    let sig = field("signature")?;
    let ok = match stored.alg {
        ES256 => signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, &stored.public_key)
            .verify(&signed, &sig)
            .is_ok(),
        EDDSA => signature::UnparsedPublicKey::new(&signature::ED25519, &stored.public_key).verify(&signed, &sig).is_ok(),
        RS256 => {
            let len = usize::from(u16::from_be_bytes([stored.public_key[0], stored.public_key[1]]));
            let (n, e) = stored.public_key[2..].split_at(len);
            signature::RsaPublicKeyComponents { n, e }
                .verify(&signature::RSA_PKCS1_2048_8192_SHA256, &signed, &sig)
                .is_ok()
        }
        _ => false,
    };
    if !ok {
        return Err("the signature doesn't match".into());
    }
    Ok(count)
}

/// The user handle the authenticator returned, if any.
pub fn user_handle(credential: &Value) -> Option<Vec<u8>> {
    credential["response"]["userHandle"].as_str().filter(|h| !h.is_empty()).and_then(|h| from_b64url(h).ok())
}

#[cfg(test)]
mod tests {
    use ring::rand::SystemRandom;
    use ring::signature::EcdsaKeyPair;
    use ring::signature::KeyPair as _;
    use serde_json::json;

    use super::*;

    const SITE: Site = Site {
        rp_id: "metrics.example.org",
        origin: "https://metrics.example.org",
    };

    /// A software authenticator with a P-256 key.
    struct Authenticator {
        key: EcdsaKeyPair,
        id: Vec<u8>,
        count: u32,
    }

    impl Authenticator {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
            let key = EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).unwrap();
            Self { key, id: vec![7; 16], count: 0 }
        }

        fn auth_data(&self, rp_id: &str, flags: u8, with_key: bool) -> Vec<u8> {
            let mut d = sha2::Sha256::digest(rp_id.as_bytes()).to_vec();
            d.push(flags);
            d.extend_from_slice(&self.count.to_be_bytes());
            if with_key {
                d.extend_from_slice(&[0; 16]);
                d.extend_from_slice(&u16::try_from(self.id.len()).unwrap().to_be_bytes());
                d.extend_from_slice(&self.id);
                let point = self.key.public_key().as_ref();
                let cose = Cbor::Map(vec![
                    (Cbor::Integer(1.into()), Cbor::Integer(2.into())),
                    (Cbor::Integer(3.into()), Cbor::Integer((-7).into())),
                    (Cbor::Integer((-1).into()), Cbor::Integer(1.into())),
                    (Cbor::Integer((-2).into()), Cbor::Bytes(point[1..33].to_vec())),
                    (Cbor::Integer((-3).into()), Cbor::Bytes(point[33..].to_vec())),
                ]);
                ciborium::into_writer(&cose, &mut d).unwrap();
            }
            d
        }

        fn create(&self, challenge: &str, origin: &str) -> Value {
            let client = json!({ "type": "webauthn.create", "challenge": challenge, "origin": origin }).to_string();
            let att = Cbor::Map(vec![
                (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
                (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
                (Cbor::Text("authData".into()), Cbor::Bytes(self.auth_data(SITE.rp_id, FLAG_UP | FLAG_UV | FLAG_AT, true))),
            ]);
            let mut att_bytes = Vec::new();
            ciborium::into_writer(&att, &mut att_bytes).unwrap();
            json!({ "id": b64url(&self.id), "response": { "clientDataJSON": b64url(client.as_bytes()), "attestationObject": b64url(&att_bytes) } })
        }

        fn get(&mut self, challenge: &str, rp_id: &str, flags: u8) -> Value {
            self.count += 1;
            let client = json!({ "type": "webauthn.get", "challenge": challenge, "origin": SITE.origin }).to_string();
            let auth = self.auth_data(rp_id, flags, false);
            let signed = [auth.as_slice(), &sha2::Sha256::digest(client.as_bytes())].concat();
            let sig = self.key.sign(&SystemRandom::new(), &signed).unwrap();
            json!({ "id": b64url(&self.id), "response": {
                "clientDataJSON": b64url(client.as_bytes()), "authenticatorData": b64url(&auth), "signature": b64url(sig.as_ref()), "userHandle": b64url(b"1") } })
        }
    }

    #[test]
    fn registers_and_signs_in() {
        let mut a = Authenticator::new();
        let cred = register(&a.create("abc", SITE.origin), "abc", &SITE).unwrap();
        assert_eq!((cred.alg, cred.id.as_str()), (ES256, b64url(&a.id).as_str()));
        assert!(register(&a.create("abc", "https://evil.example"), "abc", &SITE).is_err(), "another origin");
        assert!(register(&a.create("abc", SITE.origin), "xyz", &SITE).is_err(), "another challenge");

        let first = a.get("c1", SITE.rp_id, FLAG_UP | FLAG_UV);
        let count = authenticate(&first, &cred, "c1", &SITE).unwrap();
        assert_eq!(count, 1);
        let stored = Credential {
            sign_count: count,
            ..cred.clone()
        };
        assert!(authenticate(&first, &stored, "c1", &SITE).is_err(), "a replay: the counter didn't go up");
        assert!(authenticate(&a.get("c2", SITE.rp_id, FLAG_UP), &stored, "c2", &SITE).is_err(), "user not verified");
        assert!(authenticate(&a.get("c3", "evil.example", FLAG_UP | FLAG_UV), &stored, "c3", &SITE).is_err(), "another site");
        assert!(
            authenticate(&a.get("c4", SITE.rp_id, FLAG_UP | FLAG_UV), &stored, "other", &SITE).is_err(),
            "another challenge"
        );
        let other = Authenticator::new();
        let wrong_key = Credential {
            public_key: other.key.public_key().as_ref().to_vec(),
            ..stored.clone()
        };
        assert!(authenticate(&a.get("c5", SITE.rp_id, FLAG_UP | FLAG_UV), &wrong_key, "c5", &SITE).is_err(), "another key");
        assert!(authenticate(&a.get("c6", SITE.rp_id, FLAG_UP | FLAG_UV), &stored, "c6", &SITE).is_ok());
        assert_eq!(user_handle(&first), Some(b"1".to_vec()));
    }
}
