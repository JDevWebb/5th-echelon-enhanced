//! Match recordings: what one player's game saw of a match, as the add-on reads it from the
//! game's own objects (hooks' `pawns.rs` and `recording.rs`), for replays and cheat analysis (the
//! 0.5.0 plan's replay research). Positions only, never anything from the game's files: a
//! viewer draws the map from its own copy of the game.
//!
//! A file is gzip around: [`MAGIC`], a version (u16), the [`Header`], then [`Record`]s, each a
//! tag byte and its fields, little endian; a string is a u8 length and UTF-8. A game that ends
//! mid-write leaves a file whose records up to there still read ([`read`] stops at the cut).
//! Times are milliseconds from the recording's start.

use std::io::Read;
use std::io::Write;

use serde::Serialize;

pub const MAGIC: [u8; 4] = *b"FERP";
pub const VERSION: u16 = 1;
/// The files' extension.
pub const EXTENSION: &str = "fer";

/// What the recording is of.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Header {
    /// When it began (Unix seconds).
    pub started: u64,
    /// The recording player's name.
    pub recorder: String,
    /// The map's package, as the game has it loaded (e.g. `ADV_01`), when found.
    pub map: String,
    /// The match manager's class (the mode, e.g. `ESpyVSMercMatchManager`), when found.
    pub mode: String,
    /// The add-on's version.
    pub build: String,
}

/// One character's state in a [`Record::Frame`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PawnState {
    pub id: u16,
    /// Where it is (game units: about a centimetre).
    pub at: [f32; 3],
    /// Which way it faces: the game's rotator units (65536 a turn).
    pub yaw: u16,
    pub pitch: u16,
    pub health: i16,
    /// Its player holds fire (known only where the game has its controller).
    pub firing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// A character, when first seen and again when more is known of it.
    Pawn { t: u32, id: u16, class: String, name: String, team: i32 },
    /// Every character seen at `t`.
    Frame { t: u32, pawns: Vec<PawnState> },
    /// A shot from `id`'s weapon, from where to where it went (the weapon's bullet history),
    /// with the game's own time for it.
    Shot { t: u32, id: u16, from: [f32; 3], to: [f32; 3], game_time: f32 },
    /// `id`'s health went to 0.
    Death { t: u32, id: u16 },
    /// `killer`'s player was credited with `victim` (known only where the game has the
    /// killer's controller).
    Kill { t: u32, killer: u16, victim: u16 },
    /// The round's state changed (`RoundState_Started`, ...).
    Round { t: u32, state: String },
    /// `id` is gone (left, or its character was replaced).
    Gone { t: u32, id: u16 },
}

impl Record {
    pub fn t(&self) -> u32 {
        match self {
            Self::Pawn { t, .. } | Self::Frame { t, .. } | Self::Shot { t, .. } | Self::Death { t, .. } | Self::Kill { t, .. } | Self::Round { t, .. } | Self::Gone { t, .. } => *t,
        }
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let mut end = s.len().min(255);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    out.push(end as u8);
    out.extend_from_slice(&s.as_bytes()[..end]);
}

fn put_f3(out: &mut Vec<u8>, v: [f32; 3]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

/// The header's bytes, magic and version first.
pub fn encode_header(h: &Header) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&h.started.to_le_bytes());
    for s in [&h.recorder, &h.map, &h.mode, &h.build] {
        put_str(&mut out, s);
    }
    out
}

/// A record's bytes.
pub fn encode(r: &Record) -> Vec<u8> {
    let mut out = Vec::new();
    match r {
        Record::Pawn { t, id, class, name, team } => {
            out.push(1);
            out.extend_from_slice(&t.to_le_bytes());
            out.extend_from_slice(&id.to_le_bytes());
            put_str(&mut out, class);
            put_str(&mut out, name);
            out.extend_from_slice(&team.to_le_bytes());
        }
        Record::Frame { t, pawns } => {
            out.push(2);
            out.extend_from_slice(&t.to_le_bytes());
            let n = pawns.len().min(255);
            out.push(n as u8);
            for p in &pawns[..n] {
                out.extend_from_slice(&p.id.to_le_bytes());
                put_f3(&mut out, p.at);
                out.extend_from_slice(&p.yaw.to_le_bytes());
                out.extend_from_slice(&p.pitch.to_le_bytes());
                out.extend_from_slice(&p.health.to_le_bytes());
                out.push(u8::from(p.firing));
            }
        }
        Record::Shot { t, id, from, to, game_time } => {
            out.push(3);
            out.extend_from_slice(&t.to_le_bytes());
            out.extend_from_slice(&id.to_le_bytes());
            put_f3(&mut out, *from);
            put_f3(&mut out, *to);
            out.extend_from_slice(&game_time.to_le_bytes());
        }
        Record::Death { t, id } => {
            out.push(4);
            out.extend_from_slice(&t.to_le_bytes());
            out.extend_from_slice(&id.to_le_bytes());
        }
        Record::Kill { t, killer, victim } => {
            out.push(5);
            out.extend_from_slice(&t.to_le_bytes());
            out.extend_from_slice(&killer.to_le_bytes());
            out.extend_from_slice(&victim.to_le_bytes());
        }
        Record::Round { t, state } => {
            out.push(6);
            out.extend_from_slice(&t.to_le_bytes());
            put_str(&mut out, state);
        }
        Record::Gone { t, id } => {
            out.push(7);
            out.extend_from_slice(&t.to_le_bytes());
            out.extend_from_slice(&id.to_le_bytes());
        }
    }
    out
}

/// Writes a recording, gzipped, as it's made.
pub struct Writer<W: Write> {
    out: flate2::write::GzEncoder<W>,
}

impl<W: Write> Writer<W> {
    pub fn new(out: W, header: &Header) -> std::io::Result<Self> {
        let mut out = flate2::write::GzEncoder::new(out, flate2::Compression::default());
        out.write_all(&encode_header(header))?;
        Ok(Self { out })
    }

    pub fn write(&mut self, r: &Record) -> std::io::Result<()> {
        self.out.write_all(&encode(r))
    }

    /// Everything so far readable from the file (a game ending now loses nothing before it).
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }

    pub fn finish(self) -> std::io::Result<W> {
        self.out.finish()
    }
}

#[derive(Debug)]
pub enum Error {
    NotARecording,
    Version(u16),
    TooBig,
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotARecording => write!(f, "not a match recording"),
            Self::Version(v) => write!(f, "a recording of version {v}, this reads {VERSION}"),
            Self::TooBig => write!(f, "a recording that unpacks to more than a recording can hold"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn i16(&mut self) -> Option<i16> {
        Some(i16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }
    fn str(&mut self) -> Option<String> {
        let n = usize::from(self.u8()?);
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    fn record(&mut self) -> Option<Record> {
        let tag = self.u8()?;
        let t = self.u32()?;
        Some(match tag {
            1 => Record::Pawn {
                t,
                id: self.u16()?,
                class: self.str()?,
                name: self.str()?,
                team: self.i32()?,
            },
            2 => {
                let n = self.u8()?;
                let mut pawns = Vec::with_capacity(usize::from(n));
                for _ in 0..n {
                    pawns.push(PawnState {
                        id: self.u16()?,
                        at: self.f3()?,
                        yaw: self.u16()?,
                        pitch: self.u16()?,
                        health: self.i16()?,
                        firing: self.u8()? != 0,
                    });
                }
                Record::Frame { t, pawns }
            }
            3 => Record::Shot {
                t,
                id: self.u16()?,
                from: self.f3()?,
                to: self.f3()?,
                game_time: self.f32()?,
            },
            4 => Record::Death { t, id: self.u16()? },
            5 => Record::Kill {
                t,
                killer: self.u16()?,
                victim: self.u16()?,
            },
            6 => Record::Round { t, state: self.str()? },
            7 => Record::Gone { t, id: self.u16()? },
            _ => return None,
        })
    }
}

/// The most a recording unpacks to: 2 hours of 32 characters ten times a second is about
/// 50 MB; anything bigger is refused before it's all in memory (a file made to unpack huge).
pub const MAX_UNPACKED: usize = 96 * 1024 * 1024;

/// A recording's header and records: every whole record before the end, or before a cut
/// (a game that ended mid-write) or anything this version doesn't know.
pub fn read(input: impl Read) -> Result<(Header, Vec<Record>), Error> {
    read_limited(input, MAX_UNPACKED)
}

/// As [`read`], refusing one that unpacks to more than `max` bytes ([`Error::TooBig`]).
pub fn read_limited(input: impl Read, max: usize) -> Result<(Header, Vec<Record>), Error> {
    let mut bytes = Vec::new();
    // A cut gzip stream errors at its end: what came before it is kept.
    let mut gz = flate2::read::GzDecoder::new(input);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match gz.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if bytes.len() + n > max {
                    return Err(Error::TooBig);
                }
                bytes.extend_from_slice(&chunk[..n]);
            }
            Err(e) if bytes.is_empty() => return Err(Error::Io(e)),
            Err(_) => break,
        }
    }
    let mut c = Cursor { b: &bytes, at: 0 };
    if c.take(4) != Some(&MAGIC[..]) {
        return Err(Error::NotARecording);
    }
    let version = c.u16().ok_or(Error::NotARecording)?;
    if version != VERSION {
        return Err(Error::Version(version));
    }
    let header = (|| {
        Some(Header {
            started: c.u64()?,
            recorder: c.str()?,
            map: c.str()?,
            mode: c.str()?,
            build: c.str()?,
        })
    })()
    .ok_or(Error::NotARecording)?;
    let mut records = Vec::new();
    while let Some(r) = c.record() {
        records.push(r);
    }
    Ok((header, records))
}

/// What a recording holds, in short (the dump tool's first lines).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Summary {
    pub seconds: f32,
    pub frames: usize,
    pub pawns: usize,
    pub named: usize,
    pub shots: usize,
    pub deaths: usize,
    pub kills: usize,
    /// Each named player's fastest across the ground over a second (game units a second).
    pub fastest: Vec<(String, f32)>,
}

pub fn summary(records: &[Record]) -> Summary {
    use std::collections::HashMap;
    let mut s = Summary::default();
    let mut names: HashMap<u16, String> = HashMap::new();
    let mut ids = std::collections::HashSet::new();
    // Each character's place a second ago, for its speed.
    let mut trail: HashMap<u16, std::collections::VecDeque<(u32, [f32; 3])>> = HashMap::new();
    let mut fastest: HashMap<u16, f32> = HashMap::new();
    for r in records {
        s.seconds = s.seconds.max(r.t() as f32 / 1000.0);
        match r {
            Record::Pawn { id, name, .. } => {
                ids.insert(*id);
                if !name.is_empty() {
                    names.insert(*id, name.clone());
                }
            }
            Record::Frame { t, pawns } => {
                s.frames += 1;
                for p in pawns {
                    let across = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
                    let q = trail.entry(p.id).or_default();
                    // A jump past 1500 units is a respawn, as witness reports take it.
                    if q.back().is_some_and(|(_, last)| across(*last, p.at) > 1500.0) {
                        q.clear();
                    }
                    q.push_back((*t, p.at));
                    while q.front().is_some_and(|(at, _)| t - at > 1000) {
                        q.pop_front();
                    }
                    if let Some((t0, a)) = q.front() {
                        let span = (t - t0) as f32 / 1000.0;
                        if span >= 0.9 {
                            let f = fastest.entry(p.id).or_default();
                            *f = f.max(across(p.at, *a) / span);
                        }
                    }
                }
            }
            Record::Shot { .. } => s.shots += 1,
            Record::Death { .. } => s.deaths += 1,
            Record::Kill { .. } => s.kills += 1,
            Record::Gone { id, .. } => {
                trail.remove(id);
            }
            Record::Round { .. } => {}
        }
    }
    s.pawns = ids.len();
    s.named = names.len();
    let mut f: Vec<(String, f32)> = fastest.into_iter().filter_map(|(id, v)| Some((names.get(&id)?.clone(), v.round()))).collect();
    f.sort_by(|a, b| b.1.total_cmp(&a.1));
    s.fastest = f;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Header, Vec<Record>) {
        let header = Header {
            started: 1_761_900_000,
            recorder: "Kiwi".into(),
            map: "ADV_01".into(),
            mode: "ESpyVSMercMatchManager".into(),
            build: "0.5.0".into(),
        };
        let mut records = vec![
            Record::Round {
                t: 0,
                state: "RoundState_Started".into(),
            },
            Record::Pawn {
                t: 0,
                id: 1,
                class: "EMPSpy".into(),
                name: "Kiwi".into(),
                team: 0,
            },
            Record::Pawn {
                t: 0,
                id: 2,
                class: "EMPMerc".into(),
                name: "Zoomer".into(),
                team: 1,
            },
        ];
        for i in 0..=20u32 {
            records.push(Record::Frame {
                t: i * 100,
                pawns: vec![
                    PawnState {
                        id: 1,
                        at: [0.0, 0.0, 0.0],
                        yaw: 16384,
                        pitch: 0,
                        health: 100,
                        firing: i == 5,
                    },
                    // 1000 units a second.
                    PawnState {
                        id: 2,
                        at: [i as f32 * 100.0, 0.0, 0.0],
                        yaw: 0,
                        pitch: 65000,
                        health: 100,
                        firing: false,
                    },
                ],
            });
        }
        records.extend([
            Record::Shot {
                t: 500,
                id: 1,
                from: [0.0, 0.0, 150.0],
                to: [500.0, 10.0, 140.0],
                game_time: 81.5,
            },
            Record::Death { t: 600, id: 2 },
            Record::Kill { t: 600, killer: 1, victim: 2 },
            Record::Gone { t: 2100, id: 2 },
        ]);
        (header, records)
    }

    #[test]
    fn a_recording_reads_back_as_written() {
        let (header, records) = sample();
        let mut w = Writer::new(Vec::new(), &header).unwrap();
        for r in &records {
            w.write(r).unwrap();
        }
        let bytes = w.finish().unwrap();
        let (h, back) = read(&bytes[..]).unwrap();
        assert_eq!((h, &back), (header, &records));
        let s = summary(&back);
        assert_eq!((s.frames, s.pawns, s.named, s.shots, s.deaths, s.kills), (21, 2, 2, 1, 1, 1));
        assert_eq!(s.fastest[0].0, "Zoomer");
        assert!((s.fastest[0].1 - 1000.0).abs() < 1.0, "{:?}", s.fastest);
    }

    #[test]
    fn a_recording_cut_short_keeps_what_came_before() {
        let (header, records) = sample();
        let mut w = Writer::new(Vec::new(), &header).unwrap();
        for r in &records[..10] {
            w.write(r).unwrap();
        }
        // Flushed, as the add-on does every few seconds; then the game ends mid-record.
        w.flush().unwrap();
        let mut bytes = w.out.get_ref().clone();
        let flushed = bytes.len();
        w.write(&records[10]).unwrap();
        w.flush().unwrap();
        bytes.extend_from_slice(&w.out.get_ref()[flushed..flushed + 3]);
        let (_, back) = read(&bytes[..]).unwrap();
        assert_eq!(back, records[..10]);
        assert!(matches!(read(&b"not gzip"[..]), Err(Error::Io(_))));
        // One that unpacks past the limit is refused.
        let mut w = Writer::new(Vec::new(), &header).unwrap();
        for r in &records {
            w.write(r).unwrap();
        }
        let whole = w.finish().unwrap();
        assert!(matches!(read_limited(&whole[..], 100), Err(Error::TooBig)));
    }

    #[test]
    fn long_names_are_cut_on_a_character() {
        let r = Record::Round { t: 1, state: "é".repeat(200) };
        let (_, back) = read(
            &{
                let mut w = Writer::new(Vec::new(), &Header::default()).unwrap();
                w.write(&r).unwrap();
                w.finish().unwrap()
            }[..],
        )
        .unwrap();
        let Record::Round { state, .. } = &back[0] else { panic!() };
        assert_eq!(state.chars().count(), 127);
    }
}
