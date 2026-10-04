//! What the game's "data version" is made of, written to `bl-dataversion.txt` beside the
//! log, to find out why two copies of the game refuse each other's joins (a "different
//! version of the game", the host's `DATA_VERSION_MISMATCH`).
//!
//! The host compares the version in a join request with its own (lib.rs,
//! `DATA_VERSION_CHECK`). The game works its own out once its startup packages are loaded
//! (`FUN_00829060` in the DirectX 11 build):
//!
//! ```text
//! names    = Σ over the name table (GNames) entries, in two ranges, of
//!            entry[+4] + (entry[+4] ^ hash(the entry's index, as "%d"))
//!            · [0, native end): the names the engine registers for itself
//!            · [startup start, startup end): the names loading the startup packages
//!              added, without those containing ".ini" or starting with "..\..\"
//! version  = (low 32 bits of names) ^ engine[+0x64][+0x410] ^ crc(upper(engine[+0x68][+0x40c])) + 1
//! ```
//!
//! So the version is a checksum of everything the game loaded at startup: two installs that
//! load different packages (another language's, a mod, a missing or extra file) differ. The
//! file lists the inputs and fingerprints of the ranges of names, and of the startup range in
//! blocks, so two players' files show where their copies part (`bl-dataversion.full` in the
//! game's folder lists every name too, in `bl-dataversion-full.txt`).
//!
//! Only for the DirectX 11 build (Steam's and Ubisoft's are the same code): the addresses
//! are that build's, checked by finding the data version check where that build has it.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use tracing::info;
use tracing::warn;

/// Where the DirectX 11 build has the data version check (lib.rs `DATA_VERSION_CHECK`).
pub const DX11_CHECK: usize = 0x0085_9d72;
/// The engine object the version hangs off (`[engine + 0x70] + 0x418` is the version).
const ENGINE: usize = 0x0338_3df8;
/// The name table: its entries and their number.
const NAMES: usize = 0x0333_cd60;
const NAMES_COUNT: usize = 0x0333_cd64;
/// The ranges counted: [0, NATIVE_END) and [STARTUP_START, STARTUP_END).
const NATIVE_END: usize = 0x0333_cd54;
const STARTUP_START: usize = 0x0333_cd58;
const STARTUP_END: usize = 0x0333_cd5c;
/// How long to wait for the game to work its version out.
const WAIT: Duration = Duration::from_secs(15 * 60);
/// The most names written (the table holds tens of thousands).
const MAX_NAMES: usize = 200_000;

/// Reads a `T` at `addr`, if the memory there is readable.
fn read<T: Copy>(addr: usize) -> Option<T> {
    let size = std::mem::size_of::<T>();
    let region = region::query(addr as *const u8).ok()?;
    let end = region.as_ptr::<u8>() as usize + region.len();
    if !region.is_readable() || addr + size > end {
        return None;
    }
    Some(unsafe { std::ptr::read_unaligned(addr as *const T) })
}

fn read_u32(addr: usize) -> Option<u32> {
    read::<u32>(addr)
}

/// A C string at `addr`, up to `max` bytes.
fn read_string(addr: usize, max: usize) -> Option<String> {
    let mut bytes = Vec::new();
    for i in 0..max {
        let b = read::<u8>(addr + i)?;
        if b == 0 {
            break;
        }
        bytes.push(b);
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The game's own text string object (`Gear::GearBasicString`): its text, if it has any.
fn gear_string(object: usize) -> Option<String> {
    let data = read_u32(object + 8)? as usize;
    if data == 0 {
        return Some(String::new());
    }
    read_string(data + 0xc, 512)
}

/// The data version the game worked out, once it has.
fn version() -> Option<u32> {
    let engine = read_u32(ENGINE)? as usize;
    let online = read_u32(engine.checked_add(0x70)?)? as usize;
    read_u32(online.checked_add(0x418)?).filter(|v| *v != 0)
}

/// The engine's other two inputs: the number at `[+0x64][+0x410]` and the string at
/// `[+0x68][+0x40c]`.
fn inputs() -> (Option<u32>, Option<String>) {
    let Some(engine) = read_u32(ENGINE).map(|e| e as usize) else { return (None, None) };
    let number = read_u32(engine + 0x64).and_then(|o| read_u32(o as usize + 0x410));
    let string = read_u32(engine + 0x68).and_then(|o| gear_string(o as usize + 0x40c));
    (number, string)
}

/// Whether a name in the startup range counts: not an ini file, not a path out of the game.
fn counted_in_startup(name: &str) -> bool {
    !name.contains(".ini") && !name.starts_with("..\\..\\")
}

/// Names per block fingerprinted in the startup range.
const BLOCK: usize = 1000;
/// In the game's folder, asks for every name to be listed too (`bl-dataversion-full.txt`).
const FULL_FLAG: &str = "bl-dataversion.full";

/// A fingerprint of names, so two players' files show at a glance whether a range differs.
#[derive(Default)]
struct Fingerprint {
    hash: u64,
    count: usize,
}

impl Fingerprint {
    fn add(&mut self, index: usize, value: u32, name: &str) {
        if self.count == 0 {
            self.hash = 0xcbf2_9ce4_8422_2325;
        }
        for b in (index as u32).to_le_bytes().into_iter().chain(value.to_le_bytes()).chain(name.bytes()).chain([0]) {
            self.hash = (self.hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        self.count += 1;
    }

    fn show(&self) -> String {
        format!("{} names, fingerprint {:016x}", self.count, self.hash)
    }
}

/// The file's text: the version and its inputs, a fingerprint of each range of names and of
/// each block of [`BLOCK`] in the startup range (where two copies usually differ: the first
/// block whose fingerprints differ is where to look). With `full`, every name too, as the
/// second text.
fn report(version: u32, full: bool) -> (String, Option<String>) {
    let mut out = String::new();
    let _ = writeln!(out, "data version: {version:#010x}");
    let (number, string) = inputs();
    let _ = writeln!(out, "engine[+0x64][+0x410]: {}", number.map_or("unreadable".into(), |n| format!("{n:#010x}")));
    let _ = writeln!(out, "engine[+0x68][+0x40c]: {:?}", string.as_deref().unwrap_or("(unreadable)"));
    let field = |a| read_u32(a).map_or("unreadable".into(), |v| v.to_string());
    let (count, native_end, start, end) = (read_u32(NAMES_COUNT), read_u32(NATIVE_END), read_u32(STARTUP_START), read_u32(STARTUP_END));
    let _ = writeln!(
        out,
        "names: {} in all; counted: [0, {}) and [{}, {})",
        field(NAMES_COUNT),
        field(NATIVE_END),
        field(STARTUP_START),
        field(STARTUP_END)
    );
    let _ = writeln!(out, "game version count: {}", field(0x0330_c478));
    let (Some(table), Some(count), Some(native_end), Some(start), Some(end)) = (read_u32(NAMES), count, native_end, start, end) else {
        let _ = writeln!(out, "(the name table couldn't be read)");
        return (out, None);
    };
    let (mut native, mut startup, mut skipped, mut later) = (Fingerprint::default(), Fingerprint::default(), Fingerprint::default(), Fingerprint::default());
    let mut blocks: Vec<Fingerprint> = Vec::new();
    let mut listed = full.then(|| String::from("index\tvalue\tcounted\tname\n"));
    for i in 0..(count as usize).min(MAX_NAMES) {
        let Some(entry) = read_u32(table as usize + i * 4).filter(|e| *e != 0).map(|e| e as usize) else {
            continue;
        };
        let value = read_u32(entry + 4).unwrap_or_default();
        let name = read_string(entry + 8, 1024).unwrap_or_default();
        let index = i as u32;
        let counted = index < native_end || (start..end).contains(&index) && counted_in_startup(&name);
        if index < native_end {
            native.add(i, value, &name);
        } else if (start..end).contains(&index) {
            if counted {
                startup.add(i, value, &name);
            } else {
                skipped.add(i, value, &name);
            }
            let block = (index - start) as usize / BLOCK;
            if blocks.len() <= block {
                blocks.resize_with(block + 1, Fingerprint::default);
            }
            blocks[block].add(i, value, &name);
        } else {
            later.add(i, value, &name);
        }
        if let Some(listed) = &mut listed {
            let _ = writeln!(listed, "{i}\t{value:#010x}\t{}\t{name}", if counted { "yes" } else { "no" });
        }
    }
    let _ = writeln!(out, "\nengine's own (counted): {}", native.show());
    let _ = writeln!(out, "startup packages (counted): {}", startup.show());
    let _ = writeln!(out, "startup packages (.ini and outside paths, not counted): {}", skipped.show());
    let _ = writeln!(out, "loaded later (not counted): {}", later.show());
    let _ = writeln!(
        out,
        "\nThe startup range by blocks of {BLOCK} (compare two files: the first that differs is where they part):"
    );
    for (n, block) in blocks.iter().enumerate() {
        let from = start as usize + n * BLOCK;
        let _ = writeln!(out, "{from}..{}\t{}", from + BLOCK, block.show());
    }
    let _ = writeln!(
        out,
        "\nEvery name: put an empty {FULL_FLAG} in the game's folder, and the next start writes bl-dataversion-full.txt."
    );
    (out, listed)
}

/// Waits (on a thread of its own) for the game to work its data version out, then writes
/// what it's made of to `bl-dataversion.txt` in `dir`. Only when the data version check is
/// at `check`, where the DirectX 11 build has it.
pub fn write_when_ready(check: usize, dir: PathBuf) {
    if check != DX11_CHECK {
        info!("Data version: the check is at {check:#x}, not where the DirectX 11 build has it; what the version is made of isn't written");
        return;
    }
    let _ = std::thread::Builder::new().name("fe-dataversion".into()).spawn(move || {
        let started = Instant::now();
        while started.elapsed() < WAIT {
            if let Some(version) = version() {
                let (summary, full) = report(version, dir.join(FULL_FLAG).exists());
                let path = dir.join("bl-dataversion.txt");
                match std::fs::write(&path, summary) {
                    Ok(()) => info!("Data version {version:#010x}; what it's made of is in {}", path.display()),
                    Err(e) => warn!("Data version {version:#010x}; couldn't write {}: {e}", path.display()),
                }
                let full_path = dir.join("bl-dataversion-full.txt");
                match full {
                    Some(full) => {
                        if let Err(e) = std::fs::write(&full_path, full) {
                            warn!("Data version: couldn't write {}: {e}", full_path.display());
                        }
                    }
                    // An old full listing would be mistaken for this start's.
                    None => {
                        let _ = std::fs::remove_file(&full_path);
                    }
                }
                return;
            }
            std::thread::sleep(Duration::from_secs(5));
        }
        warn!("Data version: the game didn't work it out within {} minutes", WAIT.as_secs() / 60);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_names_skip_ini_files_and_outside_paths() {
        assert!(counted_in_startup("Loc_Messages"));
        assert!(!counted_in_startup("DefaultEngine.ini"));
        assert!(!counted_in_startup("..\\..\\Config\\X"));
    }

    #[test]
    fn fingerprints_tell_ranges_apart() {
        let mut a = Fingerprint::default();
        let mut b = Fingerprint::default();
        a.add(1, 7, "Loc_Messages");
        b.add(1, 7, "Loc_Messages");
        assert_eq!(a.show(), b.show());
        b.add(2, 9, "Loc_Messages_POR");
        assert_ne!(a.show(), b.show());
        assert!(b.show().starts_with("2 names, fingerprint "));
    }

    #[test]
    fn unreadable_memory_reads_as_nothing() {
        assert_eq!(read_u32(0), None);
        let x: u32 = 0x1234_5678;
        assert_eq!(read_u32(std::ptr::addr_of!(x) as usize), Some(0x1234_5678));
    }
}
