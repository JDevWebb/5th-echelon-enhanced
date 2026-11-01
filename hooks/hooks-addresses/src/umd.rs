//! The game's UMD files (`UMDs\*.umd`) and the read sequence each records, for the hooks'
//! `umd_read_fix`. Nothing here touches the game; it reads the files on disk.
//!
//! The game ships no loose packages. A UMD holds the bytes the game read from its packages while
//! a level (or a content pack) loaded when the game was built, back to back in the order they
//! were read, behind a header that says which bytes they are:
//! - a build string;
//! - the files: id (a hash of the file name), name, path, size;
//! - the sequence: entries, each a file id and blocks (offset in the file, offset in the UMD's
//!   data, length), in the order the bytes follow each other in the data;
//! - then the data, from the header's end rounded up to 128 KiB.
//!
//! Most UMDs are EPCK packages around that: the header and the data unpack from zlib chunks
//! listed in a table. The two PEC content packs (`PEC-ADV.umd`, `PEC-Main.umd`) are plain.
//!
//! The game reads a package through the current UMD's sequence with a cursor (see
//! [`sync_finds`]); the same package bytes are recorded alike in every UMD that has them (same
//! build, same file sizes), so a read the sequence can't serve can be served from any UMD that
//! recorded those bytes ([`cover`]).

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;

/// The data starts at the header's end rounded up to this.
pub const DATA_ALIGN: u64 = 0x2_0000;
/// The largest header read (the biggest in the game is about 210 KiB).
const MAX_HEADER: usize = 16 << 20;
/// Unpacked chunks kept by a [`UmdFile`].
const CHUNKS_KEPT: usize = 4;

/// A run of one file's bytes in a UMD: where it is in the file, where in the UMD's data, how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub file: u32,
    pub umd: u32,
    pub len: u32,
}

/// One step of the sequence: a file and the blocks read from it, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: u32,
    pub blocks: Vec<Block>,
}

/// A file a UMD has bytes of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub id: u32,
    pub name: String,
    pub size: u32,
}

/// A UMD's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub build: String,
    pub files: Vec<FileInfo>,
    pub entries: Vec<Entry>,
    /// Where the data starts in the (unpacked) UMD.
    pub data_start: u64,
}

impl Header {
    /// The size of file `id`, if this UMD lists it.
    pub fn size_of(&self, id: u32) -> Option<u32> {
        self.files.iter().find(|f| f.id == id).map(|f| f.size)
    }

    /// The name of file `id`, if this UMD lists it.
    pub fn name_of(&self, id: u32) -> Option<&str> {
        self.files.iter().find(|f| f.id == id).map(|f| f.name.as_str())
    }

    /// Every block of file `id`, by file: what [`cover`] searches.
    pub fn blocks_by_file(&self) -> HashMap<u32, Vec<Block>> {
        let mut by_file: HashMap<u32, Vec<Block>> = HashMap::new();
        for e in &self.entries {
            by_file.entry(e.id).or_default().extend_from_slice(&e.blocks);
        }
        by_file
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// The bytes end inside the header: try again with more.
    Short,
    Bad(String),
}

struct Bytes<'a> {
    d: &'a [u8],
    p: usize,
}

impl Bytes<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], HeaderError> {
        let s = self.d.get(self.p..self.p.checked_add(n).ok_or(HeaderError::Short)?).ok_or(HeaderError::Short)?;
        self.p += n;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, HeaderError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// The engine's compact integer: sign and 6 bits, then 7 bits a byte while the top bit is set.
    fn compact(&mut self) -> Result<i32, HeaderError> {
        let b = self.take(1)?[0];
        let mut v = u32::from(b & 0x3f);
        if b & 0x40 != 0 {
            let mut shift = 6;
            loop {
                let b = self.take(1)?[0];
                if shift > 27 {
                    return Err(HeaderError::Bad(String::from("a compact integer longer than 5 bytes")));
                }
                v |= u32::from(b & 0x7f) << shift;
                shift += 7;
                if b & 0x80 == 0 {
                    break;
                }
            }
        }
        let v = i32::try_from(v).map_err(|_| HeaderError::Bad(String::from("a compact integer past i32")))?;
        Ok(if b & 0x80 != 0 { -v } else { v })
    }

    fn count(&mut self, what: &str) -> Result<usize, HeaderError> {
        let n = self.compact()?;
        usize::try_from(n)
            .ok()
            .filter(|n| *n <= 1 << 24)
            .ok_or_else(|| HeaderError::Bad(format!("{what}: count {n}")))
    }

    /// A string: its length (with the terminating 0) as a compact integer, negative for UTF-16.
    fn string(&mut self) -> Result<String, HeaderError> {
        let n = self.compact()?;
        if n < 0 {
            let units = self.take(usize::try_from(-n).unwrap_or(0) * 2)?;
            let wide: Vec<u16> = units.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
            return Ok(String::from_utf16_lossy(&wide));
        }
        let b = self.take(usize::try_from(n).unwrap_or(0))?;
        let b = b.split(|c| *c == 0).next().unwrap_or_default();
        Ok(b.iter().map(|&c| char::from(c)).collect())
    }
}

/// The header at the start of `d` (an unpacked UMD, or enough of its start).
pub fn parse_header(d: &[u8]) -> Result<Header, HeaderError> {
    let mut r = Bytes { d, p: 0 };
    let build = r.string()?;
    let n = r.count("files")?;
    let mut files = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let id = r.u32()?;
        let name = r.string()?;
        let _path = r.string()?;
        let size = r.compact()?;
        let size = u32::try_from(size).map_err(|_| HeaderError::Bad(format!("{name}: size {size}")))?;
        files.push(FileInfo { id, name, size });
    }
    let n = r.count("entries")?;
    let mut entries = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let id = r.u32()?;
        let k = r.count("blocks")?;
        let mut blocks = Vec::with_capacity(k.min(1 << 16));
        for _ in 0..k {
            let mut field = || -> Result<u32, HeaderError> {
                let v = r.compact()?;
                u32::try_from(v).map_err(|_| HeaderError::Bad(format!("a block field {v}")))
            };
            let (file, umd, len) = (field()?, field()?, field()?);
            blocks.push(Block { file, umd, len });
        }
        entries.push(Entry { id, blocks });
    }
    let end = r.p as u64;
    Ok(Header {
        build,
        files,
        entries,
        data_start: end.div_ceil(DATA_ALIGN) * DATA_ALIGN,
    })
}

/// An EPCK package's chunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packing {
    pub chunk_size: u64,
    pub total: u64,
    /// Per chunk: where it starts in the file, its packed length, whether it's zlib (else stored).
    pub chunks: Vec<(u64, u64, bool)>,
}

impl Packing {
    /// The chunk table of an EPCK file, `None` for a plain UMD. `file_len` bounds what it may claim.
    pub fn read(f: &mut (impl Read + Seek), file_len: u64) -> io::Result<Option<Packing>> {
        let bad = |what: String| io::Error::new(io::ErrorKind::InvalidData, what);
        f.seek(SeekFrom::Start(0))?;
        let mut head = [0u8; 29];
        let got = read_up_to(f, &mut head)?;
        if got < 4 || &head[..4] != b"EPCK" {
            return Ok(None);
        }
        if got < 17 {
            return Err(bad(String::from("EPCK header cut short")));
        }
        let u32_at = |i: usize| u64::from(u32::from_le_bytes(head[i..i + 4].try_into().unwrap()));
        let (chunk_size, total, flags) = (u32_at(4), u32_at(8), head[16]);
        if flags & 0x80 == 0 || got < 29 {
            return Err(bad(format!("EPCK without an inline chunk table (flags {flags:#x})")));
        }
        let (stored, size, packed) = (u32_at(17), u32_at(21), u32_at(25));
        if chunk_size == 0 || chunk_size > 64 << 20 || stored > 64 << 20 || size > 64 << 20 || total > 1 << 32 {
            return Err(bad(String::from("implausible EPCK chunk table")));
        }
        let mut raw = vec![0u8; usize::try_from(if packed != 0 { packed } else { size }).unwrap_or(0)];
        f.read_exact(&mut raw)?;
        let table = if packed != 0 { inflate(&raw, usize::try_from(size).unwrap_or(0))? } else { raw };
        let mut r = Bytes { d: &table, p: 0 };
        let n = r.count("chunks").map_err(|e| bad(format!("chunk table: {e:?}")))?;
        let mut chunks = Vec::with_capacity(n.min(1 << 20));
        let (mut at, mut unpacked) = (29 + stored, 0u64);
        for _ in 0..n {
            let len = r.compact().map_err(|e| bad(format!("chunk table: {e:?}")))?;
            let _kind = r.take(1).map_err(|e| bad(format!("chunk table: {e:?}")))?;
            if flags & 0x30 != 0 {
                r.take(8).map_err(|e| bad(format!("chunk table: {e:?}")))?;
            }
            let full = chunk_size.min(total.saturating_sub(unpacked));
            // Length 0: a whole chunk, stored as it is.
            let (len, zlib) = match u64::try_from(len) {
                Ok(0) => (full, false),
                Ok(len) if len <= full + 4096 => (len, true),
                _ => return Err(bad(format!("a chunk of {len} bytes"))),
            };
            chunks.push((at, len, zlib));
            at += len;
            unpacked += full;
        }
        if unpacked != total || at > file_len {
            return Err(bad(format!("chunks add up to {unpacked} of {total} bytes, ending at {at} of {file_len}")));
        }
        Ok(Some(Packing { chunk_size, total, chunks }))
    }
}

fn read_up_to(f: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(got)
}

/// zlib, refusing to grow past `limit`.
fn inflate(raw: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(limit);
    flate2::read::ZlibDecoder::new(raw).take(limit as u64 + 1).read_to_end(&mut out)?;
    if out.len() != limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a chunk unpacks to {} bytes, not {limit}", out.len())));
    }
    Ok(out)
}

/// A UMD file, read by offset in its unpacked bytes.
pub struct UmdFile {
    file: File,
    len: u64,
    packing: Option<Packing>,
    /// Recently unpacked chunks, the latest last.
    kept: Vec<(usize, Vec<u8>)>,
}

impl UmdFile {
    pub fn open(path: &Path) -> io::Result<UmdFile> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let packing = Packing::read(&mut file, len)?;
        Ok(UmdFile {
            file,
            len,
            packing,
            kept: Vec::new(),
        })
    }

    /// The unpacked size.
    pub fn len(&self) -> u64 {
        self.packing.as_ref().map_or(self.len, |p| p.total)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills `buf` from unpacked offset `at`.
    pub fn read_at(&mut self, at: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = at.checked_add(buf.len() as u64).filter(|e| *e <= self.len());
        if end.is_none() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("{} bytes at {at} past {}", buf.len(), self.len())));
        }
        let Some(chunk_size) = self.packing.as_ref().map(|p| p.chunk_size) else {
            self.file.seek(SeekFrom::Start(at))?;
            return self.file.read_exact(buf);
        };
        let mut done = 0usize;
        while done < buf.len() {
            let pos = at + done as u64;
            let i = usize::try_from(pos / chunk_size).unwrap_or(usize::MAX);
            let from = usize::try_from(pos % chunk_size).unwrap_or(0);
            let chunk = self.chunk(i)?;
            let n = (chunk.len().saturating_sub(from)).min(buf.len() - done);
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("chunk {i} ends early")));
            }
            buf[done..done + n].copy_from_slice(&chunk[from..from + n]);
            done += n;
        }
        Ok(())
    }

    fn chunk(&mut self, i: usize) -> io::Result<&[u8]> {
        if let Some(k) = self.kept.iter().position(|(c, _)| *c == i) {
            let hit = self.kept.remove(k);
            self.kept.push(hit);
        } else {
            let p = self.packing.as_ref().expect("only packed UMDs have chunks");
            let &(at, len, zlib) = p.chunks.get(i).ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, format!("no chunk {i}")))?;
            let full = usize::try_from(p.chunk_size.min(p.total - i as u64 * p.chunk_size)).unwrap_or(0);
            let mut raw = vec![0u8; usize::try_from(len).unwrap_or(0)];
            self.file.seek(SeekFrom::Start(at))?;
            self.file.read_exact(&mut raw)?;
            let data = if zlib { inflate(&raw, full)? } else { raw };
            if self.kept.len() >= CHUNKS_KEPT {
                self.kept.remove(0);
            }
            self.kept.push((i, data));
        }
        Ok(&self.kept.last().unwrap().1)
    }

    /// The header, read in growing steps until it parses.
    pub fn header(&mut self) -> io::Result<Header> {
        let mut want = 256 << 10;
        loop {
            let n = usize::try_from(self.len()).unwrap_or(usize::MAX).min(want).min(MAX_HEADER);
            let mut buf = vec![0u8; n];
            self.read_at(0, &mut buf)?;
            match parse_header(&buf) {
                Ok(h) => return Ok(h),
                Err(HeaderError::Short) if n < MAX_HEADER && (n as u64) < self.len() => want *= 4,
                Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("UMD header: {e:?}"))),
            }
        }
    }
}

/// A read sequence as the game holds it (the tracker's entry table), by index.
pub trait Sequence {
    fn entries(&self) -> usize;
    fn id(&self, entry: usize) -> u32;
    fn blocks(&self, entry: usize) -> usize;
    fn block(&self, entry: usize, block: usize) -> Block;
}

impl Sequence for [Entry] {
    fn entries(&self) -> usize {
        self.len()
    }
    fn id(&self, entry: usize) -> u32 {
        self[entry].id
    }
    fn blocks(&self, entry: usize) -> usize {
        self[entry].blocks.len()
    }
    fn block(&self, entry: usize, block: usize) -> Block {
        self[entry].blocks[block]
    }
}

/// Where the game's cursor on a sequence stands: entry, block and offset in it (the tracker's
/// `+0x2c`, `+0x30`, `+0x34`), whether it has a current block at all (`+0x3c` set), and whether
/// the UMD is persistent (`+0x44`: searched in full, else read strictly forward).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub entry: usize,
    pub block: usize,
    pub offset: u32,
    pub has_block: bool,
    pub persistent: bool,
}

/// Why the game's sync wouldn't find a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    /// The cursor has no block: the sequence was read to its end, or an earlier read missed.
    NoBlock,
    /// Strict: the bytes aren't anywhere ahead of the cursor (behind it, or not recorded).
    NotAhead,
    /// Persistent: the bytes aren't recorded in this UMD at all.
    NotRecorded,
    /// The sequence has an entry without blocks on the way (the game would read past it).
    EmptyEntry,
}

/// Whether the cursor's current block serves `pos` of `id`: the read is where the cursor stands,
/// or further on in the same block (the game skips forward to it).
fn here(seq: &(impl Sequence + ?Sized), entry: usize, block: usize, offset: u32, id: u32, pos: i32) -> bool {
    if seq.id(entry) != id {
        return false;
    }
    let b = seq.block(entry, block);
    let expected = (b.file as i32).wrapping_add(offset as i32);
    pos == expected || (expected < pos && pos < (b.file as i32).wrapping_add(b.len as i32))
}

/// What the game's sync (DX11 `FUN_004504b0`) does with a read of `pos` in file `id` from
/// `cursor`, without doing it: `Ok` where it finds the read (the cursor moves there and the read
/// gets the right bytes), else why not. On a miss the game leaves the cursor without a block, the
/// read gets whatever follows in the UMD, and the next read through it dereferences the missing
/// block (`exe+0x504e8`).
///
/// Strict (a level's UMD): the current block, then each later block in order, until one of `id`'s
/// holds `pos` (the bytes skipped are passed by). Persistent (a PEC pack): the current block, then
/// every block of `id` anywhere in the UMD.
pub fn sync_finds(seq: &(impl Sequence + ?Sized), cursor: &Cursor, id: u32, pos: i32) -> Result<(), Miss> {
    if !cursor.has_block {
        return Err(Miss::NoBlock);
    }
    let (mut e, mut b, mut off) = (cursor.entry, cursor.block, cursor.offset);
    if e >= seq.entries() || b >= seq.blocks(e) {
        return Err(Miss::NoBlock);
    }
    if cursor.persistent {
        if here(seq, e, b, off, id, pos) {
            return Ok(());
        }
        let found = (0..seq.entries()).filter(|&e| seq.id(e) == id).any(|e| {
            (0..seq.blocks(e)).any(|b| {
                let blk = seq.block(e, b);
                (blk.file as i32) <= pos && pos < (blk.file as i32).wrapping_add(blk.len as i32)
            })
        });
        return if found { Ok(()) } else { Err(Miss::NotRecorded) };
    }
    loop {
        if here(seq, e, b, off, id, pos) {
            return Ok(());
        }
        off = 0;
        b += 1;
        if b == seq.blocks(e) {
            e += 1;
            b = 0;
            if e >= seq.entries() {
                return Err(Miss::NotAhead);
            }
            if seq.blocks(e) == 0 {
                return Err(Miss::EmptyEntry);
            }
        }
    }
}

/// Where file bytes `pos..pos + len` are in a UMD's data, given every block of that file in it:
/// (offset in the data, length) in order, or `None` when a byte of them isn't recorded.
pub fn cover(blocks: &[Block], pos: u64, len: u64) -> Option<Vec<(u64, u64)>> {
    let end = pos.checked_add(len)?;
    let mut pieces = Vec::new();
    let mut at = pos;
    while at < end {
        // Of the blocks holding `at`, the one reaching furthest.
        let b = blocks
            .iter()
            .filter(|b| u64::from(b.file) <= at && at < u64::from(b.file) + u64::from(b.len))
            .max_by_key(|b| u64::from(b.file) + u64::from(b.len))?;
        let n = (u64::from(b.file) + u64::from(b.len)).min(end) - at;
        pieces.push((u64::from(b.umd) + (at - u64::from(b.file)), n));
        at += n;
    }
    Some(pieces)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn compact(v: i32) -> Vec<u8> {
        let neg = v < 0;
        let mut v = v.unsigned_abs();
        let mut out = vec![(v & 0x3f) as u8 | if neg { 0x80 } else { 0 }];
        v >>= 6;
        if v != 0 {
            out[0] |= 0x40;
            loop {
                let mut b = (v & 0x7f) as u8;
                v >>= 7;
                if v != 0 {
                    b |= 0x80;
                }
                out.push(b);
                if v == 0 {
                    break;
                }
            }
        }
        out
    }

    fn string(s: &str) -> Vec<u8> {
        let mut out = compact(s.len() as i32 + 1);
        out.extend_from_slice(s.as_bytes());
        out.push(0);
        out
    }

    /// A UMD's header and data, as the game's files have them (unpacked).
    fn umd(files: &[(u32, &str, u32)], entries: &[(u32, &[(u32, u32, u32)])], data: &[u8]) -> Vec<u8> {
        let mut h = string("SC6_pcbranch_V2423.0_C603454");
        h.extend(compact(files.len() as i32));
        for (id, name, size) in files {
            h.extend_from_slice(&id.to_le_bytes());
            h.extend(string(name));
            h.extend(string(&format!("x\\{name}")));
            h.extend(compact(*size as i32));
        }
        h.extend(compact(entries.len() as i32));
        for (id, blocks) in entries {
            h.extend_from_slice(&id.to_le_bytes());
            h.extend(compact(blocks.len() as i32));
            for (a, b, c) in *blocks {
                h.extend(compact(*a as i32));
                h.extend(compact(*b as i32));
                h.extend(compact(*c as i32));
            }
        }
        h.resize(DATA_ALIGN as usize, 0);
        h.extend_from_slice(data);
        h
    }

    /// `raw` as an EPCK package with chunks of `chunk` bytes, the table zlib-packed or not.
    fn epck(raw: &[u8], chunk: usize, pack_table: bool) -> Vec<u8> {
        let mut table = compact(raw.len().div_ceil(chunk) as i32);
        let mut data = Vec::new();
        for (i, c) in raw.chunks(chunk).enumerate() {
            if i % 3 == 1 && c.len() == chunk {
                table.extend(compact(0));
                table.push(0);
                data.extend_from_slice(c);
            } else {
                let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                z.write_all(c).unwrap();
                let z = z.finish().unwrap();
                table.extend(compact(z.len() as i32));
                table.push(0x2c);
                data.extend(z);
            }
        }
        let mut out = b"EPCK".to_vec();
        out.extend_from_slice(&(chunk as u32).to_le_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x11u32.to_le_bytes());
        out.push(0x81);
        let stored = if pack_table {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(&table).unwrap();
            z.finish().unwrap()
        } else {
            table.clone()
        };
        out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        out.extend_from_slice(&(table.len() as u32).to_le_bytes());
        out.extend_from_slice(&(if pack_table { stored.len() as u32 } else { 0 }).to_le_bytes());
        out.extend(stored);
        out.extend(data);
        out
    }

    #[test]
    fn compact_integers_round_trip() {
        for v in [0, 1, 63, 64, 8191, 8192, 1 << 20, 17_140_664, -5] {
            let b = compact(v);
            assert_eq!(Bytes { d: &b, p: 0 }.compact(), Ok(v), "{v}");
        }
        // As in lobby.umd: 0x47 0xa5 0xca 0x04 is 4_802_887 (EchelonEffect.u's size).
        assert_eq!(
            Bytes {
                d: &[0x47, 0xa5, 0xca, 0x04],
                p: 0
            }
            .compact(),
            Ok(4_802_887)
        );
    }

    #[test]
    fn parses_a_header() {
        let data = vec![7u8; 100];
        let raw = umd(
            &[(0x6522_d35c, "WEAPM-TwoHanded.ukx", 17_151_186)],
            &[(0x6522_d35c, &[(0, 0, 60), (539_808, 60, 40)])],
            &data,
        );
        let h = parse_header(&raw).unwrap();
        assert_eq!(h.build, "SC6_pcbranch_V2423.0_C603454");
        assert_eq!(h.size_of(0x6522_d35c), Some(17_151_186));
        assert_eq!(h.name_of(0x6522_d35c), Some("WEAPM-TwoHanded.ukx"));
        assert_eq!(h.entries[0].blocks[1], Block { file: 539_808, umd: 60, len: 40 });
        assert_eq!(h.data_start, DATA_ALIGN);
        assert_eq!(parse_header(&raw[..20]), Err(HeaderError::Short));
    }

    #[test]
    fn reads_packed_and_plain_alike() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let raw = umd(&[(1, "a.u", 300_000)], &[(1, &[(0, 0, 300_000)])], &data);
        let dir = std::env::temp_dir().join(format!("umd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in [
            ("plain.umd", raw.clone()),
            ("packed.umd", epck(&raw, 0x2_0000, false)),
            ("packed-table.umd", epck(&raw, 0x8000, true)),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, &bytes).unwrap();
            let mut f = UmdFile::open(&path).unwrap();
            assert_eq!(f.len(), raw.len() as u64, "{name}");
            let h = f.header().unwrap();
            assert_eq!(h.entries.len(), 1, "{name}");
            let mut buf = vec![0u8; 70_000];
            f.read_at(h.data_start + 123_456, &mut buf).unwrap();
            assert_eq!(buf, data[123_456..193_456], "{name}");
            f.read_at(h.data_start + 5, &mut buf[..10]).unwrap();
            assert_eq!(buf[..10], data[5..15], "{name}: back again");
            assert!(f.read_at(raw.len() as u64 - 4, &mut buf[..10]).is_err(), "{name}: past the end");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// lobby.umd's first steps: lobby.unr's header, WEAPM-TwoHanded.ukx's, a texture package's;
    /// later the weapon mesh (539808) in two parts with other packages read in between.
    fn lobby() -> Vec<Entry> {
        let e = |id: u32, b: &[(u32, u32, u32)]| Entry {
            id,
            blocks: b.iter().map(|&(file, umd, len)| Block { file, umd, len }).collect(),
        };
        vec![
            e(0x0fde_49fc, &[(0, 0, 64), (31759, 64, 4)]),
            e(0x6522_d35c, &[(0, 68, 13702), (17_140_664, 13770, 10522)]),
            e(0x17c6_c507, &[(0, 24292, 7837)]),
            e(0x0fde_49fc, &[(14306, 32129, 52006)]),
            e(0x6522_d35c, &[(539_808, 84135, 5086)]),
            e(0x0fde_49fc, &[(78425, 89221, 88)]),
            e(0x6522_d35c, &[(544_894, 89309, 22)]),
        ]
    }

    const LOBBY: u32 = 0x0fde_49fc;
    const WEAPM: u32 = 0x6522_d35c;

    fn at(entry: usize, block: usize, offset: u32) -> Cursor {
        Cursor {
            entry,
            block,
            offset,
            has_block: true,
            persistent: false,
        }
    }

    #[test]
    fn strict_sync_finds_reads_in_place_and_ahead() {
        let seq = lobby();
        // Where the cursor stands, and further on in its block.
        assert_eq!(sync_finds(&seq[..], &at(1, 0, 100), WEAPM, 100), Ok(()));
        assert_eq!(sync_finds(&seq[..], &at(1, 0, 100), WEAPM, 5000), Ok(()));
        // A later block: the game skips what lies between (here lobby.unr's exports).
        assert_eq!(sync_finds(&seq[..], &at(1, 0, 100), WEAPM, 539_808), Ok(()));
        // A package whose header was read when it wasn't in memory yet, read again from the start.
        assert_eq!(sync_finds(&seq[..], &at(3, 0, 0), WEAPM, 0), Err(Miss::NotAhead));
        // The mesh's first part again after its second part was reached.
        assert_eq!(sync_finds(&seq[..], &at(6, 0, 0), WEAPM, 539_900), Err(Miss::NotAhead));
        // Bytes no step recorded.
        assert_eq!(sync_finds(&seq[..], &at(0, 0, 0), WEAPM, 600_000), Err(Miss::NotAhead));
        // A file the UMD doesn't have at all (a reader made under another UMD).
        assert_eq!(sync_finds(&seq[..], &at(0, 0, 0), 0xdead_beef, 0), Err(Miss::NotAhead));
        // Behind in the current block: the game passes it by.
        assert_eq!(sync_finds(&seq[..], &at(3, 0, 1000), LOBBY, 14306), Err(Miss::NotAhead));
    }

    #[test]
    fn a_cursor_without_a_block_finds_nothing() {
        let seq = lobby();
        let mut c = at(6, 0, 22);
        c.has_block = false;
        assert_eq!(sync_finds(&seq[..], &c, WEAPM, 544_894), Err(Miss::NoBlock));
        c.persistent = true;
        assert_eq!(sync_finds(&seq[..], &c, WEAPM, 544_894), Err(Miss::NoBlock));
    }

    #[test]
    fn persistent_sync_searches_the_whole_umd() {
        let seq = lobby();
        let c = Cursor { persistent: true, ..at(6, 0, 0) };
        assert_eq!(sync_finds(&seq[..], &c, WEAPM, 0), Ok(()), "behind the cursor");
        assert_eq!(sync_finds(&seq[..], &c, LOBBY, 14306 + 52005), Ok(()), "a block's last byte");
        assert_eq!(sync_finds(&seq[..], &c, LOBBY, 14306 + 52006), Err(Miss::NotRecorded), "just past it");
        assert_eq!(sync_finds(&seq[..], &c, 0xdead_beef, 0), Err(Miss::NotRecorded));
    }

    #[test]
    fn covers_reads_from_blocks_in_any_order() {
        let blocks = [
            Block {
                file: 544_894,
                umd: 9000,
                len: 22,
            },
            Block {
                file: 539_808,
                umd: 1000,
                len: 5086,
            },
            Block {
                file: 539_808,
                umd: 3000,
                len: 100,
            },
        ];
        assert_eq!(cover(&blocks, 539_808, 4), Some(vec![(1000, 4)]));
        assert_eq!(cover(&blocks, 544_890, 10), Some(vec![(1000 + 5082, 4), (9000, 6)]), "across two blocks");
        assert_eq!(cover(&blocks, 544_910, 10), None, "past what was recorded");
        assert_eq!(cover(&blocks, 100, 1), None);
        assert_eq!(cover(&blocks, 539_900, 0), Some(vec![]));
    }

    /// Against the game's own files: `SCBL_UMDS=<game>/src/SYSTEM/UMDs cargo test -- --ignored`.
    /// lobby.umd records the weapon mesh at 539808 of WEAPM-TwoHanded.ukx in two parts, the other
    /// UMDs in one, and the bytes are the same in each.
    #[test]
    #[ignore]
    fn the_game_files_agree() {
        let Some(dir) = std::env::var_os("SCBL_UMDS") else { return };
        let dir = Path::new(&dir);
        let mut read = |name: &str| -> Vec<u8> {
            let mut f = UmdFile::open(&dir.join(name)).unwrap();
            let h = f.header().unwrap();
            let blocks = h.blocks_by_file().remove(&WEAPM).unwrap();
            let pieces = cover(&blocks, 539_808, 5108).unwrap();
            assert_eq!(pieces.len(), if name == "lobby.umd" { 2 } else { 1 }, "{name}");
            let mut out = Vec::new();
            for (at, n) in pieces {
                let mut buf = vec![0u8; n as usize];
                f.read_at(h.data_start + at, &mut buf).unwrap();
                out.extend(buf);
            }
            out
        };
        let lobby = read("lobby.umd");
        for other in ["menu.umd", "ADV_01.umd", "PEC-Main.umd"] {
            assert_eq!(read(other), lobby, "{other}");
        }
    }
}
