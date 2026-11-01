//! A player's nav-disruption state asking about an item the player hasn't got, and the lookup it
//! asks (the hooks' `disruption_fix` has the whole story).

use super::parse;
use super::Image;

/// The lookup's start (DX11 VA `0xe72c70`, DX9 `0x1108250`), thiscall, no stack arguments:
/// `push esi` / `mov esi,ecx` / `mov ecx,[esi+0x310]` (the crash) / `cmp [esi+0x314],ecx` /
/// `jne` / `mov eax,[esi+0x318]` / `test eax,eax` / `jne`.
const LOOKUP: &str = "56 8B F1 8B 8E 10 03 00 00 39 8E 14 03 00 00 75 0A 8B 86 18 03 00 00 85 C0 75";
/// The bytes the lookup's jump replaces: its first three instructions.
pub const LOOKUP_LEN: usize = 9;

/// In `UGeoNavDisruptionState`'s tick (DX11 VA `0x150c248`, DX9 `0x179e3d8`): `mov eax,[esi+0x48]` /
/// `mov ecx,[eax+0x93c]` / `call` (the player's item, `[x+0x21c]`, or 0) / **`mov edi,eax` /
/// `mov ecx,edi` / `call <lookup>`** / `mov edx,[eax]` / `mov ebx,[eax+8]` / `push 0` /
/// `add edi,0x9c`.
const TICK: &str = "8B 46 48 8B 88 3C 09 00 00 E8 ?? ?? ?? ?? 8B F8 8B CF E8 ?? ?? ?? ?? 8B 10 8B 58 08 6A 00 81 C7 9C 00 00 00";
/// Where the replaced bytes start in [`TICK`], and how many.
const TICK_HOOK_AT: usize = 14;
pub const TICK_LEN: usize = 9;
/// The tick's end, this far after the replaced bytes: `pop edi` / `pop esi` / `pop ebx` /
/// `pop ebp` / `ret 4`.
const TICK_END_AFTER: usize = 0x28;
const TICK_END: [u8; 7] = [0x5F, 0x5E, 0x5B, 0x5D, 0xC2, 0x04, 0x00];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sites {
    /// The lookup's start (its replaced bytes), and right after them.
    pub lookup: usize,
    pub lookup_cont: usize,
    /// The tick's replaced bytes (`mov edi,eax` ...), right after them, and its end.
    pub tick: usize,
    pub tick_cont: usize,
    pub tick_end: usize,
}

pub fn locate(img: &Image) -> Result<Sites, String> {
    let lookup = img.find(&parse(LOOKUP)).map_err(|n| format!("the item lookup matched {n} times"))?;
    let tick = img.find(&parse(TICK)).map_err(|n| format!("the nav-disruption tick matched {n} times"))? + TICK_HOOK_AT;
    // The tick's call (`call` at +4 of the replaced bytes) is to that lookup.
    let called = img.rel32_target(tick + 5).ok_or("the nav-disruption tick's call")?;
    if called != lookup {
        return Err(format!("the nav-disruption tick calls {called:#010x}, not the item lookup {lookup:#010x}"));
    }
    let tick_end = tick + TICK_END_AFTER;
    if img.bytes(tick_end, TICK_END.len()) != Some(&TICK_END[..]) {
        return Err(format!("the nav-disruption tick doesn't end at {tick_end:#010x}"));
    }
    Ok(Sites {
        lookup,
        lookup_cont: lookup + LOOKUP_LEN,
        tick,
        tick_cont: tick + TICK_LEN,
        tick_end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lookup at `0x40_1000 + 0x20`, the tick after it, calling it.
    fn text() -> Vec<u8> {
        let mut text = vec![0xCC; 0x20];
        text.extend(parse(LOOKUP).into_iter().map(|b| b.unwrap_or(0x90)));
        text.resize(0x100, 0xCC);
        let tick_at = text.len();
        text.extend(parse(TICK).into_iter().map(|b| b.unwrap_or(0x90)));
        // The call's rel32 to the lookup.
        let call = tick_at + TICK_HOOK_AT + 4;
        let rel = (0x20i64 - (call as i64 + 5)) as i32;
        text[call + 1..call + 5].copy_from_slice(&rel.to_le_bytes());
        text.resize(tick_at + TICK_HOOK_AT + TICK_END_AFTER, 0x90);
        text.extend(TICK_END);
        text
    }

    #[test]
    fn finds_both_and_the_call_between_them() {
        let text = text();
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        let s = locate(&img).unwrap();
        assert_eq!((s.lookup, s.lookup_cont), (0x40_1020, 0x40_1029));
        assert_eq!(s.tick, 0x40_1100 + TICK_HOOK_AT);
        assert_eq!((s.tick_cont, s.tick_end), (s.tick + TICK_LEN, s.tick + TICK_END_AFTER));
        assert_eq!(&parse(TICK)[TICK_HOOK_AT..TICK_HOOK_AT + 4], &[Some(0x8B), Some(0xF8), Some(0x8B), Some(0xCF)]);
    }

    #[test]
    fn a_call_elsewhere_or_another_end_is_no_place() {
        let mut text = text();
        let call = 0x100 + TICK_HOOK_AT + 4;
        text[call + 1] ^= 1;
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("not the item lookup"));
        let mut text = self::tests::text();
        let n = text.len();
        text[n - 3] = 0x90;
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("doesn't end"));
    }
}
