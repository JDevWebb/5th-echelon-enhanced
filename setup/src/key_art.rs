//! Banner art for the launcher, from the player's own copy of the game: the
//! loading screens in `dynamicflash.umd`. Nothing from the game ships with
//! the launcher; the art is read where it's installed, each time.
//!
//! `dynamicflash.umd` is an EPCK package: zlib chunks listed in a table, which
//! unpack to the Flash UI's images as DDS files back to back. The loading
//! screens are the last 2048x1024 DXT5 images in it, their art in the top
//! 2048x857 and white below. Only the chunks at the end are unpacked.

use std::fs::File;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;

use anyhow::bail;
use anyhow::Context;

/// The package with the loading screens, in the game folder.
pub const PACKAGE: &str = "dynamicflash.umd";
/// How much of the end of the unpacked package is searched: the loading
/// screens take about 60 MiB of it.
const REGION: u64 = 64 << 20;
/// The loading screens: the last this many 2048x1024 images. The ones before
/// them are map plans and the world map, which make poor banners.
const SCREENS: usize = 28;
const WIDTH: usize = 2048;
const HEIGHT: usize = 1024;
const DDS_HEADER: usize = 128;

/// A decoded image, RGBA, row by row.
pub struct Art {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Loading screen number `pick` (counted round the ones found), cropped to
/// its art and halved to 1024 wide.
pub fn load(game_dir: &Path, pick: usize) -> anyhow::Result<Art> {
    let mut file = File::open(game_dir.join(PACKAGE)).with_context(|| format!("no {PACKAGE}"))?;
    let package = Package::read(&mut file)?;
    let (start, data) = package.unpack_tail(&mut file, REGION)?;
    let screens = find_screens(&data);
    if screens.is_empty() {
        bail!("no loading screens at the end of {PACKAGE} (unpacked from {start})");
    }
    let screens = &screens[screens.len().saturating_sub(SCREENS)..];
    let at = screens[pick % screens.len()];
    let image = &data[at + DDS_HEADER..at + DDS_HEADER + WIDTH * HEIGHT];
    let full = decode_bc3(image, WIDTH, HEIGHT);
    let height = content_height(&full, WIDTH, HEIGHT);
    Ok(halve(&full, WIDTH, height))
}

/// The chunk table of an EPCK package (see the RE notes).
struct Package {
    chunk_size: u64,
    total: u64,
    /// Per chunk: where it starts in the file, its packed size, whether it's
    /// zlib (else stored), and where it starts unpacked.
    chunks: Vec<(u64, u64, bool, u64)>,
}

impl Package {
    fn read(file: &mut File) -> anyhow::Result<Self> {
        let mut head = [0u8; 29];
        file.read_exact(&mut head)?;
        let u32_at = |i: usize| u64::from(u32::from_le_bytes(head[i..i + 4].try_into().unwrap_or_default()));
        if &head[..4] != b"EPCK" || u32_at(12) != 0x11 {
            bail!("{PACKAGE} isn't an EPCK v17 package");
        }
        let (chunk_size, total, flags) = (u32_at(4), u32_at(8), head[16]);
        let (packed, size, packed_again) = (u32_at(17), u32_at(21), u32_at(25));
        if chunk_size == 0 || chunk_size > 64 << 20 || packed > 64 << 20 || size > 64 << 20 {
            bail!("{PACKAGE} has an implausible chunk table");
        }
        let mut raw = vec![0u8; usize::try_from(packed)?];
        file.read_exact(&mut raw)?;
        let table = if packed_again != 0 { inflate(&raw, usize::try_from(size)?)? } else { raw };
        let mut q = 0;
        let count = read_compact(&table, &mut q)?;
        let mut chunks = Vec::with_capacity(usize::try_from(count.min(1 << 20))?);
        let (mut at, mut unpacked) = (29 + packed, 0u64);
        for _ in 0..count {
            let packed = read_compact(&table, &mut q)?;
            let zlib = *table.get(q).context("chunk table cut short")? != 0;
            q += 1;
            if flags & 0x30 != 0 {
                q += 8;
            }
            let full = chunk_size.min(total.saturating_sub(unpacked));
            // A stored chunk of size 0 is a whole raw chunk.
            let packed = if packed == 0 { full } else { packed };
            // Packed, a chunk is about its unpacked size: anything far bigger is a broken
            // table, not memory to set aside.
            if packed > full + 4096 {
                bail!("{PACKAGE}'s chunk of {packed} bytes is larger than a chunk");
            }
            chunks.push((at, packed, zlib, unpacked));
            at += packed;
            unpacked += full;
        }
        if unpacked != total {
            bail!("{PACKAGE}'s chunks add up to {unpacked} bytes, not {total}");
        }
        Ok(Self { chunk_size, total, chunks })
    }

    /// The last `region` bytes or so of the unpacked package (from a chunk
    /// boundary), and where they start.
    fn unpack_tail(&self, file: &mut File, region: u64) -> anyhow::Result<(u64, Vec<u8>)> {
        let from = self.total.saturating_sub(region);
        let first = self.chunks.iter().position(|c| c.3 + self.chunk_size > from).unwrap_or(self.chunks.len());
        let start = self.chunks.get(first).map_or(self.total, |c| c.3);
        let mut out = Vec::with_capacity(usize::try_from(self.total - start)?);
        for &(at, packed, zlib, _) in &self.chunks[first..] {
            let mut raw = vec![0u8; usize::try_from(packed)?];
            file.seek(SeekFrom::Start(at))?;
            file.read_exact(&mut raw)?;
            if zlib {
                out.extend_from_slice(&inflate(&raw, usize::try_from(self.chunk_size)?)?);
            } else {
                out.extend_from_slice(&raw);
            }
        }
        Ok((start, out))
    }
}

/// zlib, refusing to grow past `limit` (a chunk never unpacks to more).
fn inflate(raw: &[u8], limit: usize) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(limit);
    flate2::read::ZlibDecoder::new(raw).take(limit as u64 + 1).read_to_end(&mut out)?;
    if out.len() > limit {
        bail!("a chunk of {PACKAGE} unpacks past its size");
    }
    Ok(out)
}

/// The package's compact integers (6 bits, then 7 per byte while the top bit is set).
fn read_compact(d: &[u8], p: &mut usize) -> anyhow::Result<u64> {
    let mut next = || -> anyhow::Result<u8> {
        let b = *d.get(*p).context("chunk table cut short")?;
        *p += 1;
        Ok(b)
    };
    let b = next()?;
    let mut v = u64::from(b & 0x3f);
    if b & 0x40 != 0 {
        let mut shift = 6;
        loop {
            let b = next()?;
            if shift > 56 {
                bail!("a compact integer in the chunk table is too long");
            }
            v |= u64::from(b & 0x7f) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
    }
    if b & 0x80 != 0 {
        bail!("a negative count in the chunk table");
    }
    Ok(v)
}

/// Where the 2048x1024 DXT5 images in `data` start, in order.
fn find_screens(data: &[u8]) -> Vec<usize> {
    const MAGIC: &[u8] = b"DDS \x7c\x00\x00\x00";
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(i) = data[at..].windows(MAGIC.len()).position(|w| w == MAGIC) {
        let o = at + i;
        let u32_at = |p: usize| data.get(o + p..o + p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap_or_default()));
        let fits = o + DDS_HEADER + WIDTH * HEIGHT <= data.len();
        if fits && u32_at(12) == Some(HEIGHT as u32) && u32_at(16) == Some(WIDTH as u32) && data.get(o + 84..o + 88) == Some(b"DXT5") {
            found.push(o);
            at = o + DDS_HEADER + WIDTH * HEIGHT;
        } else {
            at = o + MAGIC.len();
        }
    }
    found
}

/// BC3 (DXT5) to RGBA. The alpha blocks are skipped: the art is opaque.
fn decode_bc3(blocks: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![255u8; width * height * 4];
    for (n, block) in blocks.chunks_exact(16).take((width / 4) * (height / 4)).enumerate() {
        let (bx, by) = (n % (width / 4), n / (width / 4));
        let colors = bc1_palette(&block[8..12]);
        let bits = u32::from_le_bytes(block[12..16].try_into().unwrap_or_default());
        for i in 0..16 {
            let c = colors[((bits >> (2 * i)) & 3) as usize];
            let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
            let p = (y * width + x) * 4;
            out[p..p + 3].copy_from_slice(&c);
        }
    }
    out
}

/// The four colours of a BC1 colour block (always the four-colour form in BC3).
fn bc1_palette(endpoints: &[u8]) -> [[u8; 3]; 4] {
    let rgb = |c: u16| {
        let (r, g, b) = (u32::from(c >> 11), u32::from((c >> 5) & 63), u32::from(c & 31));
        [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]
    };
    let c0 = rgb(u16::from_le_bytes([endpoints[0], endpoints[1]]));
    let c1 = rgb(u16::from_le_bytes([endpoints[2], endpoints[3]]));
    let mix = |a: u8, b: u8, wa: u32, wb: u32| ((u32::from(a) * wa + u32::from(b) * wb) / 3) as u8;
    let third = |wa, wb| [mix(c0[0], c1[0], wa, wb), mix(c0[1], c1[1], wa, wb), mix(c0[2], c1[2], wa, wb)];
    [c0, c1, third(2, 1), third(1, 2)]
}

/// The rows down to the last one that isn't flat (the white padding below
/// the art is), at least half the image.
fn content_height(rgba: &[u8], width: usize, height: usize) -> usize {
    let flat = |y: usize| {
        let row = &rgba[y * width * 4..(y + 1) * width * 4];
        let first = &row[..3];
        row.chunks_exact(4).all(|p| p[..3].iter().zip(first).all(|(a, b)| a.abs_diff(*b) <= 8))
    };
    let mut h = height;
    while h > height / 2 && flat(h - 1) {
        h -= 1;
    }
    h
}

/// Half the size, each pixel the mean of four (keeps the texture small).
fn halve(rgba: &[u8], width: usize, height: usize) -> Art {
    let (w, h) = (width / 2, height / 2);
    let mut out = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let at = |dx: usize, dy: usize| u32::from(rgba[((2 * y + dy) * width + 2 * x + dx) * 4 + c]);
                out[(y * w + x) * 4 + c] = ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) / 4) as u8;
            }
        }
    }
    Art { width: w, height: h, rgba: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_integers() {
        let mut p = 0;
        assert_eq!(read_compact(&[0x05], &mut p).unwrap(), 5);
        // 0x40 = more follows: 6 low bits, then 7.
        let mut p = 0;
        assert_eq!(read_compact(&[0x41, 0x02], &mut p).unwrap(), 1 | (2 << 6));
        let mut p = 0;
        assert!(read_compact(&[0x41], &mut p).is_err());
    }

    #[test]
    fn bc3_block() {
        // Endpoints white and black; every texel index 1 (black) but the first (white).
        let mut block = [0u8; 16];
        block[8..12].copy_from_slice(&[0xff, 0xff, 0x00, 0x00]);
        block[12..16].copy_from_slice(&0x5555_5554u32.to_le_bytes());
        let px = decode_bc3(&block, 4, 4);
        assert_eq!(&px[..4], &[255, 255, 255, 255]);
        assert_eq!(&px[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    fn crops_flat_rows() {
        let (w, h) = (4, 8);
        let mut rgba = vec![255u8; w * h * 4];
        rgba[0] = 0; // row 0 varies
        rgba[3 * w * 4] = 10; // row 3 varies a little more than the tolerance
        assert_eq!(content_height(&rgba, w, h), 4);
    }

    /// Against a real install: `FE_GAME_DIR=<SYSTEM folder> cargo test -p setup real_install -- --ignored`.
    /// Writes the picked screen to the temp folder as a PPM to look at.
    #[test]
    #[ignore = "needs the game"]
    fn real_install() {
        let dir = std::path::PathBuf::from(std::env::var("FE_GAME_DIR").expect("FE_GAME_DIR"));
        let started = std::time::Instant::now();
        let art = load(&dir, 3).unwrap();
        eprintln!("{}x{} in {:?}", art.width, art.height, started.elapsed());
        assert_eq!(art.width, WIDTH / 2);
        assert!(art.height > 400 && art.height <= HEIGHT / 2);
        let mut ppm = format!("P6 {} {} 255\n", art.width, art.height).into_bytes();
        ppm.extend(art.rgba.chunks_exact(4).flat_map(|p| p[..3].to_vec()));
        std::fs::write(std::env::temp_dir().join("key-art.ppm"), ppm).unwrap();
    }

    #[test]
    fn finds_screens() {
        let mut data = vec![0u8; 64];
        let mut dds = vec![0u8; DDS_HEADER + WIDTH * HEIGHT];
        dds[..8].copy_from_slice(b"DDS \x7c\x00\x00\x00");
        dds[12..16].copy_from_slice(&(HEIGHT as u32).to_le_bytes());
        dds[16..20].copy_from_slice(&(WIDTH as u32).to_le_bytes());
        dds[84..88].copy_from_slice(b"DXT5");
        data.extend_from_slice(&dds);
        data.extend_from_slice(&dds);
        assert_eq!(find_screens(&data), vec![64, 64 + dds.len()]);
        // One cut short isn't taken.
        data.truncate(data.len() - 1);
        assert_eq!(find_screens(&data), vec![64]);
    }
}
