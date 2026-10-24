//! A party's leader that accepts an invitation into someone else's private match keeps its party
//! and takes it along (always on; the DX11 build only).
//! Background and the evidence for every address and offset below in
//! `docs/architecture/host-and-party-migration.md`.
//!
//! The game moves a party into a match over Storm, on the leader's word: the leader sends the
//! members `NetMsgParty_FollowLeader` (the match, its host's address), and each member joins as
//! the leader did, with the leader's party id in its player descriptor, which is what puts them
//! in one team. It does so only for a match the leader *hosts*, and the UI's "accept an
//! invitation" ends the party first (a Storm secede), so a leader that is invited into someone
//! else's match arrives alone and its members are left in a party of their own. Two decisions
//! on the leader are the whole difference, so the members and the match's host run unchanged:
//!
//! 1. **Don't leave the party** (`0x00879b20`, the party controller's "leader action", which the
//!    UI's accept task calls with 5 = secede): when the leader has members and the inviter's match
//!    is private (the friend list's `invite_only`), it is called with 2 instead, "the leader is
//!    joining a session", as the friends list's join route does; the party stays and is told.
//! 2. **Send FollowLeader for the match joined** (`StateLeaderPartyWaitSync`, `vf08` at
//!    `0x00839280` and `vf00` at `0x008ae430`): once the leader is in the lobby of a private match
//!    it doesn't host, with members, the state's checks are met by showing it what it expects of a
//!    hosted private match - the goal as a private custom match (kind 8, first byte 1), the
//!    "private custom match" byte of the net core as 1 (`vf00` only), and the two-byte patch
//!    below in place of "I must be the host". The state then runs the game's own code to send the
//!    message, wait for each member to answer (15 s) and set its flags; the goal and the byte
//!    are put back after each call.
//!
//! What the members and the server do after that is the game's own: the members join the match
//! (the host's `AddParticipants` adds them, the server lets them in, see
//! `dedicated_server/src/escorts.rs`) and `StateBalancingTeams` groups them by party id. When the
//! private match is over the leader's game leaves its party and hosts a new one, as it always
//! does; the server then invites the members back into the leader's new party (`escorts`).
//!
//! Everything is found by its address (the DX11 table of `hooks-addresses`) and checked by its
//! bytes first; a build that differs is left alone, with a line in the log. Reads of the game's
//! objects go through [`Memory`], so the decisions can be tested on made-up objects.

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use retour::static_detour;
use tracing::info;
use tracing::warn;

use crate::addresses::Addresses;

static_detour! {
    static LeaderActionHook: unsafe extern "thiscall" fn(*mut c_void, i32);
    static SyncEnterHook: unsafe extern "thiscall" fn(*mut c_void);
    static SyncTickHook: unsafe extern "thiscall" fn(*mut c_void, f32);
}

/// `FUN_00879b20`'s argument from the UI's accept task: leave the party (Storm secede).
const ACTION_SECEDE: i32 = 5;
/// Its argument from the friends list's join route: the leader is joining a session (the
/// members are told, the party is kept).
const ACTION_JOINING: i32 = 2;

/// An invitation noted when the game was handed it counts for this long.
const INVITE_FOR: Duration = Duration::from_secs(60);
/// How long the game thread waits for the friend list when it has to look the inviter up.
const FRIENDS_WITHIN: Duration = Duration::from_secs(2);

// The game's objects (DX11 build, addresses as loaded; see the architecture document §2.1).
/// `NetCore`: global pointer. `+0x5d0` match session, `+0x5d4` party session, `+0x5d8` the
/// local player's descriptor (`+0x10`: its id, u64), `+0x5c8` byte "private custom match".
const NET_CORE: u32 = 0x0338_3df4;
/// `NetOnline`: global pointer. `+0x58` the party controller.
const NET_ONLINE: u32 = 0x0338_3df8;
/// The id `nsGameSession::StateInSession` has, and the id a session without a state object has.
const STATE_IN_SESSION: u32 = 0x0338_d7a4;
const STATE_NONE: u32 = 0x02cf_5c50;
const CORE_MATCH: u32 = 0x5d0;
const CORE_PARTY: u32 = 0x5d4;
const CORE_PLAYER: u32 = 0x5d8;
const CORE_PRIVATE_CUSTOM: u32 = 0x5c8;
const PLAYER_ID: u32 = 0x10;
const ONLINE_PARTY: u32 = 0x58;
/// The party controller's member list: sentinel at `+0x420`, first node at `+0x424`; a node's
/// `+4` is the next, `+8` the member record, whose first u64 is the member's id.
const PARTY_MEMBERS: u32 = 0x420;
/// A session (`NetSessionBase`): `+0x10` its state object, `+0x554` its Storm session info
/// (`+4` own id, `+8` the host's id, -1 none), `+0x5a8` private, `+0x5de` locked, `+0x5f0` kind
/// (0 match, 1 party).
const SESSION_STATE: u32 = 0x10;
const SESSION_STORM: u32 = 0x554;
const SESSION_PRIVATE: u32 = 0x5a8;
const SESSION_LOCKED: u32 = 0x5de;
const SESSION_KIND: u32 = 0x5f0;
/// The state `StateLeaderPartyWaitSync`: `+8` the match owner, `+0x10` the pending transition
/// (0 none, 4 error).
const STATE_OWNER: u32 = 0x8;
const STATE_PENDING: u32 = 0x10;
/// The match owner's current goal, an object inside it: at `+0x220` when the `u32` at `+0x328`
/// is 4..=6, else at `+0x50` when the one at `+0x158` is. In the goal, `+0x24` is the kind of its
/// parameters (8: a custom match's) and `+0x10` their first byte (private).
const OWNER_GOAL_A: u32 = 0x220;
const OWNER_GOAL_A_STAGE: u32 = 0x328;
const OWNER_GOAL_B: u32 = 0x50;
const OWNER_GOAL_B_STAGE: u32 = 0x158;
const GOAL_KIND: u32 = 0x24;
const GOAL_PRIVATE: u32 = 0x10;
const GOAL_CUSTOM_MATCH: u32 = 8;

/// Reads of the game's memory. A made-up one stands in for it in the tests.
trait Memory {
    fn byte(&self, at: u32) -> Option<u8>;
    fn word(&self, at: u32) -> Option<u32>;
    /// The id of the state `session` is in (`session+0x10` is its state object, if any): what the
    /// game does with the virtual call at `+0x30` of the state, or the id of "no state".
    fn state_id(&self, session: u32) -> Option<u32>;
}

/// The running game.
struct Game;

impl Game {
    /// Whether `len` bytes at `at` can be read: a pointer into the game's objects that has gone
    /// stale would otherwise take the game down with it.
    fn readable(at: u32, len: usize) -> bool {
        at >= 0x1_0000 && region::query_range(at as usize as *const u8, len).is_ok_and(|mut r| r.all(|r| r.is_ok_and(|r| r.is_readable())))
    }

    /// Whether `len` bytes at `at` can be written.
    fn writable(at: u32, len: usize) -> bool {
        at >= 0x1_0000 && region::query_range(at as usize as *const u8, len).is_ok_and(|mut r| r.all(|r| r.is_ok_and(|r| r.is_writable())))
    }
}

impl Memory for Game {
    fn byte(&self, at: u32) -> Option<u8> {
        // SAFETY: checked readable just before.
        Self::readable(at, 1).then(|| unsafe { std::ptr::read_unaligned(at as usize as *const u8) })
    }

    fn word(&self, at: u32) -> Option<u32> {
        // SAFETY: as above.
        Self::readable(at, 4).then(|| unsafe { std::ptr::read_unaligned(at as usize as *const u32) })
    }

    fn state_id(&self, session: u32) -> Option<u32> {
        let state = self.word(session + SESSION_STATE)?;
        if state == 0 {
            return self.word(STATE_NONE);
        }
        let vtable = self.word(state)?;
        let id_of = self.word(vtable + 0x30)?;
        // SAFETY: the state object's virtual method at +0x30, which the game calls the same way;
        // it returns a pointer to the state's id.
        let id = unsafe {
            let id_of: unsafe extern "thiscall" fn(u32) -> u32 = std::mem::transmute(id_of as usize);
            id_of(state)
        };
        self.word(id)
    }
}

/// Whether `session` (a pointer to a `NetSessionBase`) is up and running: in `StateInSession`.
fn in_session(m: &impl Memory, session: u32) -> bool {
    matches!((m.state_id(session), m.word(STATE_IN_SESSION)), (Some(id), Some(running)) if id == running)
}

/// Whether the player of the Storm session info `info` hosts its session: the game's
/// `FUN_01f92c70`, a host id that is set and is its own.
fn hosts(host: u32, own: u32) -> bool {
    host != u32::MAX && host == own
}

/// The ids of the other members of the party the local player leads, when it is up and running
/// and not locked; `None` when the objects aren't there (yet).
fn other_members(m: &impl Memory) -> Option<Vec<(u32, u32)>> {
    let core = m.word(NET_CORE).filter(|c| *c != 0)?;
    let party = m.word(core + CORE_PARTY).filter(|p| *p != 0)?;
    if !in_session(m, party) || m.byte(party + SESSION_LOCKED)? != 0 {
        return None;
    }
    // Only the party's host leads it: a member's list names the others too, but it isn't theirs to keep.
    if !hosts(m.word(party + SESSION_STORM + 8)?, m.word(party + SESSION_STORM + 4)?) {
        return None;
    }
    let controller = m.word(m.word(NET_ONLINE).filter(|o| *o != 0)? + ONLINE_PARTY).filter(|c| *c != 0)?;
    let player = m.word(core + CORE_PLAYER).filter(|p| *p != 0)?;
    let own = (m.word(player + PLAYER_ID)?, m.word(player + PLAYER_ID + 4)?);
    let head = controller + PARTY_MEMBERS;
    let mut node = m.word(head + 4)?;
    let mut others = Vec::new();
    // A party has eight seats; a list that goes on is not one to trust.
    for _ in 0..16 {
        if node == head || node == 0 {
            return Some(others);
        }
        let record = m.word(node + 8)?;
        let id = (m.word(record)?, m.word(record + 4)?);
        if id != own {
            others.push(id);
        }
        node = m.word(node + 4)?;
    }
    None
}

/// Whether the local player leads a party with other members in it.
fn leads_members(m: &impl Memory) -> bool {
    other_members(m).is_some_and(|others| !others.is_empty())
}

/// Whether the local player is in a private match someone else hosts, as the leader of a party with
/// members: the case `StateLeaderPartyWaitSync` is shown a hosted private match for.
fn joined_private_match(m: &impl Memory) -> bool {
    let Some(core) = m.word(NET_CORE).filter(|c| *c != 0) else { return false };
    let Some(session) = m.word(core + CORE_MATCH).filter(|s| *s != 0) else { return false };
    let (Some(own), Some(host), Some(private), Some(kind)) = (
        m.word(session + SESSION_STORM + 4),
        m.word(session + SESSION_STORM + 8),
        m.byte(session + SESSION_PRIVATE),
        m.word(session + SESSION_KIND),
    ) else {
        return false;
    };
    in_session(m, session) && private != 0 && kind == 0 && host != u32::MAX && !hosts(host, own) && leads_members(m)
}

/// The goal the state `state` (a `StateLeaderPartyWaitSync`) looks at: the match owner's current
/// one. The same choice the game makes in `vf08` and `vf00`.
fn current_goal(m: &impl Memory, state: u32) -> Option<u32> {
    let owner = m.word(state + STATE_OWNER).filter(|o| *o != 0)?;
    let staged = |stage: u32| m.word(owner + stage).is_some_and(|s| (4..=6).contains(&s));
    if staged(OWNER_GOAL_A_STAGE) {
        Some(owner + OWNER_GOAL_A)
    } else if staged(OWNER_GOAL_B_STAGE) {
        Some(owner + OWNER_GOAL_B)
    } else {
        None
    }
}

/// What a sync state is shown while it runs for a match joined: the goal as a private custom
/// match (and with it the "private custom match" byte of the net core, when asked for).
/// Put back when dropped.
struct Shown {
    goal: u32,
    kind: u32,
    private: u8,
    core: Option<(u32, u8)>,
}

impl Shown {
    /// Shows `goal` as a private custom match, and the core's byte as 1 when `core_byte`.
    /// `None` when the game's memory doesn't allow it.
    fn new(goal: u32, core_byte: bool) -> Option<Shown> {
        let g = Game;
        let (kind, private) = (g.word(goal + GOAL_KIND)?, g.byte(goal + GOAL_PRIVATE)?);
        let core = if core_byte {
            let core = g.word(NET_CORE).filter(|c| *c != 0)? + CORE_PRIVATE_CUSTOM;
            Some((core, g.byte(core)?))
        } else {
            None
        };
        if !Game::writable(goal + GOAL_PRIVATE, 1) || !Game::writable(goal + GOAL_KIND, 4) || core.is_some_and(|(at, _)| !Game::writable(at, 1)) {
            return None;
        }
        // SAFETY: checked writable just above; objects of the game thread we are running on.
        unsafe {
            std::ptr::write_unaligned((goal + GOAL_KIND) as usize as *mut u32, GOAL_CUSTOM_MATCH);
            std::ptr::write_unaligned((goal + GOAL_PRIVATE) as usize as *mut u8, 1);
            if let Some((at, _)) = core {
                std::ptr::write_unaligned(at as usize as *mut u8, 1);
            }
        }
        Some(Shown { goal, kind, private, core })
    }
}

impl Drop for Shown {
    fn drop(&mut self) {
        // SAFETY: the same places, written a moment ago.
        unsafe {
            std::ptr::write_unaligned((self.goal + GOAL_KIND) as usize as *mut u32, self.kind);
            std::ptr::write_unaligned((self.goal + GOAL_PRIVATE) as usize as *mut u8, self.private);
            if let Some((at, byte)) = self.core {
                std::ptr::write_unaligned(at as usize as *mut u8, byte);
            }
        }
    }
}

/// The invitation the game was last handed, for [`inviter_is_private`].
static INVITE: Mutex<Option<Noted>> = Mutex::new(None);

struct Noted {
    from: String,
    at: Instant,
}

impl Noted {
    /// Who invited, when younger than [`INVITE_FOR`] at `now`.
    fn from_at(&self, now: Instant) -> Option<&str> {
        (now.duration_since(self.at) < INVITE_FOR).then_some(self.from.as_str())
    }
}

/// The game was handed an accepted invitation from `from` (`UPLAY_GetNextEvent`'s
/// `FriendsGameInviteAccepted`). Cheap: nothing is looked up until the party is to be left.
pub fn note_invite(from: &str) {
    *INVITE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Noted {
        from: from.to_owned(),
        at: Instant::now(),
    });
}

/// Whether `friends` has `name` in a private (invite only) session.
fn in_private_session(friends: &[crate::api::Friend], name: &str) -> bool {
    friends
        .iter()
        .any(|f| (f.id == name || f.username.eq_ignore_ascii_case(name)) && f.session_id != 0 && f.invite_only)
}

/// Whether the inviter of the invitation just accepted is in a private session. Unknown (no
/// invitation lately, no friend list in time) is no: the game's own secede then runs.
fn inviter_is_private() -> bool {
    let from = INVITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|n| n.from_at(Instant::now()).map(str::to_owned));
    let Some(from) = from else {
        info!("party-follow: no invitation was handed to the game lately -> the party is left as usual");
        return false;
    };
    match crate::api::list_friends_within(FRIENDS_WITHIN) {
        Ok(friends) => in_private_session(&friends, &from),
        Err(e) => {
            warn!("party-follow: couldn't look up the inviter {from} ({e}) -> the party is left as usual");
            false
        }
    }
}

/// `FUN_00879b20`: the leader's action on its party. The UI's accept task calls it with 5, which
/// ends the party; with members, and a private match to go to, it tells them 2 instead.
fn leader_action(this: *mut c_void, action: i32) {
    if action == ACTION_SECEDE && leads_members(&Game) && inviter_is_private() {
        info!("party-follow: invitation into a private match accepted as the leader of a party with members -> keeping the party (leader action {ACTION_JOINING} instead of the secede)");
        unsafe { LeaderActionHook.call(this, ACTION_JOINING) };
        return;
    }
    unsafe { LeaderActionHook.call(this, action) }
}

/// `StateLeaderPartyWaitSync::vf08`: sends FollowLeader to the party for a hosted private match.
fn sync_enter(this: *mut c_void) {
    let g = Game;
    let goal = joined_private_match(&g).then(|| current_goal(&g, this as usize as u32)).flatten();
    if let Some(goal) = goal {
        // E3 of the architecture document: what the goal looks like before it is shown otherwise.
        info!(
            "party-follow: joined a private match as the leader of a party with members; goal {goal:#x} has kind {:?}, first byte {:?}",
            g.word(goal + GOAL_KIND),
            g.byte(goal + GOAL_PRIVATE)
        );
        if let Some(_shown) = Shown::new(goal, false) {
            unsafe { SyncEnterHook.call(this) };
            info!("party-follow: FollowLeader for the joined private match sent to the party");
            return;
        }
        warn!("party-follow: the goal can't be written -> the party isn't sent after the leader");
    }
    unsafe { SyncEnterHook.call(this) }
}

/// `StateLeaderPartyWaitSync::vf00`: waits for the members to answer (and kicks those who don't
/// within 15 s); shown the same match as `sync_enter` sent.
fn sync_tick(this: *mut c_void, dt: f32) {
    let g = Game;
    let goal = joined_private_match(&g).then(|| current_goal(&g, this as usize as u32)).flatten();
    if let Some(goal) = goal {
        let pending = g.word(this as usize as u32 + STATE_PENDING);
        let Some(_shown) = Shown::new(goal, true) else {
            unsafe { SyncTickHook.call(this, dt) };
            return;
        };
        unsafe { SyncTickHook.call(this, dt) };
        let after = g.word(this as usize as u32 + STATE_PENDING);
        if pending != after {
            info!("party-follow: waiting for the party: pending transition {pending:?} -> {after:?} ({dt:.2} s ticks)");
        }
        return;
    }
    unsafe { SyncTickHook.call(this, dt) }
}

/// The bytes a function starts with, to know it is the one that was read.
const LEADER_ACTION_PROLOGUE: [u8; 10] = [0x55, 0x8B, 0xEC, 0x6A, 0xFF, 0x68, 0xB3, 0xB9, 0x65, 0x02];
const SYNC_ENTER_PROLOGUE: [u8; 26] = [
    0x55, 0x8B, 0xEC, 0x6A, 0xFF, 0x68, 0x8B, 0xD0, 0x64, 0x02, // push ebp / mov ebp,esp / push -1 / push handler
    0x64, 0xA1, 0x00, 0x00, 0x00, 0x00, 0x50, // mov eax,fs:0 / push eax
    0x81, 0xEC, 0x98, 0x01, 0x00, 0x00, // sub esp,0x198
    0x53, 0x56, 0x57, // push ebx / push esi / push edi
];
const SYNC_TICK_PROLOGUE: [u8; 10] = [0x55, 0x8B, 0xEC, 0x6A, 0xFF, 0x68, 0xD0, 0x56, 0x66, 0x02];

/// The host test in `vf08`: `lea ecx,[edi+0x554]` / `call FUN_01f92c70` / `test al,al` /
/// **`je` over the send** / `movzx ecx,byte [esi+0x5a8]`. `WILD` matches any byte (the call's
/// offset).
const WILD: u16 = 0x100;
const HOST_TEST: [u16; 22] = [
    0x8D, 0x8F, 0x54, 0x05, 0x00, 0x00, // lea ecx,[edi+0x554]
    0xE8, WILD, WILD, WILD, WILD, // call FUN_01f92c70
    0x84, 0xC0, // test al,al
    0x74, 0x5F, // je +0x5f (to the end of the send)
    0x0F, 0xB6, 0x8E, 0xA8, 0x05, 0x00, 0x00, // movzx ecx,byte [esi+0x5a8]
];
/// Where the `je` sits in [`HOST_TEST`], and what it becomes: two `nop`s, so the message is sent
/// by a player that doesn't host the match (it still needs a private custom match goal and a
/// running match session).
const HOST_TEST_JE_AT: usize = 13;
const HOST_TEST_PATCH: [u8; 2] = [0x90, 0x90];
/// How far into `vf08` the host test is, at most.
const HOST_TEST_WITHIN: usize = 0x1c0;

/// Where `pattern` occurs in `code`, when it occurs exactly once.
fn find_once(code: &[u8], pattern: &[u16]) -> Result<usize, usize> {
    let at = |i: usize| code[i..i + pattern.len()].iter().zip(pattern).all(|(b, p)| *p == WILD || u16::from(*b) == *p);
    let hits: Vec<usize> = (0..code.len().saturating_sub(pattern.len() - 1)).filter(|i| at(*i)).collect();
    match hits[..] {
        [one] => Ok(one),
        _ => Err(hits.len()),
    }
}

/// Whether the game has exactly `expected` at `at`.
unsafe fn starts_with(at: usize, expected: &[u8]) -> bool {
    Game::readable(at as u32, expected.len()) && std::slice::from_raw_parts(at as *const u8, expected.len()) == expected
}

/// Hooks the leader's two decisions. Called once at startup; leaves a build it doesn't recognise
/// untouched.
///
/// The party is not kept after the match (`StateLeaveWithoutParty` pushing `eGoal_PartyWaitSync`
/// instead of `eGoal_LeaveThenHost`): measured live on 2026-10-23, members without that hook
/// don't answer it, the clients hang on "joining" and the leader drops them after 15 s. The
/// server brings the members back instead (`escorts`).
pub unsafe fn install(addr: &Addresses) {
    let (Some(action), Some(enter), Some(tick)) = (addr.func_party_leader_action, addr.func_party_sync_enter, addr.func_party_sync_tick) else {
        warn!("party-follow: this game version's party functions aren't known (DX9?) -> skipped");
        return;
    };
    for (name, at, prologue) in [
        ("leader action", action, &LEADER_ACTION_PROLOGUE[..]),
        ("party sync enter", enter, &SYNC_ENTER_PROLOGUE[..]),
        ("party sync tick", tick, &SYNC_TICK_PROLOGUE[..]),
    ] {
        if !starts_with(at, prologue) {
            warn!("party-follow: the {name} @ {at:#x} isn't what it was read as -> skipped (unknown build?)");
            return;
        }
    }
    let Some(code) = crate::game_code() else {
        warn!("party-follow: the game's .text wasn't found -> skipped");
        return;
    };
    let test = match find_once(code, &HOST_TEST) {
        Ok(at) => at,
        Err(n) => {
            warn!("party-follow: the host test of the party sync found {n} times, not once -> skipped (unknown build?)");
            return;
        }
    };
    let text = code.as_ptr();
    if enter.checked_sub(text as usize).is_none_or(|enter| test < enter || test - enter > HOST_TEST_WITHIN) {
        warn!("party-follow: the host test (.text+{test:#x}) isn't in the party sync's enter function -> skipped (unknown build?)");
        return;
    }
    let je = text.add(test + HOST_TEST_JE_AT);
    crate::writemem(je.cast_mut(), &HOST_TEST_PATCH);
    if std::slice::from_raw_parts(je, HOST_TEST_PATCH.len()) != HOST_TEST_PATCH {
        warn!("party-follow: the patch didn't take (write-protected?) -> the party isn't sent after the leader");
        return;
    }
    crate::hooks::hook!(LeaderActionHook, Some(action), leader_action);
    crate::hooks::hook!(SyncEnterHook, Some(enter), sync_enter);
    crate::hooks::hook!(SyncTickHook, Some(tick), sync_tick);
    info!("party-follow: a party's leader invited into a private match keeps its party and takes it along (host test @ .text+{test:#x})");
}

pub unsafe fn deinit() {
    let _ = LeaderActionHook.disable();
    let _ = SyncEnterHook.disable();
    let _ = SyncTickHook.disable();
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Made-up game objects: words and bytes at addresses, the states of sessions.
    #[derive(Default)]
    struct Fake {
        bytes: HashMap<u32, u8>,
        states: HashMap<u32, u32>,
    }

    impl Fake {
        fn put(&mut self, at: u32, value: u32) -> &mut Self {
            for (i, b) in value.to_le_bytes().into_iter().enumerate() {
                self.bytes.insert(at + i as u32, b);
            }
            self
        }
    }

    impl Memory for Fake {
        fn byte(&self, at: u32) -> Option<u8> {
            self.bytes.get(&at).copied()
        }

        fn word(&self, at: u32) -> Option<u32> {
            Some(u32::from_le_bytes([self.byte(at)?, self.byte(at + 1)?, self.byte(at + 2)?, self.byte(at + 3)?]))
        }

        fn state_id(&self, session: u32) -> Option<u32> {
            self.states.get(&session).copied()
        }
    }

    const CORE: u32 = 0x1000_0000;
    const ONLINE: u32 = 0x1100_0000;
    const PARTY: u32 = 0x1200_0000;
    const MATCH: u32 = 0x1300_0000;
    const CONTROLLER: u32 = 0x1400_0000;
    const PLAYER: u32 = 0x1500_0000;
    const RUNNING: u32 = 77;
    const ME: (u32, u32) = (1010, 0);

    /// A leader (id [`ME`]) of a running party whose member list holds `members` (the leader's
    /// own record too), and no match session yet.
    fn leader_with(members: &[(u32, u32)]) -> Fake {
        let mut f = Fake::default();
        f.put(NET_CORE, CORE).put(NET_ONLINE, ONLINE).put(STATE_IN_SESSION, RUNNING);
        f.put(CORE + CORE_PARTY, PARTY).put(CORE + CORE_PLAYER, PLAYER);
        f.put(PLAYER + PLAYER_ID, ME.0).put(PLAYER + PLAYER_ID + 4, ME.1);
        f.put(ONLINE + ONLINE_PARTY, CONTROLLER);
        f.states.insert(PARTY, RUNNING);
        f.put(PARTY + SESSION_STATE, 1).put(PARTY + SESSION_LOCKED, 0);
        f.put(PARTY + SESSION_STORM + 4, 7).put(PARTY + SESSION_STORM + 8, 7);
        // The list: sentinel at CONTROLLER+0x420, nodes of 0x10 bytes after it.
        let head = CONTROLLER + PARTY_MEMBERS;
        let node = |i: usize| head + 0x100 + 0x20 * i as u32;
        f.put(head + 4, if members.is_empty() { head } else { node(0) });
        for (i, id) in members.iter().enumerate() {
            let record = node(i) + 0x10;
            f.put(node(i) + 4, if i + 1 == members.len() { head } else { node(i + 1) }).put(node(i) + 8, record);
            f.put(record, id.0).put(record + 4, id.1);
        }
        f
    }

    /// The same, in the private match `MATCH`, hosted by the session id `host` (`own` is ours).
    fn in_match(mut f: Fake, own: u32, host: u32, private: bool, kind: u32) -> Fake {
        f.put(CORE + CORE_MATCH, MATCH);
        f.states.insert(MATCH, RUNNING);
        f.put(MATCH + SESSION_STATE, 1);
        f.put(MATCH + SESSION_STORM + 4, own).put(MATCH + SESSION_STORM + 8, host);
        f.put(MATCH + SESSION_PRIVATE, u32::from(private)).put(MATCH + SESSION_KIND, kind);
        f
    }

    #[test]
    fn the_patches_keep_their_sites_lengths() {
        assert_eq!(&HOST_TEST[HOST_TEST_JE_AT..HOST_TEST_JE_AT + 2], &[0x74, 0x5F], "the je over the send");
        assert_eq!(HOST_TEST_PATCH.len(), 2);
        assert_eq!(HOST_TEST.len(), 22);
        assert_eq!(LEADER_ACTION_PROLOGUE.len(), 10);
        assert_eq!(SYNC_ENTER_PROLOGUE.len(), 26);
        assert_eq!(SYNC_TICK_PROLOGUE.len(), 10);
    }

    #[test]
    fn a_site_counts_only_when_it_is_there_once() {
        let site: Vec<u8> = HOST_TEST.iter().map(|p| if *p == WILD { 0x93 } else { *p as u8 }).collect();
        let mut code = vec![0x90; 64];
        assert_eq!(find_once(&code, &HOST_TEST), Err(0));
        code.extend_from_slice(&site);
        assert_eq!(find_once(&code, &HOST_TEST), Ok(64));
        assert_eq!(code[64 + HOST_TEST_JE_AT], 0x74);
        // The call's offset is not part of the match.
        code.extend_from_slice(&site);
        let n = code.len();
        code[n - 14] = 0x11;
        assert_eq!(find_once(&code, &HOST_TEST), Err(2));
        // A byte that is part of it is.
        code[n - 8] ^= 1;
        assert_eq!(find_once(&code, &HOST_TEST), Ok(64));
        // A site cut off at the end of the code doesn't match, or panic.
        assert_eq!(find_once(&site[..10], &HOST_TEST), Err(0));
    }

    #[test]
    fn a_party_with_other_members_is_one_to_keep() {
        assert!(!leads_members(&leader_with(&[ME])), "alone in its party");
        assert!(!leads_members(&leader_with(&[])), "no records at all");
        assert!(leads_members(&leader_with(&[ME, (1011, 0)])));
        assert!(leads_members(&leader_with(&[(1011, 0), ME, (1012, 0)])));
        // The same low word with another high word is another player.
        assert!(leads_members(&leader_with(&[ME, (1010, 1)])));
        assert_eq!(other_members(&leader_with(&[ME, (1011, 0), (1012, 0)])), Some(vec![(1011, 0), (1012, 0)]));
    }

    #[test]
    fn a_party_that_is_not_there_or_not_running_or_locked_is_not_led() {
        assert!(!leads_members(&Fake::default()), "no net core");
        let mut f = leader_with(&[ME, (1011, 0)]);
        f.put(CORE + CORE_PARTY, 0);
        assert!(!leads_members(&f), "no party session");
        let mut f = leader_with(&[ME, (1011, 0)]);
        f.states.insert(PARTY, RUNNING + 1);
        assert!(!leads_members(&f), "not running");
        let mut f = leader_with(&[ME, (1011, 0)]);
        f.put(PARTY + SESSION_LOCKED, 1);
        assert!(!leads_members(&f), "locked");
        let mut f = leader_with(&[ME, (1011, 0)]);
        f.put(PARTY + SESSION_STORM + 8, 9);
        assert!(!leads_members(&f), "a member of somebody else's party");
        let mut f = leader_with(&[ME, (1011, 0)]);
        f.put(PARTY + SESSION_STORM + 8, u32::MAX);
        assert!(!leads_members(&f), "no host yet");
        let mut f = leader_with(&[ME, (1011, 0)]);
        f.bytes.remove(&(CONTROLLER + PARTY_MEMBERS + 4));
        assert!(!leads_members(&f), "an unreadable list");
    }

    #[test]
    fn a_list_that_does_not_end_is_not_trusted() {
        let mut f = leader_with(&[ME, (1011, 0)]);
        // The second node points back at the first.
        let head = CONTROLLER + PARTY_MEMBERS;
        f.put(head + 0x100 + 0x20 + 4, head + 0x100);
        assert_eq!(other_members(&f), None);
    }

    #[test]
    fn only_a_private_match_the_leader_joined_shows_the_party_sync_its_hosted_one() {
        let party = || leader_with(&[ME, (1011, 0)]);
        assert!(joined_private_match(&in_match(party(), 5, 9, true, 0)), "joined: someone else hosts it");
        assert!(!joined_private_match(&in_match(party(), 5, 5, true, 0)), "hosted: the game's own case");
        assert!(joined_private_match(&in_match(party(), 5, 9, true, 0)));
        assert!(!joined_private_match(&in_match(party(), 5, 9, false, 0)), "a public match takes no party along");
        assert!(!joined_private_match(&in_match(party(), 5, 9, true, 1)), "a party session isn't a match");
        assert!(!joined_private_match(&in_match(leader_with(&[ME]), 5, 9, true, 0)), "no members");
        assert!(!joined_private_match(&party()), "no match session");
        let mut not_running = in_match(party(), 5, 9, true, 0);
        not_running.states.insert(MATCH, RUNNING + 1);
        assert!(!joined_private_match(&not_running), "the match isn't up yet");
        assert!(!joined_private_match(&in_match(party(), 5, u32::MAX, true, 0)), "nobody hosts yet");
    }

    #[test]
    fn the_host_test_is_the_games() {
        assert!(hosts(7, 7));
        assert!(!hosts(7, 8));
        assert!(!hosts(u32::MAX, u32::MAX), "-1 is no host");
    }

    #[test]
    fn the_goal_is_the_one_the_game_picks() {
        let state = 0x1600_0000;
        let owner = 0x1700_0000;
        let mut f = Fake::default();
        f.put(state + STATE_OWNER, owner);
        assert_eq!(current_goal(&f, state), None, "no goal staged");
        f.put(owner + OWNER_GOAL_B_STAGE, 5);
        assert_eq!(current_goal(&f, state), Some(owner + OWNER_GOAL_B), "the second slot");
        f.put(owner + OWNER_GOAL_A_STAGE, 4);
        assert_eq!(current_goal(&f, state), Some(owner + OWNER_GOAL_A), "the first slot goes first");
        f.put(owner + OWNER_GOAL_A_STAGE, 7).put(owner + OWNER_GOAL_B_STAGE, 3);
        assert_eq!(current_goal(&f, state), None, "stages outside 4..=6 are no goal");
    }

    #[test]
    fn an_invitation_counts_for_a_minute() {
        let now = Instant::now();
        let noted = Noted { from: "Host".into(), at: now };
        assert_eq!(noted.from_at(now), Some("Host"));
        assert_eq!(noted.from_at(now + INVITE_FOR - Duration::from_secs(1)), Some("Host"));
        assert_eq!(noted.from_at(now + INVITE_FOR), None);
    }

    #[test]
    fn an_inviter_is_private_when_it_is_in_an_invite_only_session() {
        let friend = |id: &str, name: &str, session_id: u32, invite_only: bool| crate::api::Friend {
            id: id.into(),
            username: name.into(),
            is_online: true,
            session_id,
            invite_only,
            session_data: Vec::new(),
            pid: 1,
        };
        let friends = [friend("a1", "Alpha", 5, true), friend("b2", "Beta", 6, false), friend("c3", "Gamma", 0, true)];
        assert!(in_private_session(&friends, "a1"), "by id");
        assert!(in_private_session(&friends, "alpha"), "by name, as the game spells it");
        assert!(!in_private_session(&friends, "b2"), "a public session");
        assert!(!in_private_session(&friends, "c3"), "no session");
        assert!(!in_private_session(&friends, "nobody"));
    }
}
