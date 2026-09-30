//! Who may invite whom, for the game's own protocol as for the API: the
//! server's `[friends] mode`, and blocks. Set once at start.

use std::sync::OnceLock;

use crate::config::FriendsMode;
use crate::storage::Relation;
use crate::storage::Storage;

static MODE: OnceLock<FriendsMode> = OnceLock::new();

pub fn set_mode(mode: FriendsMode) {
    let _ = MODE.set(mode);
}

pub fn mode() -> FriendsMode {
    MODE.get().copied().unwrap_or_default()
}

/// Whether `from` may invite (or pull into a session) `to`: never across a
/// block, and only friends in the "mutual" mode.
pub async fn may_invite(storage: &Storage, from: u32, to: u32) -> bool {
    if from == to {
        return true;
    }
    match storage.relation(from, to).await {
        Ok(Relation::Blocked | Relation::BlockedBy) => false,
        Ok(Relation::Friend) => true,
        Ok(_) => mode() == FriendsMode::Everyone,
        Err(_) => false,
    }
}

/// [`may_invite`] from the game's (synchronous) services.
pub fn may_invite_blocking(storage: &Storage, from: u32, to: u32) -> bool {
    crate::storage::run(may_invite(storage, from, to)).unwrap_or(false)
}
