//! The game's names for the maps and game modes a match reports (session attributes 101
//! and 102), so the admin UI shows names instead of numbers. Names an admin gives
//! (`labels`) come first.
//!
//! Maps: each mission's `NetOnlineMapId` in the game's `config\SC6MissionData.xml`, named in
//! English. Game modes: the game's `EGameMode` enum, in order.

use serde_json::json;
use serde_json::Value;

/// (map id, name). A Spies vs Mercs map and its Classic version have ids of their own,
/// except Lost Bank and Presidential Complex, whose two share one.
pub const MAPS: &[(u32, &str)] = &[
    // Briggs (co-op only)
    (0x4F2E_CFAE, "Smugglers Compound"),
    (0x5742_2D32, "Missile Plant"),
    (0x868F_8D40, "VORON Station"),
    (0xFB05_D5A7, "Abandoned City"),
    // Charlie
    (0x995C_EB02, "Pakistani Embassy"),
    (0x2780_28B7, "Swiss Embassy"),
    (0x2E28_3083, "Egyptian Embassy"),
    (0xD108_88B4, "Russian Embassy"),
    // Grim
    (0xD54A_0746, "Hawkins Seafort"),
    (0x51F0_8114, "Border Crossing"),
    (0x2BAA_41A7, "Hackers' Den"),
    (0x3E75_EB6A, "Billionaire's Yacht"),
    // Kobin
    (0x04DE_DD40, "Opium Farm"),
    (0x2C35_D271, "Fish Market"),
    (0x798E_4E17, "Blood Diamond Mine"),
    (0xF19E_884F, "Dead Coast"),
    // Spies vs Mercs
    (0x05F6_9010, "Virus Vault"),
    (0xA3F9_73DD, "Virus Vault (Classic)"),
    (0xB3AA_421F, "Lost Bank"),
    (0x5752_93CC, "Cartel"),
    (0x9893_1ADC, "Cartel (Classic)"),
    (0xC67A_5C7C, "Uranium Mine"),
    (0x545F_5ED4, "Uranium Mine (Classic)"),
    (0x24AD_16A7, "Silo"),
    (0x9072_AC00, "Silo (Classic)"),
    (0x9DD4_24B3, "Lebanese Hospital"),
    (0xD68E_3BB8, "Lebanese Hospital (Classic)"),
    (0x0BF3_AA83, "Particle Accelerator"),
    (0x0454_1E64, "Particle Accelerator (Classic)"),
    (0xBC31_552A, "Presidential Complex"),
];

/// (game mode, name): the game's `EGameMode`.
pub const GAME_MODES: &[(u32, &str)] = &[
    (0, "Single player"),
    (1, "Paladin"),
    (2, "Kobin (Hunter)"),
    (3, "Grim (Ghost)"),
    (4, "Charlie (Extraction)"),
    (5, "Briggs (Combined Ops)"),
    (6, "Team Deathmatch"),
    (7, "SvM Blacklist"),
    (8, "SvM Classic"),
    (9, "Uplink Control"),
    (10, "Extraction"),
];

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
