//! Takes every copy of an observer off a Storm sent-packet ticket when it detaches, not just the
//! first (one candidate for crashes in the game's P2P code: a freed observer still listed on a
//! ticket, called when the packet is acknowledged, `Blacklist_DX11_game.exe+0x1ccb41a`, or when
//! its ring slot is reused, `+0x1cc8c2d`).
//!
//! Each outgoing peer stream keeps a ring of tickets for the packets it sent (`Storm::echo`).
//! A ticket keeps a list of observers (`[ticket+0x24]`, count `[ticket+0x1c]`) and calls each one
//! through its vtable when its state changes or the packet is acknowledged or lost. Attaching
//! pushes the observer without checking whether it's already listed, but the game's remove takes
//! out only the first copy, and closing an observer calls it once per ticket. So an observer
//! listed twice stays listed after it's closed and freed, and the ticket calls freed memory.
//! Static analysis found no path that attaches one observer twice, so this is one of two
//! candidates; the other (an observer freed without being closed) is what the log line below
//! rules in or out: if this never logs while the crash still happens, it's the other.
//!
//! The fix: a detour on the remove calls the game's own remove, then compacts the list of any
//! further copies of the same observer, the way the game's remove does for the first (keeping
//! order). With one copy, as normal, it does nothing more. The function is matched by a
//! signature unique in both executables (DX11 VA `0x20c8aa0`, DX9 VA `0x20a15b0`), left alone
//! otherwise.

use std::ffi::c_void;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use retour::static_detour;
use tracing::info;
use tracing::warn;

static_detour! {
    /// `thiscall(ticket, result*, observer) -> result*`.
    static RemoveObserverHook: unsafe extern "thiscall" fn(*mut u8, *mut c_void, *mut c_void) -> *mut c_void;
}

/// The remove's start: `push ebp / mov ebp,esp / push ebx / mov ebx,ecx / mov eax,[ebx+0x24] /
/// mov ecx,[ebx+0x1c] / lea ecx,[eax+ecx*4] / push esi / cmp eax,ecx / jz / mov edx,[ebp+0xc] /
/// cmp [eax],edx / jz / add eax,4 / cmp eax,ecx / jnz / mov esi,[ebp+8] / push 1 / mov ecx,esi /
/// call <result> / mov eax,esi / pop esi / pop ebx / pop ebp / ret 8`; `None` is a wildcard.
#[rustfmt::skip]
const SIGNATURE: [Option<u8>; 54] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x53), Some(0x8B), Some(0xD9), Some(0x8B), Some(0x43), Some(0x24),
    Some(0x8B), Some(0x4B), Some(0x1C), Some(0x8D), Some(0x0C), Some(0x88), Some(0x56), Some(0x3B), Some(0xC1),
    Some(0x74), Some(0x0E), Some(0x8B), Some(0x55), Some(0x0C), Some(0x39), Some(0x10), Some(0x74), Some(0x1B),
    Some(0x83), Some(0xC0), Some(0x04), Some(0x3B), Some(0xC1), Some(0x75), Some(0xF5), Some(0x8B), Some(0x75),
    Some(0x08), Some(0x6A), Some(0x01), Some(0x8B), Some(0xCE), Some(0xE8), None, None, None, None,
    Some(0x8B), Some(0xC6), Some(0x5E), Some(0x5B), Some(0x5D), Some(0xC2), Some(0x08), Some(0x00),
];

/// The observer list in a ticket.
const OBSERVERS: usize = 0x24;
const COUNT: usize = 0x1c;

/// Extra copies removed since the game started.
static EXTRA: AtomicU32 = AtomicU32::new(0);

/// Removes every copy of `observer` from `list[..count]`, keeping the others in order; returns
/// the new count.
fn remove_all(list: &mut [*mut c_void], observer: *mut c_void) -> usize {
    let mut kept = 0;
    for i in 0..list.len() {
        if list[i] != observer {
            list[kept] = list[i];
            kept += 1;
        }
    }
    kept
}

#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn remove_observer(ticket: *mut u8, result: *mut c_void, observer: *mut c_void) -> *mut c_void {
    let result = unsafe { RemoveObserverHook.call(ticket, result, observer) };
    unsafe {
        let count = std::ptr::read_unaligned(ticket.add(COUNT).cast::<i32>());
        let list = std::ptr::read_unaligned(ticket.add(OBSERVERS).cast::<*mut *mut c_void>());
        if count <= 0 || list.is_null() {
            return result;
        }
        let list = std::slice::from_raw_parts_mut(list, count as usize);
        let kept = remove_all(list, observer);
        if kept != list.len() {
            std::ptr::write_unaligned(ticket.add(COUNT).cast::<i32>(), kept as i32);
            let removed = (list.len() - kept) as u32;
            let n = EXTRA.fetch_add(removed, Ordering::Relaxed) + removed;
            let seq = std::ptr::read_unaligned(ticket.add(0x4).cast::<i32>());
            let state = std::ptr::read_unaligned(ticket.add(0xc).cast::<i32>());
            let vtable = std::ptr::read_unaligned(observer.cast::<usize>());
            info!(
                "storm-fix: an observer was listed more than once on a sent-packet ticket: removed {removed} more copies of {observer:p} (vtable {vtable:#010x}; ticket seq {seq}, state {state}, {kept} left; {n} so far)"
            );
        }
    }
    result
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

/// Installs the detour. Safe to call once at startup; leaves an unrecognised build untouched.
pub unsafe fn remove_all_copies_of_observers() {
    let Some(code) = crate::game_code() else {
        warn!("storm-fix: the game's .text wasn't found -> skipped");
        return;
    };
    let at = match find(code) {
        Ok(at) => at,
        Err(n) => {
            warn!("storm-fix: the ticket's observer remove was found {n} times, not once -> skipped (unknown build?)");
            return;
        }
    };
    let target = code.as_ptr().add(at);
    let res = RemoveObserverHook.initialize(std::mem::transmute(target), remove_observer).and_then(|h| h.enable());
    match res {
        Ok(()) => info!("storm-fix: observers now leave sent-packet tickets entirely (remove @ {target:p})"),
        Err(err) => warn!("storm-fix: the detour failed: {err:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_every_copy_in_order() {
        let (a, b, c) = (1 as *mut c_void, 2 as *mut c_void, 3 as *mut c_void);
        let mut list = [b, a, c, a, b];
        assert_eq!(remove_all(&mut list, a), 3);
        assert_eq!(list[..3], [b, c, b]);
        let mut list = [b, c];
        assert_eq!(remove_all(&mut list, a), 2, "nothing to remove");
        assert_eq!(list, [b, c]);
        let mut list = [a, a];
        assert_eq!(remove_all(&mut list, a), 0);
    }
}
