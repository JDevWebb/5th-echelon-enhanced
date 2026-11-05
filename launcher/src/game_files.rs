//! The game's files, hashed before Play is ready (hooks_config::game_files; the 0.5.1 plan's
//! D0 in 0.5.0, report only): the game's sign-in carries them, and the add-on reads them back
//! through the cache in a few ms, so they're always there for its first sign-in. The first
//! time takes a few seconds (a few hundred MB); after that only a changed file is read.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

/// Each game folder's state: being hashed, or done.
static STATE: Mutex<Option<HashMap<PathBuf, bool>>> = Mutex::new(None);

/// What the Play button says while it waits.
pub const PREPARING: &str = "Checking game files";

/// Starts hashing the files in `system` (the game's SYSTEM folder) in the background, unless
/// that's under way or done in this run of the launcher.
pub fn prepare(system: &Path) {
    let mut state = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = state.get_or_insert_with(HashMap::new);
    if map.contains_key(system) {
        return;
    }
    map.insert(system.to_path_buf(), false);
    let folder = system.to_path_buf();
    let spawned = std::thread::Builder::new().name("game-files".into()).spawn(move || {
        let system = folder;
        let started = std::time::Instant::now();
        let files = hooks_config::game_files::look(&system);
        tracing::info!(
            "Game files: {} hashed, {} others ({} ms)",
            files.hashed.len(),
            files.others.len(),
            started.elapsed().as_millis()
        );
        done(&system);
    });
    if let Err(e) = spawned {
        // Nothing to wait for: the game hashes them itself.
        tracing::warn!("Couldn't start hashing the game's files: {e}");
        done(&system);
    }
}

fn done(system: &Path) {
    let mut state = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.get_or_insert_with(HashMap::new).insert(system.to_path_buf(), true);
}

/// Whether the files in `system` are still being hashed (starting it if it hasn't begun).
pub fn preparing(system: &Path) -> bool {
    prepare(system);
    let state = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.as_ref().and_then(|m| m.get(system)).is_some_and(|done| !done)
}

#[cfg(test)]
mod tests {
    /// A folder is hashed once a run: preparing until done, then never again.
    #[test]
    fn a_folder_is_prepared_once() {
        let dir = std::env::temp_dir().join(format!("fes-game-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Blacklist_game.exe"), b"abc").unwrap();
        super::prepare(&dir);
        let waited = std::time::Instant::now();
        while super::preparing(&dir) {
            assert!(waited.elapsed() < std::time::Duration::from_secs(10), "never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(dir.join(hooks_config::game_files::CACHE).exists(), "the add-on's cache is written");
        std::fs::remove_file(dir.join(hooks_config::game_files::CACHE)).unwrap();
        assert!(!super::preparing(&dir), "done for this run");
        assert!(!dir.join(hooks_config::game_files::CACHE).exists(), "not hashed again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
