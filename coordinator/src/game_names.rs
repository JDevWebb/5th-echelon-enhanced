//! The game's names for the maps and game modes a match reports (session attributes 101
//! and 102), so the admin UI shows names instead of numbers. Names an admin gives
//! (`labels`) come first.
//!
//! The names themselves are shared with the server and the clients (`stat_boards::game_names`).

use serde_json::json;
use serde_json::Value;
pub use stat_boards::game_names::GAME_MODES;
pub use stat_boards::game_names::MAPS;

/// The labels for the UI: the admins' (`(kind, id, name)` rows), then the game's names for
/// whatever they haven't named.
pub fn labels(admins: Vec<(String, i64, String)>) -> Vec<Value> {
    let named = |kind: &str, id: i64| admins.iter().any(|(k, i, _)| k == kind && *i == id);
    let game = MAPS
        .iter()
        .map(|(id, name)| ("map", i64::from(*id), *name))
        .chain(GAME_MODES.iter().map(|(id, name)| ("game_mode", i64::from(*id), *name)))
        .filter(|(kind, id, _)| !named(kind, *id))
        .map(|(kind, id, name)| json!({ "kind": kind, "id": id, "name": name, "game": true }));
    admins.iter().map(|(kind, id, name)| json!({ "kind": kind, "id": id, "name": name })).chain(game).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_maps_and_modes_seen_in_matches_have_names() {
        let name = |kind: &str, id: i64| {
            labels(vec![])
                .into_iter()
                .find(|l| l["kind"] == kind && l["id"] == id)
                .map(|l| l["name"].as_str().unwrap().to_string())
        };
        // Real matches: (map, game mode).
        assert_eq!(name("map", 3_578_398_534).as_deref(), Some("Hawkins Seafort"));
        assert_eq!(name("map", 72_621_668).as_deref(), Some("Particle Accelerator (Classic)"));
        assert_eq!(name("map", 615_323_303).as_deref(), Some("Silo"));
        assert_eq!(name("game_mode", 3).as_deref(), Some("Grim (Ghost)"));
        assert_eq!(name("game_mode", 8).as_deref(), Some("SvM Classic"));
        // One id each, and an admin's name wins.
        let mut ids: Vec<u32> = MAPS.iter().map(|m| m.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MAPS.len());
        let l = labels(vec![("map".into(), 615_323_303, "Silo by night".into())]);
        let silo: Vec<&Value> = l.iter().filter(|x| x["kind"] == "map" && x["id"] == 615_323_303).collect();
        assert_eq!(silo.len(), 1);
        assert_eq!(silo[0]["name"], "Silo by night");
    }
}
