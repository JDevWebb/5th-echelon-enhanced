//! The game's linear package reader (`FArchiveFileReaderLoadLinear`) and the sync on the UMD read
//! sequence it runs before each read (the hooks' `umd_read_fix` has the whole story).

use super::parse;
use super::Image;

/// `Serialize(dst, n)` (vtable slot 3): `step(id, n, pos, archive, ?)` on the current slot's
/// tracker, then `archive->Serialize(dst, n)` on the slot's UMD archive, then `pos += n`. The
/// manager's current slot `[+0x14c]`, its trackers `[+0x2c + 4i]` and archives `[+0xec + 4i]`, and
/// the reader's manager `+0xc`, id `+0x10`, position `+0x1c` are all in it.
pub const SERIALIZE: &str = "55 8B EC 56 8B F1 8B 46 0C 8B 56 14 8B 88 4C 01 00 00 57 8B 7D 0C 52 8B 94 88 EC 00 00 00 \
     8B 4C 88 2C 52 8B 56 1C 52 8B 56 10 57 52 E8 ?? ?? ?? ?? 8B 46 0C 8B 88 4C 01 00 00 8B 84 88 EC 00 00 00 8B 4D 08 \
     8B 10 8B 52 0C 57 51 8B C8 FF D2 01 7E 1C 5F 5E 5D C2 08 00";
const SERIALIZE_STEP: usize = 45;

/// Slot 4: only the sync, `sync(id, pos, archive, ?)` on the current slot's tracker.
pub const SYNC_ONLY: &str = "8B 41 0C 8B 90 4C 01 00 00 56 8B 71 14 56 8B B4 90 EC 00 00 00 56 8B 71 1C 8B 49 10 56 51 \
     8B 4C 90 2C E8 ?? ?? ?? ?? 5E C3";
const SYNC_ONLY_SYNC: usize = 35;

/// Slot 22 (`Precache(hint)`): the sync when the tracker is persistent (`+0x44`), then a jump to
/// the slot's archive's own slot 22.
pub const PRECACHE: &str = "55 8B EC 56 8B F1 8B 46 0C 8B 88 4C 01 00 00 8B 54 88 2C 83 7A 44 00 74 23 8B 56 14 8B 88 \
     4C 01 00 00 52 8B 94 88 EC 00 00 00 8B 4C 88 2C 52 8B 56 1C 52 8B 56 10 52 E8 ?? ?? ?? ?? 8B 76 0C 8B 86 4C 01 00 00 \
     8B 8C 86 EC 00 00 00 8B 11 5E 8B 52 58 5D FF E2";
const PRECACHE_SYNC: usize = 56;

/// The sync: `cmp [linear on],0 / je` and `cmp [linear off],0 / jne`, then the tracker's fields as
/// `umd::sync_finds` models them: entry index `+0x2c`, block `+0x3c` (read unchecked: the crash),
/// offset `+0x34`, entry `+0x38` (its id at `[0]`), block fields `[0]` and `[8]`, the step to the
/// next block (`+0x30`) and the persistent flag `+0x44`.
pub const SYNC: &str = "55 8B EC 83 EC 10 83 3D ?? ?? ?? ?? 00 53 56 57 8B F1 0F 84 ?? ?? ?? ?? 83 3D ?? ?? ?? ?? 00 \
     0F 85 ?? ?? ?? ?? 8B 46 2C 89 45 F0 C7 45 F8 00 00 00 00 8B 4E 3C 8B 56 34 8B 01 8B 5D 08 8B 7E 38 03 C2 3B 1F 8B 5D \
     0C 75 17 3B D8 0F 84 ?? ?? ?? ?? 7E 0D 8B 79 08 03 39 3B DF 0F 8C ?? ?? ?? ?? 8B 49 08 2B CA BF 01 00 00 00 01 7E 30 \
     8B 46 30 33 D2 89 56 34 39 56 44 75 57";
const SYNC_LINEAR_ON: usize = 8;
const SYNC_LINEAR_OFF: usize = 26;
/// `mov eax,[ecx]` with `ecx` the current block: where a missing block crashes (DX11 `0x4504e8`).
pub const SYNC_CRASH_AT: usize = 0x38;

/// The step: counts the read (`+0x24`, `+0x28`), syncs, then moves the cursor by `n`.
pub const STEP: &str = "55 8B EC 56 57 8B 7D 0C 8B F1 FF 46 24 01 7E 28 85 FF 0F 84 ?? ?? ?? ?? 8B 45 18 8B 4D 14 \
     8B 55 10 50 8B 45 08 51 52 50 8B CE E8";
const STEP_SYNC: usize = 43;

/// The vtable slots replaced.
pub const SLOT_SERIALIZE: usize = 3;
pub const SLOT_SYNC_ONLY: usize = 4;
pub const SLOT_PRECACHE: usize = 22;

/// Where the fix goes.
#[derive(Debug)]
pub struct Site {
    /// `FArchiveFileReaderLoadLinear`'s vtable.
    pub vtable: usize,
    pub serialize: usize,
    pub sync_only: usize,
    pub precache: usize,
    pub sync: usize,
    /// The game's switches: the sync works while `[linear_on] != 0` and `[linear_off] == 0`.
    pub linear_on: usize,
    pub linear_off: usize,
}

impl Site {
    /// The address of vtable slot `i`.
    pub fn slot(&self, i: usize) -> usize {
        self.vtable + 4 * i
    }
}

fn find(img: &Image, what: &str, sig: &str) -> Result<usize, String> {
    img.find(&parse(sig)).map_err(|n| format!("{what} found {n} times, not once"))
}

/// How many leading bytes another hook may have replaced with a `jmp rel32` (a detour such as
/// the `UmdReadTrace` diagnosis puts on the sync and the step).
const HOOK_JMP: usize = 5;

/// Whether the code at `va` is `sig`; its first five bytes may instead be a hook's `jmp`.
fn is_at(img: &Image, va: usize, sig: &str) -> bool {
    let sig = parse(sig);
    let Some(code) = img.bytes(va, sig.len()) else {
        return false;
    };
    let hooked = code[0] == 0xE9;
    code.iter().zip(&sig).enumerate().all(|(i, (b, s))| (hooked && i < HOOK_JMP) || s.is_none_or(|s| s == *b))
}

pub fn locate(img: &Image) -> Result<Site, String> {
    let serialize = find(img, "the linear reader's Serialize", SERIALIZE)?;
    let sync_only = find(img, "the linear reader's sync", SYNC_ONLY)?;
    let precache = find(img, "the linear reader's Precache", PRECACHE)?;
    // The step and the sync are reached through the calls, not searched: in the running game
    // another hook may sit on their first bytes; the rest must still be theirs.
    let call = |at: usize| img.rel32_target(at).ok_or_else(|| format!("the call at {at:#010x}"));
    let step = call(serialize + SERIALIZE_STEP)?;
    if !is_at(img, step, STEP) {
        return Err(format!("Serialize's callee {step:#010x} isn't the read-sequence step"));
    }
    let sync = call(step + STEP_SYNC)?;
    if !is_at(img, sync, SYNC) {
        return Err(format!("the step's callee {sync:#010x} isn't the read-sequence sync"));
    }
    if call(sync_only + SYNC_ONLY_SYNC)? != sync || call(precache + PRECACHE_SYNC)? != sync {
        return Err(String::from("the sync slot or Precache doesn't call the sync"));
    }
    let linear_on = img.u32(sync + SYNC_LINEAR_ON).ok_or("the sync's first switch")?;
    let linear_off = img.u32(sync + SYNC_LINEAR_OFF).ok_or("the sync's second switch")?;
    for at in [linear_on, linear_off] {
        if !img.image.contains(&at) || img.text_range().contains(&at) {
            return Err(format!("the switch {at:#010x} isn't the game's data"));
        }
    }
    // The vtable: Serialize and the sync slot next to each other, Precache 19 slots on.
    let mut found = Vec::new();
    let rdata = img.rdata_range();
    let mut at = rdata.start;
    while at + 4 * (SLOT_PRECACHE + 1) <= rdata.end {
        if img.u32(at) == Some(serialize) && img.u32(at + 4) == Some(sync_only) && img.u32(at + 4 * (SLOT_PRECACHE - SLOT_SERIALIZE)) == Some(precache) {
            found.push(at - 4 * SLOT_SERIALIZE);
        }
        at += 4;
    }
    let [vtable] = found[..] else {
        return Err(format!("the linear reader's vtable found {} times, not once", found.len()));
    };
    Ok(Site {
        vtable,
        serialize,
        sync_only,
        precache,
        sync,
        linear_on,
        linear_off,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DX11 functions (VA 0x4504b0 sync, 0x450730 step, 0x4508c0 Precache, 0x450920
    /// Serialize, 0x450980 sync slot) at their places in a fake `.text`, a vtable in `.rdata`.
    #[test]
    fn finds_the_reader_and_its_vtable() {
        let base = 0x0045_0000usize;
        let mut text = vec![0xCCu8; 0x1000];
        let mut put = |va: usize, sig: &str, calls: &[(usize, usize)]| {
            let bytes: Vec<u8> = parse(sig).iter().map(|b| b.unwrap_or(0)).collect();
            text[va - base..va - base + bytes.len()].copy_from_slice(&bytes);
            for &(at, target) in calls {
                let rel = (target as u32).wrapping_sub((va + at + 4) as u32);
                text[va + at - base..va + at - base + 4].copy_from_slice(&rel.to_le_bytes());
            }
        };
        put(0x4504b0, SYNC, &[]);
        put(0x450730, STEP, &[(STEP_SYNC, 0x4504b0)]);
        put(0x4508c0, PRECACHE, &[(PRECACHE_SYNC, 0x4504b0)]);
        put(0x450920, SERIALIZE, &[(SERIALIZE_STEP, 0x450730)]);
        put(0x450980, SYNC_ONLY, &[(SYNC_ONLY_SYNC, 0x4504b0)]);
        let switches = [(0x4504b0 + SYNC_LINEAR_ON, 0x0330_c4d4u32), (0x4504b0 + SYNC_LINEAR_OFF, 0x0330_c4d8)];
        for (at, v) in switches {
            text[at - base..at - base + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut rdata = vec![0u8; 0x200];
        for (slot, f) in [(3usize, 0x450920u32), (4, 0x450980), (22, 0x4508c0)] {
            rdata[0x40 + 4 * slot..0x40 + 4 * slot + 4].copy_from_slice(&f.to_le_bytes());
        }
        let mut img = crate::crash_fixes::tests::image(&text, base, &rdata, 0x0299_3000);
        img.image = 0x0040_0000..0x0350_0000;
        let site = locate(&img).unwrap();
        assert_eq!(site.vtable, 0x0299_3040);
        assert_eq!((site.serialize, site.sync_only, site.precache, site.sync), (0x450920, 0x450980, 0x4508c0, 0x4504b0));
        assert_eq!((site.linear_on, site.linear_off), (0x0330_c4d4, 0x0330_c4d8));
        assert_eq!(site.slot(SLOT_PRECACHE), 0x0299_3040 + 88);
        assert_eq!(&text[0x4504b0 - base + SYNC_CRASH_AT..][..2], &[0x8B, 0x01], "the crash's mov eax,[ecx]");

        // A Serialize that calls something else isn't this reader.
        let rel = 0u32.to_le_bytes();
        let mut other = text.clone();
        other[0x450920 - base + SERIALIZE_STEP..][..4].copy_from_slice(&rel);
        let mut img = crate::crash_fixes::tests::image(&other, base, &rdata, 0x0299_3000);
        img.image = 0x0040_0000..0x0350_0000;
        assert!(locate(&img).unwrap_err().contains("isn't the read-sequence step"));

        // Another hook's jmp on the sync's and the step's first bytes (UmdReadTrace) is no matter.
        let mut hooked = text.clone();
        for va in [0x4504b0usize, 0x450730] {
            hooked[va - base..va - base + 5].copy_from_slice(&[0xE9, 0x11, 0x22, 0x33, 0x44]);
        }
        let mut img = crate::crash_fixes::tests::image(&hooked, base, &rdata, 0x0299_3000);
        img.image = 0x0040_0000..0x0350_0000;
        assert_eq!(locate(&img).unwrap().sync, 0x4504b0);

        // But not one that changed the sync further on.
        hooked[0x4504b0 - base + 0x20] ^= 0xFF;
        let mut img = crate::crash_fixes::tests::image(&hooked, base, &rdata, 0x0299_3000);
        img.image = 0x0040_0000..0x0350_0000;
        assert!(locate(&img).unwrap_err().contains("isn't the read-sequence sync"));
    }

    /// load_order_fix patches the sync's first read of the cursor (0x32 into it, the crash's
    /// `mov`s): this fix must be installed first, as hooks.rs does, or it finds the sync changed.
    #[test]
    fn the_load_order_fix_patches_inside_the_sync() {
        let sig = parse(SYNC);
        let first: [u8; crate::crash_fixes::load_order::HOOK_LEN] = [0x8B, 0x4E, 0x3C, 0x8B, 0x56, 0x34];
        let at = 0x4504e2 - 0x4504b0;
        assert!(sig[at..at + first.len()].iter().zip(first).all(|(s, b)| *s == Some(b)));
        assert_eq!(at + 6, SYNC_CRASH_AT, "the crash's read is right after them");
    }
}
