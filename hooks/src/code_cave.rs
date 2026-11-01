//! Pure helpers for patching code that needs more room than its site has: a code cave, a few bytes
//! of executable memory allocated for it (`migrate_lobby`), reached by a `call rel32` from the
//! patched site. No Windows types in here, so the unit tests also run on a Linux host.

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
/// `call cave` / `push ebx` / 4x `nop`. Both are addresses (32-bit: any reaches any).
pub fn patch_call(site: usize, cave: usize) -> [u8; 10] {
    let rel = (cave as i64 - (site as i64 + 5)) as i32;
    let r = rel.to_le_bytes();
    [0xE8, r[0], r[1], r[2], r[3], 0x53, 0x90, 0x90, 0x90, 0x90]
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
}
