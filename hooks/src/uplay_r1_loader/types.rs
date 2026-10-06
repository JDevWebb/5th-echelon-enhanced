use std::ffi::c_char;
use std::ffi::c_void;
use std::ffi::CString;
use std::mem::ManuallyDrop;
use std::ptr::null_mut;

use tracing::warn;

// https://github.com/Tron0xHex/uplay-r1-loader/
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UplaySave {
    pub slot_id: usize,
    pub name: String,
    pub size: usize,
}

#[repr(C)]
struct UplayKey {
    pub cd_key: *mut i8,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UplayFriend {
    pub id: String,
    pub username: String,
    pub is_online: bool,
    /// Session this friend is in; 0 means none. Where it lands in the C structure is decided
    /// by [`SessionField`].
    pub session_id: u32,
    /// Payload of that session, byte for byte as their game handed it over. Ends up in
    /// `FriendDetails.unknown4`.
    pub session_data: Vec<u8>,
    /// Principal id, for the session search. Where it lands is decided by [`PidField`].
    pub pid: u32,
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct UplayOverlapped {
    pub unk: u32,
    pub is_completed: u32,
    pub result: usize,
}

impl UplayOverlapped {
    pub fn set_result(&mut self, result: usize) {
        self.set_completed(true);
        self.result = result;
    }

    pub fn set_completed(&mut self, completed: bool) {
        self.is_completed = u32::from(completed);
    }

    pub fn set_success(&mut self) {
        self.set_result(0);
    }
}

#[allow(dead_code)]
#[repr(usize)]
pub enum UplayEventType {
    FriendsFriendListUpdated = 10000,
    FriendsFriendUpdated,
    FriendsGameInviteAccepted,
    FriendsMenuItemSelected,
    PartyMemberListChanged = 20000,
    PartyMemberUserDataUpdated,
    PartyLeaderChanged,
    PartyGameInviteReceived,
    PartyGameInviteAccepted,
    PartyMemberMenuItemSelected,
    PartyMemberUpdated,
    PartyInviteReceived,
    PartyMemberJoined,
    PartyMemberLeft,
    OverlayActivated = 30000,
    OverlayHidden,
    RewardRedeemed = 40000,
    UserAccountSharing = 50000,
    UserConnectionLost,
    UserConnectionRestored,
}

#[repr(C)]
pub struct UplayEvent {
    pub event_type: UplayEventType,
    pub unknown: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ListType {
    CdKeys,
    Saves,
    Friends,
}

#[repr(C)]
struct Save {
    pub slot_id: usize,
    pub name: *mut c_char,
    pub size: usize,
}

use hooks_config::PidField;
use hooks_config::SessionField;

#[repr(C)]
struct Friend {
    id: *mut i8,
    username: *mut c_char,
    unknown1: usize,
    unknown2: usize,
    details: *mut FriendDetails,
    unknown3: usize,
}

#[repr(C)]
struct FriendDetails {
    unknown1: usize,
    unknown2: *mut c_char,
    unknown3: usize,
    unknown4: *mut c_void,
}

#[repr(C)]
#[derive(Debug)]
pub struct List {
    pub count: usize,
    #[allow(clippy::struct_field_names)]
    pub list: *mut *mut c_void,
    pub ty: ListType,
}

macro_rules! valid_ptr {
    ($e:expr) => {
        anyhow::ensure!(!$e.is_null(), format!("{} is null", stringify!($e)));
        anyhow::ensure!($e.is_aligned(), format!("{} is not aligned", stringify!($e)));
    };
    ($e:expr, $ty:ty) => {{
        let tmp = $e.cast::<$ty>();
        valid_ptr!(tmp);
        tmp
    }};
}

impl List {
    fn into_vec<T>(self) -> anyhow::Result<Vec<Box<T>>> {
        valid_ptr!(self.list);
        let vec = unsafe { Vec::from_raw_parts(self.list.cast::<*mut T>(), self.count, self.count) };

        vec.into_iter()
            .map(|item| {
                valid_ptr!(item);
                unsafe { Ok(Box::from_raw(item)) }
            })
            .collect()
    }

    fn into_cdkeys(self) -> anyhow::Result<Vec<String>> {
        self.into_vec::<UplayKey>()?
            .into_iter()
            .map(|owned| {
                valid_ptr!(owned.cd_key);
                let s = unsafe { CString::from_raw(owned.cd_key) };
                s.into_string().map_err(anyhow::Error::from)
            })
            .collect()
    }

    fn into_saves(self) -> anyhow::Result<Vec<UplaySave>> {
        self.into_vec::<Save>()?
            .into_iter()
            .map(|item| {
                valid_ptr!(item.name);
                let s = unsafe { CString::from_raw(item.name) };
                Ok(UplaySave {
                    slot_id: item.slot_id,
                    name: s.into_string().map_err(anyhow::Error::from)?,
                    size: item.size,
                })
            })
            .collect()
    }

    /// Reads a friend list back. The entries are shared (see `interned_friend`),
    /// so only the array is freed, never the entries.
    fn into_friends(self) -> anyhow::Result<Vec<UplayFriend>> {
        valid_ptr!(self.list);
        let array = unsafe { Vec::from_raw_parts(self.list.cast::<*mut Friend>(), self.count, self.count) };
        array
            .into_iter()
            .map(|item| {
                valid_ptr!(item);
                let item = unsafe { &*item };
                valid_ptr!(item.id);
                valid_ptr!(item.username);
                let is_online = item.details.is_null() || unsafe { (*item.details).unknown1 } < 2;
                let id = unsafe { std::ffi::CStr::from_ptr(item.id) }.to_str()?.to_string();
                let username = unsafe { std::ffi::CStr::from_ptr(item.username) }.to_str()?.to_string();
                // Reverse direction: a list coming back from the game. Which field would hold
                // a session is exactly what is being determined here, so nothing is read out
                // of it - this path only needs id, name and online state.
                Ok(UplayFriend {
                    id,
                    username,
                    is_online,
                    session_id: 0,
                    session_data: Vec::new(),
                    pid: 0,
                })
            })
            .collect()
    }

    fn from_vec<T>(mut value: Vec<T>, ty: ListType) -> Self {
        value.shrink_to_fit();
        if value.capacity() > value.len() {
            warn!("Capacity still greater than len, memory leak will happen!");
        }
        let mut value = ManuallyDrop::new(value);
        let count = value.len();
        let list = value.as_mut_ptr().cast();
        List { count, list, ty }
    }
}

/// The most friends handed to the game at once.
pub const MAX_FRIENDS: usize = 500;
/// The size of the session payload the game reads from `FriendDetails.unknown4`.
pub const SESSION_DATA_SIZE: usize = 496;
/// Where the payload's fields are (`user.rs`, `SessionData`): the account id, a C string
/// in 0x80 bytes, and the length of the data after it, which has room for 0x164 bytes.
const ACCOUNT_ID_FIELD: std::ops::Range<usize> = 8..0x88;
const DATA_SIZE_FIELD: std::ops::Range<usize> = 0x1ec..0x1f0;
const DATA_CAPACITY: u32 = 0x164;

/// A friend's session payload as the game can take it: exactly [`SESSION_DATA_SIZE`]
/// bytes, with its account id ending inside its field and a data length that fits its
/// buffer, as the game's own payloads always have. Anything else becomes none: the friend
/// stays listed, only without a session to join.
pub fn session_data_for_game(data: Vec<u8>) -> Vec<u8> {
    if data.is_empty() {
        return data;
    }
    let well_formed =
        data.len() == SESSION_DATA_SIZE && data[ACCOUNT_ID_FIELD].contains(&0) && <[u8; 4]>::try_from(&data[DATA_SIZE_FIELD]).is_ok_and(|n| u32::from_le_bytes(n) <= DATA_CAPACITY);
    if well_formed {
        data
    } else {
        warn!("Leaving out a friend's session data the game can't take ({} bytes)", data.len());
        Vec::new()
    }
}

/// Longest id and username passed on (the game's own buffers are 64 and 256 bytes).
const MAX_ID: usize = 63;
const MAX_NAME: usize = 255;

impl UplayFriend {
    /// The entry with its session data checked (see [`session_data_for_game`]).
    fn normalised(mut self) -> Self {
        self.session_data = session_data_for_game(self.session_data);
        self
    }

    /// Whether the game can take this entry as it is.
    fn is_well_formed(&self) -> bool {
        let text = |s: &str, max: usize| !s.is_empty() && s.len() <= max && !s.bytes().any(|b| b == 0);
        let ok = text(&self.id, MAX_ID) && text(&self.username, MAX_NAME);
        if !ok {
            warn!("Leaving out a friend entry the game can't take ({} and {} bytes)", self.id.len(), self.username.len());
        }
        ok
    }
}

/// Where the friend fields go (see [`SessionField`] and [`PidField`]).
#[derive(Debug, Clone, Copy)]
struct FriendLayout {
    session_field: SessionField,
    share_data: bool,
    pid_field: PidField,
}

/// Friend entries handed to the game, by a hash of their content. The game keeps the
/// pointers beyond the call, so they can't be freed; an unchanged friend reuses its entry
/// instead of leaking a new one on every fetch.
static FRIEND_ENTRIES: std::sync::Mutex<Option<FriendEntries>> = std::sync::Mutex::new(None);

#[derive(Default)]
struct FriendEntries {
    table: std::collections::HashMap<[u8; 32], usize>,
    /// Entries made so far, all still allocated.
    made: usize,
}

/// Entries kept before the table starts over (the old ones stay allocated).
const MAX_FRIEND_ENTRIES: usize = 4096;
/// Entries made in all, at most: each stays allocated (under 1 KB, as strings and
/// session data are bounded), so a server sending ever-new friends can't grow the
/// game's memory past this. Real friend lists come nowhere near it.
const MAX_FRIEND_ENTRIES_MADE: usize = 65536;

/// What makes an entry: every field the game gets, each with its length, so no two
/// different entries share a key.
fn friend_key(f: &UplayFriend, layout: &FriendLayout) -> [u8; 32] {
    use sha2::Digest as _;
    let layout = format!("{layout:?}");
    let mut h = sha2::Sha256::new();
    for field in [f.id.as_bytes(), f.username.as_bytes(), f.session_data.as_slice(), layout.as_bytes()] {
        h.update((field.len() as u64).to_le_bytes());
        h.update(field);
    }
    h.update([u8::from(f.is_online)]);
    h.update(f.session_id.to_le_bytes());
    h.update(f.pid.to_le_bytes());
    h.finalize().into()
}

/// The game's entry for `f`, made once per content. None once
/// [`MAX_FRIEND_ENTRIES_MADE`] are made: the friend is left out.
fn interned_friend(f: &UplayFriend, layout: &FriendLayout) -> Option<*mut Friend> {
    let key = friend_key(f, layout);
    let mut guard = FRIEND_ENTRIES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let entries = guard.get_or_insert_with(FriendEntries::default);
    if let Some(&ptr) = entries.table.get(&key) {
        return Some(ptr as *mut Friend);
    }
    if entries.made >= MAX_FRIEND_ENTRIES_MADE {
        if entries.made == MAX_FRIEND_ENTRIES_MADE {
            warn!("{MAX_FRIEND_ENTRIES_MADE} friend entries made; changed friends are left out from now on");
            entries.made += 1;
        }
        return None;
    }
    if entries.table.len() >= MAX_FRIEND_ENTRIES {
        entries.table.clear();
    }
    let ptr = Box::into_raw(Box::new(new_friend(f, layout)));
    entries.table.insert(key, ptr as usize);
    entries.made += 1;
    Some(ptr)
}

fn new_friend(f: &UplayFriend, layout: &FriendLayout) -> Friend {
    // Where the friend's session goes is decided at runtime, see [`SessionField`].
    // Everything not selected keeps its previous value, so a wrong guess behaves exactly
    // like before.
    let session = f.session_id as usize;
    // The friend's principal id - without it the game has nothing to look their session
    // up by.
    let pid = f.pid as usize;
    let (session_field, pid_field) = (layout.session_field, layout.pid_field);
    // Checked by `is_well_formed`: no NUL inside.
    let c = |s: &str| CString::new(s).unwrap_or_default().into_raw();
    Friend {
        id: c(&f.id),
        username: c(&f.username),
        unknown1: if session_field == SessionField::FriendUnknown1 { session } else { 0 },
        unknown2: if session_field == SessionField::FriendUnknown2 {
            session
        } else if pid_field == PidField::FriendUnknown2 {
            pid
        } else {
            0
        },
        details: Box::into_raw(Box::new(FriendDetails {
            unknown1: if f.is_online { 0 } else { 2 }, // >1 is offline?
            unknown2: if session_field == SessionField::DetailsUnknown2Str && session != 0 {
                c(&session.to_string())
            } else {
                null_mut() // another string?
            },
            unknown3: if session_field == SessionField::DetailsUnknown3 {
                session
            } else if pid_field == PidField::DetailsUnknown3 {
                pid
            } else {
                0
            },
            // The session payload of that friend, byte for byte as their game handed it
            // over. Without it the other side does get past the lookup, but opens a session
            // of its own instead of joining - the id alone is not enough to enter one. The
            // game reads exactly 496 bytes from it: anything else is left out, or it would
            // read past the end.
            unknown4: if layout.share_data && f.session_data.len() == SESSION_DATA_SIZE {
                Box::into_raw(f.session_data.clone().into_boxed_slice()).cast::<c_void>()
            } else {
                null_mut() // only used when fetching, but not after??
            },
        })),
        unknown3: if session_field == SessionField::FriendUnknown3 {
            session
        } else if pid_field == PidField::FriendUnknown3 {
            pid
        } else {
            0 // must be 0?
        },
    }
}

/// Lists this DLL allocated and handed to the game (by address), the only
/// ones [`release`] may free.
static OWNED_LISTS: std::sync::Mutex<Option<std::collections::HashSet<usize>>> = std::sync::Mutex::new(None);

/// Hands `list` to the game as a pointer it later gives back to `UPLAY_Release`.
pub fn into_game(list: UplayList) -> *mut List {
    let ptr = Box::into_raw(Box::new(List::from(list)));
    OWNED_LISTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(std::collections::HashSet::new)
        .insert(ptr as usize);
    ptr
}

/// Frees a list from [`into_game`]. Anything else (a list struct of the
/// game's own, like the friend list's) is left alone.
///
/// # Safety
/// `ptr` must come from the game's `UPLAY_Release` call.
pub unsafe fn release(ptr: *mut List) -> bool {
    let owned = OWNED_LISTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .is_some_and(|set| set.remove(&(ptr as usize)));
    if !owned {
        tracing::debug!("UPLAY_Release of a list this DLL didn't allocate; left alone");
        return false;
    }
    let list = *Box::from_raw(ptr);
    match UplayList::try_from(list) {
        Ok(list) => drop(list),
        Err(e) => warn!("Couldn't release a list: {e}"),
    }
    true
}

impl From<UplayList> for List {
    fn from(value: UplayList) -> Self {
        match value {
            UplayList::CdKeys(keys) => {
                // A key with a NUL inside can't be handed over; it's skipped, not a crash.
                let keys = keys
                    .into_iter()
                    .filter_map(|k| CString::new(k).ok())
                    .map(CString::into_raw)
                    .map(|cd_key| UplayKey { cd_key })
                    .map(Box::new)
                    .map(Box::into_raw)
                    .collect::<Vec<*mut UplayKey>>();
                List::from_vec(keys, ListType::CdKeys)
            }
            UplayList::Saves(saves) => {
                let saves = saves
                    .into_iter()
                    .filter_map(|s| {
                        Some(Save {
                            slot_id: s.slot_id,
                            name: CString::new(s.name).ok()?.into_raw(),
                            size: s.size,
                        })
                    })
                    .map(Box::new)
                    .map(Box::into_raw)
                    .collect::<Vec<*mut Save>>();
                List::from_vec(saves, ListType::Saves)
            }
            UplayList::Friends(friends) => {
                let cfg = crate::config::get();
                let layout = FriendLayout {
                    session_field: cfg.map(|c| c.session_field).unwrap_or_default(),
                    // Passing the payload along can be switched off separately, so that a
                    // counter-test does not need a rebuild.
                    share_data: cfg.is_none_or(|c| c.share_session_data),
                    pid_field: cfg.map(|c| c.pid_field).unwrap_or_default(),
                };
                // Whatever the server sends, the game gets a bounded list of well-formed
                // entries: no NUL inside a string (it would end it early), no overlong
                // strings, and session data exactly the 496 bytes the game reads, or none.
                // The data is checked before anything is made of the entry.
                let friends = friends
                    .into_iter()
                    .filter(UplayFriend::is_well_formed)
                    .take(MAX_FRIENDS)
                    .map(UplayFriend::normalised)
                    .filter_map(|f| interned_friend(&f, &layout))
                    .collect::<Vec<*mut Friend>>();
                List::from_vec(friends, ListType::Friends)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum UplayList {
    CdKeys(Vec<String>),
    Saves(Vec<UplaySave>),
    Friends(Vec<UplayFriend>),
}

impl TryFrom<List> for UplayList {
    type Error = anyhow::Error;

    fn try_from(value: List) -> ::std::result::Result<Self, Self::Error> {
        let res = match value.ty {
            ListType::CdKeys => UplayList::CdKeys(value.into_cdkeys()?),
            ListType::Saves => UplayList::Saves(value.into_saves()?),
            ListType::Friends => UplayList::Friends(value.into_friends()?),
        };
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdkeys_are_same_after_conversion() {
        let expected = UplayList::CdKeys(["1234", "ABCD", "foo", "bar"].into_iter().map(String::from).collect());

        let converted: List = expected.clone().into();
        assert!(matches!(expected, UplayList::CdKeys(ref keys) if keys.len() == converted.count));
        assert_eq!(converted.ty, ListType::CdKeys);

        let converted_back: UplayList = converted.try_into().unwrap();
        assert_eq!(converted_back, expected);
    }

    #[test]
    fn saves_are_same_after_conversion() {
        let expected = UplayList::Saves(vec![
            UplaySave {
                slot_id: 1,
                name: "Save 1".into(),
                size: 123,
            },
            UplaySave {
                slot_id: 2,
                name: "Save 2".into(),
                size: 123,
            },
            UplaySave {
                slot_id: 3,
                name: "Save 3".into(),
                size: 123,
            },
        ]);

        let converted: List = expected.clone().into();
        assert!(matches!(expected, UplayList::Saves(ref saves) if saves.len() == converted.count));
        assert_eq!(converted.ty, ListType::Saves);

        let converted_back: UplayList = converted.try_into().unwrap();
        assert_eq!(converted_back, expected);
    }

    #[test]
    fn friends_are_same_after_conversion() {
        let expected = UplayList::Friends(vec![
            UplayFriend {
                id: "ID1".into(),
                username: "User 1".into(),
                is_online: true,
                session_id: 0,
                session_data: Vec::new(),
                pid: 0,
            },
            UplayFriend {
                id: "ID2".into(),
                username: "User 2".into(),
                is_online: false,
                session_id: 0,
                session_data: Vec::new(),
                pid: 0,
            },
        ]);

        let converted: List = expected.clone().into();
        assert!(matches!(expected, UplayList::Friends(ref friends) if friends.len() == converted.count));
        assert_eq!(converted.ty, ListType::Friends);

        let converted_back: UplayList = converted.try_into().unwrap();
        assert_eq!(converted_back, expected);
    }

    #[test]
    fn friends_the_game_cant_take_are_left_out() {
        // A payload laid out as the game's: an account id with its NUL, a data length that fits.
        let payload = |len: usize| {
            let mut data = vec![0; len];
            if len == SESSION_DATA_SIZE {
                data[8..12].copy_from_slice(b"Kiwi");
                data[0x1ec..0x1f0].copy_from_slice(&16u32.to_le_bytes());
            }
            data
        };
        let friend = |id: &str, name: &str, data: usize| UplayFriend {
            id: id.into(),
            username: name.into(),
            is_online: true,
            session_id: 7,
            session_data: payload(data),
            pid: 9,
        };
        let list = UplayList::Friends(vec![
            friend("ok", "Fine", 496),
            friend("nul\0inside", "Bad", 0),
            friend(&"x".repeat(64), "LongId", 0),
            friend("short", "ShortData", 3),
        ]);
        let converted: List = list.into();
        assert_eq!(converted.count, 2, "the NUL and the overlong id are left out");
        let array = unsafe { std::slice::from_raw_parts(converted.list.cast::<*mut Friend>(), converted.count) };
        let data = |i: usize| unsafe { (*(*array[i]).details).unknown4 };
        assert!(!data(0).is_null(), "496 bytes are passed on");
        assert!(data(1).is_null(), "a short payload isn't: the game would read past it");
        // The same friend again reuses its entry.
        let again: List = UplayList::Friends(vec![friend("ok", "Fine", 496)]).into();
        let again = unsafe { std::slice::from_raw_parts(again.list.cast::<*mut Friend>(), 1) };
        assert_eq!(again[0], array[0]);
    }

    #[test]
    fn session_data_is_checked_against_the_games_layout() {
        let mut data = vec![0u8; SESSION_DATA_SIZE];
        data[0x1ec..0x1f0].copy_from_slice(&0x164u32.to_le_bytes());
        assert_eq!(session_data_for_game(data.clone()).len(), SESSION_DATA_SIZE);
        let mut too_long = data.clone();
        too_long[0x1ec..0x1f0].copy_from_slice(&0x165u32.to_le_bytes());
        assert!(session_data_for_game(too_long).is_empty(), "a data length past its buffer");
        let mut no_nul = data.clone();
        no_nul[8..0x88].fill(b'A');
        assert!(session_data_for_game(no_nul).is_empty(), "an account id without its end");
        assert!(session_data_for_game(vec![0; 4 << 20]).is_empty(), "megabytes, dropped before keying");
        assert!(session_data_for_game(Vec::new()).is_empty());
    }

    #[test]
    fn only_our_own_lists_are_released() {
        let ptr = into_game(UplayList::CdKeys(vec!["A".into()]));
        assert!(unsafe { release(ptr) });
        assert!(!unsafe { release(ptr) }, "twice is a no-op");
        let mut foreign = List::from_vec(Vec::<*mut Friend>::new(), ListType::Friends);
        assert!(!unsafe { release(&raw mut foreign) });
    }
}
