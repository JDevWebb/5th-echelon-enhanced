//! Asking players how their game went, and sending what they say with their logs.
//!
//! When the game closes the launcher reads the client's log ([`read_log`]) and asks the
//! server what it saw of the session (`Misc.SessionSummary`), then decides whether to ask
//! the player ([`triggers`], [`Asked`]): after something went wrong, or now and then after a
//! normal game. What goes with their answer, if they agree, is their logs with the names,
//! folders and addresses on their PC hidden ([`redact`]), each compressed and capped
//! ([`prepare`]).

use std::io::Write as _;
use std::net::Ipv4Addr;
use std::path::Path;

/// What the client's log says about the game that just closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogSignals {
    /// The game went online: Storm's socket opened, or a session was announced.
    pub played_online: bool,
    /// Its matches went through the server's relay.
    pub relayed: bool,
    pub panicked: bool,
    /// The server refused the client's sign-in.
    pub signin_refused: bool,
}

pub fn read_log(text: &str) -> LogSignals {
    let mut s = LogSignals::default();
    for line in text.lines() {
        s.played_online |= line.contains("NAT: Storm socket bound") || line.contains("Session Some(");
        s.relayed |= line.contains("(through the server's relay)");
        s.panicked |= line.contains("panicked at");
        s.signin_refused |= line.contains("The server refused this client") || line.contains("Login error:") || line.contains("Sign-in failed:");
    }
    s
}

/// What the server saw of the session (`Misc.SessionSummary`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerSaw {
    pub failed_joins: i32,
    pub version_mismatches: i32,
    pub relayed: bool,
}

/// Whether a game's exit code means it crashed: Windows' exception codes (0xC0000005 and
/// the like). A plain exit, 0 or otherwise, isn't taken for one.
pub fn crashed(code: Option<i32>) -> bool {
    #[allow(clippy::cast_sign_loss)]
    code.is_some_and(|c| (c as u32) >= 0xC000_0000)
}

/// Why to ask the player how it went, if at all: what went wrong, or "routine" now and then
/// after a game that went online. Empty: don't ask.
pub fn triggers(log: LogSignals, server: Option<ServerSaw>, exit_code: Option<i32>, routine_due: bool) -> Vec<&'static str> {
    let mut t = Vec::new();
    let server = server.unwrap_or_default();
    if log.panicked {
        t.push("panic");
    }
    if crashed(exit_code) {
        t.push("exit_code");
    }
    if log.signin_refused {
        t.push("signin_refused");
    }
    if server.version_mismatches > 0 {
        t.push("version_mismatch");
    }
    if server.failed_joins > server.version_mismatches {
        t.push("failed_join");
    }
    if log.relayed || server.relayed {
        t.push("relayed");
    }
    if t.is_empty() && log.played_online && routine_due {
        t.push("routine");
    }
    t
}

/// When the player was last asked, kept between runs: not more than once a day, and a
/// routine ask only once a week.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Asked {
    /// Unix seconds.
    pub last: i64,
    pub last_routine: i64,
}

const DAY: i64 = 86_400;

impl Asked {
    pub fn routine_due(&self, now: i64) -> bool {
        now - self.last_routine >= 7 * DAY
    }

    /// Whether to ask now, for these triggers. "relayed" alone is asked about like a routine
    /// ask: it's often fine.
    pub fn may_ask(&self, triggers: &[&str], now: i64) -> bool {
        if triggers.is_empty() || now - self.last < DAY {
            return false;
        }
        let only_mild = triggers.iter().all(|t| matches!(*t, "relayed" | "routine"));
        !only_mild || self.routine_due(now)
    }

    pub fn note(&mut self, triggers: &[&str], now: i64) {
        self.last = now;
        if triggers.iter().all(|t| matches!(*t, "relayed" | "routine")) {
            self.last_routine = now;
        }
    }
}

/// What to hide in the logs sent: the names of the player's Windows (or Linux) account and
/// PC. Folders under a user's home are hidden whatever the name; so are public addresses
/// other than `keep` (the server's, which isn't the player's).
#[derive(Debug, Clone, Default)]
pub struct Private {
    pub names: Vec<String>,
    pub keep: Vec<Ipv4Addr>,
}

impl Private {
    /// This PC's account and computer names.
    pub fn of_this_pc(keep: Vec<Ipv4Addr>) -> Self {
        let names = ["USERNAME", "USER", "COMPUTERNAME", "HOSTNAME", "LOGNAME"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .map(|n| n.trim().to_string())
            .filter(|n| n.chars().count() >= 3)
            .collect();
        Self { names, keep }
    }
}

/// `text` with the private parts hidden: home folders (`C:\Users\<name>`, `/home/<name>`,
/// also with doubled backslashes as debug output writes them), the names in `private`
/// wherever they appear, and public IPv4 addresses (`203.0.x.x`) other than those kept.
pub fn redact(text: &str, private: &Private) -> String {
    // The names first: a home folder may be the whole of one ("C:\Users\Jo Smith").
    let mut out = text.to_string();
    for name in &private.names {
        out = replace_ignoring_case(&out, name, "<private>");
    }
    hide_public_addresses(&hide_homes(&out), &private.keep)
}

fn hide_homes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let lower = text.to_ascii_lowercase();
    let mut i = 0;
    while i < text.len() {
        let rest = &lower[i..];
        let marker = ["users\\\\", "users\\", "users/", "home/"].into_iter().find(|m| rest.starts_with(m));
        let at_boundary = i == 0 || matches!(text.as_bytes()[i - 1], b'\\' | b'/');
        if let (Some(m), true) = (marker, at_boundary) {
            out.push_str(&text[i..i + m.len()]);
            i += m.len();
            // To the end of the folder's name, spaces and all ("Jo Smith").
            let end = text[i..].find(['\\', '/', '"', '\'', '\n', '\r', ',', ';', ')', ']']).map_or(text.len(), |e| i + e);
            if end > i {
                out.push_str("<user>");
            }
            i = end;
            continue;
        }
        let c = text[i..].chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn replace_ignoring_case(text: &str, what: &str, with: &str) -> String {
    // ASCII case only: it keeps every byte where it was, so the offsets match `text`.
    let lower = text.to_ascii_lowercase();
    let what = what.to_ascii_lowercase();
    if what.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in lower.match_indices(&what) {
        out.push_str(&text[last..i]);
        out.push_str(with);
        last = i + what.len();
    }
    out.push_str(&text[last..]);
    out
}

fn hide_public_addresses(text: &str, keep: &[Ipv4Addr]) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let starts = bytes[i].is_ascii_digit() && (i == 0 || !(bytes[i - 1].is_ascii_digit() || bytes[i - 1] == b'.'));
        if starts {
            let end = text[i..].find(|c: char| !(c.is_ascii_digit() || c == '.')).map_or(text.len(), |e| i + e);
            let candidate = text[i..end].trim_end_matches('.');
            if let Ok(ip) = candidate.parse::<Ipv4Addr>() {
                let public = !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast());
                if public && !keep.contains(&ip) {
                    let [a, b, ..] = ip.octets();
                    out.push_str(&format!("{a}.{b}.x.x"));
                } else {
                    out.push_str(candidate);
                }
                i += candidate.len();
                continue;
            }
            out.push_str(&text[i..end]);
            i = end;
            continue;
        }
        let c = text[i..].chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// A file going with a report: what's sent (gzip) and what the player can look at first.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub gzip: Vec<u8>,
    /// Uncompressed.
    pub size: u64,
    /// The redacted text, as sent.
    pub text: String,
}

/// The most of each file sent: the end of a longer one (where the problem usually is).
pub const MAX_FILE: usize = 3 * 1024 * 1024;

/// A file's text, redacted and cut to [`MAX_FILE`] bytes, as an attachment: a log's last
/// part (where the trouble is), the data version's first (its summary).
pub fn attach(name: &str, text: &str, private: &Private) -> std::io::Result<Attachment> {
    let mut text = redact(text, private);
    if text.len() > MAX_FILE && name == "bl-dataversion.txt" {
        let mut end = MAX_FILE;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text = format!("{}\n(the last {} bytes left out)", &text[..end], text.len() - end);
    } else if text.len() > MAX_FILE {
        let mut start = text.len() - MAX_FILE;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text = format!("(the first {} bytes left out)\n{}", start, &text[start..]);
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(text.as_bytes())?;
    Ok(Attachment {
        name: name.to_string(),
        gzip: gz.finish()?,
        size: text.len() as u64,
        text,
    })
}

/// The files that go with a report from `game_dir` (the client's log, the one before, the
/// data version's makeup) and the launcher's log, those that exist, plus `extra` texts
/// (name, text) such as the launcher's checklist.
pub fn prepare(game_dir: &Path, launcher_log: Option<&Path>, extra: &[(&str, String)], private: &Private) -> Vec<Attachment> {
    let mut files: Vec<(String, String)> = ["bl-tracing.log", "bl-tracing.prev.log", "bl-dataversion.txt"]
        .iter()
        .filter_map(|name| std::fs::read(game_dir.join(name)).ok().map(|b| (name.to_string(), String::from_utf8_lossy(&b).into_owned())))
        .collect();
    if let Some(text) = launcher_log.and_then(|p| std::fs::read(p).ok()) {
        files.push(("launcher.log".into(), String::from_utf8_lossy(&text).into_owned()));
    }
    files.extend(extra.iter().map(|(n, t)| (n.to_string(), t.clone())));
    files.into_iter().filter_map(|(name, text)| attach(&name, &text, private).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_tells_what_happened() {
        let log = "INFO hooks::hooks::nat: NAT: Storm socket bound on 192.168.0.2:13000\n\
                   INFO NAT: this PC is 122.58.93.144:13000 to the server; advertising 139.99.171.113:40000 (through the server's relay)\n";
        let s = read_log(log);
        assert!(s.played_online && s.relayed && !s.panicked && !s.signin_refused);
        assert!(read_log("ERROR The server refused this client: update").signin_refused);
        assert!(read_log("thread 'main' panicked at src/x.rs").panicked);
        assert_eq!(read_log(""), LogSignals::default());
    }

    #[test]
    fn when_to_ask() {
        let online = LogSignals { played_online: true, ..LogSignals::default() };
        assert_eq!(triggers(online, None, Some(0), false), Vec::<&str>::new());
        assert_eq!(triggers(online, None, Some(0), true), ["routine"]);
        assert_eq!(triggers(LogSignals::default(), None, Some(0), true), Vec::<&str>::new(), "never online: nothing to ask about");
        let saw = ServerSaw { failed_joins: 2, version_mismatches: 1, relayed: true };
        assert_eq!(triggers(online, Some(saw), Some(0), true), ["version_mismatch", "failed_join", "relayed"]);
        #[allow(clippy::cast_possible_wrap)]
        let access_violation = 0xC000_0005_u32 as i32;
        assert_eq!(triggers(online, None, Some(access_violation), false), ["exit_code"]);
        assert!(!crashed(Some(1)) && !crashed(None));

        let day = 86_400;
        let asked = Asked { last: 0, last_routine: 0 };
        assert!(asked.may_ask(&["failed_join"], 10 * day));
        assert!(!Asked { last: 10 * day - 100, last_routine: 0 }.may_ask(&["failed_join"], 10 * day), "once a day");
        let routine_lately = Asked { last: 0, last_routine: 8 * day };
        assert!(!routine_lately.may_ask(&["routine"], 10 * day), "routine once a week");
        assert!(!routine_lately.may_ask(&["relayed"], 10 * day), "relayed alone is like routine");
        assert!(routine_lately.may_ask(&["relayed", "panic"], 10 * day));
        let mut a = Asked::default();
        a.note(&["routine"], 5);
        assert_eq!(a, Asked { last: 5, last_routine: 5 });
        a.note(&["panic"], 9);
        assert_eq!(a, Asked { last: 9, last_routine: 5 });
    }

    #[test]
    fn private_things_are_hidden() {
        let private = Private { names: vec!["DESKTOP-NASN".into(), "msjou".into()], keep: vec![Ipv4Addr::new(139, 99, 171, 113)] };
        let text = r#"Save game name: "C:\\Users\\msjou\\AppData\\Roaming\\5th-Echelon"
path="C:\Users\Jason\Documents" and /home/deck/.steam
gethostbyname: called with "DESKTOP-NASN" by MSJOU
this PC is 122.58.93.144:13000; advertising 139.99.171.113:40000; LAN 192.168.0.2; version 1.2.3.4.5"#;
        let out = redact(text, &private);
        assert!(out.contains(r"C:\\Users\\<user>\\AppData"), "{out}");
        assert!(out.contains(r"C:\Users\<user>\Documents"), "{out}");
        assert!(out.contains("/home/<user>/.steam"), "{out}");
        assert!(out.contains("called with \"<private>\" by <private>"), "{out}");
        assert!(out.contains("122.58.x.x:13000"), "{out}");
        assert!(out.contains("139.99.171.113:40000"), "the server's address stays: {out}");
        assert!(out.contains("192.168.0.2"), "LAN addresses stay: {out}");
        assert!(!out.contains("msjou") && !out.contains("Jason") && !out.contains("deck"));
    }

    #[test]
    fn names_with_spaces_and_other_alphabets_are_hidden() {
        let private = Private { names: vec!["Jo Smith".into(), "DESKTOP-NASN".into()], keep: vec![] };
        let text = "İstanbul: C:\\Users\\Jo Smith\\AppData and C:\\Users\\Other Name\\x; on desktop-nasn";
        let out = redact(text, &private);
        assert!(!out.contains("Smith") && !out.contains("Other Name") && !out.to_lowercase().contains("nasn"), "{out}");
        assert!(out.contains("İstanbul") && out.contains("Users\\<user>\\AppData"), "{out}");
    }

    #[test]
    fn long_files_keep_their_end() {
        let text = format!("{}THE END", "x".repeat(MAX_FILE + 10));
        let a = attach("bl-tracing.log", &text, &Private::default()).unwrap();
        assert!(a.text.ends_with("THE END") && a.text.starts_with("(the first"));
        assert!(a.size as usize <= MAX_FILE + 64);
        assert_eq!(&a.gzip[..2], &[0x1f, 0x8b]);
        // The data version's summary is at its start.
        let text = format!("data version: 0x1234\n{}", "x".repeat(MAX_FILE + 10));
        let a = attach("bl-dataversion.txt", &text, &Private::default()).unwrap();
        assert!(a.text.starts_with("data version: 0x1234") && a.text.ends_with("left out)"));
    }
}
