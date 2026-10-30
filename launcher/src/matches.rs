//! Choosing your match (the 0.4.8 plan's L2): the open public matches on the player's server,
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
    /// The server lets players pick a match or start a new one (a server before 0.4.8 doesn't).
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
    Ok("Joining the match - the game joins it.".into())
}

/// Asks for a new match: the player's quick match searches find nothing until they host one,
/// so the game starts a fresh public match. How long it holds.
pub fn new_match(cfg: &Config) -> Result<Duration, String> {
    if !cfg.current_profile().is_some_and(|p| !p.server.is_empty() && p.has_account()) {
        return Err("Set up an account on a server first.".into());
    }
    crate::flow::with_api_token(cfg, |url, token| crate::services::rt().block_on(crate::network::new_match(url, &token, true)))
}

/// Takes a new match back: searches find matches again.
pub fn cancel_new_match(cfg: &Config) -> Result<(), String> {
    crate::flow::with_api_token(cfg, |url, token| crate::services::rt().block_on(crate::network::new_match(url, &token, false))).map(|_| ())
}
