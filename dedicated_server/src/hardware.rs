//! Players' hardware ids, hashed, as their game or launcher sent them at sign-in (the 0.4.8
//! plan's F10): kept in memory only, never in the database, and sent with the player's roster
//! entry to the coordinator, which keys them again with a secret of its own and refuses a PC
//! that two of them tie to a banned player's. With them goes a hash of the network the player
//! signed in from (an IPv4 /24 or IPv6 /48, never the address): a sign of a banned player back
//! under a new name, only ever flagged for the admins.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;

/// One id as sent on: its kind and hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Id {
    pub kind: String,
    pub hash: String,
}

/// The kinds taken (the hwid crate's), and the most ids per sign-in, the network's included.
const KINDS: [&str; 5] = ["machine-guid", "smbios-uuid", "board-serial", "disk-serial", "linux-machine-id"];
const MOST_IDS: usize = 12;
/// How long a player's ids are kept after they sent them: long enough for the roster to take
/// them (it goes within seconds of a sign-in, and every few minutes), never for good.
const KEPT_FOR: Duration = Duration::from_secs(24 * 3600);
/// The most players kept.
const MOST_PLAYERS: usize = 20_000;

struct Seen {
    ids: Vec<Id>,
    vm: Option<String>,
    at: Instant,
}

fn seen() -> std::sync::MutexGuard<'static, HashMap<u32, Seen>> {
    static SEEN: std::sync::LazyLock<Mutex<HashMap<u32, Seen>>> = std::sync::LazyLock::new(Mutex::default);
    SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The ids in a sign-in that are well formed: known kinds, 64 hex characters, no repeats.
pub fn valid(ids: &[server_api::users::HardwareId]) -> Vec<Id> {
    let mut out: Vec<Id> = ids
        .iter()
        .filter(|i| KINDS.contains(&i.kind.as_str()) && i.hash.len() == 64 && i.hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|i| Id {
            kind: i.kind.clone(),
            hash: i.hash.to_ascii_lowercase(),
        })
        .collect();
    out.sort_by(|a, b| (&a.kind, &a.hash).cmp(&(&b.kind, &b.hash)));
    out.dedup();
    // Room for the network's, always added after.
    out.truncate(MOST_IDS - 1);
    out
}

/// The hash of the network `ip` is in: its /24 (IPv4) or /48 (IPv6), never the address.
pub fn network(ip: std::net::IpAddr) -> Id {
    use sha2::Digest as _;
    let prefix = match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.0/24", o[0], o[1], o[2])
        }
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => return network(std::net::IpAddr::V4(v4)),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}::/48", s[0], s[1], s[2])
            }
        },
    };
    let digest = sha2::Sha256::digest(format!("fes-net-v1\0{prefix}").as_bytes());
    Id {
        kind: String::from("network"),
        hash: digest.iter().map(|b| format!("{b:02x}")).collect(),
    }
}

/// `user` signed in from `ip` with these ids (and on a virtual machine, `vm`): kept for the
/// roster, which goes soon after.
pub fn note(user: u32, ids: &[server_api::users::HardwareId], vm: &str, ip: Option<std::net::IpAddr>) {
    let mut ids = valid(ids);
    // The network, whatever the client sent of its PC (an older client, or one sending none).
    ids.extend(ip.map(network));
    if ids.is_empty() && vm.is_empty() {
        return;
    }
    let vm: String = vm.chars().filter(|c| !c.is_control()).take(32).collect();
    let mut all = seen();
    if all.len() >= MOST_PLAYERS && !all.contains_key(&user) {
        all.retain(|_, s| s.at.elapsed() < KEPT_FOR);
        if all.len() >= MOST_PLAYERS {
            return;
        }
    }
    all.insert(
        user,
        Seen {
            ids,
            vm: Some(vm).filter(|v| !v.is_empty()),
            at: Instant::now(),
        },
    );
    drop(all);
    crate::players::changed(user);
}

/// `user`'s ids and virtual machine, if they sent them lately.
pub fn of(user: u32) -> (Vec<Id>, Option<String>) {
    let all = seen();
    match all.get(&user).filter(|s| s.at.elapsed() < KEPT_FOR) {
        Some(s) => (s.ids.clone(), s.vm.clone()),
        None => (Vec::new(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(kind: &str, hash: &str) -> server_api::users::HardwareId {
        server_api::users::HardwareId {
            kind: kind.into(),
            hash: hash.into(),
        }
    }

    #[test]
    fn only_well_formed_ids_are_kept() {
        let good = "a".repeat(64);
        let ids = [
            id("machine-guid", &good),
            id("machine-guid", &good),
            id("disk-serial", &"F".repeat(64)),
            id("mac-address", &good),
            id("smbios-uuid", "abc"),
            id("board-serial", &"z".repeat(64)),
        ];
        let kept = valid(&ids);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].kind, "disk-serial");
        assert_eq!(kept[0].hash, "f".repeat(64));
    }

    #[test]
    fn noted_for_the_roster() {
        note(900_001, &[id("linux-machine-id", &"b".repeat(64))], "", Some("203.0.113.77".parse().unwrap()));
        let (ids, vm) = of(900_001);
        assert_eq!((ids.len(), vm), (2, None));
        assert_eq!(ids[1], network("203.0.113.9".parse().unwrap()), "the same /24");
        assert_ne!(ids[1], network("203.0.114.9".parse().unwrap()));
        assert_eq!(network("::ffff:203.0.113.1".parse().unwrap()), ids[1]);
        // An older client, or one sending nothing of its PC: its network all the same.
        note(900_003, &[], "", Some("198.51.100.4".parse().unwrap()));
        assert_eq!(of(900_003).0, [network("198.51.100.9".parse().unwrap())]);
        // A full list of ids still leaves room for it.
        let many: Vec<_> = (0..20).map(|i| id("disk-serial", &format!("{i:064x}"))).collect();
        note(900_004, &many, "", Some("198.51.100.4".parse().unwrap()));
        let (ids, _) = of(900_004);
        assert_eq!((ids.len(), ids.last().map(|i| i.kind.as_str())), (MOST_IDS, Some("network")));
        assert_eq!(of(900_002), (Vec::new(), None));
    }
}
