//! The gear list the menus build for a loadout's preview, which uses each item it looks up by
//! name without checking it was found (the hooks' `gear_list_fix` has the whole story).

use super::parse;
use super::Image;

/// The lookup by name and the read of what it found: `mov eax,[esi]` / `add eax,[ebp+0xc]` (the
/// name) / `lea edx,[ebp-0x1c]` / `push edx` / `push eax` / `call <lookup>` / `mov ecx,[ebp-0x1c]`
/// / `mov edx,[ecx+0x88]` (the crash, reading 0x88 with nothing found) / `push edx` / `mov
/// ecx,edi` / `call <add>` / `mov eax,[ebp-0x28]` (the loop's next name). The same in both
/// executables (DX11 VA `0xf116a3`, DX9 VA `0x11a5373`).
const SIGNATURE: &str = "8B 06 03 45 0C 8D 55 E4 52 50 E8 ?? ?? ?? ?? 8B 4D E4 8B 91 88 00 00 00 52 8B CF E8 ?? ?? ?? ?? 8B 45 D8";

/// Where the hook starts in the signature: `mov ecx,[ebp-0x1c]`.
const HOOK_AT: usize = 15;

/// The bytes the jump replaces (`mov ecx,[ebp-0x1c]` / `mov edx,[ecx+0x88]`), which the stub runs.
pub const HOOK_LEN: usize = 9;

/// From the hook to the loop's next name (`mov eax,[ebp-0x28]`), past the item's adding.
const SKIP_AFTER: usize = 17;

/// The function's start, this far before the hook (DX11 `0xf11540`, DX9 `0x11a5210`): `push
/// ebp` / `mov ebp,esp` / `push -1` / `push <its exception handler>` / `mov eax,fs:[0]` / `push
/// eax` / `sub esp,0x54`. The handler differs between the executables.
const PROLOGUE_BEFORE: usize = 0x172;
const PROLOGUE: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 54";

/// Where the guard goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    /// The replaced bytes' start.
    pub hook: usize,
    /// Right after them (`push edx`), where an item that was found goes on.
    pub cont: usize,
    /// The loop's next name, where a name with nothing found goes.
    pub skip: usize,
}

pub fn locate(img: &Image) -> Result<Site, String> {
    let at = img.find(&parse(SIGNATURE)).map_err(|n| format!("the gear list's lookup matched {n} times"))?;
    let hook = at + HOOK_AT;
    let start = hook.checked_sub(PROLOGUE_BEFORE).ok_or("the gear list's function starts before the code")?;
    let prologue = parse(PROLOGUE);
    let found = img.bytes(start, prologue.len()).ok_or("the gear list's function is past the code")?;
    if !prologue.iter().zip(found).all(|(want, got)| want.is_none_or(|w| w == *got)) {
        return Err(format!("the code at {start:#010x} isn't the gear list's function"));
    }
    Ok(Site {
        hook,
        cont: hook + HOOK_LEN,
        skip: hook + SKIP_AFTER,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up function: the prologue, filler, the signature, filler.
    fn test_fn() -> Vec<u8> {
        let mut code: Vec<u8> = parse(PROLOGUE).into_iter().map(|b| b.unwrap_or(0x11)).collect();
        code.resize(PROLOGUE_BEFORE - HOOK_AT, 0x90);
        code.extend(parse(SIGNATURE).into_iter().map(|b| b.unwrap_or(0x22)));
        code.resize(code.len() + 0x20, 0x90);
        code
    }

    #[test]
    fn finds_the_read_and_the_next_name() {
        let mut text = vec![0xCC; 0x30];
        text.extend(test_fn());
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        let hook = 0x40_1000 + 0x30 + PROLOGUE_BEFORE;
        assert_eq!(
            locate(&img),
            Ok(Site {
                hook,
                cont: hook + HOOK_LEN,
                skip: hook + SKIP_AFTER
            })
        );
        let sig = parse(SIGNATURE);
        assert_eq!(&sig[HOOK_AT..HOOK_AT + 3], &[Some(0x8B), Some(0x4D), Some(0xE4)], "mov ecx,[ebp-0x1c] at the hook");
        assert_eq!(&sig[HOOK_AT + 3..HOOK_AT + 5], &[Some(0x8B), Some(0x91)], "the crash's read next");
        assert_eq!(
            &sig[HOOK_AT + SKIP_AFTER..HOOK_AT + SKIP_AFTER + 3],
            &[Some(0x8B), Some(0x45), Some(0xD8)],
            "the next name at the skip"
        );
    }

    #[test]
    fn leaves_another_function_alone() {
        let mut text = vec![0xCC; 0x30];
        let mut other = test_fn();
        other[1] = 0x8C; // not mov ebp,esp
        text.extend(other);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("isn't"));
        let twice = [test_fn(), test_fn()].concat();
        let img = crate::crash_fixes::tests::image(&twice, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).is_err(), "twice is no place to patch");
    }
}
