//! Witness reports (the 0.4.8 plan's F9: research, then a prototype; on for every player,
//! `WitnessReports = false` turns it off). Every player's game simulates every character, so
//! each can see another player moving faster than the game allows. The characters come from
//! pawns.rs (sampled every [`pawns::SAMPLE`]); over each second, how far each went. A jump of
//! more than [`TELEPORT`] is a respawn or a correction, not speed. Every [`REPORT`], what it
//! saw goes to the server (`Misc.Witness`), which counts independent witnesses.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use tracing::info;

use crate::pawns;

const REPORT: Duration = Duration::from_secs(60);
/// Moving more than this (game units) between two samples is a teleport (a respawn), not speed.
const TELEPORT: f32 = 1500.0;
/// Speeds this many times a character's GroundSpeed count as over.
const OVER: f32 = 1.5;

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

/// The witness: what it watches of each character, and when it last reported.
pub struct Witness {
    me: String,
    tracks: HashMap<usize, Track>,
    reported: Instant,
}

impl Witness {
    pub fn new(me: String) -> Self {
        Self {
            me,
            tracks: HashMap::new(),
            reported: Instant::now(),
        }
    }
}

impl pawns::Watcher for Witness {
    fn sample(&mut self, _: &pawns::Tables, now: Instant, characters: &[pawns::Sample]) {
        self.tracks.retain(|o, _| characters.iter().any(|c| c.object == *o));
        for c in characters {
            let track = self.tracks.entry(c.object).or_default();
            if track.name.is_none() {
                track.name = c.control.as_ref().and_then(|k| k.name.clone());
            }
            track.ground_speed = c.ground_speed;
            track.sample(c.at, now);
        }
        if self.reported.elapsed() < REPORT {
            return;
        }
        self.reported = Instant::now();
        let named = self.tracks.values().filter(|t| t.name.is_some()).count();
        let seen: Vec<server_api::misc::Witnessed> = self
            .tracks
            .values()
            .filter_map(|t| {
                let name = t.name.clone()?;
                (!name.eq_ignore_ascii_case(&self.me) && t.max_speed > 0.0).then(|| server_api::misc::Witnessed {
                    name,
                    max_speed: t.max_speed,
                    ground_speed: t.ground_speed,
                    over_seconds: t.over_seconds,
                })
            })
            .collect();
        if !self.tracks.is_empty() {
            info!(
                "Witness reports: {} characters, {named} named; {:?}",
                self.tracks.len(),
                seen.iter().map(|s| (&s.name, s.max_speed.round(), s.over_seconds)).collect::<Vec<_>>()
            );
            let (pawns, named) = (u32::try_from(self.tracks.len()).unwrap_or(u32::MAX), u32::try_from(named).unwrap_or(u32::MAX));
            // Off the sampling thread: a slow server mustn't hold up the samples.
            let _ = std::thread::Builder::new()
                .name("fe-witness-send".into())
                .spawn(move || crate::api::witness(seen, pawns, named));
        }
        for t in self.tracks.values_mut() {
            (t.max_speed, t.over_seconds) = (0.0, 0);
        }
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
