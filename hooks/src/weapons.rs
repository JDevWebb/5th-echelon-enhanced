//! Weapon archetypes (0.5.2's W1, report only): every loaded `EWeaponArchetype`'s stats that the
//! 2022 cheat table edits, and the ones a cheat would edit next (Blacklist.umd's schema: clip and
//! spare clips, penetration, close-range boost and distance, falloff, shooting and precision
//! range, rate of fire, reload times, shots per burst, bullet speed), hashed per archetype by its
//! name. Every game in a match loads the same archetypes with the same stats, so the server
//! compares a guest's hashes with its host's (as rules, R4): a weapon whose hash differs was
//! edited in one of the two games. Sent with the match state (`api::match_state`).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use crate::dataversion::read;
use crate::dataversion::read_u32;
use crate::round_state::name_text;
use crate::round_state::pack;

/// The stats read: integers (`m_n…`, `m_i…`) and floats (`m_f…`).
const STATS: [&str; 18] = [
    "m_nClipCapacity",
    "m_nMaxSpareClips",
    "m_iBulletPenetrationPower",
    "m_fMPCloseRangeDamageBoost",
    "m_fMPCloseRangeDistance",
    "m_fMaxFalloffMultiplier",
    "m_nMaxFalloffRange",
    "m_nShootingRange",
    "m_nPrecisionRange",
    "m_fRateOfFire",
    "m_fFullReloadTime",
    "m_fTacticalReloadTime",
    "m_nShotsPerBurst",
    "m_fBulletSpeed",
    "m_fLowClipAmmoThreshold",
    "m_fLowSpareAmmoThreshold",
    // Switches on the archetype (bits, round_state::BOOL_MASK_AT).
    "m_bUnlimitedAmmo",
    "m_bNoReloadNecessary",
];

/// W2: the live weapons' switches that no player's weapon has on in a match (the
/// WeaponFiringComponent's own; found on the gamebox, 2026-11-14: 0x40000, 0x80000, 0x100000 of
/// the word at 236). `m_bInfiniteBulletPenetration` and `m_bIsFullyAccurate` have no name in
/// this build's name table, so they aren't found this way.
const SWITCHES: [&str; 3] = ["m_bUnlimitedAmmo", "m_bNoReloadNecessary", "m_bDisableSpread"];
/// The most archetypes hashed.
const MOST: usize = 400;
/// How often they're read again in a match.
const EVERY: Duration = Duration::from_secs(60);

struct Found {
    class: u32,
    /// Each stat's byte offset in an archetype, and a switch's bit.
    offsets: Vec<(&'static str, u16, Option<u32>)>,
    /// The live weapons' class (WeaponFiringComponent) and its switches: (name, offset, bit).
    firing: Option<(u32, Vec<(&'static str, u16, u32)>)>,
}

fn found() -> Option<&'static Found> {
    static FOUND: std::sync::OnceLock<Option<Found>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            let mut wanted = vec!["EWeaponArchetype", "WeaponFiringComponent", "Class"];
            wanted.extend(STATS);
            wanted.extend(SWITCHES);
            let names = crate::round_state::name_indices(&wanted);
            let classes = crate::round_state::classes_named(&names, "EWeaponArchetype");
            let class = *classes.first()?;
            let owners: Vec<u32> = classes.iter().map(|c| *c as u32).collect();
            let offsets = STATS
                .iter()
                .filter_map(|stat| {
                    let prop = crate::round_state::named(pack(*names.get(stat)?, 0))
                        .into_iter()
                        .find(|p| read_u32(p + 0x18).is_some_and(|outer| owners.contains(&outer)))?;
                    let mask = stat.starts_with("m_b").then(|| read_u32(prop + crate::round_state::BOOL_MASK_AT)).flatten().filter(|m| m.count_ones() == 1);
                    if stat.starts_with("m_b") && mask.is_none() {
                        return None;
                    }
                    Some((*stat, read::<u16>(prop + 0x3a)?, mask))
                })
                .collect::<Vec<_>>();
            let firing_classes = crate::round_state::classes_named(&names, "WeaponFiringComponent");
            let firing = firing_classes.first().and_then(|class| {
                let owners: Vec<u32> = firing_classes.iter().map(|c| *c as u32).collect();
                let switches: Vec<(&'static str, u16, u32)> = SWITCHES
                    .iter()
                    .filter_map(|name| {
                        let prop = crate::round_state::named(pack(*names.get(name)?, 0))
                            .into_iter()
                            .find(|p| read_u32(p + 0x18).is_some_and(|outer| owners.contains(&outer)))?;
                        let mask = read_u32(prop + crate::round_state::BOOL_MASK_AT).filter(|m| m.count_ones() == 1)?;
                        Some((*name, read::<u16>(prop + 0x3a)?, mask))
                    })
                    .collect();
                (!switches.is_empty()).then(|| Some((read_u32(class + 0x20)?, switches))).flatten()
            });
            tracing::info!("Weapons: {} live weapon switches found", firing.as_ref().map_or(0, |(_, s)| s.len()));
            if offsets.is_empty() {
                tracing::info!("Weapons: the archetypes' stats weren't found; not compared");
                return None;
            }
            tracing::info!("Weapons: {} of {} stats found on the archetypes", offsets.len(), STATS.len());
            Some(Found { class: read_u32(class + 0x20)?, offsets, firing })
        })
        .as_ref()
}

/// FNV-1a over the stats' bytes.
fn hash(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(*b)).wrapping_mul(0x0100_0193))
}

/// Each loaded archetype's hash, by its name.
fn read_all(f: &Found) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    crate::round_state::each_object(|o| {
        if out.len() >= MOST || !crate::round_state::of_class(o, f.class) {
            return;
        }
        let mut bytes = Vec::with_capacity(f.offsets.len() * 4);
        for (_, offset, mask) in &f.offsets {
            let word = read::<u32>(o + usize::from(*offset)).unwrap_or(0);
            let value = mask.map_or(word, |m| u32::from(word & m != 0));
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        if let Some(name) = read_u32(o + 0x20).and_then(name_text) {
            out.insert(name, hash(&bytes));
        }
    });
    out
}

/// W2: the live weapons with a switch on, by switch: how many.
fn switches_on(f: &Found) -> BTreeMap<&'static str, u32> {
    let mut on = BTreeMap::new();
    let Some((class, switches)) = &f.firing else { return on };
    crate::round_state::each_object(|o| {
        if !crate::round_state::of_class(o, *class) {
            return;
        }
        for (name, offset, mask) in switches {
            if read_u32(o + usize::from(*offset)).is_some_and(|w| w & mask != 0) {
                *on.entry(*name).or_insert(0) += 1;
            }
        }
    });
    on
}

/// W2, each minute in a match: a live weapon with a cheat switch on is told the server once a
/// match (`fair_play`, check `weapon`). Report only: a stock weapon or gadget that turns one on
/// would show here first.
pub fn check_switches(under_way: bool) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !under_way {
        SAID.store(false, std::sync::atomic::Ordering::Relaxed);
        return;
    }
    let Some(f) = found() else { return };
    let on = switches_on(f);
    if on.is_empty() || SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let said: Vec<String> = on.iter().map(|(k, n)| format!("{k} on {n} weapon{}", if *n == 1 { "" } else { "s" })).collect();
    let detail = format!("live weapons with a cheat switch on: {}", said.join(", "));
    tracing::warn!("Weapons: {detail}");
    #[allow(clippy::cast_precision_loss)]
    crate::api::fair_play_check("weapon", &detail, on.values().sum::<u32>() as f64, hooks_config::running_under_wine());
}

static LAST: Mutex<Option<(Instant, BTreeMap<String, u32>, bool)>> = Mutex::new(None);

/// The archetypes' hashes to send with this match state: read again every [`EVERY`] in a match,
/// and sent when they changed or the last were sent a while ago; none outside a match.
pub fn for_match_state(under_way: bool) -> HashMap<String, u32> {
    if !under_way {
        check_switches(false);
        return HashMap::new();
    }
    let Some(f) = found() else { return HashMap::new() };
    let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let stale = last.as_ref().is_none_or(|(at, ..)| at.elapsed() >= EVERY);
    if stale {
        check_switches(under_way);
        let now = read_all(f);
        let changed = last.as_ref().is_none_or(|(_, before, _)| *before != now);
        *last = Some((Instant::now(), now, changed || last.as_ref().is_some_and(|(at, ..)| at.elapsed() >= EVERY * 4)));
    }
    match last.as_mut() {
        Some((_, all, send)) if *send => {
            *send = false;
            all.iter().map(|(k, v)| (k.clone(), *v)).collect()
        }
        _ => HashMap::new(),
    }
}
