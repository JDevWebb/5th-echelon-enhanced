//! The team service's update of its players' teams, which reads a list whose count is garbage
//! at a round's end in a room set below its mode's size (Tea_bag, 21-22 Nov 2026: three crashes
//! at round end in Spies vs Mercs with the seats turned down to 2; the hooks' `teams_fix` has the
//! whole story).

use super::parse;
use super::Image;

/// `NetServiceTeams`' team copy, `thiscall(service, list, a, b, c)`, `ret 0x10` (DX11 `0x7d72f0`,
/// DX9 `0xa74600`): `push ebp` / `mov ebp,esp` / `push ecx` / `push ebx` / `mov ebx,[ebp+8]` (the
/// list) / `mov eax,[ebx+0x64]` (its count) / ... the resize to the count / the loop over it,
/// reading each 12-byte entry (the crashes at `+0x3d731d` and `+0x3d733c`).
const ENTRY: &str = "55 8B EC 51 53 8B 5D 08 8B 43 64 56 57 8B F1 50 8D 4E 10 E8 ?? ?? ?? ?? 33 FF 89 7D FC 39 7B 64 76 ?? 8B CB 8B 46 2C 8B 54 38 0C 03 C7 3B 11";

/// The bytes the jump replaces (`push ebp` / `mov ebp,esp` / `push ecx` / `push ebx`).
pub const HOOK_LEN: usize = 5;
pub const REPLACED: [u8; HOOK_LEN] = [0x55, 0x8B, 0xEC, 0x51, 0x53];
/// Where the list's count is.
pub const COUNT_AT: u8 = 0x64;
/// The stack its arguments take (`ret 0x10`).
pub const ARGS: u16 = 0x10;

/// Where the guard goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    pub hook: usize,
    pub cont: usize,
}

pub fn locate(img: &Image) -> Result<Site, String> {
    let sig = parse(ENTRY);
    match super::find_unique(img.text, &sig) {
        Ok(at) => {
            let hook = img.text_va + at;
            Ok(Site { hook, cont: hook + HOOK_LEN })
        }
        Err(0) => Err("the team service's copy wasn't found".into()),
        Err(n) => Err(format!("the team service's copy matched {n} times")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_copy_once() {
        let f: Vec<u8> = parse(ENTRY).into_iter().map(|b| b.unwrap_or(0x11)).collect();
        let mut text = vec![0xCC; 0x20];
        text.extend(&f);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert_eq!(locate(&img), Ok(Site { hook: 0x40_1020, cont: 0x40_1025 }));
        assert_eq!(img.bytes(0x40_1020, HOOK_LEN), Some(&REPLACED[..]));
        let twice = [f.clone(), f].concat();
        let img = crate::crash_fixes::tests::image(&twice, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("2 times"));
    }
}
