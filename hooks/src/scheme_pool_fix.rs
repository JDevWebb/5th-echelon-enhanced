//! Stops a pawn's teardown from destroying the level's shared action-scheme pool: a crash in
//! Spies vs Mercs matches with more than 8 players (and a double free at map end).
//!
//! At level load the game pre-allocates `m_iMaxAllowedPlayers` (8) pawn state slots
//! (`EchelonLevelInfo.m_PreAllocatedStatePool`) and one shared `UActionSchemePool`
//! (`m_AdvActionSchemePool`, its `m_bSharedPool` bit set: `[pool+0x38]` bit 0). Every remote pawn's
//! `m_SchemePool` (`pawn+0xbd0`) points at the shared pool; a pawn that got a slot has the pointer
//! cleared again when it gives the slot back. A pawn with no slot (the 9th and later) doesn't, and
//! its teardown then empties and deletes `[pawn+0xbd0]` without checking `m_bSharedPool`: the
//! level's shared pool. Every later remote spawn walks freed memory, which crashes once the heap
//! reuses it (`Blacklist_DX11_game.exe+0xa7f3e4`), and the level frees it a second time when the
//! map ends.
//!
//! The fix: the teardown's `mov ecx,[ebx+0xbd0] / cmp ecx,edi / jz <skip>` (14 bytes, right after
//! `mov esi,0x400 / xor edi,edi`) becomes a jump to a small stub that does the same test and, for
//! a shared pool, only clears the pawn's pointer and skips the teardown, exactly as the slot
//! release does for a pawn with a slot. Any other pool takes the original path unchanged. At
//! `<skip>` only EBX, ESI (0x400) and EDI (0) are live, as on the original path. Three earlier
//! branches of the same function jump straight to the replaced bytes; each comes after an
//! identical `mov esi,0x400 / xor edi,edi`, so the stub's `test ecx,ecx` is the game's
//! `cmp ecx,edi` on every path.
//!
//! The site is matched by a signature that is unique in both executables (DX11 VA `0xd5e514`,
//! DX9 VA `0xff40e4`) and the build is left alone when it isn't found exactly once. The stub
//! counts what it kept, and a thread logs the count when it changes.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tracing::info;
use tracing::warn;

/// `mov esi,0x400 / xor edi,edi / mov ecx,[ebx+0xbd0] / cmp ecx,edi / jz rel32 / call rel32 /
/// mov eax,[ebx+0xbd0] / cmp [abs32],edi`; `None` is a wildcard.
const SIGNATURE: [Option<u8>; 34] = {
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
const HOOK_AT: usize = 7;
const HOOK_LEN: usize = 14;
/// The `jz` rel32 and the instruction after it (the call), relative to the signature.
const JZ_REL: usize = 17;
const CONTINUE_AT: usize = 21;

/// Shared pools kept from a pawn's teardown since the game started.
static KEPT: AtomicU32 = AtomicU32::new(0);

/// Assembles the stub at `stub` (its final address) for the given absolute targets.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn assemble(stub: u32, skip: u32, cont: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(48);
    let rel = |code: &Vec<u8>, target: u32| target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes();
    code.extend_from_slice(&[0x8B, 0x8B, 0xD0, 0x0B, 0x00, 0x00]); // mov ecx,[ebx+0xbd0]
    code.extend_from_slice(&[0x85, 0xC9]); // test ecx,ecx
    code.extend_from_slice(&[0x0F, 0x84]); // jz skip
    let r = rel(&code, skip);
    code.extend_from_slice(&r);
    code.extend_from_slice(&[0xF6, 0x41, 0x38, 0x01]); // test byte [ecx+0x38],1 (m_bSharedPool)
    code.extend_from_slice(&[0x75, 0x05]); // jnz shared
    code.push(0xE9); // jmp cont (the original path, ECX = the pool)
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    // shared:
    code.extend_from_slice(&[0xC7, 0x83, 0xD0, 0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // mov dword [ebx+0xbd0],0
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    code.push(0xE9); // jmp skip
    let r = rel(&code, skip);
    code.extend_from_slice(&r);
    code
}

/// Where the signature is, if it's found exactly once.
fn find(code: &[u8]) -> Result<usize, usize> {
    let matches = |w: &[u8]| w.iter().zip(SIGNATURE).all(|(b, want)| want.is_none_or(|v| *b == v));
    let hits: Vec<usize> = code.windows(SIGNATURE.len()).enumerate().filter(|(_, w)| matches(w)).map(|(i, _)| i).collect();
    match hits[..] {
        [at] => Ok(at),
        _ => Err(hits.len()),
    }
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
pub unsafe fn keep_shared_scheme_pool() {
    let Some(code) = crate::game_code() else {
        warn!("scheme-pool-fix: the game's .text wasn't found -> skipped");
        return;
    };
    let at = match find(code) {
        Ok(at) => at,
        Err(n) => {
            warn!("scheme-pool-fix: the pawn teardown was found {n} times, not once -> skipped (unknown build?)");
            return;
        }
    };
    let site = code.as_ptr().add(at);
    let hook = site.add(HOOK_AT) as u32;
    let jz_next = site.add(JZ_REL + 4) as u32;
    let skip = jz_next.wrapping_add(std::ptr::read_unaligned(site.add(JZ_REL).cast::<u32>()));
    let cont = site.add(CONTINUE_AT) as u32;

    let Ok(alloc) = region::alloc(64, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("scheme-pool-fix: couldn't allocate the stub -> skipped");
        return;
    };
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let body = assemble(stub as u32, skip, cont, std::ptr::addr_of!(KEPT) as u32);
    std::ptr::copy_nonoverlapping(body.as_ptr(), stub, body.len());
    // The stub lives as long as the game.
    std::mem::forget(alloc);

    let mut patch = [0x90u8; HOOK_LEN];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(hook + 5).to_le_bytes());
    crate::writemem(hook as *mut u8, &patch);
    info!(
        "scheme-pool-fix: pawn teardown @ {hook:#010x} now keeps the shared action-scheme pool (stub {:#010x}, skip {skip:#010x})",
        stub as u32
    );

    std::thread::spawn(|| {
        let mut logged = 0;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let kept = KEPT.load(Ordering::Relaxed);
            if kept != logged {
                info!("scheme-pool-fix: kept the shared action-scheme pool from a leaving pawn's teardown ({kept} so far): a pawn beyond the level's state slots left");
                logged = kept;
            }
        }
    });
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
        assert_eq!(find(&dx11_site()), Ok(8));
        let mut twice = dx11_site();
        twice.extend(dx11_site());
        assert_eq!(find(&twice), Err(2));
        assert_eq!(find(&[0x90; 64]), Err(0));
    }

    #[test]
    fn stub_jumps_where_the_original_did() {
        // DX11: skip 0xd5e5f9, continue 0xd5e529.
        let stub = 0x1000_0000;
        let code = assemble(stub, 0xd5e5f9, 0xd5e529, 0x2000_0000);
        let target = |at: usize| (stub + at as u32 + 4).wrapping_add(u32::from_le_bytes(code[at..at + 4].try_into().unwrap()));
        assert_eq!(&code[0..6], &[0x8B, 0x8B, 0xD0, 0x0B, 0x00, 0x00]);
        assert_eq!(&code[6..10], &[0x85, 0xC9, 0x0F, 0x84]);
        assert_eq!(target(10), 0xd5e5f9, "null pool: skip");
        assert_eq!(&code[14..20], &[0xF6, 0x41, 0x38, 0x01, 0x75, 0x05]);
        assert_eq!(code[20], 0xE9);
        assert_eq!(target(21), 0xd5e529, "private pool: original path");
        // jnz lands right after the jmp.
        assert_eq!(&code[25..35], &[0xC7, 0x83, 0xD0, 0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(&code[35..38], &[0xF0, 0xFF, 0x05]);
        assert_eq!(u32::from_le_bytes(code[38..42].try_into().unwrap()), 0x2000_0000);
        assert_eq!(code[42], 0xE9);
        assert_eq!(target(43), 0xd5e5f9, "shared pool: skip");
        assert_eq!(code.len(), 47);
    }
}
