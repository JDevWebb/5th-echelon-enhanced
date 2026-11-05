//! Match recordings (the 0.5.0 plan's replay research, step 1: on this PC only). While a match
//! has player characters in it, every sample of them (pawns.rs, ten a second) goes to a file in
//! the game folder's `replays`: where each is and faces, its health, whether its player holds
//! fire, each shot its weapon's bullet history adds (from where to where), each death, each
//! kill a controller the game has here is credited with, and the round's state. The format is
//! the `replay` crate's; `replay-dump` reads it. The newest [`KEPT`] are kept.
//!
//! On for every player (all are its testers, the user decided 2026-10-31); `ReplayRecording =
//! false` turns it off. Each finished one of a minute or more with two characters or more goes
//! to the player's server (step 2, `Misc.Recording`; `ReplayUpload = false` keeps them here),
//! which keeps it a week for its admins' replay viewer. What the logs' lines say (how many
//! characters, how many named, shots, kills) is what step 1 is to learn.

use std::collections::HashMap;
use std::collections::HashSet;
use std::io::BufWriter;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use replay::PawnState;
use replay::Record;
use tracing::info;
use tracing::warn;

use crate::pawns;
use crate::round_state as rs;

/// Recordings kept in the folder (the oldest go).
const KEPT: usize = 20;
/// A recording ends once fewer than two characters (what a match needs to begin one) have been
/// seen for this long: the lobby shows a character of its own (ESam), so "none" never came
/// after a match and the recording ran on (the gamebox, 2026-11-05).
const ENDS_AFTER: Duration = Duration::from_secs(30);
/// And at the latest after this, another beginning straight after.
const LONGEST: Duration = Duration::from_secs(2 * 3600);
const FLUSH_EVERY: Duration = Duration::from_secs(5);
const MAP_CHECK_EVERY: Duration = Duration::from_secs(10);
/// Recordings this long or longer go to the server.
const UPLOAD_FROM: Duration = Duration::from_secs(60);
/// The most one may be to go (the server's limit is 7.5 MB).
const MAX_UPLOAD: u64 = 7 * 1024 * 1024;

pub struct Recorder {
    dir: PathBuf,
    me: String,
    open: Option<Open>,
    last_seen: Option<Instant>,
    failed: bool,
    upload: bool,
}

struct Open {
    writer: replay::Writer<BufWriter<std::fs::File>>,
    path: PathBuf,
    began: Instant,
    flushed: Instant,
    ids: HashMap<usize, u16>,
    /// Every object's latest id, gone ones too (a victim is often gone before its killer's
    /// controller is read).
    every_id: HashMap<usize, u16>,
    next_id: u16,
    /// What each id was last described as: class, name, team.
    described: HashMap<u16, (String, String, i32)>,
    health: HashMap<u16, i32>,
    /// Each weapon's newest shot written (the game's time).
    weapons: HashMap<usize, f32>,
    /// Kills seen: (victim, the game's time).
    kills: HashSet<(usize, u32)>,
    round: String,
    /// The map it began on, and when that was last checked (a new map is a new recording).
    map: String,
    map_checked: Instant,
    counts: Counts,
}

#[derive(Default)]
struct Counts {
    characters: HashSet<u16>,
    named: HashSet<u16>,
    shots: usize,
    deaths: usize,
    kills: usize,
}

impl Recorder {
    pub fn new(game_dir: &std::path::Path, me: String, upload: bool) -> Self {
        Self {
            dir: game_dir.join("replays"),
            me,
            open: None,
            last_seen: None,
            failed: false,
            upload,
        }
    }

    fn begin(&mut self, tables: &pawns::Tables) -> Option<Open> {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            warn!("Recording: couldn't make {} ({e})", self.dir.display());
            return None;
        }
        prune(&self.dir);
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let map = rs::map_or_server(&tables.names).unwrap_or_default();
        let (round, _, mode) = rs::current();
        let safe: String = map.chars().filter(char::is_ascii_alphanumeric).take(32).collect();
        let path = self
            .dir
            .join(format!("match-{started}{}{safe}.{}", if safe.is_empty() { "" } else { "-" }, replay::EXTENSION));
        let header = replay::Header {
            started,
            recorder: self.me.clone(),
            map: map.clone(),
            mode: mode.clone(),
            build: env!("CARGO_PKG_VERSION").to_string(),
        };
        let file = match std::fs::File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                warn!("Recording: couldn't make {} ({e})", path.display());
                return None;
            }
        };
        let mut writer = replay::Writer::new(BufWriter::new(file), &header).ok()?;
        let _ = writer.write(&Record::Round { t: 0, state: round.clone() });
        crate::net_capture::begin(&path, &self.me);
        info!(
            "Recording: the match on {} ({}) to {}",
            if map.is_empty() { "an unknown map" } else { &map },
            if mode.is_empty() { "mode unknown" } else { &mode },
            path.display()
        );
        Some(Open {
            writer,
            path,
            began: Instant::now(),
            flushed: Instant::now(),
            ids: HashMap::new(),
            every_id: HashMap::new(),
            next_id: 1,
            described: HashMap::new(),
            health: HashMap::new(),
            weapons: HashMap::new(),
            kills: HashSet::new(),
            round,
            map,
            map_checked: Instant::now(),
            counts: Counts::default(),
        })
    }

    fn end(&mut self) {
        let Some(mut o) = self.open.take() else { return };
        crate::net_capture::end();
        let t = ms(o.began);
        for id in o.ids.values() {
            let _ = o.writer.write(&Record::Gone { t, id: *id });
        }
        let c = &o.counts;
        info!(
            "Recording: ended after {} s: {} characters ({} named), {} shots, {} deaths, {} kills: {}",
            o.began.elapsed().as_secs(),
            c.characters.len(),
            c.named.len(),
            c.shots,
            c.deaths,
            c.kills,
            o.path.display()
        );
        if let Err(e) = o.writer.finish() {
            warn!("Recording: couldn't finish {} ({e})", o.path.display());
            return;
        }
        if self.upload && o.began.elapsed() >= UPLOAD_FROM && c.characters.len() >= 2 {
            let path = o.path.clone();
            // Off the sampling thread: the next match mustn't wait on the server.
            let _ = std::thread::Builder::new().name("fe-recording-send".into()).spawn(move || send(&path));
        }
    }
}

/// Sends a finished recording to the player's server.
fn send(path: &std::path::Path) {
    match std::fs::metadata(path).map(|m| m.len()) {
        Ok(size) if size > MAX_UPLOAD => {
            info!("Recording: {} is {} MB, more than the server takes; kept here only", path.display(), size / (1024 * 1024));
            return;
        }
        Err(e) => {
            warn!("Recording: couldn't read {} ({e})", path.display());
            return;
        }
        Ok(_) => {}
    }
    let Ok(data) = std::fs::read(path) else { return };
    let size = data.len();
    match crate::api::recording(data) {
        Ok(id) => info!("Recording: sent to the server ({} KB, kept there as {id})", size / 1024),
        Err(e) => warn!("Recording: the server didn't take it ({e}); kept here"),
    }
}

fn ms(since: Instant) -> u32 {
    u32::try_from(since.elapsed().as_millis()).unwrap_or(u32::MAX)
}

/// The oldest recordings beyond the newest [`KEPT`] - 1 deleted (one is about to begin).
fn prune(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == replay::EXTENSION))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort();
    let over = files.len().saturating_sub(KEPT - 1);
    for (_, p) in files.into_iter().take(over) {
        let _ = std::fs::remove_file(p);
    }
}

impl pawns::Watcher for Recorder {
    fn sample(&mut self, tables: &pawns::Tables, now: Instant, characters: &[pawns::Sample]) {
        if self.failed {
            return;
        }
        if characters.len() >= 2 {
            self.last_seen = Some(now);
        }
        let quiet = self.last_seen.is_none_or(|at| now.duration_since(at) >= ENDS_AFTER);
        let new_map = self.open.as_mut().is_some_and(|o| {
            if o.map_checked.elapsed() < MAP_CHECK_EVERY {
                return false;
            }
            o.map_checked = Instant::now();
            rs::map_name(&tables.names).is_some_and(|m| !o.map.is_empty() && m != o.map)
        });
        if self.open.as_ref().is_some_and(|o| quiet || new_map || o.began.elapsed() >= LONGEST) {
            self.end();
        }
        // A match: two characters or more.
        if self.open.is_none() && characters.len() >= 2 {
            self.open = self.begin(tables);
            self.failed = self.open.is_none();
        }
        let Some(o) = &mut self.open else { return };
        if let Err(e) = record(o, tables, characters) {
            warn!("Recording: writing {} failed ({e}); not recording again this run", o.path.display());
            crate::net_capture::end();
            self.open = None;
            self.failed = true;
        }
    }
}

/// One sample written: new and gone characters, the frame, shots, deaths, kills, the round.
fn record(o: &mut Open, tables: &pawns::Tables, characters: &[pawns::Sample]) -> std::io::Result<()> {
    let t = ms(o.began);
    let first = o.ids.is_empty() && o.counts.characters.is_empty();
    // Gone first: an object freed and made again is a new character.
    let gone: Vec<usize> = o.ids.keys().filter(|k| !characters.iter().any(|c| c.object == **k)).copied().collect();
    for object in gone {
        if let Some(id) = o.ids.remove(&object) {
            o.writer.write(&Record::Gone { t, id })?;
            o.health.remove(&id);
        }
    }
    let mut frame = Vec::with_capacity(characters.len());
    for c in characters {
        let id = match o.ids.get(&c.object) {
            Some(id) => *id,
            None => {
                let id = o.next_id;
                o.next_id = o.next_id.wrapping_add(1).max(1);
                o.ids.insert(c.object, id);
                o.every_id.insert(c.object, id);
                id
            }
        };
        o.counts.characters.insert(id);
        let name = c.control.as_ref().and_then(|k| k.name.clone()).unwrap_or_default();
        let team = c.control.as_ref().and_then(|k| k.team).unwrap_or(-1);
        let now = (c.class.to_string(), name.clone(), team);
        // Described when new, and again when its name or team is learnt (never forgotten).
        let before = o.described.get(&id);
        if before.is_none_or(|(_, n, tm)| (!name.is_empty() && *n != name) || (team != -1 && *tm != team)) {
            let merged = (
                now.0,
                if name.is_empty() { before.map(|b| b.1.clone()).unwrap_or_default() } else { name },
                if team == -1 { before.map_or(-1, |b| b.2) } else { team },
            );
            o.writer.write(&Record::Pawn {
                t,
                id,
                class: merged.0.clone(),
                name: merged.1.clone(),
                team: merged.2,
            })?;
            if !merged.1.is_empty() {
                o.counts.named.insert(id);
            }
            o.described.insert(id, merged);
        }
        let health = c.health.unwrap_or(0);
        if o.health.insert(id, health).is_some_and(|h| h > 0) && health <= 0 {
            o.writer.write(&Record::Death { t, id })?;
            o.counts.deaths += 1;
        }
        let (pitch, yaw) = c.rotation.unwrap_or((0, 0));
        frame.push(PawnState {
            id,
            at: c.at,
            yaw: (yaw & 0xffff) as u16,
            pitch: (pitch & 0xffff) as u16,
            health: health.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            firing: c.control.as_ref().is_some_and(|k| k.firing),
        });
    }
    o.writer.write(&Record::Frame { t, pawns: frame })?;
    // Shots: a weapon's history from when it was first seen on.
    for c in characters {
        let (Some(weapon), Some(id)) = (c.weapon, o.ids.get(&c.object).copied()) else {
            continue;
        };
        let shots = pawns::shots(tables, weapon);
        let newest = shots.last().map_or(0.0, |s| s.2);
        match o.weapons.get(&weapon).copied() {
            None => {}
            Some(seen) => {
                for (from, to, game_time) in shots.iter().filter(|s| s.2 > seen) {
                    o.writer.write(&Record::Shot {
                        t,
                        id,
                        from: *from,
                        to: *to,
                        game_time: *game_time,
                    })?;
                    o.counts.shots += 1;
                }
            }
        }
        let seen = o.weapons.entry(weapon).or_insert(newest);
        *seen = seen.max(newest);
    }
    // Kills: a controller's last victim and time, new since the recording began.
    for c in characters {
        let Some((victim, at)) = c.control.as_ref().and_then(|k| k.last_kill) else { continue };
        if !o.kills.insert((victim, at.to_bits())) || first {
            continue;
        }
        if let (Some(killer), Some(victim)) = (o.ids.get(&c.object), o.every_id.get(&victim)) {
            o.writer.write(&Record::Kill {
                t,
                killer: *killer,
                victim: *victim,
            })?;
            o.counts.kills += 1;
        }
    }
    let (round, _, _) = rs::current();
    if round != o.round {
        o.writer.write(&Record::Round { t, state: round.clone() })?;
        o.round = round;
    }
    if o.flushed.elapsed() >= FLUSH_EVERY {
        o.flushed = Instant::now();
        o.writer.flush()?;
    }
    Ok(())
}
