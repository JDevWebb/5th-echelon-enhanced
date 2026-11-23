//! Stops a crash inside XAudio2 2.7 (0.5.6's watch list: `xaudio2.dll+0x3da72`, 4 times from 3
//! players between 16 and 22 Nov 2026, in SvM rounds, a co-op mission and a lobby, all fatal).
//!
//! The game's audio applies its deferred changes (`IXAudio2::CommitChanges`, or a source voice's
//! `Stop` with an operation set); XAudio2 goes through each queued operation (`+0x3d9b0`) and, for
//! each, calls `+0x3da50`: it calls the operation's target's first virtual (`[[op+0xc]]`) and then
//! releases the operation (`+0x3daa0`). At the crash the operation's target was null (`edx = 0`
//! reading `[edx]`): an operation queued for a voice that has since gone.
//!
//! The fix: once the game has loaded `XAudio2_7.dll` (through COM, with its audio), and only when
//! the function's bytes are exactly the June 2010 runtime's (the only XAudio2 2.7 there is), its
//! first instruction (the hot-patch `mov edi,edi`, then `push ebp` / `mov ebp,esp`) becomes a jump
//! to a stub: an operation with no target is released, as the function does, and reported done
//! (S_OK), without the call through nothing. Every other operation runs the function unchanged.
//! Counted, and logged when it changes.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tracing::info;
use tracing::warn;

static STOPPED: AtomicU32 = AtomicU32::new(0);

/// The function, from the module's base, and its bytes up to the call through the target.
const APPLY: usize = 0x3da50;
const APPLY_BYTES: [u8; 40] = [
    0x8B, 0xFF, 0x55, 0x8B, 0xEC, 0x83, 0xEC, 0x08, 0x89, 0x4D, 0xF8, 0xC7, 0x45, 0xFC, 0x00, 0x00, 0x00, 0x00, 0x8B, 0x45, 0x08, 0x50, 0x8B, 0x4D, 0x08,
    0x8B, 0x51, 0x0C, 0x8B, 0x45, 0x08, 0x8B, 0x48, 0x0C, 0x8B, 0x12, 0x8B, 0x02, 0xFF, 0xD0,
];
/// The release of an operation, `thiscall(this, op)`, `ret 4`; its first bytes, checked too.
const RELEASE: usize = 0x3daa0;
const RELEASE_BYTES: [u8; 9] = [0x8B, 0xFF, 0x55, 0x8B, 0xEC, 0x51, 0x89, 0x4D, 0xFC];
/// The bytes the jump replaces (`mov edi,edi` / `push ebp` / `mov ebp,esp`).
const HOOK_LEN: usize = 5;

#[allow(clippy::cast_possible_truncation)]
fn assemble(stub: u32, release: u32, cont: u32, counter: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(64);
    let rel = |code: &Vec<u8>, target: u32| target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes();
    code.extend_from_slice(&[0x8B, 0x44, 0x24, 0x04]); // mov eax,[esp+4] (the operation)
    code.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    code.extend_from_slice(&[0x74, 0x18]); // jz run (no operation: as the game would)
    code.extend_from_slice(&[0x83, 0x78, 0x0C, 0x00]); // cmp dword [eax+0xc],0
    code.extend_from_slice(&[0x75, 0x12]); // jnz run (it has a target)
    // No target: release it (ecx is still `this`), count it, and say done.
    code.extend_from_slice(&[0x50]); // push eax (the operation)
    code.push(0xE8); // call release
    let r = rel(&code, release);
    code.extend_from_slice(&r);
    code.extend_from_slice(&[0xF0, 0xFF, 0x05]); // lock inc dword [counter]
    code.extend_from_slice(&counter.to_le_bytes());
    code.extend_from_slice(&[0x31, 0xC0]); // xor eax,eax (S_OK)
    code.extend_from_slice(&[0xC2, 0x04, 0x00]); // ret 4
    // run: the replaced bytes, then back.
    code.extend_from_slice(&APPLY_BYTES[..HOOK_LEN]);
    code.push(0xE9); // jmp cont
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    code
}

fn module_base() -> Option<usize> {
    use windows::core::PCSTR;
    use windows::Win32::System::LibraryLoader::GetModuleHandleA;
    // SAFETY: a lookup by name; nothing is loaded.
    unsafe { GetModuleHandleA(PCSTR(b"XAudio2_7.dll\0".as_ptr())) }.ok().map(|h| h.0 as usize).filter(|b| *b != 0)
}

/// Waits for the game to load XAudio2 2.7, then guards it (a thread: the game loads it with its
/// audio, after the hooks start).
pub fn guard_xaudio() {
    let spawned = std::thread::Builder::new().name("fe-xaudio-fix".into()).spawn(|| {
        let base = loop {
            if let Some(base) = module_base() {
                break base;
            }
            std::thread::sleep(Duration::from_millis(500));
        };
        // SAFETY: the module is loaded; its code is read before anything is written.
        unsafe { install(base) };
    });
    if let Err(e) = spawned {
        warn!("xaudio-fix: couldn't start ({e})");
    }
}

#[allow(clippy::cast_possible_truncation)]
unsafe fn install(base: usize) {
    let (apply, release) = (base + APPLY, base + RELEASE);
    let same = |at: usize, want: &[u8]| crate::crash::readable(at, want.len()) && std::slice::from_raw_parts(at as *const u8, want.len()) == want;
    if !same(apply, &APPLY_BYTES) || !same(release, &RELEASE_BYTES) {
        warn!("xaudio-fix: XAudio2_7.dll isn't the June 2010 runtime's as known -> skipped");
        return;
    }
    let Ok(alloc) = region::alloc(96, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("xaudio-fix: couldn't allocate the stub -> skipped");
        return;
    };
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let body = assemble(stub as u32, release as u32, (apply + HOOK_LEN) as u32, std::ptr::addr_of!(STOPPED) as u32);
    std::ptr::copy_nonoverlapping(body.as_ptr(), stub, body.len());
    std::mem::forget(alloc);
    let mut patch = [0x90u8; HOOK_LEN];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(apply as u32 + 5).to_le_bytes());
    if !crate::writemem(apply as *mut u8, &patch) {
        return;
    }
    crate::fix_stats::installed("xaudio-fix", &[&STOPPED]);
    info!("xaudio-fix: XAudio2's apply of an operation @ {apply:#010x} skips one whose voice is gone (stub {:#010x})", stub as u32);
    let mut logged = 0;
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let stopped = STOPPED.load(Ordering::Relaxed);
        if stopped != logged {
            info!("xaudio-fix: an audio operation for a voice that's gone ({stopped} so far): the game would have crashed");
            logged = stopped;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_releases_an_operation_with_no_target_and_runs_the_rest() {
        let (stub, release, cont, counter) = (0x1000_0000u32, 0x2003_daa0u32, 0x2003_da55u32, 0x3000_0000u32);
        let code = assemble(stub, release, cont, counter);
        // jz/jnz both land on the replaced bytes.
        let run = code.len() - HOOK_LEN - 5;
        assert_eq!(6 + 2 + 0x18, run, "jz run");
        assert_eq!(12 + 2 + 0x12, run, "jnz run");
        assert_eq!(&code[run..run + HOOK_LEN], &APPLY_BYTES[..HOOK_LEN]);
        let call = 15;
        assert_eq!(code[call], 0xE8);
        let r = u32::from_le_bytes(code[call + 1..call + 5].try_into().unwrap());
        assert_eq!(stub + call as u32 + 5 + r, release);
        let back = u32::from_le_bytes(code[code.len() - 4..].try_into().unwrap());
        assert_eq!(stub + code.len() as u32 + back, cont);
        assert_eq!(&code[run - 3..run], &[0xC2, 0x04, 0x00], "ret 4 before run");
    }
}
