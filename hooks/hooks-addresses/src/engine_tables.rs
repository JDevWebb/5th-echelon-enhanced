//! The engine's own tables that the hooks read objects through (`round_state`, `pawns`,
//! `dataversion`): the object hash and the name table, by signature, in both builds.
//!
//! - The object hash: `StaticFindObject`'s bucket lookup, `shr eax,0x13` / `imul eax,eax,0x141`
//!   / `add eax,edx` / `and eax,0xffff` / `mov eax,[eax*4+hash]` (DX11 `0x004d74ad`, hash
//!   `0x0333cde8`; DX9 `0x007846cd`, `0x0326edb0`). The function is the same code in both.
//! - The name table: the data version's sum over its two ranges of names (DX11 `0x008290f6`, DX9
//!   `0x00ac4476`): `cmp edi,[native end]` / `jb` / `mov ebx,[startup start]` / `cmp
//!   ebx,[startup end]` / `jae` / `lea esp,[esp]` / `mov edx,[names]` / `mov edi,[edx+ebx*4]` /
//!   `test edi,edi` / `je` / `lea eax,[edi+8]`.
//! - The names' count, and the game version count: the end of the loop that marks every name
//!   (DX11 `0x005aaaa0`, DX9 `0x0084f130`), `mov eax,[names]` / … / `mov edx,[count]` / `inc ecx`
//!   / `cmp ecx,edx` / `jl` / `mov [startup end],edx` / `mov [game version count],edx`. Its
//!   names and startup end must be the walk's, and the count the word after the names (the
//!   table is a `TArray`).
//!
//! In each build, every address here sits at the same distance from the others and is read as
//! often (2026-11-03: names 117 times, the count 49, the hash 8, the ranges 6, 5 and 7).

use crate::crash_fixes::parse;
use crate::crash_fixes::Image;

/// The object hash's lookup; the hash's address follows it.
pub const HASH_LOOKUP: &str = "C1 E8 13 69 C0 41 01 00 00 03 C2 25 FF FF 00 00 8B 04 85";

/// The data version's walk over the names, with where its four globals are read.
const NAME_RANGES: &str = "3B 3D ?? ?? ?? ?? 72 ?? 8B 1D ?? ?? ?? ?? 3B 1D ?? ?? ?? ?? 73 ?? 8D 64 24 00 \
    8B 15 ?? ?? ?? ?? 8B 3C 9A 85 FF 74 ?? 8D 47 08";
const NAME_RANGES_NATIVE_END: usize = 2;
const NAME_RANGES_STARTUP_START: usize = 10;
const NAME_RANGES_STARTUP_END: usize = 16;
const NAME_RANGES_NAMES: usize = 28;

/// The end of the loop that marks every name, with where it reads the names and their count,
/// and writes the startup end and the game version count.
const NAME_MARKS: &str = "A1 ?? ?? ?? ?? 8B 04 88 85 C0 74 ?? 83 08 01 8B 15 ?? ?? ?? ?? 41 3B CA 7C ?? \
    89 15 ?? ?? ?? ?? 89 15 ?? ?? ?? ?? 8B E5 5D C3";
const NAME_MARKS_NAMES: usize = 1;
const NAME_MARKS_COUNT: usize = 17;
const NAME_MARKS_STARTUP_END: usize = 28;
const NAME_MARKS_VERSION_COUNT: usize = 34;

/// Where the tables are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tables {
    /// The lookup's code, and the hash's 0x10000 buckets.
    pub hash_lookup: usize,
    pub object_hash: usize,
    /// The name table: its entries' array and their count.
    pub names: usize,
    pub names_count: usize,
    /// The ranges of names the data version counts: [0, native end) and [startup start,
    /// startup end), each a global holding an index.
    pub native_end: usize,
    pub startup_start: usize,
    pub startup_end: usize,
    /// How many names the game version counts (a diagnostic of the data version).
    pub game_version_count: usize,
}

/// The tables in `img`, if both pieces of code are there once and read globals of the image.
pub fn locate(img: &Image) -> Result<Tables, String> {
    let hash_lookup = img.find(&parse(HASH_LOOKUP)).map_err(|n| format!("the object hash's lookup matched {n} times"))?;
    let object_hash = img.u32(hash_lookup + parse(HASH_LOOKUP).len()).ok_or("the object hash's address couldn't be read")?;
    let ranges = img.find(&parse(NAME_RANGES)).map_err(|n| format!("the name table's walk matched {n} times"))?;
    let read = |at: usize| img.u32(ranges + at).ok_or_else(|| "the name table's walk couldn't be read".to_string());
    let names = read(NAME_RANGES_NAMES)?;
    let marks = img.find(&parse(NAME_MARKS)).map_err(|n| format!("the names' marking matched {n} times"))?;
    let mark = |at: usize| img.u32(marks + at).ok_or_else(|| "the names' marking couldn't be read".to_string());
    let tables = Tables {
        hash_lookup,
        object_hash,
        names,
        names_count: mark(NAME_MARKS_COUNT)?,
        native_end: read(NAME_RANGES_NATIVE_END)?,
        startup_start: read(NAME_RANGES_STARTUP_START)?,
        startup_end: read(NAME_RANGES_STARTUP_END)?,
        game_version_count: mark(NAME_MARKS_VERSION_COUNT)?,
    };
    if mark(NAME_MARKS_NAMES)? != names || mark(NAME_MARKS_STARTUP_END)? != tables.startup_end || tables.names_count != names + 4 {
        return Err("the names' marking and walk read different tables".into());
    }
    let globals = [
        tables.object_hash,
        tables.names,
        tables.native_end,
        tables.startup_start,
        tables.startup_end,
        tables.game_version_count,
    ];
    if globals.iter().any(|g| !img.image.contains(g)) {
        return Err("a table's address isn't in the game's image".into());
    }
    Ok(tables)
}

/// The lookup's bytes as the game has them where [`Tables::hash_lookup`] is (with the hash's
/// address), to check before reading through them.
pub fn hash_lookup_bytes(tables: &Tables) -> Vec<u8> {
    let mut bytes: Vec<u8> = parse(HASH_LOOKUP).into_iter().map(|b| b.unwrap_or_default()).collect();
    bytes.extend((tables.object_hash as u32).to_le_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: usize = 0x40_1000;

    fn piece(signature: &str, fill: u8, globals: &[(usize, u32)]) -> Vec<u8> {
        let mut bytes: Vec<u8> = parse(signature).into_iter().map(|b| b.unwrap_or(fill)).collect();
        for (at, value) in globals {
            bytes[*at..*at + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    /// The DX11 build's addresses, as the code reads them.
    fn text() -> Vec<u8> {
        let mut text = vec![0xCC; 0x40];
        text.extend(piece(HASH_LOOKUP, 0, &[]));
        text.extend(0x0333_cde8u32.to_le_bytes());
        text.extend([0xCC; 0x20]);
        let ranges = [
            (NAME_RANGES_NATIVE_END, 0x0333_cd54),
            (NAME_RANGES_STARTUP_START, 0x0333_cd58),
            (NAME_RANGES_STARTUP_END, 0x0333_cd5c),
            (NAME_RANGES_NAMES, 0x0333_cd60),
        ];
        text.extend(piece(NAME_RANGES, 0x11, &ranges));
        text.extend([0xCC; 0x10]);
        let marks = [
            (NAME_MARKS_NAMES, 0x0333_cd60),
            (NAME_MARKS_COUNT, 0x0333_cd64),
            (NAME_MARKS_STARTUP_END, 0x0333_cd5c),
            (NAME_MARKS_VERSION_COUNT, 0x0330_c478),
        ];
        text.extend(piece(NAME_MARKS, 0x12, &marks));
        text
    }

    #[test]
    fn the_signatures_parse() {
        assert_eq!(parse(NAME_RANGES).len(), NAME_RANGES.split_whitespace().count());
        assert_eq!(parse(NAME_MARKS).len(), NAME_MARKS.split_whitespace().count());
        for at in [NAME_RANGES_NATIVE_END, NAME_RANGES_STARTUP_START, NAME_RANGES_STARTUP_END, NAME_RANGES_NAMES] {
            assert!(parse(NAME_RANGES)[at..at + 4].iter().all(Option::is_none), "{at}");
        }
        for at in [NAME_MARKS_NAMES, NAME_MARKS_COUNT, NAME_MARKS_STARTUP_END, NAME_MARKS_VERSION_COUNT] {
            assert!(parse(NAME_MARKS)[at..at + 4].iter().all(Option::is_none), "{at}");
        }
        assert!(parse(HASH_LOOKUP).iter().all(Option::is_some));
    }

    #[test]
    fn finds_the_tables() {
        let text = text();
        let img = crate::crash_fixes::tests::image(&text, BASE, &[], 0x400_0000);
        let t = locate(&img).unwrap();
        assert_eq!(t.hash_lookup, BASE + 0x40);
        assert_eq!(
            [t.object_hash, t.names, t.names_count, t.native_end, t.startup_start, t.startup_end, t.game_version_count],
            [0x0333_cde8, 0x0333_cd60, 0x0333_cd64, 0x0333_cd54, 0x0333_cd58, 0x0333_cd5c, 0x0330_c478]
        );
        let bytes = hash_lookup_bytes(&t);
        assert_eq!(&text[0x40..0x40 + bytes.len()], &bytes[..], "the bytes checked before a read are the game's");
    }

    #[test]
    fn missing_twice_or_outside_is_no_place() {
        let mut text = text();
        text[0x40] ^= 0xFF;
        let img = crate::crash_fixes::tests::image(&text, BASE, &[], 0x400_0000);
        assert!(locate(&img).unwrap_err().contains("0 times"));
        let mut text = self::text();
        text.extend(piece(NAME_RANGES, 0x22, &[]));
        let img = crate::crash_fixes::tests::image(&text, BASE, &[], 0x400_0000);
        assert!(locate(&img).unwrap_err().contains("2 times"));
        let text = self::text();
        let img = crate::crash_fixes::tests::image(&text, BASE, &[], 0x100_0000);
        assert!(locate(&img).unwrap_err().contains("image"));
        // The marking reading another name table than the walk: neither is trusted.
        let mut text = self::text();
        let at = text.len() - parse(NAME_MARKS).len() + NAME_MARKS_NAMES;
        text[at..at + 4].copy_from_slice(&0x0333_cd70u32.to_le_bytes());
        let img = crate::crash_fixes::tests::image(&text, BASE, &[], 0x400_0000);
        assert!(locate(&img).unwrap_err().contains("different tables"));
    }
}
