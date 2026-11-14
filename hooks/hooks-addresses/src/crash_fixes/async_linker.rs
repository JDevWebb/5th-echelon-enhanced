//! An async package load whose linker came back empty (the hooks' `async_linker_fix` has the
//! whole story).

use super::parse;
use super::Image;

/// In the async package tick ("LOAD", DX11 VA `0x521eb7`, DX9 `0x7ccec7`): the current entry
/// (`esi`, 0x54 bytes each) / its name or `""` / `call <create linker>` (cdecl, 4 arguments) /
/// `mov [esi],eax` / `call` / **`mov eax,[ecx+edx]` / `add esp,0x14`** (the entry's linker again,
/// and the two calls' arguments) / `cmp dword [eax+0x9c],0x1e240` (the crash, with no linker) /
/// `je`.
const SITE: &str = "8B 35 ?? ?? ?? ?? 6B F6 54 03 35 ?? ?? ?? ?? 39 7E 08 74 05 8B 46 04 EB 05 B8 ?? ?? ?? ?? \
                    57 51 50 57 E8 ?? ?? ?? ?? 57 89 06 E8 ?? ?? ?? ?? 8B 0D ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? \
                    6B C9 54 8B 04 11 83 C4 14 81 B8 9C 00 00 00 40 E2 01 00 0F 84";
/// Where the replaced bytes start in [`SITE`], and how many.
const HOOK_AT: usize = 62;
pub const HOOK_LEN: usize = 6;
/// The bytes replaced: `mov eax,[ecx+edx]` / `add esp,0x14`.
pub const MOVES: [u8; HOOK_LEN] = [0x8B, 0x04, 0x11, 0x83, 0xC4, 0x14];
/// The tick's own "out of time this frame" (DX11 VA `0x5220c9`, DX9 `0x7cd0d9`), this far after
/// the replaced bytes: `mov ebx,1` / `mov [ebp-0x10],ebx` / `jmp <the tick's end>`. Nothing on
/// that way reads the linker.
const STOP_AFTER: usize = 0x1d4;
const STOP: [u8; 9] = [0xBB, 0x01, 0x00, 0x00, 0x00, 0x89, 0x5D, 0xF0, 0xE9];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sites {
    /// The replaced bytes, and right after them (the crashing compare).
    pub site: usize,
    pub cont: usize,
    /// The tick's "out of time".
    pub stop: usize,
}

pub fn locate(img: &Image) -> Result<Sites, String> {
    let site = img.find(&parse(SITE)).map_err(|n| format!("the async package tick's linker read matched {n} times"))? + HOOK_AT;
    let stop = site + STOP_AFTER;
    if img.bytes(stop, STOP.len()) != Some(&STOP[..]) {
        return Err(format!("the async package tick's \"out of time\" isn't at {stop:#010x}"));
    }
    Ok(Sites {
        site,
        cont: site + HOOK_LEN,
        stop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> Vec<u8> {
        let mut text = vec![0xCC; 0x10];
        text.extend(parse(SITE).into_iter().map(|b| b.unwrap_or(0x00)));
        text.resize(0x10 + HOOK_AT + STOP_AFTER, 0x90);
        text.extend(STOP);
        text.extend([0, 0, 0, 0]);
        text
    }

    #[test]
    fn finds_the_read_and_the_out_of_time() {
        let text = text();
        assert_eq!(&text[0x10 + HOOK_AT..0x10 + HOOK_AT + HOOK_LEN], &MOVES);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        let site = 0x40_1010 + HOOK_AT;
        assert_eq!(
            locate(&img),
            Ok(Sites {
                site,
                cont: site + 6,
                stop: site + STOP_AFTER
            })
        );
    }

    #[test]
    fn another_out_of_time_is_no_place() {
        let mut text = text();
        let n = text.len();
        text[n - 13] = 0x02; // mov ebx,2
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("out of time"));
    }
}
