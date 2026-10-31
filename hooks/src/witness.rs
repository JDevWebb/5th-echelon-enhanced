//! Witness reports (the 0.4.8 plan's F9: research, then a prototype; off for players, on for
//! testers with `WitnessReports`). Every player's game simulates every character, so each can
//! see another player moving faster than the game allows. The add-on finds the player
//! characters through the engine's own tables (as round_state.rs finds the match manager):
//!
//! - the characters: living instances of the player pawn classes ([`PAWNS`], echelon.u),
//!   named after their class with a number;
//! - each one's `Actor.Location` and `Pawn.GroundSpeed` (engine.u), and its
//!   `Pawn.Controller`, whose `CharacterControl.m_MPNickname` (echelon.u) names its player
//!   when the game has it (whether a remote player's character has one is what the
//!   research is to learn: the counts go with each report).
//!
//! Each [`SAMPLE`] it notes where each character is; over each second, how far it went. A jump
//! of more than [`TELEPORT`] is a respawn or a correction, not speed. Every [`REPORT`], what it
//! saw goes to the server (`Misc.Witness`), which counts independent witnesses.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use tracing::info;
use tracing::warn;

use crate::dataversion::read;
use crate::dataversion::read_u32;
use crate::round_state as rs;

/// The player character classes (echelon.u: EPlayerPawn's subclasses).
const PAWNS: [&str; 9] = [
    "EMPSpy",
    "EMPMerc",
    "EMPFirstPersonMerc",
    "EPlayerSpy",
    "EPlayerMerc",
    "EPlayerAdversarial",
    "ESam",
    "ESPFirstPerson",
    "EPlayerSPFPPawn",
];
const SAMPLE: Duration = Duration::from_millis(100);
const REPORT: Duration = Duration::from_secs(60);
/// Instance numbers looked through, past the highest found.
const NUMBERS: u32 = 48;
/// Moving more than this (game units) between two samples is a teleport (a respawn), not speed.
const TELEPORT: f32 = 1500.0;
/// Speeds this many times a character's GroundSpeed count as over.
const OVER: f32 = 1.5;

/// What's looked up once the classes are loaded.
struct Tables {
    names: HashMap<&'static str, u32>,
    pawn_classes: Vec<(&'static str, usize)>,
    character_control: Option<usize>,
    location: u16,
    ground_speed: Option<u16>,
    controller: Option<u16>,
    nickname: Option<u16>,
}

fn look_up() -> Option<Tables> {
    let mut wanted: Vec<&'static str> = PAWNS.to_vec();
    wanted.extend(["Class", "Actor", "Pawn", "CharacterControl", "Location", "GroundSpeed", "Controller", "m_MPNickname"]);
    let names = rs::name_indices(&wanted);
    let pawn_classes: Vec<(&'static str, usize)> = PAWNS.iter().filter_map(|p| Some((*p, rs::class_named(&names, p)?))).collect();
    let actor = rs::class_named(&names, "Actor")?;
    let pawn = rs::class_named(&names, "Pawn")?;
    let character_control = rs::class_named(&names, "CharacterControl");
    Some(Tables {
        location: rs::offset_in(&names, "Location", actor)?,
        ground_speed: rs::offset_in(&names, "GroundSpeed", pawn),
        controller: rs::offset_in(&names, "Controller", pawn),
        nickname: character_control.and_then(|c| rs::offset_in(&names, "m_MPNickname", c)),
        character_control,
        pawn_classes,
        names,
    })
}

/// A living player character: its object.
fn characters(t: &Tables, from: &mut HashMap<&'static str, u32>) -> Vec<usize> {
    let mut out = Vec::new();
    for (name, class) in &t.pawn_classes {
        let Some(index) = t.names.get(name) else { continue };
        let start = from.get(name).copied().unwrap_or(1).saturating_sub(NUMBERS / 2).max(1);
        let mut highest = None;
        for number in start..start + NUMBERS * 2 {
            for o in rs::named(rs::pack(*index, number)) {
                if read_u32(o + 0x24) == Some(*class as u32) && rs::alive(o) {
                    out.push(o);
                    highest = Some(number);
                }
            }
        }
        if let Some(n) = highest {
            from.insert(name, n);
        }
    }
    out
}

/// An FString's text: (pointer to UTF-16, count with the NUL).
fn fstring(at: usize) -> Option<String> {
    let (data, count) = (read_u32(at)? as usize, read::<i32>(at + 4)?);
    if data == 0 || !(1..=64).contains(&count) {
        return None;
    }
    let units: Option<Vec<u16>> = (0..count as usize - 1).map(|i| read::<u16>(data + i * 2)).collect();
    Some(String::from_utf16_lossy(&units?)).filter(|s| !s.is_empty())
}

/// A character's player's name, when its controller is a CharacterControl that has one.
fn name_of(t: &Tables, character: usize) -> Option<String> {
    let controller = read_u32(character + usize::from(t.controller?))? as usize;
    if controller == 0 || !rs::chain(read_u32(controller + 0x24)? as usize).contains(&(t.character_control? as u32)) {
        return None;
    }
    fstring(controller + usize::from(t.nickname?))
}

/// One character's movement, watched.
#[derive(Default)]
struct Track {
    name: Option<String>,
    last: Option<([f32; 3], Instant)>,
    /// The second being measured: where it started, and how far it has gone.
    second: Option<(Instant, f32)>,
    max_speed: f32,
    ground_speed: f32,
    over_seconds: u32,
}

impl Track {
    fn sample(&mut self, at: [f32; 3], now: Instant) {
        if let Some((before, _)) = self.last {
            // Across the ground only: a fall isn't speed.
            let step = ((at[0] - before[0]).powi(2) + (at[1] - before[1]).powi(2)).sqrt();
            if step > TELEPORT {
                self.second = None;
            } else {
                let (start, gone) = self.second.get_or_insert((now, 0.0));
                *gone += step;
                let span = now.duration_since(*start).as_secs_f32();
                if span >= 1.0 {
                    let speed = *gone / span;
                    self.max_speed = self.max_speed.max(speed);
                    if self.ground_speed > 0.0 && speed > self.ground_speed * OVER {
                        self.over_seconds += 1;
                    }
                    self.second = None;
                }
            }
        }
        self.last = Some((at, now));
    }
}

pub fn start(me: String) {
    if !rs::tables_here() {
        info!("Witness reports: not this build's (DirectX 11 only)");
        return;
    }
    let spawned = std::thread::Builder::new().name("fe-witness".into()).spawn(move || {
        let mut tables: Option<Tables> = None;
        let mut looked: Option<Instant> = None;
        let mut numbers = HashMap::new();
        let mut tracks: HashMap<usize, Track> = HashMap::new();
        let mut reported = Instant::now();
        let mut found_list = Instant::now() - Duration::from_secs(10);
        let mut list: Vec<usize> = Vec::new();
        loop {
            std::thread::sleep(SAMPLE);
            if tables.as_ref().is_none_or(|t| t.pawn_classes.is_empty()) && looked.is_none_or(|t| t.elapsed() >= Duration::from_secs(30)) {
                looked = Some(Instant::now());
                tables = look_up();
                if let Some(t) = &tables {
                    info!(
                        "Witness reports: {} player character classes, names {}",
                        t.pawn_classes.len(),
                        if t.nickname.is_some() { "readable" } else { "not found" }
                    );
                }
            }
            let Some(t) = &tables else { continue };
            // The characters looked for once a second; their places every sample.
            if found_list.elapsed() >= Duration::from_secs(1) {
                found_list = Instant::now();
                list = characters(t, &mut numbers);
                tracks.retain(|o, _| list.contains(o));
            }
            let now = Instant::now();
            for &o in &list {
                let Some(at) = (0..3).map(|i| read::<f32>(o + usize::from(t.location) + i * 4)).collect::<Option<Vec<f32>>>() else {
                    continue;
                };
                let track = tracks.entry(o).or_default();
                if track.name.is_none() {
                    track.name = name_of(t, o);
                }
                track.ground_speed = t.ground_speed.and_then(|g| read::<f32>(o + usize::from(g))).unwrap_or(0.0);
                track.sample([at[0], at[1], at[2]], now);
            }
            if reported.elapsed() >= REPORT {
                reported = Instant::now();
                let named = tracks.values().filter(|t| t.name.is_some()).count();
                let seen: Vec<server_api::misc::Witnessed> = tracks
                    .values()
                    .filter_map(|t| {
                        let name = t.name.clone()?;
                        (!name.eq_ignore_ascii_case(&me) && t.max_speed > 0.0).then(|| server_api::misc::Witnessed {
                            name,
                            max_speed: t.max_speed,
                            ground_speed: t.ground_speed,
                            over_seconds: t.over_seconds,
                        })
                    })
                    .collect();
                if !tracks.is_empty() {
                    info!(
                        "Witness reports: {} characters, {named} named; {:?}",
                        tracks.len(),
                        seen.iter().map(|s| (&s.name, s.max_speed.round(), s.over_seconds)).collect::<Vec<_>>()
                    );
                    crate::api::witness(seen, u32::try_from(tracks.len()).unwrap_or(u32::MAX), u32::try_from(named).unwrap_or(u32::MAX));
                }
                for t in tracks.values_mut() {
                    (t.max_speed, t.over_seconds) = (0.0, 0);
                }
            }
        }
    });
    if let Err(e) = spawned {
        warn!("Witness reports: couldn't start ({e})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_over_the_limit_counts_and_a_teleport_doesnt() {
        let t0 = Instant::now();
        let mut t = Track {
            ground_speed: 400.0,
            ..Track::default()
        };
        // 1000 units a second for 2 s.
        for i in 0..=20u32 {
            t.sample([i as f32 * 100.0, 0.0, 0.0], t0 + Duration::from_millis(u64::from(i) * 100));
        }
        assert_eq!(t.over_seconds, 2);
        assert!(t.max_speed > 900.0);
        // A respawn across the map, then standing still.
        let mut r = Track {
            ground_speed: 400.0,
            ..Track::default()
        };
        r.sample([0.0, 0.0, 0.0], t0);
        r.sample([90_000.0, 0.0, 0.0], t0 + Duration::from_millis(100));
        for i in 2..=20u64 {
            r.sample([90_000.0, 0.0, 0.0], t0 + Duration::from_millis(i * 100));
        }
        assert_eq!((r.over_seconds, r.max_speed), (0, 0.0));
    }
}
