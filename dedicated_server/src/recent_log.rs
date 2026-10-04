//! The server's last hour or so of log lines, kept in memory, so a player's problem report
//! (`reports.rs`) can carry what the server logged about them: picked out by their account
//! id, name and the addresses they connected from. The log itself (journald, files) isn't
//! read back: it may be anywhere, or rotated.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::OnceLock;

/// The most lines kept, and for how long.
const MAX_LINES: usize = 60_000;
const MAX_AGE_SECS: i64 = 2 * 3600;
/// The most of one line kept.
const MAX_LINE: usize = 600;

struct Line {
    at: i64,
    text: String,
}

fn lines() -> &'static Mutex<VecDeque<Line>> {
    static LINES: OnceLock<Mutex<VecDeque<Line>>> = OnceLock::new();
    LINES.get_or_init(|| Mutex::new(VecDeque::with_capacity(4096)))
}

/// A drain that keeps each record as one line: time, level, message and the
/// connection's client and pid.
pub struct Recent;

/// Picks the keys worth keeping out of a record's values.
#[derive(Default)]
struct Keys(String);

impl slog::Serializer for Keys {
    fn emit_arguments(&mut self, key: slog::Key, val: &std::fmt::Arguments) -> slog::Result {
        if matches!(key, "client" | "pid" | "user" | "service") {
            let _ = write!(self.0, " {key}={val}");
        }
        Ok(())
    }
}

impl slog::Drain for Recent {
    type Ok = ();
    type Err = slog::Never;

    fn log(&self, record: &slog::Record, values: &slog::OwnedKVList) -> Result<(), slog::Never> {
        use slog::KV as _;
        let mut keys = Keys::default();
        let _ = values.serialize(record, &mut keys);
        let _ = record.kv().serialize(record, &mut keys);
        let at = identity::now();
        let mut text = format!("{} {}{} {}", clock(at), record.level().as_short_str(), keys.0, record.msg());
        if text.len() > MAX_LINE {
            let mut end = MAX_LINE;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        let mut lines = lines().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while lines.len() >= MAX_LINES || lines.front().is_some_and(|l| at - l.at > MAX_AGE_SECS) {
            lines.pop_front();
        }
        lines.push_back(Line { at, text });
        Ok(())
    }
}

/// `HH:MM:SS` (UTC) of a Unix time.
fn clock(at: i64) -> String {
    let s = at.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// Whether `text` mentions `word` on its own (not inside a longer word or number).
fn mentions(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let is_part = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(is_part) && !after.is_some_and(is_part)
    })
}

/// The kept lines since `since` (Unix seconds) about a player: those mentioning their
/// account id, their name or one of `addresses`, up to `max_bytes` (the newest kept).
pub fn about(id: u32, name: &str, addresses: &[String], since: i64, max_bytes: usize) -> String {
    let id = id.to_string();
    let lowered = name.to_lowercase();
    let lines = lines().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let picked: Vec<&str> = lines
        .iter()
        .filter(|l| l.at >= since)
        .filter(|l| mentions(&l.text, &id) || mentions(&l.text.to_lowercase(), &lowered) || addresses.iter().any(|a| mentions(&l.text, a)))
        .map(|l| l.text.as_str())
        .collect();
    let mut out = Vec::new();
    let mut size = 0;
    for line in picked.iter().rev() {
        size += line.len() + 1;
        if size > max_bytes {
            break;
        }
        out.push(*line);
    }
    out.reverse();
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_players_lines_are_picked_out() {
        let logger = slog::Logger::root(slog::Drain::fuse(Recent), slog::o!());
        let conn = logger.new(slog::o!("client" => "82.174.182.136:3074", "pid" => 1011));
        slog::info!(conn, "Calling GameSessionProtocol.JoinSession");
        slog::warn!(logger, "Join failed: 1011 did not get into session 4 (type 1)");
        slog::info!(logger, "NAT helper: ijsman5530 at 82.174.182.136:13000 advertises it");
        slog::info!(logger, "Join failed: 10111 did not get in");
        slog::info!(logger, "user 1012 advertises session 3");
        let text = about(1011, "IJSMAN5530", &["82.174.182.136".into()], 0, 1 << 20);
        let found: Vec<&str> = text.lines().collect();
        assert_eq!(found.len(), 3, "{text}");
        assert!(found[0].contains("client=82.174.182.136:3074") && found[0].contains("pid=1011"));
        assert!(!text.contains("10111") && !text.contains("1012"));
        // The newest within the size given.
        assert!(about(1011, "x", &[], 0, 120).lines().all(|l| !l.contains("JoinSession")));
    }

    #[test]
    fn words_on_their_own() {
        assert!(mentions("user 1011 left", "1011"));
        assert!(!mentions("user 10111 left", "1011"));
        assert!(mentions("pid=1011", "1011"));
        assert!(!mentions("anything", ""));
    }
}
