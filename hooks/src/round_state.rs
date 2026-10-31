//! Whether this game's match is under way (0.4.8 plan M2), read from the game's own match
//! state, and said to the server (`api::match_state`), which leaves a match its host says is
//! under way out of searches, the friend list's Join and JoinNow.
//!
//! The game's session lock isn't it (it's set from matchmaking on, pre-round lobby included;
//! blacklist-re notes session-locked.md, and the servers' data agreed). The round is in the
//! match manager's script state: `EAdversarialMatchManager.m_eRoundState` (Spies vs Mercs:
//! PreMatch, PreRound, Started, PreEnded, Ended, SuddenDeath, PostMatch) and
//! `ECooperativeMatchManager.m_missionState` (co-op: None, InGame, Results). Script classes
//! have no fixed native offsets, so they're found through the engine's own tables, as its
//! `StaticFindObject` (DX11 `FUN_004d7480`) does:
//!
//! - names: the name table (`dataversion::NAMES`), each entry's text at `+8`;
//! - objects: a hash of 0x10000 buckets at [`OBJECT_HASH`], bucket
//!   `((name >> 19) * 0x141 + name) & 0xffff` for a packed name (index, number << 19), each
//!   object's next at `+0x08`, outer at `+0x18`, flags at `+0x1c`, name at `+0x20`, class at
//!   `+0x24`; a class's super at `+0x28` (`execIsA`, `FUN_0051be40`);
//! - a property's byte offset in its object: the u16 at `+0x3a` (`UByteProperty`'s reads
//!   and its link, which aligns it).
//!
//! With the state go the round's settings as the manager has them ([`SETTINGS`]: respawn
//! times, the time limit, score and respawn limits; F4), found the same way on the manager's
//! class or the classes it's made from: a host changing them during a round is shown to the
//! admins. Floats are `m_f…`, the rest 32-bit integers.
//!
//! The live match manager is an instance of one of the concrete classes below, named after its
//! class with a number (suffix + 1) that rises with each match. Only the DirectX 11 build
//! (Steam's and Ubisoft's are the same code), checked by the hash lookup's bytes before any
//! address is used; elsewhere nothing is read, and the server's locked refusals stay the only
//! word on started matches.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use tracing::info;

use crate::dataversion::read;
use crate::dataversion::read_string;
use crate::dataversion::read_u32;

/// The object hash's buckets, and the code that reads them, with its bytes (DX11).
const OBJECT_HASH: usize = 0x0333_cde8;
const HASH_LOOKUP: usize = 0x004d_74ad;
const HASH_LOOKUP_BYTES: [u8; 23] = [
    0xc1, 0xe8, 0x13, 0x69, 0xc0, 0x41, 0x01, 0x00, 0x00, 0x03, 0xc2, 0x25, 0xff, 0xff, 0x00, 0x00, 0x8b, 0x04, 0x85, 0xe8, 0xcd, 0x33, 0x03,
];
/// The name table (as dataversion.rs reads it): its entries, and their number.
const NAMES: usize = 0x0333_cd60;
const NAMES_COUNT: usize = 0x0333_cd64;
const MAX_NAMES: usize = 400_000;

/// How often the state is read, and said again while under way.
const READ_EVERY: Duration = Duration::from_secs(2);
const SAY_AGAIN: Duration = Duration::from_secs(30);
/// Instance numbers searched past the last found.
const NUMBERS: u32 = 64;
/// How long before the classes are looked up again, while they aren't loaded yet (a look-up
/// reads the whole name table).
const LOOK_AGAIN: Duration = Duration::from_secs(30);

/// The concrete match managers: Spies vs Mercs modes, then co-op ones.
const ADVERSARIAL: [&str; 7] = [
    "ESpyVSMercMatchManager",
    "EClassicSvMMatchManager",
    "ECTIMatchManager",
    "EEspionageMatchManager",
    "EFrontlineMatchManager",
    "ETeamDMMatchManager",
    "EAdversarialMatchManager",
];
const COOPERATIVE: [&str; 5] = [
    "EHunterMatchManager",
    "EGhostMatchManager",
    "EExtractionMatchManager",
    "ECombinedOpsMatchManager",
    "ECooperativeMatchManager",
];

const ROUND_STATES: [&str; 7] = [
    "RoundState_PreMatch",
    "RoundState_PreRound",
    "RoundState_Started",
    "RoundState_PreEnded",
    "RoundState_Ended",
    "RoundState_SuddenDeath",
    "RoundState_PostMatch",
];
/// The round's settings read from the match manager (F4), with the class each is on
/// (echelon.u's exports): EAdversarialMatchManager's for every Spies vs Mercs mode,
/// ESpyVSMercMatchManager's team respawn times, ECTIMatchManager's (extraction).
const SETTINGS: [&str; 9] = [
    "m_fGenericRespawnTime",
    "m_fAutoRespawnTime",
    "m_fCurrentRoundTimeLimit",
    "m_iScoreLimit",
    "m_iSuddenDeathScoreLimit",
    "m_fRespawnTimeSpyTeam",
    "m_fRespawnTimeMercTeam",
    "m_fRespawnTimeDefendingTeam",
    "m_fRespawnTimeCourierTeam",
];

const MISSION_STATES: [&str; 3] = ["CoopMissionState_None", "CoopMissionState_InGame", "CoopMissionState_Results"];

/// What the game's match state says: its name, and whether the match is under way.
fn meaning(adversarial: bool, value: u8) -> (String, bool) {
    if adversarial {
        let name = ROUND_STATES.get(usize::from(value)).map_or_else(|| format!("RoundState_{value}"), |s| (*s).to_string());
        // A round running (or about to end, or in sudden death): the host's game turns joiners away.
        (name, matches!(value, 2 | 3 | 5))
    } else {
        let name = MISSION_STATES
            .get(usize::from(value))
            .map_or_else(|| format!("CoopMissionState_{value}"), |s| (*s).to_string());
        (name, value == 1)
    }
}

/// The bucket of a packed name.
fn bucket(packed: u32) -> u32 {
    ((packed >> 19).wrapping_mul(0x141).wrapping_add(packed)) & 0xffff
}

fn pack(index: u32, number: u32) -> u32 {
    index | (number << 19)
}

/// An object as the engine's own lookup takes it: not pending kill nor unreachable.
fn alive(object: usize) -> bool {
    let flags = read_u32(object + 0x1c).unwrap_or(u32::MAX);
    let internal = read_u32(object + 4).unwrap_or(u32::MAX);
    flags & 0x0080_0400 == 0 && (internal >> 0x14) & 4 == 0
}

/// The objects named `packed`, in its bucket.
fn named(packed: u32) -> Vec<usize> {
    let mut found = Vec::new();
    let mut object = read_u32(OBJECT_HASH + bucket(packed) as usize * 4).unwrap_or(0) as usize;
    for _ in 0..4096 {
        if object == 0 {
            break;
        }
        if read_u32(object + 0x20) == Some(packed) {
            found.push(object);
        }
        object = read_u32(object + 8).unwrap_or(0) as usize;
    }
    found
}

/// What's looked up once: the names' indices, the classes, and the two properties' offsets.
struct Found {
    names: HashMap<&'static str, u32>,
    classes: HashMap<&'static str, usize>,
    round_offset: Option<u16>,
    mission_offset: Option<u16>,
    /// Each setting's properties (objects named so, of whichever class).
    settings: Vec<(&'static str, Vec<usize>)>,
}

/// The name indices of `wanted`, from the name table (once: it holds tens of thousands).
fn name_indices(wanted: &[&'static str]) -> HashMap<&'static str, u32> {
    let mut indices = HashMap::new();
    let (Some(table), Some(count)) = (read_u32(NAMES), read_u32(NAMES_COUNT)) else {
        return indices;
    };
    for i in 0..(count as usize).min(MAX_NAMES) {
        let Some(entry) = read_u32(table as usize + i * 4).filter(|e| *e != 0) else { continue };
        let Some(text) = read_string(entry as usize + 8, 64) else { continue };
        if let Some(w) = wanted.iter().find(|w| **w == text) {
            indices.insert(*w, i as u32);
            if indices.len() == wanted.len() {
                break;
            }
        }
    }
    indices
}

fn look_up() -> Option<Found> {
    let mut wanted: Vec<&'static str> = ADVERSARIAL.iter().chain(COOPERATIVE.iter()).copied().collect();
    wanted.extend(["Class", "m_eRoundState", "m_missionState"]);
    wanted.extend(SETTINGS);
    let names = name_indices(&wanted);
    let class_name = *names.get("Class")?;
    // A class: the object with the class's name whose own class is named "Class".
    let mut classes = HashMap::new();
    for name in ADVERSARIAL.iter().chain(COOPERATIVE.iter()) {
        let Some(index) = names.get(name) else { continue };
        let class = named(pack(*index, 0))
            .into_iter()
            .find(|o| read_u32(o + 0x24).is_some_and(|c| read_u32(c as usize + 0x20) == Some(pack(class_name, 0))));
        if let Some(c) = class {
            classes.insert(*name, c);
        }
    }
    // A property: the one with its name whose outer is its class.
    let offset = |property: &str, class: &str| -> Option<u16> {
        let owner = *classes.get(class)?;
        let object = named(pack(*names.get(property)?, 0)).into_iter().find(|o| read_u32(o + 0x18) == Some(owner as u32))?;
        read::<u16>(object + 0x3a)
    };
    let settings = SETTINGS
        .iter()
        .filter_map(|s| Some((*s, named(pack(*names.get(s)?, 0)))))
        .filter(|(_, props)| !props.is_empty())
        .collect();
    Some(Found {
        round_offset: offset("m_eRoundState", "EAdversarialMatchManager"),
        mission_offset: offset("m_missionState", "ECooperativeMatchManager"),
        settings,
        names,
        classes,
    })
}

/// A class and those it's made from (its supers), nearest first.
fn chain(class: usize) -> Vec<u32> {
    let mut out = Vec::new();
    let mut c = class as u32;
    while c != 0 && out.len() < 32 {
        out.push(c);
        c = read_u32(c as usize + 0x28).unwrap_or(0);
    }
    out
}

/// The round's settings on the match manager `object`: each of [`SETTINGS`] its class (or one
/// it's made from) has, with its value.
fn settings_of(found: &Found, object: usize) -> std::collections::BTreeMap<String, f32> {
    let classes = read_u32(object + 0x24).map(|c| chain(c as usize)).unwrap_or_default();
    let mut out = std::collections::BTreeMap::new();
    for (name, props) in &found.settings {
        let Some(prop) = props.iter().find(|p| read_u32(*p + 0x18).is_some_and(|outer| classes.contains(&outer))) else {
            continue;
        };
        let Some(offset) = read::<u16>(prop + 0x3a) else { continue };
        let at = object + usize::from(offset);
        #[allow(clippy::cast_precision_loss)]
        let value = if name.starts_with("m_f") { read::<f32>(at) } else { read::<i32>(at).map(|v| v as f32) };
        if let Some(v) = value.filter(|v| v.is_finite()) {
            out.insert((*name).to_string(), v);
        }
    }
    out
}

/// The live match manager: a living instance of one of the concrete classes, searched from the
/// number found last (each match spawns one with the next). Whether it's adversarial.
fn manager(found: &Found, from: &mut HashMap<&'static str, u32>) -> Option<(usize, bool)> {
    for (name, adversarial) in ADVERSARIAL.iter().map(|n| (n, true)).chain(COOPERATIVE.iter().map(|n| (n, false))) {
        let (Some(index), Some(class)) = (found.names.get(name), found.classes.get(name)) else {
            continue;
        };
        let start = from.get(name).copied().unwrap_or(1);
        let mut last = None;
        for number in start..start + NUMBERS {
            if let Some(o) = named(pack(*index, number)).into_iter().find(|o| read_u32(o + 0x24) == Some(*class as u32) && alive(*o)) {
                last = Some((o, number));
            } else if last.is_some() {
                break;
            }
        }
        if let Some((object, number)) = last {
            from.insert(name, number);
            return Some((object, adversarial));
        }
    }
    None
}

/// Reads the match state every [`READ_EVERY`] and tells the server of each change (and every
/// [`SAY_AGAIN`] while under way). DX11 only, once the hash lookup's bytes are where expected.
pub fn start() {
    let bytes: Option<Vec<u8>> = (0..HASH_LOOKUP_BYTES.len()).map(|i| read::<u8>(HASH_LOOKUP + i)).collect();
    if bytes.as_deref() != Some(&HASH_LOOKUP_BYTES[..]) {
        info!("Match state: not this build's (DirectX 11 only); the server learns of started matches from refused joins");
        return;
    }
    let spawned = std::thread::Builder::new().name("fe-round-state".into()).spawn(|| {
        let mut found: Option<Found> = None;
        let mut looked: Option<Instant> = None;
        let mut numbers = HashMap::new();
        let mut said: Option<(String, bool, Instant)> = None;
        let mut said_settings: Option<std::collections::BTreeMap<String, f32>> = None;
        loop {
            std::thread::sleep(READ_EVERY);
            // Looked up once the script classes are loaded (the first match manager).
            if found.as_ref().is_none_or(|f| f.classes.is_empty()) && looked.is_none_or(|t| t.elapsed() >= LOOK_AGAIN) {
                looked = Some(Instant::now());
                found = look_up();
                if let Some(f) = &found {
                    if !f.classes.is_empty() {
                        info!(
                            "Match state: {} match manager classes, round state at {:?}, mission state at {:?}",
                            f.classes.len(),
                            f.round_offset,
                            f.mission_offset
                        );
                    }
                }
            }
            let Some(f) = &found else { continue };
            let live = manager(f, &mut numbers);
            let now = live.and_then(|(object, adversarial)| {
                let offset = if adversarial { f.round_offset } else { f.mission_offset }?;
                Some(meaning(adversarial, read::<u8>(object + usize::from(offset))?))
            });
            let settings = live.map(|(object, _)| settings_of(f, object)).unwrap_or_default();
            let (state, under_way) = now.unwrap_or_else(|| ("none".to_string(), false));
            // The round's settings changing while it runs is said at once too (F4).
            let settings_changed = under_way && said_settings.as_ref().is_some_and(|s| *s != settings);
            if settings != said_settings.clone().unwrap_or_default() {
                info!("Match settings: {settings:?}");
            }
            let changed = said.as_ref().is_none_or(|(s, _, _)| *s != state) || settings_changed;
            let again = under_way && said.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= SAY_AGAIN);
            if changed || again {
                if changed {
                    info!("Match state: {state}{}", if under_way { " (under way)" } else { "" });
                }
                // Nothing to say before the first match, nor of no match after a joinable one.
                if under_way || said.as_ref().is_some_and(|(_, u, _)| *u) || (changed && said.is_some()) {
                    crate::api::match_state(under_way, &state, &settings);
                }
                said = Some((state, under_way, Instant::now()));
            }
            said_settings = Some(settings);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("Match state: couldn't start its thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_hash_and_states_read_as_the_game_has_them() {
        assert_eq!(bucket(pack(0x1234, 0)), 0x1234);
        assert_eq!(bucket(pack(0x1234, 2)), (2 * 0x141 + 0x1234 + (2 << 19)) & 0xffff);
        assert_eq!(meaning(true, 2), ("RoundState_Started".into(), true));
        assert_eq!(meaning(true, 1), ("RoundState_PreRound".into(), false));
        assert_eq!(meaning(true, 5), ("RoundState_SuddenDeath".into(), true));
        assert_eq!(meaning(false, 1), ("CoopMissionState_InGame".into(), true));
        assert_eq!(meaning(false, 9), ("CoopMissionState_9".into(), false));
    }
}
