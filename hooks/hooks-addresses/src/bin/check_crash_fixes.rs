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

use hooks_addresses::crash_fixes::disruption;
use hooks_addresses::crash_fixes::gear_list;
use hooks_addresses::crash_fixes::load_order;
use hooks_addresses::crash_fixes::migration;
use hooks_addresses::crash_fixes::particle;
use hooks_addresses::crash_fixes::perception;
use hooks_addresses::crash_fixes::scheme_pool;
use hooks_addresses::crash_fixes::storm;
use hooks_addresses::crash_fixes::tagging_device;
use hooks_addresses::crash_fixes::umd_read;
use hooks_addresses::crash_fixes::voice_device;
use hooks_addresses::crash_fixes::Image;
use hooks_addresses::engine_tables;
use hooks_addresses::host_migration;
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
        "tagging-device-fix",
        tagging_device::locate(&img).map(|s| format!("hook {:#010x}, end {:#010x}, continue {:#010x}", s.hook, s.skip, s.cont)),
    );
    report(
        "perception-fix",
        perception::locate(&img).map(|s| format!("hook {:#010x}, no {:#010x}, continue {:#010x}", s.hook, s.skip, s.cont)),
    );
    report(
        "gear-list-fix",
        gear_list::locate(&img).map(|s| format!("hook {:#010x}, continue {:#010x}, next name {:#010x}", s.hook, s.cont, s.skip)),
    );
    report(
        "disruption-fix",
        disruption::locate(&img).map(|s| format!("lookup {:#010x}, nav-disruption tick {:#010x} (end {:#010x})", s.lookup, s.tick, s.tick_end)),
    );
    report(
        "load-order-fix",
        load_order::locate(&img).map(|s| format!("reads {:#010x}, {:#010x}, end {:#010x}", s.first, s.second, s.done)),
    );
    report(
        "migration-fix",
        migration::locate(&img)
            .map(|(tick, _)| format!("tick {tick:#010x}"))
            .map_err(|what| format!("{what} wasn't found or didn't match")),
    );
    report("session lock", migration::lock_test(&img).map(|at| format!("test {at:#010x}")));
    report(
        "umd-read-fix",
        umd_read::locate(&img).map(|s| {
            format!(
                "vtable {:#010x} (Serialize {:#010x}, sync slot {:#010x}, Precache {:#010x}), sync {:#010x}, switches {:#010x}/{:#010x}",
                s.vtable, s.serialize, s.sync_only, s.precache, s.sync, s.linear_on, s.linear_off
            )
        }),
    );
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
    // Host migration (on by default): both builds.
    report(
        "host migration",
        host_migration::locate(&img).map(|s| {
            format!(
                "setter {:#010x}, check {:#010x}, leader action {:#010x}, sync {:#010x}/{:#010x}, follow task {:#010x}, \
                 FollowDelegate {:#010x}, facade {:#010x}, task runner {:#010x}",
                s.setter, s.check, s.leader_action, s.sync_enter, s.sync_tick, s.follow_task, s.follow_delegate, s.facade, s.facade_gate
            )
        }),
    );
    // The engine's tables (match state, witness reports, recordings, the data version file).
    report(
        "engine tables",
        engine_tables::locate(&img).map(|t| {
            format!(
                "objects {:#010x} (lookup {:#010x}), names {:#010x}/{:#010x}, ranges {:#010x} {:#010x} {:#010x}, version count {:#010x}",
                t.object_hash, t.hash_lookup, t.names, t.names_count, t.native_end, t.startup_start, t.startup_end, t.game_version_count
            )
        }),
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
