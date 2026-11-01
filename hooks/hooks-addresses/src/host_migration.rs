//! Where the beta "host migration" goes (the hooks' `migrate_lobby`, `party_follow` and
//! `follow_unblock`): every function, site and global it uses, by signature, and checked
//! against each other where two read the same global. All or nothing: the hooks install the
//! three only when everything here is found. Found in the DX11 build; the DX9 one lacks two of
//! them (`FollowDelegate` and the facade code), so the beta stays off there.

use crate::crash_fixes::parse;
use crate::crash_fixes::Image;

/// The "may migrate" flag's setter (`0x0086a1e0` in DX11, from `+0x1f9`): `cmp dword [esi+0x5f0],1`
/// / `mov [esi+0x5a8],al` / `mov ecx,[edi+0x298]` / `mov [esi+0x5b4],ecx` / `sete dl` /
/// `push ebx` / `lea ecx,[ebp-0x1e0]` / `mov [esi+0x5a9],dl`.
pub const SETTER: [u8; 41] = [
    0x83, 0xBE, 0xF0, 0x05, 0x00, 0x00, 0x01, // cmp dword [esi+0x5f0],1
    0x88, 0x86, 0xA8, 0x05, 0x00, 0x00, // mov [esi+0x5a8],al
    0x8B, 0x8F, 0x98, 0x02, 0x00, 0x00, // mov ecx,[edi+0x298]
    0x89, 0x8E, 0xB4, 0x05, 0x00, 0x00, // mov [esi+0x5b4],ecx
    0x0F, 0x94, 0xC2, // sete dl
    0x53, // push ebx
    0x8D, 0x8D, 0x20, 0xFE, 0xFF, 0xFF, // lea ecx,[ebp-0x1e0]
    0x88, 0x96, 0xA9, 0x05, 0x00, 0x00, // mov [esi+0x5a9],dl
];

/// The engine's host-loss check, the whole function (`0x00779580`): `cmp dword [ecx+0x610],0` /
/// `je +7` / `mov al,[ecx+0x5a9]` / `ret` / `xor al,al` / `ret`.
pub const CHECK: [u8; 19] = [
    0x83, 0xB9, 0x10, 0x06, 0x00, 0x00, 0x00, // cmp dword [ecx+0x610],0
    0x74, 0x07, // je .no
    0x8A, 0x81, 0xA9, 0x05, 0x00, 0x00, // mov al,[ecx+0x5a9]
    0xC3, // ret
    0x32, 0xC0, // .no: xor al,al
    0xC3, // ret
];

/// The party controller's "leader action" (`0x00879b20`, thiscall, one argument), through its
/// check that the party session is in `StateInSession` and not locked: it reads the net core
/// (twice), the id of "no state" and the id of `StateInSession`.
const LEADER_ACTION: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 78 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 64 A3 00 00 00 00 \
    8B F1 A1 ?? ?? ?? ?? 33 DB 3B C3 0F 84 ?? ?? ?? ?? 8B 88 D4 05 00 00 39 59 10 74 13 8B 49 10 8B 01 8B 50 30 FF D2 8B 08 \
    A1 ?? ?? ?? ?? EB 06 8B 0D ?? ?? ?? ?? 3B 0D ?? ?? ?? ?? 0F 85 ?? ?? ?? ?? 8B 88 D4 05 00 00 38 99 DE 05 00 00";
const LEADER_ACTION_NET_CORE: [usize; 2] = [43, 81];
const LEADER_ACTION_STATE_NONE: usize = 89;
const LEADER_ACTION_IN_SESSION: usize = 95;

/// `StateLeaderPartyWaitSync::vf08` (`0x00839280`), its start.
const SYNC_ENTER: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 81 EC 98 01 00 00 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 64 A3 00 00 00 00 8B D9 8B 43 08";

/// The host test in `vf08`: `lea ecx,[edi+0x554]` / `call` (am I the host) / `test al,al` /
/// **`je` over the FollowLeader send** / `movzx ecx,byte [esi+0x5a8]`.
const HOST_TEST: &str = "8D 8F 54 05 00 00 E8 ?? ?? ?? ?? 84 C0 74 5F 0F B6 8E A8 05 00 00";
/// Where the `je` is in [`HOST_TEST`], and its two bytes.
pub const HOST_TEST_JE_AT: usize = 13;
pub const HOST_TEST_JE: [u8; 2] = [0x74, 0x5F];
/// How far into `vf08` the host test is, at most.
const HOST_TEST_WITHIN: usize = 0x1c0;

/// `StateLeaderPartyWaitSync::vf00` (`0x008ae430`), into its first use of `NetOnline`.
const SYNC_TICK: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 28 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 64 A3 00 00 00 00 \
    8B F9 A1 ?? ?? ?? ?? 8B 70 48 85 F6 74 ?? E8 ?? ?? ?? ?? 80 B8 48 04 00 00 00";
const SYNC_TICK_NET_ONLINE: usize = 43;

/// `PartyFollowLeaderTask::vf04` (`0x018eae00`): thiscall, no stack arguments.
const FOLLOW_TASK: &str = "55 8B EC 83 EC 08 A1 ?? ?? ?? ?? 56 8B F1 8B 48 58 6A 06 E8";
const FOLLOW_TASK_NET_ONLINE: usize = 7;

/// `FollowDelegate` (`0x01886720`): thiscall, one stack argument (the result).
const FOLLOW_DELEGATE: &str = "55 8B EC 33 C0 89 41 34 89 41 38 39 45 08 75 2C 8B 0D ?? ?? ?? ?? 6A 03 81 C1 50 01 00 00";
const FOLLOW_DELEGATE_NET_ONLINE: usize = 18;

/// A small UI function (`0x018f5ac0`) that reads the facade's task runner (its slot at `+0x30`),
/// `NetOnline` and the facade: where those two globals of the UI are read from.
const FACADE_USER: &str = "56 8B F1 83 BE 64 02 00 00 FF 74 42 A1 ?? ?? ?? ?? 83 78 30 00 75 37 8B 0D ?? ?? ?? ?? E8 ?? ?? ?? ?? 84 C0 75 28 \
    8B 8E 64 02 00 00 51 E8 ?? ?? ?? ?? 8B C8 E8 ?? ?? ?? ?? C7 86 64 02 00 00 FF FF FF FF 8B 0D ?? ?? ?? ??";
const FACADE_USER_GATE: usize = 13;
const FACADE_USER_NET_ONLINE: usize = 25;
const FACADE_USER_FACADE: usize = 69;

/// Everything the three hooks use, by address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sites {
    /// `migrate_lobby`: the flag's setter ([`SETTER`]'s start) and the engine's check.
    pub setter: usize,
    pub check: usize,
    /// `party_follow`: the leader action, the sync state's enter and tick, and the `je` of the
    /// host test in the enter.
    pub leader_action: usize,
    pub sync_enter: usize,
    pub sync_tick: usize,
    pub host_test_je: usize,
    /// Globals: the net core and `NetOnline` (pointers), the ids of "no state" and of
    /// `StateInSession`.
    pub net_core: usize,
    pub net_online: usize,
    pub state_none: usize,
    pub state_in_session: usize,
    /// `follow_unblock`: the UI facade's tick, `PartyFollowLeaderTask::vf04`, `FollowDelegate`,
    /// and the globals of the facade and its task runner.
    pub facade_tick: usize,
    pub follow_task: usize,
    pub follow_delegate: usize,
    pub facade: usize,
    pub facade_gate: usize,
}

fn find(img: &Image, what: &str, signature: &str) -> Result<usize, String> {
    img.find(&parse(signature)).map_err(|n| format!("{what} matched {n} times"))
}

fn find_bytes(img: &Image, what: &str, bytes: &[u8]) -> Result<usize, String> {
    img.find(&bytes.iter().map(|b| Some(*b)).collect::<Vec<_>>())
        .map_err(|n| format!("{what} matched {n} times"))
}

fn word(img: &Image, at: usize, what: &str) -> Result<usize, String> {
    img.u32(at).ok_or_else(|| format!("{what} couldn't be read"))
}

/// Everything in `img`, if all of it is there once and agrees.
pub fn locate(img: &Image) -> Result<Sites, String> {
    let setter = find_bytes(img, "the migration flag's setter", &SETTER)?;
    let check = find_bytes(img, "the engine's host-loss check", &CHECK)?;
    let leader_action = find(img, "the party's leader action", LEADER_ACTION)?;
    let net_cores = LEADER_ACTION_NET_CORE.map(|at| img.u32(leader_action + at));
    let net_core = match net_cores {
        [Some(a), Some(b)] if a == b => a,
        _ => return Err("the leader action's two reads of the net core differ".into()),
    };
    let state_none = word(img, leader_action + LEADER_ACTION_STATE_NONE, "the id of no state")?;
    let state_in_session = word(img, leader_action + LEADER_ACTION_IN_SESSION, "the id of StateInSession")?;
    let sync_enter = find(img, "the party sync's enter", SYNC_ENTER)?;
    let host_test = find(img, "the party sync's host test", HOST_TEST)?;
    if host_test < sync_enter || host_test - sync_enter > HOST_TEST_WITHIN {
        return Err("the host test isn't in the party sync's enter".into());
    }
    let host_test_je = host_test + HOST_TEST_JE_AT;
    if img.bytes(host_test_je, 2) != Some(&HOST_TEST_JE[..]) {
        return Err("the host test's jump isn't as expected".into());
    }
    let sync_tick = find(img, "the party sync's tick", SYNC_TICK)?;
    let follow_task = find(img, "the follow task", FOLLOW_TASK)?;
    let follow_delegate = find(img, "FollowDelegate", FOLLOW_DELEGATE)?;
    let facade_user = find(img, "the facade's task check", FACADE_USER)?;
    let net_online = word(img, sync_tick + SYNC_TICK_NET_ONLINE, "NetOnline")?;
    for (at, what) in [
        (follow_task + FOLLOW_TASK_NET_ONLINE, "the follow task"),
        (follow_delegate + FOLLOW_DELEGATE_NET_ONLINE, "FollowDelegate"),
        (facade_user + FACADE_USER_NET_ONLINE, "the facade's task check"),
    ] {
        if img.u32(at) != Some(net_online) {
            return Err(format!("{what} reads another NetOnline than the party sync"));
        }
    }
    let facade_tick = crate::crash_fixes::migration::tick(img).map_err(String::from)?;
    let globals = [net_core, net_online, state_none, state_in_session];
    let facade = word(img, facade_user + FACADE_USER_FACADE, "the facade")?;
    let facade_gate = word(img, facade_user + FACADE_USER_GATE, "the facade's task runner")?;
    if globals.iter().chain([&facade, &facade_gate]).any(|g| !img.image.contains(g)) {
        return Err("a global read isn't in the game's image".into());
    }
    Ok(Sites {
        setter,
        check,
        leader_action,
        sync_enter,
        sync_tick,
        host_test_je,
        net_core,
        net_online,
        state_none,
        state_in_session,
        facade_tick,
        follow_task,
        follow_delegate,
        facade,
        facade_gate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(signature: &str, fill: u8) -> Vec<u8> {
        parse(signature).into_iter().map(|b| b.unwrap_or(fill)).collect()
    }

    #[test]
    fn the_signatures_parse() {
        for s in [LEADER_ACTION, SYNC_ENTER, HOST_TEST, SYNC_TICK, FOLLOW_TASK, FOLLOW_DELEGATE, FACADE_USER] {
            assert!(parse(s).iter().filter(|b| b.is_some()).count() >= 8, "{s}");
            assert_eq!(parse(s).len(), s.split_whitespace().count());
        }
        let host_test = parse(HOST_TEST);
        assert_eq!(host_test[HOST_TEST_JE_AT..HOST_TEST_JE_AT + 2], [Some(0x74), Some(0x5F)]);
        // The globals' offsets are wildcards of four bytes each.
        for (s, at) in [
            (LEADER_ACTION, LEADER_ACTION_NET_CORE[0]),
            (LEADER_ACTION, LEADER_ACTION_NET_CORE[1]),
            (LEADER_ACTION, LEADER_ACTION_STATE_NONE),
            (LEADER_ACTION, LEADER_ACTION_IN_SESSION),
            (SYNC_TICK, SYNC_TICK_NET_ONLINE),
            (FOLLOW_TASK, FOLLOW_TASK_NET_ONLINE),
            (FOLLOW_DELEGATE, FOLLOW_DELEGATE_NET_ONLINE),
            (FACADE_USER, FACADE_USER_GATE),
            (FACADE_USER, FACADE_USER_NET_ONLINE),
            (FACADE_USER, FACADE_USER_FACADE),
        ] {
            assert!(parse(s)[at..at + 4].iter().all(Option::is_none), "{at} in {s}");
        }
    }

    #[test]
    fn a_build_missing_a_piece_gets_none() {
        // The setter and the check alone: not enough.
        let mut text = vec![0xCC; 0x40];
        text.extend(SETTER);
        text.extend([0xCC; 0x10]);
        text.extend(CHECK);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("leader action"));
        // A leader action whose two reads of the net core differ isn't trusted.
        let mut action = bytes(LEADER_ACTION, 0x11);
        action[LEADER_ACTION_NET_CORE[1]] = 0x22;
        text.extend(action);
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &[], 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("differ"));
    }
}
