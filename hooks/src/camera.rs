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
/// Camera instances looked through (`ECamera_0`, `_1`, ...).
const NUMBERS: u32 = 16;

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
    let mut said = (false, false);
    let mut class = None;
    loop {
        std::thread::sleep(EVERY);
        if class.is_none() {
            class = rs::class_named(&names, "ECamera");
        }
        let Some(class) = class else { continue };
        let Some(index) = names.get("ECamera") else { continue };
        let cameras: Vec<usize> = (0..NUMBERS)
            .flat_map(|n| rs::named(rs::pack(*index, n)))
            .filter(|o| rs::of_class(*o, rs::pack(*index, 0)) && rs::alive(*o))
            .collect();
        for camera in cameras {
            if no_shake {
                if let Some((offset, mask)) = shake_place(&names, class) {
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
                if let Some(offset) = rs::offset_in(&names, "m_fFreeFOVOffset", class) {
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
