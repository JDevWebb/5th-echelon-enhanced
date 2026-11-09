//! Where the chat relay (the hooks' `chat_relay`) finds what it uses in the game, and its one
//! decision: who gets a player's chat line. Not a crash fix, but found and checked the same way.
//!
//! In a match the game sends a chat line from the host to every player, but from anyone else
//! only to the host (a replica's message goes to its object's master). The host shows it and
//! passes it on to nobody. The relay hooks the match manager's chat display on the host and sends
//! each line from another player on to the rest, as the host sends to one player at a time
//! elsewhere (a master unicast, as with kill rewards). Everything here is the same in both
//! executables (DX11 addresses first, DX9 second).

use super::crash_fixes::find_unique;
use super::crash_fixes::parse;
use super::crash_fixes::Image;

/// The match manager's chat display, `thiscall(manager, message)` (`0x104ed60`, `0x12e1b80`). It
/// reads the message's team chat flag (`+0x64`) and team (`+0x54`) and shows the line if it's for
/// everyone or the reader's team; the receivers' filter, which the relay leaves as it is.
const DISPLAY: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 83 EC 1C 53 56 57 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 64 A3 00 00 00 00 \
                       8B 81 88 00 00 00 8B 75 08 8D 4D D8 51 8B CE 89 45 F0 E8 ?? ?? ?? ?? C7 45 FC 00 00 00 00 8D 55 E4 52 8B CE E8 ?? ?? ?? ?? \
                       C6 45 FC 01 80 7E 64 00 0F B6 7E 54";

/// The game's own chat send, from its length check (a line of more than [`MAX_TEXT`] characters is
/// dropped) to the release of the send's result (`0x10b5150`, `0x13479a0`). The master broadcasts
/// (`mov edx,[channel]` ... `call broadcast`), a replica sends to the master. Gives the channel both
/// pass, the broadcast send and the release.
const SENDER: &str = "8B 43 04 85 C0 74 1E 48 3D 90 00 00 00 7E 16 32 C0 8B 4D F4 64 89 0D 00 00 00 00 59 5F 5E 5B 8B E5 5D C2 0C 00 \
                      8D 4D 84 E8 ?? ?? ?? ?? C7 45 FC 00 00 00 00 8B 17 8B 82 30 04 00 00 8B CF FF D0 83 7D 10 00 0F 95 C1 0F B6 D1 52 50 \
                      8B 45 08 53 50 8D 4D 84 E8 ?? ?? ?? ?? 83 EC 08 83 7E 14 00 89 65 0C 8B C4 6A FF 75 37 8B 15 ?? ?? ?? ?? 89 10 \
                      8B 15 ?? ?? ?? ?? 33 C9 89 50 04 8B 76 6C 51 8D 45 84 50 8D 4D EC 51 E8 ?? ?? ?? ?? 83 C4 04 50 8D 55 0C 56 52 \
                      E8 ?? ?? ?? ?? 83 C4 20 EB 32 8B 0D ?? ?? ?? ?? 89 08 8B 15 ?? ?? ?? ?? 89 50 04 8B 76 6C 8D 45 84 50 8D 4D EC 51 \
                      E8 ?? ?? ?? ?? 83 C4 04 50 8D 55 0C 56 52 E8 ?? ?? ?? ?? 83 C4 1C 8D 4D 0C E8";
/// In [`SENDER`]: the channel's address (twice, its two words), the broadcast's and the release's rel32.
const SENDER_CHANNEL: usize = 107;
const SENDER_CHANNEL_HIGH: usize = 115;
const SENDER_BROADCAST: usize = 151;
const SENDER_RELEASE: usize = 214;

/// The master's send to one replica, `cdecl(result, handle, peer, mode, message, 0, -1, channel)`
/// (`0x1fdd530`, `0x1fb6040`): the broadcast's, with the peer added.
const UNICAST: &str = "55 8B EC 83 EC 34 8B 0D ?? ?? ?? ?? 8B 15 ?? ?? ?? ?? 53 83 C8 FF 89 4D D8 8B 4D 1C 89 45 D4 89 45 E4 89 45 F0 \
                       A1 ?? ?? ?? ?? 56 51 33 DB 89 55 DC 8B 55 24 89 45 F4 83 EC 08 8B C4 89 5D D0 89 5D E8 89 5D EC C7 45 CC ?? ?? ?? ?? \
                       89 5D F8 89 5D FC C6 45 E0 01 89 10 8B 4D 28 89 48 04 E8";
/// The game's sends sit side by side: to the master, the broadcast, then the unicast.
const UNICAST_AFTER_BROADCAST: usize = 0xd0;

/// The chat message's unicast mode by name; its getter is the only code that pushes it
/// (`0x1040550`, `0x12d3830`), `thiscall(any, out: *mut [u32; 2])`, `ret 4`.
const MODE_NAME: &[u8] = b"NetMessageMatchManager_ChatString_PR_UR\0";
/// The getter's start, from the push of the name: `55 8B EC 6A FF 68 ...` up to its one-time guard.
const MODE_GETTER: &str = "55 8B EC 6A FF 68 ?? ?? ?? ?? 64 A1 00 00 00 00 50 A1 ?? ?? ?? ?? 33 C5 50 8D 45 F4 64 A3 00 00 00 00 B8 01 00 00 00 84 05";
const MODE_PUSH_AFTER: usize = 0x3c;
const MODE_RETURN_AFTER: usize = 0x78;
const MODE_RETURN: [u8; 3] = [0xC2, 0x04, 0x00];

/// A net object turning master: `mov eax,[esi+0x50]` (its session) / `mov [esi+0x14],ebx` /
/// `mov eax,[eax+own]` / `mov [esi+0x18],eax`; the session's own peer becomes the object's owner
/// (`0x82a1d6`, `0xac5556`).
const OWN_PEER: &str = "8B 46 50 89 5E 14 8B 80 ?? ?? ?? ?? 89 46 18";
const OWN_PEER_AT: usize = 8;
/// The session of a net object, `[object+0x50]` (in [`OWN_PEER`]).
pub const SESSION: usize = 0x50;

/// The incoming queue's check (`0x80f2bb`, `0xaab20b`): it calls the object's session getter
/// (vtable `+0x80`), walks the session's peer list (`mov ecx,[eax+first]` / `add eax,head`) and
/// compares each entry's first word with a waiting message's sender (`+0x04`).
const PEER_LIST: &str = "8B 82 80 00 00 00 8B CE 89 7D F8 FF D0 8B 88 ?? ?? ?? ?? 05 ?? ?? ?? ?? 3B C8 74 18 8B 51 08 8B 5D F4 39 1A";
const PEER_LIST_FIRST: usize = 15;
const PEER_LIST_HEAD: usize = 20;

/// The longest line the game sends (its own check, in [`SENDER`]).
pub const MAX_TEXT: u32 = 0x90;
/// The host is peer 0; the others count from 1 in the order they joined.
pub const HOST_PEER: u32 = 0;
/// No peer (the game's "none").
pub const NO_PEER: u32 = u32::MAX;
/// More peers than a match can hold: a list this long isn't one.
pub const MAX_PEERS: usize = 32;

/// What the relay uses, by address, and the session's layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sites {
    /// The chat display (hooked).
    pub display: usize,
    /// The master's send to one replica.
    pub unicast: usize,
    /// The chat message's unicast mode getter.
    pub mode: usize,
    /// The channel every send passes (two words).
    pub channel: usize,
    /// The release of a send's result.
    pub release: usize,
    /// In the session: its own peer.
    pub own_peer: usize,
    /// In the session: the peer list's head (the first node is at `head + 4`).
    pub peers: usize,
}

/// Everything the relay uses, if this build has each piece exactly once and they fit together.
pub fn locate(img: &Image) -> Result<Sites, String> {
    let display = img.find(&parse(DISPLAY)).map_err(|n| format!("the chat display matched {n} times"))?;

    let sender = img.find(&parse(SENDER)).map_err(|n| format!("the chat send matched {n} times"))?;
    let channel = img.u32(sender + SENDER_CHANNEL).ok_or("the chat send's channel isn't readable")?;
    if img.u32(sender + SENDER_CHANNEL_HIGH) != Some(channel + 4) {
        return Err("the chat send's channel isn't two words".into());
    }
    let broadcast = img.rel32_target(sender + SENDER_BROADCAST).ok_or("the chat send's broadcast isn't readable")?;
    let release = img.rel32_target(sender + SENDER_RELEASE).ok_or("the chat send's release isn't readable")?;

    let unicast = img.find(&parse(UNICAST)).map_err(|n| format!("the unicast send matched {n} times"))?;
    if unicast != broadcast + UNICAST_AFTER_BROADCAST {
        return Err(format!("the unicast send at {unicast:#010x} isn't beside the broadcast at {broadcast:#010x}"));
    }

    let mode = mode_getter(img)?;

    let at = img.find(&parse(OWN_PEER)).map_err(|n| format!("a net object turning master matched {n} times"))?;
    let own_peer = img.u32(at + OWN_PEER_AT).ok_or("the session's own peer isn't readable")?;

    let at = img.find(&parse(PEER_LIST)).map_err(|n| format!("the incoming queue's peer check matched {n} times"))?;
    let first = img.u32(at + PEER_LIST_FIRST).ok_or("the peer list isn't readable")?;
    let peers = img.u32(at + PEER_LIST_HEAD).ok_or("the peer list isn't readable")?;
    if first != peers + 4 {
        return Err(format!("the peer list's first node ({first:#x}) isn't after its head ({peers:#x})"));
    }

    Ok(Sites {
        display,
        unicast,
        mode,
        channel,
        release,
        own_peer,
        peers,
    })
}

/// The getter that pushes [`MODE_NAME`], checked from its start to its `ret 4`.
fn mode_getter(img: &Image) -> Result<usize, String> {
    let name = find_unique(img.rdata, &MODE_NAME.iter().map(|b| Some(*b)).collect::<Vec<_>>())
        .map(|at| img.rdata_va + at)
        .map_err(|n| format!("the chat message's unicast mode name was found {n} times"))?;
    // push 0 / push 0 / push name / mov ecx,...
    let mut push = parse("6A 00 6A 00 68");
    push.extend((name as u32).to_le_bytes().map(Some));
    push.push(Some(0xB9));
    let at = img.find(&push).map_err(|n| format!("the chat message's unicast mode was registered {n} times"))?;
    let start = at.checked_sub(MODE_PUSH_AFTER).ok_or("the unicast mode getter starts before the code")?;
    let prologue = parse(MODE_GETTER);
    let fits = img
        .bytes(start, prologue.len())
        .is_some_and(|code| code.iter().zip(&prologue).all(|(b, want)| want.is_none_or(|w| *b == w)));
    if !fits || img.bytes(start + MODE_RETURN_AFTER, MODE_RETURN.len()) != Some(&MODE_RETURN[..]) {
        return Err(format!("the code at {start:#010x} isn't the unicast mode getter"));
    }
    Ok(start)
}

/// Why a line isn't passed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The host's own line (the host already sent it to everyone).
    FromHost,
    /// A sender the session doesn't list (gone, or not a peer at all).
    UnknownSender,
}

/// Who gets a line the host received from `sender`: every listed peer but the host, the host's own
/// session (`own`, the host's peer after a host migration) and the sender, who showed it already.
/// Empty with a single other player.
pub fn relay_targets(sender: u32, own: u32, peers: &[u32]) -> Result<Vec<u32>, Skip> {
    if sender == HOST_PEER || sender == own {
        return Err(Skip::FromHost);
    }
    if sender == NO_PEER || !peers.contains(&sender) {
        return Err(Skip::UnknownSender);
    }
    let mut targets = Vec::new();
    for &peer in peers {
        if ![HOST_PEER, own, sender, NO_PEER].contains(&peer) && !targets.contains(&peer) {
            targets.push(peer);
        }
    }
    Ok(targets)
}

/// Whether a line of `len` characters in a buffer of `cap` is one the game itself would send:
/// not empty, at most [`MAX_TEXT`], and inside its buffer.
pub fn text_fits(len: u32, cap: u32) -> bool {
    (1..=MAX_TEXT).contains(&len) && len <= cap
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(signature: &str, wildcard: u8) -> Vec<u8> {
        parse(signature).into_iter().map(|b| b.unwrap_or(wildcard)).collect()
    }

    /// Made-up code with every piece once, as the DX11 build has them (offsets kept, contents
    /// made up), and `.rdata` with the mode's name.
    fn game() -> (Vec<u8>, Vec<u8>) {
        const TEXT: usize = 0x40_1000;
        const RDATA: usize = 0x60_0000;
        let mut text = vec![0xCC; 0x100];
        let put = |code: &[u8], text: &mut Vec<u8>| {
            let at = text.len();
            text.extend_from_slice(code);
            text.extend([0xCC; 0x20]);
            at
        };
        put(&bytes(DISPLAY, 0x11), &mut text);
        let sender = put(&bytes(SENDER, 0x00), &mut text);
        let broadcast = text.len();
        text.extend(vec![0x90; UNICAST_AFTER_BROADCAST]);
        put(&bytes(UNICAST, 0x22), &mut text);
        // The mode getter: its prologue, filler, the push of the name, filler, its ret 4.
        let getter = text.len();
        text.extend(bytes(MODE_GETTER, 0x33));
        text.resize(getter + MODE_PUSH_AFTER, 0x90);
        text.extend(parse("6A 00 6A 00 68").into_iter().map(Option::unwrap));
        text.extend((RDATA as u32 + 8).to_le_bytes());
        text.extend([0xB9, 0x44, 0x44, 0x44, 0x44]);
        text.resize(getter + MODE_RETURN_AFTER, 0x90);
        text.extend(MODE_RETURN);
        text.extend([0xCC; 0x20]);
        let mut own = bytes(OWN_PEER, 0x00);
        own[OWN_PEER_AT..OWN_PEER_AT + 4].copy_from_slice(&0x55cu32.to_le_bytes());
        put(&own, &mut text);
        let mut list = bytes(PEER_LIST, 0x00);
        list[PEER_LIST_FIRST..PEER_LIST_FIRST + 4].copy_from_slice(&0x588u32.to_le_bytes());
        list[PEER_LIST_HEAD..PEER_LIST_HEAD + 4].copy_from_slice(&0x584u32.to_le_bytes());
        put(&list, &mut text);
        // The send's channel and calls.
        let rel = |from: usize, to: usize| (to as u32).wrapping_sub((TEXT + from + 4) as u32).to_le_bytes();
        text[sender + SENDER_CHANNEL..][..4].copy_from_slice(&0x0070_0000u32.to_le_bytes());
        text[sender + SENDER_CHANNEL_HIGH..][..4].copy_from_slice(&0x0070_0004u32.to_le_bytes());
        let r = rel(sender + SENDER_BROADCAST, TEXT + broadcast);
        text[sender + SENDER_BROADCAST..][..4].copy_from_slice(&r);
        text.extend([0xCC; 0x10]);
        let release = text.len() - 8;
        let r = rel(sender + SENDER_RELEASE, TEXT + release);
        text[sender + SENDER_RELEASE..][..4].copy_from_slice(&r);
        let mut rdata = vec![0u8; 8];
        rdata.extend_from_slice(MODE_NAME);
        rdata.extend(b"NetMessageMatchManager_ChatString_PR_URO\0");
        (text, rdata)
    }

    #[test]
    fn finds_every_piece_and_reads_the_layout() {
        let (text, rdata) = game();
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &rdata, 0x60_0000);
        let sites = locate(&img).unwrap();
        assert_eq!(img.bytes(sites.display, 3), Some(&[0x55, 0x8B, 0xEC][..]));
        assert_eq!(img.bytes(sites.unicast, 6), Some(&[0x55, 0x8B, 0xEC, 0x83, 0xEC, 0x34][..]));
        assert_eq!(img.bytes(sites.mode + MODE_RETURN_AFTER, 3), Some(&MODE_RETURN[..]));
        assert_eq!(sites.channel, 0x0070_0000);
        assert_eq!(img.bytes(sites.release - 8, 1), Some(&[0xCC][..]));
        assert_eq!((sites.own_peer, sites.peers), (0x55c, 0x584));
    }

    #[test]
    fn leaves_a_build_whose_pieces_dont_fit_alone() {
        let (text, rdata) = game();
        // The unicast send not beside the broadcast.
        let at = find_unique(&text, &parse(UNICAST)).unwrap();
        let mut moved = text.clone();
        moved.insert(at, 0x90);
        let img = crate::crash_fixes::tests::image(&moved, 0x40_1000, &rdata, 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("isn't beside"));
        // The mode's name twice.
        let twice = [rdata.clone(), MODE_NAME.to_vec()].concat();
        let img = crate::crash_fixes::tests::image(&text, 0x40_1000, &twice, 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("found 2 times"));
        // The getter without its ret 4.
        let ret = text.windows(3).position(|w| w == MODE_RETURN).unwrap();
        let mut no_ret = text.clone();
        no_ret[ret] = 0xC3;
        let img = crate::crash_fixes::tests::image(&no_ret, 0x40_1000, &rdata, 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("isn't the unicast mode getter"));
        // The chat display twice.
        let display = bytes(DISPLAY, 0x11);
        let img_text = [text.clone(), display].concat();
        let img = crate::crash_fixes::tests::image(&img_text, 0x40_1000, &rdata, 0x60_0000);
        assert!(locate(&img).unwrap_err().contains("matched 2 times"));
    }

    #[test]
    fn signatures_are_well_formed() {
        for signature in [DISPLAY, SENDER, UNICAST, MODE_GETTER, OWN_PEER, PEER_LIST] {
            let parsed = parse(signature);
            assert_eq!(parsed.len(), signature.split_whitespace().count(), "{signature}");
            assert!(parsed.iter().zip(signature.split_whitespace()).all(|(b, s)| b.is_some() || s == "??"));
        }
        let sender = parse(SENDER);
        assert_eq!(sender.len(), SENDER_RELEASE, "the release's rel32 follows the signature");
        assert_eq!(&sender[SENDER_CHANNEL - 2..SENDER_CHANNEL], &[Some(0x8B), Some(0x15)]);
        assert_eq!(&sender[SENDER_CHANNEL_HIGH - 2..SENDER_CHANNEL_HIGH], &[Some(0x8B), Some(0x15)]);
        assert_eq!(&sender[SENDER_BROADCAST - 3..SENDER_BROADCAST], &[Some(0x56), Some(0x52), Some(0xE8)]);
        assert_eq!(&sender[8..13], &parse("3D 90 00 00 00")[..], "the game's own length check: 0x90");
        assert_eq!(parse(OWN_PEER)[2], Some(SESSION as u8));
    }

    #[test]
    fn a_line_goes_to_everyone_but_the_host_and_its_sender() {
        assert_eq!(relay_targets(1, 0, &[0, 1, 2]), Ok(vec![2]));
        assert_eq!(relay_targets(2, 0, &[0, 1, 2, 3]), Ok(vec![1, 3]));
        // Without the host in the list, and in any order.
        assert_eq!(relay_targets(3, 0, &[3, 1, 2]), Ok(vec![1, 2]));
    }

    #[test]
    fn gaps_in_the_peers_are_kept() {
        // Peer 2 left, 4 joined after.
        assert_eq!(relay_targets(1, 0, &[0, 1, 3, 4]), Ok(vec![3, 4]));
        assert_eq!(relay_targets(4, 0, &[0, 1, 3, 4]), Ok(vec![1, 3]));
    }

    #[test]
    fn a_single_other_player_needs_nothing_passed_on() {
        assert_eq!(relay_targets(1, 0, &[0, 1]), Ok(vec![]));
        assert_eq!(relay_targets(1, 0, &[1]), Ok(vec![]));
    }

    #[test]
    fn the_hosts_own_lines_and_unknown_senders_are_left_alone() {
        assert_eq!(relay_targets(0, 0, &[0, 1, 2]), Err(Skip::FromHost));
        // A host after a migration, peer 2.
        assert_eq!(relay_targets(2, 2, &[1, 2, 3]), Err(Skip::FromHost));
        assert_eq!(relay_targets(3, 2, &[1, 2, 3]), Ok(vec![1]));
        assert_eq!(relay_targets(5, 0, &[0, 1, 2]), Err(Skip::UnknownSender), "gone, or never a peer");
        assert_eq!(relay_targets(1, 0, &[]), Err(Skip::UnknownSender));
        assert_eq!(relay_targets(NO_PEER, 0, &[0, 1, NO_PEER]), Err(Skip::UnknownSender));
    }

    #[test]
    fn a_peer_listed_twice_or_no_peer_gets_nothing_extra() {
        assert_eq!(relay_targets(1, 0, &[0, 1, 2, 2, NO_PEER]), Ok(vec![2]));
    }

    #[test]
    fn only_lines_the_game_itself_would_send_go_on() {
        assert!(text_fits(1, 16));
        assert!(text_fits(MAX_TEXT, MAX_TEXT + 1));
        assert!(!text_fits(0, 16), "empty");
        assert!(!text_fits(MAX_TEXT + 1, 0x100), "longer than the game sends");
        assert!(!text_fits(20, 16), "past its buffer");
    }
}
