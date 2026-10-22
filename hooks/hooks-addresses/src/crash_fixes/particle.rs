//! The particle thread's queueing of a particle fill, whose vertex stream a map load can free
//! first (the hooks' `particle_fix` has the whole story).

use super::Image;

/// `lea eax,[eax*4+counts] / push eax / lea ecx,[ebp-4] / push ecx / lea eax,[edx*4+streams] /
/// push eax / push <fill> / push 0 / call <enqueue>`; `None` is a wildcard.
#[rustfmt::skip]
pub const SIGNATURE: [Option<u8>; 32] = [
    Some(0x8D), Some(0x04), Some(0x85), None, None, None, None, // lea eax,[eax*4+counts]
    Some(0x50), // push eax
    Some(0x8D), Some(0x4D), Some(0xFC), // lea ecx,[ebp-4]
    Some(0x51), // push ecx
    Some(0x8D), Some(0x04), Some(0x95), None, None, None, None, // lea eax,[edx*4+streams]
    Some(0x50), // push eax
    Some(0x68), None, None, None, None, // push <fill>
    Some(0x6A), Some(0x00), // push 0
    Some(0xE8), None, None, None, None, // call <enqueue>
];

/// Where `&streams[0]` and the fill's address sit in the signature.
const STREAMS_AT: usize = 15;
const FILL_AT: usize = 21;

/// The fill's first bytes: `push ebp / mov ebp,esp / push ecx / push ebx / push edi /
/// mov edi,[ebp+8] / xor ebx,ebx / cmp edi,ebx / jz`.
pub const FILL_PROLOGUE: [u8; 15] = [0x55, 0x8B, 0xEC, 0x51, 0x53, 0x57, 0x8B, 0x7D, 0x08, 0x33, 0xDB, 0x3B, 0xFB, 0x0F, 0x84];

/// Where the guard goes.
#[derive(Debug)]
pub struct Site {
    /// The `push <fill>` immediate, rewritten to the guard.
    pub fill_slot: usize,
    /// The game's fill, checked by its prologue.
    pub fill: usize,
    /// `&streams[0]` (two live vertex streams), in the game's image.
    pub streams: usize,
}

pub fn locate(img: &Image) -> Result<Site, String> {
    let at = img.find(&SIGNATURE).map_err(|n| format!("the particle fill's queueing was found {n} times, not once"))?;
    let streams = img.u32(at + STREAMS_AT).ok_or("the vertex streams' address")?;
    let fill = img.u32(at + FILL_AT).ok_or("the queued function's address")?;
    let text = img.text_range();
    if !text.contains(&fill) || !text.contains(&(fill + FILL_PROLOGUE.len() - 1)) {
        return Err(format!("the queued function {fill:#010x} isn't in the game's code"));
    }
    if img.bytes(fill, FILL_PROLOGUE.len()) != Some(&FILL_PROLOGUE[..]) {
        return Err(format!("the queued function {fill:#010x} isn't the particle fill"));
    }
    if streams < img.image.start || streams + 8 > img.image.end {
        return Err(format!("the vertex streams {streams:#010x} aren't in the game's image"));
    }
    Ok(Site {
        fill_slot: at + FILL_AT,
        fill,
        streams,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DX11: the queueing in the particle thread's batcher.
    const SITE: [u8; 32] = [
        0x8D, 0x04, 0x85, 0xEC, 0xEE, 0x3E, 0x03, 0x50, 0x8D, 0x4D, 0xFC, 0x51, 0x8D, 0x04, 0x95, 0xC4, 0xEE, 0x3E, 0x03, 0x50, 0x68, 0xC0, 0xA9, 0x61, 0x01, 0x6A, 0x00, 0xE8,
        0x97, 0xFC, 0xFF, 0xFF,
    ];

    #[test]
    fn finds_the_site_once() {
        let mut code = vec![0xCC; 5];
        code.extend_from_slice(&SITE);
        assert_eq!(super::super::find_unique(&code, &SIGNATURE), Ok(5));
        assert_eq!(u32::from_le_bytes(code[5 + STREAMS_AT..5 + STREAMS_AT + 4].try_into().unwrap()), 0x033e_eec4);
        assert_eq!(u32::from_le_bytes(code[5 + FILL_AT..5 + FILL_AT + 4].try_into().unwrap()), 0x0161_a9c0);
        code.extend_from_slice(&SITE);
        assert_eq!(super::super::find_unique(&code, &SIGNATURE), Err(2));
    }

    #[test]
    fn the_queued_function_must_be_the_fill() {
        // A fake .text from 0x0161_a900 with the site at its start and the fill at 0x0161_a9c0.
        let mut code = vec![0xCC; 0x100];
        code[..32].copy_from_slice(&SITE);
        code[0xc0..0xc0 + FILL_PROLOGUE.len()].copy_from_slice(&FILL_PROLOGUE);
        let mut img = super::super::tests::image(&code, 0x0161_a900, &[], 0x0300_0000);
        img.image = 0x0040_0000..0x0400_0000;
        let site = locate(&img).unwrap();
        assert_eq!((site.fill_slot, site.fill, site.streams), (0x0161_a915, 0x0161_a9c0, 0x033e_eec4));

        code[0xc0] = 0x90;
        let img = super::super::tests::image(&code, 0x0161_a900, &[], 0x0300_0000);
        assert!(locate(&img).unwrap_err().contains("isn't the particle fill"));
    }
}
