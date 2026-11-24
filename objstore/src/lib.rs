//! A small client for an S3-compatible object store (Cloudflare R2): a signed PUT and GET of one
//! object (AWS Signature Version 4, path style, region `auto`). For the match recordings kept 30
//! days (0.5.6's RP3): each server puts its own with its own key, the coordinator reads them with
//! a read-only one. The keys come from an environment file the operator writes ([`Store::from_env`]).

use hmac::Mac as _;
use sha2::Digest as _;

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

/// Where the objects are and the key to them.
#[derive(Clone)]
pub struct Store {
    /// `https://<account>.r2.cloudflarestorage.com` (no path).
    pub endpoint: String,
    pub bucket: String,
    pub key_id: String,
    secret: String,
    pub region: String,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the secret.
        f.debug_struct("Store")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl Store {
    pub fn new(endpoint: &str, bucket: &str, key_id: &str, secret: &str) -> Result<Self, String> {
        let endpoint = endpoint.trim_end_matches('/').to_string();
        if !endpoint.starts_with("https://") || endpoint[8..].contains('/') {
            return Err("the endpoint must be https://HOST".into());
        }
        if bucket.is_empty() || !bucket.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.') {
            return Err("not a bucket name".into());
        }
        if key_id.is_empty() || secret.is_empty() {
            return Err("no key".into());
        }
        Ok(Self {
            endpoint,
            bucket: bucket.into(),
            key_id: key_id.into(),
            secret: secret.into(),
            region: "auto".into(),
        })
    }

    /// From `<PREFIX>_ENDPOINT`, `_BUCKET`, `_KEY_ID` and `_SECRET`; None when any is missing
    /// (the feature is then off).
    pub fn from_env(prefix: &str) -> Option<Result<Self, String>> {
        let get = |k: &str| std::env::var(format!("{prefix}_{k}")).ok().filter(|v| !v.is_empty());
        Some(Self::new(&get("ENDPOINT")?, &get("BUCKET")?, &get("KEY_ID")?, &get("SECRET")?))
    }

    fn host(&self) -> &str {
        &self.endpoint[8..]
    }

    /// The signed headers of a request for `key` (`method`, the payload's SHA-256 in hex) at
    /// `now` (`YYYYMMDDTHHMMSSZ`).
    fn sign(&self, method: &str, key: &str, payload_sha: &str, amz_date: &str) -> Vec<(String, String)> {
        let date = &amz_date[..8];
        let path = format!("/{}/{}", self.bucket, encode_path(key));
        let headers = [
            ("host", self.host().to_string()),
            ("x-amz-content-sha256", payload_sha.to_string()),
            ("x-amz-date", amz_date.to_string()),
        ];
        let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
        let signed: Vec<&str> = headers.iter().map(|(k, _)| *k).collect();
        let signed = signed.join(";");
        let canonical = format!("{method}\n{path}\n\n{canonical_headers}\n{signed}\n{payload_sha}");
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex(&sha2::Sha256::digest(canonical.as_bytes())));
        let mut k = mac(format!("AWS4{}", self.secret).as_bytes(), date.as_bytes());
        for part in [self.region.as_str(), "s3", "aws4_request"] {
            k = mac(&k, part.as_bytes());
        }
        let signature = hex(&mac(&k, to_sign.as_bytes()));
        vec![
            ("x-amz-content-sha256".into(), payload_sha.into()),
            ("x-amz-date".into(), amz_date.into()),
            (
                "authorization".into(),
                format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}", self.key_id),
            ),
        ]
    }

    fn url(&self, key: &str) -> String {
        format!("{}/{}/{}", self.endpoint, self.bucket, encode_path(key))
    }

    /// Puts `body` at `key`.
    pub async fn put(&self, client: &reqwest::Client, key: &str, body: Vec<u8>, content_type: &str) -> Result<(), String> {
        let sha = hex(&sha2::Sha256::digest(&body));
        let mut req = client.put(self.url(key)).header("content-type", content_type).body(body);
        for (k, v) in self.sign("PUT", key, &sha, &amz_now()) {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("{} from the store", resp.status()))
        }
    }

    /// The object at `key`; None when there's none.
    pub async fn get(&self, client: &reqwest::Client, key: &str) -> Result<Option<Vec<u8>>, String> {
        let sha = hex(&sha2::Sha256::digest(b""));
        let mut req = client.get(self.url(key));
        for (k, v) in self.sign("GET", key, &sha, &amz_now()) {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        match resp.status() {
            s if s.is_success() => Ok(Some(resp.bytes().await.map_err(|e| e.to_string())?.to_vec())),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            s => Err(format!("{s} from the store")),
        }
    }
}

fn mac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC takes any key");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A key as a URI path: each segment percent-encoded, the slashes kept.
fn encode_path(key: &str) -> String {
    key.split('/')
        .map(|seg| {
            seg.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Now as `YYYYMMDDTHHMMSSZ`.
fn amz_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    amz_date(secs)
}

/// Unix seconds as `YYYYMMDDTHHMMSSZ`.
pub fn amz_date(secs: i64) -> String {
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", t / 3600, t / 60 % 60, t % 60)
}

/// A day number (days since 1970-01-01) as year, month, day.
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc() {
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(1_795_384_123), "20261122T214843Z");
        assert_eq!(civil(20_780), (2026, 11, 23));
        assert_eq!(civil(-1), (1969, 12, 31));
    }

    /// The signing key's chain against AWS's published example (the SigV4 docs' "deriving the
    /// signing key": 20120215, us-east-1, iam).
    #[test]
    fn the_signing_key_is_awss() {
        let mut k = mac(b"AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", b"20120215");
        for part in ["us-east-1", "iam", "aws4_request"] {
            k = mac(&k, part.as_bytes());
        }
        assert_eq!(hex(&k), "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d");
        let s = Store::new("https://acct.r2.cloudflarestorage.com", "recordings", "KEY", "SECRET").unwrap();
        let h = s.sign("GET", "recordings/2026/11/x.fer", &hex(&sha2::Sha256::digest(b"")), "20261123T000000Z");
        assert!(
            h[2].1
                .starts_with("AWS4-HMAC-SHA256 Credential=KEY/20261123/auto/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="),
            "{}",
            h[2].1
        );
        assert_eq!(
            h,
            s.sign("GET", "recordings/2026/11/x.fer", &hex(&sha2::Sha256::digest(b"")), "20261123T000000Z"),
            "the same request signs the same"
        );
    }

    #[test]
    fn keys_and_settings_are_checked() {
        assert_eq!(encode_path("recordings/2026/11/ab cd.fer"), "recordings/2026/11/ab%20cd.fer");
        assert!(Store::new("http://x", "b", "k", "s").is_err());
        assert!(Store::new("https://x/y", "b", "k", "s").is_err());
        assert!(Store::new("https://x", "B!", "k", "s").is_err());
        assert!(!format!("{:?}", Store::new("https://x", "b", "k", "secret").unwrap()).contains("secret"));
    }
}
