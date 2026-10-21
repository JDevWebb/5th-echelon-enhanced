//! The game's names for the maps and game modes a match reports (session attributes 101
//! and 102), for the admin UI, the launcher and the overlay.
//!
//! Maps: each mission's `NetOnlineMapId` in the game's `config\SC6MissionData.xml`, named in
//! English. Game modes: the game's `EGameMode` enum, in order. Only the numbers and names.

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

/// (playlist, game mode): the Spies vs Mercs playlists a matchmaking search names (attribute
/// 103, the playlist's id in the game's own data), and the game mode their matches are.
pub const PLAYLISTS: &[(u32, u32)] = &[
    (4_008_726_265, 7), // Blacklist
    (2_165_463_540, 8), // Classic
    (709_003_443, 6),   // Team Deathmatch
    (655_282_787, 10),  // Capture (Extraction)
    (4_078_930_099, 9), // Frontline (Uplink Control)
    (1_487_234_637, 7), // Training Grounds
    (1_365_420_058, 6), // Training Grounds (Team Deathmatch)
];

/// The game mode of a playlist's matches, if it's one the game has.
pub fn playlist_mode(playlist: u32) -> Option<u32> {
    PLAYLISTS.iter().find(|(p, _)| *p == playlist).map(|(_, mode)| *mode)
}

/// A map's name, if it's one the game has.
pub fn map(id: u32) -> Option<&'static str> {
    MAPS.iter().find(|(m, _)| *m == id).map(|(_, name)| *name)
}

/// A game mode's name, if it's one the game has.
pub fn mode(id: u32) -> Option<&'static str> {
    GAME_MODES.iter().find(|(m, _)| *m == id).map(|(_, name)| *name)
}

/// What a match is, for players: its game mode's name ("SvM Classic", "Kobin (Hunter)"),
/// or, when the room didn't say or the number is new, "Spies vs Mercs" or "Co-op" from
/// `kind` ("svm" or "coop"); `None` for anything else.
pub fn match_mode(game_mode: Option<u32>, kind: &str) -> Option<&'static str> {
    game_mode.and_then(mode).filter(|_| game_mode != Some(0)).or(match kind {
        "svm" => Some("Spies vs Mercs"),
        "coop" => Some("Co-op"),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_and_modes_have_names() {
        assert_eq!(map(3_578_398_534), Some("Hawkins Seafort"));
        assert_eq!(map(1), None);
        assert_eq!(mode(8), Some("SvM Classic"));
        assert_eq!(playlist_mode(2_165_463_540).and_then(mode), Some("SvM Classic"));
        assert_eq!(playlist_mode(1), None);
        assert_eq!(match_mode(Some(7), "svm"), Some("SvM Blacklist"));
        assert_eq!(match_mode(Some(3), "coop"), Some("Grim (Ghost)"));
        assert_eq!(match_mode(None, "svm"), Some("Spies vs Mercs"));
        assert_eq!(match_mode(Some(99), "coop"), Some("Co-op"), "a number the game doesn't have");
        assert_eq!(match_mode(Some(0), "coop"), Some("Co-op"), "single player isn't a match");
        assert_eq!(match_mode(None, ""), None);
        let mut ids: Vec<u32> = MAPS.iter().map(|m| m.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MAPS.len(), "one id each");
    }
}
