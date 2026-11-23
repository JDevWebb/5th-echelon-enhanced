//! The test build's `findtext <text>` (0.5.6's N1 research): where a piece of text (another
//! player's name, say) is kept in the game's memory, ASCII and UTF-16, and what points at each
//! copy: an engine object (by its class and the offset in it) or other memory (by its region).
//! Read with ReadProcessMemory on this process, so a region freed meanwhile is an error, not a
//! crash. Logged only; never in a release.

use std::collections::BTreeMap;
use std::ffi::c_void;

use tracing::info;

use crate::dataversion::read_u32;

#[repr(C)]
#[derive(Default)]
struct MemoryInfo {
    base: usize,
    allocation_base: usize,
    allocation_protect: u32,
    size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn VirtualQuery(address: *const c_void, info: *mut MemoryInfo, length: usize) -> usize;
    fn GetCurrentProcess() -> isize;
    fn ReadProcessMemory(process: isize, base: *const c_void, buffer: *mut c_void, size: usize, read: *mut usize) -> i32;
}

const MEM_COMMIT: u32 = 0x1000;
const MEM_IMAGE: u32 = 0x0100_0000;
const PAGE_GUARD: u32 = 0x100;
const PAGE_NOACCESS: u32 = 0x01;
const MOST_HITS: usize = 24;
const MOST_POINTERS: usize = 6;

/// The committed, readable regions: (base, size, kind).
fn regions() -> Vec<(usize, usize, u32)> {
    let mut out = Vec::new();
    let mut at = 0x10000usize;
    while at < 0x7fff_0000 {
        let mut i = MemoryInfo::default();
        if unsafe { VirtualQuery(at as *const c_void, &mut i, std::mem::size_of::<MemoryInfo>()) } == 0 || i.size == 0 {
            break;
        }
        if i.state == MEM_COMMIT && i.protect & (PAGE_GUARD | PAGE_NOACCESS) == 0 && i.protect != 0 {
            out.push((i.base, i.size, i.kind));
        }
        at = i.base.saturating_add(i.size);
    }
    out
}

/// Every region's bytes, a chunk at a time, to `each(base of chunk, bytes)`.
fn each_chunk(regions: &[(usize, usize, u32)], mut each: impl FnMut(usize, &[u8])) {
    const CHUNK: usize = 1 << 20;
    let mut buffer = vec![0u8; CHUNK + 64];
    for (base, size, _) in regions {
        let mut at = *base;
        let end = base + size;
        while at < end {
            // Overlap by 64 bytes so a match across two chunks is found.
            let n = (end - at).min(CHUNK + 64);
            let mut got = 0usize;
            let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), at as *const c_void, buffer.as_mut_ptr().cast(), n, &mut got) };
            if ok != 0 && got > 0 {
                each(at, &buffer[..got]);
            }
            at += CHUNK.min(end - at);
        }
    }
}

fn kind_of(regions: &[(usize, usize, u32)], address: usize) -> &'static str {
    match regions.iter().find(|(b, s, _)| (*b..b + s).contains(&address)) {
        Some((_, _, k)) if *k == MEM_IMAGE => "image",
        Some((_, _, 0x20000)) => "private",
        Some((_, _, 0x40000)) => "mapped",
        _ => "?",
    }
}

pub fn find(text: &str) {
    if text.len() < 3 {
        return;
    }
    let ascii: Vec<u8> = text.as_bytes().to_vec();
    let wide: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let regions = regions();
    let mut hits: Vec<(usize, &'static str)> = Vec::new();
    each_chunk(&regions, |base, bytes| {
        for (needle, how) in [(&ascii, "ascii"), (&wide, "utf16")] {
            let mut from = 0;
            while hits.len() < MOST_HITS {
                let Some(i) = bytes[from..].windows(needle.len()).position(|w| w == needle.as_slice()) else { break };
                let at = base + from + i;
                if !hits.iter().any(|(h, _)| *h == at) {
                    hits.push((at, how));
                }
                from += i + 1;
            }
        }
    });
    // The engine's objects, for naming what points at a copy (the nearest object below a pointer,
    // within 4 KB: objects are smaller than that, as the game's classes go).
    let mut objects: BTreeMap<usize, String> = BTreeMap::new();
    crate::round_state::each_object(|o| {
        let class = read_u32(o + 0x24).and_then(|c| read_u32(c as usize + 0x20)).and_then(crate::round_state::name_text).unwrap_or_default();
        objects.insert(o, class);
    });
    let targets: Vec<u32> = hits.iter().map(|(h, _)| *h as u32).collect();
    let mut pointers: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    each_chunk(&regions, |base, bytes| {
        for (i, w) in bytes.chunks_exact(4).enumerate() {
            let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            if targets.contains(&v) {
                let list = pointers.entry(v).or_default();
                let at = base + i * 4;
                if list.len() < MOST_POINTERS && !list.contains(&at) {
                    list.push(at);
                }
            }
        }
    });
    info!("Find text: test: {text:?}: {} copies", hits.len());
    for (at, how) in &hits {
        let before: Vec<String> = (1..=4).rev().filter_map(|k| read_u32(at - k * 4)).map(|w| format!("{w:x}")).collect();
        let by: Vec<String> = pointers
            .get(&(*at as u32))
            .map(|list| {
                list.iter()
                    .map(|p| match objects.range(..=*p).next_back().filter(|(o, _)| p - **o < 0x1000) {
                        Some((o, class)) => format!("{p:#x} = {class}+{}", p - o),
                        None => format!("{p:#x} ({})", kind_of(&regions, *p)),
                    })
                    .collect()
            })
            .unwrap_or_default();
        info!("Find text: test: {at:#x} {how} in {}; before {}; pointed at by {by:?}", kind_of(&regions, *at), before.join(" "));
    }
}
