//! Voice chat's headset output and microphone, and the sound-device callback that reads them
//! (the hooks' `voice_device_fix` has the whole story).

use super::Image;

/// The output's destructor (it installs the output's vtable, whose immediate is at +11).
#[rustfmt::skip]
pub const PLAYBACK_DTOR: [Option<u8>; 42] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x51), Some(0x53), Some(0x56), Some(0x57), Some(0x8B), Some(0xF9),
    Some(0xC7), Some(0x07), None, None, None, None, // mov [edi],<vtable>
    Some(0xC7), Some(0x47), Some(0x2C), None, None, None, None, // mov [edi+0x2c],<vtable 2>
    Some(0x8B), Some(0x87), Some(0x80), Some(0x00), Some(0x00), Some(0x00), Some(0x8B), Some(0x50), Some(0x10),
    Some(0x6A), Some(0x01), Some(0x8D), Some(0x8F), Some(0x80), Some(0x00), Some(0x00), Some(0x00),
    Some(0xC7), Some(0x47), Some(0x7C),
];
const PLAYBACK_VTABLE_AT: usize = 11;

/// The microphone's destructor (its vtable's immediate is at +17).
#[rustfmt::skip]
pub const CAPTURE_DTOR: [Option<u8>; 21] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x51), Some(0x53), Some(0x56), Some(0x8B), Some(0xF1), Some(0x57),
    Some(0x8D), Some(0x8E), Some(0x04), Some(0x26), Some(0x00), Some(0x00), // lea ecx,[esi+0x2604]
    Some(0xC7), Some(0x06), None, None, None, None, // mov [esi],<vtable>
];
const CAPTURE_VTABLE_AT: usize = 17;

/// The output's teardown: `if (opened) { opened = 0; …; close }`, `thiscall(this, result*)`.
#[rustfmt::skip]
pub const PLAYBACK_DEINIT: [Option<u8>; 36] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x83), Some(0xEC), Some(0x08), Some(0x53), Some(0x8B), Some(0xD9),
    Some(0x80), Some(0x7B), Some(0x08), Some(0x00), Some(0x56), Some(0x89), Some(0x5D), Some(0xF8), // cmp byte [ebx+8],0
    Some(0x0F), Some(0x84), None, None, None, None,
    Some(0x8B), Some(0x73), Some(0x0C), Some(0x57), Some(0x8D), Some(0x7B), Some(0x0C), Some(0xC6), Some(0x43), Some(0x08), Some(0x00), // mov byte [ebx+8],0
    Some(0x3B), Some(0xF7),
];

/// The generic device teardown (the microphone's): `if (opened) { opened = 0; vtable[5] close }`.
#[rustfmt::skip]
pub const GENERIC_DEINIT: [Option<u8>; 34] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x51), Some(0x56), Some(0x8B), Some(0xF1),
    Some(0x80), Some(0x7E), Some(0x08), Some(0x00), Some(0x74), Some(0x4B), // cmp byte [esi+8],0 / jz
    Some(0x8B), Some(0x06), Some(0x8B), Some(0x50), Some(0x14), Some(0x8D), Some(0x4D), Some(0xFC), Some(0x51), Some(0x8B), Some(0xCE),
    Some(0xC6), Some(0x46), Some(0x08), Some(0x00), Some(0xFF), Some(0xD2), // mov byte [esi+8],0 / call [vtable+0x14]
    Some(0x8D), Some(0x4D), Some(0xFC), Some(0xE8),
];

/// `OnDeviceStateChanged(this, id, state)`: builds a string of `id`, then looks it up among the
/// owner's outputs (`lea ecx,[esi-0x2c]`) and sets the match's availability (`[eax+0x58]`).
#[rustfmt::skip]
pub const CALLBACK: [Option<u8>; 56] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x83), Some(0xEC), Some(0x14), Some(0x53), Some(0x56), Some(0x8B), Some(0x75), Some(0x08),
    Some(0x57), Some(0x8B), Some(0x7D), Some(0x0C), Some(0x83), Some(0xEC), Some(0x10), Some(0x8B), Some(0xCC), Some(0x33), Some(0xDB),
    Some(0x57), Some(0x89), Some(0x5D), Some(0xFC), Some(0xE8), None, None, None, None,
    Some(0x8D), Some(0x4E), Some(0xD4), Some(0xE8), None, None, None, None,
    Some(0x85), Some(0xC0), Some(0x74), Some(0x0A), Some(0x83), Some(0x7D), Some(0x10), Some(0x01),
    Some(0x0F), Some(0x94), Some(0xC1), Some(0x88), Some(0x48), Some(0x58), Some(0x8B), Some(0x76), Some(0xD8),
];

/// What the fix replaces and calls, by address.
#[derive(Debug)]
pub struct Sites {
    /// The output's vtable (its slot 0, the deleting destructor, is replaced) and that slot's
    /// original.
    pub playback_vtable: usize,
    pub playback_delete: usize,
    pub capture_vtable: usize,
    pub capture_delete: usize,
    /// The game's teardowns for the output and the microphone.
    pub playback_teardown: usize,
    pub capture_teardown: usize,
    /// The game's sound-device callback, and the one `.rdata` slot holding it.
    pub callback: usize,
    pub callback_slot: usize,
}

/// The vtable a destructor at `dtor` installs (its immediate at `dtor + vtable_at`) when it's in
/// `.rdata` and its slot 0 (the deleting destructor, `push ebp / mov ebp,esp / push esi /
/// mov esi,ecx / call <dtor>`) calls `dtor`: (vtable, slot 0).
fn checked_vtable(img: &Image, dtor: usize, vtable_at: usize) -> Option<(usize, usize)> {
    let vtable = img.u32(dtor + vtable_at)?;
    if !img.rdata_range().contains(&vtable) {
        return None;
    }
    let delete = img.u32(vtable)?;
    let text = img.text_range();
    if !text.contains(&delete) || !text.contains(&(delete + 10)) {
        return None;
    }
    let head = img.bytes(delete, 11)?;
    if head[..7] != [0x55, 0x8B, 0xEC, 0x56, 0x8B, 0xF1, 0xE8] {
        return None;
    }
    (img.rel32_target(delete + 7)? == dtor).then_some((vtable, delete))
}

/// The one `.rdata` word that holds `value`, by address.
fn word_holding(img: &Image, value: usize) -> Option<usize> {
    #[allow(clippy::cast_possible_truncation)]
    let bytes = (value as u32).to_le_bytes();
    let rdata = img.rdata;
    let hits: Vec<usize> = (0..rdata.len().saturating_sub(3)).step_by(4).filter(|&i| rdata[i..i + 4] == bytes).collect();
    match hits[..] {
        [at] => Some(img.rdata_va + at),
        _ => None,
    }
}

pub fn locate(img: &Image) -> Result<Sites, String> {
    let find = |what: &str, signature: &[Option<u8>]| img.find(signature).map_err(|n| format!("{what} found {n} times, not once"));
    let playback_dtor = find("the headset output's destructor", &PLAYBACK_DTOR)?;
    let capture_dtor = find("the microphone's destructor", &CAPTURE_DTOR)?;
    let playback_teardown = find("the headset output's teardown", &PLAYBACK_DEINIT)?;
    let capture_teardown = find("the device teardown", &GENERIC_DEINIT)?;
    let callback = find("the sound-device callback", &CALLBACK)?;
    let (playback_vtable, playback_delete) = checked_vtable(img, playback_dtor, PLAYBACK_VTABLE_AT).ok_or("the headset output's vtable didn't check out")?;
    let (capture_vtable, capture_delete) = checked_vtable(img, capture_dtor, CAPTURE_VTABLE_AT).ok_or("the microphone's vtable didn't check out")?;
    let callback_slot = word_holding(img, callback).ok_or("the sound-device callback's vtable slot wasn't found once")?;
    Ok(Sites {
        playback_vtable,
        playback_delete,
        capture_vtable,
        capture_delete,
        playback_teardown,
        capture_teardown,
        callback,
        callback_slot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_one_word_holding_a_value() {
        let mut rdata = vec![0u8; 64];
        rdata[16..20].copy_from_slice(&0x0200_92f0u32.to_le_bytes());
        let img = super::super::tests::image(&[], 0x0040_1000, &rdata, 0x02a0_0000);
        assert_eq!(word_holding(&img, 0x0200_92f0), Some(0x02a0_0010));
        rdata[40..44].copy_from_slice(&0x0200_92f0u32.to_le_bytes());
        let img = super::super::tests::image(&[], 0x0040_1000, &rdata, 0x02a0_0000);
        assert_eq!(word_holding(&img, 0x0200_92f0), None, "twice");
        assert_eq!(word_holding(&img, 0x1234), None);
    }

    #[test]
    fn a_vtable_checks_out_when_its_deleting_destructor_calls_the_destructor() {
        // .text from 0x0100_0000: the destructor at +0x10 installing vtable 0x0200_0008 at
        // +0x10+11, the deleting destructor at +0x80 calling it; .rdata from 0x0200_0000.
        let mut code = vec![0xCCu8; 0x100];
        code[0x10 + 11..0x10 + 15].copy_from_slice(&0x0200_0008u32.to_le_bytes());
        code[0x80..0x87].copy_from_slice(&[0x55, 0x8B, 0xEC, 0x56, 0x8B, 0xF1, 0xE8]);
        let rel = 0x0100_0010u32.wrapping_sub(0x0100_0080 + 11);
        code[0x87..0x8b].copy_from_slice(&rel.to_le_bytes());
        let mut rdata = vec![0u8; 0x20];
        rdata[8..12].copy_from_slice(&0x0100_0080u32.to_le_bytes());
        let img = super::super::tests::image(&code, 0x0100_0000, &rdata, 0x0200_0000);
        assert_eq!(checked_vtable(&img, 0x0100_0010, 11), Some((0x0200_0008, 0x0100_0080)));
        assert_eq!(checked_vtable(&img, 0x0100_0020, 11), None, "another destructor");
        code[0x80] = 0x90;
        let img = super::super::tests::image(&code, 0x0100_0000, &rdata, 0x0200_0000);
        assert_eq!(checked_vtable(&img, 0x0100_0010, 11), None, "not a deleting destructor");
    }
}
