//! Asking players how their game went, and sending what they say with their logs.
//!
//! When the game closes the launcher reads the client's log ([`read_log`]) and asks the
//! server what it saw of the session (`Misc.SessionSummary`), then decides whether to ask
//! the player ([`triggers`], [`Asked`]): after something went wrong, or now and then after a
//! normal game. What goes with their answer, if they agree, is their logs with the names,
//! folders and addresses on their PC hidden ([`redact`]), each compressed and capped
//! ([`prepare`]).

use std::io::Write as _;
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

pub use hooks_config::redact::redact;
pub use hooks_config::redact::Private;

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
    attach_within(name, text, private, MAX_FILE)
}

/// [`attach`], cut to `max` bytes.
fn attach_within(name: &str, text: &str, private: &Private, max: usize) -> std::io::Result<Attachment> {
    let mut text = redact(text, private);
    if text.len() > max && name == "bl-dataversion.txt" {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text = format!("{}\n(the last {} bytes left out)", &text[..end], text.len() - end);
    } else if text.len() > max {
        let mut start = text.len() - max;
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
    within_total(files, private, MAX_TOTAL_GZIP)
}

/// The most a report's files may come to, compressed: under the 4 MB a server's proxy took
/// before 0.4.2 (a larger report was refused before reaching the server), with room for the
/// rest of the report.
pub const MAX_TOTAL_GZIP: usize = 3 * 1024 * 1024 + 512 * 1024;

/// Files that go first when a report is too large, least useful first.
const LEAST_USEFUL: [&str; 2] = ["bl-tracing.prev.log", "launcher.log"];

/// The files as attachments, coming to at most `max_total` bytes compressed: the least
/// useful ones left out first, then the client's log cut to a shorter end.
fn within_total(files: Vec<(String, String)>, private: &Private, max_total: usize) -> Vec<Attachment> {
    let mut attached: Vec<Attachment> = files.iter().filter_map(|(name, text)| attach(name, text, private).ok()).collect();
    let total = |a: &[Attachment]| a.iter().map(|f| f.gzip.len()).sum::<usize>();
    for name in LEAST_USEFUL {
        if total(&attached) <= max_total {
            return attached;
        }
        attached.retain(|f| f.name != name);
    }
    let mut max = MAX_FILE;
    while total(&attached) > max_total && max > 16 * 1024 {
        max /= 2;
        let Some((name, text)) = files.iter().find(|(n, _)| n == "bl-tracing.log") else { break };
        if let (Some(i), Ok(cut)) = (attached.iter().position(|f| f.name == *name), attach_within(name, text, private, max)) {
            attached[i] = cut;
        } else {
            break;
        }
    }
    attached
}

#[cfg(test)]
mod tests {

    /// A report too large for a server's proxy leaves out the least useful files first, then
    /// keeps a shorter end of the client's log.
    #[test]
    fn reports_stay_within_what_a_server_takes() {
        // Text that hardly compresses (random letters and digits), 3 MB each.
        let noisy = |seed: u64| {
            const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut x = seed;
            (0..3 * 1024 * 1024)
                .map(|_| {
                    x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                    char::from(ABC[(x >> 58) as usize])
                })
                .collect::<String>()
        };
        let files = vec![
            ("bl-tracing.log".to_string(), noisy(1)),
            ("bl-tracing.prev.log".to_string(), noisy(2)),
            ("launcher.log".to_string(), "small".to_string()),
        ];
        let out = super::within_total(files.clone(), &Private::default(), super::MAX_TOTAL_GZIP);
        let names: Vec<&str> = out.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["bl-tracing.log", "launcher.log"], "the log before goes first");
        assert!(out.iter().map(|f| f.gzip.len()).sum::<usize>() <= super::MAX_TOTAL_GZIP);
        let out = super::within_total(files, &Private::default(), 1024 * 1024);
        assert_eq!(out[0].name, "bl-tracing.log", "the client's log stays, its end");
        assert!(out.iter().map(|f| f.gzip.len()).sum::<usize>() <= 1024 * 1024);
        assert!(out[0].text.starts_with("(the first"));
    }
    use std::net::Ipv4Addr;
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
