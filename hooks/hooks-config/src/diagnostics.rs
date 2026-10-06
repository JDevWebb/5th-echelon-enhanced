//! Which of the game's log lines go to the server as its diagnostics (the hooks DLL's
//! `diagnostics.rs` sends them, `send_diagnostics`), and finding those lines in a log file
//! written before (the launcher shows a player an example of what's sent).

/// Modules whose ordinary (INFO) lines go too: the network code.
const NETWORK: &[&str] = &["hooks::hooks::nat", "hooks::hooks::portmap", "hooks::hooks::nla"];
/// What the game is doing: its session, saves, achievements and the overlay (`hooks::game_state`).
const GAME_STATE: &str = "hooks::game_state";

/// Whether a log line goes to the server: this client's warnings and errors, and the network
/// code's lines but its packet dumps (those are for the player's own packet logging).
/// `level` as the log writes it: ERROR, WARN, INFO, DEBUG or TRACE.
#[must_use]
pub fn wanted(level: &str, target: &str, message: &str) -> bool {
    if !target.starts_with("hooks") || target.starts_with("hooks::diagnostics") {
        return false;
    }
    match level {
        "ERROR" | "WARN" => true,
        "INFO" if target.starts_with(GAME_STATE) => true,
        "INFO" => NETWORK.iter().any(|m| target.starts_with(m)) && !message.starts_with("sendto ") && !message.starts_with("recvfrom "),
        _ => false,
    }
}

/// A line of the game's log (`bl-tracing.log`) taken apart: its level, target and message.
/// The log writes the time, the level, the thread, spans, the target with a colon, the source
/// file and line, then the message.
#[must_use]
pub fn parse_line(line: &str) -> Option<(&str, &str, &str)> {
    let mut rest = line;
    let mut level = None;
    // The level: the first of its words.
    while let Some((word, after)) = next_word(rest) {
        rest = after;
        if matches!(word, "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE") {
            level = Some(word);
            break;
        }
    }
    let level = level?;
    // The target: the first word ending in a colon that names a module of this client.
    loop {
        let (word, after) = next_word(rest)?;
        rest = after;
        if let Some(target) = word.strip_suffix(':').filter(|t| t.starts_with("hooks") && !t.contains('/') && !t.contains('\\')) {
            let mut message = rest.trim_start();
            // The source file and line, when the log has them.
            if let Some((file, after)) = message.split_once(": ") {
                if file.contains(".rs:") && !file.contains(' ') {
                    message = after;
                }
            }
            return Some((level, target, message.trim_end()));
        }
    }
}

fn next_word(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if text.is_empty() {
        return None;
    }
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    Some((&text[..end], &text[end..]))
}

/// The last `count` lines of a log that would have gone to the server, as
/// `LEVEL target: message` (not yet redacted).
#[must_use]
pub fn example(log: &str, count: usize) -> Vec<String> {
    let mut lines: Vec<String> = log
        .lines()
        .filter_map(parse_line)
        .filter(|(level, target, message)| wanted(level, target, message))
        .map(|(level, target, message)| format!("{level} {target}: {message}"))
        .collect();
    let skip = lines.len().saturating_sub(count);
    lines.drain(..skip);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_game_is_doing_goes_too() {
        assert!(wanted("INFO", "hooks::game_state", "Saved the game (422 KB): a checkpoint, or a mission's end"));
        assert!(!wanted("INFO", "hooks::uplay_r1_loader::save", "UPLAY_SAVE_Write"), "not the calls themselves");
        assert!(!wanted("DEBUG", "hooks::game_state", "anything"));
    }

    #[test]
    fn warnings_errors_and_network_lines_go_packet_dumps_dont() {
        assert!(wanted("WARN", "hooks::overlay", "Lost the server"));
        assert!(wanted("ERROR", "hooks::api", "sign-in failed"));
        assert!(wanted("INFO", "hooks::hooks::nat", "NAT: told the game to advertise 1.2.3.4:5"));
        assert!(!wanted("INFO", "hooks::hooks::nat", "sendto Some(1.2.3.4:5): 00ff"));
        assert!(!wanted("INFO", "hooks::overlay", "refreshing"));
        assert!(!wanted("DEBUG", "hooks::hooks::nat", "probe"));
        assert!(!wanted("ERROR", "hyper::proto", "connection error"), "only this client's own");
        assert!(!wanted("WARN", "hooks::diagnostics", "x"), "never its own");
    }

    #[test]
    fn lines_of_the_log_are_taken_apart() {
        let full = "2026-10-04T22:12:40.090123Z  INFO ThreadId(07) fe-api hooks::hooks::nat: hooks/src/hooks/nat.rs:452: NAT: this PC is 203.0.113.5:13000 to the server";
        assert_eq!(parse_line(full), Some(("INFO", "hooks::hooks::nat", "NAT: this PC is 203.0.113.5:13000 to the server")));
        let short = "2026-09-30T10:00:02Z ERROR hooks::hooks: Hook GetAdaptersInfo failed: NotFound";
        assert_eq!(parse_line(short), Some(("ERROR", "hooks::hooks", "Hook GetAdaptersInfo failed: NotFound")));
        assert_eq!(parse_line("random text"), None);
        let log = format!("{full}\n{short}\n2026-09-30T10:00:03Z  INFO hooks::overlay: refreshing\n");
        assert_eq!(example(&log, 1), ["ERROR hooks::hooks: Hook GetAdaptersInfo failed: NotFound"]);
        assert_eq!(example(&log, 10).len(), 2);
    }
}
