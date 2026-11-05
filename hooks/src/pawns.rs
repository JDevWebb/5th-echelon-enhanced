//! The player characters as the game has them, read through the engine's own tables (as
//! round_state.rs finds the match manager), sampled every [`SAMPLE`] for witness reports
//! (witness.rs, F9) and match recordings (recording.rs). What's read, by class (engine.u and
//! echelon.u):
//!
//! - the characters: living instances of the player pawn classes ([`PAWNS`]), named after
//!   their class with a number;
//! - each one's `Actor.Location` and `Actor.Rotation` (a rotator: pitch, yaw, roll, 65536 a
//!   turn), `Pawn.GroundSpeed`, `Pawn.Health`, and `EPawn.CurrentWeapon`;
//! - its `Pawn.Controller`, when it's a `CharacterControl`: its player's name
//!   (`m_MPNickname`), team (`m_MPTeamId`), whether they hold fire (`m_Input_bFireWeaponHold`),
//!   and the last they killed and when (`m_LastKillVictim`, `m_fLastKillTime`). Whether a
//!   remote player's character has one is what the research is to learn: the counts are logged;
//! - a weapon's bullet history (`BasicWeapon.m_BulletHistory`, 32 `BulletStamp`s: `m_vStart`,
//!   `m_vEnd`, `m_fTimeStamp`), each shot from where to where.

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
/// Look-ups again once the classes are found, while names or shots are missing.
const LOOKS_AGAIN: u32 = 40;
pub const SAMPLE: Duration = Duration::from_millis(100);
/// Instance numbers looked through, past the highest found.
const NUMBERS: u32 = 48;
/// A weapon's bullet history's length (echelon.u: `m_BulletHistory[32]`).
const BULLETS: usize = 32;

/// What's looked up once the classes are loaded.
pub struct Tables {
    pub names: HashMap<&'static str, u32>,
    pawn_classes: Vec<(&'static str, usize)>,
    character_control: Vec<usize>,
    location: u16,
    rotation: Option<u16>,
    ground_speed: Option<u16>,
    health: Option<u16>,
    controller: Option<u16>,
    nickname: Option<u16>,
    team: Option<u16>,
    fire_hold: Option<u16>,
    last_victim: Option<u16>,
    last_kill_time: Option<u16>,
    weapon: Option<u16>,
    bullets: Option<Bullets>,
    /// Actor.NetInstanceId (a QWord), by name or place.
    net_id: Option<u16>,
}

/// Where a weapon keeps its shots.
struct Bullets {
    history: u16,
    /// Within a stamp: start, end, time; and a stamp's size.
    start: u16,
    end: u16,
    time: u16,
    stride: usize,
}

impl Tables {
    pub fn has_classes(&self) -> bool {
        !self.pawn_classes.is_empty()
    }
}

/// Where the properties are when their names don't resolve (the shipped game: GroundSpeed,
/// CurrentWeapon and CharacterControl's m_MP* among them), in the order `look_up` wants them:
/// the owner class, the property's kind and its offset. Worked out offline from the script
/// packages (each class's properties in declared order and size from its super's end: the
/// RE folder's lead_layout.py), checked against ten offsets the engine resolves by name
/// (m_eRoundState 970, the round settings on three match managers, Pawn.Health 1120,
/// Pawn.Controller 856, Actor.Location 156); each taken only when this game has a property of
/// that kind in that place (rs::Places).
const PLACES: [(&str, &str, u16); 7] = [
    ("Pawn", "FloatProperty", 892),               // GroundSpeed
    ("EPawn", "ObjectProperty", 1208),            // CurrentWeapon
    ("CharacterControl", "StrProperty", 1760),    // m_MPNickname
    ("CharacterControl", "IntProperty", 1756),    // m_MPTeamId
    ("CharacterControl", "ByteProperty", 1371),   // m_Input_bFireWeaponHold
    ("CharacterControl", "ObjectProperty", 1348), // m_LastKillVictim
    ("CharacterControl", "FloatProperty", 1344),  // m_fLastKillTime
];
/// Actor.NetInstanceId (a QWord struct), as PLACES.
const NET_ID_AT: u16 = 708;
/// BasicWeapon.m_BulletHistory[32] (BulletStamp: m_vStart 0, m_vEnd 12, m_fTimeStamp 24; 28
/// bytes), as PLACES.
const BULLET_HISTORY_AT: u16 = 1952;

/// A field `field` of the struct `name` declared in `class`: its offset in the struct.
fn struct_field(names: &HashMap<&'static str, u32>, name: &str, class: usize, field: &str) -> Option<u16> {
    let strukt = rs::named(rs::pack(*names.get(name)?, 0)).into_iter().find(|o| read_u32(o + 0x18) == Some(class as u32))?;
    let prop = rs::named(rs::pack(*names.get(field)?, 0)).into_iter().find(|o| read_u32(o + 0x18) == Some(strukt as u32))?;
    read::<u16>(prop + 0x3a)
}

pub fn look_up() -> Option<Tables> {
    let mut wanted: Vec<&'static str> = PAWNS.to_vec();
    wanted.extend([
        "Class",
        "Actor",
        "Pawn",
        "EPawn",
        "CharacterControl",
        "BasicWeapon",
        "BulletStamp",
        "LevelInfo",
        "NetInstanceId",
        "Location",
        "Rotation",
        "GroundSpeed",
        "Health",
        "Controller",
        "CurrentWeapon",
        "m_MPNickname",
        "m_MPTeamId",
        "m_Input_bFireWeaponHold",
        "m_LastKillVictim",
        "m_fLastKillTime",
        "m_BulletHistory",
        "m_vStart",
        "m_vEnd",
        "m_fTimeStamp",
        "StrProperty",
        "IntProperty",
        "ByteProperty",
        "ObjectProperty",
        "FloatProperty",
        "StructProperty",
    ]);
    let names = rs::name_indices(&wanted);
    let pawn_classes: Vec<(&'static str, usize)> = PAWNS.iter().filter_map(|p| Some((*p, rs::class_named(&names, p)?))).collect();
    let actor = rs::class_named(&names, "Actor")?;
    let pawn = rs::class_named(&names, "Pawn")?;
    let e_pawn = rs::class_named(&names, "EPawn");
    // A name can stand for more than one class object (see is_of): each is tried.
    let controls = rs::classes_named(&names, "CharacterControl");
    let in_control = |p: &str| controls.iter().find_map(|c| rs::offset_in(&names, p, *c));
    let weapons = rs::classes_named(&names, "BasicWeapon");
    let bullets = weapons.iter().copied().find_map(|class| {
        let (start, end, time) = (
            struct_field(&names, "BulletStamp", class, "m_vStart")?,
            struct_field(&names, "BulletStamp", class, "m_vEnd")?,
            struct_field(&names, "BulletStamp", class, "m_fTimeStamp")?,
        );
        Some(Bullets {
            history: rs::offset_in(&names, "m_BulletHistory", class)?,
            stride: usize::from(start.max(end).max(time)) + if time > start.max(end) { 4 } else { 12 },
            start,
            end,
            time,
        })
    });
    // By name, else by place (PLACES): most of these names don't resolve in the shipped game.
    let named = [
        rs::offset_in(&names, "GroundSpeed", pawn),
        e_pawn.and_then(|c| rs::offset_in(&names, "CurrentWeapon", c)),
        in_control("m_MPNickname"),
        in_control("m_MPTeamId"),
        in_control("m_Input_bFireWeaponHold"),
        in_control("m_LastKillVictim"),
        in_control("m_fLastKillTime"),
    ];
    let named_net_id = rs::offset_in(&names, "NetInstanceId", actor);
    let places = if named.iter().any(Option::is_none) || bullets.is_none() || named_net_id.is_none() {
        let mut all = controls.clone();
        all.extend(rs::classes_named(&names, "Pawn"));
        all.extend(rs::classes_named(&names, "EPawn"));
        all.extend(&weapons);
        all.extend(rs::classes_named(&names, "Actor"));
        rs::Places::of(&all)
    } else {
        rs::Places::none()
    };
    let owners = |class: &str| rs::classes_named(&names, class);
    let placed: Vec<Option<u16>> = PLACES
        .iter()
        .zip(named)
        .map(|((class, kind, at), named)| named.or_else(|| places.at(&names, &owners(class), kind, *at)))
        .collect();
    let bullets = bullets.or_else(|| {
        Some(Bullets {
            history: places.at(&names, &weapons, "StructProperty", BULLET_HISTORY_AT)?,
            stride: 28,
            start: 0,
            end: 12,
            time: 24,
        })
    });
    Some(Tables {
        location: rs::offset_in(&names, "Location", actor)?,
        rotation: rs::offset_in(&names, "Rotation", actor),
        ground_speed: placed[0],
        health: rs::offset_in(&names, "Health", pawn),
        controller: rs::offset_in(&names, "Controller", pawn),
        weapon: placed[1],
        nickname: placed[2],
        team: placed[3],
        fire_hold: placed[4],
        last_victim: placed[5],
        last_kill_time: placed[6],
        net_id: named_net_id.or_else(|| places.at(&names, &owners("Actor"), "StructProperty", NET_ID_AT)),
        character_control: controls.clone(),
        pawn_classes,
        bullets,
        names,
    })
}

/// Living player characters: their object and class. Each of [`PAWNS`] by its name, whether or
/// not its class was loaded when the tables were looked up: a match's characters (EMPSpy,
/// EMPMerc, EMPFirstPersonMerc) are loaded with the match, after the single player ones the
/// look-up found, so they were never looked for (every game saw none: no recording began and
/// witness reports saw nobody; the gamebox, 2026-11-05).
fn characters(t: &Tables, from: &mut HashMap<&'static str, u32>) -> Vec<(usize, &'static str)> {
    let mut out = Vec::new();
    for name in &PAWNS {
        let Some(index) = t.names.get(name) else { continue };
        let start = from.get(name).copied().unwrap_or(1).saturating_sub(NUMBERS / 2).max(1);
        let mut highest = None;
        for number in start..start + NUMBERS * 2 {
            for o in rs::named(rs::pack(*index, number)) {
                if rs::of_class(o, rs::pack(*index, 0)) && rs::alive(o) {
                    out.push((o, *name));
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

/// An FString's text: (pointer to its 8-bit characters, count with the NUL).
fn fstring(at: usize) -> Option<String> {
    let (data, count) = (read_u32(at)? as usize, read::<i32>(at + 4)?);
    if data == 0 || !(1..=64).contains(&count) {
        return None;
    }
    // The game's strings are 8-bit (its name getter copies `count` bytes); read as 16-bit,
    // "gbox1" came out as "执硯1ല" (the gamebox, 2026-11-05).
    let bytes: Option<Vec<u8>> = (0..count as usize).map(|i| read::<u8>(data + i)).collect();
    let bytes = bytes?;
    let text = bytes.split(|b| *b == 0).next().unwrap_or_default();
    Some(String::from_utf8_lossy(text).into_owned()).filter(|s| !s.is_empty())
}

/// What a character's controller says, when it's a CharacterControl.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Control {
    pub name: Option<String>,
    pub team: Option<i32>,
    pub firing: bool,
    /// The character it last killed (its object), and the game's time for it.
    pub last_kill: Option<(usize, f32)>,
}

fn control_of(t: &Tables, character: usize) -> Option<Control> {
    let controller = read_u32(character + usize::from(t.controller?))? as usize;
    if controller == 0 || t.character_control.is_empty() || !rs::made_from(read_u32(controller + 0x24)? as usize, rs::pack(*t.names.get("CharacterControl")?, 0)) {
        return None;
    }
    let at = |o: Option<u16>| o.map(|o| controller + usize::from(o));
    Some(Control {
        name: at(t.nickname).and_then(fstring),
        team: at(t.team).and_then(read::<i32>),
        firing: at(t.fire_hold).and_then(read::<u8>).is_some_and(|b| b != 0),
        last_kill: at(t.last_victim)
            .and_then(read_u32)
            .filter(|v| *v != 0)
            .and_then(|v| Some((v as usize, at(t.last_kill_time).and_then(read::<f32>)?))),
    })
}

/// One character as read in one sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub object: usize,
    pub class: &'static str,
    pub at: [f32; 3],
    /// Pitch and yaw (65536 a turn).
    pub rotation: Option<(i32, i32)>,
    pub ground_speed: f32,
    pub health: Option<i32>,
    pub control: Option<Control>,
    /// Its weapon, when it's one that keeps a bullet history.
    pub weapon: Option<usize>,
    /// Its player's number in the game session (see [`peer_of`]).
    pub peer: Option<u8>,
}

/// A character's player's number in the game session: the top byte of the low word of its
/// network id (Actor.NetInstanceId, `0x0N000001`), the session peer index + 1 (host 1), the
/// same in every game (the gamebox: gb1 1, gb3 2, gb2 3, gb4 4 in all four games).
fn peer_of(t: &Tables, object: usize) -> Option<u8> {
    let low = read_u32(object + usize::from(t.net_id?))?;
    let peer = (low >> 24) as u8;
    (low & 0x00ff_ffff == 1 && (1..=32).contains(&peer)).then_some(peer)
}

/// This game's own player's number (0: not seen yet), for the match state it sends.
static OWN_PEER: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// The other players' names by their numbers, as the server answered (api::match_state).
static PEER_NAMES: std::sync::Mutex<Vec<(u8, String)>> = std::sync::Mutex::new(Vec::new());

/// Whether the last characters sampled had one with a number but no name (round_state then
/// asks the server again sooner).
static UNNAMED_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn unnamed_seen() -> bool {
    UNNAMED_SEEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// This game's own player's number in the session, 0 while not known.
pub fn own_peer() -> u32 {
    u32::from(OWN_PEER.load(std::sync::atomic::Ordering::Relaxed))
}

/// The room's players' numbers and names, as the server has them.
pub fn set_peer_names(peers: Vec<(u32, String)>) {
    let peers: Vec<(u8, String)> = peers.into_iter().filter_map(|(p, n)| Some((u8::try_from(p).ok()?, n))).take(32).collect();
    let mut known = PEER_NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if *known != peers {
        info!("Characters: the server names the players by number: {peers:?}");
        *known = peers;
    }
}

fn peer_name(peer: u8) -> Option<String> {
    PEER_NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(p, _)| *p == peer).map(|(_, n)| n.clone())
}

fn sample(t: &Tables, object: usize, class: &'static str) -> Option<Sample> {
    let loc = object + usize::from(t.location);
    let at = [read::<f32>(loc)?, read::<f32>(loc + 4)?, read::<f32>(loc + 8)?];
    let weapon = t
        .weapon
        .and_then(|w| read_u32(object + usize::from(w)))
        .filter(|w| *w != 0)
        .map(|w| w as usize)
        .filter(|w| {
            t.bullets.as_ref().is_some_and(|_| {
                t.names
                    .get("BasicWeapon")
                    .is_some_and(|i| read_u32(w + 0x24).is_some_and(|c| rs::made_from(c as usize, rs::pack(*i, 0))))
            })
        });
    // A game has only its own player's name: that character's number is its own; the others'
    // names come from the server by their numbers (peer_names on the server).
    let peer = peer_of(t, object);
    let mut control = control_of(t, object);
    if let Some(p) = peer {
        if control.as_ref().is_some_and(|c| c.name.is_some()) {
            if OWN_PEER.swap(p, std::sync::atomic::Ordering::Relaxed) != p {
                info!("Characters: this player's number in the session is {p}");
            }
        } else if let Some(name) = peer_name(p) {
            control.get_or_insert_with(Control::default).name = Some(name);
        }
    }
    Some(Sample {
        object,
        class,
        at,
        rotation: t
            .rotation
            .and_then(|r| Some((read::<i32>(object + usize::from(r))?, read::<i32>(object + usize::from(r) + 4)?))),
        ground_speed: t.ground_speed.and_then(|g| read::<f32>(object + usize::from(g))).unwrap_or(0.0),
        health: t.health.and_then(|h| read::<i32>(object + usize::from(h))),
        control,
        weapon,
        peer,
    })
}

/// A weapon's bullet history: each stamp's start, end and the game's time, oldest to newest
/// as far as their times say (unused ones, at time 0, left out).
pub fn shots(t: &Tables, weapon: usize) -> Vec<([f32; 3], [f32; 3], f32)> {
    let Some(b) = &t.bullets else { return Vec::new() };
    let base = weapon + usize::from(b.history);
    let f3 = |at: usize| Some([read::<f32>(at)?, read::<f32>(at + 4)?, read::<f32>(at + 8)?]);
    let mut out: Vec<_> = (0..BULLETS)
        .filter_map(|i| {
            let s = base + i * b.stride;
            let time = read::<f32>(s + usize::from(b.time))?;
            (time.is_finite() && time > 0.0).then_some((f3(s + usize::from(b.start))?, f3(s + usize::from(b.end))?, time))
        })
        .collect();
    out.sort_by(|a, b| a.2.total_cmp(&b.2));
    out
}

/// What uses the samples.
pub trait Watcher: Send {
    fn sample(&mut self, tables: &Tables, now: Instant, characters: &[Sample]);
}

/// The gamebox's `pawns` command (a test build only): every living character (an object whose
/// class is made from Pawn) in the whole object hash, against those the sampler found.
#[cfg(feature = "test-commands")]
fn test_pawns(t: &Tables, list: &[(usize, &'static str)]) {
    let Some(file) = crate::LOG_PATH.get().and_then(|p| p.parent()).map(|d| d.join("gamebox-command")) else {
        return;
    };
    if !std::fs::read_to_string(&file).is_ok_and(|t| t.trim() == "pawns") {
        return;
    }
    let _ = std::fs::remove_file(&file);
    let Some(pawn) = t.names.get("Pawn").map(|i| rs::pack(*i, 0)) else {
        info!("Characters: test: no Pawn name");
        return;
    };
    info!(
        "Characters: test: the sampler has {} ({:?})",
        list.len(),
        list.iter().map(|(o, c)| format!("{o:#x} {c}")).collect::<Vec<_>>()
    );
    info!(
        "Characters: test: names {:?}, classes {:?}",
        PAWNS.iter().map(|p| (*p, t.names.get(p).map(|i| format!("{:#x}", rs::pack(*i, 0))))).collect::<Vec<_>>(),
        t.pawn_classes
            .iter()
            .map(|(n, c)| format!("{n} {c:#x} named {:#x}", read_u32(c + 0x20).unwrap_or(0)))
            .collect::<Vec<_>>()
    );
    let mut seen = 0;
    rs::each_object(|o| {
        let Some(class) = read_u32(o + 0x24) else { return };
        if class == 0 || !rs::made_from(class as usize, pawn) {
            return;
        }
        seen += 1;
        if seen > 64 {
            return;
        }
        let loc = o + usize::from(t.location);
        info!(
            "Characters: test: {o:#x} {} ({:#x}) class {class:#x} {} ({:#x}) (exact {}) alive {} at {:?}",
            read_u32(o + 0x20).and_then(rs::name_text).unwrap_or_default(),
            read_u32(o + 0x20).unwrap_or(0),
            read_u32(class as usize + 0x20).and_then(rs::name_text).unwrap_or_default(),
            read_u32(class as usize + 0x20).unwrap_or(0),
            t.pawn_classes.iter().any(|(n, _)| t.names.get(n).is_some_and(|i| rs::of_class(o, rs::pack(*i, 0)))),
            rs::alive(o),
            (read::<f32>(loc), read::<f32>(loc + 4), read::<f32>(loc + 8)),
        );
        // Who it is: its network id (Actor.NetInstanceId, a QWord at 708; UID at 204), its
        // controller's class, flag +0x4d (the name getter's local/remote test), team and name.
        let controller = t.controller.and_then(|c| read_u32(o + usize::from(c))).unwrap_or(0) as usize;
        let control = control_of(t, o);
        info!(
            "Characters: test: {o:#x} net id {:#x}:{:#x} uid {:#x} controller {controller:#x} flag {:?} role {:?} team {:?} name {:?}",
            read_u32(o + 712).unwrap_or(0),
            read_u32(o + 708).unwrap_or(0),
            read_u32(o + 204).unwrap_or(0),
            (controller != 0).then(|| read::<u8>(controller + 0x4d)).flatten(),
            read::<u8>(o + 0x4d),
            control.as_ref().and_then(|c| c.team),
            control.and_then(|c| c.name),
        );
    });
    info!("Characters: test: {seen} objects made from Pawn");
}

/// The offsets the look-up resolved by name (a test build's `offsets` command: route 2's check).
#[cfg(feature = "test-commands")]
pub(crate) fn test_offsets() {
    let names = rs::name_indices(&[
        "Class",
        "Actor",
        "Pawn",
        "EPawn",
        "Location",
        "Rotation",
        "GroundSpeed",
        "Health",
        "Controller",
        "CurrentWeapon",
    ]);
    for (class, props) in [
        ("Actor", &["Location", "Rotation"][..]),
        ("Pawn", &["GroundSpeed", "Health", "Controller"][..]),
        ("EPawn", &["CurrentWeapon"][..]),
    ] {
        for c in rs::classes_named(&names, class) {
            for p in props {
                info!("Characters: test: offsets: {class}.{p} {:?}", rs::offset_in(&names, p, c));
            }
        }
    }
}

/// Samples the characters every [`SAMPLE`] for each of `watchers` (none: nothing starts), in
/// either build once the engine's tables are found.
pub fn start(mut watchers: Vec<Box<dyn Watcher>>) {
    if watchers.is_empty() {
        return;
    }
    if !rs::tables_here() {
        info!("Characters: the engine's tables aren't found in this game: no witness reports nor recordings");
        return;
    }
    let spawned = std::thread::Builder::new().name("fe-pawns".into()).spawn(move || {
        let mut tables: Option<Tables> = None;
        let mut looked: Option<Instant> = None;
        let mut again = 0;
        let mut numbers = HashMap::new();
        let mut found_list = Instant::now() - Duration::from_secs(10);
        let mut list: Vec<(usize, &'static str)> = Vec::new();
        loop {
            std::thread::sleep(SAMPLE);
            // Looked up again while a match's parts are missing: its classes (names, shots) are
            // loaded with the first match, after the game starts.
            // At most LOOKS_AGAIN times once the classes are found: each is a whole scan.
            let missing = |t: &Tables| !t.has_classes() || ((t.nickname.is_none() || t.bullets.is_none()) && again < LOOKS_AGAIN);
            if tables.as_ref().is_none_or(missing) && looked.is_none_or(|t| t.elapsed() >= Duration::from_secs(30)) {
                looked = Some(Instant::now());
                if tables.as_ref().is_some_and(Tables::has_classes) {
                    again += 1;
                }
                let was = tables.as_ref().map(|t| (t.has_classes(), t.nickname.is_some(), t.bullets.is_some()));
                tables = look_up().or(tables);
                if let Some(t) = tables
                    .as_ref()
                    .filter(|t| t.has_classes() && was != Some((true, t.nickname.is_some(), t.bullets.is_some())))
                {
                    info!(
                        "Characters: {} player character classes, names {}, facing {}, health {}, weapons' shots {}",
                        t.pawn_classes.len(),
                        if t.nickname.is_some() { "readable" } else { "not found" },
                        if t.rotation.is_some() { "readable" } else { "not found" },
                        if t.health.is_some() { "readable" } else { "not found" },
                        t.bullets.as_ref().map_or_else(
                            || String::from("not found"),
                            |b| format!("at +{:#x} (stamp: start +{}, end +{}, time +{}, {} bytes)", b.history, b.start, b.end, b.time, b.stride)
                        ),
                    );
                }
            }
            let Some(t) = &tables else { continue };
            // The characters looked for once a second; their state every sample.
            if found_list.elapsed() >= Duration::from_secs(1) {
                found_list = Instant::now();
                list = characters(t, &mut numbers);
            }
            #[cfg(feature = "test-commands")]
            test_pawns(t, &list);
            let now = Instant::now();
            let samples: Vec<Sample> = list.iter().filter_map(|(o, class)| sample(t, *o, class)).collect();
            UNNAMED_SEEN.store(
                samples.iter().any(|s| s.peer.is_some() && s.control.as_ref().is_none_or(|c| c.name.is_none())),
                std::sync::atomic::Ordering::Relaxed,
            );
            for w in &mut watchers {
                w.sample(t, now, &samples);
            }
        }
    });
    if let Err(e) = spawned {
        warn!("Characters: couldn't start ({e})");
    }
}
