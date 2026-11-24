//! A player's address changing during one sign-in (0.5.6's IP). Shmoe played through Cloudflare
//! WARP on 18 Nov: twice the secure service refused their game's ticket from a new address, and
//! they were dropped from matches; only the journal said so. Now each change is a session event,
//! `address_change`:
//! - `where`: where it was seen (`secure`: the game's ticket at the secure service, used from
//!   another address than the one it was issued to; `api`: the launcher's or game's API sign-in,
//!   from another address than the one ten minutes or less before);
//! - `outcome`: `near` (the same /24 or /64: let in), `proven` (the player's API sign-in over TLS
//!   came from there: let in) or `refused`;
//! - `from` and `to`: the addresses masked as the client logs mask them (`104.28.x.x`), since
//!   session events keep no whole addresses;
//! - `net`: a known VPN's range when one of them is in it (`cloudflare_warp`), else whether the
//!   /16 changed (`same_16`, `other_16`).
//!
//! A repeat of the same change is one event with a count ([`crate::session_events`]).

use std::net::IpAddr;
use std::net::Ipv4Addr;

/// Known VPN and relay services' ranges (IPv4: network, prefix length), by the name `net` gives.
/// A short list of the ones seen; an address outside them is told by its /16.
const KNOWN: &[(&str, Ipv4Addr, u8)] = &[("cloudflare_warp", Ipv4Addr::new(104, 28, 0, 0), 16)];

/// quazal's ticket check found the game's ticket used from `used`, not `issued` (quazal's
/// `TICKET_ADDRESS_MOVED`).
pub fn ticket_moved(user: u32, issued: IpAddr, used: IpAddr, outcome: &'static str) {
    note(user, "secure", issued, used, outcome);
}

/// `user`'s API sign-in (over TLS, with their password) came from `to`, ten minutes or less after
/// one from `from`: their address moved (proven theirs both times).
pub fn api_moved(user: u32, from: IpAddr, to: IpAddr) {
    note(user, "api", from, to, "proven");
}

fn note(user: u32, place: &'static str, from: IpAddr, to: IpAddr, outcome: &'static str) {
    crate::session_events::note(
        crate::session_events::Who::Id(user),
        "address_change",
        serde_json::json!({ "where": place, "outcome": outcome, "from": masked(from), "to": masked(to), "net": net(from, to) }),
    );
}

/// An address with its host part hidden: `104.28.x.x`, or an IPv6 address's first two groups.
fn masked(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.x.x", o[0], o[1])
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}::x", s[0], s[1])
        }
    }
}

/// The known service `ip` is in, if any.
fn known(ip: IpAddr) -> Option<&'static str> {
    let IpAddr::V4(v4) = ip.to_canonical() else { return None };
    KNOWN
        .iter()
        .find(|(_, net, len)| {
            let mask = u32::MAX.checked_shl(32 - u32::from(*len)).unwrap_or(0);
            u32::from(v4) & mask == u32::from(*net) & mask
        })
        .map(|(name, ..)| *name)
}

/// What kind of change it was: a known service's, else the /16 kept or changed.
fn net(from: IpAddr, to: IpAddr) -> &'static str {
    if let Some(name) = known(to).or_else(|| known(from)) {
        return name;
    }
    match (from.to_canonical(), to.to_canonical()) {
        (IpAddr::V4(a), IpAddr::V4(b)) if a.octets()[..2] == b.octets()[..2] => "same_16",
        (IpAddr::V6(a), IpAddr::V6(b)) if a.segments()[..2] == b.segments()[..2] => "same_16",
        _ => "other_16",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_masked_and_told_by_its_network() {
        let warp_a: IpAddr = "104.28.198.246".parse().unwrap();
        let warp_b: IpAddr = "104.28.40.9".parse().unwrap();
        let home: IpAddr = "203.0.113.4".parse().unwrap();
        assert_eq!(masked(warp_a), "104.28.x.x");
        assert_eq!(masked("::ffff:203.0.113.4".parse().unwrap()), "203.0.x.x", "an IPv4-mapped address is IPv4");
        assert_eq!(masked("2001:db8:1:2::5".parse().unwrap()), "2001:db8::x");
        assert_eq!(net(warp_a, warp_b), "cloudflare_warp");
        assert_eq!(net(home, warp_a), "cloudflare_warp", "onto the VPN");
        assert_eq!(net(home, "203.0.200.1".parse().unwrap()), "same_16");
        assert_eq!(net(home, "198.51.100.1".parse().unwrap()), "other_16");
        assert_eq!(known("104.29.0.1".parse().unwrap()), None);
    }
}
