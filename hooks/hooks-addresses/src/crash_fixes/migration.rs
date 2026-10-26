//! The disconnect state machine's tick, and the game's own ways of ending a lost host (the hooks'
//! `migration_fix` has the whole story).

use super::find_unique;
use super::parse;
use super::Image;

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

/// What the fix reads and calls in the game, by address.
#[derive(Debug)]
pub struct Game {
    pub popups: usize,
    pub close: usize,
    pub close_thiscall: bool,
    pub manager: usize,
    pub owner: usize,
    pub in_match: usize,
    pub callback: usize,
    pub runner: usize,
    pub task_vtable: usize,
    pub shutdown: usize,
}

/// The `u32` at `at` in `code`, and the target of the rel32 there.
fn word(code: &[u8], at: usize) -> usize {
    u32::from_le_bytes([code[at], code[at + 1], code[at + 2], code[at + 3]]) as usize
}

fn target(code: &[u8], base: usize, at: usize) -> usize {
    (base + at + 4).wrapping_add(word(code, at)) & 0xffff_ffff
}

/// The one signature of `options` found exactly once in `code`, with where.
fn find_one<'a, T>(code: &[u8], options: &'a [T], signature: impl Fn(&T) -> &str) -> Option<(&'a T, usize)> {
    let found: Vec<(&T, usize)> = options.iter().filter_map(|o| find_unique(code, &parse(signature(o))).ok().map(|at| (o, at))).collect();
    match found[..] {
        [one] => Some(one),
        _ => None,
    }
}

/// Reads the pieces from `code` (loaded at `base`); `Err` names what didn't check out.
fn locate_in(code: &[u8], base: usize) -> Result<(usize, Game), &'static str> {
    let tick = find_unique(code, &parse(TICK)).map_err(|_| "the disconnect state machine's tick")?;
    let (close, c) = find_one(code, &CLOSE, |c| c.signature).ok_or("the migration wait's end")?;
    let (leave, l) = find_one(code, &LEAVE, |l| l.signature).ok_or("the lost-connection branch")?;
    let manager = word(code, l + leave.manager);
    let callback = word(code, l + leave.callback);
    if word(code, l + leave.manager_again) != manager || target(code, base, l + leave.callback_call) != callback {
        return Err("the lost-connection branch's two halves");
    }
    let constructor = target(code, base, l + leave.constructor).checked_sub(base).ok_or("the task constructor")?;
    let body = code.get(constructor..constructor + CONSTRUCTOR_SEARCH).ok_or("the task constructor")?;
    let at = find_unique(body, &parse(CONSTRUCTOR)).map_err(|_| "the task constructor's body")?;
    if word(body, at + CONSTRUCTOR_BASE) != word(code, l + leave.base_vtable) {
        return Err("the task's base vtable");
    }
    let in_match = target(code, base, l + leave.in_match);
    let check = code.get(in_match.wrapping_sub(base)..).ok_or("the in-match check")?;
    if find_unique(check.get(..IN_MATCH.split_whitespace().count()).ok_or("the in-match check")?, &parse(IN_MATCH)) != Ok(0) {
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

/// The tick's address, and the rest the fix needs.
pub fn locate(img: &Image) -> Result<(usize, Game), &'static str> {
    locate_in(img.text, img.text_va)
}

/// The host's answer to a join testing a session's lock: `cmp byte [edi+0x5de], 0`, then
/// `jz` past the "Locked Session" refusal (DX11 `0x859d37`, DX9 `0xaf4c87`).
const LOCK_TEST: &str = "80 BF DE 05 00 00 00";

/// Where the game tests a session's lock (byte `+0x5de`), if this build has the test once and
/// at that offset: what `migration_fix` reads for `game_state::online`.
pub fn lock_test(img: &Image) -> Result<usize, String> {
    img.find(&parse(LOCK_TEST)).map_err(|n| format!("the session lock test matched {n} times"))
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
        let (tick, game) = locate_in(&code, base).unwrap();
        assert_eq!(tick, 0x018e_ab60);
        assert_eq!((game.popups, game.close, game.close_thiscall), (0x0344_d674, 0x0189_8c30, true));
        assert_eq!((game.manager, game.owner, game.in_match), (0x0338_3df8, 0x0338_3df4, 0x0078_9510));
        assert_eq!((game.callback, game.runner, game.shutdown), (0x0183_d430, 0x0344_d640, 0x0087_8680));
        assert_eq!(game.task_vtable, 0x02c1_1874);

        // A different base vtable in the constructor than the branch resets to: refused.
        put(&mut code, 0x0183_dd13, "00 00 00 00");
        assert_eq!(locate_in(&code, base).err(), Some("the task's base vtable"));
    }
}
