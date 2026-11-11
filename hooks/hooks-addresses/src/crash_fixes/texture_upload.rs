//! A video texture's upload copying past its mapping (DirectX 11 only; the hooks'
//! `texture_upload_fix` has the whole story).

use super::parse;
use super::Image;

/// After the texture is mapped (DX11 VA `0x16dd3e7`): the two pitches compared; equal, one copy of
/// `min(rows, height) * pitch` bytes (**`call memcpy`** at +31), then unmap and return; else row by
/// row, `min(source pitch, mapped pitch)` bytes a row (**`call memcpy`** at +95), each pointer
/// moved on by its own pitch.
///
/// `mov ecx,[ebp+0x14]` / `mov eax,[ebp-8]` / `cmp ecx,eax` / `jne rows` / `mov eax,[ebp+0x18]` /
/// `mov edx,[ebp+0xc]` / `cmp eax,edx` / `jb` / `mov eax,edx` / `imul eax,ecx` / `mov ecx,[ebp-4]` /
/// `push eax` / `push edi` / `push ecx` / **`call memcpy`** / `mov ecx,[ebp+8]` / `add esp,0xc` /
/// `push 0` / `call unmap` / `pop edi` / `pop esi` / `mov esp,ebp` / `pop ebp` / `ret` /
/// rows: `mov esi,[ebp-4]` / ... / `mov edx,[ebp-4]` / `push edx` / `push edi` / `push esi` /
/// **`call memcpy`** / `add esi,[ebp-8]` / `add edi,[ebp+0x14]`.
const COPIES: &str = "8B 4D 14 8B 45 F8 3B C8 75 2D 8B 45 18 8B 55 0C 3B C2 72 02 8B C2 0F AF C1 8B 4D FC 50 57 51 \
                      E8 ?? ?? ?? ?? 8B 4D 08 83 C4 0C 6A 00 E8 ?? ?? ?? ?? 5F 5E 8B E5 5D C3 \
                      8B 75 FC 89 4D FC 3B C8 72 03 89 45 FC 8B 45 18 53 8B 5D 0C 3B C3 73 02 8B D8 85 DB 74 1B \
                      8D 64 24 00 8B 55 FC 52 57 56 E8 ?? ?? ?? ?? 03 75 F8 03 7D 14";
/// Where the whole-texture copy's `call` is.
const WHOLE_AT: usize = 31;
/// Where the row copy's `call` is.
const ROWS_AT: usize = 95;
/// A `call rel32`'s length.
pub const CALL_LEN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sites {
    /// The `call memcpy` copying the whole texture in one go (the pitches equal).
    pub whole: usize,
    /// The `call memcpy` copying one row.
    pub rows: usize,
    /// The game's `memcpy`, which both call.
    pub memcpy: usize,
}

/// The two copies, in DirectX 11's executable; DirectX 9's uploads its video another way and
/// hasn't this code (`Err`, which the hooks expect there).
pub fn locate(img: &Image) -> Result<Sites, String> {
    let at = img.find(&parse(COPIES)).map_err(|n| format!("the video texture upload matched {n} times"))?;
    let (whole, rows) = (at + WHOLE_AT, at + ROWS_AT);
    let memcpy = img.rel32_target(whole + 1).ok_or("the whole-texture copy's call")?;
    if img.rel32_target(rows + 1) != Some(memcpy) {
        return Err(format!("the video texture upload's two copies ({whole:#010x}, {rows:#010x}) call different functions"));
    }
    Ok(Sites { whole, rows, memcpy })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The copies at `0x40_1010`, both calling a stand-in memcpy at `memcpy`.
    fn text(memcpy: usize, rows_to: usize) -> Vec<u8> {
        let mut text = vec![0xCC; 0x10];
        text.extend(parse(COPIES).into_iter().map(|b| b.unwrap_or(0x00)));
        text.resize(0x200, 0xCC);
        for (at, to) in [(WHOLE_AT, memcpy), (ROWS_AT, rows_to)] {
            let call = 0x10 + at;
            let rel = (to as i64 - (0x40_1000 + call as i64 + 5)) as i32;
            text[call + 1..call + 5].copy_from_slice(&rel.to_le_bytes());
        }
        text
    }

    #[test]
    fn finds_both_copies_and_memcpy() {
        let text = text(0x40_1180, 0x40_1180);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert_eq!(
            locate(&img),
            Ok(Sites {
                whole: 0x40_1010 + WHOLE_AT,
                rows: 0x40_1010 + ROWS_AT,
                memcpy: 0x40_1180
            })
        );
    }

    #[test]
    fn copies_calling_two_functions_are_no_place() {
        let text = text(0x40_1180, 0x40_11a0);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).is_err());
    }

    #[test]
    fn no_upload_is_no_place() {
        let text = vec![0xCC; 0x200];
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).is_err());
    }
}
