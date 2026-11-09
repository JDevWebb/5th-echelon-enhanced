//! The AI's test of whether a co-op player is in one of its zones, which reads the player's
//! character when there's none (the hooks' `perception_fix` has the whole story).

use super::parse;
use super::Image;

/// The character's lookup and the read: `mov eax,[eax+0x354]` / `mov eax,[eax+0x538]` (the
/// crash, reading 0x538 with no character) / `test eax,eax` / `jz <false>`. The same in both
/// executables (DX11 VA `0xaf2054`, DX9 VA `0xd8a1c4`). The hook is the read, 6 bytes in: the
/// other way to the character (a call, then `jmp` to the read) joins there.
const SIGNATURE: &str = "8B 80 54 03 00 00 8B 80 38 05 00 00 85 C0 0F 84";

/// Where the hook starts in the signature.
const HOOK_AT: usize = 6;

/// The bytes the jump replaces (`mov eax,[eax+0x538]` / `test eax,eax`), which the stub runs.
pub const HOOK_LEN: usize = 8;

/// The test's start, this far before the hook: `push ebp` / `mov ebp,esp` / `push esi` /
/// `mov esi,ecx` / `mov eax,[esi+0x90]`.
const PROLOGUE_BEFORE: usize = 0x5a;
const PROLOGUE: [u8; 12] = [0x55, 0x8B, 0xEC, 0x56, 0x8B, 0xF1, 0x8B, 0x86, 0x90, 0x00, 0x00, 0x00];

/// Its "no" answer, where the `jz` after the hook goes: `xor al,al` / `pop esi` / `pop ebp` /
/// `ret 4` (DX11 `0xaf214d`).
const FALSE_EXIT: [u8; 7] = [0x32, 0xC0, 0x5E, 0x5D, 0xC2, 0x04, 0x00];

/// Where the guard goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    /// The replaced bytes' start (the read).
    pub hook: usize,
    /// Right after them: the `jz`, which the stub's `test` feeds as before.
    pub cont: usize,
    /// The test's "no" answer, where a player without a character goes.
    pub skip: usize,
}

pub fn locate(img: &Image) -> Result<Site, String> {
    let at = img.find(&parse(SIGNATURE)).map_err(|n| format!("the AI zone test's character read matched {n} times"))?;
    let hook = at + HOOK_AT;
    let start = hook.checked_sub(PROLOGUE_BEFORE).ok_or("the AI zone test starts before the code")?;
    if img.bytes(start, PROLOGUE.len()) != Some(&PROLOGUE[..]) {
        return Err(format!("the code at {start:#010x} isn't the AI zone test"));
    }
    let cont = hook + HOOK_LEN;
    // The jz's own target: `0F 84 rel32` right after the replaced bytes.
    let rel = img
        .bytes(cont + 2, 4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or("the AI zone test's jz is past the code")?;
    let skip = (cont + 6).wrapping_add_signed(rel as isize);
    if img.bytes(skip, FALSE_EXIT.len()) != Some(&FALSE_EXIT[..]) {
        return Err(format!("the AI zone test's jz doesn't go to its \"no\" at {skip:#010x}"));
    }
    Ok(Site { hook, cont, skip })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up test: the prologue, filler, the signature with a jz to the false exit, filler,
    /// the false exit. The real distance is 0xf3 from the hook; this one is the same.
    fn test_fn() -> Vec<u8> {
        let mut code = PROLOGUE.to_vec();
        code.resize(PROLOGUE_BEFORE - HOOK_AT, 0x90);
        code.extend(parse(SIGNATURE).into_iter().map(|b| b.unwrap()));
        code.extend(0xe5u32.to_le_bytes()); // jz rel32, as in both executables
        code.resize(PROLOGUE_BEFORE + 0xf3, 0x90);
        code.extend(FALSE_EXIT);
        code
    }

    #[test]
    fn finds_the_read_and_the_false_exit() {
        let mut text = vec![0xCC; 0x30];
        text.extend(test_fn());
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        let hook = 0x40_1000 + 0x30 + PROLOGUE_BEFORE;
        assert_eq!(
            locate(&img),
            Ok(Site {
                hook,
                cont: hook + HOOK_LEN,
                skip: hook + 0xf3
            })
        );
        assert_eq!(&parse(SIGNATURE)[HOOK_AT..HOOK_AT + 2], &[Some(0x8B), Some(0x80)], "the crash's read at the hook");
    }

    #[test]
    fn leaves_another_function_alone() {
        let mut text = vec![0xCC; 0x30];
        let mut other = test_fn();
        other[3] = 0x57; // another prologue
        text.extend(other);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("isn't"));
        let mut text = vec![0xCC; 0x30];
        let mut other = test_fn();
        let n = other.len();
        other[n - 7] = 0xB0; // mov al,..: not the false exit
        text.extend(other);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("doesn't go"));
        let twice = [test_fn(), test_fn()].concat();
        let img = crate::crash_fixes::tests::image(&twice, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).is_err(), "twice is no place to patch");
    }
}
