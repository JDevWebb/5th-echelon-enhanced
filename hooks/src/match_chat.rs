//! The game's in-match chat, as this player sees it (0.5.6's TK2 and TK3).
//!
//! The chat goes peer to peer through the host and never reaches the server, so the server can't
//! moderate it. Here, at the match manager's chat display (the detour `chat_relay` installs), a
//! line isn't shown when:
//! - its sender is on the player's block list;
//! - the player muted them in this match (the overlay's Match tab; mutes end with the match);
//! - or the player's setting says team only (a line for everyone isn't shown) or off
//!   (`InMatchChat`).
//!
//! The host's game still passes a line it doesn't show on to the others (`chat_relay`): one
//! player's choice is theirs only.
//!
//! And the last [`KEPT`] lines that reached this player, for 30 minutes (lines to everyone and their
//! own team's: never another team's team lines, which reach every game but show only to that
//! team; a muted or hidden player's lines too, as evidence), go with a report the player makes
//! from the overlay: the reported player's lines only. Nothing is sent
//! otherwise.
//!
//! A ChatString message: the sender's name (a string at `+0x1c`), the line (`+0x38`), the
//! sender's team (`+0x54`) and team-only (`+0x64`). A string is `{.., .., buffer}`: the buffer is
//! `{references, length, capacity, UTF-16 characters}` (the game's `0x4195e0`).

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Duration;
use std::time::Instant;

use hooks_config::InMatchChat;

/// The lines kept for reports.
pub const KEPT: usize = 50;
/// How long a line is kept.
const KEPT_FOR: Duration = Duration::from_secs(30 * 60);
/// The longest name and line read (the game's own line limit is 0x90 characters).
const MOST_NAME: usize = 64;
const MOST_TEXT: usize = 0x90;

const NAME: usize = 0x1c;
const TEXT: usize = 0x38;
const TEAM: usize = 0x54;
const TEAM_ONLY: usize = 0x64;

/// A line this game showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub at: Instant,
    /// Unix seconds, for the report.
    pub unix: i64,
    pub name: String,
    pub team_only: bool,
    pub text: String,
}

static LINES: Mutex<VecDeque<Line>> = Mutex::new(VecDeque::new());
/// Players muted in this match, by name (lowercase).
static MUTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SETTING: Mutex<InMatchChat> = Mutex::new(InMatchChat::All);

/// From the config, at start.
pub fn init(setting: InMatchChat) {
    *SETTING.lock().unwrap_or_else(PoisonError::into_inner) = setting;
    #[cfg(feature = "test-commands")]
    let _ = std::thread::Builder::new().name("fe-chat-test".into()).spawn(test_commands);
}

/// The gamebox's stand-ins (a test build only), in `gamebox-command`: `mute <name>` /
/// `unmute <name>` (the overlay's Mute), `chatmode all|team|off` (Settings › Game),
/// `peername <n> <name>` (the server's name for number n: a stand-in for a spoofed name, N1), and
/// `chatlines <name>` (logs how many of their lines a report would carry, and the first).
#[cfg(feature = "test-commands")]
fn test_commands() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Some(file) = crate::LOG_PATH.get().and_then(|p| p.parent()).map(|d| d.join("gamebox-command")) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let text = text.trim().to_string();
        let mut words = text.splitn(3, ' ');
        let (Some(verb), arg) = (words.next(), words.next().unwrap_or_default()) else { continue };
        let rest = words.next().unwrap_or_default();
        match verb {
            "mute" | "unmute" => set_muted(arg, verb == "mute"),
            "chatmode" => {
                let mode = match arg {
                    "team" => InMatchChat::Team,
                    "off" => InMatchChat::Off,
                    _ => InMatchChat::All,
                };
                *SETTING.lock().unwrap_or_else(PoisonError::into_inner) = mode;
            }
            "findtext" => {
                let text = format!("{arg} {rest}").trim().to_string();
                let _ = std::thread::Builder::new().name("fe-find-text".into()).spawn(move || crate::find_text::find(&text));
            }
            "peers" => crate::net_names::log_all(),
            "netname" => {
                let Ok(n) = arg.parse::<i32>() else { continue };
                crate::net_names::test_rename(n - 1, rest.trim());
            }
            "selfname" => {
                *crate::pawns::SELF_NAME_TEST.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = (!arg.is_empty()).then(|| arg.to_string());
                tracing::info!("Match chat: test: the account's name stands in as {arg:?}");
            }
            "peername" => {
                let Ok(n) = arg.parse::<u32>() else { continue };
                crate::pawns::set_peer_names(vec![(n, rest.to_string())]);
            }
            "report" => {
                let Some((id, name, _)) = crate::community::snapshot().match_players.into_iter().find(|(_, n, _)| n.eq_ignore_ascii_case(arg)) else {
                    crate::community::match_players();
                    tracing::info!("Match chat: test: {arg} isn't in the match players yet (asked again)");
                    continue;
                };
                crate::community::report_player(id, name, "other", String::from("gamebox test"));
            }
            "chatlines" => {
                let lines = lines_of(arg);
                tracing::info!("Match chat: test: {} lines of {arg} kept; first {:?}", lines.len(), lines.first());
            }
            _ => continue,
        }
        let _ = std::fs::remove_file(&file);
        tracing::info!("Match chat: test: {text}");
    }
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Mutes or unmutes `name` for this match.
pub fn set_muted(name: &str, muted: bool) {
    let mut list = MUTED.lock().unwrap_or_else(PoisonError::into_inner);
    let k = key(name);
    list.retain(|n| *n != k);
    if muted && list.len() < 64 {
        list.push(k);
    }
}

pub fn is_muted(name: &str) -> bool {
    MUTED.lock().unwrap_or_else(PoisonError::into_inner).contains(&key(name))
}

/// The match ended or the player left it: mutes end with it.
pub fn match_left() {
    MUTED.lock().unwrap_or_else(PoisonError::into_inner).clear();
    SAID_NAMES.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

/// Why a line isn't shown, if it isn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    Blocked,
    Muted,
    Setting,
}

/// Whether a line from `name` (to everyone, or to the team) is shown, given the block list.
pub fn hidden(setting: InMatchChat, name: &str, team_only: bool, blocked: &[String], muted: bool) -> Option<Hidden> {
    let k = key(name);
    if !k.is_empty() && blocked.iter().any(|b| key(b) == k) {
        return Some(Hidden::Blocked);
    }
    if muted {
        return Some(Hidden::Muted);
    }
    match setting {
        InMatchChat::All => None,
        InMatchChat::Team if team_only => None,
        InMatchChat::Team | InMatchChat::Off => Some(Hidden::Setting),
    }
}

fn readable(at: usize, len: usize) -> bool {
    at >= 0x1_0000 && unsafe { crate::crash::readable(at, len) }
}

fn word(at: usize) -> Option<u32> {
    // SAFETY: checked readable just before.
    readable(at, 4).then(|| unsafe { std::ptr::read_unaligned(at as *const u32) })
}

fn byte(at: usize) -> Option<u8> {
    // SAFETY: checked readable just before.
    readable(at, 1).then(|| unsafe { std::ptr::read(at as *const u8) })
}

/// The text of the game's string at `at` (its buffer's UTF-16 characters), at most `max`.
fn string(at: usize, max: usize) -> Option<String> {
    let buffer = word(at + 8).filter(|b| *b != 0)? as usize;
    let len = (word(buffer + 4)? as usize).min(max);
    if !readable(buffer + 0xc, len * 2) {
        return None;
    }
    // SAFETY: checked readable just before.
    let units = unsafe { std::slice::from_raw_parts((buffer + 0xc) as *const u16, len) };
    Some(String::from_utf16_lossy(units).trim_end_matches('\0').to_string())
}

/// A received chat message's sender, line and whether it's for the team only.
fn read(message: usize) -> Option<(String, String, bool, u8)> {
    let name = string(message + NAME, MOST_NAME)?;
    // The game's line is the name's ": " and the text (the gamebox: ": hello").
    let text = string(message + TEXT, MOST_TEXT).unwrap_or_default();
    let text = text.strip_prefix(": ").map(str::to_string).unwrap_or(text);
    Some((name, text, byte(message + TEAM_ONLY)? != 0, byte(message + TEAM)?))
}

/// A message's sender: their number in the session, from 0 (the host).
const SENDER: usize = 0x04;

/// Names already said to the server this match: (number, name shown).
static SAID_NAMES: Mutex<Vec<(u8, String)>> = Mutex::new(Vec::new());

/// N1: on the host, whose game has each line from its sender, a line under another name than
/// the server has for the sender's number is said (once a match a name).
fn check_name(message: usize, shown: &str) {
    let Some(sender) = word(message + SENDER).filter(|s| *s < 32) else { return };
    let peer = (sender + 1) as u8;
    let Some(real) = crate::pawns::peer_name(peer) else { return };
    if real.trim().eq_ignore_ascii_case(shown.trim()) || shown.trim().is_empty() {
        return;
    }
    {
        let mut said = SAID_NAMES.lock().unwrap_or_else(PoisonError::into_inner);
        let item = (peer, shown.trim().to_lowercase());
        if said.contains(&item) || said.len() >= 64 {
            return;
        }
        said.push(item);
    }
    tracing::warn!("Match chat: number {peer} ({real} to the server) writes as another name");
    let shown = shown.to_string();
    let _ = std::thread::Builder::new().name("fe-name-seen".into()).spawn(move || crate::api::name_seen(u32::from(peer), &shown, 0));
}

/// At the chat display, before the game shows a line: whether it shows it. A line shown is kept
/// for reports. A message that can't be read is shown, as the game would. `host`: this game hosts
/// the match (it has each line from its sender).
pub fn on_display(message: usize, host: bool) -> bool {
    let Some((name, text, team_only, team)) = std::panic::catch_unwind(|| read(message)).ok().flatten() else {
        return true;
    };
    if host {
        check_name(message, &name);
    }
    // Kept for a report whatever this player chose to hide (a muted player's lines are the
    // evidence), but only lines the game would show them: a team line reaches every game and
    // each shows it only to the sender's team, so another team's is never kept (nor when this
    // player's team isn't known yet).
    let theirs = !team_only || crate::pawns::own_team().is_some_and(|own| own == i32::from(team));
    if theirs {
        keep(Line {
            at: Instant::now(),
            unix: identity_now(),
            name: hooks_config::text::clip(&name, MOST_NAME),
            team_only,
            text: hooks_config::text::clip(&text, MOST_TEXT),
        });
    }
    let blocked: Vec<String> = crate::community::snapshot().blocked.into_iter().map(|p| p.name).collect();
    let setting = *SETTING.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(why) = hidden(setting, &name, team_only, &blocked, is_muted(&name)) {
        tracing::info!("Match chat: a line not shown ({why:?})");
        return false;
    }
    true
}

fn identity_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn keep(line: Line) {
    let mut lines = LINES.lock().unwrap_or_else(PoisonError::into_inner);
    lines.retain(|l| l.at.elapsed() < KEPT_FOR);
    while lines.len() >= KEPT {
        lines.pop_front();
    }
    lines.push_back(line);
}

/// The lines from `name` this game showed lately, for a report: "HH:MM:SS [ALL|TEAM] text".
pub fn lines_of(name: &str) -> Vec<String> {
    let k = key(name);
    let lines = LINES.lock().unwrap_or_else(PoisonError::into_inner);
    lines_from(lines.iter(), &k)
}

fn lines_from<'a>(lines: impl Iterator<Item = &'a Line>, k: &str) -> Vec<String> {
    lines
        .filter(|l| l.at.elapsed() < KEPT_FOR && key(&l.name) == k)
        .map(|l| {
            let t = l.unix.rem_euclid(86_400);
            format!("{:02}:{:02}:{:02} UTC [{}] {}", t / 3600, t / 60 % 60, t % 60, if l.team_only { "TEAM" } else { "ALL" }, l.text)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_muted_and_the_setting_hide_lines() {
        let blocked = vec!["LeviChuKing".to_string()];
        assert_eq!(hidden(InMatchChat::All, "levichuking", false, &blocked, false), Some(Hidden::Blocked), "by name, any case");
        assert_eq!(hidden(InMatchChat::All, "Tea_bag", false, &blocked, true), Some(Hidden::Muted));
        assert_eq!(hidden(InMatchChat::All, "Tea_bag", false, &blocked, false), None);
        assert_eq!(hidden(InMatchChat::Team, "Tea_bag", true, &blocked, false), None, "team lines on team only");
        assert_eq!(hidden(InMatchChat::Team, "Tea_bag", false, &blocked, false), Some(Hidden::Setting));
        assert_eq!(hidden(InMatchChat::Off, "Tea_bag", true, &blocked, false), Some(Hidden::Setting));
        assert_eq!(hidden(InMatchChat::All, "", false, &[String::new()], false), None, "no name matches no one");
    }

    #[test]
    fn mutes_end_with_the_match() {
        set_muted("Gexiii", true);
        assert!(is_muted("GEXIII"));
        set_muted("Gexiii", false);
        assert!(!is_muted("Gexiii"));
        set_muted("Gexiii", true);
        match_left();
        assert!(!is_muted("Gexiii"));
    }

    #[test]
    fn a_reports_lines_are_the_reported_players_only() {
        let at = Instant::now();
        let lines = [
            Line { at, unix: 3600 * 17 + 60 * 20 + 5, name: "LeviChuKing".into(), team_only: true, text: "kill them".into() },
            Line { at, unix: 3600 * 17 + 60 * 21, name: "Tea_bag".into(), team_only: false, text: "stop".into() },
            Line { at, unix: 3600 * 17 + 60 * 22, name: "levichuking".into(), team_only: false, text: "lol".into() },
        ];
        assert_eq!(lines_from(lines.iter(), &key("LeviChuKing")), vec!["17:20:05 UTC [TEAM] kill them", "17:22:00 UTC [ALL] lol"]);
    }
}
