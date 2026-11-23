//! A character's speed update, which reads a movement history that's gone once its player left a
//! co-op match past its size (the gamebox, 23 Nov 2026; the hooks' `movement_fix` has the story).

use super::parse;
use super::Image;

/// The speed update, `thiscall(this)`, `ret` (DX11 `0xd4c330`, DX9 `0xfe1f00`): `push ebp` / `mov
/// ebp,esp` / `sub esp,0xc` / `push esi` / `mov esi,ecx` / `mov ecx,[esi+0x24]` (the owner) /
/// `mov ecx,[ecx+0x12a4]` (its movement history) / ... the call that reads it (the crash at
/// `+0x914fb6`, `[ecx+0x93c]` with ecx 0).
const ENTRY: &str = "55 8B EC 83 EC 0C 56 8B F1 8B 4E 24 8B 89 A4 12 00 00 8D 45 F4 50 E8";

/// The bytes the jump replaces (`push ebp` / `mov ebp,esp` / `sub esp,0xc`).
pub const HOOK_LEN: usize = 6;
pub const REPLACED: [u8; HOOK_LEN] = [0x55, 0x8B, 0xEC, 0x83, 0xEC, 0x0C];
/// Where the owner is on `this`, and the history on the owner.
pub const OWNER_AT: u8 = 0x24;
pub const HISTORY_AT: u32 = 0x12a4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    pub hook: usize,
    pub cont: usize,
}

pub fn locate(img: &Image) -> Result<Site, String> {
    match super::find_unique(img.text, &parse(ENTRY)) {
        Ok(at) => {
            let hook = img.text_va + at;
            Ok(Site { hook, cont: hook + HOOK_LEN })
        }
        Err(0) => Err("the character's speed update wasn't found".into()),
        Err(n) => Err(format!("the character's speed update matched {n} times")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_update_once() {
        let f: Vec<u8> = parse(ENTRY).into_iter().map(|b| b.unwrap_or(0x11)).collect();
        let mut text = vec![0xCC; 0x10];
        text.extend(&f);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert_eq!(locate(&img), Ok(Site { hook: 0x40_1010, cont: 0x40_1016 }));
        assert_eq!(img.bytes(0x40_1010, HOOK_LEN), Some(&REPLACED[..]));
        let twice = [f.clone(), f].concat();
        let img = crate::crash_fixes::tests::image(&twice, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).is_err());
    }
}
