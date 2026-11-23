//! Every player's name as the game session has it (0.5.6's N1, the witnesses' side). Each player's
//! game sends the others its own player's description (Storm's replicated `NetPlayerDescriptor`):
//! their id and name, the name a cheat table rewrites ("name in lobby") and the one every other
//! game shows in the lobby and the scoreboard. So any game, not only the host's, can see a name
//! that isn't the player's and say so (`NameSeen`, as the host's chat check does).
//!
//! The way there (the DX11 and DX9 decompiles, the same offsets in both):
//!
//! ```text
//! NetCore     = what its constructor returns (hooked), or [the engine global - 4] (DX11
//!               0x03383df4, DX9 0x032b5dc4: the cheat table's base)
//! session     = [NetCore + 0x5d0]         the game session (+0x5d4 the party's)
//! list        = session + 0x584           a ring: node = [list + 4] .. until node == list
//! node        +4 next, +8 NetPeer
//! NetPeer     +0 Storm's peer id, +0xa has a description, +0xc the description
//! description +0x10 the player's account number (u64), +0x18 name, +0x34 a hardware id, +0x50 tag
//!             (Storm DataStrings: a Gear::GearBasicString at +0xc, whose text block is at +8:
//!             +4 its length, +0xc the text, not ended by a 0)
//! ```
//! `[NetCore + 0x5d8]` is the own player's description.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::PoisonError;

use crate::dataversion::read;
use crate::dataversion::read_u32;

const SESSION_AT: usize = 0x5d0;
const PARTY_AT: usize = 0x5d4;
const OWN_AT: usize = 0x5d8;
const LIST_AT: usize = 0x584;
const PEER_DESCRIPTION_AT: usize = 0xc;
const PEER_HAS_DESCRIPTION_AT: usize = 0xa;
const ID_AT: usize = 0x10;
const NAME_AT: usize = 0x18;
const SECOND_AT: usize = 0x34;
const TAG_AT: usize = 0x50;
/// A string's GearBasicString inside its DataString.
const GEAR_AT: usize = 0xc;
/// The most members followed (a session has 16 at most).
const MOST: usize = 32;

/// One player in a session, as its description says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub storm: i32,
    pub id: u64,
    pub name: String,
    pub tag: String,
    pub second: String,
}

/// NetCore, as its constructor (hooked, `hooks.rs`) made it; else the global beside the engine's.
fn net_core() -> Option<usize> {
    #[allow(clippy::useless_conversion)]
    if let Some(c) = unsafe { crate::hooks::NET_CORE_ADDR } {
        return usize::try_from(c).ok().filter(|c| *c != 0);
    }
    let global = crate::dataversion::engine_global()?.checked_sub(4)?;
    read_u32(global).filter(|c| *c != 0).map(|c| c as usize)
}

/// The longest text a description carries (Storm's limit).
const LONGEST: usize = 160;

/// A DataString's text, by its block's length (the text isn't ended by a 0: the gamebox read
/// "gbox1wn" past "gbox1").
fn text(description: usize, at: usize) -> String {
    let Some(block) = read_u32(description + at + GEAR_AT + 8).filter(|b| *b != 0).map(|b| b as usize) else { return String::new() };
    let Some(len) = read_u32(block + 4).map(|l| l as usize).filter(|l| *l <= LONGEST) else { return String::new() };
    let bytes: Vec<u8> = (0..len).map_while(|i| read::<u8>(block + 0xc + i)).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn describe(storm: i32, description: usize) -> Option<Member> {
    Some(Member {
        storm,
        id: read::<u64>(description + ID_AT)?,
        name: text(description, NAME_AT),
        tag: text(description, TAG_AT),
        second: text(description, SECOND_AT),
    })
}

/// The members of the session at `NetCore + at` (the game's or the party's).
fn members_of(at: usize) -> Option<Vec<Member>> {
    let session = read_u32(net_core()? + at).filter(|s| *s != 0)? as usize;
    let list = session + LIST_AT;
    let mut node = read_u32(list + 4)? as usize;
    let mut out = Vec::new();
    while node != list && node != 0 && out.len() < MOST {
        let peer = read_u32(node + 8)? as usize;
        if peer != 0 && read::<u8>(peer + PEER_HAS_DESCRIPTION_AT) == Some(1) {
            if let Some(m) = describe(read::<i32>(peer)?, peer + PEER_DESCRIPTION_AT) {
                out.push(m);
            }
        }
        node = read_u32(node + 4)? as usize;
    }
    Some(out)
}

/// The game session's members (the match's), as their own games describe them.
pub fn members() -> Vec<Member> {
    std::panic::catch_unwind(|| members_of(SESSION_AT)).ok().flatten().unwrap_or_default()
}

/// The own player's number in the game session (its Storm peer id + 1), from the session's list.
pub fn own_peer() -> Option<u8> {
    let own = own()?.id;
    members().into_iter().find(|m| m.id == own).and_then(|m| u8::try_from(m.storm + 1).ok()).filter(|p| *p >= 1)
}

/// The own player's description.
pub fn own() -> Option<Member> {
    std::panic::catch_unwind(|| describe(-1, read_u32(net_core()? + OWN_AT)? as usize)).ok().flatten()
}

/// The match's other players' names as their descriptions say, against the server's for their
/// numbers: what a read found (`peer` → the name shown), when each was first seen without the
/// server's name, and what's been said already.
#[derive(Default)]
struct Witness {
    last: HashMap<u8, String>,
    unnamed_since: HashMap<u8, std::time::Instant>,
    said: Vec<(u8, String)>,
}
static WITNESS: Mutex<Option<Witness>> = Mutex::new(None);

/// How long a player can go without the server's name for their number before the server is
/// asked by their account instead (a game that never says its own number: the Surface's DX9,
/// 24 Nov). Most get theirs within a minute of spawning, so this keeps the asks rare.
const UNNAMED_FOR: std::time::Duration = std::time::Duration::from_secs(180);

/// One to tell the server: number, account, the name shown, and whether the server's name for the
/// number was known (and differed) or the server is to check it by the account.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    peer: u8,
    account: u32,
    shown: String,
    checked: bool,
}

/// What one read comes to: the players whose description has another name than the server's for
/// their number on this read and the one before (and not said yet); and the ones without the
/// server's name for `UNNAMED_FOR`, for the server to check by their account (once a name).
fn compare(w: &mut Witness, members: &[Member], own: Option<u64>, real: impl Fn(u8) -> Option<String>, now: std::time::Instant) -> Vec<Seen> {
    let mut last = HashMap::new();
    let mut out = Vec::new();
    for m in members {
        // Storm's peer id is the session's index; the server numbers from 1 (`peer_names`).
        let Some(peer) = u8::try_from(m.storm + 1).ok().filter(|p| *p >= 1) else { continue };
        if Some(m.id) == own {
            continue;
        }
        let shown: String = m.name.chars().filter(|c| !c.is_control()).take(40).collect();
        if shown.trim().is_empty() {
            continue;
        }
        let account = u32::try_from(m.id).unwrap_or(0);
        let again = w.last.get(&peer) == Some(&shown);
        last.insert(peer, shown.clone());
        let checked = match real(peer) {
            Some(real) if shown.trim().eq_ignore_ascii_case(real.trim()) => {
                w.unnamed_since.remove(&peer);
                continue;
            }
            Some(_) => {
                w.unnamed_since.remove(&peer);
                true
            }
            None => {
                let since = *w.unnamed_since.entry(peer).or_insert(now);
                if account == 0 || now.duration_since(since) < UNNAMED_FOR {
                    continue;
                }
                false
            }
        };
        if !again {
            continue;
        }
        let item = (peer, shown.to_lowercase());
        if !w.said.contains(&item) && w.said.len() < 64 {
            w.said.push(item);
            out.push(Seen { peer, account, shown, checked });
        }
    }
    w.last = last;
    out
}

/// From the match state's loop: in a match, every player's description against the server's
/// names (once both are known), said as a host's chat check says one (`NameSeen`); out of one,
/// forgotten.
pub fn tick(in_match: bool) {
    let mut guard = WITNESS.lock().unwrap_or_else(PoisonError::into_inner);
    if !in_match {
        *guard = None;
        return;
    }
    let w = guard.get_or_insert_with(Witness::default);
    let own = own().map(|m| m.id);
    for seen in compare(w, &members(), own, crate::pawns::peer_name, std::time::Instant::now()) {
        if seen.checked {
            let real = crate::pawns::peer_name(seen.peer).unwrap_or_default();
            tracing::warn!("Net names: number {} ({real} to the server) is shown to everyone as {:?}", seen.peer, seen.shown);
        } else {
            tracing::info!("Net names: number {} (account {}) has no name from the server yet: asking it about {:?}", seen.peer, seen.account, seen.shown);
        }
        let _ = std::thread::Builder::new()
            .name("fe-name-seen".into())
            .spawn(move || crate::api::name_seen(u32::from(seen.peer), &seen.shown, seen.account));
    }
}

/// The test build's `netname <number> <name>`: this game's copy of a player's name rewritten (as a
/// cheat table's would reach everyone), to see the witness say it.
#[cfg(feature = "test-commands")]
pub fn test_rename(storm: i32, name: &str) {
    let done = std::panic::catch_unwind(|| -> Option<()> {
        let session = read_u32(net_core()? + SESSION_AT)? as usize;
        let list = session + LIST_AT;
        let mut node = read_u32(list + 4)? as usize;
        while node != list && node != 0 {
            let peer = read_u32(node + 8)? as usize;
            if read::<i32>(peer) == Some(storm) {
                let block = read_u32(peer + PEER_DESCRIPTION_AT + NAME_AT + GEAR_AT + 8)? as usize;
                let cap = read_u32(block + 8)? as usize;
                let bytes = name.as_bytes();
                if bytes.len() > cap.min(LONGEST) {
                    return None;
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), (block + 0xc) as *mut u8, bytes.len());
                    *((block + 0xc + bytes.len()) as *mut u8) = 0;
                    *((block + 4) as *mut u32) = bytes.len() as u32;
                }
                return Some(());
            }
            node = read_u32(node + 4)? as usize;
        }
        None
    });
    tracing::info!("Net names: test: number {} renamed {name:?}: {}", storm + 1, matches!(done, Ok(Some(()))));
}

/// The test build's `peers`: everything above, logged.
#[cfg(feature = "test-commands")]
pub fn log_all() {
    let core = net_core();
    tracing::info!("Net names: NetCore {core:x?}, own {:?}", own());
    for (what, at) in [("game", SESSION_AT), ("party", PARTY_AT)] {
        let session = core.and_then(|c| read_u32(c + at));
        let members = std::panic::catch_unwind(|| members_of(at)).ok().flatten();
        tracing::info!("Net names: the {what} session {session:x?}: {members:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(storm: i32, id: u64, name: &str) -> Member {
        Member { storm, id, name: name.into(), tag: String::new(), second: String::new() }
    }

    #[test]
    fn another_name_twice_in_a_row_is_said_once() {
        let real = |p: u8| match p {
            1 => Some("gbox1".to_string()),
            2 => Some("gbox2".to_string()),
            _ => None,
        };
        let t = std::time::Instant::now();
        let mut w = Witness::default();
        let clean = [m(0, 1003, "gbox1"), m(1, 1004, "GBox2 ")];
        assert!(compare(&mut w, &clean, Some(1003), real, t).is_empty(), "the real names, case and spaces aside");
        let renamed = [m(0, 1003, "gbox1"), m(1, 1004, "Tea_bag"), m(2, 1008, "nobody known")];
        assert!(compare(&mut w, &renamed, Some(1003), real, t).is_empty(), "one read isn't enough");
        let said = compare(&mut w, &renamed, Some(1003), real, t);
        assert_eq!(said, vec![Seen { peer: 2, account: 1004, shown: "Tea_bag".into(), checked: true }]);
        assert!(compare(&mut w, &renamed, Some(1003), real, t).is_empty(), "said once");
        // Number 3 has no name from the server: after UNNAMED_FOR the server checks it by account.
        let later = t + UNNAMED_FOR;
        let said = compare(&mut w, &renamed, Some(1003), real, later);
        assert_eq!(said, vec![Seen { peer: 3, account: 1008, shown: "nobody known".into(), checked: false }]);
        assert!(compare(&mut w, &renamed, Some(1003), real, later).is_empty(), "asked once");
        // The own player is the own check's (pawns.rs), not this one's.
        let mut w = Witness::default();
        let own = [m(0, 1003, "Tea_bag")];
        compare(&mut w, &own, Some(1003), real, t);
        assert!(compare(&mut w, &own, Some(1003), real, t).is_empty());
    }
}
