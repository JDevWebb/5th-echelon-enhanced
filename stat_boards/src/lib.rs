//! The game's statistics: which boards it writes, which stats each holds and how a
//! write adds to what is stored, and the leaderboards ranked from them. The game
//! reads these numbers from its own stats configuration; the servers (each player's
//! stats on that server) and the coordinator (each player's stats across the network,
//! and the global leaderboards) need them to keep the stats the way Ubisoft's did.
//! Only the numbers are here.

use std::ops::RangeInclusive;

/// How a write to a stat changes the stored value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregation {
    Add,
    Overwrite,
    Maximum,
    Minimum,
    /// Not written: the first stat divided by the second, worked out when read.
    Ratio(u32, u32),
}

use Aggregation::Add;
use Aggregation::Maximum;
use Aggregation::Minimum;
use Aggregation::Overwrite;
use Aggregation::Ratio;

/// A statboard: the stats it holds and, for boards kept per game mode, weapon,
/// gadget, mission or ladder, the contexts it is kept for. A board without
/// contexts is kept once, as context 0.
pub struct Board {
    pub id: u32,
    contexts: &'static [RangeInclusive<u32>],
    pub entries: &'static [(u32, Aggregation)],
}

impl Board {
    pub fn has_context(&self, context: u32) -> bool {
        if self.contexts.is_empty() {
            context == 0
        } else {
            self.contexts.iter().any(|r| r.contains(&context))
        }
    }

    /// The contexts it is kept for: 0 alone for a board without contexts.
    pub fn context_ids(&self) -> Vec<u32> {
        if self.contexts.is_empty() {
            vec![0]
        } else {
            self.contexts.iter().flat_map(Clone::clone).collect()
        }
    }

    pub fn aggregation(&self, stat: u32) -> Option<Aggregation> {
        self.entries.iter().find(|(id, _)| *id == stat).map(|(_, a)| *a)
    }
}

const NONE: &[RangeInclusive<u32>] = &[];
const SOLO_MISSIONS: &[RangeInclusive<u32>] = &[100..=112];
const COOP_MISSIONS: &[RangeInclusive<u32>] = &[113..=128];
const PER_MISSION: &[(u32, Aggregation)] = &[
    (120, Overwrite),
    (121, Overwrite),
    (126, Overwrite),
    (127, Overwrite),
    (129, Overwrite),
    (178, Overwrite),
    (179, Overwrite),
    (180, Overwrite),
    (181, Overwrite),
    (190, Overwrite),
    (191, Overwrite),
    (192, Overwrite),
    (193, Overwrite),
];
const HIGH_SCORE: &[(u32, Aggregation)] = &[(153, Overwrite), (156, Overwrite), (157, Overwrite), (158, Overwrite), (159, Overwrite), (160, Overwrite)];
const BEST_TIME: &[(u32, Aggregation)] = &[(154, Overwrite), (155, Overwrite), (157, Overwrite), (158, Overwrite), (159, Overwrite), (160, Overwrite)];
const OVERVIEW: &[(u32, Aggregation)] = &[
    (100, Overwrite),
    (101, Overwrite),
    (104, Overwrite),
    (105, Overwrite),
    (106, Overwrite),
    (107, Overwrite),
    (108, Overwrite),
    (111, Overwrite),
    (112, Overwrite),
    (113, Overwrite),
    (114, Overwrite),
];
/// Medals: stats 238 to 280, all added up.
const MEDALS: &[(u32, Aggregation)] = &{
    let mut medals = [(0, Add); 43];
    let mut i = 0;
    while i < medals.len() {
        medals[i].0 = 238 + i as u32;
        i += 1;
    }
    medals
};

pub const BOARDS: &[Board] = &[
    // Global, kept by the server.
    Board {
        id: 1,
        contexts: NONE,
        entries: &[
            (132, Add),
            (133, Add),
            (134, Add),
            (135, Add),
            (136, Add),
            (137, Overwrite),
            (182, Add),
            (183, Add),
            (184, Add),
        ],
    },
    // Global, kept by the game: experience, tokens and cash.
    Board {
        id: 2,
        contexts: NONE,
        entries: &[
            (140, Overwrite),
            (142, Overwrite),
            (143, Overwrite),
            (144, Overwrite),
            (145, Overwrite),
            (146, Overwrite),
            (147, Overwrite),
        ],
    },
    // Spies vs Mercs, per game mode.
    Board {
        id: 10,
        contexts: &[227..=231],
        entries: &[
            (100, Add),
            (101, Add),
            (102, Ratio(100, 101)),
            (104, Add),
            (105, Add),
            (106, Ratio(105, 104)),
            (107, Add),
            (108, Ratio(107, 104)),
            (111, Add),
            (113, Add),
            (120, Add),
            (121, Add),
            (122, Add),
            (123, Add),
            (124, Add),
            (125, Ratio(122, 124)),
            (126, Add),
            (127, Add),
            (129, Add),
            (199, Add),
            (200, Add),
            (201, Add),
            (202, Add),
            (203, Add),
            (204, Add),
            (205, Add),
            (211, Add),
            (212, Maximum),
            (213, Minimum),
            (214, Add),
            (215, Ratio(211, 214)),
            (216, Maximum),
            (217, Maximum),
            (218, Maximum),
            (219, Maximum),
            (225, Maximum),
            (226, Maximum),
            (227, Maximum),
            (228, Maximum),
            (229, Maximum),
            (230, Maximum),
            (231, Maximum),
            (232, Maximum),
        ],
    },
    // Spies vs Mercs, per weapon and per gadget.
    Board {
        id: 11,
        contexts: &[129..=162],
        entries: &[(100, Add), (104, Add), (105, Add), (106, Ratio(105, 104)), (107, Add), (108, Ratio(107, 104))],
    },
    Board {
        id: 12,
        contexts: &[201..=211],
        entries: &[(100, Add), (109, Add)],
    },
    Board {
        id: 13,
        contexts: NONE,
        entries: MEDALS,
    },
    // The ladders.
    Board {
        id: 17,
        contexts: &[1..=3],
        entries: &[(100, Add), (111, Add), (113, Add), (122, Add), (126, Add), (127, Add), (199, Add)],
    },
    // Solo missions.
    Board {
        id: 20,
        contexts: NONE,
        entries: OVERVIEW,
    },
    Board {
        id: 21,
        contexts: SOLO_MISSIONS,
        entries: PER_MISSION,
    },
    Board {
        id: 22,
        contexts: SOLO_MISSIONS,
        entries: HIGH_SCORE,
    },
    Board {
        id: 23,
        contexts: SOLO_MISSIONS,
        entries: BEST_TIME,
    },
    // Co-op missions.
    Board {
        id: 25,
        contexts: NONE,
        entries: OVERVIEW,
    },
    Board {
        id: 26,
        contexts: COOP_MISSIONS,
        entries: PER_MISSION,
    },
    Board {
        id: 27,
        contexts: COOP_MISSIONS,
        entries: HIGH_SCORE,
    },
    Board {
        id: 28,
        contexts: COOP_MISSIONS,
        entries: BEST_TIME,
    },
    // Solo and co-op, per weapon and per gadget.
    Board {
        id: 30,
        contexts: &[163..=200],
        entries: &[(100, Overwrite), (104, Overwrite), (105, Overwrite), (106, Overwrite), (107, Overwrite), (108, Overwrite)],
    },
    Board {
        id: 31,
        contexts: &[212..=225],
        entries: &[(100, Overwrite), (109, Overwrite)],
    },
    // The last mission played.
    Board {
        id: 32,
        contexts: NONE,
        entries: &[
            (166, Overwrite),
            (167, Overwrite),
            (168, Overwrite),
            (169, Overwrite),
            (170, Overwrite),
            (171, Overwrite),
            (172, Overwrite),
        ],
    },
];

pub fn board(id: u32) -> Option<&'static Board> {
    BOARDS.iter().find(|b| b.id == id)
}

/// The stats that are fractions; all others are whole numbers.
const FRACTIONS: [u32; 7] = [102, 106, 108, 125, 215, 228, 229];

pub fn is_fraction(stat: u32) -> bool {
    FRACTIONS.contains(&stat)
}

/// A stat's value before anything was written: 0, except for the stats where lower
/// is better (best time, shortest life), which start at the largest value.
pub fn default_value(stat: u32) -> f64 {
    match stat {
        154 | 213 => f64::from(i32::MAX),
        _ => 0.0,
    }
}

/// A leaderboard: the players with `stat` on `board`, best first.
pub struct Leaderboard {
    pub id: u32,
    pub board: u32,
    pub stat: u32,
    pub descending: bool,
}

pub const LEADERBOARDS: &[Leaderboard] = &[
    // High score and best time, per solo mission and per co-op mission.
    Leaderboard {
        id: 1,
        board: 22,
        stat: 153,
        descending: true,
    },
    Leaderboard {
        id: 2,
        board: 23,
        stat: 154,
        descending: false,
    },
    Leaderboard {
        id: 3,
        board: 27,
        stat: 153,
        descending: true,
    },
    Leaderboard {
        id: 4,
        board: 28,
        stat: 154,
        descending: false,
    },
    // Spies vs Mercs total score, per game mode.
    Leaderboard {
        id: 5,
        board: 10,
        stat: 127,
        descending: true,
    },
    // The ladders: time played, wins, deaths from above, takedowns, kills, score, objectives.
    Leaderboard {
        id: 6,
        board: 17,
        stat: 126,
        descending: true,
    },
    Leaderboard {
        id: 7,
        board: 17,
        stat: 122,
        descending: true,
    },
    Leaderboard {
        id: 8,
        board: 17,
        stat: 113,
        descending: true,
    },
    Leaderboard {
        id: 9,
        board: 17,
        stat: 111,
        descending: true,
    },
    Leaderboard {
        id: 10,
        board: 17,
        stat: 100,
        descending: true,
    },
    Leaderboard {
        id: 11,
        board: 17,
        stat: 127,
        descending: true,
    },
    Leaderboard {
        id: 12,
        board: 17,
        stat: 199,
        descending: true,
    },
];

pub fn leaderboard(id: u32) -> Option<&'static Leaderboard> {
    LEADERBOARDS.iter().find(|l| l.id == id)
}

/// `stored` after a write of `written`.
pub fn aggregate(aggregation: Aggregation, stored: Option<f64>, written: f64) -> f64 {
    match (aggregation, stored) {
        (Add, Some(s)) => s + written,
        (Maximum, Some(s)) => s.max(written),
        (Minimum, Some(s)) => s.min(written),
        _ => written,
    }
}

/// A ratio stat: `left / right`, or `left` while `right` is 0 (a player without
/// deaths has a kill/death ratio of their kills).
pub fn ratio(left: f64, right: f64) -> f64 {
    if right == 0.0 {
        left
    } else {
        left / right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boards_are_consistent() {
        for (i, b) in BOARDS.iter().enumerate() {
            assert!(BOARDS[..i].iter().all(|o| o.id != b.id), "board {} twice", b.id);
            for (stat, aggregation) in b.entries {
                if let Ratio(left, right) = aggregation {
                    assert!(b.aggregation(*left).is_some() && b.aggregation(*right).is_some(), "board {} stat {stat}", b.id);
                    assert!(is_fraction(*stat), "{stat} is a ratio");
                }
            }
        }
        for l in LEADERBOARDS {
            let b = board(l.board).expect("leaderboard on a known board");
            assert!(matches!(b.aggregation(l.stat), Some(a) if !matches!(a, Ratio(..))), "leaderboard {}", l.id);
        }
        assert_eq!(MEDALS.first(), Some(&(238, Add)));
        assert_eq!(MEDALS.last(), Some(&(280, Add)));
    }

    #[test]
    fn writes_add_up_as_the_board_says() {
        assert_eq!(aggregate(Add, Some(3.0), 2.0), 5.0);
        assert_eq!(aggregate(Add, None, 2.0), 2.0);
        assert_eq!(aggregate(Overwrite, Some(3.0), 2.0), 2.0);
        assert_eq!(aggregate(Maximum, Some(3.0), 2.0), 3.0);
        assert_eq!(aggregate(Minimum, Some(3.0), 2.0), 2.0);
        assert_eq!(ratio(10.0, 4.0), 2.5);
        assert_eq!(ratio(10.0, 0.0), 10.0);
    }

    #[test]
    fn contexts() {
        let svm = board(10).unwrap();
        assert!(svm.has_context(228) && !svm.has_context(0) && !svm.has_context(232));
        let global = board(1).unwrap();
        assert!(global.has_context(0) && !global.has_context(1));
        assert_eq!(global.context_ids(), [0]);
        assert_eq!(board(17).unwrap().context_ids(), [1, 2, 3]);
    }
}
