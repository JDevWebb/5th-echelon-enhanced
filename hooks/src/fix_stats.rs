//! What the crash fixes did, for the server (the 0.5.0 crash work, item 5): which found their
//! place in this build and went in, and how often each kept the game from crashing. Each fix
//! already counted that for its own log line ("... (12 so far): the game would have
//! crashed"); here the counts reach the admin UI's Crashes page, so a fix can be seen working
//! in players' games, and one that no longer finds its place (a game update moved the code)
//! shows as missing on that build.
//!
//! Each fix says it went in, with its counters ([`installed`]); [`expect`] lists those that
//! should. A minute after the start the first request says which went in, then every
//! [`EVERY`] the counts since the last one when any changed. The same agreement as the
//! diagnostics (`send_diagnostics`).

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

use tracing::warn;

/// The first request this long after the start (the fixes go in at once; the sign-in comes
/// later), and the others this often.
const FIRST_AFTER: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(120);

struct Fix {
    name: &'static str,
    installed: bool,
    counters: Vec<&'static AtomicU32>,
    /// What was sent so far.
    sent: u32,
}

static FIXES: Mutex<Vec<Fix>> = Mutex::new(Vec::new());

fn fixes() -> std::sync::MutexGuard<'static, Vec<Fix>> {
    FIXES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The fixes that should go in (each one not [`installed`] by the first request is missing).
pub fn expect(names: &[&'static str]) {
    let mut f = fixes();
    for name in names {
        if !f.iter().any(|x| x.name == *name) {
            f.push(Fix {
                name,
                installed: false,
                counters: Vec::new(),
                sent: 0,
            });
        }
    }
}

/// `name` went in; `counters` count the crashes it kept from happening.
pub fn installed(name: &'static str, counters: &[&'static AtomicU32]) {
    let mut f = fixes();
    match f.iter_mut().find(|x| x.name == name) {
        Some(x) => {
            x.installed = true;
            x.counters = counters.to_vec();
        }
        None => f.push(Fix {
            name,
            installed: true,
            counters: counters.to_vec(),
            sent: 0,
        }),
    }
}

/// Starts sending (once, after the fixes went in).
pub fn start(config: &hooks_config::Config) {
    if !config.send_diagnostics || config.disable_crash_fixes {
        return;
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let spawned = std::thread::Builder::new().name("fix-stats".into()).spawn(move || {
        std::thread::sleep(FIRST_AFTER);
        let mut first = true;
        loop {
            if crate::api::has_token() {
                let (report, any) = {
                    let f = fixes();
                    let mut any = false;
                    let list = f
                        .iter()
                        .map(|x| {
                            let total = x.counters.iter().map(|c| c.load(Ordering::Relaxed)).fold(0u32, u32::saturating_add);
                            let saves = total.saturating_sub(x.sent);
                            any |= saves > 0;
                            server_api::misc::CrashFix {
                                name: x.name.into(),
                                installed: x.installed,
                                saves,
                            }
                        })
                        .collect::<Vec<_>>();
                    (list, any)
                };
                // No crash fixes in this build (patch-free): nothing to say.
                if report.is_empty() {
                    return;
                }
                if first || any {
                    let request = server_api::misc::CrashFixesRequest {
                        first,
                        exe: exe.clone(),
                        release: env!("FE_RELEASE").into(),
                        fixes: report.clone(),
                    };
                    match crate::api::crash_fixes(request) {
                        Ok(()) => {
                            first = false;
                            let mut f = fixes();
                            for sent in &report {
                                if let Some(x) = f.iter_mut().find(|x| x.name == sent.name) {
                                    x.sent = x.sent.saturating_add(sent.saves);
                                }
                            }
                        }
                        Err(crate::api::Error::GRPCStatus(s)) if s.code() == tonic::Code::Unimplemented => return,
                        Err(_) => {}
                    }
                }
            }
            std::thread::sleep(EVERY);
        }
    });
    if spawned.is_err() {
        warn!("Crash fix counts: the thread couldn't start");
    }
}
