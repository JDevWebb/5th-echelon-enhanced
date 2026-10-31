//! EPCK containers (`.umd`, `.ass`, `.asstrm`): the game's chunked zlib packing (its writer
//! is at DX11 `FUN_004e9f50`).
//!
//! `EPCK`, u32 chunk size, u32 unpacked size, u32 version 0x11, u8 flags (0x80: a chunk
//! table), u32 packed table size, u32 table size, u32 packed table size again (0: stored),
//! the table, then the chunks back to back. The table: a compact count, then per chunk a
//! compact packed size, a flag (1 zlib, 0 stored; a stored size 0 is a whole chunk) and,
//! with flags & 0x30 (the `.ass` files), 8 more bytes.

use std::io::Read as _;

use crate::compact;
use crate::Error;

pub fn unpack(raw: &[u8]) -> Result<Vec<u8>, Error> {
    let u32_at = |p: usize| raw.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap_or_default()));
    if raw.get(..4) != Some(&b"EPCK"[..]) || u32_at(12) != Some(0x11) {
        return Err(Error::Format("not an EPCK v17 file"));
    }
    let (chunk, total) = (u32_at(4).unwrap_or(0) as usize, u32_at(8).unwrap_or(0) as usize);
    let flags = raw[16];
    let (table_packed, table_size, packed_again) = (u32_at(17).unwrap_or(0) as usize, u32_at(21).unwrap_or(0) as usize, u32_at(25).unwrap_or(0));
    let mut p = 29;
    let table = if packed_again != 0 {
        inflate(raw.get(p..p + table_packed).ok_or(Error::Format("EPCK table cut"))?, table_size)?
    } else {
        raw.get(p..p + table_size).ok_or(Error::Format("EPCK table cut"))?.to_vec()
    };
    p += table_packed;
    let (count, mut q) = compact(&table, 0)?;
    let mut out = Vec::with_capacity(total);
    for _ in 0..count {
        let (size, next) = compact(&table, q)?;
        let zlib = *table.get(next).ok_or(Error::Format("EPCK table cut"))? != 0;
        q = next + 1 + if flags & 0x30 != 0 { 8 } else { 0 };
        let size = if size == 0 { chunk.min(total - out.len()) } else { size as usize };
        let data = raw.get(p..p + size).ok_or(Error::Format("EPCK chunk cut"))?;
        p += size;
        if zlib {
            out.extend(inflate(data, chunk)?);
        } else {
            out.extend_from_slice(data);
        }
        if out.len() > total {
            return Err(Error::Format("EPCK chunks past its size"));
        }
    }
    if out.len() != total {
        return Err(Error::Format("EPCK chunks short of its size"));
    }
    Ok(out)
}

fn inflate(data: &[u8], hint: usize) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(hint);
    flate2::read::ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|_| Error::Format("EPCK chunk not zlib"))?;
    Ok(out)
}
