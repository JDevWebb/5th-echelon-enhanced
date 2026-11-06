//! Choosing your match (the 0.5.0 plan's L2): the open public matches on the player's server,
//! to join one they pick (the server's JoinRoom), or a brand-new public match of their own
//! (NewMatch: their searches find nothing until they host one, so the game hosts). For players
//! avoiding someone who sits in a public lobby. Servers that can list "choose-match" in
//! `/api/info`'s features.

use std::path::Path;
use std::time::Duration;

use setup::config::Config;

/// How long the list waits on the server.
const LIST_TIMEOUT: Duration = Duration::from_secs(4);

/// An open public match.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Row {
    /// Its room, to join it by.
    pub room: u32,
    /// "SvM Classic", "Co-op".
    pub mode: String,
    pub map: Option<String>,
    /// "6/8", or "6 players".
    pub players: String,
    /// Under way: its host's game turns joiners away until it's over.
    pub started: bool,
    /// Every seat taken.
    pub full: bool,
}

/// What's open on a server.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Matches {
    /// The server lets players pick a match or start a new one (a server before 0.5.0 doesn't).
    pub offered: bool,
    /// Whether the list could be read at all.
    pub answered: bool,
    pub rows: Vec<Row>,
    /// "2 more matches, 9 players · 1 private match · 3 in lobbies", or empty.
    pub others: String,
}

/// The open public matches on `server`, busiest first.
pub fn list(server: &str) -> Matches {
    let offered = crate::flow::offers(server, "choose-match");
    let Some(playing) = setup::server_info::playing(server, LIST_TIMEOUT) else {
        return Matches { offered, ..Matches::default() };
    };
    let rows = playing
        .matches
        .iter()
        .filter(|m| m.room != 0)
        .map(|m| Row {
            room: m.room,
            mode: m.mode_name().to_string(),
            map: m.map.and_then(server_api::game_names::map).map(String::from),
            players: if m.seats == 0 {
                format!("{} players", m.players)
            } else {
                format!("{}/{}", m.players, m.seats)
            },
            started: m.started,
            full: m.seats > 0 && m.players >= m.seats,
        })
        .collect();
    let mut others = Vec::new();
    if playing.more_matches > 0 {
        others.push(format!(
            "{} more {}, {} players",
            playing.more_matches,
            plural(playing.more_matches, "match", "matches"),
            playing.more_players
        ));
    }
    if playing.private_matches > 0 {
        others.push(format!("{} private {}", playing.private_matches, plural(playing.private_matches, "match", "matches")));
    }
    if playing.lobby_players > 0 {
        others.push(format!("{} in lobbies", playing.lobby_players));
    }
    Matches {
        offered,
        answered: true,
        rows,
        others: others.join(" · "),
    }
}

fn plural(n: u32, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// Joins the public match in `room`: the server queues its host's invitation, and the game
/// (running, or started for it) takes it without a click, as the join request beside it names
/// that host. A line for the player either way.
pub fn join(cfg: &Config, game_dir: &Path, room: u32) -> Result<String, String> {
    use server_api::friends::join_now_response::Status as JoinStatus;
    if !cfg.current_profile().is_some_and(|p| !p.server.is_empty() && p.has_account()) {
        return Err("Set up an account on a server first.".into());
    }
    let resp = crate::flow::with_api_token(cfg, |url, token| crate::services::rt().block_on(crate::network::join_room(url, &token, room)))?;
    match JoinStatus::try_from(resp.status).unwrap_or(JoinStatus::NotFound) {
        JoinStatus::Ok => {}
        _ if !resp.detail.is_empty() => return Err(hooks_config::text::clip(&resp.detail, 200)),
        JoinStatus::Started => return Err("That match has started - pick another, or start a new one.".into()),
        JoinStatus::NotFriends => return Err("You've blocked that match's host.".into()),
        _ => return Err("That match isn't open any more.".into()),
    }
    // The game takes the invitation as the join asked for only from the host the request names
    // (`hooks_config::join_request`); it reads the request when the invitation arrives, a poll
    // after this, so written now it's there in time.
    let request = game_dir.join(hooks_config::join_request::FILE);
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    if resp.host_id.is_empty() || std::fs::write(&request, hooks_config::join_request::contents(&resp.host_id, at)).is_err() {
        tracing::warn!("couldn't write the join request for room {room} ({})", request.display());
        return Ok("Joining the match - accept the invitation in the game.".into());
    }
    // What happens next, in plain words.
    Ok("Joining the match: the game goes straight in once it's online.".into())
}

/// Asks for a new match: the player's quick match searches find nothing until they host one,
/// so the game starts a fresh public match. How long it holds.
pub fn new_match(cfg: &Config) -> Result<Duration, String> {
    if !cfg.current_profile().is_some_and(|p| !p.server.is_empty() && p.has_account()) {
        return Err("Set up an account on a server first.".into());
    }
    crate::flow::with_api_token(cfg, |url, token| crate::services::rt().block_on(crate::network::new_match(url, &token, true)))
}

/// Asks the game about to start to start matchmaking by itself
/// (`hooks_config::new_match_request`): once in the lobby its quick start selects Quick match and
/// the mode, without a click. For a new match (the server asked first) it hosts; for a quick one
/// (`quick`) it joins an open match if there is one. `playlist`: the Spies vs Mercs mode chosen
/// ([`svm_modes`]); none for Co-op. False when the request couldn't be written (the player starts
/// matchmaking themselves).
pub fn ask_game_to_matchmake(game_dir: &std::path::Path, playlist: Option<u32>, quick: bool) -> bool {
    let request = game_dir.join(hooks_config::new_match_request::FILE);
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let written = std::fs::write(&request, hooks_config::new_match_request::contents(at, playlist, quick)).is_ok();
    if !written {
        tracing::warn!("couldn't write the matchmaking request ({})", request.display());
    }
    written
}

/// What the player is told when a quick match starts the game (`by_itself`: the game was asked
/// to start matchmaking).
pub fn quick_match_notice(by_itself: bool) -> &'static str {
    if by_itself {
        "The game starts and finds you a match by itself once it's online: an open one if there is one, else it hosts."
    } else {
        "The game starts in the lobby: start Quick match there."
    }
}

/// The Spies vs Mercs modes a new match can be hosted in: (playlist, name).
pub fn svm_modes() -> Vec<(u32, &'static str)> {
    server_api::game_names::svm_new_match_modes()
}

/// Takes back the game's matchmaking request ([`ask_game_to_matchmake`]), when it hasn't read it
/// yet.
pub fn forget_game_hosting(game_dir: &std::path::Path) {
    let _ = std::fs::remove_file(game_dir.join(hooks_config::new_match_request::FILE));
}

/// What the player is told once a new match is asked for, the game started (`hosts_itself`:
/// the game was asked to host by itself) or already running.
pub fn new_match_notice(running: bool, hosts_itself: bool, minutes: u64) -> String {
    if running {
        format!("Start matchmaking in the game: it hosts a new public match rather than join an open one (for the next {minutes} minutes).")
    } else if hosts_itself {
        "The game starts and hosts a new public match by itself once it's online.".into()
    } else {
        format!("The game starts in the lobby: start matchmaking there and it hosts a new public match (for the next {minutes} minutes).")
    }
}

/// Takes a new match back: searches find matches again.
pub fn cancel_new_match(cfg: &Config) -> Result<(), String> {
    crate::flow::with_api_token(cfg, |url, token| crate::services::rt().block_on(crate::network::new_match(url, &token, false))).map(|_| ())
}

/// The Matches tab (the 0.5.0 lobby browser): every public match on the player's server in
/// full, from the server's `Lobbies` (signed in; servers listing "lobbies").
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Lobbies {
    /// The server offers it (a server before 0.5.0 doesn't: the tab says so).
    pub offered: bool,
    /// Whether the list could be read at all, and why not.
    pub error: Option<String>,
    pub lobbies: Vec<Lobby>,
    /// "7 public · 31 players · 2 private matches · 6 in parties".
    pub summary: String,
    /// What's left out and why: "1 with a player you blocked".
    pub hidden: String,
}

/// One public match.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Lobby {
    pub room: u32,
    /// "SvM Classic", "Co-op".
    pub mode: String,
    /// "svm" or "coop".
    pub kind: String,
    pub map: Option<String>,
    pub players: usize,
    pub seats: u32,
    /// The players its mode is made for, when the host raised the seats past it.
    pub made_for: Option<u32>,
    pub host: String,
    /// "In lobby · waiting for 1 more", "Round 2 under way · 6 min", "Between rounds".
    pub status: String,
    /// "ok" (joinable), "warn" (under way, or full).
    pub tone: &'static str,
    pub under_way: bool,
    pub full: bool,
    pub joinable: bool,
    /// "4 min".
    pub open_for: String,
    /// How long it's been open, in seconds (newest first sorts by it).
    pub open_secs: u32,
    /// The player's estimated round trip to its host (None: unknown).
    pub ping_ms: Option<u32>,
    pub friends: Vec<String>,
    /// Sides known: the players split into spies and mercs.
    pub teams: bool,
    pub people: Vec<Person>,
    /// This player is in it already: nothing to join ("You're in this match").
    pub with_me: bool,
    /// The round's settings as the host's game has them: (label, value), in a set order.
    pub settings: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Person {
    pub name: String,
    pub host: bool,
    /// Who came together share a number (0: on their own).
    pub party: u32,
    pub friend: bool,
    /// "spy", "merc" or "".
    pub team: String,
    pub ping_ms: Option<u32>,
    /// This player.
    #[serde(default)]
    pub me: bool,
}

/// The public matches on the player's server in full.
pub fn lobbies(cfg: &Config) -> Lobbies {
    let Some(profile) = cfg.current_profile().filter(|p| !p.server.is_empty() && p.has_account()) else {
        return Lobbies {
            error: Some("Set up an account on a server first.".into()),
            ..Lobbies::default()
        };
    };
    if !crate::flow::offers(&profile.server, "lobbies") {
        return Lobbies::default();
    }
    match crate::flow::with_api_token(cfg, |url, token| crate::services::rt().block_on(crate::network::lobbies(url, &token))) {
        Ok(resp) => {
            // This player among each match's players: no Join on the match they're in (the
            // gamebox offered gbox5 'Join this match' on gbox1's, with gbox5 in it).
            let mut all = from_response(&resp);
            let me = profile.user.username.to_lowercase();
            for l in &mut all.lobbies {
                for p in &mut l.people {
                    p.me = p.name.to_lowercase() == me;
                }
                l.with_me = l.people.iter().any(|p| p.me);
            }
            all
        }
        Err(e) => Lobbies {
            offered: true,
            error: Some(e),
            ..Lobbies::default()
        },
    }
}

/// The Matches tab's list on any server: in full where the server offers it, else (a server
/// before 0.5.0) the open matches as it lists them, with no one named.
pub fn lobbies_or_open(cfg: &Config) -> Lobbies {
    let got = lobbies(cfg);
    if got.offered || got.error.is_some() {
        return got;
    }
    let Some(server) = cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty()) else {
        return got;
    };
    let open = list(&server);
    if !open.answered {
        return Lobbies {
            error: Some("Your server didn't answer.".into()),
            ..Lobbies::default()
        };
    }
    let lobbies = open
        .rows
        .iter()
        .map(|r| {
            let (players, seats) = match r.players.split_once('/') {
                Some((n, of)) => (n.parse().unwrap_or(0), of.parse().unwrap_or(0)),
                None => (r.players.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0), 0),
            };
            Lobby {
                room: r.room,
                kind: if r.mode.to_ascii_lowercase().contains("co-op") || r.mode.to_ascii_lowercase().contains("coop") {
                    "coop"
                } else {
                    "svm"
                }
                .into(),
                mode: r.mode.clone(),
                map: r.map.clone(),
                players,
                seats,
                status: if r.started { "Under way" } else { "In lobby" }.into(),
                tone: if r.started || r.full { "warn" } else { "ok" },
                under_way: r.started,
                full: r.full,
                joinable: !r.started && !r.full,
                ..Lobby::default()
            }
        })
        .collect();
    Lobbies {
        offered: false,
        error: None,
        lobbies,
        summary: open.others,
        hidden: String::new(),
    }
}

/// The server's answer, worded for the tab.
pub fn from_response(resp: &server_api::friends::LobbiesResponse) -> Lobbies {
    let clip = |s: &str| hooks_config::text::clip(s, 32);
    let lobbies: Vec<Lobby> = resp.lobbies.iter().map(|l| lobby(l, clip)).collect();
    let players: usize = lobbies.iter().map(|l| l.players).sum();
    let mut summary = vec![
        format!("{} public", lobbies.len()),
        format!("{players} {}", if players == 1 { "player" } else { "players" }),
    ];
    if resp.private_matches > 0 {
        summary.push(format!("{} private {}", resp.private_matches, plural(resp.private_matches, "match", "matches")));
    }
    if resp.party_players > 0 {
        summary.push(format!("{} in parties", resp.party_players));
    }
    let hidden = match resp.hidden_blocked {
        0 => String::new(),
        n => format!("{n} more left out: with a player you blocked."),
    };
    Lobbies {
        offered: true,
        error: None,
        lobbies,
        summary: summary.join(" · "),
        hidden,
    }
}

fn lobby(l: &server_api::friends::Lobby, clip: impl Fn(&str) -> String) -> Lobby {
    let mode = server_api::game_names::match_mode(Some(l.game_mode).filter(|m| *m != 0), &l.kind)
        .unwrap_or("A match")
        .to_string();
    let players = l.players.len();
    let full = l.seats > 0 && players as u32 >= l.seats;
    let people: Vec<Person> = l
        .players
        .iter()
        .map(|p| Person {
            name: clip(&p.name),
            host: p.host,
            party: p.party,
            friend: p.friend,
            team: if matches!(p.team.as_str(), "spy" | "merc") { p.team.clone() } else { String::new() },
            ping_ms: (p.ping_ms > 0).then_some(p.ping_ms),
            me: false,
        })
        .collect();
    Lobby {
        with_me: false,
        room: l.room,
        mode,
        kind: l.kind.clone(),
        map: (l.map != 0).then(|| server_api::game_names::map(l.map)).flatten().map(String::from),
        players,
        seats: l.seats,
        made_for: (l.mode_size > 0 && l.seats > l.mode_size).then_some(l.mode_size),
        host: people.iter().find(|p| p.host).map(|p| p.name.clone()).unwrap_or_default(),
        status: status(l, players),
        tone: if l.under_way || full { "warn" } else { "ok" },
        under_way: l.under_way,
        full,
        joinable: !l.under_way && !full,
        open_for: minutes(l.open_secs),
        open_secs: l.open_secs,
        ping_ms: (l.ping_ms > 0).then_some(l.ping_ms),
        friends: people.iter().filter(|p| p.friend).map(|p| p.name.clone()).collect(),
        teams: !people.is_empty() && people.iter().all(|p| !p.team.is_empty()),
        settings: settings(&l.settings),
        people,
    }
}

/// Example matches, for the screens' tests and screenshots.
#[cfg(test)]
pub fn sample() -> Lobbies {
    let person = |name: &str, host: bool, friend: bool, party: u32, ping: u32| Person {
        name: name.into(),
        host,
        party,
        friend,
        team: String::new(),
        ping_ms: Some(ping),
        me: false,
    };
    Lobbies {
        offered: true,
        summary: "3 public · 9 players · 1 private match".into(),
        lobbies: vec![
            Lobby {
                room: 1,
                mode: "SvM Classic".into(),
                kind: "svm".into(),
                map: Some("Penthouse".into()),
                players: 3,
                seats: 4,
                host: "Tank".into(),
                status: "In lobby · waiting for 1 more".into(),
                tone: "ok",
                joinable: true,
                open_for: "4 min".into(),
                open_secs: 240,
                ping_ms: Some(48),
                friends: vec!["Raven".into()],
                people: vec![
                    person("Tank", true, false, 0, 48),
                    person("Raven", false, true, 0, 32),
                    person("Nova", false, false, 0, 120),
                ],
                settings: vec![("Rounds".into(), "3".into()), ("Time".into(), "10 min".into())],
                ..Lobby::default()
            },
            Lobby {
                room: 2,
                mode: "Co-op".into(),
                kind: "coop".into(),
                map: Some("Mine".into()),
                players: 2,
                seats: 2,
                host: "Ghost".into(),
                status: "Under way · 6 min".into(),
                tone: "warn",
                under_way: true,
                full: true,
                open_for: "9 min".into(),
                open_secs: 540,
                ping_ms: Some(180),
                people: vec![person("Ghost", true, false, 1, 180), person("Kiwi", false, false, 1, 190)],
                ..Lobby::default()
            },
            Lobby {
                room: 3,
                mode: "SvM Blacklist".into(),
                kind: "svm".into(),
                map: Some("Estate".into()),
                players: 4,
                seats: 8,
                made_for: Some(4),
                host: "Echo".into(),
                status: "In lobby".into(),
                tone: "ok",
                joinable: true,
                open_for: "1 min".into(),
                open_secs: 60,
                ping_ms: Some(95),
                people: vec![
                    person("Echo", true, false, 0, 95),
                    person("Vex", false, false, 0, 90),
                    person("Rook", false, false, 0, 99),
                    person("Moss", false, false, 0, 101),
                ],
                ..Lobby::default()
            },
        ],
        ..Lobbies::default()
    }
}

/// "45 s", "4 min", "1 h 10 min".
fn minutes(secs: u32) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

/// What the match is doing, as its host's game said it.
fn status(l: &server_api::friends::Lobby, players: usize) -> String {
    let lasted = |what: String| {
        if l.under_way_secs > 0 {
            format!("{what} · {}", minutes(l.under_way_secs))
        } else {
            what
        }
    };
    let round = |what: &str| if l.round > 0 { format!("Round {} {what}", l.round) } else { format!("Round {what}") };
    match l.state.as_str() {
        "RoundState_Started" => lasted(round("under way")),
        "RoundState_PreRound" => round("starting"),
        "RoundState_PreEnded" => round("ending"),
        "RoundState_Ended" => "Between rounds".into(),
        "RoundState_SuddenDeath" => lasted("Sudden death".into()),
        "RoundState_PostMatch" | "CoopMissionState_Results" => "Results".into(),
        "CoopMissionState_InGame" => lasted("Mission under way".into()),
        _ if l.under_way => lasted("Under way".into()),
        _ => {
            let size = if l.mode_size > 0 { l.mode_size } else { l.seats } as usize;
            if size > players {
                let more = size - players;
                format!("In lobby · waiting for {more} more")
            } else {
                "In lobby".into()
            }
        }
    }
}

/// The round's settings in words, in a set order; the ones the host's game didn't say left out.
fn settings(said: &std::collections::HashMap<String, f32>) -> Vec<(String, String)> {
    const NAMES: [(&str, &str, bool); 9] = [
        ("m_fCurrentRoundTimeLimit", "Time limit", true),
        ("m_iScoreLimit", "Score limit", false),
        ("m_iSuddenDeathScoreLimit", "Sudden death limit", false),
        ("m_fRespawnTimeSpyTeam", "Spies' respawn", true),
        ("m_fRespawnTimeMercTeam", "Mercs' respawn", true),
        ("m_fRespawnTimeDefendingTeam", "Defenders' respawn", true),
        ("m_fRespawnTimeCourierTeam", "Couriers' respawn", true),
        ("m_fGenericRespawnTime", "Respawn", true),
        ("m_fAutoRespawnTime", "Auto respawn", true),
    ];
    NAMES
        .iter()
        .filter_map(|(key, label, time)| {
            let v = *said.get(*key)?;
            if !v.is_finite() || v < 0.0 {
                return None;
            }
            let text = if *time {
                let s = v.round() as u32;
                if s >= 60 && s % 60 == 0 {
                    format!("{} min", s / 60)
                } else {
                    format!("{s} s")
                }
            } else {
                format!("{}", v.round() as i64)
            };
            Some(((*label).to_string(), text))
        })
        .collect()
}

#[cfg(test)]
mod lobby_tests {
    use server_api::friends::LobbyPlayer;

    use super::*;

    /// The game's quick start knows the mode list by its playlists: the same ones the server
    /// names matches by, and every mode the menus offer is one of them.
    #[test]
    fn the_quick_start_knows_every_playlist() {
        let names: Vec<u32> = server_api::game_names::PLAYLISTS.iter().map(|(p, _)| *p).collect();
        assert_eq!(hooks_config::navigation::SVM_PLAYLISTS, names.as_slice());
        for (playlist, _) in svm_modes() {
            assert!(hooks_config::navigation::is_mode_list(&[playlist]));
        }
    }

    fn player(name: &str, host: bool, party: u32, friend: bool, team: &str) -> LobbyPlayer {
        LobbyPlayer {
            name: name.into(),
            host,
            party,
            friend,
            team: team.into(),
            ping_ms: 40,
        }
    }

    /// A lobby waiting for players and a round under way read as the mock-up has them.
    #[test]
    fn lobbies_are_worded_for_the_tab() {
        let resp = server_api::friends::LobbiesResponse {
            lobbies: vec![
                server_api::friends::Lobby {
                    room: 10,
                    kind: "svm".into(),
                    game_mode: 7,
                    map: 0x05F6_9010,
                    seats: 8,
                    mode_size: 8,
                    open_secs: 245,
                    ping_ms: 38,
                    players: vec![player("Liara", true, 0, false, ""), player("Tank", false, 0, true, ""), player("mpw", false, 1, false, "")],
                    ..Default::default()
                },
                server_api::friends::Lobby {
                    room: 11,
                    kind: "svm".into(),
                    game_mode: 8,
                    seats: 4,
                    mode_size: 4,
                    state: "RoundState_Started".into(),
                    under_way: true,
                    under_way_secs: 360,
                    round: 2,
                    settings: [("m_fCurrentRoundTimeLimit".to_string(), 600.0), ("m_iScoreLimit".to_string(), 3.0)].into_iter().collect(),
                    players: vec![player("Ma1orenko", true, 0, false, "spy"), player("Tea_bag", false, 0, false, "merc")],
                    ..Default::default()
                },
            ],
            hidden_blocked: 1,
            private_matches: 2,
            party_players: 6,
            ..Default::default()
        };
        let view = from_response(&resp);
        assert_eq!(view.summary, "2 public · 5 players · 2 private matches · 6 in parties");
        assert_eq!(view.hidden, "1 more left out: with a player you blocked.");
        let (a, b) = (&view.lobbies[0], &view.lobbies[1]);
        assert_eq!((a.mode.as_str(), a.map.as_deref(), a.host.as_str()), ("SvM Blacklist", Some("Virus Vault"), "Liara"));
        assert_eq!(
            (a.status.as_str(), a.tone, a.joinable, a.open_for.as_str()),
            ("In lobby · waiting for 5 more", "ok", true, "4 min")
        );
        assert_eq!(a.friends, vec!["Tank"]);
        assert!(!a.teams);
        assert_eq!((b.status.as_str(), b.tone, b.joinable), ("Round 2 under way · 6 min", "warn", false));
        assert!(b.teams);
        assert_eq!(
            b.settings,
            vec![("Time limit".to_string(), "10 min".to_string()), ("Score limit".to_string(), "3".to_string())]
        );
    }
}
