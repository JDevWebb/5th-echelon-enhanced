use hooks_proc::forwardable_export;
use tracing::error;
use tracing::info;

use super::List;
use super::UplayList;
use super::UplayOverlapped;
use crate::config::get;

/// The game's buffers for these strings, NUL included (from its one caller of all
/// three: 256, 64 and 64 bytes on the stack). Nothing may write more.
const USERNAME_BUFFER: usize = 256;
const PASSWORD_BUFFER: usize = 64;
const ACCOUNT_ID_BUFFER: usize = 64;

/// Copies `value` and its NUL into the game's `buffer` of `capacity` bytes.
/// A value that doesn't fit whole, or has a NUL inside, is refused (a cut
/// password or id would only fail later, less clearly).
unsafe fn copy_c_string(buffer: *mut u8, value: &str, capacity: usize, what: &str) -> bool {
    if buffer.is_null() {
        return false;
    }
    if value.len() >= capacity || value.bytes().any(|b| b == 0) {
        error!("The {what} doesn't fit the game's {capacity}-byte buffer; refusing it");
        return false;
    }
    buffer.copy_from_nonoverlapping(value.as_ptr(), value.len());
    *buffer.add(value.len()) = 0;
    true
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_USER_ClearGameSession() -> bool {
    // Counterpart to UPLAY_USER_SetGameSession: the player left their session and should no
    // longer show up as joinable in anybody's friend list.
    crate::api::announce_game_session(None, false, &[]);
    true
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_USER_GetAccountId(buffer: *mut u8) -> bool {
    // The server's word for who we are, once signed in; the settings file's
    // until then (they're the same for accounts the launcher made).
    let account_id = crate::api::account_id().unwrap_or_else(|| cfg.user.account_id.clone());
    copy_c_string(buffer, &account_id, ACCOUNT_ID_BUFFER, "account id")
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_USER_GetCdKeys(cd_keys_list: *mut *mut List, overlapped: *mut UplayOverlapped) -> bool {
    let list = UplayList::CdKeys(cfg.user.cd_keys.clone());
    *cd_keys_list = super::types::into_game(list);

    if !overlapped.is_null() {
        (*overlapped).unk = 0;
        (*overlapped).is_completed = 1;
        (*overlapped).result = 0;
    }
    true
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_USER_GetEmail(out_email: *mut i8) -> bool {
    false
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_USER_GetPassword(buffer: *mut u8) -> bool {
    let Some(cfg) = get() else {
        error!("Config not loaded!");
        return false;
    };
    copy_c_string(buffer, &cfg.user.secret().unwrap_or_default(), PASSWORD_BUFFER, "password")
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_USER_GetUsername(buffer: *mut u8) -> bool {
    copy_c_string(buffer, &cfg.user.username, USERNAME_BUFFER, "username")
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_USER_IsConnected() -> bool {
    true
}

#[repr(C)]
struct SessionData {
    unknown1: u32,
    checksum: u32,
    account_id: [u8; 0x80],
    some_data: [u8; 0x164],
    some_data_size: u32,
}

impl std::fmt::Debug for SessionData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionData")
            .field("unknown1", &self.unknown1)
            .field("checksum", &self.checksum)
            .field("account_id", &std::ffi::CStr::from_bytes_until_nul(&self.account_id))
            .field("some_data", &&self.some_data[..self.some_data_size as usize])
            .field("some_data_size", &self.some_data_size)
            .finish()
    }
}

#[derive(Debug)]
#[repr(C)]
struct SessionDataWrapper<'a> {
    data: &'a SessionData,
    size: u32,
}

#[forwardable_export(always_call)]
unsafe extern "cdecl" fn UPLAY_USER_SetGameSession(game_session_identifier: *mut (), flags: usize, session_data: &SessionDataWrapper<'_>, invite_only: bool) -> bool {
    if crate::hooks::is_modded() {
        crate::show_msgbox("Fatal error. Game modified", "MOD");
        std::process::exit(1);
    }

    // The game announces here which session it is in - genuine Uplay would pass that on to
    // Ubisoft's presence service, from where friends' clients read it back. Dropping it is
    // what breaks invitations: when the other side accepts, the game looks for the inviter's
    // session in its friend list, finds nothing and gives up with
    // "Failed to find party session for invite."
    //
    // The identifier is not a pointer despite its type - the game passes small numbers that
    // match the session ids of the dedicated server (0x33 = session 51).
    #[allow(clippy::cast_possible_truncation)]
    let session_id = game_session_identifier as usize as u32;

    // The payload has to travel along: the id alone gets an accepted invitation past the
    // lookup, but the other side then opens a session of its own instead of joining. The
    // block is passed through byte for byte - nothing in it needs interpreting, and `size`
    // comes from the game itself (496 bytes).
    // Guard against a bogus size before reading that much memory.
    const MAX_SESSION_DATA: usize = 4096;
    let size = session_data.size as usize;
    let payload = if size <= MAX_SESSION_DATA && !std::ptr::from_ref(session_data.data).is_null() {
        std::slice::from_raw_parts(std::ptr::from_ref(session_data.data).cast::<u8>(), size)
    } else {
        error!("Ignoring session data of implausible size {size}");
        &[]
    };

    // On a background thread: this is the game's thread.
    info!("Announcing session {session_id} (invite_only={invite_only})");
    crate::api::announce_game_session(Some(session_id), invite_only, payload);
    true
}
