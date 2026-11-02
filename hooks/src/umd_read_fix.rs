//! Serves a package read the game's UMD read sequence can't, from the UMD files themselves,
//! instead of letting the sequence break: a crash on going back to the lobby after a match
//! (`Blacklist_DX11_game.exe+0x504e8`, a `USkeletalMesh` loading), on the host and on its own.
//!
//! The game ships no loose packages. Each `UMDs\*.umd` holds the bytes the game read from its
//! packages when a level (or a PEC content pack) loaded at build time, back to back in the order
//! they were read, with a header listing them (`hooks_addresses::umd`). While a UMD is open (a
//! stack of slots in the file manager, the top one current), every package reader made for a file
//! in it is an `FArchiveFileReaderLoadLinear`; each of its reads first syncs a cursor on the
//! *current* slot's sequence to the read (`FUN_004504b0` in DX11) and then takes the next bytes of
//! that slot's UMD stream. When the read isn't where the sequence has it (behind the cursor in a
//! level's strictly forward UMD, not recorded at all, or the reader's file isn't in the current
//! UMD), the sync runs the cursor off the end, the read gets the wrong bytes, and the cursor is
//! left without a block: the next read through it reads `[0]` (`exe+0x504e8`). Going from a match
//! to the lobby is where that happens: the lobby's sequence was recorded coming from the menu, and
//! what the match left in memory (packages and their readers, the player's own gear, a bot's)
//! changes which bytes are read and in what order; the stack is the same in every report.
//!
//! The fix: the reader's vtable slots that sync (`Serialize`, the bare sync, `Precache`) are
//! pointed here. Each first works out, without touching anything, whether the game's sync would
//! find the read (`umd::sync_finds`, the same walk). If it would, the game's own code runs as
//! before. If not, the bytes come from the UMD files on disk: the current slot's UMD, the other
//! open ones, else any UMD that recorded them for a file of that size (the same bytes in each, the
//! same build); the reader's position moves on and the sequence and its stream stay where they
//! were, for the reads that do follow them. A read no UMD has is left to the game, as before. The
//! bare sync and `Precache` skip the sync for such a read, so nothing ever empties the cursor.
//!
//! Matched by signatures unique in both executables (DX11 `Serialize` VA `0x450920`), cross-checked
//! (who calls whom, the vtable holding all three); anything else leaves the game alone. Logged:
//! the first reads served in full (which file, which UMD, why the sequence missed), then counts.

// The game's fields are 32-bit values read in a 32-bit process; the file names compared by
// extension are lowercased first.
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss)]
#![allow(clippy::case_sensitive_file_extension_comparisons)]

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use hooks_addresses::crash_fixes::umd_read;
use hooks_addresses::umd;
use hooks_addresses::umd::Block;
use hooks_addresses::umd::Cursor;
use hooks_addresses::umd::Miss;
use hooks_addresses::umd::Sequence;
use tracing::error;
use tracing::info;
use tracing::warn;

type SerializeFn = unsafe extern "thiscall" fn(*mut u8, *mut u8, i32);
type SyncOnlyFn = unsafe extern "thiscall" fn(*mut u8) -> u32;
type PrecacheFn = unsafe extern "thiscall" fn(*mut u8, i32) -> u32;

/// The game's functions and switches, set once before the vtable is rewritten.
static SERIALIZE: AtomicUsize = AtomicUsize::new(0);
static SYNC_ONLY: AtomicUsize = AtomicUsize::new(0);
static PRECACHE: AtomicUsize = AtomicUsize::new(0);
static LINEAR_ON: AtomicUsize = AtomicUsize::new(0);
static LINEAR_OFF: AtomicUsize = AtomicUsize::new(0);

/// Reads served from the files, reads no UMD had, syncs skipped (bare sync and `Precache`).
static SERVED: AtomicU32 = AtomicU32::new(0);
static UNSERVED: AtomicU32 = AtomicU32::new(0);
static SKIPPED: AtomicU32 = AtomicU32::new(0);
/// Reads logged in full; after these only the counts.
const LOGGED_IN_FULL: u32 = 24;

// The reader (`FArchiveFileReaderLoadLinear`).
const R_MANAGER: usize = 0xc;
const R_ID: usize = 0x10;
const R_SIZE: usize = 0x18;
const R_POS: usize = 0x1c;
// The file manager (`FFileManagerLinear`): per slot a tracker, a UMD archive and the UMD's name.
const M_TRACKERS: usize = 0x2c;
const M_NAMES: usize = 0x8c;
const M_NAME_STRIDE: usize = 0xc;
const M_ARCHIVES: usize = 0xec;
const M_SLOT: usize = 0x14c;
const SLOTS: usize = 16;
// The tracker: the sequence and the cursor on it.
const T_ENTRIES: usize = 0x18;
const T_COUNT: usize = 0x1c;
const T_ENTRY_INDEX: usize = 0x2c;
const T_BLOCK_INDEX: usize = 0x30;
const T_OFFSET: usize = 0x34;
const T_ENTRY: usize = 0x38;
const T_BLOCK: usize = 0x3c;
const T_PERSISTENT: usize = 0x44;
/// The archives' vtable slot 22 (`Precache`).
const A_PRECACHE: usize = 22;

unsafe fn rd(at: usize) -> usize {
    std::ptr::read_unaligned(at as *const u32) as usize
}

/// A tracker's sequence, read where the game keeps it (entries of 16 bytes: id, blocks, count;
/// blocks of 12: file offset, UMD offset, length).
struct GameSequence {
    table: usize,
    count: usize,
}

impl Sequence for GameSequence {
    fn entries(&self) -> usize {
        self.count
    }
    fn id(&self, entry: usize) -> u32 {
        unsafe { rd(self.table + 16 * entry) as u32 }
    }
    fn blocks(&self, entry: usize) -> usize {
        unsafe { rd(self.table + 16 * entry + 8) }
    }
    fn block(&self, entry: usize, block: usize) -> Block {
        unsafe {
            let at = rd(self.table + 16 * entry + 4) + 12 * block;
            Block {
                file: rd(at) as u32,
                umd: rd(at + 4) as u32,
                len: rd(at + 8) as u32,
            }
        }
    }
}

/// The cursor as the game's sync would take it, or `None` when its pointers and indices don't
/// agree (then the game's own code decides, as before).
unsafe fn cursor(tracker: usize, seq: &GameSequence) -> Option<Cursor> {
    let persistent = rd(tracker + T_PERSISTENT) != 0;
    let (entry_ptr, block_ptr) = (rd(tracker + T_ENTRY), rd(tracker + T_BLOCK));
    let offset = rd(tracker + T_OFFSET) as u32;
    if block_ptr == 0 {
        return Some(Cursor {
            entry: 0,
            block: 0,
            offset,
            has_block: false,
            persistent,
        });
    }
    if seq.table == 0 || entry_ptr < seq.table || !(entry_ptr - seq.table).is_multiple_of(16) {
        return None;
    }
    let entry = (entry_ptr - seq.table) / 16;
    if entry >= seq.count {
        return None;
    }
    let blocks = rd(entry_ptr + 4);
    if block_ptr < blocks || !(block_ptr - blocks).is_multiple_of(12) || (block_ptr - blocks) / 12 >= seq.blocks(entry) {
        return None;
    }
    let block = (block_ptr - blocks) / 12;
    // A strict sequence steps on by the indices: they must be where the pointers are.
    if !persistent && (rd(tracker + T_ENTRY_INDEX) != entry || rd(tracker + T_BLOCK_INDEX) != block) {
        return None;
    }
    Some(Cursor {
        entry,
        block,
        offset,
        has_block: true,
        persistent,
    })
}

fn linear_on() -> bool {
    unsafe { rd(LINEAR_ON.load(Ordering::Relaxed)) != 0 && rd(LINEAR_OFF.load(Ordering::Relaxed)) == 0 }
}

/// What a read of the reader's next `n` bytes would meet.
struct Read {
    manager: usize,
    slot: usize,
    tracker: usize,
    id: u32,
    size: u32,
    pos: i32,
    /// `None`: the game finds it (or decides itself). `Some`: why its sync wouldn't.
    miss: Option<Miss>,
    cursor: Option<Cursor>,
}

unsafe fn look(this: *mut u8) -> Option<Read> {
    if !linear_on() {
        return None;
    }
    let r = this as usize;
    let manager = rd(r + R_MANAGER);
    if manager == 0 {
        return None;
    }
    let slot = rd(manager + M_SLOT);
    if slot >= SLOTS {
        return None;
    }
    let tracker = rd(manager + M_TRACKERS + 4 * slot);
    let (id, size, pos) = (rd(r + R_ID) as u32, rd(r + R_SIZE) as u32, rd(r + R_POS) as i32);
    if tracker == 0 {
        // The game would step a null tracker; only the files can serve this.
        return Some(Read {
            manager,
            slot,
            tracker,
            id,
            size,
            pos,
            miss: Some(Miss::NoBlock),
            cursor: None,
        });
    }
    let seq = GameSequence {
        table: rd(tracker + T_ENTRIES),
        count: rd(tracker + T_COUNT),
    };
    let cursor = cursor(tracker, &seq);
    let miss = cursor.and_then(|c| umd::sync_finds(&seq, &c, id, pos).err());
    Some(Read {
        manager,
        slot,
        tracker,
        id,
        size,
        pos,
        miss,
        cursor,
    })
}

/// `Serialize(dst, n)` in place of the game's (vtable slot 3).
unsafe extern "thiscall" fn serialize(this: *mut u8, dst: *mut u8, n: i32) {
    let original: SerializeFn = std::mem::transmute(SERIALIZE.load(Ordering::Relaxed));
    let read = if n > 0 && !dst.is_null() { look(this) } else { None };
    let Some(read) = read.filter(|r| r.miss.is_some()) else {
        return original(this, dst, n);
    };
    let len = n as usize;
    let buf = std::slice::from_raw_parts_mut(dst, len);
    let names = open_umds(read.manager);
    match serve(&read, &names, buf) {
        Ok(source) => {
            std::ptr::write_unaligned((this as usize + R_POS) as *mut i32, read.pos.wrapping_add(n));
            let count = SERVED.fetch_add(1, Ordering::Relaxed) + 1;
            if count <= LOGGED_IN_FULL {
                info!("umd-read-fix: {} -> read from {source}", describe(&read, n, &names));
            }
        }
        Err(why) => {
            let count = UNSERVED.fetch_add(1, Ordering::Relaxed) + 1;
            if count <= LOGGED_IN_FULL {
                error!("umd-read-fix: {} -> no UMD has these bytes ({why}); left to the game", describe(&read, n, &names));
            }
            original(this, dst, n);
        }
    }
}

/// The bare sync (vtable slot 4) in place of the game's: skipped for a read the sequence can't
/// serve (it would empty the cursor); the read itself then comes from the files.
unsafe extern "thiscall" fn sync_only(this: *mut u8) -> u32 {
    if look(this).is_some_and(|r| r.miss.is_some() && r.tracker != 0) {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        return 1;
    }
    let original: SyncOnlyFn = std::mem::transmute(SYNC_ONLY.load(Ordering::Relaxed));
    original(this)
}

/// `Precache(hint)` (vtable slot 22) in place of the game's: the game syncs a persistent sequence
/// first; skipped for a read it can't serve, then the archive's own `Precache` as the game does.
unsafe extern "thiscall" fn precache(this: *mut u8, hint: i32) -> u32 {
    if let Some(r) = look(this).filter(|r| r.miss.is_some() && r.tracker != 0 && rd(r.tracker + T_PERSISTENT) != 0) {
        let archive = rd(r.manager + M_ARCHIVES + 4 * r.slot);
        if archive != 0 {
            SKIPPED.fetch_add(1, Ordering::Relaxed);
            let f: PrecacheFn = std::mem::transmute(rd(rd(archive) + 4 * A_PRECACHE));
            return f(archive as *mut u8, hint);
        }
    }
    let original: PrecacheFn = std::mem::transmute(PRECACHE.load(Ordering::Relaxed));
    original(this, hint)
}

/// The open slots' UMD file names (`lobby.umd`), the current one first.
unsafe fn open_umds(manager: usize) -> Vec<(usize, String)> {
    let top = rd(manager + M_SLOT).min(SLOTS - 1);
    (0..=top)
        .rev()
        .filter(|&i| rd(manager + M_ARCHIVES + 4 * i) != 0)
        .filter_map(|i| slot_name(manager + M_NAMES + M_NAME_STRIDE * i).map(|n| (i, n)))
        .collect()
}

/// A slot's name (an engine string: pointer, length with the 0), as a UMD file name.
unsafe fn slot_name(at: usize) -> Option<String> {
    let (p, n) = (rd(at), rd(at + 4));
    if p < 0x10000 || !(2..=520).contains(&n) || !readable(p, n) {
        return None;
    }
    let bytes = std::slice::from_raw_parts(p as *const u8, n);
    // Narrow, or UTF-16 when the second byte is 0.
    let text: String = if bytes[1] == 0 {
        if !readable(p, n * 2) {
            return None;
        }
        let wide: Vec<u16> = std::slice::from_raw_parts(p as *const u16, n).iter().copied().take_while(|c| *c != 0).collect();
        String::from_utf16_lossy(&wide)
    } else {
        bytes.iter().take_while(|c| **c != 0).map(|&c| char::from(c)).collect()
    };
    let file = text.rsplit(['\\', '/']).next()?.to_ascii_lowercase();
    if file.is_empty() {
        return None;
    }
    Some(if file.ends_with(".umd") { file } else { format!("{file}.umd") })
}

fn readable(at: usize, len: usize) -> bool {
    region::query_range(at as *const u8, len).is_ok_and(|mut r| r.all(|r| r.is_ok_and(|r| r.is_readable())))
}

/// One read for the log: the file, the bytes, the current slot and its cursor, why it missed.
fn describe(read: &Read, n: i32, names: &[(usize, String)]) -> String {
    let file = store().lock().ok().and_then(|mut s| s.file_name(read.id, names)).unwrap_or_else(|| String::from("?"));
    let current = names.iter().find(|(i, _)| *i == read.slot).map_or("?", |(_, n)| n.as_str());
    let cursor = match read.cursor {
        Some(c) if c.has_block => format!(
            "{} cursor at entry {} block {} +{}",
            if c.persistent { "persistent" } else { "strict" },
            c.entry,
            c.block,
            c.offset
        ),
        Some(c) => format!("{} cursor without a block", if c.persistent { "persistent" } else { "strict" }),
        None => String::from("no tracker"),
    };
    let why = match read.miss {
        Some(Miss::NoBlock) => "the cursor has no block (read to its end, or an earlier read missed)",
        Some(Miss::NotAhead) => "not ahead of the cursor (behind it, or not recorded here)",
        Some(Miss::NotRecorded) => "not recorded in this UMD",
        Some(Miss::EmptyEntry) => "an entry without blocks on the way",
        None => "-",
    };
    let open: Vec<String> = names.iter().map(|(i, n)| format!("{i}:{n}")).collect();
    format!(
        "read off the UMD sequence: {file} ({:#010x}, {} bytes) at {} +{n}; slot {} \"{current}\", {cursor}: {why}; open: [{}]",
        read.id,
        read.size,
        read.pos,
        read.slot,
        open.join(", ")
    )
}

/// Fills `buf` with the bytes of the read from a UMD file; which one served it.
fn serve(read: &Read, names: &[(usize, String)], buf: &mut [u8]) -> Result<String, String> {
    let pos = u64::try_from(read.pos).map_err(|_| format!("position {}", read.pos))?;
    let mut store = store().lock().map_err(|_| String::from("the UMD store is poisoned"))?;
    store.serve(read.id, read.size, pos, buf, names)
}

/// The UMD files, opened and parsed as reads need them.
struct Store {
    dir: Option<PathBuf>,
    open: HashMap<String, Umd>,
    /// Use order of `open`, the latest last.
    used: Vec<String>,
    /// Every UMD's files (id, size), once a read wasn't in the open ones.
    catalog: Option<Vec<(String, HashMap<u32, u32>)>>,
}

struct Umd {
    file: umd::UmdFile,
    header: umd::Header,
    blocks: HashMap<u32, Vec<Block>>,
}

/// UMDs kept open.
const KEEP_OPEN: usize = 12;

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        let dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.join("UMDs")));
        Mutex::new(Store {
            dir,
            open: HashMap::new(),
            used: Vec::new(),
            catalog: None,
        })
    })
}

impl Store {
    fn umd(&mut self, name: &str) -> Result<&mut Umd, String> {
        if !self.open.contains_key(name) {
            let dir = self.dir.as_ref().ok_or("the game's folder is unknown")?;
            let mut file = umd::UmdFile::open(&dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
            let header = file.header().map_err(|e| format!("{name}: {e}"))?;
            let blocks = header.blocks_by_file();
            if self.used.len() >= KEEP_OPEN {
                let oldest = self.used.remove(0);
                self.open.remove(&oldest);
            }
            self.open.insert(name.to_owned(), Umd { file, header, blocks });
        }
        self.used.retain(|n| n != name);
        self.used.push(name.to_owned());
        Ok(self.open.get_mut(name).unwrap())
    }

    /// The bytes of file `id` (`size` bytes long) at `pos` from UMD `name`, if it has them all.
    fn read_from(&mut self, name: &str, id: u32, size: u32, pos: u64, buf: &mut [u8]) -> Result<bool, String> {
        let umd = self.umd(name)?;
        if umd.header.size_of(id) != Some(size) {
            return Ok(false);
        }
        let Some(pieces) = umd.blocks.get(&id).and_then(|b| umd::cover(b, pos, buf.len() as u64)) else {
            return Ok(false);
        };
        let mut done = 0usize;
        for (at, n) in pieces {
            let n = usize::try_from(n).map_err(|_| String::from("a piece past memory"))?;
            umd.file.read_at(umd.header.data_start + at, &mut buf[done..done + n]).map_err(|e| format!("{name}: {e}"))?;
            done += n;
        }
        Ok(true)
    }

    fn serve(&mut self, id: u32, size: u32, pos: u64, buf: &mut [u8], names: &[(usize, String)]) -> Result<String, String> {
        let mut tried = Vec::new();
        for (_, name) in names {
            if !tried.contains(name) {
                tried.push(name.clone());
                match self.read_from(name, id, size, pos, buf) {
                    Ok(true) => return Ok(name.clone()),
                    Ok(false) => {}
                    Err(e) => warn!("umd-read-fix: {e}"),
                }
            }
        }
        // Map-specific files (a level's generated XML) differ between UMDs under one name.
        if self.file_name(id, &[]).is_some_and(|n| n.to_ascii_lowercase().ends_with(".xml")) {
            return Err(String::from("a level's own file, not taken from another UMD"));
        }
        let candidates: Vec<String> = self
            .catalog()
            .iter()
            .filter(|(name, files)| files.get(&id) == Some(&size) && !tried.contains(name))
            .map(|(name, _)| name.clone())
            .collect();
        for name in &candidates {
            match self.read_from(name, id, size, pos, buf) {
                Ok(true) => return Ok(format!("{name} (not open)")),
                Ok(false) => {}
                Err(e) => warn!("umd-read-fix: {e}"),
            }
        }
        Err(format!("{} UMDs list the file, none has these bytes", tried.len() + candidates.len()))
    }

    /// Every UMD's files, read once (about a second for the game's 861).
    fn catalog(&mut self) -> &[(String, HashMap<u32, u32>)] {
        if self.catalog.is_none() {
            let started = Instant::now();
            let catalog = self.dir.as_deref().map(read_catalog).unwrap_or_default();
            info!("umd-read-fix: listed the files of {} UMDs in {:.1?}", catalog.len(), started.elapsed());
            self.catalog = Some(catalog);
        }
        self.catalog.as_deref().unwrap_or_default()
    }

    /// File `id`'s name, from the open UMDs or the catalog's.
    fn file_name(&mut self, id: u32, names: &[(usize, String)]) -> Option<String> {
        for (_, name) in names {
            if let Ok(umd) = self.umd(name) {
                if let Some(n) = umd.header.name_of(id) {
                    return Some(n.to_owned());
                }
            }
        }
        self.open.values().find_map(|u| u.header.name_of(id).map(str::to_owned))
    }
}

fn read_catalog(dir: &Path) -> Vec<(String, HashMap<u32, u32>)> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        warn!("umd-read-fix: couldn't list {}", dir.display());
        return Vec::new();
    };
    let mut names: Vec<String> = listing
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_ascii_lowercase())
        .filter(|n| n.ends_with(".umd"))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let header = umd::UmdFile::open(&dir.join(&name)).and_then(|mut f| f.header());
            match header {
                Ok(h) => Some((name, h.files.iter().map(|f| (f.id, f.size)).collect())),
                Err(e) => {
                    warn!("umd-read-fix: {name}: {e}");
                    None
                }
            }
        })
        .collect()
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
pub unsafe fn serve_reads_off_the_sequence() {
    let Some(image) = crate::game_image() else {
        warn!("umd-read-fix: the game's sections weren't found -> skipped");
        return;
    };
    let site = match umd_read::locate(&image) {
        Ok(site) => site,
        Err(what) => {
            warn!("umd-read-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    SERIALIZE.store(site.serialize, Ordering::Relaxed);
    SYNC_ONLY.store(site.sync_only, Ordering::Relaxed);
    PRECACHE.store(site.precache, Ordering::Relaxed);
    LINEAR_ON.store(site.linear_on, Ordering::Relaxed);
    LINEAR_OFF.store(site.linear_off, Ordering::Relaxed);
    // All three slots or none: Serialize serving a read whose bare sync still empties the
    // cursor would be half a fix. Each slot is put back if a later one can't be written.
    let slots = [
        (umd_read::SLOT_SERIALIZE, serialize as SerializeFn as usize, site.serialize),
        (umd_read::SLOT_SYNC_ONLY, sync_only as SyncOnlyFn as usize, site.sync_only),
        (umd_read::SLOT_PRECACHE, precache as PrecacheFn as usize, site.precache),
    ];
    for (done, &(slot, f, _)) in slots.iter().enumerate() {
        if !crate::writemem(site.slot(slot) as *mut u8, &(f as u32).to_le_bytes()) {
            for &(slot, _, game) in &slots[..done] {
                let _ = crate::writemem(site.slot(slot) as *mut u8, &(game as u32).to_le_bytes());
            }
            warn!("umd-read-fix: the reader's vtable couldn't be written -> skipped");
            return;
        }
    }
    info!(
        "umd-read-fix: linear package reads (vtable {:#010x}, sync {:#010x}) now take what their UMD read sequence can't serve from the UMD files",
        site.vtable, site.sync
    );

    // Every UMD's file list, read now in the background (about a second), so a read the open
    // UMDs don't have doesn't wait for it while a level loads.
    std::thread::spawn(|| {
        if let Ok(mut store) = store().lock() {
            store.catalog();
        }
    });

    std::thread::spawn(|| {
        let mut logged = (0, 0, 0);
        loop {
            std::thread::sleep(Duration::from_secs(10));
            let now = (SERVED.load(Ordering::Relaxed), UNSERVED.load(Ordering::Relaxed), SKIPPED.load(Ordering::Relaxed));
            if now != logged {
                info!(
                    "umd-read-fix: {} reads off the UMD sequence served from the files, {} no UMD had, {} syncs skipped (so far)",
                    now.0, now.1, now.2
                );
                logged = now;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn umd_name_at(text: &[u8]) -> Option<String> {
        let at = text.as_ptr() as usize;
        let mut s = [0u32; 3];
        s[0] = at as u32;
        s[1] = text.len() as u32;
        unsafe { slot_name(s.as_ptr() as usize) }
    }

    #[test]
    fn slot_names_become_umd_file_names() {
        assert_eq!(umd_name_at(b"UMDs\\PEC-ADV.umd\0").as_deref(), Some("pec-adv.umd"));
        assert_eq!(umd_name_at(b"lobby\0").as_deref(), Some("lobby.umd"));
        let wide: Vec<u8> = "UMDs\\menu.umd\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let at = wide.as_ptr() as usize;
        let s = [at as u32, 14, 14];
        assert_eq!(unsafe { slot_name(s.as_ptr() as usize) }.as_deref(), Some("menu.umd"));
        assert_eq!(umd_name_at(b"\0").as_deref(), None);
    }
}
