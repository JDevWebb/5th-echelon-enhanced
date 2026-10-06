//! Support answers waiting for players online here: the network's coordinator says, in
//! each pulse's answer, which of this server's players have answers from its admins they
//! haven't read (`[{player, unread}]`); the game's event poll passes the count to the overlay.

use std::collections::HashMap;
use std::sync::Mutex;

static UNREAD: Mutex<Option<HashMap<u32, u32>>> = Mutex::new(None);

/// Takes a pulse's answer: its list replaces the last (a player not in it has none unread).
pub fn from_pulse(answer: &serde_json::Value) {
    let list: HashMap<u32, u32> = answer["support"]
        .as_array()
        .into_iter()
        .flatten()
        .take(1000)
        .filter_map(|v| {
            let player = u32::try_from(v["player"].as_u64()?).ok()?;
            let unread = u32::try_from(v["unread"].as_u64()?.min(999)).ok()?;
            Some((player, unread))
        })
        .collect();
    *UNREAD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(list);
}

/// Answers `user` hasn't read.
pub fn unread(user: u32) -> u32 {
    UNREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|m| m.get(&user).copied())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn each_pulse_answer_replaces_the_last() {
        super::from_pulse(&serde_json::json!({ "actions": [], "support": [{ "player": 1032, "unread": 2 }, { "player": "x" }] }));
        assert_eq!((super::unread(1032), super::unread(7)), (2, 0));
        super::from_pulse(&serde_json::json!({ "actions": [] }));
        assert_eq!(super::unread(1032), 0, "read since, or an older coordinator");
    }
}
