//! A package load's walk through its table of expected reads, past the table's end (the hooks'
//! `load_order_fix` has the whole story).

use super::parse;
use super::Image;

/// The walk's first read of the current entry (DX11 VA `0x4504e2`, DX9 `0x6ffdb2`):
/// **`mov ecx,[esi+0x3c]` / `mov edx,[esi+0x34]`** / `mov eax,[ecx]` (the crash) /
/// `mov ebx,[ebp+8]` / `mov edi,[esi+0x38]` / `add eax,edx` / `cmp ebx,[edi]`.
const FIRST: &str = "8B 4E 3C 8B 56 34 8B 01 8B 5D 08 8B 7E 38 03 C2 3B 1F";
/// Its other path's read of the current table (DX11 VA `0x450657`, DX9 `0x6fff27`), a jump
/// target: **`mov ecx,[esi+0x38]` / `mov edx,[ebp+8]`** / `cmp edx,[ecx]` / `je <done>` /
/// `mov edx,[esi+0x2c]` / `mov dword [ebp-4],0`.
const SECOND: &str = "8B 4E 38 8B 55 08 3B 11 0F 84 ?? ?? ?? ?? 8B 56 2C C7 45 FC 00 00 00 00";
/// The bytes each jump replaces.
pub const HOOK_LEN: usize = 6;
/// Where [`SECOND`]'s `je` is: to the function's "fine" end.
const SECOND_JE_AT: usize = 8;
/// That end: `pop edi` / `pop esi` / `mov al,1` / `pop ebx` / `mov esp,ebp` / `pop ebp` / `ret 0x10`.
const DONE: [u8; 11] = [0x5F, 0x5E, 0xB0, 0x01, 0x5B, 0x8B, 0xE5, 0x5D, 0xC2, 0x10, 0x00];
/// The two reads are this far apart in one function.
const APART: usize = 0x175;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sites {
    pub first: usize,
    pub second: usize,
    /// The function's "fine" end, where a walk past its table goes.
    pub done: usize,
}

pub fn locate(img: &Image) -> Result<Sites, String> {
    let first = img.find(&parse(FIRST)).map_err(|n| format!("the load-order walk's first read matched {n} times"))?;
    let second = img.find(&parse(SECOND)).map_err(|n| format!("the load-order walk's second read matched {n} times"))?;
    if second.checked_sub(first) != Some(APART) {
        return Err(format!("the load-order walk's reads ({first:#010x}, {second:#010x}) aren't in one function"));
    }
    let je = second + SECOND_JE_AT;
    let done = img.rel32_target(je + 2).ok_or("the load-order walk's jump")?;
    if img.bytes(done, DONE.len()) != Some(&DONE[..]) {
        return Err(format!("the load-order walk's end {done:#010x} isn't as expected"));
    }
    Ok(Sites { first, second, done })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> Vec<u8> {
        let mut text = vec![0xCC; 0x10];
        text.extend(parse(FIRST).into_iter().map(|b| b.unwrap()));
        text.resize(0x10 + APART, 0x90);
        let second = text.len();
        text.extend(parse(SECOND).into_iter().map(|b| b.unwrap_or(0x00)));
        text.resize(second + 0x40, 0x90);
        let done = text.len();
        text.extend(DONE);
        let je = second + SECOND_JE_AT;
        let rel = (done as i64 - (je as i64 + 6)) as i32;
        text[je + 2..je + 6].copy_from_slice(&rel.to_le_bytes());
        text
    }

    #[test]
    fn finds_both_reads_and_the_end() {
        let text = text();
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert_eq!(
            locate(&img),
            Ok(Sites {
                first: 0x40_1010,
                second: 0x40_1010 + APART,
                done: 0x40_1010 + APART + 0x40
            })
        );
    }

    #[test]
    fn reads_apart_or_another_end_are_no_place() {
        let mut text = text();
        text.insert(0x30, 0x90);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("one function"));
        let mut text = self::tests::text();
        let n = text.len();
        text[n - 9] = 0x00; // mov al,0
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("isn't as expected"));
    }
}
