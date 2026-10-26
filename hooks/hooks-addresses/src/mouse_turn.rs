//! Where the beta "mouse turning the same at any frame rate" goes (the hooks'
//! `mouse_turn_fix`): in the look routine's mouse branch, just after the game's clamp of the
//! mouse's turn (`mouse_fix` widens that clamp), where the clamped X has been stored and Y is
//! about to be (DX11 `0xd8e315`, DX9 `0x1023e15`). Controller input takes the routine's other
//! branch and never comes here. Notes: blacklist-re `notes/mouse-turn-fps.md`.

use crate::crash_fixes::parse;
use crate::crash_fixes::Image;

/// The site and what follows, the same in both executables: `fld dword [esi+0x4b4]`,
/// `movss [edi],xmm0` (the 10 bytes replaced), then the history fields' stores. Nothing before
/// it: Fusion Fix NOPs the clamp's `movaps` there.
const SIGNATURE: &str = "D9 86 B4 04 00 00 F3 0F 11 07 D9 9E BC 04 00 00 F3 0F 11 8E B4 04 00 00 D9 86 B8 04 00 00 F3 0F 11 86 B8 04 00 00";

/// The bytes the jump replaces (and the stub replays).
pub const ORIGINAL: [u8; 10] = [0xD9, 0x86, 0xB4, 0x04, 0x00, 0x00, 0xF3, 0x0F, 0x11, 0x07];

/// Where the stub jumps from, and back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    pub patch: usize,
    pub resume: usize,
}

/// The site in `img`, if this build has it exactly once.
pub fn locate(img: &Image) -> Result<Site, String> {
    let patch = img.find(&parse(SIGNATURE)).map_err(|n| format!("the look routine's mouse store matched {n} times"))?;
    if img.bytes(patch, ORIGINAL.len()) != Some(&ORIGINAL[..]) {
        return Err("the look routine's mouse store isn't as expected".into());
    }
    Ok(Site {
        patch,
        resume: patch + ORIGINAL.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_store_after_the_clamp() {
        let mut text = vec![0xCC; 0x40];
        text.extend(parse(SIGNATURE).into_iter().map(|b| b.unwrap()));
        text.extend([0xC3; 8]);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert_eq!(
            locate(&img),
            Ok(Site {
                patch: 0x40_1040,
                resume: 0x40_104a
            })
        );
        let twice = [text.clone(), text].concat();
        let img = crate::crash_fixes::tests::image(&twice, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).is_err(), "twice is no place to patch");
    }
}
