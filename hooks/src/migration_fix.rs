//! Ends a host migration that never finishes: "Host has changed. Migrating match..." stayed up
//! for good when nobody could take over, and the game had to be restarted (passiverat,
//! playnwork).
//!
//! The game's disconnect state machine (ticked every frame, `thiscall(this, seconds)`) sets
//! reason 5 (the match session migrating) or 6 (the lobby's), shows the spinner (its id at
//! `[this+0x48]`) and moves to phase 2 (`[this+0x38]`), which only waits: it takes the spinner
//! down and clears the state once the session's migration task is gone or reports done (state
//! 0xd). There's no time limit, so a task that stops anywhere else keeps it up for ever.
//!
//! The fix: a detour on the tick. After the game's own tick, a reason 5 or 6 still waiting in
//! phase 2 after [`GIVE_UP_AFTER`], with no UI task running, ends the way the game ends a lost
//! host, from its own code:
//! - the spinner closes and the state clears, as the wait does when the migration finishes;
//! - in a match, a `UI::Task::LeaveMatchTask` with the game's host-lost arguments (1, 4, and
//!   `+0x28` = 1) goes on the UI task runner, completing to the menu the game shows after a host
//!   timeout (its callback, with the reason cleared, opens screen 0x32);
//! - outside one, the game's shutdown goal and the same callback, as its own code does there.
//!
//! The task is built in place (its layout is the constructor's in both executables, checked),
//! and started as the runner starts one (clone through vtable slot 2, start through slot 4), so
//! nothing here calls the DX9 build's register-passing functions. The player is told why.
//!
//! Matched by signatures unique in each executable (DX11 tick VA `0x18eab60`, DX9 `0x43def0`),
//! the pieces cross-checked against each other; any mismatch leaves the game alone.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use hooks_addresses::crash_fixes::migration;
use hooks_addresses::crash_fixes::migration::Game;
use retour::static_detour;
use tracing::info;
use tracing::warn;

static_detour! {
    /// `thiscall(state_machine, seconds)`, `ret 4`.
    static TickHook: unsafe extern "thiscall" fn(*mut u8, f32);
}

/// How long a migration may wait before the match is left.
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(15);

/// The state machine's fields.
const REASON: usize = 0x34;
const PHASE: usize = 0x38;
const SPINNER: usize = 0x48;
/// The sessions under the owner (match, lobby), and a session's migration task and its state.
const MATCH_SESSION: usize = 0x5d0;
const LOBBY_SESSION: usize = 0x5d4;
const MIGRATION: usize = 0x610;

static GAME: OnceLock<Game> = OnceLock::new();

/// The migration being watched: the state machine, its reason, and since when.
static WAITING: Mutex<Option<(usize, u32, Instant)>> = Mutex::new(None);

unsafe fn read<T>(at: usize) -> T {
    std::ptr::read_unaligned(at as *const T)
}

/// The migration task's state for `reason`'s session, if there is one.
unsafe fn migration_state(game: &Game, reason: u32) -> Option<u32> {
    let owner = read::<usize>(game.owner);
    if owner == 0 {
        return None;
    }
    let session = read::<usize>(owner + if reason == 5 { MATCH_SESSION } else { LOBBY_SESSION });
    if session == 0 {
        return None;
    }
    let task = read::<usize>(session + MIGRATION);
    (task != 0).then(|| read::<u32>(task + 4))
}

/// Leaves, as the game does when its host is lost: in a match a `LeaveMatchTask` on the runner,
/// else the shutdown goal and the menu. `this` is the state machine (the task's callback object).
unsafe fn leave(game: &Game, this: *mut u8) -> &'static str {
    let manager = read::<usize>(game.manager);
    let online = if manager == 0 { 0 } else { read::<usize>(manager + 0x50) };
    if online == 0 {
        return "nothing to leave";
    }
    let callback: unsafe extern "thiscall" fn(*mut u8, u32) = std::mem::transmute(game.callback);
    let in_match: unsafe extern "thiscall" fn(usize) -> u8 = std::mem::transmute(game.in_match);
    if in_match(online) == 0 {
        let shutdown: unsafe extern "thiscall" fn(usize, u32) = std::mem::transmute(game.shutdown);
        shutdown(online, 0);
        callback(this, 0);
        return "the online session shut down";
    }
    let mut task = [0usize; 11];
    task[0] = game.task_vtable;
    task[2] = this as usize;
    task[3] = game.callback;
    task[7] = 1;
    task[8] = 1;
    task[9] = 4;
    task[10] = 1;
    let runner = read::<usize>(game.runner);
    let vtable = task[0] as *const usize;
    let clone: unsafe extern "thiscall" fn(*mut usize) -> *mut u8 = std::mem::transmute(*vtable.add(2));
    let started = clone(task.as_mut_ptr());
    std::ptr::write_unaligned((runner + 0x30) as *mut *mut u8, started);
    if *started.add(0x19) == 0 {
        let start: unsafe extern "thiscall" fn(*mut u8) = std::mem::transmute(*read::<*const usize>(started as usize).add(4));
        start(started);
    }
    "left the match"
}

fn tick(this: *mut u8, seconds: f32) {
    unsafe { TickHook.call(this, seconds) };
    let Some(game) = GAME.get() else { return };
    unsafe {
        let reason = read::<u32>(this as usize + REASON);
        let waiting = matches!(reason, 5 | 6) && read::<u32>(this as usize + PHASE) == 2;
        let mut watched = WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !waiting {
            if let Some((_, reason, since)) = watched.take() {
                info!("migration-fix: the host migration ({}) ended by itself after {} s", what(reason), since.elapsed().as_secs());
            }
            return;
        }
        let since = match *watched {
            Some((at, r, since)) if at == this as usize && r == reason => since,
            _ => {
                info!(
                    "migration-fix: host migration ({}) started; the match is left if it hasn't finished in {} s",
                    what(reason),
                    GIVE_UP_AFTER.as_secs()
                );
                *watched = Some((this as usize, reason, Instant::now()));
                return;
            }
        };
        let runner = read::<usize>(game.runner);
        if since.elapsed() < GIVE_UP_AFTER || runner == 0 || read::<usize>(runner + 0x30) != 0 {
            return;
        }
        *watched = None;
        let state = migration_state(game, reason).map_or_else(|| String::from("none"), |s| format!("{s:#x}"));
        let spinner = read::<u32>(this as usize + SPINNER);
        let popups = read::<usize>(game.popups);
        if game.close_thiscall {
            let close: unsafe extern "thiscall" fn(usize, u32) = std::mem::transmute(game.close);
            close(popups, spinner);
        } else {
            let close: unsafe extern "stdcall" fn(usize, u32) = std::mem::transmute(game.close);
            close(popups, spinner);
        }
        std::ptr::write_unaligned((this as usize + SPINNER) as *mut u32, u32::MAX);
        std::ptr::write_unaligned((this as usize + REASON) as *mut u32, 0);
        std::ptr::write_unaligned((this as usize + PHASE) as *mut u32, 0);
        let done = leave(game, this);
        warn!(
            "migration-fix: the host migration ({}) hadn't finished after {} s (its task's state {state}): spinner closed, {done}",
            what(reason),
            GIVE_UP_AFTER.as_secs()
        );
        crate::community::tell(
            "The host migration didn't finish",
            &format!(
                "No one could take over as host in {} s, so you've left the match. Find or host another from the menu.",
                GIVE_UP_AFTER.as_secs()
            ),
        );
    }
}

fn what(reason: u32) -> &'static str {
    if reason == 5 {
        "match"
    } else {
        "lobby"
    }
}

/// Installs the detour. Safe to call once at startup; leaves an unrecognised build untouched.
pub unsafe fn end_stuck_migrations() {
    let Some(image) = crate::game_image() else {
        warn!("migration-fix: the game's sections weren't found -> skipped");
        return;
    };
    let (at, game) = match migration::locate(&image) {
        Ok(found) => found,
        Err(what) => {
            warn!("migration-fix: {what} wasn't found or didn't match -> skipped (unknown build?)");
            return;
        }
    };
    let _ = GAME.set(game);
    match TickHook
        .initialize(std::mem::transmute::<usize, unsafe extern "thiscall" fn(*mut u8, f32)>(at), tick)
        .and_then(|h| h.enable())
    {
        Ok(()) => info!(
            "migration-fix: a host migration still running after {} s leaves the match (tick @ {at:#010x})",
            GIVE_UP_AFTER.as_secs()
        ),
        Err(err) => warn!("migration-fix: the tick's detour failed: {err:?}"),
    }
}
