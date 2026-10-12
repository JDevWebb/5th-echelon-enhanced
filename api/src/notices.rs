//! What the server tells a player's game, for its overlay, the moment something goes wrong
//! for them: a join that failed and why, a search that found nothing because of mods. The
//! launcher says the same lines after the game closes ([`mismatch_line`], [`search_mismatch_line`]).

/// Whose game data differs, for a join refused as "a different version", from the server's
/// summary (`mismatch_host`, `mismatch_mine`, `mismatch_theirs`: each side's data by what
/// makes it, "unmodified", "balaclava mod", or as hex). None when the server didn't say
/// (servers before 0.4.4).
pub fn mismatch_line(host: &str, mine: &str, theirs: &str) -> Option<String> {
    fn has(data: &str) -> String {
        match data {
            "unmodified" => "is unmodified".to_string(),
            hex if hex.starts_with("0x") => format!("has other game data ({hex})"),
            name => format!("has the {name}"),
        }
    }
    let host = host.trim();
    let theirs_game = if host.is_empty() { "The host's game".to_string() } else { format!("{host}'s game") };
    let said = match (mine.trim(), theirs.trim()) {
        ("", "") => return None,
        (mine, theirs) if mine == theirs => return None,
        ("", theirs) => format!("{theirs_game} {}.", has(theirs)),
        (mine, "") => format!("Your game {}.", has(mine)),
        (mine, theirs) => format!("{theirs_game} {}; yours {}.", has(theirs), has(mine)),
    };
    Some(format!("{said} Everyone in a match needs the same mods."))
}

/// Why searches found no lobbies, when the open ones have other game data (the server's
/// `other_data_mine`, `other_data_theirs`, named as for [`mismatch_line`]). The game shows
/// only lobbies whose data is the same as its own, and says nothing of the others. None when
/// the server didn't say (servers before 0.4.4).
pub fn search_mismatch_line(mine: &str, theirs: &str) -> Option<String> {
    let (mine, theirs) = (mine.trim(), theirs.trim());
    if theirs.is_empty() || mine == theirs {
        return None;
    }
    let lobbies = match theirs {
        "unmodified" => "The open lobbies were unmodified games".to_string(),
        hex if hex.starts_with("0x") => format!("The open lobbies had other game data ({hex})"),
        name => format!("The open lobbies had the {name}"),
    };
    let yours = match mine {
        "" => String::new(),
        "unmodified" => ", and yours is unmodified".to_string(),
        hex if hex.starts_with("0x") => format!(", and yours has other game data ({hex})"),
        name => format!(", and yours has the {name}"),
    };
    let fix = if mine == "unmodified" {
        "they need the same game files as you to show up"
    } else {
        "remove your mods (or verify the game files in Steam or Ubisoft Connect) to see them"
    };
    Some(format!("{lobbies}{yours}: the game only shows lobbies with the same game data, so {fix}."))
}

/// What a failed join ran into, as the server's `JoinOutcome` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinProblem {
    /// The lobby had no seat free at the size its host set.
    Full,
    /// A party of its own, or a match already under way.
    Locked,
    /// The room had just closed (its host left, or moved on).
    Closed,
    /// The two games' data differ (a mod).
    DataMismatch,
    /// The join itself failed: the host's game didn't answer, or let them in, in time.
    Failed,
}

/// The overlay's notice for a failed join: a title and a line. `host`, `mine` and `theirs`
/// as for [`mismatch_line`] (empty when not known). Whether it explains the failure, so the
/// player needn't report it, is [`JoinProblem::explained`].
pub fn join_notice(problem: JoinProblem, host: &str, mine: &str, theirs: &str) -> (String, String) {
    let (title, text) = match problem {
        JoinProblem::Full => (
            "Couldn't join: the lobby is full",
            "It's full at the size its host set. Try another, or again when a seat frees up.".to_string(),
        ),
        JoinProblem::Locked => (
            "Couldn't join: the session is locked",
            "It's a party of its own, or its match is already under way. Try another, or again once it's back in its lobby.".to_string(),
        ),
        JoinProblem::Closed => (
            "Couldn't join: that room had just closed",
            "Its host left or moved on. Search again, or ask them to invite you.".to_string(),
        ),
        JoinProblem::DataMismatch => (
            "Couldn't join: different game data",
            mismatch_line(host, mine, theirs).unwrap_or_else(|| "One of the two games has a mod the other doesn't. Everyone in a match needs the same mods.".to_string()),
        ),
        JoinProblem::Failed => (
            "Couldn't join",
            "The host's game didn't let you in in time. Try again; if it keeps happening, the launcher will ask how it went after you quit.".to_string(),
        ),
    };
    (title.to_string(), text)
}

impl JoinProblem {
    /// Whether the notice says all there is: the player needn't be asked to report it.
    pub fn explained(self) -> bool {
        self != Self::Failed
    }
}

/// The overlay's notice for a search that found nothing while lobbies with other game data
/// were open (`mine`, `theirs` as for [`search_mismatch_line`]). None when there's nothing
/// to say.
pub fn search_notice(mine: &str, theirs: &str) -> Option<(String, String)> {
    search_mismatch_line(mine, theirs).map(|line| ("No lobbies with your game data".to_string(), line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_join_says_why_it_failed() {
        let (title, text) = join_notice(JoinProblem::DataMismatch, "Theusma01", "balaclava mod", "unmodified");
        assert_eq!(title, "Couldn't join: different game data");
        assert_eq!(
            text,
            "Theusma01's game is unmodified; yours has the balaclava mod. Everyone in a match needs the same mods."
        );
        let (_, text) = join_notice(JoinProblem::DataMismatch, "", "", "");
        assert!(text.contains("same mods"), "not known: still why");
        for problem in [JoinProblem::Full, JoinProblem::Locked, JoinProblem::Closed] {
            assert!(problem.explained());
            assert!(join_notice(problem, "", "", "").0.starts_with("Couldn't join: "));
        }
        assert!(!JoinProblem::Failed.explained(), "a join that failed for no known reason is worth a report");
        assert_eq!(
            search_notice("unmodified", "balaclava mod").map(|n| n.0),
            Some("No lobbies with your game data".to_string())
        );
        assert_eq!(search_notice("", ""), None);
    }
}
