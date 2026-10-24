//! Pure helpers for patching code that needs more room than its site has: a code cave is a run of
//! `0xCC` padding between functions, reached by a `call rel32` from the patched site. No Windows
//! types in here, so the unit tests also run on a Linux host.

/// Length of the code cave the privacy-aware flag setter needs.
pub const CAVE_LEN: usize = 16;

/// The cave's code (see `migrate_lobby`): the flag `dl` is `(kind == 1)` from the `cmp` before the
/// call, or "not private" (`al` is the privacy byte).
///   `sete dl` / `xor al,1` / `or dl,al` / `xor al,1` (give `al` back) / `lea ecx,[ebp-0x1e0]` / `ret`
pub const CAVE: [u8; CAVE_LEN] = [
    0x0F, 0x94, 0xC2, // sete dl
    0x34, 0x01, // xor al,1
    0x08, 0xC2, // or dl,al
    0x34, 0x01, // xor al,1
    0x8D, 0x8D, 0x20, 0xFE, 0xFF, 0xFF, // lea ecx,[ebp-0x1e0]
    0xC3, // ret
];

/// The 10 bytes that replace `sete dl` / `push ebx` / `lea ecx,[ebp-0x1e0]` at `site`:
/// `call cave` / `push ebx` / 4x `nop`. Both are offsets into the same image.
pub fn patch_call(site: usize, cave: usize) -> [u8; 10] {
    let rel = (cave as i64 - (site as i64 + 5)) as i32;
    let r = rel.to_le_bytes();
    [0xE8, r[0], r[1], r[2], r[3], 0x53, 0x90, 0x90, 0x90, 0x90]
}

/// Where the first run of at least `len` bytes of `0xCC` starts in `code`.
pub fn find_cave(code: &[u8], len: usize) -> Option<usize> {
    let mut run = 0;
    for (i, b) in code.iter().enumerate() {
        if *b == 0xCC {
            run += 1;
            if run == len {
                return Some(i + 1 - len);
            }
        } else {
            run = 0;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cave_ends_in_ret_and_fills_its_length() {
        assert_eq!(CAVE.len(), CAVE_LEN);
        assert_eq!(CAVE[CAVE_LEN - 1], 0xC3);
        assert_eq!(&CAVE[..3], &[0x0F, 0x94, 0xC2], "sete dl first, off the cmp's flags");
        assert_eq!(&CAVE[3..5], &[0x34, 0x01], "al = not private");
        assert_eq!(&CAVE[5..7], &[0x08, 0xC2], "dl |= al");
        assert_eq!(&CAVE[7..9], &[0x34, 0x01], "al restored");
        assert_eq!(&CAVE[9..15], &[0x8D, 0x8D, 0x20, 0xFE, 0xFF, 0xFF], "lea ecx,[ebp-0x1e0]");
    }

    #[test]
    fn the_call_is_relative_to_the_end_of_the_call() {
        let p = patch_call(0x1000, 0x2000);
        assert_eq!(p[0], 0xE8);
        assert_eq!(i32::from_le_bytes([p[1], p[2], p[3], p[4]]), 0x2000 - 0x1005);
        assert_eq!(&p[5..], &[0x53, 0x90, 0x90, 0x90, 0x90]);
    }

    #[test]
    fn the_call_reaches_a_cave_before_the_site() {
        let p = patch_call(0x4000, 0x1000);
        assert_eq!(i32::from_le_bytes([p[1], p[2], p[3], p[4]]), 0x1000 - 0x4005);
        assert!(i32::from_le_bytes([p[1], p[2], p[3], p[4]]) < 0);
    }

    #[test]
    fn the_first_long_enough_run_of_padding_is_taken() {
        let mut code = vec![0x90; 8];
        code.extend([0xCC; 15]); // too short
        code.push(0x90);
        code.extend([0xCC; 16]); // first fit, at 24
        code.extend([0xCC; 20]);
        assert_eq!(find_cave(&code, 16), Some(24));
    }

    #[test]
    fn no_cave_without_a_long_enough_run() {
        let mut code = vec![0xCC; 15];
        code.push(0x00);
        code.extend([0xCC; 15]);
        assert_eq!(find_cave(&code, 16), None);
        assert_eq!(find_cave(&[], 16), None);
    }
}
