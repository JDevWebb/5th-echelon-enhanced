//! The round's settings as the host's game has them (the 0.5.0 plan's F4): respawn times, the
//! time limit, score and respawn limits, sent with its match state (`Misc.MatchState`). Each
//! round's are noted as it starts; a change while it runs is noted with what changed, for the
//! admins: players reported the 2013 trick of a host pushing the other team's respawn out to
//! about 500 s mid-match.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Mutex;

/// The most settings taken from one game, and the longest name.
const MOST: usize = 32;
const LONGEST: usize = 48;
const MOST_HOSTS: usize = 20_000;

/// Each host's round under way: its settings as it started (or as last changed).
fn rounds() -> std::sync::MutexGuard<'static, HashMap<u32, BTreeMap<String, f32>>> {
    static ROUNDS: std::sync::LazyLock<Mutex<HashMap<u32, BTreeMap<String, f32>>>> = std::sync::LazyLock::new(Mutex::default);
    ROUNDS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The settings sent that look like the game's own: script property names, finite values.
fn clean(settings: &HashMap<String, f32>) -> BTreeMap<String, f32> {
    settings
        .iter()
        .filter(|(k, v)| k.len() <= LONGEST && k.starts_with("m_") && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && v.is_finite())
        .take(MOST)
        .map(|(k, v)| (k.clone(), (*v * 100.0).round() / 100.0))
        .collect()
}

/// `host` said its match is (or isn't) `under_way`, with these settings: what to note, if
/// anything (a round starting, with its settings; or a change while it runs, with each
/// setting's old and new value).
pub fn said(host: u32, under_way: bool, settings: &HashMap<String, f32>) -> Option<serde_json::Value> {
    let settings = clean(settings);
    let mut all = rounds();
    if !under_way {
        all.remove(&host);
        return None;
    }
    if settings.is_empty() {
        return None;
    }
    match all.get(&host) {
        None => {
            if all.len() >= MOST_HOSTS {
                all.clear();
            }
            all.insert(host, settings.clone());
            Some(serde_json::json!({ "start": true, "settings": settings }))
        }
        Some(before) if *before != settings => {
            let changes: BTreeMap<&String, (Option<f32>, Option<f32>)> = before
                .keys()
                .chain(settings.keys())
                .filter(|k| before.get(*k) != settings.get(*k))
                .map(|k| (k, (before.get(k).copied(), settings.get(k).copied())))
                .collect();
            let detail = serde_json::json!({ "during": true, "changes": changes });
            all.insert(host, settings);
            Some(detail)
        }
        Some(_) => None,
    }
}

/// Guests whose rules were found to differ from their host's this round, with the host: told
/// once a round (0.5.2's R4).
fn told() -> std::sync::MutexGuard<'static, HashMap<u32, u32>> {
    static TOLD: std::sync::LazyLock<Mutex<HashMap<u32, u32>>> = std::sync::LazyLock::new(Mutex::default);
    TOLD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// R4, consensus: `guest`'s game, in `host`'s match under way, has these settings. The settings
/// both games read must agree (the host sets them; every game has the same): one that differs
/// is a local edit of the guest's game (a cheat table's "rules" on a guest). What to note, once
/// a round: each differing setting, the guest's value and the host's.
pub fn guest_said(guest: u32, host: u32, under_way: bool, settings: &HashMap<String, f32>) -> Option<serde_json::Value> {
    if !under_way {
        told().remove(&guest);
        return None;
    }
    let mine = clean(settings);
    let hosts = rounds();
    let theirs = hosts.get(&host)?;
    let differ: BTreeMap<&String, (f32, f32)> = mine
        .iter()
        .filter_map(|(k, v)| theirs.get(k).filter(|h| (*h - v).abs() > 0.011).map(|h| (k, (*v, *h))))
        .collect();
    if differ.is_empty() {
        return None;
    }
    let mut t = told();
    if t.get(&guest) == Some(&host) {
        return None;
    }
    if t.len() >= MOST_HOSTS {
        t.clear();
    }
    t.insert(guest, host);
    let said: Vec<String> = differ.iter().map(|(k, (g, h))| format!("{k} {g} (host {h})")).collect();
    Some(serde_json::json!({
        "check": "rules",
        "detail": format!("their game's rules aren't their host's: {}", said.join(", ")),
        "value": differ.len(),
        "host": host,
        "wine": false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R4: a guest whose game reads other rules than its host's is noted once a round.
    #[test]
    fn a_guest_whose_rules_differ_from_the_hosts_is_noted_once() {
        let s = |respawn: f32, limit: f32| HashMap::from([(String::from("m_fRespawnTimeSpyTeam"), respawn), (String::from("m_iScoreLimit"), limit)]);
        assert!(guest_said(700_101, 700_100, true, &s(5.0, 10.0)).is_none(), "the host said nothing yet");
        said(700_100, true, &s(5.0, 10.0));
        assert!(guest_said(700_101, 700_100, true, &s(5.0, 10.0)).is_none(), "the same");
        let d = guest_said(700_102, 700_100, true, &s(0.5, 10.0)).unwrap();
        assert_eq!((d["check"].as_str(), d["value"].as_u64()), (Some("rules"), Some(1)));
        assert!(d["detail"].as_str().unwrap().contains("m_fRespawnTimeSpyTeam 0.5 (host 5)"), "{d}");
        assert!(guest_said(700_102, 700_100, true, &s(0.5, 10.0)).is_none(), "once a round");
        guest_said(700_102, 700_100, false, &s(0.5, 10.0));
        assert!(guest_said(700_102, 700_100, true, &s(0.5, 10.0)).is_some(), "the next round again");
    }

    #[test]
    fn a_round_is_noted_as_it_starts_and_a_change_while_it_runs() {
        let s = |respawn: f32| HashMap::from([(String::from("m_fRespawnTimeSpyTeam"), respawn), (String::from("bad name!"), 1.0)]);
        assert_eq!(said(700_001, false, &s(5.0)), None, "not under way");
        let start = said(700_001, true, &s(5.0)).unwrap();
        assert_eq!(start["settings"], serde_json::json!({ "m_fRespawnTimeSpyTeam": 5.0 }));
        assert_eq!(said(700_001, true, &s(5.0)), None, "the same");
        let during = said(700_001, true, &s(500.0)).unwrap();
        assert_eq!(during["changes"]["m_fRespawnTimeSpyTeam"], serde_json::json!([5.0, 500.0]));
        assert_eq!(said(700_001, false, &s(500.0)), None);
        assert!(said(700_001, true, &s(500.0)).unwrap()["start"] == true, "the next round");
    }
}
