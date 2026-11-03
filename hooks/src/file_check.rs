//! The game's files that change play, hashed for the sign-in (hooks_config::game_files; the
//! 0.4.9 plan's D0 in 0.4.8, report only). The launcher hashed them before Play was ready, so
//! this reads its cache in a few ms; a game started without the launcher hashes them here, in
//! the background, and a sign-in before that's done goes without (the next one carries them).

use std::path::PathBuf;
use std::sync::OnceLock;

use hooks_config::game_files::Files;
use tracing::info;
use tracing::warn;

static FILES: OnceLock<Files> = OnceLock::new();

/// The game's files, once read (`None` while they're still being hashed).
pub fn files() -> Option<&'static Files> {
    FILES.get()
}

/// Reads (or hashes) the files in `system`, the game's SYSTEM folder, in the background.
pub fn start(system: PathBuf) {
    let spawned = std::thread::Builder::new().name("file-check".into()).spawn(move || {
        let started = std::time::Instant::now();
        let files = hooks_config::game_files::look(&system);
        info!(
            "Game files: {} hashed, {} others{} ({} ms)",
            files.hashed.len(),
            files.others.len(),
            if files.others.is_empty() {
                String::new()
            } else {
                format!(" ({})", files.others.join(", "))
            },
            started.elapsed().as_millis()
        );
        let _ = FILES.set(files);
    });
    if let Err(e) = spawned {
        warn!("Couldn't start hashing the game's files: {e}");
    }
}
