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
const STATS: [&str; 16] = [
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
];
/// The most archetypes hashed.
const MOST: usize = 400;
/// How often they're read again in a match.
const EVERY: Duration = Duration::from_secs(60);

struct Found {
    class: u32,
    /// Each stat's byte offset in an archetype.
    offsets: Vec<(&'static str, u16)>,
}

fn found() -> Option<&'static Found> {
    static FOUND: std::sync::OnceLock<Option<Found>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            let mut wanted = vec!["EWeaponArchetype", "Class"];
            wanted.extend(STATS);
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
                    Some((*stat, read::<u16>(prop + 0x3a)?))
                })
                .collect::<Vec<_>>();
            if offsets.is_empty() {
                tracing::info!("Weapons: the archetypes' stats weren't found; not compared");
                return None;
            }
            tracing::info!("Weapons: {} of {} stats found on the archetypes", offsets.len(), STATS.len());
            Some(Found { class: read_u32(class + 0x20)?, offsets })
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
        for (_, offset) in &f.offsets {
            bytes.extend_from_slice(&read::<u32>(o + usize::from(*offset)).unwrap_or(0).to_le_bytes());
        }
        if let Some(name) = read_u32(o + 0x20).and_then(name_text) {
            out.insert(name, hash(&bytes));
        }
    });
    out
}

static LAST: Mutex<Option<(Instant, BTreeMap<String, u32>, bool)>> = Mutex::new(None);

/// The archetypes' hashes to send with this match state: read again every [`EVERY`] in a match,
/// and sent when they changed or the last were sent a while ago; none outside a match.
pub fn for_match_state(under_way: bool) -> HashMap<String, u32> {
    if !under_way {
        return HashMap::new();
    }
    let Some(f) = found() else { return HashMap::new() };
    let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let stale = last.as_ref().is_none_or(|(at, ..)| at.elapsed() >= EVERY);
    if stale {
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
