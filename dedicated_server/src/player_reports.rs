//! Reporting a player (the 0.5.0 plan's F5): from the overlay's Match tab, a player reports
//! someone in their match, or in one they left lately, for cheating, griefing, their name or
//! something else. The report goes the way feedback goes (`reports.rs`): to the network's
//! admins with the server's log of the reporter's session, and the match's room, and it's a
//! session event (`player_report`) on the Sessions page.
//!
//! Only players they shared a room with lately can be reported: [`seen_with`] keeps who was in
//! each player's rooms for [`RECENT`] (memory only, as each answer to [`players`] finds them).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use crate::storage::Storage;
use crate::storage::UrlsFor;

/// How long a player can report someone they no longer share a room with.
pub const RECENT: Duration = Duration::from_secs(30 * 60);
/// The same player reported again by the same reporter only after this.
const AGAIN_AFTER: Duration = Duration::from_secs(6 * 3600);
/// The most players kept a reporter, and reporters kept.
const MOST_SEEN: usize = 64;
const MOST_REPORTERS: usize = 20_000;
/// The reasons a report gives.
pub const REASONS: [&str; 4] = ["cheating", "griefing", "name", "other"];
const MOST_TEXT: usize = 500;

/// Who a reporter was in a room with lately: their id, name, the room and when last seen.
fn seen() -> std::sync::MutexGuard<'static, HashMap<u32, Vec<(u32, String, u32, Instant)>>> {
    static SEEN: std::sync::LazyLock<Mutex<HashMap<u32, Vec<(u32, String, u32, Instant)>>>> = std::sync::LazyLock::new(Mutex::default);
    SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// When each reporter last reported each player.
fn reported() -> std::sync::MutexGuard<'static, HashMap<(u32, u32), Instant>> {
    static REPORTED: std::sync::LazyLock<Mutex<HashMap<(u32, u32), Instant>>> = std::sync::LazyLock::new(Mutex::default);
    REPORTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Notes that `user` is in `room` with these players (id, name).
fn note(user: u32, room: u32, players: impl IntoIterator<Item = (u32, String)>) {
    let now = Instant::now();
    let mut all = seen();
    if all.len() >= MOST_REPORTERS && !all.contains_key(&user) {
        all.retain(|_, list| list.iter().any(|(_, _, _, at)| at.elapsed() < RECENT));
        if all.len() >= MOST_REPORTERS {
            return;
        }
    }
    let list = all.entry(user).or_default();
    for (id, name) in players {
        list.retain(|(other, ..)| *other != id);
        list.push((id, name, room, now));
    }
    list.retain(|(.., at)| at.elapsed() < RECENT);
    let extra = list.len().saturating_sub(MOST_SEEN);
    list.drain(..extra);
}

/// Who `user` shared a room with lately and isn't in one with now: id, name, room.
pub fn recent(user: u32) -> Vec<(u32, String, u32)> {
    seen()
        .get(&user)
        .map(|list| {
            list.iter()
                .filter(|(.., at)| at.elapsed() < RECENT)
                .map(|(id, name, room, _)| (*id, name.clone(), *room))
                .collect()
        })
        .unwrap_or_default()
}

/// The players in `user`'s rooms now (hosting or in them), then those of the last
/// [`RECENT`]: id, name, whether now.
pub async fn players(storage: &Storage, user: u32) -> eyre::Result<Vec<(u32, String, bool)>> {
    let mut rooms = storage.find_host_sessions_async(user).await?;
    rooms.extend(storage.member_sessions_async(user).await?);
    let rooms = storage.with_participants_async(rooms, UrlsFor::AllBut(user)).await?;
    let mut now: Vec<(u32, String)> = Vec::new();
    for room in &rooms {
        let others: Vec<(u32, String)> = room.participants.iter().filter(|p| p.user_id != user).map(|p| (p.user_id, p.name.clone())).collect();
        note(user, room.session_id, others.clone());
        now.extend(others);
    }
    now.sort();
    now.dedup_by_key(|(id, _)| *id);
    let mut out: Vec<(u32, String, bool)> = now.iter().map(|(id, name)| (*id, name.clone(), true)).collect();
    for (id, name, _) in recent(user) {
        if !out.iter().any(|(o, ..)| *o == id) {
            out.push((id, name, false));
        }
    }
    out.truncate(MOST_SEEN);
    Ok(out)
}

/// Why a report wasn't taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not someone they played with lately.
    NotSeen,
    BadReason,
    /// They reported this player a little while ago.
    Again,
    /// As many reports today as feedback allows, or the outbox is full.
    TooMany,
}

impl Refused {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotSeen => "You can report players you're in a match with, or were in the last 30 minutes",
            Self::BadReason => "Choose why: cheating, griefing, their name or other",
            Self::Again => "You reported this player a little while ago; the admins have it",
            Self::TooMany => "Enough reports for today; the admins have the ones you sent",
        }
    }
}

/// `user` reports player `target` for `reason`, in their words `text`: the report's id.
pub async fn report(
    storage: &Storage,
    user: u32,
    peer: Option<std::net::IpAddr>,
    target: u32,
    reason: &str,
    text: &str,
    chat_lines: &[String],
) -> eyre::Result<Result<String, Refused>> {
    if !REASONS.contains(&reason) {
        return Ok(Err(Refused::BadReason));
    }
    // Refreshed first: a player who just joined their room counts.
    let _ = players(storage, user).await?;
    let Some((_, name, room)) = recent(user).into_iter().find(|(id, ..)| *id == target) else {
        return Ok(Err(Refused::NotSeen));
    };
    // Marked before it's filed, so two at once can't both pass; taken back if it isn't filed.
    {
        let mut all = reported();
        if all.get(&(user, target)).is_some_and(|at| at.elapsed() < AGAIN_AFTER) {
            return Ok(Err(Refused::Again));
        }
        if all.len() >= MOST_REPORTERS {
            all.retain(|_, at| at.elapsed() < AGAIN_AFTER);
        }
        all.insert((user, target), Instant::now());
    }
    let unmark = || {
        reported().remove(&(user, target));
    };
    let text: String = text.chars().filter(|c| !c.is_control() || *c == '\n').take(MOST_TEXT).collect();
    let text = text.trim();
    let incoming = crate::reports::Incoming {
        rating: String::new(),
        problems: vec![String::from("player")],
        comment: format!("Reported {name} for {reason}{}", if text.is_empty() { String::new() } else { format!(": {text}") }),
        triggers: vec![String::from("player_report")],
        client: HashMap::from([
            (String::from("reported_id"), target.to_string()),
            (String::from("reported_name"), name.clone()),
            (String::from("reason"), reason.to_string()),
            (String::from("room"), room.to_string()),
        ]),
        files: chat_file(&name, chat_lines).into_iter().collect(),
    };
    let id = match crate::reports::accept(storage, user, peer, incoming, false).await {
        Ok(Ok(id)) => id,
        Ok(Err(_)) => {
            unmark();
            return Ok(Err(Refused::TooMany));
        }
        Err(e) => {
            unmark();
            return Err(e);
        }
    };
    crate::federation::report_queued();
    crate::session_events::note(
        crate::session_events::Who::Id(user),
        "player_report",
        serde_json::json!({ "reported": target, "reported_name": name, "reason": reason, "room": room, "report": id }),
    );
    Ok(Ok(id))
}

/// The most chat lines a report carries, and each one's length.
const MOST_CHAT_LINES: usize = 50;
const MOST_CHAT_LINE: usize = 200;

/// The reported player's in-match chat lines the reporter's game showed (0.5.6's TK3), as a
/// report file (`chat-lines.txt`, gzip): the chat never reaches the server otherwise.
fn chat_file(name: &str, lines: &[String]) -> Option<(String, Vec<u8>, u64)> {
    use std::io::Write as _;
    let lines: Vec<String> = lines
        .iter()
        .take(MOST_CHAT_LINES)
        .map(|l| l.chars().filter(|c| !c.is_control()).take(MOST_CHAT_LINE).collect::<String>())
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let text = format!(
        "{name}'s lines in the in-match chat, as the reporter's game showed them (their sender name as the game gave it):\n{}\n",
        lines.join("\n")
    );
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(text.as_bytes()).ok()?;
    Some((String::from("chat-lines.txt"), gz.finish().ok()?, text.len() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_seen_lately_are_kept_newest_last() {
        note(800_001, 5, [(2, "Tank".to_string()), (3, "Nova".to_string())]);
        note(800_001, 6, [(2, "Tank".to_string())]);
        let r = recent(800_001);
        assert_eq!(r, vec![(3, "Nova".to_string(), 5), (2, "Tank".to_string(), 6)]);
        assert!(recent(800_002).is_empty());
    }

    /// TK3: the reported player's chat lines go as a file; none, no file.
    #[test]
    fn chat_lines_go_with_the_report_as_a_file() {
        use std::io::Read as _;
        assert!(chat_file("Levi", &[]).is_none());
        assert!(chat_file("Levi", &["  ".into()]).is_none());
        let (name, gz, size) = chat_file("Levi", &["17:20:05 UTC [TEAM] kill them\u{7}".into()]).unwrap();
        assert_eq!(name, "chat-lines.txt");
        let mut text = String::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut text).unwrap();
        assert!(text.ends_with("17:20:05 UTC [TEAM] kill them\n"), "{text:?}");
        assert_eq!(size, text.len() as u64);
    }
}
