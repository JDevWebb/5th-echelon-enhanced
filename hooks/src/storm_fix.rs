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
//! order). With one copy, as normal, it does nothing more.
//!
//! And a guard for either cause: the two functions that call a ticket's observers (its state
//! change, and its ack or loss) first drop any observer whose vtable isn't in the game's `.rdata`,
//! which a live one's always is (both crashes had garbage there: a float's bits, a small int). It
//! reads only the observer's first word, which the game reads next anyway. It doesn't catch an
//! observer freed by another one's call during the same notification, which neither crash showed.
//!
//! The three functions are matched by signatures unique in both executables (DX11 VA `0x20c8aa0`,
//! `0x20c8b30`, `0x20cb3e0`; DX9 VA `0x20a15b0`, `0x20a1640`, `0x20a3ef0`), and each detour is
//! left out when its function isn't found exactly once.

use std::ffi::c_void;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use hooks_addresses::crash_fixes::storm;
use retour::static_detour;
use tracing::info;
use tracing::warn;

static_detour! {
    /// `thiscall(ticket, result*, observer) -> result*`.
    static RemoveObserverHook: unsafe extern "thiscall" fn(*mut u8, *mut c_void, *mut c_void) -> *mut c_void;
    /// `thiscall(ticket, context, new_state)`, `ret 8`.
    static SetStateHook: unsafe extern "thiscall" fn(*mut u8, *mut c_void, i32) -> usize;
    /// `thiscall(ticket, context, code, unused)`, `ret 0xc`: the ticket's vtable slot 0.
    static OnResultHook: unsafe extern "thiscall" fn(*mut u8, *mut c_void, i32, usize) -> usize;
}

/// The game's `.rdata`, where every live observer's vtable is.
static RDATA: AtomicUsize = AtomicUsize::new(0);
static RDATA_END: AtomicUsize = AtomicUsize::new(0);
/// Observers dropped by the guard since the game started.
static DROPPED: AtomicU32 = AtomicU32::new(0);

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

/// Whether `observer` looks like a live object: a plausible pointer whose first word (its
/// vtable) is in the game's `.rdata`.
fn looks_live(observer: *mut c_void, rdata: &std::ops::Range<usize>) -> bool {
    let at = observer as usize;
    if !(0x10000..0x8000_0000).contains(&at) || at % 4 != 0 {
        return false;
    }
    let vtable = unsafe { std::ptr::read(observer.cast::<usize>()) };
    rdata.contains(&vtable) && vtable % 4 == 0
}

/// Drops every observer of `ticket` that doesn't look live, keeping the others in order.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss)]
unsafe fn drop_dead_observers(ticket: *mut u8, from: &str) {
    let rdata = RDATA.load(Ordering::Relaxed)..RDATA_END.load(Ordering::Relaxed);
    let count = std::ptr::read_unaligned(ticket.add(COUNT).cast::<i32>());
    let list = std::ptr::read_unaligned(ticket.add(OBSERVERS).cast::<*mut *mut c_void>());
    if count <= 0 || list.is_null() {
        return;
    }
    let list = std::slice::from_raw_parts_mut(list, count as usize);
    let mut kept = 0;
    for i in 0..list.len() {
        let observer = list[i];
        if looks_live(observer, &rdata) {
            list[kept] = observer;
            kept += 1;
        } else {
            let n = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
            let seq = std::ptr::read_unaligned(ticket.add(0x4).cast::<i32>());
            let state = std::ptr::read_unaligned(ticket.add(0xc).cast::<i32>());
            warn!("storm-fix: dropped a dead observer {observer:p} from a sent-packet ticket before its {from} (ticket seq {seq}, state {state}, {} listed; {n} so far): the game would have called freed memory", list.len());
        }
    }
    if kept != list.len() {
        std::ptr::write_unaligned(ticket.add(COUNT).cast::<i32>(), kept as i32);
    }
}

fn set_state(ticket: *mut u8, context: *mut c_void, state: i32) -> usize {
    unsafe {
        drop_dead_observers(ticket, "state change");
        SetStateHook.call(ticket, context, state)
    }
}

fn on_result(ticket: *mut u8, context: *mut c_void, code: i32, unused: usize) -> usize {
    unsafe {
        drop_dead_observers(ticket, "ack or loss");
        OnResultHook.call(ticket, context, code, unused)
    }
}

/// Installs the detour. Safe to call once at startup; leaves an unrecognised build untouched.
pub unsafe fn fix_ticket_observers() {
    let Some(image) = crate::game_image() else {
        warn!("storm-fix: the game's sections weren't found -> skipped");
        return;
    };
    let sites = storm::locate(&image);
    match sites.remove {
        Ok(target) => match RemoveObserverHook.initialize(std::mem::transmute(target), remove_observer).and_then(|h| h.enable()) {
            Ok(()) => info!("storm-fix: observers now leave sent-packet tickets entirely (remove @ {target:#010x})"),
            Err(err) => warn!("storm-fix: the remove's detour failed: {err:?}"),
        },
        Err(what) => warn!("storm-fix: {what} -> skipped (unknown build?)"),
    }

    RDATA.store(image.rdata_va, Ordering::Relaxed);
    RDATA_END.store(image.rdata_va + image.rdata.len(), Ordering::Relaxed);
    match sites.set_state {
        Ok(target) => match SetStateHook.initialize(std::mem::transmute(target), set_state).and_then(|h| h.enable()) {
            Ok(()) => info!("storm-fix: a ticket's state change drops dead observers first (@ {target:#010x})"),
            Err(err) => warn!("storm-fix: the state change's detour failed: {err:?}"),
        },
        Err(what) => warn!("storm-fix: {what} -> not guarded"),
    }
    match sites.on_result {
        Ok(target) => match OnResultHook.initialize(std::mem::transmute(target), on_result).and_then(|h| h.enable()) {
            Ok(()) => info!("storm-fix: a ticket's ack or loss drops dead observers first (@ {target:#010x})"),
            Err(err) => warn!("storm-fix: the ack's detour failed: {err:?}"),
        },
        Err(what) => warn!("storm-fix: {what} -> not guarded"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_live_observers_from_dead_ones() {
        let rdata = 0x02a0_0000..0x0320_0000;
        let mut live = 0x02cf_39b8usize;
        let mut float = 0x7f7f_ffffusize;
        assert!(looks_live(std::ptr::addr_of_mut!(live).cast(), &rdata));
        assert!(!looks_live(std::ptr::addr_of_mut!(float).cast(), &rdata), "a float's bits");
        assert!(!looks_live(std::ptr::null_mut(), &rdata));
        assert!(!looks_live(0x71 as *mut c_void, &rdata), "a small int as a pointer");
    }

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
