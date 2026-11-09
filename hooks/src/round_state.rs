//! Whether this game's match is under way (0.5.0 plan M2), read from the game's own match
//! state, and said to the server (`api::match_state`), which leaves a match its host says is
//! under way out of searches, the friend list's Join and JoinNow.
//!
//! The game's session lock isn't it (it's set from matchmaking on, pre-round lobby included;
//! blacklist-re notes session-locked.md, and the servers' data agreed). The round is in the
//! match manager's script state: `EAdversarialMatchManager.m_eRoundState` (Spies vs Mercs:
//! PreMatch, PreRound, Started, PreEnded, Ended, SuddenDeath, PostMatch) and
//! `ECooperativeMatchManager.m_missionState` (co-op: None, InGame, Results). Script classes
//! have no fixed native offsets, so they're found through the engine's own tables, as its
//! `StaticFindObject` (DX11 `FUN_004d7480`, DX9 `FUN_007846a0`) does:
//!
//! - names: the name table (`hooks_addresses::engine_tables`), each entry's text at `+8`;
//! - objects: a hash of 0x10000 buckets (`engine_tables`), bucket
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
//! class with a number (suffix + 1) that rises with each match. Both builds (DirectX 9 and 11;
//! Steam's and Ubisoft's are the same code): the tables are found by signature
//! (`hooks_addresses::engine_tables`) and the hash lookup's bytes checked before any of them is
//! read. A build where they aren't found reads nothing, and the server's locked refusals stay the
//! only word on started matches.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use hooks_addresses::engine_tables;
use hooks_addresses::engine_tables::Tables;
use tracing::info;

use crate::dataversion::read;
use crate::dataversion::read_string;
use crate::dataversion::read_u32;

const MAX_NAMES: usize = 400_000;

/// The engine's tables in this game, found once (by signature, in either build), and only if
/// the hash lookup's bytes are where they were found as the game runs.
pub(crate) fn tables() -> Option<&'static Tables> {
    static TABLES: OnceLock<Option<Tables>> = OnceLock::new();
    TABLES
        .get_or_init(|| {
            // SAFETY: the game's own sections, as loaded.
            let image = unsafe { crate::game_image() }?;
            let tables = match engine_tables::locate(&image) {
                Ok(t) => t,
                Err(why) => {
                    info!("Engine tables: not found ({why})");
                    return None;
                }
            };
            let want = engine_tables::hash_lookup_bytes(&tables);
            let bytes: Option<Vec<u8>> = (0..want.len()).map(|i| read::<u8>(tables.hash_lookup + i)).collect();
            if bytes.as_deref() != Some(&want[..]) {
                info!("Engine tables: the hash lookup at {:#010x} isn't as found", tables.hash_lookup);
                return None;
            }
            info!(
                "Engine tables: objects {:#010x}, names {:#010x} (lookup {:#010x})",
                tables.object_hash, tables.names, tables.hash_lookup
            );
            Some(tables)
        })
        .as_ref()
}

/// How often the state is read, and said again while under way.
const READ_EVERY: Duration = Duration::from_secs(2);
const SAY_AGAIN: Duration = Duration::from_secs(30);
/// How soon it's said again while this game's number is new or a character has no name.
const NAME_AGAIN: Duration = Duration::from_secs(5);
/// At most this many sooner a round (a name that never comes, e.g. a number two players claim).
const NAMING_TRIES: u32 = 12;
/// Instance numbers searched past the last found.
const NUMBERS: u32 = 64;
/// How long before the classes are looked up again, while they aren't loaded yet (a look-up
/// reads the whole name table).
const LOOK_AGAIN: Duration = Duration::from_secs(30);
/// Look-ups again once the classes are found, while a state's place is still missing.
const LOOKS_AGAIN: u32 = 40;

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
pub(crate) fn bucket(packed: u32) -> u32 {
    ((packed >> 19).wrapping_mul(0x141).wrapping_add(packed)) & 0xffff
}

pub(crate) fn pack(index: u32, number: u32) -> u32 {
    index | (number << 19)
}

/// An object as the engine's own lookup takes it: not pending kill nor unreachable.
pub(crate) fn alive(object: usize) -> bool {
    let flags = read_u32(object + 0x1c).unwrap_or(u32::MAX);
    let internal = read_u32(object + 4).unwrap_or(u32::MAX);
    flags & 0x0080_0400 == 0 && (internal >> 0x14) & 4 == 0
}

/// The objects named `packed`, in its bucket.
pub(crate) fn named(packed: u32) -> Vec<usize> {
    let mut found = Vec::new();
    let Some(tables) = tables() else { return found };
    let mut object = read_u32(tables.object_hash + bucket(packed) as usize * 4).unwrap_or(0) as usize;
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

/// A packed name's text (`Name` or `Name_N` for number N + 1), from the name table.
pub(crate) fn name_text(packed: u32) -> Option<String> {
    let (index, number) = (packed & 0x7_ffff, packed >> 19);
    let tables = tables()?;
    let (table, count) = (read_u32(tables.names)?, read_u32(tables.names_count)?);
    if index >= count.min(MAX_NAMES as u32) {
        return None;
    }
    let entry = read_u32(table as usize + index as usize * 4).filter(|e| *e != 0)?;
    let text = read_string(entry as usize + 8, 64)?;
    Some(if number == 0 { text } else { format!("{text}_{}", number - 1) })
}

/// The map loaded: the package holding a living level info (`LevelInfo0` and on, of
/// `LevelInfo` or a class made from it), its outermost object's name. `names` holds "Class"
/// and "LevelInfo".
pub(crate) fn map_name(names: &HashMap<&'static str, u32>) -> Option<String> {
    let index = *names.get("LevelInfo")?;
    class_named(names, "LevelInfo")?;
    for number in 1..8 {
        for o in named(pack(index, number)) {
            if !alive(o) || !read_u32(o + 0x24).is_some_and(|c| made_from(c as usize, pack(index, 0))) {
                continue;
            }
            let mut outermost = o;
            for _ in 0..8 {
                match read_u32(outermost + 0x18) {
                    Some(outer) if outer != 0 => outermost = outer as usize,
                    _ => break,
                }
            }
            if outermost != o {
                return name_text(read_u32(outermost + 0x20)?);
            }
        }
    }
    None
}

/// The map's name as the server has the room's (the match state's answer): the level's own
/// name isn't readable in the shipped game, so recordings had none.
static SERVER_MAP: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

pub(crate) fn set_server_map(map: &str) {
    let map: String = map.chars().filter(|c| !c.is_control()).take(64).collect();
    *SERVER_MAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = map;
}

/// The server's name for the room's map (empty: none yet).
pub(crate) fn server_map() -> String {
    SERVER_MAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// The map: the level's own name, else the server's name for the room's.
pub(crate) fn map_or_server(names: &HashMap<&'static str, u32>) -> Option<String> {
    map_name(names).or_else(|| Some(SERVER_MAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()).filter(|m| !m.is_empty()))
}

/// The match state as last read: its name, whether it's under way, and the match manager's
/// class (the mode), for the recorder (recording.rs).
pub(crate) fn current() -> (String, bool, String) {
    CURRENT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

static CURRENT: std::sync::Mutex<(String, bool, String)> = std::sync::Mutex::new((String::new(), false, String::new()));

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
pub(crate) fn name_indices(wanted: &[&'static str]) -> HashMap<&'static str, u32> {
    let mut indices = HashMap::new();
    let Some(tables) = tables() else { return indices };
    let (Some(table), Some(count)) = (read_u32(tables.names), read_u32(tables.names_count)) else {
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
    wanted.extend(["Class", "ByteProperty", "m_eRoundState", "m_missionState"]);
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
    // The round state by name, else by its place: in the Ubisoft Connect build (and likely
    // every build: the script packages are shared) the name doesn't resolve, but
    // EAdversarialMatchManager has an unnamed byte property at 970 that runs through the
    // RoundState values (0 PreMatch, 1 PreRound, 2 Started at the round's start; the gamebox,
    // 2026-11-05). Taken only when that property is there, so another layout reads nothing.
    let named_round = offset("m_eRoundState", "EAdversarialMatchManager");
    let named_mission = offset("m_missionState", "ECooperativeMatchManager");
    let owners = |name: &str| classes_named(&names, name);
    let places = if named_round.is_none() || named_mission.is_none() {
        let mut all = owners("EAdversarialMatchManager");
        all.extend(owners("ECooperativeMatchManager"));
        Places::of(&all)
    } else {
        Places::none()
    };
    let round_offset = named_round.or_else(|| {
        let at = places.at(&names, &owners("EAdversarialMatchManager"), "ByteProperty", ROUND_STATE_AT);
        if at.is_some() && !SAID_ROUND.swap(true, std::sync::atomic::Ordering::Relaxed) {
            info!("Match state: the round state found by its place ({ROUND_STATE_AT}), its name unresolved");
        }
        at
    });
    let mission_offset = named_mission.or_else(|| {
        let at = places.at(&names, &owners("ECooperativeMatchManager"), "ByteProperty", MISSION_STATE_AT);
        if at.is_some() && !SAID_MISSION.swap(true, std::sync::atomic::Ordering::Relaxed) {
            info!("Match state: the mission state found by its place ({MISSION_STATE_AT}), its name unresolved");
        }
        at
    });
    Some(Found {
        round_offset,
        mission_offset,
        settings,
        names,
        classes,
    })
}

/// Where EAdversarialMatchManager keeps its round state when its property's name doesn't
/// resolve (see `look_up`).
const ROUND_STATE_AT: u16 = 970;
/// Where ECooperativeMatchManager keeps its mission state (m_missionState, unresolved the same
/// way): from the script packages' layout (see [`Places`]).
const MISSION_STATE_AT: u16 = 940;
static SAID_ROUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SAID_MISSION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The properties of some classes as the engine has them, named or not: (owner class, offset,
/// the property's class's packed name). Many property names don't resolve in the shipped game
/// (m_eRoundState, m_missionState, m_MPNickname, GroundSpeed, CurrentWeapon...), but their
/// objects are there, with their owner and offset. One whole scan of the object hash.
pub(crate) struct Places(Vec<(u32, u16, u32)>);

impl Places {
    pub(crate) fn none() -> Self {
        Self(Vec::new())
    }

    pub(crate) fn of(owners: &[usize]) -> Self {
        let mut out = Vec::new();
        let Some(tables) = tables() else { return Self(out) };
        let owners: Vec<u32> = owners.iter().map(|o| *o as u32).collect();
        if owners.is_empty() {
            return Self(out);
        }
        for b in 0..0x10000usize {
            let mut object = read_u32(tables.object_hash + b * 4).unwrap_or(0) as usize;
            for _ in 0..4096 {
                if object == 0 {
                    break;
                }
                if let Some(owner) = read_u32(object + 0x18).filter(|o| owners.contains(o)) {
                    if let (Some(at), Some(kind)) = (read::<u16>(object + 0x3a), read_u32(object + 0x24).and_then(|c| read_u32(c as usize + 0x20))) {
                        out.push((owner, at, kind));
                    }
                }
                object = read_u32(object + 8).unwrap_or(0) as usize;
            }
        }
        Self(out)
    }

    /// `at`, when one of `owners` has a property of the kind named `kind` (e.g. "ByteProperty")
    /// there: the offsets worked out offline (the script packages' declared order and sizes,
    /// checked against ten the engine resolves by name: the round state's 970, the round
    /// settings on three match managers, Pawn.Health and Controller, Actor.Location), taken
    /// only when this game has such a property in that place.
    pub(crate) fn at(&self, names: &HashMap<&'static str, u32>, owners: &[usize], kind: &str, at: u16) -> Option<u16> {
        let kind = pack(*names.get(kind)?, 0);
        self.0.iter().any(|(o, a, k)| owners.contains(&(*o as usize)) && *a == at && *k == kind).then_some(at)
    }
}

/// Every object in the engine's object hash (a test build's diagnostics: slow, a whole scan).
#[cfg(feature = "test-commands")]
pub(crate) fn each_object(mut f: impl FnMut(usize)) {
    let Some(tables) = tables() else { return };
    for b in 0..0x10000usize {
        let mut object = read_u32(tables.object_hash + b * 4).unwrap_or(0) as usize;
        for _ in 0..4096 {
            if object == 0 {
                break;
            }
            f(object);
            object = read_u32(object + 8).unwrap_or(0) as usize;
        }
    }
}

/// Whether `object`'s class is named `packed`. Classes are told by name, not by the class
/// object: a class can be loaded again (each match's characters and match manager can come
/// from another class object of the same name than the one looked up first; the gamebox,
/// 2026-11-05: no character was ever found, so nothing was recorded).
pub(crate) fn of_class(object: usize, packed: u32) -> bool {
    read_u32(object + 0x24).is_some_and(|c| c != 0 && read_u32(c as usize + 0x20) == Some(packed))
}

/// Whether `class` is, or is made from, a class named `packed`.
pub(crate) fn made_from(class: usize, packed: u32) -> bool {
    chain(class).iter().any(|c| read_u32(*c as usize + 0x20) == Some(packed))
}

/// A class and those it's made from (its supers), nearest first.
pub(crate) fn chain(class: usize) -> Vec<u32> {
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

/// `m_bForceEndRound`'s bitfield and bit in an adversarial match manager (test command `end-round`).
#[cfg(feature = "test-commands")]
const FORCE_END_ROUND_AT: usize = 852;
#[cfg(feature = "test-commands")]
const FORCE_END_ROUND_BIT: u32 = 1 << 27;

/// The gamebox's stand-in for Cheat Engine (svm-settings; a test build only): the command file's
/// `setting <name> <value>` written into the live match manager, as a cheat would.
#[cfg(feature = "test-commands")]
fn test_setting(found: &Found, object: usize, adversarial: bool) {
    let Some(file) = crate::LOG_PATH.get().and_then(|p| p.parent()).map(|d| d.join("gamebox-command")) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&file) else { return };
    if text.trim() == "offsets" {
        let _ = std::fs::remove_file(&file);
        // The settings' offsets as the engine has them, by their owner class (route 2's check).
        for (name, props) in &found.settings {
            let at: Vec<String> = props
                .iter()
                .map(|p| {
                    let owner = read_u32(*p + 0x18).and_then(|o| read_u32(o as usize + 0x20)).and_then(name_text).unwrap_or_default();
                    format!("{owner} {}", read::<u16>(*p + 0x3a).unwrap_or(0))
                })
                .collect();
            info!("Match settings: test: offsets: {name} {at:?}");
        }
        crate::pawns::test_offsets();
        return;
    }
    if text.trim() == "end-round" && adversarial {
        let _ = std::fs::remove_file(&file);
        // m_bForceEndRound: bit 27 of the bitfield at 852 (EAdversarialMatchManager, the offline
        // layout), which the host's manager ends the round on: the gamebox can't play one out.
        let at = (object + FORCE_END_ROUND_AT) as *mut u32;
        // SAFETY: the live adversarial manager's own bitfield, read before it's written.
        let before = read_u32(at as usize);
        if let Some(bits) = before {
            unsafe { at.write_unaligned(bits | FORCE_END_ROUND_BIT) };
        }
        info!("Match settings: test: m_bForceEndRound set (bits {before:#x?})");
        return;
    }
    let mut words = text.split_whitespace();
    let (Some("setting"), Some(name), Some(value)) = (words.next(), words.next(), words.next()) else {
        return;
    };
    let _ = std::fs::remove_file(&file);
    let Ok(value) = value.parse::<f32>() else {
        tracing::warn!("Match settings: test: {value:?} isn't a number");
        return;
    };
    let classes = read_u32(object + 0x24).map(|c| chain(c as usize)).unwrap_or_default();
    let Some(offset) = found
        .settings
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, props)| props.iter().find(|p| read_u32(*p + 0x18).is_some_and(|outer| classes.contains(&outer))))
        .and_then(|prop| read::<u16>(prop + 0x3a))
    else {
        tracing::warn!("Match settings: test: no {name} on this match manager");
        return;
    };
    let at = (object + usize::from(offset)) as *mut u8;
    // SAFETY: the live manager's own property (its offset from the class), read just before.
    #[allow(clippy::cast_possible_truncation)]
    unsafe {
        if name.starts_with("m_f") {
            at.cast::<f32>().write_unaligned(value);
        } else {
            at.cast::<i32>().write_unaligned(value as i32);
        }
    }
    info!("Match settings: test: {name} set to {value}");
}

/// The live match manager: a living instance of one of the concrete classes, searched from the
/// number found last (each match spawns one with the next). Whether it's adversarial.
fn manager(found: &Found, from: &mut HashMap<&'static str, u32>) -> Option<(usize, bool)> {
    for (name, adversarial) in ADVERSARIAL.iter().map(|n| (n, true)).chain(COOPERATIVE.iter().map(|n| (n, false))) {
        let Some(index) = found.names.get(name) else { continue };
        let start = from.get(name).copied().unwrap_or(1);
        let mut last = None;
        for number in start..start + NUMBERS {
            if let Some(o) = named(pack(*index, number)).into_iter().find(|o| of_class(*o, pack(*index, 0)) && alive(*o)) {
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
/// [`SAY_AGAIN`] while under way), once the engine's tables are found ([`tables`]).
/// Whether the engine's tables were found in this game, in either build.
pub(crate) fn tables_here() -> bool {
    tables().is_some()
}

/// A class named `name` (its object, whose own class is named "Class"), with the name table's
/// indices of "Class" and of `name`.
pub(crate) fn class_named(names: &HashMap<&'static str, u32>, name: &str) -> Option<usize> {
    let class_name = *names.get("Class")?;
    named(pack(*names.get(name)?, 0))
        .into_iter()
        .find(|o| read_u32(o + 0x24).is_some_and(|c| read_u32(c as usize + 0x20) == Some(pack(class_name, 0))))
}

/// Every class named `name` (a name can stand for more than one class object: a match's
/// characters are made from a second EMPSpy class).
pub(crate) fn classes_named(names: &HashMap<&'static str, u32>, name: &str) -> Vec<usize> {
    let (Some(class_name), Some(index)) = (names.get("Class"), names.get(name)) else {
        return Vec::new();
    };
    named(pack(*index, 0))
        .into_iter()
        .filter(|o| read_u32(o + 0x24).is_some_and(|c| read_u32(c as usize + 0x20) == Some(pack(*class_name, 0))))
        .collect()
}

/// The byte offset of property `property` in objects of class `class` (or one it's made from).
pub(crate) fn offset_in(names: &HashMap<&'static str, u32>, property: &str, class: usize) -> Option<u16> {
    let classes = chain(class);
    let object = named(pack(*names.get(property)?, 0))
        .into_iter()
        .find(|o| read_u32(o + 0x18).is_some_and(|outer| classes.contains(&outer)))?;
    read::<u16>(object + 0x3a)
}

pub fn start() {
    if !tables_here() {
        info!("Match state: the engine's tables aren't found in this game; the server learns of started matches from refused joins");
        return;
    }
    let spawned = std::thread::Builder::new().name("fe-round-state".into()).spawn(|| {
        let mut found: Option<Found> = None;
        let mut looked: Option<Instant> = None;
        // Looked up again while a state's place is missing (co-op's classes load with a co-op
        // lobby, not at the start), a limited number of times: each is a whole scan.
        let mut again = 0;
        let mut numbers = HashMap::new();
        let mut said: Option<(String, bool, Instant)> = None;
        let mut said_settings: Option<std::collections::BTreeMap<String, f32>> = None;
        let mut said_peer = 0;
        let mut naming_tries = 0;
        loop {
            std::thread::sleep(READ_EVERY);
            // Looked up once the script classes are loaded (the first match manager).
            let missing = |f: &Found| f.classes.is_empty() || ((f.round_offset.is_none() || f.mission_offset.is_none()) && again < LOOKS_AGAIN);
            if found.as_ref().is_none_or(missing) && looked.is_none_or(|t| t.elapsed() >= LOOK_AGAIN) {
                looked = Some(Instant::now());
                if found.as_ref().is_some_and(|f| !f.classes.is_empty()) {
                    again += 1;
                }
                let before = found.as_ref().map(|f| (f.round_offset, f.mission_offset));
                found = look_up().or(found);
                if before.is_some() && before == found.as_ref().map(|f| (f.round_offset, f.mission_offset)) {
                    continue;
                }
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
            #[cfg(feature = "test-commands")]
            if let Some((object, adversarial)) = live {
                test_setting(f, object, adversarial);
            }
            let settings = live.map(|(object, _)| settings_of(f, object)).unwrap_or_default();
            let (state, under_way) = now.unwrap_or_else(|| ("none".to_string(), false));
            let mode = live
                .and_then(|(object, _)| name_text(read_u32(read_u32(object + 0x24)? as usize + 0x20)?))
                .unwrap_or_default();
            *CURRENT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = (state.clone(), under_way, mode);
            // The round's settings changing while it runs is said at once too (F4).
            let settings_changed = under_way && said_settings.as_ref().is_some_and(|s| *s != settings);
            if settings != said_settings.clone().unwrap_or_default() {
                info!("Match settings: {settings:?}");
            }
            let changed = said.as_ref().is_none_or(|(s, _, _)| *s != state) || settings_changed;
            // Sooner while players aren't all named (the server names them by the numbers each
            // game says): at once when this game learns its own number, then every NAME_AGAIN
            // while a character it sees has no name, rather than every SAY_AGAIN.
            let own = crate::pawns::own_peer();
            if !under_way {
                naming_tries = 0;
            }
            let naming = under_way
                && naming_tries < NAMING_TRIES
                && (own != said_peer || crate::pawns::unnamed_seen())
                && said.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= NAME_AGAIN);
            if naming {
                naming_tries += 1;
            }
            let again = naming || (under_way && said.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= SAY_AGAIN));
            if changed || again {
                if changed {
                    info!("Match state: {state}{}", if under_way { " (under way)" } else { "" });
                }
                // Nothing to say before the first match, nor of no match after a joinable one.
                if under_way || said.as_ref().is_some_and(|(_, u, _)| *u) || (changed && said.is_some()) {
                    crate::api::match_state(under_way, &state, &settings);
                }
                said = Some((state, under_way, Instant::now()));
                said_peer = own;
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
