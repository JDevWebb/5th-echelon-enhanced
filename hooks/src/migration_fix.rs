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

use retour::static_detour;
use tracing::info;
use tracing::warn;

static_detour! {
    /// `thiscall(state_machine, seconds)`, `ret 4`.
    static TickHook: unsafe extern "thiscall" fn(*mut u8, f32);
}

/// How long a migration may wait before the match is left.
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(60);

/// The tick's start, up to `push ebx` (the same in both executables).
const TICK: &str = "55 8B EC 56 8B F1 83 7E 30 00 0F 84 ?? ?? ?? ?? 83 3D ?? ?? ?? ?? 00 0F 85 ?? ?? ?? ?? 83 7E 34 00 53";

/// The wait's end: `mov edx,[esi+0x48]`, the spinner closed, `[esi+0x48] = -1`, the state cleared.
struct Close {
    signature: &'static str,
    popups: usize,
    close: usize,
    /// DX11: `thiscall(popups, id)`; DX9: `stdcall(popups, id)`.
    thiscall: bool,
}

const CLOSE: [Close; 2] = [
    Close {
        signature: "8B 56 48 8B 0D ?? ?? ?? ?? 52 E8 ?? ?? ?? ?? 33 C0 C7 46 48 FF FF FF FF 89 46 34 89 46 38",
        popups: 5,
        close: 11,
        thiscall: true,
    },
    Close {
        signature: "8B 56 48 A1 ?? ?? ?? ?? 52 50 E8 ?? ?? ?? ?? 33 C0 C7 46 48 FF FF FF FF 89 46 34 89 46 38",
        popups: 4,
        close: 11,
        thiscall: false,
    },
];

/// The handler's lost-connection branch: in a match a `LeaveMatchTask` on the runner, else the
/// shutdown goal and the callback. Offsets of what's read from it.
struct Leave {
    signature: &'static str,
    manager: usize,
    in_match: usize,
    callback: usize,
    constructor: usize,
    runner: usize,
    base_vtable: usize,
    manager_again: usize,
    shutdown: usize,
    callback_call: usize,
}

const LEAVE: [Leave; 2] = [
    Leave {
        signature: "A1 ?? ?? ?? ?? 8B 48 50 85 C9 74 ?? E8 ?? ?? ?? ?? 84 C0 74 ?? 6A 02 6A 01 8D 4D EC 51 8D 4D C0 C7 45 F0 ?? ?? ?? ?? 89 75 EC E8 ?? ?? ?? ?? C7 45 FC 00 00 00 00 8B 0D ?? ?? ?? ?? 8D 55 C0 52 E8 ?? ?? ?? ?? C7 45 FC FF FF FF FF C7 45 C0 ?? ?? ?? ?? 8B 4D F4 64 89 0D 00 00 00 00 59 5F 5E 8B E5 5D C3 A1 ?? ?? ?? ?? 8B 48 50 6A 00 E8 ?? ?? ?? ?? 6A 00 8B CE E8 ?? ?? ?? ??",
        manager: 1,
        in_match: 13,
        callback: 35,
        constructor: 43,
        runner: 56,
        base_vtable: 79,
        manager_again: 101,
        shutdown: 111,
        callback_call: 120,
    },
    Leave {
        signature: "A1 ?? ?? ?? ?? 8B 48 50 85 C9 74 ?? E8 ?? ?? ?? ?? 84 C0 74 ?? 6A 02 6A 01 8D 4C 24 18 51 8D 54 24 14 C7 44 24 18 ?? ?? ?? ?? 89 7C 24 14 E8 ?? ?? ?? ?? C7 44 24 48 00 00 00 00 8B 35 ?? ?? ?? ?? 8D 4C 24 10 E8 ?? ?? ?? ?? C7 44 24 48 FF FF FF FF C7 44 24 10 ?? ?? ?? ?? 8B 4C 24 40 64 89 0D 00 00 00 00 59 5E 8B E5 5D C3 8B 15 ?? ?? ?? ?? 8B 4A 50 6A 00 E8 ?? ?? ?? ?? 6A 00 8B CF E8 ?? ?? ?? ??",
        manager: 1,
        in_match: 13,
        callback: 38,
        constructor: 47,
        runner: 61,
        base_vtable: 86,
        manager_again: 109,
        shutdown: 119,
        callback_call: 128,
    },
];

/// The task constructor's body (both executables): the base vtable, the delegate at +8, +0x10,
/// +0x14, +0x18, `+0x1c` = 1, the two arguments at +0x20 and +0x24, then `LeaveMatchTask`'s
/// vtable and `+0x28` = 0.
const CONSTRUCTOR: &str = "0F 57 C0 33 C9 C7 00 ?? ?? ?? ?? 89 48 04 8B 32 89 70 08 8B 52 04 89 50 0C 89 48 10 F3 0F 11 40 14 66 89 48 18 C7 40 1C 01 00 00 00 89 4D FC 8B 55 0C 89 50 20 8B 55 10 C7 00 ?? ?? ?? ?? 89 50 24 89 48 28";
const CONSTRUCTOR_BASE: usize = 7;
const CONSTRUCTOR_VTABLE: usize = 58;
/// How far into the constructor its body may start (past the SEH prologue).
const CONSTRUCTOR_SEARCH: usize = 0x80;

/// The in-match check's start: `mov eax,[owner] / mov eax,[eax+0x5d0] / test eax,eax / jz`.
const IN_MATCH: &str = "A1 ?? ?? ?? ?? 8B 80 D0 05 00 00 85 C0 74";

/// The state machine's fields.
const REASON: usize = 0x34;
const PHASE: usize = 0x38;
const SPINNER: usize = 0x48;
/// The sessions under the owner (match, lobby), and a session's migration task and its state.
const MATCH_SESSION: usize = 0x5d0;
const LOBBY_SESSION: usize = 0x5d4;
const MIGRATION: usize = 0x610;

struct Game {
    popups: usize,
    close: usize,
    close_thiscall: bool,
    manager: usize,
    owner: usize,
    in_match: usize,
    callback: usize,
    runner: usize,
    task_vtable: usize,
    shutdown: usize,
}

static GAME: OnceLock<Game> = OnceLock::new();

/// The migration being watched: the state machine, its reason, and since when.
static WAITING: Mutex<Option<(usize, u32, Instant)>> = Mutex::new(None);

/// `"8B 56 ?? 48"` as a signature.
fn parse(signature: &str) -> Vec<Option<u8>> {
    signature
        .split_whitespace()
        .map(|b| if b == "??" { None } else { u8::from_str_radix(b, 16).ok() })
        .collect()
}

/// The `u32` at `at` in `code`, and the target of the rel32 there.
fn word(code: &[u8], at: usize) -> usize {
    u32::from_le_bytes([code[at], code[at + 1], code[at + 2], code[at + 3]]) as usize
}

fn target(code: &[u8], base: usize, at: usize) -> usize {
    (base + at + 4).wrapping_add(word(code, at))
}

/// The one signature of `options` found exactly once in `code`, with where.
fn find_one<'a, T>(code: &[u8], options: &'a [T], signature: impl Fn(&T) -> &str) -> Option<(&'a T, usize)> {
    let found: Vec<(&T, usize)> = options
        .iter()
        .filter_map(|o| crate::find_unique(code, &parse(signature(o))).ok().map(|at| (o, at)))
        .collect();
    match found[..] {
        [one] => Some(one),
        _ => None,
    }
}

/// Reads the pieces from `code` (loaded at `base`); `None` names what didn't check out.
fn locate(code: &[u8], base: usize) -> Result<(usize, Game), &'static str> {
    let tick = crate::find_unique(code, &parse(TICK)).map_err(|_| "the disconnect state machine's tick")?;
    let (close, c) = find_one(code, &CLOSE, |c| c.signature).ok_or("the migration wait's end")?;
    let (leave, l) = find_one(code, &LEAVE, |l| l.signature).ok_or("the lost-connection branch")?;
    let manager = word(code, l + leave.manager);
    let callback = word(code, l + leave.callback);
    if word(code, l + leave.manager_again) != manager || target(code, base, l + leave.callback_call) != callback {
        return Err("the lost-connection branch's two halves");
    }
    let constructor = target(code, base, l + leave.constructor).checked_sub(base).ok_or("the task constructor")?;
    let body = code.get(constructor..constructor + CONSTRUCTOR_SEARCH).ok_or("the task constructor")?;
    let at = crate::find_unique(body, &parse(CONSTRUCTOR)).map_err(|_| "the task constructor's body")?;
    if word(body, at + CONSTRUCTOR_BASE) != word(code, l + leave.base_vtable) {
        return Err("the task's base vtable");
    }
    let in_match = target(code, base, l + leave.in_match);
    let check = code.get(in_match.wrapping_sub(base)..).ok_or("the in-match check")?;
    if crate::find_unique(&check[..IN_MATCH.split_whitespace().count()], &parse(IN_MATCH)) != Ok(0) {
        return Err("the in-match check");
    }
    Ok((
        base + tick,
        Game {
            popups: word(code, c + close.popups),
            close: target(code, base, c + close.close),
            close_thiscall: close.thiscall,
            manager,
            owner: word(check, 1),
            in_match,
            callback,
            runner: word(code, l + leave.runner),
            task_vtable: word(body, at + CONSTRUCTOR_VTABLE),
            shutdown: target(code, base, l + leave.shutdown),
        },
    ))
}

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
    let Some(code) = crate::game_code() else {
        warn!("migration-fix: the game's .text wasn't found -> skipped");
        return;
    };
    let (at, game) = match locate(code, code.as_ptr() as usize) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(hex: &str) -> Vec<u8> {
        hex.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn parses_signatures() {
        assert_eq!(parse("8B ?? 48"), [Some(0x8B), None, Some(0x48)]);
        for s in [TICK, CONSTRUCTOR, IN_MATCH]
            .into_iter()
            .chain(CLOSE.iter().map(|c| c.signature))
            .chain(LEAVE.iter().map(|l| l.signature))
        {
            assert!(parse(s).iter().all(|b| b.is_some() || s.contains("??")), "{s}");
            assert_eq!(parse(s).len(), s.split_whitespace().count());
        }
    }

    #[test]
    fn reads_the_dx11_pieces() {
        // DX11 bytes at their own addresses in a fake .text from 0x0078_0000.
        let base = 0x0078_0000;
        let mut code = vec![0xCCu8; 0x0117_0000];
        let put = |code: &mut Vec<u8>, va: usize, hex: &str| {
            let b = bytes(hex);
            code[va - base..va - base + b.len()].copy_from_slice(&b);
        };
        put(
            &mut code,
            0x018e_ab60,
            "55 8b ec 56 8b f1 83 7e 30 00 0f 84 81 02 00 00 83 3d 28 c2 30 03 00 0f 85 74 02 00 00 83 7e 34 00 53 0f 84",
        );
        put(
            &mut code,
            0x018a_5f75,
            "8b 56 48 8b 0d 74 d6 44 03 52 e8 ac 2c ff ff 33 c0 c7 46 48 ff ff ff ff 89 46 34 89 46 38",
        );
        put(&mut code, 0x018d_d734, "a1 f8 3d 38 03 8b 48 50 85 c9 74 58 e8 cb bd ea fe 84 c0 74 4f 6a 02 6a 01 8d 4d ec 51 8d 4d c0 c7 45 f0 30 d4 83 01 89 75 ec e8 7d 05 f6 ff c7 45 fc 00 00 00 00 8b 0d 40 d6 44 03 8d 55 c0 52 e8 a7 14 f6 ff c7 45 fc ff ff ff ff c7 45 c0 24 17 c1 02 8b 4d f4 64 89 0d 00 00 00 00 59 5f 5e 8b e5 5d c3 a1 f8 3d 38 03 8b 48 50 6a 00 e8 d9 ae f9 fe 6a 00 8b ce e8 80 fc f5 ff");
        put(&mut code, 0x0183_dd0c, "0f 57 c0 33 c9 c7 00 24 17 c1 02 89 48 04 8b 32 89 70 08 8b 52 04 89 50 0c 89 48 10 f3 0f 11 40 14 66 89 48 18 c7 40 1c 01 00 00 00 89 4d fc 8b 55 0c 89 50 20 8b 55 10 c7 00 74 18 c1 02 89 50 24 89 48 28");
        put(&mut code, 0x0078_9510, "a1 f4 3d 38 03 8b 80 d0 05 00 00 85 c0 74 2b");
        let (tick, game) = locate(&code, base).unwrap();
        assert_eq!(tick, 0x018e_ab60);
        assert_eq!((game.popups, game.close, game.close_thiscall), (0x0344_d674, 0x0189_8c30, true));
        assert_eq!((game.manager, game.owner, game.in_match), (0x0338_3df8, 0x0338_3df4, 0x0078_9510));
        assert_eq!((game.callback, game.runner, game.shutdown), (0x0183_d430, 0x0344_d640, 0x0087_8680));
        assert_eq!(game.task_vtable, 0x02c1_1874);

        // A different base vtable in the constructor than the branch resets to: refused.
        put(&mut code, 0x0183_dd13, "00 00 00 00");
        assert_eq!(locate(&code, base).err(), Some("the task's base vtable"));
    }
}
