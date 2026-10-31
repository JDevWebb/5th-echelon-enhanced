//! A map's `.umd` once unpacked: the "linear load" bundle (DX11 `FUN_004609a0` and on). An
//! FString build, the files (u32 hash, FString name, FString path, compact size), the read
//! groups (u32 file hash, then compact segments: file offset, stream position, size), padding
//! to a 128 KiB boundary, then the segments' bytes in read order. A file is rebuilt from its
//! segments; what no segment covers stays zero (the game reads only what it needs).

use crate::compact;
use crate::Error;

pub struct File {
    pub hash: u32,
    pub path: String,
    pub size: usize,
}

pub struct Bundle<'a> {
    pub data: &'a [u8],
    pub files: Vec<File>,
    groups: Vec<(u32, Vec<(i64, i64, i64)>)>,
    pub start: usize,
}

fn fstring(d: &[u8], p: usize) -> Result<(String, usize), Error> {
    let (n, p) = compact(d, p)?;
    let n = usize::try_from(n).map_err(|_| Error::Format("bad string"))?;
    let bytes = d.get(p..p + n).ok_or(Error::Format("string cut"))?;
    let text: String = bytes.iter().take_while(|b| **b != 0).map(|b| char::from(*b)).collect();
    Ok((text, p + n))
}

fn u32_at(d: &[u8], p: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(d.get(p..p + 4).ok_or(Error::Format("cut"))?.try_into().unwrap_or_default()))
}

impl<'a> Bundle<'a> {
    pub fn read(data: &'a [u8]) -> Result<Self, Error> {
        let (_, mut p) = fstring(data, 0)?;
        let (n, q) = compact(data, p)?;
        p = q;
        let mut files = Vec::new();
        for _ in 0..n {
            let hash = u32_at(data, p)?;
            let (name, q) = fstring(data, p + 4)?;
            let (path, q) = fstring(data, q)?;
            let (size, q) = compact(data, q)?;
            p = q;
            files.push(File {
                hash,
                path: if path.is_empty() { name } else { path },
                size: usize::try_from(size).map_err(|_| Error::Format("bad size"))?,
            });
        }
        let (n, q) = compact(data, p)?;
        p = q;
        let mut groups = Vec::new();
        for _ in 0..n {
            let hash = u32_at(data, p)?;
            let (k, q) = compact(data, p + 4)?;
            p = q;
            let mut segs = Vec::new();
            for _ in 0..k {
                let (a, q) = compact(data, p)?;
                let (b, q) = compact(data, q)?;
                let (c, q) = compact(data, q)?;
                p = q;
                segs.push((a, b, c));
            }
            groups.push((hash, segs));
        }
        Ok(Self {
            data,
            files,
            groups,
            start: (p + 0x1ffff) & !0x1ffff,
        })
    }

    /// A file's bytes as the game reads them, and which are there.
    pub fn rebuild(&self, f: &File) -> (Vec<u8>, Vec<bool>) {
        let mut buf = vec![0u8; f.size];
        let mut covered = vec![false; f.size];
        for (_, segs) in self.groups.iter().filter(|(h, _)| *h == f.hash) {
            for &(off, pos, n) in segs {
                let (Ok(off), Ok(pos), Ok(n)) = (usize::try_from(off), usize::try_from(pos), usize::try_from(n)) else {
                    continue;
                };
                if off >= f.size {
                    continue;
                }
                let end = (off + n).min(f.size);
                let Some(src) = self.data.get(self.start + pos..self.start + pos + (end - off)) else {
                    continue;
                };
                buf[off..end].copy_from_slice(src);
                covered[off..end].fill(true);
            }
        }
        (buf, covered)
    }
}
