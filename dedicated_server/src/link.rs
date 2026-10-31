//! The link between members' NAT helpers (global matchmaking, G1 items 7 and 8): UDP between
//! servers, never seen by a game. Every message names its sender (its server id), carries the
//! time it was sent, and ends with an HMAC under the key the coordinator gave the pair (its
//! heartbeat's `links`); a message is taken only from the address the coordinator lists for
//! that sender, only fresh ([`FRESH`]), and only with the right MAC.
//!
//! - [`Body::Report`]: a relay's registrations, for the matchmaking server (its whole table
//!   every 2 s, in as many messages as it takes; an entry not heard of again lapses).
//! - [`Body::Ask`] and [`Body::Answer`]: whether the matchmaking server put two players
//!   together, so the relay may carry their traffic.
//!
//! `MAGIC` and the version differ from the games' protocol (nat_proto), so neither side takes
//! the other's messages.

use std::net::Ipv4Addr;
use std::net::SocketAddrV4;

pub const MAGIC: [u8; 4] = [0x5e, 0xc4, b'L', b'K'];
pub const VERSION: u8 = 1;
/// How far a message's time may be from ours, in seconds.
pub const FRESH: i64 = 30;
/// The longest server id or player name carried.
pub const MAX_TEXT: usize = 32;
/// The most registrations in one report (each at most 48 bytes: under 1300 in all).
pub const MAX_REGS: usize = 25;
const MAC: usize = 16;

const OP_REPORT: u8 = 1;
const OP_ASK: u8 = 2;
const OP_ANSWER: u8 = 3;
const OP_EVENT: u8 = 4;

/// A player registered with a relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reg {
    pub name: String,
    /// Their Storm socket's public address, as the relay sees it.
    pub real: SocketAddrV4,
    /// What their game advertises (a relay address when relayed).
    pub advertise: SocketAddrV4,
    pub relayed: bool,
    /// Their game's round trip to the relay, if it said.
    pub rtt_ms: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Report(Vec<Reg>),
    Ask {
        nonce: u32,
        a: String,
        b: String,
    },
    Answer {
        nonce: u32,
        yes: bool,
    },
    /// A relay's session event about one of its players (its `nat`, `relay_drop`...), for the
    /// matchmaking server to keep with the player's sessions: a relay alone has no database.
    Event {
        name: String,
        kind: String,
        detail: String,
    },
}

/// The longest event detail carried (JSON).
pub const MAX_DETAIL: usize = 600;

/// Whether `data` is a link message (and not a game's).
pub fn is_link(data: &[u8]) -> bool {
    data.len() > 6 && data[..4] == MAGIC && data[4] == VERSION
}

fn mac(key: &[u8; 32], data: &[u8]) -> [u8; MAC] {
    use hmac::Mac as _;
    let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("any key length");
    m.update(data);
    m.finalize().into_bytes()[..MAC].try_into().expect("16 of 32 bytes")
}

fn put_text(out: &mut Vec<u8>, text: &str) {
    let mut end = text.len().min(MAX_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    out.push(end as u8);
    out.extend_from_slice(&text.as_bytes()[..end]);
}

fn put_addr(out: &mut Vec<u8>, a: SocketAddrV4) {
    out.extend_from_slice(&a.ip().octets());
    out.extend_from_slice(&a.port().to_be_bytes());
}

/// `body` from `from` at `time`, signed with `key`.
pub fn seal(key: &[u8; 32], from: &str, time: i64, body: &Body) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.push(VERSION);
    out.push(match body {
        Body::Report(_) => OP_REPORT,
        Body::Ask { .. } => OP_ASK,
        Body::Answer { .. } => OP_ANSWER,
        Body::Event { .. } => OP_EVENT,
    });
    out.extend_from_slice(&time.to_be_bytes());
    put_text(&mut out, from);
    match body {
        Body::Report(regs) => {
            let regs = &regs[..regs.len().min(MAX_REGS)];
            out.push(regs.len() as u8);
            for r in regs {
                put_text(&mut out, &r.name);
                put_addr(&mut out, r.real);
                put_addr(&mut out, r.advertise);
                out.push(u8::from(r.relayed));
                out.extend_from_slice(&r.rtt_ms.unwrap_or(0).to_be_bytes());
            }
        }
        Body::Ask { nonce, a, b } => {
            out.extend_from_slice(&nonce.to_be_bytes());
            put_text(&mut out, a);
            put_text(&mut out, b);
        }
        Body::Answer { nonce, yes } => {
            out.extend_from_slice(&nonce.to_be_bytes());
            out.push(u8::from(*yes));
        }
        Body::Event { name, kind, detail } => {
            put_text(&mut out, name);
            put_text(&mut out, kind);
            let mut end = detail.len().min(MAX_DETAIL);
            while !detail.is_char_boundary(end) {
                end -= 1;
            }
            out.extend_from_slice(&(end as u16).to_be_bytes());
            out.extend_from_slice(&detail.as_bytes()[..end]);
        }
    }
    let m = mac(key, &out);
    out.extend_from_slice(&m);
    out
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = rest;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn text(&mut self) -> Option<String> {
        let len = usize::from(self.u8()?);
        if len > MAX_TEXT {
            return None;
        }
        Some(String::from_utf8_lossy(self.take(len)?).into_owned())
    }
    fn addr(&mut self) -> Option<SocketAddrV4> {
        let b = self.take(6)?;
        Some(SocketAddrV4::new(Ipv4Addr::new(b[0], b[1], b[2], b[3]), u16::from_be_bytes([b[4], b[5]])))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
}

/// Who sent `data` (its server id): read without checking anything, to find the key.
pub fn sender(data: &[u8]) -> Option<String> {
    if !is_link(data) {
        return None;
    }
    Reader(data.get(14..)?).text()
}

/// The message in `data` from `from`, when its MAC holds under `key` and it's fresh at `now`.
pub fn open(data: &[u8], key: &[u8; 32], now: i64) -> Option<(String, Body)> {
    if !is_link(data) || data.len() < 15 + MAC {
        return None;
    }
    let (signed, tag) = data.split_at(data.len() - MAC);
    // In constant time: a MAC compared byte by byte tells an attacker how far it got.
    let expected = mac(key, signed);
    if expected.iter().zip(tag).fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0 {
        return None;
    }
    let mut r = Reader(&signed[5..]);
    let op = r.u8()?;
    let time = i64::from_be_bytes(r.take(8)?.try_into().ok()?);
    if now.abs_diff(time) > FRESH.unsigned_abs() {
        return None;
    }
    let from = r.text()?;
    let body = match op {
        OP_REPORT => {
            let n = usize::from(r.u8()?).min(MAX_REGS);
            let mut regs = Vec::with_capacity(n);
            for _ in 0..n {
                let name = r.text()?;
                let real = r.addr()?;
                let advertise = r.addr()?;
                let relayed = r.u8()? != 0;
                let rtt = u16::from_be_bytes(r.take(2)?.try_into().ok()?);
                regs.push(Reg {
                    name,
                    real,
                    advertise,
                    relayed,
                    rtt_ms: (rtt > 0).then_some(rtt),
                });
            }
            Body::Report(regs)
        }
        OP_ASK => Body::Ask {
            nonce: r.u32()?,
            a: r.text()?,
            b: r.text()?,
        },
        OP_ANSWER => Body::Answer {
            nonce: r.u32()?,
            yes: r.u8()? != 0,
        },
        OP_EVENT => {
            let name = r.text()?;
            let kind = r.text()?;
            let len = usize::from(u16::from_be_bytes(r.take(2)?.try_into().ok()?)).min(MAX_DETAIL);
            Body::Event {
                name,
                kind,
                detail: String::from_utf8_lossy(r.take(len)?).into_owned(),
            }
        }
        _ => return None,
    };
    Some((from, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_opens_only_with_its_key_while_fresh_and_whole() {
        let key = [7; 32];
        let report = Body::Report(vec![Reg {
            name: "Kiwi".into(),
            real: "203.0.113.9:13000".parse().unwrap(),
            advertise: "5.6.7.8:40012".parse().unwrap(),
            relayed: true,
            rtt_ms: Some(31),
        }]);
        let sealed = seal(&key, "syd", 1_000, &report);
        assert_eq!(sender(&sealed).as_deref(), Some("syd"));
        assert_eq!(open(&sealed, &key, 1_010), Some(("syd".to_string(), report)));
        assert_eq!(open(&sealed, &[8; 32], 1_010), None, "another key");
        assert_eq!(open(&sealed, &key, 1_000 + FRESH + 1), None, "stale");
        let mut changed = sealed.clone();
        changed[20] ^= 1;
        assert_eq!(open(&changed, &key, 1_000), None, "changed on the way");
        assert_eq!(open(&sealed[..sealed.len() - 1], &key, 1_000), None, "cut short");
        for body in [
            Body::Ask {
                nonce: 9,
                a: "Kiwi".into(),
                b: "Tank".into(),
            },
            Body::Answer { nonce: 9, yes: true },
            Body::Event {
                name: "Kiwi".into(),
                kind: "relay_drop".into(),
                detail: r#"{"direction":"to"}"#.into(),
            },
        ] {
            assert_eq!(open(&seal(&key, "mm", 5, &body), &key, 5), Some(("mm".to_string(), body)));
        }
        // Not a game's message, and a game's isn't one.
        assert!(!nat_proto::is_nat_message(&sealed));
        assert!(!is_link(&nat_proto::Message::Bye { tag: [0; 8] }.encode()));
    }

    #[test]
    fn a_full_report_fits_a_datagram() {
        let reg = Reg {
            name: "x".repeat(MAX_TEXT),
            real: "1.2.3.4:1".parse().unwrap(),
            advertise: "1.2.3.4:1".parse().unwrap(),
            relayed: false,
            rtt_ms: None,
        };
        assert!(seal(&[0; 32], &"y".repeat(MAX_TEXT), 0, &Body::Report(vec![reg; MAX_REGS])).len() <= 1300);
    }
}
