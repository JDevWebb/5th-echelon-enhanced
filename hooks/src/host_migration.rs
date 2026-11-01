//! The beta "host migration" (Settings › Beta features; `[Beta] HostMigration` in `uplay.toml`):
//! a match lobby carries on under a new host when its host leaves (`migrate_lobby`), a party's
//! leader invited into a private match takes its party along (`party_follow`), and a member's
//! game left waiting on a stale follow is set going again (`follow_unblock`). The server's side
//! (taking a room over, letting the party in, bringing it back) is in `dedicated_server`'s
//! `game_session.rs` and `escorts.rs`. Background in `docs/architecture/host-and-party-migration.md`.
//!
//! The three go in together or not at all: a lobby that migrates without the follow fixes, or
//! a party taken along without them, leaves players worse off than the game's own way. So
//! everything is found first (`hooks_addresses::host_migration`, by signature: the DX11 build;
//! the DX9 one lacks pieces of it), and a part that doesn't go in takes the others out again.

use tracing::info;
use tracing::warn;

/// Installs the three, after the crash fixes (`migration_fix` may already hook the UI facade's
/// tick, which `follow_unblock` then runs from).
pub unsafe fn install() {
    let Some(image) = crate::game_image() else {
        warn!("host-migration (beta): the game's sections weren't found -> off");
        return;
    };
    let sites = match hooks_addresses::host_migration::locate(&image) {
        Ok(sites) => sites,
        Err(why) => {
            warn!("host-migration (beta): {why} -> off (the DX11 build only)");
            return;
        }
    };
    if !crate::migrate_lobby::install(&sites) {
        warn!("host-migration (beta): off");
        return;
    }
    if !crate::follow_unblock::install(&sites, crate::migration_fix::tick_hooked()) {
        crate::migrate_lobby::undo();
        warn!("host-migration (beta): off");
        return;
    }
    if !crate::party_follow::install(&sites) {
        crate::follow_unblock::deinit();
        crate::migrate_lobby::undo();
        warn!("host-migration (beta): off");
        return;
    }
    info!("host-migration (beta): on - match lobbies carry on under a new host, and a party follows its leader into a private match");
}
