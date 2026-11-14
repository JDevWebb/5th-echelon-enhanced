//! Weapon archetypes compared between the games of a match (0.5.2's W1, report only): each game
//! sends its loaded archetypes' stats hashed, by name (`Misc.MatchState`'s `weapons`). Every game
//! has the same stock stats, so a guest whose archetype differs from its host's has an edited
//! game (or the host has: both are named, the guest's report says whose). Noted once per guest
//! and host until the guest's hashes agree again.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Mutex;

/// The most archetypes kept from one game, and the longest name.
const MOST: usize = 400;
const LONGEST: usize = 64;
const MOST_HOSTS: usize = 20_000;
/// Differing archetypes named in one report.
const NAMED: usize = 8;

type Hashes = BTreeMap<String, u32>;

#[derive(Default)]
pub struct Weapons {
    hosts: HashMap<u32, Hashes>,
    /// Guests told about, with their host.
    told: HashMap<u32, u32>,
}

fn clean(sent: &HashMap<String, u32>) -> Hashes {
    sent.iter()
        .filter(|(k, _)| !k.is_empty() && k.len() <= LONGEST && k.bytes().all(|b| b.is_ascii_graphic()))
        .take(MOST)
        .map(|(k, v)| (k.clone(), *v))
        .collect()
}

impl Weapons {
    /// A host's game sent its archetypes.
    pub fn host_said(&mut self, host: u32, sent: &HashMap<String, u32>) {
        let hashes = clean(sent);
        if hashes.is_empty() {
            return;
        }
        if self.hosts.len() >= MOST_HOSTS {
            self.hosts.clear();
        }
        self.hosts.insert(host, hashes);
    }

    /// A guest's game in `host`'s match sent its archetypes: what to note, if any differ from
    /// the host's (the archetypes both have).
    pub fn guest_said(&mut self, guest: u32, host: u32, sent: &HashMap<String, u32>) -> Option<serde_json::Value> {
        let mine = clean(sent);
        let theirs = self.hosts.get(&host)?;
        let differ: Vec<&String> = mine.iter().filter(|(k, v)| theirs.get(*k).is_some_and(|h| h != *v)).map(|(k, _)| k).collect();
        if differ.is_empty() {
            self.told.remove(&guest);
            return None;
        }
        if self.told.get(&guest) == Some(&host) {
            return None;
        }
        if self.told.len() >= MOST_HOSTS {
            self.told.clear();
        }
        self.told.insert(guest, host);
        let named: Vec<&str> = differ.iter().take(NAMED).map(|s| s.as_str()).collect();
        Some(serde_json::json!({
            "check": "weapon",
            "detail": format!(
                "{} weapon{} with other stats than their host's game: {}{}",
                differ.len(),
                if differ.len() == 1 { "" } else { "s" },
                named.join(", "),
                if differ.len() > NAMED { ", ..." } else { "" }
            ),
            "value": differ.len(),
            "host": host,
            "wine": false,
        }))
    }
}

static WEAPONS: std::sync::LazyLock<Mutex<Weapons>> = std::sync::LazyLock::new(Mutex::default);

pub fn weapons() -> std::sync::MutexGuard<'static, Weapons> {
    WEAPONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guests_edited_weapon_is_noted_once() {
        let mut w = Weapons::default();
        let set = |pairs: &[(&str, u32)]| pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect::<HashMap<_, _>>();
        assert!(w.guest_said(2, 1, &set(&[("AR_SC3000", 7)])).is_none(), "the host said nothing yet");
        w.host_said(1, &set(&[("AR_SC3000", 7), ("SMG_MP5", 9)]));
        assert!(
            w.guest_said(2, 1, &set(&[("AR_SC3000", 7), ("SMG_MP5", 9), ("Only_Mine", 3)])).is_none(),
            "the same, and one the host doesn't load"
        );
        let d = w.guest_said(3, 1, &set(&[("AR_SC3000", 8), ("SMG_MP5", 9)])).unwrap();
        assert_eq!((d["check"].as_str(), d["value"].as_u64()), (Some("weapon"), Some(1)));
        assert!(d["detail"].as_str().unwrap().contains("AR_SC3000"));
        assert!(w.guest_said(3, 1, &set(&[("AR_SC3000", 8)])).is_none(), "once");
        assert!(w.guest_said(3, 1, &set(&[("AR_SC3000", 7)])).is_none(), "agrees again");
        assert!(w.guest_said(3, 1, &set(&[("AR_SC3000", 8)])).is_some(), "and differs again");
    }
}
