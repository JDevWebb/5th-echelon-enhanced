//! Fire rate from the witnesses (0.5.6's W3, F9's missing half; report only). Each player's game
//! sees every character's shots (its weapon's bullet history, with the game's own times): a
//! witness says, for each player, the weapon class they fired most in a minute and the shortest
//! gap between two of its shots. Every weapon of a class fires alike in every game, so a player
//! whose shortest gap is well under what everyone else's is with that class (a cheat table's
//! rate of fire) is noted, as speed is.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::PoisonError;

/// Teamkills in a match a witness's game saw that make it a note (0.5.6's TK4; report only, like
/// the rest of fair play: one or two are accidents with explosives).
pub const TEAMKILLS_NOTED: u32 = 3;

/// Others' samples of a class needed before a player is judged against them.
pub const LEAST_OTHERS: usize = 5;
/// A gap under this share of the others' median is too fast.
pub const TOO_FAST: f32 = 0.6;
/// Samples kept a class (the newest).
const KEPT: usize = 200;
const MOST_CLASSES: usize = 500;

/// By weapon class: (player, their shortest gap in ms), newest last.
static GAPS: Mutex<Option<HashMap<String, VecDeque<(u32, f32)>>>> = Mutex::new(None);

/// `player` fired `class` with a shortest gap of `gap_ms`: the median of the other players' with
/// that class, when it's too fast against it (and there are enough of them).
pub fn seen(player: u32, class: &str, gap_ms: f32) -> Option<f32> {
    if class.is_empty() || class == "?" || !gap_ms.is_finite() || gap_ms <= 0.0 || class.len() > 64 {
        return None;
    }
    let mut all = GAPS.lock().unwrap_or_else(PoisonError::into_inner);
    let all = all.get_or_insert_with(HashMap::new);
    if all.len() >= MOST_CLASSES && !all.contains_key(class) {
        return None;
    }
    let list = all.entry(class.to_string()).or_default();
    // Each other player's shortest, once.
    let mut others: HashMap<u32, f32> = HashMap::new();
    for (p, g) in list.iter().filter(|(p, _)| *p != player) {
        let e = others.entry(*p).or_insert(*g);
        *e = e.min(*g);
    }
    list.push_back((player, gap_ms));
    while list.len() > KEPT {
        list.pop_front();
    }
    if others.len() < LEAST_OTHERS {
        return None;
    }
    let mut gaps: Vec<f32> = others.into_values().collect();
    gaps.sort_by(f32::total_cmp);
    let median = gaps[gaps.len() / 2];
    (gap_ms < median * TOO_FAST).then_some(median)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_firing_far_faster_than_everyone_else_with_the_class_is_noted() {
        let class = "EWeaponTestRifle";
        assert_eq!(seen(1, class, 30.0), None, "nobody to compare with");
        for p in 2..=6 {
            assert_eq!(seen(p, class, 100.0 + p as f32), None);
        }
        assert_eq!(seen(7, class, 95.0), None, "about the same");
        // The others: player 1's 30, players 2-6's 102-106 and player 7's 95: median 103.
        assert_eq!(seen(8, class, 40.0), Some(103.0), "far faster");
        assert_eq!(seen(9, "?", 1.0), None, "no class");
    }
}
