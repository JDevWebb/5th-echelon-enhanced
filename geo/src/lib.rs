//! Where an IP address is, from DB-IP's free "IP to City Lite" database
//! (<https://db-ip.com>, CC BY 4.0: anything showing its results credits
//! "IP Geolocation by DB-IP").
//!
//! The database is downloaded into a folder of the caller's and replaced
//! with each month's release. Lookups happen on this machine: addresses are
//! never sent anywhere.

use std::net::IpAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::Duration;
use std::time::SystemTime;

use serde::Deserialize;
use serde::Serialize;

/// The attribution DB-IP's licence asks for, wherever results are shown.
pub const ATTRIBUTION: &str = "IP Geolocation by DB-IP (https://db-ip.com)";

const FILE: &str = "dbip-city-lite.mmdb";
/// A new database is looked for when the one here is older than this.
const MAX_AGE: Duration = Duration::from_secs(35 * 24 * 3600);
/// The largest download (the city database is about 130 MB unpacked).
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

/// Where an address is. Fields DB-IP doesn't know are empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// ISO 3166-1 alpha-2, e.g. "NZ".
    pub country: String,
    pub country_name: String,
    pub city: String,
    /// Region or state, e.g. "Auckland".
    pub region: String,
    pub lat: f64,
    pub lon: f64,
}

/// The database, once there is one.
pub struct Geo {
    dir: PathBuf,
    reader: RwLock<Option<maxminddb::Reader<maxminddb::Mmap>>>,
}

impl Geo {
    /// Uses the database in `dir`, if one was downloaded already; [`Geo::keep_current`]
    /// downloads it.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let reader = open(&dir.join(FILE));
        Self { dir, reader: RwLock::new(reader) }
    }

    /// Whether there's a database to look in.
    pub fn ready(&self) -> bool {
        self.reader.read().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// Where `ip` is; None for private addresses, or without a database.
    pub fn lookup(&self, ip: IpAddr) -> Option<Place> {
        if !is_public(ip) {
            return None;
        }
        let guard = self.reader.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let city: maxminddb::geoip2::City = guard.as_ref()?.lookup(ip).ok()??;
        let name = |names: Option<&std::collections::BTreeMap<&str, &str>>| names.and_then(|n| n.get("en").copied()).unwrap_or_default().to_string();
        let place = Place {
            country: city.country.as_ref().and_then(|c| c.iso_code).unwrap_or_default().to_string(),
            country_name: name(city.country.as_ref().and_then(|c| c.names.as_ref())),
            city: name(city.city.as_ref().and_then(|c| c.names.as_ref())),
            region: name(city.subdivisions.as_ref().and_then(|s| s.first()).and_then(|s| s.names.as_ref())),
            lat: city.location.as_ref().and_then(|l| l.latitude).unwrap_or_default(),
            lon: city.location.as_ref().and_then(|l| l.longitude).unwrap_or_default(),
        };
        (!place.country.is_empty()).then_some(place)
    }

    /// Downloads this month's database when the one here is missing or old,
    /// then again every day it's due. Runs until the process ends.
    pub async fn keep_current(&self, user_agent: &str) {
        loop {
            if self.due() {
                match self.download(user_agent).await {
                    Ok(month) => tracing::info!("GeoIP: using DB-IP's city database for {month}"),
                    Err(e) => tracing::warn!("GeoIP: couldn't download DB-IP's city database: {e}"),
                }
            }
            tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
        }
    }

    fn due(&self) -> bool {
        let age = std::fs::metadata(self.dir.join(FILE))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok());
        age.is_none_or(|age| age > MAX_AGE)
    }

    /// Downloads the newest monthly database (this month's, else last
    /// month's) and switches to it. Answers its month.
    async fn download(&self, user_agent: &str) -> Result<String, String> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?;
        let (year, month) = year_month(SystemTime::now());
        let mut last_error = String::new();
        for (y, m) in [(year, month), if month == 1 { (year - 1, 12) } else { (year, month - 1) }] {
            let label = format!("{y}-{m:02}");
            let url = format!("https://download.db-ip.com/free/dbip-city-lite-{label}.mmdb.gz");
            match fetch(&http, &url).await {
                Ok(gz) => {
                    let dir = self.dir.clone();
                    let reader = tokio::task::spawn_blocking(move || install(&dir, &gz)).await.map_err(|e| e.to_string())??;
                    *self.reader.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reader);
                    return Ok(label);
                }
                Err(e) => last_error = e,
            }
        }
        Err(last_error)
    }
}

async fn fetch(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let mut resp = http.get(url).send().await.and_then(reqwest::Response::error_for_status).map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        data.extend_from_slice(&chunk);
        if data.len() as u64 > MAX_DOWNLOAD {
            return Err("the download is larger than expected".into());
        }
    }
    Ok(data)
}

/// Unpacks `gz` next to the database, checks it opens, and puts it in place.
fn install(dir: &Path, gz: &[u8]) -> Result<maxminddb::Reader<maxminddb::Mmap>, String> {
    use std::io::Read as _;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut data)
        .map_err(|e| format!("unpacking: {e}"))?;
    let tmp = dir.join(format!("{FILE}.download"));
    std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
    if open(&tmp).is_none() {
        let _ = std::fs::remove_file(&tmp);
        return Err("the download isn't a city database".into());
    }
    let path = dir.join(FILE);
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    open(&path).ok_or_else(|| "the database can't be opened".into())
}

fn open(path: &Path) -> Option<maxminddb::Reader<maxminddb::Mmap>> {
    let reader = maxminddb::Reader::open_mmap(path).ok()?;
    // A sanity check: a well-known public address must resolve.
    reader.lookup::<maxminddb::geoip2::Country>(IpAddr::from([8, 8, 8, 8])).ok()??;
    Some(reader)
}

/// Whether `ip` is on the public internet (private, loopback, link-local,
/// CGNAT and documentation ranges aren't anywhere).
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || o[0] == 100 && (64..128).contains(&o[1])
                || o[0] >= 224)
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80)
                && v6.to_ipv4_mapped().is_none_or(|v4| is_public(IpAddr::V4(v4)))
        }
    }
}

/// The UTC year and month of `t`.
fn year_month(t: SystemTime) -> (i64, u32) {
    let days = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86400) as i64;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month)
}

impl Place {
    /// Whether the database knew where it is (DB-IP gives 0, 0 when it doesn't).
    pub fn located(&self) -> bool {
        self.lat != 0.0 || self.lon != 0.0
    }

    /// The great-circle distance to `other`, in km.
    pub fn km_to(&self, other: &Place) -> f64 {
        const EARTH_KM: f64 = 6371.0;
        let (a, b) = ((self.lat.to_radians(), self.lon.to_radians()), (other.lat.to_radians(), other.lon.to_radians()));
        let h = ((b.0 - a.0) / 2.0).sin().powi(2) + a.0.cos() * b.0.cos() * ((b.1 - a.1) / 2.0).sin().powi(2);
        2.0 * EARTH_KM * h.sqrt().min(1.0).asin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        let at = |lat, lon| Place { lat, lon, ..Place::default() };
        let (auckland, sydney, london) = (at(-36.85, 174.76), at(-33.87, 151.21), at(51.51, -0.13));
        assert!((auckland.km_to(&sydney) - 2160.0).abs() < 30.0, "{}", auckland.km_to(&sydney));
        assert!((auckland.km_to(&london) - 18_350.0).abs() < 100.0, "{}", auckland.km_to(&london));
        assert!(auckland.km_to(&auckland) < 0.001);
        assert!(auckland.located() && !Place::default().located());
    }

    #[test]
    fn months() {
        let at = |secs: u64| year_month(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), (1970, 1));
        assert_eq!(at(1_791_000_000), (2026, 10)); // 2026-10-02
        assert_eq!(at(1_772_323_200), (2026, 3)); // 2026-03-01 00:00
        assert_eq!(at(1_772_323_199), (2026, 2));
    }

    #[test]
    fn private_addresses_are_nowhere() {
        for ip in ["10.1.2.3", "192.168.1.1", "100.64.0.1", "127.0.0.1", "::1", "fd00::1", "fe80::1", "::ffff:10.0.0.1"] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "2001:4860:4860::8888", "::ffff:1.1.1.1"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
        assert_eq!(Geo::new("/nonexistent").lookup("8.8.8.8".parse().unwrap()), None);
    }
}

#[cfg(test)]
mod online {
    /// Downloads the real database (about 60 MB): `cargo test -p geo -- --ignored`.
    #[tokio::test]
    #[ignore = "downloads DB-IP's database"]
    async fn downloads_and_finds_places() {
        let dir = std::env::temp_dir().join("geo-test");
        let geo = super::Geo::new(&dir);
        geo.download("5th-echelon-test").await.unwrap();
        let place = geo.lookup("1.1.1.1".parse().unwrap()).unwrap();
        println!("{place:?}");
        assert!(!place.country.is_empty());
    }
}
