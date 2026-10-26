//! Checks that every crash fix (and the quick start) finds its place in the game's executables, without starting the
//! game: the same search the hooks run on the game as loaded (`hooks_addresses::crash_fixes`),
//! on the files.
//!
//!   check_crash_fixes <game folder or executable> ...
//!
//! A folder is searched for `Blacklist_game.exe` (DX9) and `Blacklist_DX11_game.exe`, in it and
//! in `src/SYSTEM`. Prints what each fix found, and exits 1 when any fix wouldn't go in.

use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use hooks_addresses::crash_fixes::migration;
use hooks_addresses::crash_fixes::particle;
use hooks_addresses::crash_fixes::scheme_pool;
use hooks_addresses::crash_fixes::storm;
use hooks_addresses::crash_fixes::voice_device;
use hooks_addresses::crash_fixes::Image;
use hooks_addresses::mouse_turn;
use hooks_addresses::quick_start;

const EXECUTABLES: [&str; 2] = ["Blacklist_game.exe", "Blacklist_DX11_game.exe"];

fn executables(arg: &Path) -> Vec<PathBuf> {
    if !arg.is_dir() {
        return vec![arg.to_path_buf()];
    }
    [arg.to_path_buf(), arg.join("src").join("SYSTEM")]
        .iter()
        .flat_map(|dir| EXECUTABLES.iter().map(move |exe| dir.join(exe)))
        .filter(|p| p.is_file())
        .collect()
}

/// Checks one executable; whether every fix went in.
fn check(path: &Path) -> bool {
    println!("{}", path.display());
    let file = match std::fs::read(path) {
        Ok(file) => file,
        Err(e) => {
            println!("  FAILED: couldn't read it: {e}");
            return false;
        }
    };
    let img = match Image::from_pe(&file) {
        Ok(img) => img,
        Err(e) => {
            println!("  FAILED: {e}");
            return false;
        }
    };
    let mut ok = true;
    let mut report = |fix: &str, found: Result<String, String>| match found {
        Ok(what) => println!("  ok      {fix:<18} {what}"),
        Err(why) => {
            println!("  FAILED  {fix:<18} {why}");
            ok = false;
        }
    };
    report(
        "scheme-pool-fix",
        scheme_pool::locate(&img).map(|s| format!("hook {:#010x}, skip {:#010x}, continue {:#010x}", s.hook, s.skip, s.cont)),
    );
    report(
        "particle-fix",
        particle::locate(&img).map(|s| format!("fill {:#010x} (pushed at {:#010x}), streams {:#010x}", s.fill, s.fill_slot, s.streams)),
    );
    let sites = storm::locate(&img);
    for (what, found) in [("remove", sites.remove), ("state change", sites.set_state), ("ack", sites.on_result)] {
        report(&format!("storm-fix {what}"), found.map(|at| format!("{at:#010x}")));
    }
    report(
        "voice-device-fix",
        voice_device::locate(&img).map(|s| {
            format!(
                "vtables {:#010x}, {:#010x}; teardowns {:#010x}, {:#010x}; callback {:#010x} (slot {:#010x})",
                s.playback_vtable, s.capture_vtable, s.playback_teardown, s.capture_teardown, s.callback, s.callback_slot
            )
        }),
    );
    report(
        "migration-fix",
        migration::locate(&img)
            .map(|(tick, _)| format!("tick {tick:#010x}"))
            .map_err(|what| format!("{what} wasn't found or didn't match")),
    );
    report("session lock", migration::lock_test(&img).map(|at| format!("test {at:#010x}")));
    // Not a crash fix, but found the same way: the quick start (QuickStartLobby).
    report(
        "quick-start",
        quick_start::locate(&img).map(|s| format!("{s:#x?}").split_whitespace().collect::<Vec<_>>().join(" ")),
    );
    // Beta (Settings › Beta features): found the same way.
    report(
        "beta: mouse turn",
        mouse_turn::locate(&img).map(|s| format!("{:#010x}, back to {:#010x}", s.patch, s.resume)),
    );
    ok
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if args.is_empty() {
        eprintln!("usage: check_crash_fixes <game folder or executable> ...");
        return ExitCode::from(2);
    }
    let exes: Vec<PathBuf> = args.iter().flat_map(|a| executables(a)).collect();
    if exes.is_empty() {
        eprintln!("no game executable found in {args:?}");
        return ExitCode::from(2);
    }
    let mut all = true;
    for exe in &exes {
        all &= check(exe);
    }
    if all {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
