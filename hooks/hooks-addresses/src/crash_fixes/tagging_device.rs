//! The mercs' tagging device's reset, which uses a part of the device that's already gone (the
//! hooks' `tagging_device_fix` has the whole story).

use super::parse;
use super::Image;

/// The reset's use of the part: `mov ecx,[esi+0x268]` / `lea edx,[ebp-0x18]` /
/// `fstp dword [esi+0x1a4]` / `mov eax,[ecx]` (the crash) / `mov eax,[eax+0xfc]` / `push edx` /
/// `call eax`. The same in both executables (DX11 VA `0xaa88bf`, DX9 VA `0xd4128f`).
const SIGNATURE: &str = "8B 8E 68 02 00 00 8D 55 E8 D9 9E A4 01 00 00 8B 01 8B 80 FC 00 00 00 52 FF D0";

/// The bytes the jump replaces (the three instructions before the crash), which the stub runs.
pub const HOOK_LEN: usize = 15;

/// The reset's start, this far before the site: `push ebp` / `mov ebp,esp` / `sub esp,0x24` /
/// `push esi` / `mov esi,ecx` / `test dword [esi+0x4b0],0x80000000`.
const PROLOGUE_BEFORE: usize = 0xaf;
const PROLOGUE: [u8; 19] = [
    0x55, 0x8B, 0xEC, 0x83, 0xEC, 0x24, 0x56, 0x8B, 0xF1, 0xF7, 0x86, 0xB0, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
];

/// Its end, this far after the site: `pop esi` / `mov esp,ebp` / `pop ebp` / `ret`.
const EPILOGUE_AFTER: usize = 0x8e;
const EPILOGUE: [u8; 5] = [0x5E, 0x8B, 0xE5, 0x5D, 0xC3];

/// Where the guard goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    /// The replaced bytes' start.
    pub hook: usize,
    /// Right after them: the original path.
    pub cont: usize,
    /// The reset's end, where a device without the part goes.
    pub skip: usize,
}

pub fn locate(img: &Image) -> Result<Site, String> {
    let hook = img.find(&parse(SIGNATURE)).map_err(|n| format!("the tagging device's reset matched {n} times"))?;
    let start = hook.checked_sub(PROLOGUE_BEFORE).ok_or("the tagging device's reset starts before the code")?;
    if img.bytes(start, PROLOGUE.len()) != Some(&PROLOGUE[..]) {
        return Err(format!("the code at {start:#010x} isn't the tagging device's reset"));
    }
    let skip = hook + EPILOGUE_AFTER;
    if img.bytes(skip, EPILOGUE.len()) != Some(&EPILOGUE[..]) {
        return Err(format!("the tagging device's reset doesn't end at {skip:#010x}"));
    }
    Ok(Site {
        hook,
        cont: hook + HOOK_LEN,
        skip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up reset: the prologue, filler, the site, filler, the epilogue.
    fn reset() -> Vec<u8> {
        let mut code = PROLOGUE.to_vec();
        code.resize(PROLOGUE_BEFORE, 0x90);
        code.extend(parse(SIGNATURE).into_iter().map(|b| b.unwrap()));
        code.resize(PROLOGUE_BEFORE + EPILOGUE_AFTER, 0x90);
        code.extend(EPILOGUE);
        code
    }

    #[test]
    fn finds_the_site_and_both_ends() {
        let mut text = vec![0xCC; 0x30];
        text.extend(reset());
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        let hook = 0x40_1000 + 0x30 + PROLOGUE_BEFORE;
        assert_eq!(
            locate(&img),
            Ok(Site {
                hook,
                cont: hook + HOOK_LEN,
                skip: hook + EPILOGUE_AFTER
            })
        );
        assert_eq!(
            &parse(SIGNATURE)[HOOK_LEN..HOOK_LEN + 2],
            &[Some(0x8B), Some(0x01)],
            "the crash right after the replaced bytes"
        );
    }

    #[test]
    fn leaves_another_function_alone() {
        let mut text = vec![0xCC; 0x30];
        let mut other = reset();
        other[3] = 0x81; // another prologue
        text.extend(other);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("isn't"));
        let mut text = vec![0xCC; 0x30];
        let mut other = reset();
        let n = other.len();
        other[n - 1] = 0x90; // no ret where expected
        text.extend(other);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("doesn't end"));
        let twice = [reset(), reset()].concat();
        let img = crate::crash_fixes::tests::image(&twice, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).is_err(), "twice is no place to patch");
    }
}
