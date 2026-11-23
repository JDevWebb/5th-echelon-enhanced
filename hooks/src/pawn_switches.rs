//! 0.5.2's P1 (report only): the cheat switches on this player's own character and its
//! controller, read every few seconds in a match under way. Any of them on is told the server
//! once a match (`fair_play`, check `pawn`): god mode, invincibility, ghost mode, invisibility,
//! blocked damage, unlimited inventory, overridden health tuning; and health over the
//! character's own starting health (EPawn.InitialHealth). No stock multiplayer character turns
//! them on or goes over it.
//!
//! Where they are: a class's bools share 32-bit words, in declaration order (Blacklist.umd's
//! package order), one bit each, from the class's first bool word. Most of these have no name
//! in this build's name table, so each is placed by its index in its class's declarations, and
//! taken only when the class has exactly as many bools as the script declares (Pawn 26, EPawn
//! 85, CharacterControl 39; the gamebox, 14 Nov: Pawn's at 884, EPawn's from 1248,
//! CharacterControl's from 1376, and the two named ones, EPawn's m_bUseNightVision (38) and
//! CharacterControl's m_bUseRumble (2), exactly where that order puts them).

use std::time::Duration;
use std::time::Instant;

use tracing::info;
use tracing::warn;

use crate::dataversion::read;
use crate::dataversion::read_u32;
use crate::pawns;
use crate::round_state as rs;

/// The switches: (class, how many bools it declares, (index among them, name)).
const SWITCHES: [(&str, usize, &[(usize, &str)]); 3] = [
    (
        "Pawn",
        26,
        &[(2, "m_bInvisible"), (3, "m_bStateInvisible"), (4, "m_bStateUnhittable"), (25, "m_bBlockDamage")],
    ),
    (
        "EPawn",
        85,
        &[
            (0, "m_bInvincible"),
            (1, "m_bOverrideHealthTuning"),
            (3, "m_bGodMode"),
            (4, "m_bGodModeOverride"),
            (5, "m_bGhostMode"),
            (45, "bDamageInsensitive"),
        ],
    ),
    ("CharacterControl", 39, &[(30, "m_bUnlimitedInventory")]),
];

/// FN: switches co-op itself turns on (a downed or reviving player, a cutscene: 128 invisible
/// notes in two days, all in co-op, mostly pairs of teammates at the same moments, with the
/// unhittable and god mode override ones beside them), so not read in co-op.
const NOT_IN_COOP: [&str; 3] = ["m_bStateInvisible", "m_bStateUnhittable", "m_bGodModeOverride"];

/// Whether a match state is co-op's (`CoopMissionState_…`).
fn coop(state: &str) -> bool {
    state.starts_with("CoopMissionState")
}

/// P1: the character's health system (EPawn.m_pHealthSystem) and its numbers, by their places in
/// the script's layout (blacklist-re `lead_layout.py`: the pointer at 1244 on EPawn; HealthSystem's
/// m_initialHealth 40, m_MaxHealth 104, m_currentHealth 108, floats). Pawn.Health is only a copy the
/// game writes back from here; a cheat that keeps a character alive edits these. Read only when the
/// object there is a HealthSystem (its class or one made from it).
const HEALTH_SYSTEM_AT: usize = 1244;
const HS_INITIAL: usize = 40;
const HS_MAX: usize = 104;
const HS_CURRENT: usize = 108;

/// The own character's health system: (current, max, initial), when it's one.
fn health_system(character: usize) -> Option<(f32, f32, f32)> {
    let hs = read_u32(character + HEALTH_SYSTEM_AT).filter(|h| *h != 0)? as usize;
    let names = rs::name_indices(&["HealthSystem"]);
    let packed = rs::pack(*names.get("HealthSystem")?, 0);
    if !read_u32(hs + 0x24).is_some_and(|c| rs::made_from(c as usize, packed)) {
        return None;
    }
    let f = |at: usize| read::<f32>(hs + at).filter(|v| v.is_finite());
    Some((f(HS_CURRENT)?, f(HS_MAX)?, f(HS_INITIAL)?))
}

/// What P1 makes of a health system's numbers: current over max, or max over the character's
/// own starting health (no stock character's max is above it).
fn health_says(current: f32, max: f32, initial: Option<i32>) -> Option<String> {
    if max > 0.0 && current > max + 1.0 {
        return Some(format!("health system {current:.0} of max {max:.0}"));
    }
    #[allow(clippy::cast_precision_loss)]
    let start = initial.filter(|i| *i > 0)? as f32;
    (max > start * 1.01 + 1.0).then(|| format!("health system max {max:.0} over the character's {start:.0}"))
}

/// How often the own character is read.
const EVERY: Duration = Duration::from_secs(3);
/// How often the look-up is tried again while it found nothing (each is a few whole scans).
const LOOK_AGAIN: Duration = Duration::from_secs(60);
const MOST_LOOKS: u32 = 10;

/// A switch's place: on the character (true) or its controller, the byte offset and the bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Place {
    name: &'static str,
    on_character: bool,
    offset: u16,
    mask: u32,
}

/// The places of `wanted` (index, name) among a class's bools as the engine has them, when
/// their count is the script's.
fn placed(bools: &[(u16, u32)], declared: usize, wanted: &[(usize, &'static str)], on_character: bool) -> Vec<Place> {
    if bools.len() != declared {
        return Vec::new();
    }
    wanted
        .iter()
        .filter_map(|(i, name)| {
            bools.get(*i).map(|(offset, mask)| Place {
                name,
                on_character,
                offset: *offset,
                mask: *mask,
            })
        })
        .collect()
}

fn look_up() -> Vec<Place> {
    let mut out = Vec::new();
    for (class, declared, wanted) in SWITCHES {
        let bools: Vec<(u16, u32)> = rs::class_bools(class).into_iter().map(|(o, m, _)| (o, m)).collect();
        let found = placed(&bools, declared, wanted, class != "CharacterControl");
        if found.is_empty() {
            info!(
                "Character switches: {class} has {} bools, not the {declared} expected: its switches aren't read",
                bools.len()
            );
        }
        out.extend(found);
    }
    out
}

/// The switches on, of `places`, for a character and its controller.
fn on(places: &[Place], character: usize, controller: Option<usize>) -> Vec<&'static str> {
    places
        .iter()
        .filter(|p| {
            let object = if p.on_character { Some(character) } else { controller };
            object.and_then(|o| read_u32(o + usize::from(p.offset))).is_some_and(|w| w & p.mask != 0)
        })
        .map(|p| p.name)
        .collect()
}

pub struct PawnSwitches {
    me: String,
    places: Option<Vec<Place>>,
    /// EPawn.InitialHealth's offset (found by name).
    initial_health: Option<u16>,
    /// Reads in a row with health over it.
    over: u32,
    /// Reads in a row with the health system over its limits (P1).
    hs_over: u32,
    looked: Option<Instant>,
    looks: u32,
    read: Option<Instant>,
    said: bool,
    #[cfg(feature = "test-commands")]
    logged: Option<Instant>,
}

impl PawnSwitches {
    pub fn new(me: String) -> Self {
        Self {
            me,
            places: None,
            initial_health: None,
            over: 0,
            hs_over: 0,
            looked: None,
            looks: 0,
            read: None,
            said: false,
            #[cfg(feature = "test-commands")]
            logged: None,
        }
    }

    fn own(&self, c: &pawns::Sample) -> bool {
        let own = pawns::own_peer();
        (own != 0 && c.peer.is_some_and(|p| u32::from(p) == own)) || c.control.as_ref().and_then(|k| k.name.as_deref()).is_some_and(|n| n.eq_ignore_ascii_case(&self.me))
    }
}

impl pawns::Watcher for PawnSwitches {
    fn sample(&mut self, tables: &pawns::Tables, now: Instant, characters: &[pawns::Sample]) {
        let (state, under_way, _) = rs::current();
        if !under_way {
            self.said = false;
            return;
        }
        if self.read.is_some_and(|r| now.duration_since(r) < EVERY) || characters.is_empty() {
            return;
        }
        self.read = Some(now);
        if self.places.as_ref().is_none_or(Vec::is_empty) {
            if self.looks >= MOST_LOOKS || self.looked.is_some_and(|l| now.duration_since(l) < LOOK_AGAIN) {
                return;
            }
            self.looked = Some(now);
            self.looks += 1;
            let places = look_up();
            let names = rs::name_indices(&["Class", "EPawn", "InitialHealth"]);
            self.initial_health = rs::class_named(&names, "EPawn").and_then(|c| rs::offset_in(&names, "InitialHealth", c));
            info!("Character switches: starting health at {:?}", self.initial_health);
            info!(
                "Character switches: {} of 11 found: {:?}",
                places.len(),
                places.iter().map(|p| (p.name, p.offset, p.mask)).collect::<Vec<_>>()
            );
            self.places = Some(places);
        }
        let Some(places) = self.places.as_ref().filter(|p| !p.is_empty()) else { return };
        #[cfg(feature = "test-commands")]
        if self.logged.is_none_or(|l| now.duration_since(l) >= Duration::from_secs(20)) {
            self.logged = Some(now);
            let all: Vec<String> = characters
                .iter()
                .map(|c| {
                    let name = c.control.as_ref().and_then(|k| k.name.clone()).unwrap_or_default();
                    let initial = self.initial_health.and_then(|o| read::<i32>(c.object + usize::from(o)));
                    format!(
                        "{name}({:?}{}): {:?}, health {:?} of {initial:?}, health system {:?}",
                        c.peer,
                        if self.own(c) { ", own" } else { "" },
                        on(places, c.object, pawns::controller_of(tables, c.object)),
                        c.health,
                        health_system(c.object)
                    )
                })
                .collect();
            info!("Character switches: test: {}", all.join("; "));
        }
        let Some(own) = characters.iter().find(|c| self.own(c)) else { return };
        #[cfg(feature = "test-commands")]
        test_command(places, own);
        if self.said {
            return;
        }
        let in_coop = coop(&state);
        let mut found: Vec<String> = on(places, own.object, pawns::controller_of(tables, own.object))
            .into_iter()
            .filter(|name| !(in_coop && NOT_IN_COOP.contains(name)))
            .map(String::from)
            .collect();
        // Health over the character's own starting health (spies 90, mercs 170 in SvM: the
        // gamebox, 14 Nov), on two reads in a row: no stock character goes over it. Pawn.Health
        // is a copy: the game writes it back from its health system (m_pHealthSystem) within
        // seconds (999 written once was 170 again by the next read), so only a cheat that keeps
        // writing it shows here; one that edits the health system needs that object's layout.
        let initial = self.initial_health.and_then(|o| read::<i32>(own.object + usize::from(o))).filter(|i| *i > 0);
        // P1: the health system's own numbers, on two reads in a row.
        match health_system(own.object).and_then(|(current, max, _)| health_says(current, max, initial)) {
            Some(said) => {
                self.hs_over += 1;
                if self.hs_over >= 2 {
                    found.push(said);
                }
            }
            None => self.hs_over = 0,
        }
        match (own.health, initial) {
            (Some(health), Some(initial)) if health > initial => {
                self.over += 1;
                if self.over >= 2 {
                    found.push(format!("health {health} of {initial}"));
                }
            }
            _ => self.over = 0,
        }
        if found.is_empty() {
            return;
        }
        self.said = true;
        let detail = format!("own character: {}", found.join(", "));
        warn!("Character switches: {detail}");
        let count = found.len();
        let spawned = std::thread::Builder::new().name("fe-pawn-switch".into()).spawn(move || {
            #[allow(clippy::cast_precision_loss)]
            crate::api::fair_play_check("pawn", &detail, count as f64, hooks_config::running_under_wine());
        });
        if let Err(e) = spawned {
            warn!("Character switches: couldn't report ({e})");
        }
    }
}

/// The gamebox's stand-ins for a cheat (a test build only): `godmode` sets m_bGodMode on the own
/// character, `health999` its health.
#[cfg(feature = "test-commands")]
fn test_command(places: &[Place], own: &pawns::Sample) {
    let Some(file) = crate::LOG_PATH.get().and_then(|p| p.parent()).map(|d| d.join("gamebox-command")) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&file) else { return };
    match text.trim() {
        "godmode" => {
            let _ = std::fs::remove_file(&file);
            if let Some(p) = places.iter().find(|p| p.name == "m_bGodMode") {
                let at = own.object + usize::from(p.offset);
                if let Some(w) = read_u32(at) {
                    // SAFETY: the own character's bool word, read just before.
                    unsafe { (at as *mut u32).write_unaligned(w | p.mask) };
                    info!("Character switches: test: m_bGodMode set on the own character");
                }
            }
        }
        "health999" => {
            let _ = std::fs::remove_file(&file);
            if let Some(at) = pawns::health_at(own.object) {
                // SAFETY: the own character's Health, an int the sampler reads every 100 ms.
                unsafe { (at as *mut i32).write_unaligned(999) };
                info!("Character switches: test: health set to 999 on the own character");
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_health_system_over_its_limits_is_said() {
        assert_eq!(health_says(90.0, 90.0, Some(90)), None, "full health");
        assert_eq!(health_says(40.0, 90.0, Some(90)), None);
        assert_eq!(health_says(999.0, 90.0, Some(90)).as_deref(), Some("health system 999 of max 90"));
        assert_eq!(health_says(170.0, 999.0, Some(170)).as_deref(), Some("health system max 999 over the character's 170"));
        assert_eq!(health_says(170.0, 999.0, None), None, "no starting health to set it against");
    }

    #[test]
    fn switches_are_placed_by_declaration_order_only_when_the_count_is_the_scripts() {
        let bools: Vec<(u16, u32)> = (0..26u32).map(|i| (884, 1 << i)).collect();
        let p = placed(&bools, 26, SWITCHES[0].2, true);
        assert_eq!(p.len(), 4);
        assert_eq!((p[0].name, p[0].offset, p[0].mask), ("m_bInvisible", 884, 1 << 2));
        assert_eq!((p[3].name, p[3].mask), ("m_bBlockDamage", 1 << 25));
        assert!(placed(&bools[..25], 26, SWITCHES[0].2, true).is_empty(), "one missing: nothing placed");
        // EPawn: three words, its 45th bool in the second (bit 13).
        let e: Vec<(u16, u32)> = (0..85u32).map(|i| (1248 + 4 * u16::try_from(i / 32).unwrap(), 1 << (i % 32))).collect();
        let p = placed(&e, 85, SWITCHES[1].2, true);
        assert_eq!(p.iter().find(|p| p.name == "bDamageInsensitive").map(|p| (p.offset, p.mask)), Some((1252, 1 << 13)));
    }
}
