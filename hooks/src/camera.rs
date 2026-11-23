//! Two beta features of the view (0.5.6's S, Montague's suggestions): no camera shake, and a
//! wider field of view. Both are the live camera's own properties (echelon.u's `ECamera`:
//! `m_playCameraShakes`, `m_fFreeFOVOffset`), found by name in the engine's tables as round_state
//! finds the match manager, and set again every [`EVERY`] (the camera is made anew with each
//! map). Only this player's view changes; nothing is sent to anyone.

use std::time::Duration;

use tracing::info;

use crate::dataversion::read;
use crate::dataversion::read_u32;
use crate::round_state as rs;

const EVERY: Duration = Duration::from_secs(2);
/// Degrees the wider view adds.
pub const WIDER_BY: f32 = 10.0;
/// Where the two are when their names don't resolve (this build's name table lacks most of
/// echelon.u's): their places in ECamera's layout as the engine makes it from the script
/// (blacklist-re `lead_layout.py`: `m_playCameraShakes` bit 1 of the word at 880, `m_freeFOV` at
/// 1488 and `m_fFreeFOVOffset` after it at 1492). Taken only when `m_freeFOV` there reads as a field
/// of view, so another layout is left alone.
const SHAKE_PLACE: (u16, u32) = (880, 1 << 1);
const FREE_FOV_AT: u16 = 1488;
const FREE_FOV_OFFSET_AT: u16 = 1492;

/// Whether `camera`'s layout is the one the places above are for.
fn layout_known(camera: usize) -> bool {
    read::<f32>(camera + usize::from(FREE_FOV_AT)).is_some_and(|fov| (30.0..=150.0).contains(&fov))
}

/// How often the live cameras are looked for again (a whole scan of the object table).
const LOOK_EVERY: Duration = Duration::from_secs(10);

/// Starts the thread, when either is on.
pub fn start(no_shake: bool, wider: bool) {
    if !no_shake && !wider {
        return;
    }
    let spawned = std::thread::Builder::new().name("fe-camera".into()).spawn(move || run(no_shake, wider));
    if let Err(e) = spawned {
        tracing::warn!("Camera: couldn't start ({e})");
    }
}

fn run(no_shake: bool, wider: bool) {
    // The engine's tables come once the game has loaded its packages.
    let names = loop {
        if rs::tables_here() {
            let names = rs::name_indices(&["Class", "ECamera", "m_playCameraShakes", "m_fFreeFOVOffset"]);
            if names.contains_key("ECamera") {
                break names;
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    };
    let mut said = (false, false, false);
    let mut class = None;
    let mut cameras: Vec<usize> = Vec::new();
    let mut looked: Option<std::time::Instant> = None;
    loop {
        std::thread::sleep(EVERY);
        if class.is_none() {
            class = rs::class_named(&names, "ECamera");
        }
        let Some(class) = class else { continue };
        // Every live object made from ECamera (its instances aren't found by a low number:
        // the gamebox, 23 Nov), looked for again every [`LOOK_EVERY`].
        if looked.is_none_or(|at: std::time::Instant| at.elapsed() >= LOOK_EVERY) || cameras.iter().any(|c| !rs::alive(*c)) {
            looked = Some(std::time::Instant::now());
            let Some(index) = names.get("ECamera") else { continue };
            let packed = rs::pack(*index, 0);
            let mut found = Vec::new();
            rs::each_object(|o| {
                if found.len() < 8 && read_u32(o + 0x24).is_some_and(|c| rs::made_from(c as usize, packed)) && rs::alive(o) {
                    found.push(o);
                }
            });
            if found.len() != cameras.len() {
                info!("Camera: {} live cameras", found.len());
            }
            cameras = found;
            if !cameras.is_empty() && !said.2 {
                said.2 = true;
                info!(
                    "Camera: shake switch {:?}, free view offset at {:?} by name; the layout's places {} (free view {:?})",
                    shake_place(&names, class),
                    rs::offset_in(&names, "m_fFreeFOVOffset", class),
                    if layout_known(cameras[0]) { "fit" } else { "don't fit" },
                    read::<f32>(cameras[0] + usize::from(FREE_FOV_AT))
                );
            }
        }
        for &camera in &cameras {
            if no_shake {
                if let Some((offset, mask)) = shake_place(&names, class).or_else(|| layout_known(camera).then_some(SHAKE_PLACE)) {
                    let at = camera + usize::from(offset);
                    if let Some(w) = read_u32(at).filter(|w| w & mask != 0) {
                        // SAFETY: the live camera's bool word, read just before.
                        unsafe { (at as *mut u32).write_unaligned(w & !mask) };
                        if !said.0 {
                            info!("Camera: shakes off (m_playCameraShakes at +{offset})");
                            said.0 = true;
                        }
                    }
                }
            }
            if wider {
                if let Some(offset) = rs::offset_in(&names, "m_fFreeFOVOffset", class).or_else(|| layout_known(camera).then_some(FREE_FOV_OFFSET_AT)) {
                    let at = camera + usize::from(offset);
                    if read::<f32>(at).is_some_and(|v| v.is_finite() && (v - WIDER_BY).abs() > 0.01 && v.abs() < 90.0) {
                        // SAFETY: the live camera's float, read just before.
                        unsafe { (at as *mut f32).write_unaligned(WIDER_BY) };
                        if !said.1 {
                            info!("Camera: field of view {WIDER_BY} degrees wider (m_fFreeFOVOffset at +{offset})");
                            said.1 = true;
                        }
                    }
                }
            }
        }
    }
}

/// `m_playCameraShakes`'s word and bit on the camera's class.
fn shake_place(names: &std::collections::HashMap<&'static str, u32>, class: usize) -> Option<(u16, u32)> {
    let chain = rs::chain(class);
    let prop = rs::named(rs::pack(*names.get("m_playCameraShakes")?, 0))
        .into_iter()
        .find(|o| read_u32(o + 0x18).is_some_and(|outer| chain.contains(&outer)))?;
    let mask = read_u32(prop + rs::BOOL_MASK_AT).filter(|m| m.count_ones() == 1)?;
    Some((read::<u16>(prop + 0x3a)?, mask))
}
