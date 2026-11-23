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

/// Two more in the same word, whose names this build lacks: placed by declaration order
/// (Blacklist.umd's package order: the word at 236 holds WeaponFiringComponent's bools 10-41,
/// bit = index - 10; on the gamebox, 14 Nov, the bits set at rest were exactly the flags that
/// should be on). Taken only when m_bDisableSpread's bit is where that order puts it.
const PLACED_SWITCHES: [(&str, u32); 2] = [("m_bInfiniteBulletPenetration", 1 << 3), ("m_bIsFullyAccurate", 1 << 21)];
const DISABLE_SPREAD_BIT: u32 = 1 << 20;
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
                    let mask = stat
                        .starts_with("m_b")
                        .then(|| read_u32(prop + crate::round_state::BOOL_MASK_AT))
                        .flatten()
                        .filter(|m| m.count_ones() == 1);
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
                let mut switches = switches;
                if let Some((_, offset, _)) = switches.iter().find(|(n, _, m)| *n == "m_bDisableSpread" && *m == DISABLE_SPREAD_BIT).copied() {
                    switches.extend(PLACED_SWITCHES.iter().map(|(n, m)| (*n, offset, *m)));
                }
                (!switches.is_empty()).then(|| Some((read_u32(class + 0x20)?, switches))).flatten()
            });
            tracing::info!("Weapons: {} live weapon switches found", firing.as_ref().map_or(0, |(_, s)| s.len()));
            if offsets.is_empty() {
                tracing::info!("Weapons: the archetypes' stats weren't found; not compared");
                return None;
            }
            tracing::info!("Weapons: {} of {} stats found on the archetypes", offsets.len(), STATS.len());
            Some(Found {
                class: read_u32(class + 0x20)?,
                offsets,
                firing,
            })
        })
        .as_ref()
}

/// FNV-1a over the stats' bytes.
fn hash(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(*b)).wrapping_mul(0x0100_0193))
}

/// W1, attachments: an attachment's stat changes (clip capacity and spare clips, penetration,
/// recoil, dispersion, damage, impulse, falloff, ranges, FOV, aim and reload times; modifiers and
/// biases) aren't on the weapon's archetype but on the attachment's own (`EWeaponAttachmentArchetype`,
/// 37 floats from 104 to 252 in the script's layout, blacklist-re `lead_layout.py`), so a scope is
/// never an edit of the weapon; the attachments are hashed the same way, by name, so an edited
/// one is. Taken only when the block reads as that layout says (finite; the FOV multiplier, at
/// 240, between 0 and 5).
const ATTACHMENT_MODIFIERS: std::ops::Range<usize> = 104..252;
const ATTACHMENT_FOV_AT: usize = 240;

fn attachment_hash(o: usize) -> Option<u32> {
    let mut bytes = Vec::with_capacity(ATTACHMENT_MODIFIERS.len());
    for at in ATTACHMENT_MODIFIERS.step_by(4) {
        let v = read::<f32>(o + at)?;
        if !v.is_finite() {
            return None;
        }
        bytes.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    let fov = read::<f32>(o + ATTACHMENT_FOV_AT)?;
    (fov > 0.0 && fov <= 5.0).then(|| hash(&bytes))
}

/// The attachment archetypes' own class name isn't in this build's name table (the gamebox,
/// 23 Nov), so they're reached through the live weapons characters hold (pawns.rs):
/// BasicWeapon.m_WeaponUpgrades (an array at 1892) of WeaponUpgrades, each with its
/// m_AttachmentArchetypeName (1788) and m_AttachmentArchetypeInfo (1792). The array has a slot
/// per attachment place, null when none is fitted (the gamebox's rank 5 loadouts: 10 empty slots
/// on the UMP45, 13 on the AK-47).
const WEAPON_UPGRADES_AT: usize = 1892;
const UPGRADE_ATTACHMENT_NAME_AT: usize = 1788;
const UPGRADE_ATTACHMENT_AT: usize = 1792;
/// The most upgrades read on one weapon.
const MOST_UPGRADES: u32 = 16;

/// The attachments on live weapons: (attachment archetype's name, its object).
fn attachments() -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for o in crate::pawns::live_weapons() {
        let (Some(data), Some(count)) = (read_u32(o + WEAPON_UPGRADES_AT), read_u32(o + WEAPON_UPGRADES_AT + 4)) else { continue };
        if data == 0 || count > MOST_UPGRADES {
            continue;
        }
        for k in 0..count as usize {
            let Some(upgrade) = read_u32(data as usize + k * 4).filter(|u| *u != 0).map(|u| u as usize) else { continue };
            let Some(archetype) = read_u32(upgrade + UPGRADE_ATTACHMENT_AT).filter(|a| *a != 0).map(|a| a as usize) else { continue };
            let name = read_u32(upgrade + UPGRADE_ATTACHMENT_NAME_AT)
                .and_then(name_text)
                .or_else(|| read_u32(archetype + 0x20).and_then(name_text))
                .filter(|n| !n.is_empty() && !n.contains(' '));
            if let Some(name) = name {
                if !out.iter().any(|(n, a)| *n == name || *a == archetype) {
                    out.push((name, archetype));
                }
            }
        }
    }
    out
}

/// Each loaded archetype's hash, by its name (attachments' too).
fn read_all(f: &Found) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    {
        let mut skipped = 0;
        let mut hashed = 0;
        for (name, archetype) in attachments() {
            match attachment_hash(archetype) {
                Some(h) => {
                    out.insert(format!("attachment:{name}"), h);
                    hashed += 1;
                }
                None => skipped += 1,
            }
        }
        static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if hashed + skipped > 0 && !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::info!("Weapons: {hashed} attachment archetypes hashed, {skipped} left out (not as their layout says)");
        }
    }
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

/// FN: weapons whose own archetype turns a switch on (the game's, not a cheat's): by the
/// weapon's class name, the switches it has on by design. Filled from the gamebox's `firing`
/// test command and the notes' weapon names (0.5.6's FN: `m_bUnlimitedAmmo on 1 weapon` was a
/// third of all players in SvM).
const STOCK_SWITCHES: &[(&str, &[&str])] = &[];

/// The class name of the weapon a firing component belongs to (its outer's class), if readable.
fn weapon_class(component: usize) -> Option<String> {
    let outer = read_u32(component + 0x18).filter(|o| *o != 0)? as usize;
    crate::round_state::kind_name(outer)
}

/// Whether `weapon` (a class name) has `switch` on by design ([`STOCK_SWITCHES`]).
fn stock(weapon: &str, switch: &str) -> bool {
    STOCK_SWITCHES.iter().any(|(w, on)| w.eq_ignore_ascii_case(weapon) && on.contains(&switch))
}

/// W2: the live weapons with a switch on (stock ones left out), by switch: the weapons' classes.
fn switches_on(f: &Found) -> BTreeMap<&'static str, Vec<String>> {
    let mut on: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let Some((class, switches)) = &f.firing else { return on };
    crate::round_state::each_object(|o| {
        if !crate::round_state::of_class(o, *class) {
            return;
        }
        let mut weapon: Option<String> = None;
        for (name, offset, mask) in switches {
            if read_u32(o + usize::from(*offset)).is_some_and(|w| w & mask != 0) {
                let w = weapon.get_or_insert_with(|| weapon_class(o).unwrap_or_else(|| String::from("?"))).clone();
                if !stock(&w, name) {
                    on.entry(*name).or_default().push(w);
                }
            }
        }
    });
    on
}

/// How a switch's weapons read in a note: "m_bUnlimitedAmmo on 1 weapon (EWeaponX)".
fn said(switch: &str, weapons: &[String]) -> String {
    let n = weapons.len();
    let mut names: Vec<&str> = weapons.iter().map(String::as_str).collect();
    names.sort_unstable();
    names.dedup();
    let names: Vec<&str> = names.into_iter().take(4).collect();
    format!("{switch} on {n} weapon{} ({})", if n == 1 { "" } else { "s" }, names.join(", "))
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
    let parts: Vec<String> = on.iter().map(|(k, w)| said(k, w)).collect();
    let detail = format!("live weapons with a cheat switch on: {}", parts.join(", "));
    tracing::warn!("Weapons: {detail}");
    #[allow(clippy::cast_precision_loss)]
    crate::api::fair_play_check("weapon", &detail, on.values().map(Vec::len).sum::<usize>() as f64, hooks_config::running_under_wine());
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

/// The gamebox's `firing`: the live weapons' switch word (WeaponFiringComponent's at 236), each
/// value seen and how often, and how many weapons have each bit on: for checking which bit is
/// which before reading more of them.
#[cfg(feature = "test-commands")]
pub fn test_firing_words() {
    {
        let mut seen = Vec::new();
        crate::pawns::live_weapons().into_iter().for_each(|o| {
            {
                let arr = |at: usize| (read_u32(o + at), read_u32(o + at + 4));
                if let (Some(data), Some(n)) = arr(WEAPON_UPGRADES_AT) {
                    for k in 0..n.min(3) as usize {
                        let u = read_u32(data as usize + k * 4).unwrap_or(0) as usize;
                        if u == 0 {
                            continue;
                        }
                        let class = read_u32(u + 0x24).and_then(|c| read_u32(c as usize + 0x20)).and_then(name_text);
                        let words: Vec<String> = (1776..1808).step_by(4).map(|at| format!("{at}:{:x}", read_u32(u + at).unwrap_or(0))).collect();
                        seen.push(format!("  upgrade {k}: {:?} class {class:?} {}", read_u32(u + 0x20).and_then(name_text), words.join(" ")));
                    }
                }
                seen.push(format!(
                    "{:?} upgrades {:?} starting {:?}",
                    crate::round_state::kind_name(o),
                    arr(WEAPON_UPGRADES_AT),
                    arr(WEAPON_UPGRADES_AT - 12)
                ));
            }
        });
        tracing::info!("Weapons: test: live weapons {seen:?}; attachments {:?}", attachments());
    }
    let Some(f) = found() else { return };
    let Some((class, switches)) = &f.firing else { return };
    let Some(offset) = switches.first().map(|(_, o, _)| *o) else { return };
    let mut values: BTreeMap<u32, u32> = BTreeMap::new();
    let mut bits = [0u32; 32];
    crate::round_state::each_object(|o| {
        if !crate::round_state::of_class(o, *class) {
            return;
        }
        if let Some(w) = read_u32(o + usize::from(offset)) {
            *values.entry(w).or_insert(0) += 1;
            for (b, n) in bits.iter_mut().enumerate() {
                if w & (1 << b) != 0 {
                    *n += 1;
                }
            }
        }
    });
    let total: u32 = values.values().sum();
    let on: Vec<String> = bits.iter().enumerate().filter(|(_, n)| **n > 0).map(|(b, n)| format!("{b}:{n}")).collect();
    tracing::info!(
        "Weapons: test: {total} live weapons at {offset}; bits on (bit:weapons) {}; values {values:x?}",
        on.join(" ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_switch_note_names_its_weapons() {
        assert_eq!(said("m_bUnlimitedAmmo", &["EWeaponB".into()]), "m_bUnlimitedAmmo on 1 weapon (EWeaponB)");
        assert_eq!(
            said("m_bUnlimitedAmmo", &["EWeaponB".into(), "EWeaponA".into(), "EWeaponB".into()]),
            "m_bUnlimitedAmmo on 3 weapons (EWeaponA, EWeaponB)"
        );
        assert!(!stock("EWeaponA", "m_bUnlimitedAmmo"), "nothing stock until the gamebox names it");
    }
}
