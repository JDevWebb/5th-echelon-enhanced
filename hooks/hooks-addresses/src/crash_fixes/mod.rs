//! Where the hooks' crash fixes go in the game's code: each fix's signatures, and the checks
//! that the pieces found belong together. The hooks run these on the game as loaded
//! ([`Image`] over its sections in memory) and patch what they return; `check_crash_fixes`
//! runs them on an executable on disk, to see that every fix finds its place in a build without
//! starting the game. Nothing here writes anything.

pub mod migration;
pub mod particle;
pub mod scheme_pool;
pub mod storm;
pub mod voice_device;

use std::ops::Range;

/// The parts of the game's image the fixes read, by virtual address: `.text` and `.rdata`, and
/// the whole image's span.
pub struct Image<'a> {
    pub text: &'a [u8],
    pub text_va: usize,
    pub rdata: &'a [u8],
    pub rdata_va: usize,
    /// The image's base and `SizeOfImage`.
    pub image: Range<usize>,
}

impl<'a> Image<'a> {
    /// An executable's file, as the loader would place it at its preferred base: its pointers
    /// and sections are where they'd be in memory (a relocation would move both alike).
    pub fn from_pe(file: &'a [u8]) -> Result<Image<'a>, String> {
        let pe = goblin::pe::PE::parse(file).map_err(|e| format!("not an executable: {e}"))?;
        let base = usize::try_from(pe.image_base).map_err(|_| "an image base past this machine's addresses")?;
        let size = pe.header.optional_header.map(|o| o.windows_fields.size_of_image as usize).ok_or("no optional header")?;
        let section = |name: &[u8]| -> Result<(&'a [u8], usize), String> {
            let s = pe
                .sections
                .iter()
                .find(|s| s.name.starts_with(name) && s.name[name.len()..].iter().all(|b| *b == 0))
                .ok_or_else(|| format!("no {} section", String::from_utf8_lossy(name)))?;
            let start = s.pointer_to_raw_data as usize;
            let len = (s.size_of_raw_data as usize).min(s.virtual_size as usize);
            let bytes = file.get(start..start + len).ok_or("a section past the file's end")?;
            Ok((bytes, base + s.virtual_address as usize))
        };
        let (text, text_va) = section(b".text")?;
        let (rdata, rdata_va) = section(b".rdata")?;
        Ok(Image {
            text,
            text_va,
            rdata,
            rdata_va,
            image: base..base + size,
        })
    }

    pub fn text_range(&self) -> Range<usize> {
        self.text_va..self.text_va + self.text.len()
    }

    pub fn rdata_range(&self) -> Range<usize> {
        self.rdata_va..self.rdata_va + self.rdata.len()
    }

    /// `len` bytes at `va`, in `.text` or `.rdata`.
    pub fn bytes(&self, va: usize, len: usize) -> Option<&'a [u8]> {
        [(self.text, self.text_va), (self.rdata, self.rdata_va)]
            .into_iter()
            .find_map(|(bytes, start)| bytes.get(va.checked_sub(start)?..va.checked_sub(start)?.checked_add(len)?))
    }

    /// The `u32` at `va`, in `.text` or `.rdata`.
    pub fn u32(&self, va: usize) -> Option<usize> {
        self.bytes(va, 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
    }

    /// The target of the rel32 at `va` (in `.text`).
    pub fn rel32_target(&self, va: usize) -> Option<usize> {
        self.u32(va).map(|rel| (va + 4).wrapping_add(rel) & 0xffff_ffff)
    }

    /// Where `signature` is in `.text`, by address, if it's there exactly once; else how many
    /// times it was found.
    pub fn find(&self, signature: &[Option<u8>]) -> Result<usize, usize> {
        find_unique(self.text, signature).map(|at| self.text_va + at)
    }
}

/// Where `signature` (`None` is a wildcard) is in `code`, if it's there exactly once; else how
/// many times it was found.
pub fn find_unique(code: &[u8], signature: &[Option<u8>]) -> Result<usize, usize> {
    let matches = |w: &[u8]| w.iter().zip(signature).all(|(b, want)| want.is_none_or(|v| *b == v));
    let hits: Vec<usize> = code.windows(signature.len()).enumerate().filter(|(_, w)| matches(w)).map(|(i, _)| i).collect();
    match hits[..] {
        [at] => Ok(at),
        _ => Err(hits.len()),
    }
}

/// `"8B 56 ?? 48"` as a signature.
pub fn parse(signature: &str) -> Vec<Option<u8>> {
    signature
        .split_whitespace()
        .map(|b| if b == "??" { None } else { u8::from_str_radix(b, 16).ok() })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An image of `text` at `text_va`, with `rdata` at `rdata_va` (owned, for tests).
    pub(crate) fn image<'a>(text: &'a [u8], text_va: usize, rdata: &'a [u8], rdata_va: usize) -> Image<'a> {
        Image {
            text,
            text_va,
            rdata,
            rdata_va,
            image: text_va.min(rdata_va)..(text_va + text.len()).max(rdata_va + rdata.len()),
        }
    }

    #[test]
    fn reads_by_address_in_either_section() {
        let text = [0x90, 0x78, 0x56, 0x34, 0x12, 0xC3];
        let rdata = [0xF0, 0x00, 0x40, 0x00];
        let img = image(&text, 0x40_1000, &rdata, 0x60_0000);
        assert_eq!(img.u32(0x40_1001), Some(0x1234_5678));
        assert_eq!(img.u32(0x60_0000), Some(0x0040_00F0));
        assert_eq!(img.u32(0x40_1003), None, "runs past .text");
        assert_eq!(img.u32(0x50_0000), None, "in neither");
        assert_eq!(img.find(&[Some(0x12), None]), Ok(0x40_1004));
        assert_eq!(img.find(&[Some(0x77)]), Err(0));
    }

    #[test]
    fn parses_signatures() {
        assert_eq!(parse("8B ?? 48"), [Some(0x8B), None, Some(0x48)]);
    }
}
