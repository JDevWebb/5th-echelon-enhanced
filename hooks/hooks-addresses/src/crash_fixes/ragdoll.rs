//! The ragdoll's body-parts merge, which reads a parts list freed under it in co-op with more
//! than two players (the hooks' `ragdoll_fix` has the whole story).

use super::parse;
use super::Image;

/// The function's start: `push ebp` / `mov ebp,esp` / `push -1` / `push <handler>` / the SEH
/// frame / `sub esp,<n>` / the stack cookie / `push ebx` / `push esi` / `push edi` / ... The same in
/// both executables (DX11 VA `0x9bd510`, DX9 VA `0xc56df0`).
const ENTRY: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 81 EC ?? ?? 00 00 A1 ?? ?? ?? ?? 33 C5 89 45 F0 53 56 57 50 8D 45 F4 64 A3 00 00 00 00";

/// Its first statement, soon after the entry: `or dword [edi+0x1c],0x80004`.
const FLAGS: &str = "81 4F 1C 04 00 08 00";
/// How far after the entry signature's end the flags may come.
const FLAGS_WITHIN: usize = 0x30;

/// The crash's loop, this far in (both executables): for each of a part's 10 slots, `mov
/// eax,[esi]` / `test eax,eax` / `jz next` / `mov ecx,[eax+0x40]` (the crash at `+0x5bdb36` with a
/// freed slot) / `add ecx,0x34` / `push ecx` / `lea ecx,[ebp-..]` / `call AddUniqueItem` (the crash
/// at `+0x5b9b03` reading `[ecx]`) / `add esi,4` / `dec ebx` / `jnz loop`.
const LOOP: &str = "8B 06 85 C0 74 0F 8B 48 40 83 C1 34 51 8D 4D ?? E8 ?? ?? ?? ?? 83 C6 04 4B 75 E5";
const LOOP_AT: usize = 0x620;

/// The bytes the jump replaces (`push ebp` / `mov ebp,esp` / `push -1`), which the stub runs.
pub const HOOK_LEN: usize = 5;
/// Those bytes, as the stub runs them.
pub const REPLACED: [u8; HOOK_LEN] = [0x55, 0x8B, 0xEC, 0x6A, 0xFF];

/// Where the guard goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    /// The function's start, where the jump goes.
    pub hook: usize,
    /// Right after the replaced bytes, where the stub goes back.
    pub cont: usize,
}

pub fn locate(img: &Image) -> Result<Site, String> {
    let entry = parse(ENTRY);
    let flags = parse(FLAGS);
    let loop_sig = parse(LOOP);
    let matches = |at: usize, sig: &[Option<u8>]| img.bytes(at, sig.len()).is_some_and(|b| b.iter().zip(sig).all(|(b, s)| s.is_none_or(|s| s == *b)));
    // Every function with this entry is a candidate; the one with the flags and the loop is it.
    let mut found = Vec::new();
    let mut from = img.text_va;
    while let Some(at) = next(img, &entry, from) {
        let end = at + entry.len();
        if (end..end + FLAGS_WITHIN).any(|f| matches(f, &flags)) && matches(at + LOOP_AT, &loop_sig) {
            found.push(at);
        }
        from = at + 1;
    }
    match found[..] {
        [hook] => Ok(Site { hook, cont: hook + HOOK_LEN }),
        [] => Err("the ragdoll's parts merge wasn't found".into()),
        _ => Err(format!("the ragdoll's parts merge matched {} times", found.len())),
    }
}

/// The next place at or after `from` where `sig` matches.
fn next(img: &Image, sig: &[Option<u8>], from: usize) -> Option<usize> {
    let start = from.checked_sub(img.text_va)?;
    let text = img.text.get(start..)?;
    text.windows(sig.len())
        .position(|w| w.iter().zip(sig).all(|(b, s)| s.is_none_or(|s| s == *b)))
        .map(|i| from + i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up function: the entry, filler, the flags, filler to the loop, the loop.
    fn test_fn() -> Vec<u8> {
        let mut code: Vec<u8> = parse(ENTRY).into_iter().map(|b| b.unwrap_or(0x11)).collect();
        code.extend([0x90; 0x10]);
        code.extend(parse(FLAGS).into_iter().map(|b| b.unwrap()));
        code.resize(LOOP_AT, 0x90);
        code.extend(parse(LOOP).into_iter().map(|b| b.unwrap_or(0x22)));
        code.extend([0xC2, 0x10, 0x00]);
        code
    }

    #[test]
    fn finds_the_merge_by_its_entry_flags_and_loop() {
        let mut text = vec![0xCC; 0x30];
        text.extend(test_fn());
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        let hook = 0x40_1000 + 0x30;
        assert_eq!(locate(&img), Ok(Site { hook, cont: hook + HOOK_LEN }));
        assert_eq!(img.bytes(hook, HOOK_LEN), Some(&REPLACED[..]), "the stub runs the bytes it replaces");
    }

    #[test]
    fn leaves_other_functions_alone() {
        // Another function with the same entry but not the flags.
        let mut other = test_fn();
        let flags_at = parse(ENTRY).len() + 0x10;
        other[flags_at + 3] = 0x05;
        let mut text = vec![0xCC; 0x30];
        text.extend(&other);
        text.extend(test_fn());
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert_eq!(locate(&img).map(|s| s.hook), Ok(0x40_1000 + 0x30 + other.len()), "only the one with the flags");
        // The loop elsewhere: not it.
        let mut moved = test_fn();
        moved[LOOP_AT] = 0x90;
        let img = crate::crash_fixes::tests::image(&moved, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).is_err());
        let twice = [test_fn(), test_fn()].concat();
        let img = crate::crash_fixes::tests::image(&twice, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("2 times"), "twice is no place to patch");
    }
}
