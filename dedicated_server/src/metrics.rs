//! What this server reports to its coordinator for the network's admin UI:
//! players and where they are (city counts from `geo`, never addresses),
//! what's being played, counters, and the machine's load and traffic.
//!
//! Collected every minute by `federation::run` and sent with
//! `POST /v1/metrics`.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

use serde::Serialize;

use crate::community_api::activity_of;
use crate::storage::Storage;

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Counters since the server started; the coordinator turns them into rates.
#[derive(Default)]
struct Counters {
    game_logins: AtomicU64,
    api_logins: AtomicU64,
    failed_logins: AtomicU64,
    registrations: AtomicU64,
    relayed_bytes: AtomicU64,
    relayed_packets: AtomicU64,
}

static COUNTERS: OnceLock<Counters> = OnceLock::new();

fn counters() -> &'static Counters {
    COUNTERS.get_or_init(Counters::default)
}

/// Where each signed-in player connected from, by account id: looked up in
/// `geo` when reporting, never sent.
fn addresses() -> &'static Mutex<HashMap<u32, IpAddr>> {
    static ADDRESSES: OnceLock<Mutex<HashMap<u32, IpAddr>>> = OnceLock::new();
    ADDRESSES.get_or_init(Mutex::default)
}

/// The geolocation database, when the server has one.
static GEO: OnceLock<std::sync::Arc<geo::Geo>> = OnceLock::new();

pub fn start(geo: std::sync::Arc<geo::Geo>) {
    let _ = STARTED.set(Instant::now());
    let _ = GEO.set(geo);
}

/// A player signed in to the game (the secure server) from `ip`.
pub fn game_login(user_id: u32, ip: IpAddr) {
    counters().game_logins.fetch_add(1, Ordering::Relaxed);
    addresses().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(user_id, ip);
}

/// A player's game connection closed.
pub fn game_logout(user_id: u32) {
    addresses().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&user_id);
}

pub fn api_login() {
    counters().api_logins.fetch_add(1, Ordering::Relaxed);
}

pub fn failed_login() {
    counters().failed_logins.fetch_add(1, Ordering::Relaxed);
}

pub fn registration() {
    counters().registrations.fetch_add(1, Ordering::Relaxed);
}

/// The NAT helper relayed a packet of `bytes` between players.
pub fn relayed(bytes: usize) {
    let c = counters();
    c.relayed_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    c.relayed_packets.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Metrics {
    pub version: String,
    pub uptime_secs: u64,
    pub players: Players,
    /// What's being played: sessions and players per mode, room kind and map.
    pub activity: Vec<Activity>,
    /// Online players per city (DB-IP); players whose address is private or
    /// unknown count under an empty country.
    pub places: Vec<Place>,
    pub counters: CounterValues,
    pub system: System,
    /// Whether a GeoIP database is loaded.
    pub geo: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Players {
    pub online: u32,
    pub total: u32,
    pub in_match: u32,
    pub in_lobby: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Activity {
    /// "svm" or "coop".
    pub mode: &'static str,
    /// "match" or "lobby".
    pub room: &'static str,
    /// The session's map attribute (101), if set.
    pub map: Option<u32>,
    /// The session's game mode attribute (102), if set.
    pub game_mode: Option<u32>,
    pub sessions: u32,
    pub players: u32,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Place {
    pub country: String,
    pub country_name: String,
    pub region: String,
    pub city: String,
    pub lat: f64,
    pub lon: f64,
    pub players: u32,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CounterValues {
    pub game_logins: u64,
    pub api_logins: u64,
    pub failed_logins: u64,
    pub registrations: u64,
    pub relayed_bytes: u64,
    pub relayed_packets: u64,
}

/// The machine (Linux; zero elsewhere). Byte counts are since boot.
#[derive(Debug, Clone, Serialize, Default)]
pub struct System {
    pub cpus: u32,
    /// The whole machine, over the last interval.
    pub cpu_percent: f32,
    /// This server's process, over the last interval (of one CPU's time).
    pub process_cpu_percent: f32,
    pub load: [f32; 3],
    pub mem_total: u64,
    pub mem_available: u64,
    pub process_rss: u64,
    pub disk_total: u64,
    pub disk_free: u64,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub uptime_secs: u64,
}

/// Collects the metrics now.
pub async fn collect(storage: &Storage) -> Metrics {
    let mut m = Metrics {
        version: crate::community_api::RELEASE.to_string(),
        uptime_secs: STARTED.get().map_or(0, |t| t.elapsed().as_secs()),
        geo: GEO.get().is_some_and(|g| g.ready()),
        ..Metrics::default()
    };
    if let Ok((online, total)) = storage.player_counts().await {
        (m.players.online, m.players.total) = (online, total);
    }
    if let Ok((players, sessions)) = storage.presence_async().await {
        let mut activity: BTreeMap<(&'static str, &'static str, Option<u32>, Option<u32>), (u32, u32)> = BTreeMap::new();
        for s in &sessions {
            let svm = crate::game_session::attribute_value(&s.attributes, 103).unwrap_or(0) != 0;
            let room = if crate::game_session::attribute_value(&s.attributes, 113) == Some(0) {
                "match"
            } else {
                "lobby"
            };
            let key = (
                if svm { "svm" } else { "coop" },
                room,
                crate::game_session::attribute_value(&s.attributes, 101),
                crate::game_session::attribute_value(&s.attributes, 102),
            );
            let e = activity.entry(key).or_default();
            e.0 += 1;
            e.1 += u32::try_from(s.players.len()).unwrap_or(u32::MAX);
        }
        m.activity = activity
            .into_iter()
            .map(|((mode, room, map, game_mode), (sessions, players))| Activity {
                mode,
                room,
                map,
                game_mode,
                sessions,
                players,
            })
            .collect();
        for (name, online) in &players {
            if !online {
                continue;
            }
            match activity_of(name, &sessions).map(|a| a.room) {
                Some("match") => m.players.in_match += 1,
                Some(_) => m.players.in_lobby += 1,
                None => {}
            }
        }
    }
    m.places = places();
    let c = counters();
    m.counters = CounterValues {
        game_logins: c.game_logins.load(Ordering::Relaxed),
        api_logins: c.api_logins.load(Ordering::Relaxed),
        failed_logins: c.failed_logins.load(Ordering::Relaxed),
        registrations: c.registrations.load(Ordering::Relaxed),
        relayed_bytes: c.relayed_bytes.load(Ordering::Relaxed),
        relayed_packets: c.relayed_packets.load(Ordering::Relaxed),
    };
    m.system = tokio::task::spawn_blocking(system).await.unwrap_or_default();
    m
}

/// Online players per city.
fn places() -> Vec<Place> {
    let ips: Vec<IpAddr> = addresses().lock().unwrap_or_else(std::sync::PoisonError::into_inner).values().copied().collect();
    let geo = GEO.get();
    let mut by_city: BTreeMap<(String, String, String), Place> = BTreeMap::new();
    for ip in ips {
        let place = geo.and_then(|g| g.lookup(ip)).unwrap_or_default();
        let key = (place.country.clone(), place.region.clone(), place.city.clone());
        let e = by_city.entry(key).or_insert_with(|| Place {
            country: place.country,
            country_name: place.country_name,
            region: place.region,
            city: place.city,
            lat: place.lat,
            lon: place.lon,
            players: 0,
        });
        e.players += 1;
    }
    by_city.into_values().collect()
}

/// The previous CPU sample: (machine busy, machine total, process ticks, when).
static LAST_CPU: Mutex<Option<(u64, u64, u64, Instant)>> = Mutex::new(None);

#[cfg(target_os = "linux")]
fn system() -> System {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let mut s = System {
        cpus: std::thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(1)),
        ..System::default()
    };
    // Machine CPU: the first line of /proc/stat (user nice system idle iowait irq softirq steal).
    let stat = read("/proc/stat");
    let fields: Vec<u64> = stat.lines().next().unwrap_or_default().split_whitespace().skip(1).filter_map(|f| f.parse().ok()).collect();
    let total: u64 = fields.iter().take(8).sum();
    let idle = fields.get(3).copied().unwrap_or(0) + fields.get(4).copied().unwrap_or(0);
    let busy = total.saturating_sub(idle);
    // This process: utime + stime, fields 14 and 15 of /proc/self/stat (after the name).
    let me = read("/proc/self/stat");
    let after_name = me.rsplit_once(')').map_or("", |(_, rest)| rest);
    let proc_fields: Vec<u64> = after_name.split_whitespace().filter_map(|f| f.parse().ok()).collect();
    // after ") " the fields start at #3 (state, not numeric, skipped by the parse filter): utime is #14.
    let ticks = proc_fields.get(10).copied().unwrap_or(0) + proc_fields.get(11).copied().unwrap_or(0);
    let now = Instant::now();
    let mut last = LAST_CPU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((b0, t0, p0, at)) = *last {
        let dt = total.saturating_sub(t0);
        if dt > 0 {
            s.cpu_percent = 100.0 * busy.saturating_sub(b0) as f32 / dt as f32;
        }
        let secs = now.duration_since(at).as_secs_f32();
        if secs > 0.0 {
            // Clock ticks are 100 a second on Linux.
            s.process_cpu_percent = ticks.saturating_sub(p0) as f32 / secs;
        }
    }
    *last = Some((busy, total, ticks, now));
    drop(last);

    let loadavg = read("/proc/loadavg");
    for (i, v) in loadavg.split_whitespace().take(3).enumerate() {
        s.load[i] = v.parse().unwrap_or(0.0);
    }
    let kb = |text: &str, key: &str| -> u64 {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    let meminfo = read("/proc/meminfo");
    s.mem_total = kb(&meminfo, "MemTotal:");
    s.mem_available = kb(&meminfo, "MemAvailable:");
    s.process_rss = kb(&read("/proc/self/status"), "VmRSS:");
    // Every interface but loopback.
    for line in read("/proc/net/dev").lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else { continue };
        if name.trim() == "lo" {
            continue;
        }
        let v: Vec<u64> = rest.split_whitespace().filter_map(|f| f.parse().ok()).collect();
        s.net_rx_bytes += v.first().copied().unwrap_or(0);
        s.net_tx_bytes += v.get(8).copied().unwrap_or(0);
    }
    s.uptime_secs = read("/proc/uptime").split('.').next().and_then(|v| v.parse().ok()).unwrap_or(0);
    // The disk the server's files are on.
    if let Ok(path) = std::ffi::CString::new(".") {
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: a valid C string and a zeroed statvfs the call fills in.
        if unsafe { libc::statvfs(path.as_ptr(), &mut st) } == 0 {
            s.disk_total = u64::from(st.f_blocks) * u64::from(st.f_frsize);
            s.disk_free = u64::from(st.f_bavail) * u64::from(st.f_frsize);
        }
    }
    s
}

#[cfg(not(target_os = "linux"))]
fn system() -> System {
    let _ = &LAST_CPU;
    System {
        cpus: std::thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(1)),
        ..System::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_are_counted_by_city_without_addresses() {
        game_login(1, "10.0.0.1".parse().unwrap());
        game_login(2, "10.0.0.2".parse().unwrap());
        game_login(2, "10.0.0.3".parse().unwrap());
        let p = places();
        assert_eq!(p.iter().map(|p| p.players).sum::<u32>(), 2, "one place per player, the latest address");
        game_logout(1);
        game_logout(2);
        assert!(places().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_the_machine() {
        let first = system();
        assert!(first.mem_total > 0 && first.cpus > 0);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let second = system();
        assert!((0.0..=100.0).contains(&second.cpu_percent));
    }
}
