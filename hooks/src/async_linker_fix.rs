//! A crash in the game's async package loading: `Blacklist_DX11_game.exe+0x121efb` (Gex, 13 Nov,
//! 2 h into a session on Silo) and `Blacklist_game.exe+0x3ccf0b` (tokealotasmoke, 14 Nov, DX9,
//! in the lobby after a match on Silo, 66 min in), reading `0x9c`.
//!
//! Each frame the game's async package tick ("LOAD", DX11 `0x521cb0`) works on one entry of its
//! queue (0x54 bytes each: `+0` the package's linker, `+4`/`+8` its name, `+0x34` "creating
//! the linker"). Creating the linker (DX11 `0x4dabf0`, by name) can return nothing, and the
//! tick then reads `[linker+0x9c]` unchecked. The linker is read in five more places on the
//! entry's way through the tick, so going on without one isn't possible.
//!
//! The fix: right after the creation the read (`mov eax,[ecx+edx]` / `add esp,0x14`, 6 bytes)
//! jumps to a stub. With a linker, on as before. Without one, it asks [`missing_linker`], which
//! logs the package's name (the first time, and when it gives up), clears the entry's
//! "creating" flag so the next frame creates the linker again, and has the tick leave through
//! its own "out of time this frame" (which reads no linker). A package whose linker is still
//! missing after [`MAX_TRIES`] tries or [`MAX_WAIT`] goes on as the game always did: the same
//! crash, with the package named in the log just before it. So a passing failure (a file the
//! game couldn't open for a moment) no longer crashes the game, and one that doesn't pass
//! names the package for the next look.
//!
//! The site is matched by one signature unique in both executables (DX11 `0x521ef5`, DX9
//! `0x7ccf05`, through the entry's name and the creating call), and the "out of time" is
//! checked at its place after it; the build is left alone otherwise.

use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use hooks_addresses::crash_fixes::async_linker;
use hooks_addresses::crash_fixes::async_linker::HOOK_LEN;
use hooks_addresses::crash_fixes::async_linker::MOVES;
use tracing::info;
use tracing::warn;

/// Tries a package gets before the game goes on as it always did.
const MAX_TRIES: u32 = 300;
const MAX_WAIT: Duration = Duration::from_secs(10);
/// The entry's "creating the linker" flag.
const CREATING: usize = 0x34;

/// Packages whose linker came back empty (each once), since the game started.
static MISSING: AtomicU32 = AtomicU32::new(0);

struct Tries {
    first: Instant,
    count: u32,
}

static TRIES: Mutex<Option<HashMap<String, Tries>>> = Mutex::new(None);

/// What to do with a package whose linker came back empty, after `tries` tries over `waited`:
/// try again, or go on as the game always did.
fn try_again(tries: u32, waited: Duration) -> bool {
    tries <= MAX_TRIES && waited <= MAX_WAIT
}

/// The entry's name: its string (`+4` the characters, one byte each, `+8` their count with the
/// 0), as the gamebox showed it: `maps\hub\stream\HUB_Global_13_str.umd`.
unsafe fn entry_name(entry: *const u8) -> String {
    let chars = *(entry.add(4) as *const *const u8);
    let count = *(entry.add(8) as *const i32);
    if chars.is_null() || count <= 1 {
        return String::new();
    }
    #[allow(clippy::cast_sign_loss)]
    let bytes = std::slice::from_raw_parts(chars, (count as usize - 1).min(260));
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Called by the stub with the queue's current entry when its linker came back empty; 1: the
/// tick leaves for this frame and the linker is created again next frame, 0: on as before.
unsafe extern "C" fn missing_linker(entry: *mut u8) -> u32 {
    if entry.is_null() {
        return 0;
    }
    let name = entry_name(entry);
    let mut tries = TRIES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let t = tries
        .get_or_insert_with(HashMap::new)
        .entry(name.clone())
        .or_insert_with(|| Tries { first: Instant::now(), count: 0 });
    t.count += 1;
    let (count, waited) = (t.count, t.first.elapsed());
    drop(tries);
    if count == 1 {
        MISSING.fetch_add(1, Ordering::Relaxed);
        warn!("async-linker-fix: package {name:?} came back with no linker: trying again next frame (the game would have crashed)");
    }
    if try_again(count, waited) {
        *(entry.add(CREATING) as *mut u32) = 0;
        1
    } else {
        warn!(
            "async-linker-fix: package {name:?} still has no linker after {count} tries in {:.1} s: the game goes on as it always did (a crash)",
            waited.as_secs_f32()
        );
        0
    }
}

/// The stub at `stub`: the replaced bytes, then a linker goes on to `cont`; none asks `ask`
/// (cdecl, the entry in ESI, every register kept but EAX) and goes to `stop` on 1, to `cont`
/// with EAX 0 on 0. With `force` (a test build's counter), while it's above 0 a linker is taken
/// away from the entry as if its creation had failed, and the counter goes down.
fn assemble(stub: u32, cont: u32, stop: u32, ask: u32, force: Option<u32>) -> Vec<u8> {
    let mut code = Vec::with_capacity(64);
    let rel = |code: &Vec<u8>, target: u32| {
        #[allow(clippy::cast_possible_truncation)]
        target.wrapping_sub(stub + code.len() as u32 + 4).to_le_bytes()
    };
    code.extend_from_slice(&MOVES);
    if let Some(force) = force {
        code.extend_from_slice(&[0x83, 0x3D]); // cmp dword [force],0
        code.extend_from_slice(&force.to_le_bytes());
        code.push(0x00);
        code.extend_from_slice(&[0x74, 0x0E]); // je on
        code.extend_from_slice(&[0xFF, 0x0D]); // dec dword [force]
        code.extend_from_slice(&force.to_le_bytes());
        code.extend_from_slice(&[0x31, 0xC0]); // xor eax,eax
        code.extend_from_slice(&[0xC7, 0x06, 0, 0, 0, 0]); // mov dword [esi],0 (the entry's linker)
                                                           // on:
    }
    code.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    code.extend_from_slice(&[0x74, 0x05]); // jz none
    code.push(0xE9); // jmp cont
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    // none:
    code.push(0x60); // pushad
    code.push(0x56); // push esi
    code.push(0xE8); // call ask
    let r = rel(&code, ask);
    code.extend_from_slice(&r);
    code.extend_from_slice(&[0x83, 0xC4, 0x04]); // add esp,4
    code.extend_from_slice(&[0x89, 0x44, 0x24, 0x1C]); // mov [esp+0x1c],eax (pushad's EAX)
    code.push(0x61); // popad
    code.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    code.extend_from_slice(&[0x0F, 0x84]); // jz cont (EAX 0: on as before)
    let r = rel(&code, cont);
    code.extend_from_slice(&r);
    code.push(0xE9); // jmp stop
    let r = rel(&code, stop);
    code.extend_from_slice(&r);
    code
}

/// Installs the fix. Safe to call once at startup; leaves an unrecognised build untouched.
#[allow(clippy::cast_possible_truncation)]
pub unsafe fn guard_async_linker() {
    let Some(image) = crate::game_image() else {
        warn!("async-linker-fix: the game's sections weren't found -> skipped");
        return;
    };
    let s = match async_linker::locate(&image) {
        Ok(s) => s,
        Err(what) => {
            warn!("async-linker-fix: {what} -> skipped (unknown build?)");
            return;
        }
    };
    let site = s.site as u32;
    if std::slice::from_raw_parts(site as *const u8, HOOK_LEN) != MOVES {
        warn!("async-linker-fix: the bytes at {site:#010x} aren't as found -> stopped");
        return;
    }
    let Ok(alloc) = region::alloc(64, region::Protection::READ_WRITE_EXECUTE) else {
        warn!("async-linker-fix: couldn't allocate a stub -> stopped");
        return;
    };
    let stub = alloc.as_ptr::<u8>().cast_mut();
    let ask = missing_linker as unsafe extern "C" fn(*mut u8) -> u32 as usize as u32;
    #[cfg(feature = "test-commands")]
    let force = Some(std::ptr::addr_of!(FORCE) as u32);
    #[cfg(not(feature = "test-commands"))]
    let force = None;
    let code = assemble(stub as u32, s.cont as u32, s.stop as u32, ask, force);
    std::ptr::copy_nonoverlapping(code.as_ptr(), stub, code.len());
    std::mem::forget(alloc);
    let mut patch = [0x90u8; HOOK_LEN];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&(stub as u32).wrapping_sub(site + 5).to_le_bytes());
    if !crate::writemem(site as *mut u8, &patch) {
        return;
    }
    crate::fix_stats::installed("async-linker-fix", &[&MISSING]);
    info!(
        "async-linker-fix: an async package load with no linker is tried again, not a crash (@ {site:#010x}, out of time {:#010x}; stub {:#010x})",
        s.stop, stub as u32
    );
    #[cfg(feature = "test-commands")]
    std::thread::spawn(test_command);
}

/// A test build's count of linkers still to take away (the gamebox's `no-linker` command).
#[cfg(feature = "test-commands")]
static FORCE: AtomicU32 = AtomicU32::new(0);

/// The gamebox's `no-linker [n]` command (a test build only): the next n (3) async package
/// loads lose their linker as if its creation had failed, so the fix's way can be seen working
/// (and, past [`MAX_TRIES`], giving up as the game always did).
#[cfg(feature = "test-commands")]
fn test_command() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Some(file) = crate::LOG_PATH.get().and_then(|p| p.parent()).map(|d| d.join("gamebox-command")) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let Some(rest) = text.trim().strip_prefix("no-linker") else { continue };
        let _ = std::fs::remove_file(&file);
        let n = rest.trim().parse().unwrap_or(3);
        FORCE.store(n, Ordering::Relaxed);
        info!("async-linker-fix: test: the next {n} async package loads lose their linker");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_goes_on_with_a_linker_and_asks_without_one() {
        let stub = 0x1000_0000;
        let code = assemble(stub, 0x521efb, 0x5220c9, 0x2000_0000, None);
        let target = |at: usize| (stub + at as u32 + 4).wrapping_add(u32::from_le_bytes(code[at..at + 4].try_into().unwrap()));
        assert_eq!(&code[0..6], &MOVES, "the game's own bytes");
        assert_eq!(&code[6..10], &[0x85, 0xC0, 0x74, 0x05]);
        assert_eq!(code[10], 0xE9);
        assert_eq!(target(11), 0x521efb, "a linker: on as before");
        assert_eq!(&code[15..17], &[0x60, 0x56], "none: pushad, the entry");
        assert_eq!(code[17], 0xE8);
        assert_eq!(target(18), 0x2000_0000, "asks");
        assert_eq!(&code[22..30], &[0x83, 0xC4, 0x04, 0x89, 0x44, 0x24, 0x1C, 0x61]);
        assert_eq!(&code[30..34], &[0x85, 0xC0, 0x0F, 0x84]);
        assert_eq!(target(34), 0x521efb, "0: on as before");
        assert_eq!(code[38], 0xE9);
        assert_eq!(target(39), 0x5220c9, "1: out of time");
        assert_eq!(code.len(), 43);
    }

    #[test]
    fn a_test_build_can_take_the_linker_away() {
        let code = assemble(0x1000_0000, 0x521efb, 0x5220c9, 0x2000_0000, Some(0x3000_0000));
        assert_eq!(&code[0..6], &MOVES);
        assert_eq!(&code[6..13], &[0x83, 0x3D, 0x00, 0x00, 0x00, 0x30, 0x00], "cmp [force],0");
        assert_eq!(&code[13..15], &[0x74, 0x0E], "past the 14 bytes below");
        assert_eq!(&code[15..21], &[0xFF, 0x0D, 0x00, 0x00, 0x00, 0x30]);
        assert_eq!(&code[21..29], &[0x31, 0xC0, 0xC7, 0x06, 0, 0, 0, 0]);
        assert_eq!(&code[29..33], &[0x85, 0xC0, 0x74, 0x05], "then as without");
        assert_eq!(code.len(), 43 + 23);
    }

    #[test]
    fn tries_again_for_a_while_then_goes_on() {
        assert!(try_again(1, Duration::ZERO));
        assert!(try_again(MAX_TRIES, Duration::from_secs(9)));
        assert!(!try_again(MAX_TRIES + 1, Duration::from_secs(1)));
        assert!(!try_again(2, Duration::from_secs(11)));
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn the_entry_names_its_package() {
        let name = b"maps\\hub\\stream\\HUB_Global_13_str.umd\0";
        let mut entry = [0u8; 0x54];
        entry[4..8].copy_from_slice(&(name.as_ptr() as u32).to_le_bytes());
        entry[8..12].copy_from_slice(&(name.len() as i32).to_le_bytes());
        // 32-bit only: the entry holds 4-byte pointers.
        if std::mem::size_of::<usize>() == 4 {
            assert_eq!(unsafe { entry_name(entry.as_ptr()) }, "maps\\hub\\stream\\HUB_Global_13_str.umd");
        }
        let empty = [0u8; 0x54];
        assert_eq!(unsafe { entry_name(empty.as_ptr()) }, "");
    }
}
