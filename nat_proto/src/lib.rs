//! The NAT helper protocol, spoken between the server's NAT helper (UDP,
//! 21128 by default) and the hook inside the game.
//!
//! Matches run peer to peer on the game's Storm socket (UDP 13000). The
//! game advertises one address to the other players, and by itself that's
//! its local one, which nobody outside its network can reach. The hook sends
//! these messages **from the Storm socket**, so the server sees that
//! socket's public mapping:
//!
//! - [`Message::Probe`] asks for the public address. The reply says what to
//!   advertise: the public address, a port mapping the router made, or a
//!   relay address on the server.
//! - [`Message::DataTo`] and [`Message::DataFrom`] carry game packets through
//!   the relay, for players whose NAT can't be punched through (symmetric
//!   NAT, carrier-grade NAT). The hook wraps and unwraps them, so the game
//!   sees each relayed player at its advertised relay address.
//!
//! Every message starts with [`MAGIC`] and a version byte; everything is
//! big-endian. IPv4 only, like the game.
//!
//! Nothing here trusts a UDP source address alone:
//! - a probe carries the player's [`Ticket`] (from signing in), so no one can
//!   register under another player's name;
//! - a registration only takes effect once a probe echoes the [`Cookie`] from
//!   the server's reply, so an address that can't receive can't be registered
//!   (no spoofed registrations);
//! - relayed data carries the registration's [`Tag`], checked both ways, so
//!   nobody can inject packets posing as the server or as a player.

use std::net::Ipv4Addr;
use std::net::SocketAddrV4;

/// Starts every message. Never the start of a Storm or PRUDP packet.
pub const MAGIC: [u8; 4] = [0x5e, 0xc4, b'N', b'T'];
/// The protocol version.
pub const VERSION: u8 = 2;
/// The server's default port. The next port (21129) answers probes too, to
/// tell a NAT that changes ports per destination (symmetric) from one that
/// doesn't.
pub const DEFAULT_PORT: u16 = 21128;
/// Probes are padded to this size, so a reply is never bigger than the
/// request that caused it (the helper can't be used to amplify traffic).
pub const PROBE_SIZE: usize = 96;
/// The longest account name a probe carries.
pub const MAX_NAME: usize = 32;
/// The largest game packet the relay carries: the largest UDP payload on a
/// 1500-byte link, so every packet the game sends. Storm fills its packets up
/// to about 1460 bytes while a co-op mission loads; at 1400 the relay dropped
/// those, and the guest's game gave up waiting for them. Wrapped, the biggest
/// ones exceed the link and travel as two IP fragments.
pub const MAX_PAYLOAD: usize = 1472;
/// The port Storm, the game's peer-to-peer layer, binds.
pub const STORM_PORT: u16 = 13000;

const HEADER: usize = MAGIC.len() + 2;
const ADDR: usize = 6;

/// Proves a probe's name: the server's MAC of it, handed to the player when
/// they sign in.
pub type Ticket = [u8; 16];
/// Proves a probe's source can receive: the server's reply carries it, the
/// next probe echoes it.
pub type Cookie = [u8; 16];
/// A registration's secret, carried by relayed data both ways.
pub type Tag = [u8; 8];

/// [`Message::Probe`] flags.
pub mod probe_flags {
    /// The player asked to always go through the relay.
    pub const WANT_RELAY: u8 = 1;
    /// `mapping` holds a port mapping the router made (UPnP or NAT-PMP).
    pub const HAS_MAPPING: u8 = 2;
    /// Sent to the second port, only to compare mappings; changes nothing.
    pub const SECOND_PORT: u8 = 4;
    /// The hook found its NAT gives a new port per destination.
    pub const SYMMETRIC: u8 = 8;
}

/// [`Message::ProbeReply`] flags.
pub mod reply_flags {
    /// The player is relayed: `advertise` is a relay address.
    pub const RELAYED: u8 = 1;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Hook → server: "what address should I advertise?" Also the keepalive
    /// that keeps the NAT mapping (and the relay registration) alive.
    Probe {
        flags: u8,
        nonce: u32,
        /// The router's port mapping for the Storm port, if any.
        mapping: Option<SocketAddrV4>,
        /// The account name, to tie this Storm socket to the player's
        /// matchmaking connection.
        name: String,
        /// The player's ticket for `name` (zeros: none, only asks for the address).
        ticket: Ticket,
        /// The cookie from the last reply (zeros: none yet).
        cookie: Cookie,
    },
    /// Server → hook.
    ProbeReply {
        nonce: u32,
        /// The Storm socket's public address, as the server saw it.
        observed: SocketAddrV4,
        /// What the game should advertise to other players.
        advertise: SocketAddrV4,
        flags: u8,
        /// Relay addresses are `relay_ip` with a port in `relay_ports`. Game
        /// packets to them must be wrapped in [`Message::DataTo`].
        relay_ip: Ipv4Addr,
        relay_ports: (u16, u16),
        /// To echo in the next probe.
        cookie: Cookie,
        /// The registration's tag; zeros until it's registered (the next
        /// probe, with the cookie, registers it).
        tag: Tag,
    },
    /// Hook → server: a game packet for `to`, through the relay.
    DataTo { tag: Tag, to: SocketAddrV4, payload: Vec<u8> },
    /// Server → hook: a game packet relayed from `from`, the sender's
    /// advertised address.
    DataFrom { tag: Tag, from: SocketAddrV4, payload: Vec<u8> },
}

const OP_PROBE: u8 = 1;
const OP_PROBE_REPLY: u8 = 2;
const OP_DATA_TO: u8 = 3;
const OP_DATA_FROM: u8 = 4;

/// Whether `data` looks like one of these messages (and not a game packet).
pub fn is_nat_message(data: &[u8]) -> bool {
    data.len() >= HEADER && data[..4] == MAGIC && data[4] == VERSION
}

fn put_addr(out: &mut Vec<u8>, addr: SocketAddrV4) {
    out.extend_from_slice(&addr.ip().octets());
    out.extend_from_slice(&addr.port().to_be_bytes());
}

fn addr(data: &[u8]) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::new(data[0], data[1], data[2], data[3]), u16::from_be_bytes([data[4], data[5]]))
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(PROBE_SIZE);
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        match self {
            Message::Probe {
                flags,
                nonce,
                mapping,
                name,
                ticket,
                cookie,
            } => {
                out.push(OP_PROBE);
                out.push(*flags);
                out.extend_from_slice(&nonce.to_be_bytes());
                put_addr(&mut out, mapping.unwrap_or(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)));
                out.extend_from_slice(ticket);
                out.extend_from_slice(cookie);
                let name = truncate(name, MAX_NAME);
                out.push(name.len() as u8);
                out.extend_from_slice(name.as_bytes());
                out.resize(PROBE_SIZE, 0);
            }
            Message::ProbeReply {
                nonce,
                observed,
                advertise,
                flags,
                relay_ip,
                relay_ports,
                cookie,
                tag,
            } => {
                out.push(OP_PROBE_REPLY);
                out.extend_from_slice(&nonce.to_be_bytes());
                put_addr(&mut out, *observed);
                put_addr(&mut out, *advertise);
                out.push(*flags);
                out.extend_from_slice(&relay_ip.octets());
                out.extend_from_slice(&relay_ports.0.to_be_bytes());
                out.extend_from_slice(&relay_ports.1.to_be_bytes());
                out.extend_from_slice(cookie);
                out.extend_from_slice(tag);
            }
            Message::DataTo { tag, to, payload } => encode_data_to(&mut out, *tag, *to, payload),
            Message::DataFrom { tag, from, payload } => encode_data_from(&mut out, *tag, *from, payload),
        }
        out
    }

    pub fn decode(data: &[u8]) -> Option<Message> {
        if !is_nat_message(data) {
            return None;
        }
        let body = &data[HEADER..];
        match data[5] {
            OP_PROBE => {
                let fixed = 1 + 4 + ADDR + 16 + 16 + 1;
                if data.len() < PROBE_SIZE || body.len() < fixed {
                    return None;
                }
                let flags = body[0];
                let nonce = u32::from_be_bytes(body[1..5].try_into().ok()?);
                let mapping = Some(addr(&body[5..11])).filter(|a| !a.ip().is_unspecified() && a.port() != 0);
                let ticket: Ticket = body[11..27].try_into().ok()?;
                let cookie: Cookie = body[27..43].try_into().ok()?;
                let len = usize::from(body[43]).min(MAX_NAME);
                let name = body.get(fixed..fixed + len)?;
                Some(Message::Probe {
                    flags,
                    nonce,
                    mapping,
                    name: String::from_utf8_lossy(name).into_owned(),
                    ticket,
                    cookie,
                })
            }
            OP_PROBE_REPLY => {
                if body.len() < 4 + ADDR * 2 + 1 + 4 + 4 + 16 + 8 {
                    return None;
                }
                Some(Message::ProbeReply {
                    nonce: u32::from_be_bytes(body[0..4].try_into().ok()?),
                    observed: addr(&body[4..10]),
                    advertise: addr(&body[10..16]),
                    flags: body[16],
                    relay_ip: Ipv4Addr::new(body[17], body[18], body[19], body[20]),
                    relay_ports: (u16::from_be_bytes([body[21], body[22]]), u16::from_be_bytes([body[23], body[24]])),
                    cookie: body[25..41].try_into().ok()?,
                    tag: body[41..49].try_into().ok()?,
                })
            }
            OP_DATA_TO | OP_DATA_FROM => {
                let (tag, a, offset) = data_header(data)?;
                let payload = data[offset..].to_vec();
                Some(if data[5] == OP_DATA_TO { Message::DataTo { tag, to: a, payload } } else { Message::DataFrom { tag, from: a, payload } })
            }
            _ => None,
        }
    }
}

/// A data message's tag, address and payload offset, if it is one.
fn data_header(data: &[u8]) -> Option<(Tag, SocketAddrV4, usize)> {
    if !is_nat_message(data) || data.len() < DATA_OVERHEAD || data.len() - DATA_OVERHEAD > MAX_PAYLOAD {
        return None;
    }
    let tag: Tag = data[HEADER..HEADER + 8].try_into().ok()?;
    Some((tag, addr(&data[HEADER + 8..DATA_OVERHEAD]), DATA_OVERHEAD))
}

fn encode_data(out: &mut Vec<u8>, op: u8, tag: Tag, addr: SocketAddrV4, payload: &[u8]) {
    out.clear();
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(op);
    out.extend_from_slice(&tag);
    put_addr(out, addr);
    out.extend_from_slice(payload);
}

/// Writes a [`Message::DataTo`] into `out` without allocating per packet.
pub fn encode_data_to(out: &mut Vec<u8>, tag: Tag, to: SocketAddrV4, payload: &[u8]) {
    encode_data(out, OP_DATA_TO, tag, to, payload);
}

/// Writes a [`Message::DataFrom`] into `out`.
pub fn encode_data_from(out: &mut Vec<u8>, tag: Tag, from: SocketAddrV4, payload: &[u8]) {
    encode_data(out, OP_DATA_FROM, tag, from, payload);
}

/// If `data` is a [`Message::DataFrom`]: its tag, sender and the offset of
/// the game packet in `data`.
pub fn data_from(data: &[u8]) -> Option<(Tag, SocketAddrV4, usize)> {
    data_header(data).filter(|_| data[5] == OP_DATA_FROM)
}

/// If `data` is a [`Message::DataTo`]: its tag, destination and the offset
/// of the game packet in `data`.
pub fn data_to(data: &[u8]) -> Option<(Tag, SocketAddrV4, usize)> {
    data_header(data).filter(|_| data[5] == OP_DATA_TO)
}

/// The overhead [`Message::DataTo`] and [`Message::DataFrom`] add to a game
/// packet.
pub const DATA_OVERHEAD: usize = HEADER + 8 + ADDR;

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Addresses nobody outside their own network can reach: private ranges,
/// carrier-grade NAT (100.64/10), loopback and link-local.
pub fn is_private(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || (a == 100 && (64..128).contains(&b))
}

/// The station URL the game advertises for `addr` (the reply the game's own
/// NAT echo would have carried).
pub fn station_url(addr: SocketAddrV4) -> String {
    format!("prudp:/address={};port={}", addr.ip(), addr.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddrV4 {
        s.parse().unwrap()
    }

    fn reply(nonce: u32) -> Message {
        Message::ProbeReply {
            nonce,
            observed: a("198.51.100.2:61001"),
            advertise: a("192.0.2.1:40001"),
            flags: reply_flags::RELAYED,
            relay_ip: Ipv4Addr::new(192, 0, 2, 1),
            relay_ports: (40000, 40999),
            cookie: [7; 16],
            tag: [9; 8],
        }
    }

    #[test]
    fn messages_round_trip() {
        let messages = [
            Message::Probe {
                flags: probe_flags::HAS_MAPPING | probe_flags::WANT_RELAY,
                nonce: 0xdead_beef,
                mapping: Some(a("203.0.113.7:13000")),
                name: "sam".into(),
                ticket: [1; 16],
                cookie: [2; 16],
            },
            Message::Probe {
                flags: 0,
                nonce: 1,
                mapping: None,
                name: String::new(),
                ticket: [0; 16],
                cookie: [0; 16],
            },
            reply(7),
            Message::DataTo {
                tag: [3; 8],
                to: a("192.0.2.1:40002"),
                payload: vec![1, 5, 0x33, 0],
            },
            Message::DataFrom {
                tag: [4; 8],
                from: a("198.51.100.9:13000"),
                payload: vec![],
            },
        ];
        for m in messages {
            assert_eq!(Message::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn probes_are_never_smaller_than_replies() {
        let probe = Message::Probe {
            flags: 0,
            nonce: 0,
            mapping: None,
            name: "x".repeat(200),
            ticket: [0; 16],
            cookie: [0; 16],
        }
        .encode();
        assert_eq!(probe.len(), PROBE_SIZE);
        assert!(reply(0).encode().len() <= probe.len());
        // A short probe (someone trying to get a bigger reply) is refused.
        assert_eq!(Message::decode(&probe[..40]), None);
    }

    #[test]
    fn game_packets_are_not_messages() {
        // A Storm packet, a PRUDP packet, and garbage.
        for data in [&b"\x01\x05\x33\x00\x00\x00\x00\xdc"[..], b"\x3f\x31\x00\x00", b"", b"\x5e\xc4NT\x02\x01"] {
            assert!(Message::decode(data).is_none(), "{data:x?}");
        }
    }

    #[test]
    fn data_is_wrapped_and_unwrapped_in_place() {
        let mut buf = Vec::new();
        encode_data_to(&mut buf, [5; 8], a("192.0.2.1:40003"), b"game");
        assert_eq!(data_to(&buf), Some(([5; 8], a("192.0.2.1:40003"), DATA_OVERHEAD)));
        assert!(data_from(&buf).is_none());
        let from = Message::DataFrom {
            tag: [6; 8],
            from: a("198.51.100.9:13000"),
            payload: b"game".to_vec(),
        }
        .encode();
        let (tag, sender, offset) = data_from(&from).unwrap();
        assert_eq!((tag, sender, &from[offset..]), ([6; 8], a("198.51.100.9:13000"), &b"game"[..]));
        assert_eq!(from.len() - b"game".len(), DATA_OVERHEAD);
    }

    #[test]
    fn the_largest_game_packets_are_relayed() {
        // Storm's packets while a co-op mission loads, and the largest a 1500-byte link carries.
        let mut buf = Vec::new();
        for size in [1460, 1472] {
            let packet = vec![0x33; size];
            encode_data_to(&mut buf, [5; 8], a("192.0.2.1:40003"), &packet);
            let (_, _, offset) = data_to(&buf).unwrap_or_else(|| panic!("a {size}-byte packet isn't relayed"));
            assert_eq!(&buf[offset..], &packet[..]);
        }
        encode_data_to(&mut buf, [5; 8], a("192.0.2.1:40003"), &[0; MAX_PAYLOAD + 1]);
        assert_eq!(data_to(&buf), None);
    }

    #[test]
    fn private_addresses() {
        for ip in ["10.1.2.3", "172.16.0.1", "192.168.1.20", "100.64.0.1", "100.127.255.254", "127.0.0.1", "169.254.1.1", "0.0.0.0"] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["203.0.113.7", "100.128.0.1", "8.8.8.8", "26.1.2.3"] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }
}
