//! Host migration, on for every player by default (Settings › Game turns it off:
//! `DisableHostMigration` in `uplay.toml`; it was a beta until 0.5.0):
//! a match lobby carries on under a new host when its host leaves (`migrate_lobby`), a party's
//! leader invited into a private match takes its party along (`party_follow`), and a member's
//! game left waiting on a stale follow is set going again (`follow_unblock`). The server's side
//! (taking a room over, letting the party in, bringing it back) is in `dedicated_server`'s
//! `game_session.rs` and `escorts.rs`. Background in `docs/architecture/host-and-party-migration.md`.
//!
//! The three go in together or not at all: a lobby that migrates without the follow fixes, or
//! a party taken along without them, leaves players worse off than the game's own way. So
//! everything is found first (`hooks_addresses::host_migration`, by signature, in both the DX9
//! and DX11 builds), and a part that doesn't go in takes the others out again.

use tracing::info;
use tracing::warn;

/// Installs the three, after the crash fixes (`migration_fix` may already hook the UI facade's
/// tick, which `follow_unblock` then runs from).
pub unsafe fn install() {
    let Some(image) = crate::game_image() else {
        warn!("host-migration: the game's sections weren't found -> off");
        return;
    };
    // The tick as migration-fix hooked it, when it did: its signature is gone from memory.
    let sites = match hooks_addresses::host_migration::locate_with(&image, crate::migration_fix::hooked_tick()) {
        Ok(sites) => sites,
        Err(why) => {
            warn!("host-migration: {why} -> off (this game build isn't one it knows)");
            return;
        }
    };
    if !crate::migrate_lobby::install(&sites) {
        warn!("host-migration: off");
        return;
    }
    if !crate::follow_unblock::install(&sites, crate::migration_fix::tick_hooked()) {
        crate::migrate_lobby::undo();
        warn!("host-migration: off");
        return;
    }
    if !crate::party_follow::install(&sites) {
        crate::follow_unblock::deinit();
        crate::migrate_lobby::undo();
        warn!("host-migration: off");
        return;
    }
    info!("host-migration: on - match lobbies carry on under a new host, and a party follows its leader into a private match");
}
