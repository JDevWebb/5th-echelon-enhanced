//! The pawn teardown that destroys the level's shared action-scheme pool (the hooks'
//! `scheme_pool_fix` has the whole story).

use super::Image;

/// `mov esi,0x400 / xor edi,edi / mov ecx,[ebx+0xbd0] / cmp ecx,edi / jz rel32 / call rel32 /
/// mov eax,[ebx+0xbd0] / cmp [abs32],edi`; `None` is a wildcard.
pub const SIGNATURE: [Option<u8>; 34] = {
    const fn b(v: u8) -> Option<u8> {
        Some(v)
    }
    const W: Option<u8> = None;
    [
        b(0xBE),
        b(0x00),
        b(0x04),
        b(0x00),
        b(0x00), // mov esi,0x400
        b(0x33),
        b(0xFF), // xor edi,edi
        b(0x8B),
        b(0x8B),
        b(0xD0),
        b(0x0B),
        b(0x00),
        b(0x00), // mov ecx,[ebx+0xbd0]
        b(0x3B),
        b(0xCF), // cmp ecx,edi
        b(0x0F),
        b(0x84),
        W,
        W,
        W,
        W, // jz <skip>
        b(0xE8),
        W,
        W,
        W,
        W, // call <empty the pool>
        b(0x8B),
        b(0x83),
        b(0xD0),
        b(0x0B),
        b(0x00),
        b(0x00), // mov eax,[ebx+0xbd0]
        b(0x39),
        b(0x3D), // cmp [abs32],edi
    ]
};

/// Where the replaced instructions start in the signature, and how many bytes they take.
pub const HOOK_AT: usize = 7;
pub const HOOK_LEN: usize = 14;
/// The `jz` rel32 and the instruction after it (the call), relative to the signature.
const JZ_REL: usize = 17;
const CONTINUE_AT: usize = 21;

/// Where the fix goes: the replaced instructions, and where the original's two ways lead.
#[derive(Debug)]
pub struct Site {
    /// The first of the [`HOOK_LEN`] bytes replaced.
    pub hook: usize,
    /// The `jz`'s target (the teardown skipped).
    pub skip: usize,
    /// The instruction after it (the teardown).
    pub cont: usize,
}

pub fn locate(img: &Image) -> Result<Site, String> {
    let at = img.find(&SIGNATURE).map_err(|n| format!("the pawn teardown was found {n} times, not once"))?;
    let skip = img.rel32_target(at + JZ_REL).ok_or("the teardown's jz")?;
    Ok(Site {
        hook: at + HOOK_AT,
        skip,
        cont: at + CONTINUE_AT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DX11 bytes around the site (the signature plus a few bytes either side).
    fn dx11_site() -> Vec<u8> {
        let mut code = vec![0xCC; 8];
        code.extend_from_slice(&[
            0xBE, 0x00, 0x04, 0x00, 0x00, 0x33, 0xFF, 0x8B, 0x8B, 0xD0, 0x0B, 0x00, 0x00, 0x3B, 0xCF, 0x0F, 0x84, 0xD0, 0x00, 0x00, 0x00, 0xE8, 0x12, 0x0F, 0x12, 0x00, 0x8B, 0x83,
            0xD0, 0x0B, 0x00, 0x00, 0x39, 0x3D, 0x98, 0xC1, 0x30, 0x03,
        ]);
        code.extend_from_slice(&[0xCC; 8]);
        code
    }

    #[test]
    fn finds_the_site_once() {
        assert_eq!(super::super::find_unique(&dx11_site(), &SIGNATURE), Ok(8));
        let mut twice = dx11_site();
        twice.extend(dx11_site());
        assert_eq!(super::super::find_unique(&twice, &SIGNATURE), Err(2));
        assert_eq!(super::super::find_unique(&[0x90; 64], &SIGNATURE), Err(0));
    }

    #[test]
    fn locates_the_dx11_site() {
        // The signature at DX11 VA 0xd5e514 (8 bytes in): skip 0xd5e5f9, continue 0xd5e529.
        let code = dx11_site();
        let img = super::super::tests::image(&code, 0xd5e50c, &[], 0x0300_0000);
        let site = locate(&img).unwrap();
        assert_eq!((site.hook, site.skip, site.cont), (0xd5e51b, 0xd5e5f9, 0xd5e529));
    }
}
