//! The gear screen's choice of what to open with (`UI::SMILoadout`, its activation): where the
//! hooks' `loadout_chooser` looks at it (the hooks' module has the whole story).

use crate::crash_fixes::parse;
use crate::crash_fixes::Image;

/// The activation's call to the choice and the read of what it chose: `mov ecx,esi` (DX9 `mov
/// eax,esi`: the choice takes the screen in EAX there) / `mov byte [esi+0xf75],0` / `call
/// <choice>` / `cmp dword [esi+0x3a4],5` (5: open the campaign's loadout A). DX11 VA `0x1a2805c`
/// in `UI::SMILoadout` slot 6 (`0x1a28010`), the choice `0x1a24b20`; DX9 `0x58784c` in `0x587820`,
/// the choice `0x593d30`.
const SIGNATURE: &str = "8B ?? C6 86 75 0F 00 00 00 E8 ?? ?? ?? ?? 83 BE A4 03 00 00 05";

/// The call's operand, and the compare after it.
const CALL_AT: usize = 9;
const CHECK_AT: usize = 14;

/// The compare (`cmp dword [esi+0x3a4],5`), which the hook replaces and its stub runs.
pub const CHECK: [u8; 7] = [0x83, 0xBE, 0xA4, 0x03, 0x00, 0x00, 0x05];

/// The mode the choice leaves at the screen's `+0x3a4`: 0 is the Characters chooser (Campaign,
/// Spy, Merc), 5 the campaign's loadout A.
pub const MODE: usize = 0x3a4;

/// How the choice starts: DX11 `push ebp` / `mov ebp,esp` / `push -1` / `push <handler>`; DX9
/// `push ebx` / `push esi` / `push edi` / `mov esi,eax`.
const OPENS: [&str; 2] = ["55 8B EC 6A FF 68", "53 56 57 8B F0"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    /// The compare after the call (7 bytes).
    pub check: usize,
    /// The choice itself.
    pub choice: usize,
}

pub fn locate(img: &Image) -> Result<Site, String> {
    let at = img.find(&parse(SIGNATURE)).map_err(|n| format!("the gear screen's choice matched {n} times"))?;
    let choice = img.rel32_target(at + CALL_AT + 1).ok_or("the gear screen's choice is past the code")?;
    let opens = OPENS.iter().any(|o| {
        let want = parse(o);
        img.bytes(choice, want.len())
            .is_some_and(|got| want.iter().zip(got).all(|(w, g)| w.is_none_or(|w| w == *g)))
    });
    if !opens {
        return Err(format!("the code at {choice:#010x} isn't the gear screen's choice"));
    }
    Ok(Site { check: at + CHECK_AT, choice })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_compare_after_the_choice() {
        // The activation's call, then the choice (DX9's start) right after it.
        let mut text = vec![0xCC; 0x10];
        text.extend([0x8B, 0xC6, 0xC6, 0x86, 0x75, 0x0F, 0x00, 0x00, 0x00, 0xE8, 0, 0, 0, 0]);
        text.extend(CHECK);
        let choice_off = text.len();
        text.extend([0x53, 0x56, 0x57, 0x8B, 0xF0, 0xC3]);
        let base = 0x40_1000;
        let rel = (choice_off - (0x10 + CALL_AT + 5)) as u32;
        text[0x10 + CALL_AT + 1..0x10 + CALL_AT + 5].copy_from_slice(&rel.to_le_bytes());
        let img = crate::crash_fixes::tests::image(&text, base, &[], 0x60_0000);
        assert_eq!(
            locate(&img),
            Ok(Site {
                check: base + 0x10 + CHECK_AT,
                choice: base + choice_off
            })
        );
        text[choice_off] = 0x90;
        let img = crate::crash_fixes::tests::image(&text, base, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("isn't"));
    }
}
