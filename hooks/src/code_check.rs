//! Code integrity (the 0.4.8 plan's F8, report only): whether the game's code (its executable's
//! `.text`) is as the game shipped it, but for 5th Echelon's own changes. Once the game has
//! settled ([`SETTLE`]), the code in memory is compared with the executable on disk, then every
//! [`EVERY`] with how it was the time before. Each run of changed bytes is put down to
//!
//! * 5th Echelon: bytes it wrote ([`ours`], from `writemem`), or a hook's jump into this
//!   add-on (retour's detours);
//! * Fusion Fix or another ASI mod: a jump into its module;
//! * something else: told the server (`Misc.FairPlayCheck`, a session event for the admins),
//!   with where. Casual tools only: the add-on is open source.

use std::sync::Mutex;
use std::time::Duration;

use tracing::info;
use tracing::warn;

/// Time for every fix, hook and mod to be in before the first look.
const SETTLE: Duration = Duration::from_secs(120);
const EVERY: Duration = Duration::from_secs(30);
/// Changed bytes this close together are one change.
const MERGE: usize = 16;
/// The most changes said in one report.
const MOST_SAID: usize = 5;

/// Bytes of the game's code this add-on wrote: (address, length).
fn written() -> std::sync::MutexGuard<'static, Vec<(usize, usize)>> {
    static WRITTEN: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
    WRITTEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// This add-on wrote `len` bytes at `at` (`writemem`): its own change, never reported.
pub fn ours(at: usize, len: usize) {
    let mut w = written();
    if w.len() < 4096 {
        w.push((at, len));
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> usize;
    fn GetModuleHandleExW(flags: u32, address: usize, module: *mut usize) -> i32;
    fn GetModuleFileNameW(module: usize, name: *mut u16, size: u32) -> u32;
}

/// The loaded image at `base`: (`.text`'s address, its size in memory, its offset and size in
/// the file), the image's size, and its preferred base.
unsafe fn text_of(base: usize) -> Option<((usize, usize, usize, usize), usize, usize)> {
    let read = |at: usize| (at as *const u32).read_unaligned() as usize;
    let nt = base + read(base + 0x3C);
    let sections = (nt as *const u16).add(3).read_unaligned() as usize;
    let optional_size = (nt as *const u16).add(10).read_unaligned() as usize;
    let optional = nt + 24;
    let (image_base, image_size) = (read(optional + 28), read(optional + 56));
    for i in 0..sections.min(96) {
        let s = optional + optional_size + i * 40;
        let name = std::slice::from_raw_parts(s as *const u8, 8);
        if name.starts_with(b".text") {
            let (vsize, rva, raw_size, raw_at) = (read(s + 8), read(s + 12), read(s + 16), read(s + 20));
            return Some(((base + rva, vsize, raw_at, raw_size), image_size, image_base));
        }
    }
    None
}

/// The module holding `address`: its range and file name.
fn module_at(address: usize) -> Option<(usize, usize, String)> {
    let mut module = 0usize;
    // FROM_ADDRESS | UNCHANGED_REFCOUNT.
    if unsafe { GetModuleHandleExW(0x4 | 0x2, address, &mut module) } == 0 || module == 0 {
        return None;
    }
    let size = unsafe { text_of(module) }.map_or(0, |(_, size, _)| size);
    let mut name = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), 260) } as usize;
    let file = String::from_utf16_lossy(&name[..n.min(260)]);
    Some((module, size, file.rsplit(['\\', '/']).next().unwrap_or(&file).to_string()))
}

/// Who changed the run of bytes at `at..at + len`, now `now`: this add-on, a mod, or None.
fn who_changed(at: usize, now: &[u8], mine: (usize, usize)) -> Option<String> {
    if written().iter().any(|(w, n)| at < w + n && *w < at + now.len()) {
        return Some(String::from("5th Echelon"));
    }
    // A jump or call (E9/E8 rel32) in it: where to.
    for i in 0..now.len() {
        if !matches!(now[i], 0xE9 | 0xE8) || i + 5 > now.len() {
            continue;
        }
        let rel = i32::from_le_bytes([now[i + 1], now[i + 2], now[i + 3], now[i + 4]]);
        let to = (at + i + 5).wrapping_add_signed(rel as isize);
        if (mine.0..mine.0 + mine.1).contains(&to) {
            return Some(String::from("5th Echelon"));
        }
        if let Some((_, _, file)) = module_at(to) {
            let lower = file.to_lowercase();
            if lower.contains("fusionfix") || lower.contains("fusion") {
                return Some(String::from("Fusion Fix"));
            }
            if lower.ends_with(".asi") {
                return Some(format!("the ASI mod {file}"));
            }
            return Some(format!("unknown ({file})"));
        }
    }
    None
}

/// Runs of differing bytes between `before` and `now` (offsets, merged when close).
fn changed_runs(before: &[u8], now: &[u8]) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    let n = before.len().min(now.len());
    while i < n {
        // Whole equal chunks skipped fast.
        if i + 64 <= n && before[i..i + 64] == now[i..i + 64] {
            i += 64;
            continue;
        }
        if before[i] != now[i] {
            match runs.last_mut() {
                Some((start, len)) if i <= *start + *len + MERGE => *len = i + 1 - *start,
                _ => runs.push((i, 1)),
            }
        }
        i += 1;
    }
    runs
}

/// The changes between `before` and `now` (the code at `text`) no one known made: offset
/// (from the code's start) and whose they look to be. Bytes put back as the game shipped them
/// (`original`, from the file) are no one's change.
fn foreign(text: usize, before: &[u8], now: &[u8], original: Option<&[u8]>, mine: (usize, usize)) -> Vec<(usize, String)> {
    changed_runs(before, now)
        .into_iter()
        .filter_map(|(off, len)| {
            if original.and_then(|o| o.get(off..off + len)) == Some(&now[off..off + len]) {
                return None;
            }
            let who = who_changed(text + off, &now[off..off + len], mine);
            match who.as_deref() {
                Some("5th Echelon" | "Fusion Fix") => None,
                Some(w) if w.starts_with("the ASI mod") => None,
                Some(w) => Some((off, w.to_string())),
                None => Some((off, String::from("bytes changed"))),
            }
        })
        .collect()
}

pub fn start() {
    let wine = hooks_config::running_under_wine();
    let spawned = std::thread::Builder::new().name("fe-code-check".into()).spawn(move || {
        std::thread::sleep(SETTLE);
        let game = unsafe { GetModuleHandleW(std::ptr::null()) };
        let Some(((text, size, raw_at, raw_size), _, preferred)) = (unsafe { text_of(game) }) else {
            warn!("Code check: the game's code wasn't found");
            return;
        };
        let mine = module_at(start as usize).map_or((0, 0), |(base, size, _)| (base, size));
        let snapshot = || unsafe { std::slice::from_raw_parts(text as *const u8, size) }.to_vec();
        let mut before = snapshot();
        // First against the file, where it was loaded at its own base (no relocations moved it).
        let original: Option<Vec<u8>> = if game == preferred {
            std::env::current_exe()
                .ok()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|d| d.get(raw_at..raw_at + raw_size.min(size)).map(<[u8]>::to_vec))
        } else {
            info!("Code check: the game was loaded away from its base; comparing as it runs only");
            None
        };
        if let Some(file) = &original {
            tell(&foreign(text, file, &before[..file.len()], None, mine), "since the game started", wine);
        }
        info!("Code check: watching {} KB of the game's code", size / 1024);
        loop {
            std::thread::sleep(EVERY);
            let now = snapshot();
            tell(&foreign(text, &before, &now, original.as_deref(), mine), "while it ran", wine);
            before = now;
        }
    });
    if let Err(e) = spawned {
        warn!("Code check: couldn't start ({e})");
    }
}

/// Tells the server of changes to the game's code no one known made, if any.
fn tell(found: &[(usize, String)], when: &str, wine: bool) {
    if found.is_empty() {
        return;
    }
    let said: Vec<String> = found.iter().take(MOST_SAID).map(|(off, who)| format!("+{off:#x} ({who})")).collect();
    let detail = format!(
        "{} change{} to the game's code {when} not made by 5th Echelon or Fusion Fix: {}{}",
        found.len(),
        if found.len() == 1 { "" } else { "s" },
        said.join(", "),
        if found.len() > MOST_SAID { ", ..." } else { "" }
    );
    warn!("Code check: {detail}");
    #[allow(clippy::cast_precision_loss)]
    crate::api::fair_play_check("code", &detail, found.len() as f64, wine);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_bytes_are_runs_merged_when_close() {
        let before = vec![0u8; 1000];
        let mut now = before.clone();
        now[10] = 1;
        now[20] = 1;
        now[500] = 2;
        assert_eq!(changed_runs(&before, &now), vec![(10, 11), (500, 1)]);
        assert!(changed_runs(&before, &before).is_empty());
    }
}
