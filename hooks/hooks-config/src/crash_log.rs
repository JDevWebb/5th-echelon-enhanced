//! The crashes and freezes in a game log, as the add-on's crash handler writes them
//! (`hooks::crash`), for sending to the server when the game starts again: every player's,
//! not only those who send a report (the 0.5.0 crash work, item 1).
//!
//! The handler's lines:
//! * `The game hit an exception: 0xC0000005 access violation (reading 0x00000538) at
//!   Blacklist_DX11_game.exe+0x6f205a. On the stack: A, B. If the game handles it, it carries on.`
//! * then `Crash registers: eax=... ; code at eip: 8b 80 ...; pointing to: ...` and
//!   `Crash callers: A, B; the game was: ...` (0.5.0; older logs have neither);
//! * `The game is closing on an unhandled exception ...` when nothing handled it (0.5.0);
//! * `The game froze: no frame for 11 s; the thread that draws (thread 1) is at X. On the
//!   stack: ...` (and the main thread's), then a warning that it froze.
//!
//! A log can hold more than one run (a run that closed at once is added to, not replaced):
//! each run starts at the add-on's `attaching` line, and each is read on its own.

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Exception,
    Freeze,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Exception => "exception",
            Kind::Freeze => "freeze",
        }
    }
}

/// One crash or freeze.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crash {
    pub kind: Kind,
    /// Unix seconds (the log's UTC time).
    pub at: i64,
    /// `0xC0000005 access violation (reading 0x00000538)`, or `no frame for 11 s`.
    pub code: String,
    /// Where: `Blacklist_DX11_game.exe+0x6f205a` (a freeze: the drawing thread's first frame
    /// in the game's code, else where it was).
    pub place: String,
    /// The code addresses on the stack, as written.
    pub stack: String,
    /// The registers line (empty before 0.5.0).
    pub registers: String,
    /// The callers by frame pointers (empty before 0.5.0).
    pub callers: String,
    /// What the game was doing (empty before 0.5.0).
    pub doing: String,
    /// The game didn't carry on: nothing handled it, or the run ended soon after without
    /// closing normally.
    pub fatal: bool,
}

impl Crash {
    /// What crashes with the same cause share: where, plus the kind for a freeze.
    pub fn signature(&self) -> String {
        match self.kind {
            Kind::Exception => self.place.clone(),
            Kind::Freeze => format!("freeze at {}", self.place),
        }
    }
}

/// A run that ends this soon after its last crash, without closing normally, ended with it.
const ENDED_WITHIN: i64 = 60;

const EXCEPTION: &str = "hooks::crash: The game hit an exception: ";
const REGISTERS: &str = "hooks::crash: Crash registers: ";
const CALLERS: &str = "hooks::crash: Crash callers: ";
const UNHANDLED: &str = "hooks::crash: The game is closing on an unhandled exception";
const FROZE: &str = "hooks::crash: The game froze: no frame for ";
const CLOSING: &str = "The game is closing";
/// The add-on's first line in a run (`hooks: 591: attaching`).
fn starts_run(line: &str) -> bool {
    line.contains(" hooks: ") && line.trim_end().ends_with(": attaching")
}

/// The crashes and freezes in `log`, oldest first.
pub fn crashes(log: &str) -> Vec<Crash> {
    let mut runs: Vec<Vec<&str>> = vec![Vec::new()];
    for line in log.lines() {
        if starts_run(line) && runs.last().is_some_and(|r| !r.is_empty()) {
            runs.push(Vec::new());
        }
        if let Some(run) = runs.last_mut() {
            run.push(line);
        }
    }
    runs.iter().flat_map(|run| in_run(run)).collect()
}

fn in_run(lines: &[&str]) -> Vec<Crash> {
    let mut found: Vec<Crash> = Vec::new();
    let mut unhandled_after: Option<usize> = None;
    let mut closed = false;
    let mut last_time = None;
    // Index into `found` of the last exception, for the lines that follow it.
    let mut last_exception: Option<usize> = None;
    // The freeze being read: its drawing thread's line seen, waiting for the main thread's.
    let mut freeze: Option<usize> = None;
    for line in lines {
        let time = crate::diagnostics::line_time(line);
        if time.is_some() {
            last_time = time;
        }
        if let Some(rest) = after(line, EXCEPTION) {
            if let Some(c) = exception(rest, time.unwrap_or(0)) {
                found.push(c);
                last_exception = Some(found.len() - 1);
            }
            freeze = None;
        } else if let Some(rest) = after(line, REGISTERS) {
            if let Some(c) = last_exception.and_then(|i| found.get_mut(i)) {
                if c.registers.is_empty() {
                    c.registers = rest.trim().to_string();
                }
            }
        } else if let Some(rest) = after(line, CALLERS) {
            if let Some(c) = last_exception.and_then(|i| found.get_mut(i)) {
                if c.callers.is_empty() {
                    let (callers, doing) = rest.split_once("; the game was: ").unwrap_or((rest, ""));
                    c.callers = callers.trim().to_string();
                    c.doing = doing.trim().trim_end_matches('.').to_string();
                }
            }
        } else if line.contains(UNHANDLED) {
            unhandled_after = Some(found.len());
        } else if let Some(rest) = after(line, FROZE) {
            // The drawing thread's line first, then the main thread's: one freeze.
            let drawing = rest.contains("the thread that draws");
            match freeze {
                Some(i) if !drawing => {
                    if let Some(c) = found.get_mut(i) {
                        if let Some((_, main)) = rest.split_once(" is at ") {
                            c.stack = format!("{}; main thread at {}", c.stack, main.trim());
                        }
                    }
                    freeze = None;
                }
                _ => {
                    if let Some(c) = froze(rest, time.unwrap_or(0)) {
                        found.push(c);
                        freeze = Some(found.len() - 1);
                    }
                }
            }
        } else if line.contains(CLOSING) && line.contains(" hooks: ") {
            closed = true;
        }
    }
    // Nothing handled them: those before the unhandled line are fatal (its last one, really).
    if let Some(n) = unhandled_after {
        if let Some(c) = found[..n].iter_mut().rev().find(|c| c.kind == Kind::Exception) {
            c.fatal = true;
        }
    }
    // A run that didn't close, and ended soon after its last crash: that one ended it.
    if !closed {
        if let (Some(c), Some(end)) = (found.last_mut(), last_time) {
            if end - c.at <= ENDED_WITHIN {
                c.fatal = true;
            }
        }
    }
    found
}

/// What's after `marker` in `line`.
fn after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.find(marker).map(|i| &line[i + marker.len()..])
}

fn exception(rest: &str, at: i64) -> Option<Crash> {
    let (code, rest) = rest.split_once(" at ")?;
    // `X (eip Y). On the stack: ...` or `X. On the stack: ...`
    let (place, rest) = match rest.split_once(". On the stack:") {
        Some((p, r)) => (p, r),
        None => (rest.split_once(". If the game").map_or(rest, |(p, _)| p), ""),
    };
    let place = place.split_once(" (eip ").map_or(place, |(p, _)| p).trim();
    let stack = rest.split_once(". If the game").map_or(rest, |(s, _)| s).trim();
    Some(Crash {
        kind: Kind::Exception,
        at,
        code: code.trim().to_string(),
        place: place.to_string(),
        stack: stack.to_string(),
        registers: String::new(),
        callers: String::new(),
        doing: String::new(),
        fatal: false,
    })
}

fn froze(rest: &str, at: i64) -> Option<Crash> {
    // `11 s; the thread that draws (thread 1) is at X. On the stack: A, B; callers: ...;
    // the game was: ...`
    let (secs, rest) = rest.split_once(';')?;
    let (_, rest) = rest.split_once(" is at ")?;
    let (doing_rest, doing) = rest.split_once("; the game was: ").unwrap_or((rest, ""));
    let (at_stack, callers) = doing_rest.split_once("; callers: ").unwrap_or((doing_rest, ""));
    let (place, stack) = at_stack.split_once(". On the stack:").unwrap_or((at_stack, ""));
    let game = |s: &str| s.split(", ").map(str::trim).find(|f| f.to_ascii_lowercase().contains("_game.exe+")).map(str::to_string);
    let place = game(callers).or_else(|| game(stack)).unwrap_or_else(|| place.trim().to_string());
    Some(Crash {
        kind: Kind::Freeze,
        at,
        code: format!("no frame for {}", secs.trim()),
        place,
        stack: stack.trim().to_string(),
        registers: String::new(),
        callers: callers.trim().to_string(),
        doing: doing.trim().trim_end_matches('.').to_string(),
        fatal: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANGLED: &str = "\
2026-11-04T21:10:00.000000Z  INFO ThreadId(01) hooks: 591: attaching
2026-11-04T21:13:06.800418Z  INFO ThreadId(01) UPLAY_SAVE_Close{out_save_handle=0x23e883d0}: hooks::uplay_r1_loader::save: 54: UPLAY_SAVE_Close
2026-11-04T21:14:32.094000Z ERROR hooks::crash: The game hit an exception: 0xC0000005 access violation (reading 0x00000538) at Blacklist_DX11_game.exe+0x6f205a. On the stack: Blacklist_DX11_game.exe+0xa94680, Blacklist_DX11_game.exe+0x9881ec. If the game handles it, it carries on.
2026-11-04T21:14:40.140116Z  WARN      overlay-chat ThreadId(17) hooks::chat: 325: Chat: waiting for messages failed
";

    #[test]
    fn a_crash_from_before_0_5_0_that_ended_the_game() {
        let c = crashes(ANGLED);
        assert_eq!(c.len(), 1);
        let c = &c[0];
        assert_eq!(c.kind, Kind::Exception);
        assert_eq!(c.code, "0xC0000005 access violation (reading 0x00000538)");
        assert_eq!(c.place, "Blacklist_DX11_game.exe+0x6f205a");
        assert_eq!(c.signature(), "Blacklist_DX11_game.exe+0x6f205a");
        assert_eq!(c.stack, "Blacklist_DX11_game.exe+0xa94680, Blacklist_DX11_game.exe+0x9881ec");
        assert_eq!(c.at, crate::diagnostics::line_time("2026-11-04T21:14:32").unwrap());
        assert!(c.fatal, "the log ended 8 s later without closing");
    }

    #[test]
    fn a_handled_exception_is_not_fatal() {
        let log = format!("{ANGLED}2026-11-04T21:30:00.000000Z  INFO hooks: The game is closing (a normal exit)\n");
        assert!(!crashes(&log)[0].fatal);
        // Still running long after, killed later: not this one's doing either.
        let log = format!("{ANGLED}2026-11-04T21:40:00.000000Z  INFO ThreadId(01) hooks::overlay: 1646: something\n");
        assert!(!crashes(&log)[0].fatal);
    }

    #[test]
    fn the_details_lines_and_the_unhandled_line() {
        let log = "\
2026-11-09T10:00:00.000000Z  INFO ThreadId(01) hooks: 591: attaching
2026-11-09T10:05:00.000000Z ERROR hooks::crash: The game hit an exception: 0xC0000005 access violation (reading 0x00000538) at Blacklist_DX11_game.exe+0x6f205a (eip Blacklist_DX11_game.exe+0x6f205a). On the stack: Blacklist_DX11_game.exe+0xa94680. If the game handles it, it carries on.
2026-11-09T10:05:00.000000Z ERROR hooks::crash: Crash registers: eax=00000000 ecx=1234abcd; code at eip: 8b 80 38 05 00 00
2026-11-09T10:05:00.000000Z ERROR hooks::crash: Crash callers: Blacklist_DX11_game.exe+0xa94680, Blacklist_DX11_game.exe+0x9881ec; the game was: in a match, co-op, map Site F, saved 90 s before.
2026-11-09T10:05:00.000000Z ERROR hooks::crash: The game is closing on an unhandled exception 0xC0000005 at Blacklist_DX11_game.exe+0x6f205a
2026-11-09T10:09:00.000000Z  INFO ThreadId(17) hooks::chat: 325: still going somehow
";
        let c = &crashes(log)[0];
        assert_eq!(c.place, "Blacklist_DX11_game.exe+0x6f205a");
        assert_eq!(c.registers, "eax=00000000 ecx=1234abcd; code at eip: 8b 80 38 05 00 00");
        assert_eq!(c.callers, "Blacklist_DX11_game.exe+0xa94680, Blacklist_DX11_game.exe+0x9881ec");
        assert_eq!(c.doing, "in a match, co-op, map Site F, saved 90 s before");
        assert!(c.fatal, "nothing handled it");
    }

    #[test]
    fn a_freeze_is_one_crash_named_by_the_games_code() {
        let log = "\
2026-11-05T18:39:00.000000Z  INFO ThreadId(01) hooks: 591: attaching
2026-11-05T18:39:42.988000Z ERROR hooks::crash: The game froze: no frame for 11 s; the thread that draws (thread 20488) is at ntdll.dll+0x7826c. On the stack: KERNELBASE.dll+0x15e808, Blacklist_DX11_game.exe+0x123456, Blacklist_DX11_game.exe+0x654321
2026-11-05T18:39:42.989000Z ERROR hooks::crash: The game froze: no frame for 11 s; the main thread (thread 8368) is at ntdll.dll+0x7858c. On the stack: ntdll.dll+0x93dc9
2026-11-05T18:39:42.993654Z  WARN      freeze-watch ThreadId(02) hooks::crash: 419: The game froze: no frame for 11 s with its window in front (where its threads were is above)
2026-11-05T18:45:00.000000Z  INFO hooks: The game is closing (a normal exit)
";
        let c = crashes(log);
        assert_eq!(c.len(), 1, "the warning after the two threads' lines isn't another");
        let c = &c[0];
        assert_eq!(c.kind, Kind::Freeze);
        assert_eq!(c.code, "no frame for 11 s");
        assert_eq!(c.place, "Blacklist_DX11_game.exe+0x123456");
        assert_eq!(c.signature(), "freeze at Blacklist_DX11_game.exe+0x123456");
        assert!(c.stack.ends_with("; main thread at ntdll.dll+0x7858c. On the stack: ntdll.dll+0x93dc9"));
        assert!(!c.fatal);
    }

    #[test]
    fn each_run_is_read_on_its_own() {
        // The run that crashed, then a run that closed at once added to the same file.
        let log =
            format!("{ANGLED}2026-11-04T21:14:54.166153Z  INFO ThreadId(01) hooks: 591: attaching\n2026-11-04T21:14:58.000000Z  INFO hooks: The game is closing (a normal exit)\n");
        let c = crashes(&log);
        assert_eq!(c.len(), 1);
        assert!(c[0].fatal, "the later run's normal close isn't the crashed run's");
        assert!(crashes("2026-11-04T21:14:54Z nothing here\n").is_empty());
    }
}
