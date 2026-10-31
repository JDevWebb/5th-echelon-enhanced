//! Match recordings the players' games send (the replay research, step 2; `Misc.Recording`):
//! each kept as it came, gzipped in the `replay` crate's format, in `data/recordings` beside
//! a note of what it is, for [`KEEP_FOR`] and at most [`MOST_KEPT_BYTES`] in all (the oldest
//! go first). Nothing reads them here but the check that one is a recording: an admin who
//! wants one has the coordinator ask for it (`send_recording`, an admin action), and
//! [`file`] is what's sent.
//!
//! A player sends at most one every [`EVERY`] and [`MOST_A_DAY`] a day, each at most
//! [`MAX_UPLOAD`] (the RPC's limit) and unpacking to at most the crate's limit.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde::Serialize;

/// The most one recording may be, as sent (packed).
pub const MAX_UPLOAD: usize = 7 * 1024 * 1024 + 512 * 1024;
const KEEP_FOR: i64 = 7 * 86_400;
const MOST_KEPT_BYTES: u64 = 3 * 1024 * 1024 * 1024;
const EVERY: Duration = Duration::from_secs(60);
const MOST_A_DAY: u32 = 40;

fn dir() -> PathBuf {
    PathBuf::from("data").join("recordings")
}

/// What one is, kept beside it (`<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Kept {
    pub id: String,
    /// Who sent it (their account here), and when (Unix seconds).
    pub player: u32,
    pub received: i64,
    pub size: u64,
    /// From its header (the recorder's own word) and records.
    pub started: u64,
    pub map: String,
    pub mode: String,
    pub seconds: f32,
    pub characters: usize,
    pub named: usize,
    pub shots: usize,
    pub kills: usize,
}

/// Why one wasn't taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    TooSoon,
    TooMany,
    TooBig,
    NotARecording(String),
    Empty,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooSoon => write!(f, "one recording a minute"),
            Self::TooMany => write!(f, "{MOST_A_DAY} recordings a day"),
            Self::TooBig => write!(f, "a recording is at most {} MB", MAX_UPLOAD / (1024 * 1024)),
            Self::NotARecording(why) => write!(f, "not a match recording ({why})"),
            Self::Empty => write!(f, "a recording with no characters in it"),
        }
    }
}

/// Each player's last upload and today's count (the day's start, count).
fn limits() -> std::sync::MutexGuard<'static, HashMap<u32, (Instant, Instant, u32)>> {
    static LIMITS: std::sync::LazyLock<Mutex<HashMap<u32, (Instant, Instant, u32)>>> = std::sync::LazyLock::new(Mutex::default);
    LIMITS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether `player` may send one now; counted when they may.
fn may_send(player: u32, now: Instant) -> Result<(), Refused> {
    let mut all = limits();
    if all.len() > 20_000 {
        all.retain(|_, (last, _, _)| now.duration_since(*last) < Duration::from_secs(86_400));
    }
    let entry = all.entry(player).or_insert((now - EVERY, now, 0));
    if now.duration_since(entry.0) < EVERY {
        return Err(Refused::TooSoon);
    }
    if now.duration_since(entry.1) >= Duration::from_secs(86_400) {
        (entry.1, entry.2) = (now, 0);
    }
    if entry.2 >= MOST_A_DAY {
        return Err(Refused::TooMany);
    }
    entry.0 = now;
    entry.2 += 1;
    Ok(())
}

/// What `data` holds, if it's a recording with characters in it.
pub fn describe(player: u32, data: &[u8], received: i64) -> Result<Kept, Refused> {
    if data.len() > MAX_UPLOAD {
        return Err(Refused::TooBig);
    }
    let (header, records) = replay::read(data).map_err(|e| Refused::NotARecording(e.to_string()))?;
    let s = replay::summary(&records);
    if s.pawns == 0 {
        return Err(Refused::Empty);
    }
    let clean = |t: &str| t.chars().filter(|c| !c.is_control()).take(64).collect::<String>();
    use rand::Rng as _;
    let id: String = (0..16).map(|_| format!("{:x}", rand::rng().random_range(0..16u8))).collect();
    Ok(Kept {
        id,
        player,
        received,
        size: data.len() as u64,
        started: header.started,
        map: clean(&header.map),
        mode: clean(&header.mode),
        seconds: s.seconds,
        characters: s.pawns,
        named: s.named,
        shots: s.shots,
        kills: s.kills,
    })
}

/// Keeps `player`'s recording `data`: what it was kept as.
pub fn keep(player: u32, data: &[u8]) -> Result<Kept, Refused> {
    let now = identity::now();
    // Checked before it's counted, so a broken file doesn't use up the minute.
    let kept = describe(player, data, now)?;
    may_send(player, Instant::now())?;
    let dir = dir();
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join(format!("{}.{}", kept.id, replay::EXTENSION)), data))
        .and_then(|()| std::fs::write(dir.join(format!("{}.json", kept.id)), serde_json::to_vec(&kept).unwrap_or_default()));
    if let Err(e) = written {
        return Err(Refused::NotARecording(format!("couldn't keep it: {e}")));
    }
    prune(&dir, now);
    Ok(kept)
}

/// A kept recording's note and bytes, by id (16 hex characters, nothing else).
pub fn file(id: &str) -> Option<(Kept, Vec<u8>)> {
    if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let dir = dir();
    let kept: Kept = serde_json::from_slice(&std::fs::read(dir.join(format!("{id}.json"))).ok()?).ok()?;
    let data = std::fs::read(dir.join(format!("{id}.{}", replay::EXTENSION))).ok()?;
    Some((kept, data))
}

/// Recordings past [`KEEP_FOR`] deleted, then the oldest until they fit [`MOST_KEPT_BYTES`].
fn prune(dir: &std::path::Path, now: i64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut all: Vec<Kept> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    all.sort_by_key(|k| k.received);
    let mut total: u64 = all.iter().map(|k| k.size).sum();
    for k in all {
        if k.received >= now - KEEP_FOR && total <= MOST_KEPT_BYTES {
            break;
        }
        total = total.saturating_sub(k.size);
        let _ = std::fs::remove_file(dir.join(format!("{}.{}", k.id, replay::EXTENSION)));
        let _ = std::fs::remove_file(dir.join(format!("{}.json", k.id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(characters: u16) -> Vec<u8> {
        let mut w = replay::Writer::new(
            Vec::new(),
            &replay::Header {
                map: "ADV_01\u{7}".into(),
                ..Default::default()
            },
        )
        .unwrap();
        for id in 1..=characters {
            w.write(&replay::Record::Pawn {
                t: 0,
                id,
                class: "EMPSpy".into(),
                name: format!("P{id}"),
                team: 0,
            })
            .unwrap();
        }
        w.write(&replay::Record::Frame { t: 61_000, pawns: vec![] }).unwrap();
        w.finish().unwrap()
    }

    #[test]
    fn a_recording_is_described_and_limited() {
        let k = describe(7, &recording(3), 1000).unwrap();
        assert_eq!((k.player, k.characters, k.named, k.map.as_str(), k.seconds.round()), (7, 3, 3, "ADV_01", 61.0));
        assert_eq!(k.id.len(), 16);
        assert_eq!(describe(7, &recording(0), 1000), Err(Refused::Empty));
        assert!(matches!(describe(7, b"nope", 1000), Err(Refused::NotARecording(_))));
        assert_eq!(describe(7, &vec![0; MAX_UPLOAD + 1], 1000), Err(Refused::TooBig));
        // One a minute, 40 a day.
        let t = Instant::now();
        assert_eq!(may_send(900_001, t), Ok(()));
        assert_eq!(may_send(900_001, t + Duration::from_secs(30)), Err(Refused::TooSoon));
        for i in 1..MOST_A_DAY {
            assert_eq!(may_send(900_001, t + Duration::from_secs(60 * u64::from(i))), Ok(()));
        }
        assert_eq!(may_send(900_001, t + Duration::from_secs(60 * 41)), Err(Refused::TooMany));
        assert_eq!(may_send(900_001, t + Duration::from_secs(86_400 + 60)), Ok(()));
        assert!(file("../../etc/passwd").is_none());
        assert!(file("0123456789abcdeg").is_none());
    }
}
