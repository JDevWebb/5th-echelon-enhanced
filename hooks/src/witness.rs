//! Witness reports (the 0.5.0 plan's F9: research, then a prototype; on for every player,
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
    /// W3: its weapon's newest shot time seen (the game's), and this minute's shots per weapon
    /// class: (count, the shortest gap in seconds).
    last_shot: Option<(usize, f32)>,
    fire: HashMap<String, (u32, f32)>,
}

/// Two bullet stamps closer than this are one shot's (a shotgun's pellets).
const SAME_SHOT: f32 = 0.002;

impl Track {
    /// W3: the new shots of `weapon` (class `class`), from its bullet history (oldest first).
    fn shots(&mut self, weapon: usize, class: &str, times: &[f32]) {
        let seen = match self.last_shot {
            Some((w, t)) if w == weapon => t,
            // A weapon not watched before: from now on (its history is older shots).
            _ => {
                self.last_shot = Some((weapon, times.last().copied().unwrap_or(0.0)));
                return;
            }
        };
        let mut prev = seen;
        for &t in times.iter().filter(|t| **t > seen) {
            let gap = t - prev;
            if gap < SAME_SHOT {
                continue;
            }
            let e = self.fire.entry(class.to_string()).or_insert((0, f32::MAX));
            e.0 += 1;
            if prev > 0.0 {
                e.1 = e.1.min(gap);
            }
            prev = t;
        }
        self.last_shot = Some((weapon, prev.max(seen)));
    }

    /// The weapon class fired most, its shot count and shortest gap (ms), if it fired twice.
    fn most_fired(&self) -> Option<(String, u32, f32)> {
        self.fire
            .iter()
            .filter(|(_, (n, gap))| *n >= 2 && *gap < f32::MAX)
            .max_by_key(|(_, (n, _))| *n)
            .map(|(c, (n, gap))| (c.clone(), *n, gap * 1000.0))
    }
}

impl Track {
    fn sample(&mut self, at: [f32; 3], now: Instant) {
        if let Some((before, then)) = self.last {
            // Across the ground only: a fall isn't speed.
            let step = ((at[0] - before[0]).powi(2) + (at[1] - before[1]).powi(2)).sqrt();
            if step > TELEPORT {
                self.second = None;
            } else {
                // The second starts where this step did (the previous sample), so its time covers
                // all the movement it counts.
                let (start, gone) = self.second.get_or_insert((then, 0.0));
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
    fn sample(&mut self, tables: &pawns::Tables, now: Instant, characters: &[pawns::Sample]) {
        self.tracks.retain(|o, _| characters.iter().any(|c| c.object == *o));
        for c in characters {
            let track = self.tracks.entry(c.object).or_default();
            if track.name.is_none() {
                track.name = c.control.as_ref().and_then(|k| k.name.clone());
            }
            track.ground_speed = c.ground_speed;
            track.sample(c.at, now);
            if let Some(weapon) = c.weapon {
                let class = crate::round_state::kind_name(weapon).unwrap_or_else(|| String::from("?"));
                let times: Vec<f32> = pawns::shots(tables, weapon).into_iter().map(|s| s.2).collect();
                track.shots(weapon, &class, &times);
            }
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
                let fired = t.most_fired();
                let teamkills = crate::recording::teamkills_of(&name);
                (!name.eq_ignore_ascii_case(&self.me) && (t.max_speed > 0.0 || fired.is_some() || teamkills > 0)).then(|| {
                    let (weapon, shots, fastest_shot_ms) = fired.unwrap_or_default();
                    server_api::misc::Witnessed {
                        name,
                        max_speed: t.max_speed,
                        ground_speed: t.ground_speed,
                        over_seconds: t.over_seconds,
                        weapon,
                        fastest_shot_ms,
                        shots,
                        teamkills,
                    }
                })
            })
            .collect();
        if !self.tracks.is_empty() {
            info!(
                "Witness reports: {} characters, {named} named; {:?}",
                self.tracks.len(),
                seen.iter()
                    .map(|s| (&s.name, s.max_speed.round(), s.over_seconds, &s.weapon, s.fastest_shot_ms.round(), s.shots))
                    .collect::<Vec<_>>()
            );
            let (pawns, named) = (u32::try_from(self.tracks.len()).unwrap_or(u32::MAX), u32::try_from(named).unwrap_or(u32::MAX));
            // Off the sampling thread: a slow server mustn't hold up the samples.
            let _ = std::thread::Builder::new()
                .name("fe-witness-send".into())
                .spawn(move || crate::api::witness(seen, pawns, named));
        }
        for t in self.tracks.values_mut() {
            (t.max_speed, t.over_seconds) = (0.0, 0);
            t.fire.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W3: shots are counted from the bullet history's new stamps, a shotgun's pellets as one,
    /// with the shortest gap between shots.
    #[test]
    fn shots_and_the_shortest_gap_between_them() {
        let mut t = Track::default();
        t.shots(1, "EWeaponAR", &[1.0, 1.1]);
        assert!(t.most_fired().is_none(), "the history before watching isn't counted");
        t.shots(1, "EWeaponAR", &[1.0, 1.1, 1.2, 1.35, 1.351, 1.5]);
        let (class, n, gap) = t.most_fired().unwrap();
        assert_eq!((class.as_str(), n), ("EWeaponAR", 3), "1.351 is 1.35's pellet");
        assert!((gap - 100.0).abs() < 1.0, "{gap}");
        t.shots(2, "EWeaponPistol", &[9.0]);
        assert_eq!(t.most_fired().unwrap().0, "EWeaponAR", "a new weapon starts from its newest shot");
    }

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
        // Measured as it was, not 10% over (a second's time covers every step it counts).
        assert!((t.max_speed - 1000.0).abs() < 1.0, "{}", t.max_speed);
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
