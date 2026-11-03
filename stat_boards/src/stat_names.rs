//! The game's own names for its stats, statboards and contexts, by number: the
//! `PlayerStatsConfig.xml` in its `dynamicwin.umd` names each (`m_Kills` is stat 100,
//! `sb_AdvPerGameMode` board 10, `cxt_SVM_Classic` context 231). Only the numbers and the
//! names; [`readable`] turns one into words for the admin UI.

/// (stat, the game's name).
pub const STATS: &[(u32, &str)] = &[
    (100, "m_Kills"),
    (101, "m_Deaths"),
    (102, "m_KDRatio"),
    (103, "m_Assists"),
    (104, "m_ShotsFired"),
    (105, "m_ShotsHit"),
    (106, "m_Accuracy"),
    (107, "m_Headshots"),
    (108, "m_HeadshotPercentage"),
    (109, "m_NumTimesUsed"),
    (111, "m_h2hLethal"),
    (112, "m_h2hNonLethal"),
    (113, "m_DeathFromAbove"),
    (114, "m_MarkAndExecute"),
    (120, "m_GamesStarted"),
    (121, "m_GamesCompleted"),
    (122, "m_Wins"),
    (123, "m_Ties"),
    (124, "m_Losses"),
    (125, "m_WLRatio"),
    (126, "m_TotalTime"),
    (127, "m_TotalScore"),
    (129, "m_TotalEarnings"),
    (132, "m_W2GSuitGoggles"),
    (133, "m_W2GSuitBoots"),
    (134, "m_W2GSuitGloves"),
    (135, "m_W2GTransfer"),
    (136, "m_Logins"),
    (137, "m_LastStatWrite"),
    (140, "m_Experience"),
    (142, "m_AdvTokensOnHand"),
    (143, "m_AdvTokensEarned"),
    (144, "m_AdvTokensSpent"),
    (145, "m_CashOnHand"),
    (146, "m_CashEarned"),
    (147, "m_CashSpent"),
    (153, "m_HighScore"),
    (154, "m_BestTime"),
    (155, "m_HSBT_Score"),
    (156, "m_HSBT_Time"),
    (157, "m_HSBT_Difficulty"),
    (158, "m_HSBT_GhostComponent"),
    (159, "m_HSBT_PantherComponent"),
    (160, "m_HSBT_AssaultComponent"),
    (166, "m_LM_MissionID"),
    (167, "m_LM_Score"),
    (168, "m_LM_Time"),
    (169, "m_LM_Difficulty"),
    (170, "m_LM_GhostComponent"),
    (171, "m_LM_PantherComponent"),
    (172, "m_LM_AssaultComponent"),
    (178, "m_HighestDifficulty"),
    (179, "m_GPA_HighestEverGhost"),
    (180, "m_GPA_HighestEverPanther"),
    (181, "m_GPA_HighestEverAssault"),
    (182, "m_GPA_AccumulatedGhost"),
    (183, "m_GPA_AccumulatedPanther"),
    (184, "m_GPA_AccumulatedAssault"),
    (190, "m_SecondaryObjectivesCount"),
    (191, "m_SO_HVTCaptured"),
    (192, "m_SO_LaptopHacked"),
    (193, "m_SO_DeadDropPickedUp"),
    (199, "m_AdvObjectivesCount"),
    (200, "m_AO_CTI_Captures"),
    (201, "m_AO_CTI_Resets"),
    (202, "m_AO_FLD_Captures"),
    (203, "m_AO_FLD_Defends"),
    (204, "m_AO_SVM_Hacks"),
    (205, "m_AO_SVM_Disrupts"),
    (211, "m_TimeSpentAlive"),
    (212, "m_LongestLife"),
    (213, "m_ShortestLife"),
    (214, "m_Spawns"),
    (215, "m_AverageLife"),
    (216, "m_LongestKillstreak"),
    (217, "m_LongestDeathstreak"),
    (218, "m_FarthestKill"),
    (219, "m_FarthestHeadshot"),
    (225, "m_AMPG_Kills"),
    (226, "m_AMPG_Score"),
    (227, "m_AMPG_Earnings"),
    (228, "m_AMPG_KDR"),
    (229, "m_AMPG_Acc"),
    (230, "m_AMPG_Ojectives"),
    (231, "m_AMPG_h2h"),
    (232, "m_AMPG_DFA"),
    (238, "m_Medal_Weapon"),
    (239, "m_Medal_Frag"),
    (240, "m_Medal_ProxyMine"),
    (241, "m_Medal_TearGas"),
    (242, "m_Medal_Incendiary"),
    (243, "m_Medal_Cam"),
    (244, "m_Medal_Drone"),
    (245, "m_Medal_Melee"),
    (246, "m_Medal_LedgeGrab"),
    (247, "m_Medal_DeathFromAbove"),
    (248, "m_Medal_CoverGrab"),
    (249, "m_Medal_NeckSnap"),
    (250, "m_Medal_Assist"),
    (251, "m_Medal_MajorAssist"),
    (252, "m_Medal_CoopKill"),
    (253, "m_Medal_TagAssist"),
    (254, "m_Medal_DisruptorAssist"),
    (255, "m_Medal_EMPAssist"),
    (256, "m_Medal_FlashbangAssist"),
    (257, "m_Medal_StunAssist"),
    (258, "m_Medal_DroneDestroyed"),
    (259, "m_Medal_Headshot"),
    (260, "m_Medal_Killstreak3"),
    (261, "m_Medal_Killstreak5"),
    (262, "m_Medal_Killstreak10"),
    (263, "m_Medal_Killstreak15"),
    (264, "m_Medal_Killstreak20"),
    (265, "m_Medal_Comeback"),
    (266, "m_Medal_Defender"),
    (267, "m_Medal_HackerKiller"),
    (268, "m_Medal_CourierKiller"),
    (269, "m_Medal_LongShot"),
    (270, "m_Medal_KillstreakDisruption"),
    (271, "m_Medal_FromTheGrave"),
    (272, "m_Medal_GhostKill"),
    (273, "m_Medal_HangedKill"),
    (274, "m_Medal_FirstBlood"),
    (275, "m_Medal_DoubleKill"),
    (276, "m_Medal_TripleKill"),
    (277, "m_Medal_MultiKill"),
    (278, "m_Medal_Avenger"),
    (279, "m_Medal_Nemesis"),
    (280, "m_Medal_BlindKill"),
];

/// (board, the game's name).
pub const BOARDS: &[(u32, &str)] = &[
    (1, "sb_GlobalServer"),
    (2, "sb_GlobalLocal"),
    (10, "sb_AdvPerGameMode"),
    (11, "sb_AdvPerWeapon"),
    (12, "sb_AdvPerGadget"),
    (13, "sb_AdvMedals"),
    (17, "sb_Ladder"),
    (20, "sb_SoloOverview"),
    (21, "sb_SoloPerMission"),
    (22, "sb_SoloHighScore"),
    (23, "sb_SoloBestTime"),
    (25, "sb_CoopOverview"),
    (26, "sb_CoopPerMission"),
    (27, "sb_CoopHighScore"),
    (28, "sb_CoopBestTime"),
    (30, "sb_SoloAndCoopPerWeapon"),
    (31, "sb_SoloAndCoopPerGadget"),
    (32, "sb_LastMission"),
];

/// (context, the game's name): ladders 1-3, solo missions 100-112, co-op missions 113-128, SvM
/// weapons 129-162, solo and co-op weapons 163-200, SvM gadgets 201-211, solo and co-op gadgets
/// 212-225, SvM game modes 227-231.
pub const CONTEXTS: &[(u32, &str)] = &[
    (1, "cxt_Ladder_1"),
    (2, "cxt_Ladder_2"),
    (3, "cxt_Ladder_3"),
    (100, "cxt_S_AFB"),
    (101, "cxt_S_KOB"),
    (102, "cxt_S_BAD"),
    (103, "cxt_S_CON"),
    (104, "cxt_S_NOU"),
    (105, "cxt_S_CHE"),
    (106, "cxt_S_QOD"),
    (107, "cxt_S_FRE"),
    (108, "cxt_S_DET"),
    (109, "cxt_S_AIR"),
    (110, "cxt_S_PAL"),
    (111, "cxt_S_FUE"),
    (112, "cxt_S_BLO"),
    (113, "cxt_C01"),
    (114, "cxt_C02"),
    (115, "cxt_C03"),
    (116, "cxt_C04"),
    (117, "cxt_E01"),
    (118, "cxt_E02"),
    (119, "cxt_E03"),
    (120, "cxt_E04"),
    (121, "cxt_G01"),
    (122, "cxt_G02"),
    (123, "cxt_G03"),
    (124, "cxt_G04"),
    (125, "cxt_H01"),
    (126, "cxt_H02"),
    (127, "cxt_H03"),
    (128, "cxt_H04"),
    (129, "cxt_ADV_Merc_Pistol_DesertEagle"),
    (130, "cxt_ADV_Merc_Pistol_Makarov"),
    (131, "cxt_ADV_Merc_Pistol_Px4"),
    (132, "cxt_ADV_Merc_Assault_AK47"),
    (133, "cxt_ADV_Merc_Assault_ACE"),
    (134, "cxt_ADV_Merc_Assault_ARX160"),
    (135, "cxt_ADV_Merc_Assault_FAMASG2"),
    (136, "cxt_ADV_Merc_Assault_G36C"),
    (137, "cxt_ADV_Merc_Assault_Goblin"),
    (138, "cxt_ADV_Merc_Assault_HoneyBadger"),
    (139, "cxt_ADV_Merc_Assault_M416"),
    (140, "cxt_ADV_Merc_Assault_SA58"),
    (141, "cxt_ADV_Merc_LMG_HKMG4"),
    (142, "cxt_ADV_Merc_LMG_U100"),
    (143, "cxt_ADV_Merc_Shotgun_AA12"),
    (144, "cxt_ADV_Merc_Shotgun_M1014"),
    (145, "cxt_ADV_Merc_Shotgun_RMB93"),
    (146, "cxt_ADV_Merc_Shotgun_Saiga12"),
    (147, "cxt_ADV_Spy_SMG_MP5"),
    (148, "cxt_ADV_Spy_Pistol_F40"),
    (149, "cxt_ADV_Spy_Pistol_FiveSeven"),
    (150, "cxt_ADV_Spy_Pistol_Proto"),
    (151, "cxt_ADV_Spy_Pistol_USP45"),
    (152, "cxt_ADV_Spy_Assault_Proto"),
    (153, "cxt_ADV_Spy_Shotgun_Proto"),
    (154, "cxt_ADV_Spy_SMG_MAC11"),
    (155, "cxt_ADV_Spy_SMG_MP7"),
    (156, "cxt_ADV_Spy_SMG_PP19"),
    (157, "cxt_ADV_Spy_SMG_PP2000"),
    (158, "cxt_ADV_Spy_SMG_Proto"),
    (159, "cxt_ADV_Spy_SMG_Scorpion"),
    (160, "cxt_ADV_Spy_SMG_UMP45"),
    (161, "cxt_ADV_Spy_SMG_Vector"),
    (162, "cxt_ADV_Spy_CrossBow"),
    (163, "cxt_SnCW_Pistol_FiveSeven"),
    (164, "cxt_SnCW_Pistol_CZ75"),
    (165, "cxt_SnCW_Pistol_DesertEagle"),
    (166, "cxt_SnCW_Pistol_F40"),
    (167, "cxt_SnCW_Pistol_Makarov"),
    (168, "cxt_SnCW_Pistol_Proto"),
    (169, "cxt_SnCW_Pistol_PX4"),
    (170, "cxt_SnCW_Pistol_USP45"),
    (171, "cxt_SnCW_Rifle_AK47"),
    (172, "cxt_SnCW_Rifle_AKS74U"),
    (173, "cxt_SnCW_Rifle_ARX"),
    (174, "cxt_SnCW_Rifle_FAMAS"),
    (175, "cxt_SnCW_Rifle_G36C"),
    (176, "cxt_SnCW_Rifle_Goblin"),
    (177, "cxt_SnCW_Rifle_HoneyBadger"),
    (178, "cxt_SnCW_Rifle_M416"),
    (179, "cxt_SnCW_Rifle_Proto"),
    (180, "cxt_SnCW_Rifle_SA58"),
    (181, "cxt_SnCW_Shotgun_AA12"),
    (182, "cxt_SnCW_Shotgun_M1014"),
    (183, "cxt_SnCW_Shotgun_Proto"),
    (184, "cxt_SnCW_Shotgun_RMB93"),
    (185, "cxt_SnCW_Shotgun_Saiga12"),
    (186, "cxt_SnCW_SMG_MAC11"),
    (187, "cxt_SnCW_SMG_MP5"),
    (188, "cxt_SnCW_SMG_MP7A2"),
    (189, "cxt_SnCW_SMG_PP19"),
    (190, "cxt_SnCW_SMG_PP2000"),
    (191, "cxt_SnCW_SMG_Proto"),
    (192, "cxt_SnCW_SMG_Scorpion"),
    (193, "cxt_SnCW_SMG_UMP45"),
    (194, "cxt_SnCW_SMG_Vector"),
    (195, "cxt_SnCW_Sniper_Proto"),
    (196, "cxt_SnCW_Sniper_SR25"),
    (197, "cxt_SnCW_Sniper_SVU"),
    (198, "cxt_SnCW_Sniper_VSS"),
    (199, "cxt_SnCW_Crossbow_1H"),
    (200, "cxt_SnCW_Tazer"),
    (201, "cxt_GI_Adv_Grenade_Flashbang"),
    (202, "cxt_GI_Adv_Sticky_Camera"),
    (203, "cxt_GI_Adv_Grenade_EMP"),
    (204, "cxt_GI_Adv_AntiGrenadeSystem"),
    (205, "cxt_GI_Adv_EscBomb"),
    (206, "cxt_GI_Adv_Grenade_Concussion"),
    (207, "cxt_GI_Adv_Grenade_Frag"),
    (208, "cxt_GI_Adv_Grenade_TearGas"),
    (209, "cxt_GI_Adv_AmmoPack"),
    (210, "cxt_GI_Adv_Proximity_Mine"),
    (211, "cxt_GI_Adv_TaggingDevice"),
    (212, "cxt_GI_Sticky_Camera"),
    (213, "cxt_GI_Quadrocopter"),
    (214, "cxt_GI_DoorBreach"),
    (215, "cxt_GI_SnakeCamera"),
    (216, "cxt_GI_Proximity_Mine"),
    (217, "cxt_GI_Proximity_Shocker_Mine"),
    (218, "cxt_GI_Grenade_Frag"),
    (219, "cxt_GI_Grenade_Incendiary"),
    (220, "cxt_GI_Grenade_EMP"),
    (221, "cxt_GI_Grenade_Flashbang"),
    (222, "cxt_GI_Grenade_Smoke"),
    (223, "cxt_GI_Grenade_SleepingGas"),
    (224, "cxt_GI_Grenade_TearGas"),
    (225, "cxt_GI_Sticky_Noisemaker"),
    (227, "cxt_TDM_Standard"),
    (228, "cxt_SvM_Standard"),
    (229, "cxt_CTI_Standard"),
    (230, "cxt_FLD_Standard"),
    (231, "cxt_SVM_Classic"),
];

/// Stats whose words the game's name doesn't give.
const STAT_WORDS: &[(u32, &str)] = &[
    (102, "Kill/death ratio"),
    (109, "Times used"),
    (111, "Hand-to-hand kills"),
    (112, "Hand-to-hand takedowns (non-lethal)"),
    (125, "Win/loss ratio"),
    (142, "SvM tokens on hand"),
    (143, "SvM tokens earned"),
    (144, "SvM tokens spent"),
    (199, "SvM objectives"),
    (200, "Uplink Control: captures"),
    (201, "Uplink Control: resets"),
    (202, "Extraction: captures"),
    (203, "Extraction: defends"),
    (204, "Spies vs Mercs: hacks"),
    (205, "Spies vs Mercs: disrupts"),
    (228, "Most in a game: kill/death ratio"),
    (229, "Most in a game: accuracy"),
    (230, "Most in a game: objectives"),
    (231, "Most in a game: hand-to-hand"),
    (232, "Most in a game: deaths from above"),
];

/// SvM game modes' contexts, in the game's words for the modes.
const MODE_WORDS: &[(u32, &str)] = &[
    (227, "Team Deathmatch"),
    (228, "SvM Blacklist"),
    (229, "Uplink Control"),
    (230, "Extraction"),
    (231, "SvM Classic"),
];

/// Boards in words.
const BOARD_WORDS: &[(u32, &str)] = &[
    (1, "Global (kept by the server)"),
    (2, "Global (kept by the game)"),
    (10, "Spies vs Mercs, per game mode"),
    (11, "Spies vs Mercs, per weapon"),
    (12, "Spies vs Mercs, per gadget"),
    (13, "Spies vs Mercs medals"),
    (17, "Ladders"),
    (20, "Solo overview"),
    (21, "Solo, per mission"),
    (22, "Solo high scores"),
    (23, "Solo best times"),
    (25, "Co-op overview"),
    (26, "Co-op, per mission"),
    (27, "Co-op high scores"),
    (28, "Co-op best times"),
    (30, "Solo and co-op, per weapon"),
    (31, "Solo and co-op, per gadget"),
    (32, "Last mission played"),
];

fn find(list: &[(u32, &'static str)], id: u32) -> Option<&'static str> {
    list.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
}

/// A name's words: `ShotsFired` "Shots fired", `KDRatio` "KD ratio", `Killstreak3`
/// "Killstreak 3", `AK47` as it is; a word in capitals (an initialism) kept as it is.
fn words(name: &str) -> String {
    sentence(name, true)
}

/// As [`words`], the first word lowercase too (after a "Prefix: ").
fn lower(name: &str) -> String {
    sentence(name, false)
}

fn sentence(name: &str, capital: bool) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut word = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1).copied();
        let starts = c == '_'
            || prev.is_some_and(|p| {
                (p.is_lowercase() && c.is_uppercase())
                    || (p.is_lowercase() && c.is_ascii_digit() && word.len() > 3)
                    || (p.is_uppercase() && c.is_uppercase() && next.is_some_and(char::is_lowercase))
            });
        if starts && !word.is_empty() {
            parts.push(std::mem::take(&mut word));
        }
        if c != '_' {
            word.push(c);
        }
    }
    if !word.is_empty() {
        parts.push(word);
    }
    parts
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let initialism = w.len() > 1 && w.chars().all(|c| c.is_uppercase() || c.is_ascii_digit());
            if initialism {
                w.clone()
            } else if i == 0 && capital {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
            } else {
                w.to_lowercase()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A stat in words ("Shots fired", "Medal: headshot", "Most in a game: kills"), or None for a
/// number the game doesn't name.
pub fn stat(id: u32) -> Option<String> {
    if let Some(w) = find(STAT_WORDS, id) {
        return Some(w.into());
    }
    let name = find(STATS, id)?.trim_start_matches("m_");
    let (prefix, rest) = match name.split_once('_') {
        Some(("Medal", rest)) => ("Medal: ", rest),
        Some(("AMPG", rest)) => ("Most in a game: ", rest),
        Some(("LM", rest)) => ("Last mission: ", rest),
        Some(("HSBT", rest)) => ("Best run: ", rest),
        Some(("SO", rest)) => ("Secondary objective: ", rest),
        Some(("GPA", rest)) => ("Playstyle points: ", rest),
        _ => ("", name),
    };
    Some(if prefix.is_empty() { words(rest) } else { format!("{prefix}{}", lower(rest)) })
}

/// A board in words, or None.
pub fn board(id: u32) -> Option<String> {
    find(BOARD_WORDS, id)
        .map(str::to_string)
        .or_else(|| find(BOARDS, id).map(|n| words(n.trim_start_matches("sb_"))))
}

/// A context in words ("SvM Classic", "Ladder 2", "Solo mission AFB", "Co-op mission C01",
/// "SvM merc: assault AK47", "Solo and co-op gadget: grenade frag"), or None.
pub fn context(id: u32) -> Option<String> {
    if let Some(w) = find(MODE_WORDS, id) {
        return Some(w.into());
    }
    let name = find(CONTEXTS, id)?.trim_start_matches("cxt_");
    Some(if let Some(n) = name.strip_prefix("Ladder_") {
        format!("Ladder {n}")
    } else if let Some(code) = name.strip_prefix("S_") {
        format!("Solo mission {code}")
    } else if (113..=128).contains(&id) {
        format!("Co-op mission {name}")
    } else if let Some(rest) = name.strip_prefix("ADV_Merc_") {
        format!("SvM merc: {}", lower(rest))
    } else if let Some(rest) = name.strip_prefix("ADV_Spy_") {
        format!("SvM spy: {}", lower(rest))
    } else if let Some(rest) = name.strip_prefix("SnCW_") {
        format!("Solo and co-op: {}", lower(rest))
    } else if let Some(rest) = name.strip_prefix("GI_Adv_") {
        format!("SvM gadget: {}", lower(rest))
    } else if let Some(rest) = name.strip_prefix("GI_") {
        format!("Solo and co-op gadget: {}", lower(rest))
    } else {
        words(name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every number the boards keep has a name, and the words read as the admin UI shows them.
    #[test]
    fn every_kept_stat_board_and_context_is_named() {
        for b in crate::BOARDS {
            assert!(board(b.id).is_some(), "board {}", b.id);
            for (s, _) in b.entries {
                assert!(stat(*s).is_some(), "stat {s} on board {}", b.id);
            }
            for c in b.context_ids().into_iter().filter(|c| *c != 0) {
                assert!(context(c).is_some(), "context {c} on board {}", b.id);
            }
        }
        assert_eq!(stat(100).as_deref(), Some("Kills"));
        assert_eq!(stat(104).as_deref(), Some("Shots fired"));
        assert_eq!(stat(102).as_deref(), Some("Kill/death ratio"));
        assert_eq!(stat(259).as_deref(), Some("Medal: headshot"));
        assert_eq!(stat(225).as_deref(), Some("Most in a game: kills"));
        assert_eq!(stat(262).as_deref(), Some("Medal: killstreak 10"));
        assert_eq!(board(10).as_deref(), Some("Spies vs Mercs, per game mode"));
        assert_eq!(context(231).as_deref(), Some("SvM Classic"));
        assert_eq!(context(2).as_deref(), Some("Ladder 2"));
        assert_eq!(context(132).as_deref(), Some("SvM merc: assault AK47"));
        assert_eq!(context(218).as_deref(), Some("Solo and co-op gadget: grenade frag"));
        assert_eq!(context(113).as_deref(), Some("Co-op mission C01"));
        assert_eq!(context(131).as_deref(), Some("SvM merc: pistol px4"));
        assert_eq!(context(146).as_deref(), Some("SvM merc: shotgun saiga 12"));
        assert_eq!(stat(999), None);
    }
}
