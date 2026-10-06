use std::ffi::c_char;
use std::sync::mpsc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use hooks_proc::forwardable_export;
use tracing::error;
use tracing::info;
use tracing::warn;
use windows::core::s;
use windows::core::PCSTR;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::GetProcAddress;
use windows::Win32::System::LibraryLoader::LoadLibraryA;
use windows::Win32::UI::WindowsAndMessaging::MessageBoxA;
use windows::Win32::UI::WindowsAndMessaging::MB_OK;

mod ach;
mod avatar;
mod friends;
mod overlay;
mod party;
mod presence;
mod save;
mod types;
mod user;
mod win;

pub(crate) use types::session_data_for_game;
use types::List;
use types::UplayFriend;
use types::UplayList;
use types::UplayOverlapped;
use types::UplaySave;
pub(crate) use types::MAX_FRIENDS;

use self::types::UplayEvent;
use self::types::UplayEventType;

type Result<T> = std::result::Result<T, anyhow::Error>;

fn get_proc(name: PCSTR) -> Option<unsafe extern "system" fn() -> isize> {
    static DLL_HANDLE: OnceLock<windows::core::Result<HMODULE>> = OnceLock::new();
    let handle = DLL_HANDLE
        .get_or_init(|| unsafe { LoadLibraryA(s!("uplay_r1_loader.orig.dll")) })
        .as_ref()
        .inspect_err(|&e| unsafe {
            error!("Library loading error: {e:?}");
            let mut s = format!("{e:?}");
            let v = s.as_mut_vec();
            v.push(b'\0');
            MessageBoxA(None, PCSTR(v.as_ptr()), s!("Error"), MB_OK);
        })
        .inspect(|_l| {
            info!("Library loaded");
        })
        .unwrap();
    unsafe { GetProcAddress(*handle, name) }
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_GetLastError(out_error_string: *mut *const c_char) -> bool {
    false
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum Event {
    UserAccountSharing,
    FriendsGameInviteAccepted(String),
    PartyGameInviteAccepted(String),
    /// The friend list changed: the game fetches it again (`UPLAY_FRIENDS_GetFriendList`).
    FriendsListUpdated,
}

pub static EVENTS: OnceLock<Mutex<mpsc::Receiver<Event>>> = OnceLock::new();
/// The sending side of [`EVENTS`], for events that don't come from the overlay's render loop.
pub static EVENT_SENDER: OnceLock<Mutex<mpsc::Sender<Event>>> = OnceLock::new();

/// Queues `event` for the game's next `UPLAY_GetNextEvent`. Dropped when the overlay (which
/// sets the queue up) isn't running.
pub fn queue_event(event: Event) {
    if let Some(tx) = EVENT_SENDER.get() {
        let _ = tx.lock().unwrap_or_else(std::sync::PoisonError::into_inner).send(event);
    }
}

unsafe fn into_friend_invite_accepted(event: *mut UplayEvent, ubi_name: String) {
    #![allow(static_mut_refs)]
    // TODO user better names

    #[repr(C)]
    struct Bar {
        unknown: usize,
        unknown1: usize,
        username: [u8; 128],
    }

    #[repr(C)]
    struct Foo {
        unknown: [usize; 2],
        bar: *const Bar,
        value496: usize,
    }

    #[repr(C)]
    struct FriendAccepted {
        foo: *const Foo,
    }

    static mut BAR: Bar = Bar {
        unknown: 1,
        unknown1: 0,
        username: [0u8; 128],
    };
    static mut FOO: Foo = Foo {
        unknown: [0usize; 2],
        bar: std::ptr::addr_of!(BAR),
        value496: 496,
    };

    static mut FRIEND_ACCEPTED: FriendAccepted = FriendAccepted { foo: std::ptr::addr_of!(FOO) };

    (*event).event_type = UplayEventType::FriendsGameInviteAccepted;
    copy_terminated(&mut *std::ptr::addr_of_mut!(BAR.username), &ubi_name);
    (*event).unknown = std::ptr::addr_of!(FRIEND_ACCEPTED) as usize;
}

/// Copies `text` into `buf` with its NUL, cut to fit (at most `buf.len() - 1`
/// bytes, never inside a character). Returns the bytes written, NUL included.
fn copy_terminated(buf: &mut [u8], text: &str) -> usize {
    let mut len = text.len().min(buf.len().saturating_sub(1));
    while !text.is_char_boundary(len) {
        len -= 1;
    }
    let text = &text.as_bytes()[..len];
    let len = text.iter().position(|&b| b == 0).unwrap_or(len);
    buf[..len].copy_from_slice(&text[..len]);
    buf[len] = 0;
    len + 1
}

unsafe fn into_party_invite_accepted(event: *mut UplayEvent, ubi_name: String) {
    #![allow(static_mut_refs)]
    // TODO user better names

    #[repr(C)]
    struct PartyAccepted {
        unknown: usize,
        username: *const Foo,
    }

    #[repr(C)]
    struct Foo {
        unknown: [usize; 2],
        username: *const [u8; 128],
        length: usize,
    }

    static mut BAR: [u8; 128] = [0u8; 128];
    static mut FOO: Foo = Foo {
        unknown: [0usize; 2],
        username: std::ptr::addr_of!(BAR),
        length: 0usize,
    };

    static mut PARTY_ACCEPTED: PartyAccepted = PartyAccepted {
        unknown: 0,
        username: std::ptr::addr_of!(FOO),
    };

    (*event).event_type = UplayEventType::PartyGameInviteAccepted;
    FOO.length = copy_terminated(&mut *std::ptr::addr_of_mut!(BAR), &ubi_name);
    warn!(
        "&PARTY_ACCEPTED = {:?} &FOO = {:?} &BAR = {:?}",
        std::ptr::addr_of!(PARTY_ACCEPTED),
        std::ptr::addr_of!(FOO),
        std::ptr::addr_of!(BAR)
    );
    (*event).unknown = std::ptr::addr_of!(PARTY_ACCEPTED) as usize;
}

#[forwardable_export(log = false)]
unsafe extern "cdecl" fn UPLAY_GetNextEvent(event: *mut UplayEvent) -> bool {
    if event.is_null() {
        return false;
    }

    if let Some(evt) = EVENTS
        .get()
        .map(Mutex::lock)
        .map(std::result::Result::unwrap)
        .as_deref()
        .map(mpsc::Receiver::try_recv)
        .and_then(std::result::Result::ok)
    {
        info!("New event {evt:?}");
        match evt {
            Event::UserAccountSharing => {
                (*event).event_type = UplayEventType::UserAccountSharing;
            }
            Event::FriendsGameInviteAccepted(user) => into_friend_invite_accepted(event, user),
            Event::PartyGameInviteAccepted(user) => into_party_invite_accepted(event, user),
            Event::FriendsListUpdated => {
                // The game's event loop (FUN_008842b0 in the DX11 exe) answers 10000 and 10001
                // alike, without reading the payload: it sets its presence goal
                // eGoal_FriendList, whose state (nsOnlinePresence::StateFetchFriends) calls
                // GetFriendList again.
                (*event).event_type = UplayEventType::FriendsFriendListUpdated;
                (*event).unknown = 0;
            }
        }
        // (*event).event_type =
        true
    } else {
        false
    }
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_GetOverlappedOperationResult_(overlapped: *mut UplayOverlapped, result: *mut usize) -> bool {
    let overlapped = unsafe { overlapped.as_ref() };
    let result = unsafe { result.as_mut() };
    if let Some((overlapped, result)) = overlapped.filter(|o| o.is_completed != 0).zip(result) {
        *result = overlapped.result;
        true
    } else {
        false
    }
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_GetOverlappedOperationResult(overlapped: *mut UplayOverlapped, result: *mut usize) -> bool {
    let overlapped = unsafe { overlapped.as_ref() };
    let result = unsafe { result.as_mut() };
    if let Some((overlapped, result)) = overlapped.filter(|o| o.is_completed != 0).zip(result) {
        *result = overlapped.result;
        true
    } else {
        false
    }
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_HasOverlappedOperationCompleted(overlapped: *mut UplayOverlapped) -> bool {
    let overlapped = unsafe { overlapped.as_ref() };
    overlapped.is_some_and(|o| o.is_completed != 0)
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_Quit() -> bool {
    // The game is exiting normally: take down the router's port mapping now, not while the
    // DLL unloads.
    crate::hooks::remove_port_mapping();
    false
}

#[forwardable_export]
unsafe extern "cdecl" fn UPLAY_Release(ptr: *mut List) -> bool {
    // Only lists this DLL made are freed: the friend list, for one, is the game's own struct.
    if !ptr.is_null() {
        types::release(ptr);
    }
    true
}

#[forwardable_export(always_call)]
unsafe extern "cdecl" fn UPLAY_Startup(uplay_id: usize, game_version: usize, language_country_code_utf8: *const c_char) -> isize {
    if let Some(config) = crate::config::get() {
        if config.enable_overlay {
            info!("Initializing overlay");
            let (tx, rx) = crossbeam_channel::unbounded();
            // needs to be done in a separate thread, otherwise it'll not work
            std::thread::Builder::new()
                .name(String::from("overlay-thread"))
                .spawn(|| {
                    let Some(engine) = crate::overlay::Engine::detect() else {
                        error!("Couldn't identify DX version");
                        return;
                    };
                    info!("Detected DX version {engine:?}");
                    // TODO: blocks game if running in fullscreen mode. Waiting 10s seems to do the trick
                    std::thread::sleep(std::time::Duration::from_secs(10));
                    if let Err(err) = crate::overlay::init(engine, rx) {
                        error!("Couldn't initialize overlay: {err}");
                    } else {
                        info!("Overlay initialized");
                    }
                })
                .unwrap();

            std::thread::Builder::new()
                .name(String::from("updates-thread"))
                .spawn(move || {
                    crate::api::runtime().unwrap().block_on(async {
                        let mut failures = 0;
                        loop {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            let event: Option<std::result::Result<_, _>> = crate::api::event()
                                .await
                                .map(|resp| {
                                    // A friend request or an accepted one: a notice in the overlay.
                                    if let Some(friend) = resp.friend.as_ref() {
                                        crate::community::friend_event(friend);
                                    }
                                    // An answer from support: a notice too.
                                    crate::community::support_unread(resp.support_unread);
                                    resp.invite
                                })
                                .transpose();
                            if let Some(invite) = event {
                                if invite.is_err() {
                                    failures += 1;
                                } else {
                                    failures = 0;
                                }
                                let invite = invite.map(Some);
                                if tx.send(invite).is_err() {
                                    return;
                                }
                                if failures > 0 && failures % 10 == 0 && crate::api::relogin().await {
                                    // signal successful relogin
                                    let _ = tx.send(Ok(None));
                                }
                            }
                        }
                    });
                })
                .unwrap();
        } else {
            info!("Overlay is disabled");
        }
    } else {
        warn!("config not loaded!");
    }
    0 // 0 = all good, 1 = error occured, 2 = ??? (), 3 = ??? (potentially offline mode)
}

#[forwardable_export(log = false)]
unsafe extern "cdecl" fn UPLAY_Update() -> bool {
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn invite_names_always_fit_and_end() {
        let mut buf = [0xffu8; 8];
        assert_eq!(super::copy_terminated(&mut buf, "abc"), 4);
        assert_eq!(&buf[..4], b"abc\0");
        assert_eq!(super::copy_terminated(&mut buf, "much too long"), 8);
        assert_eq!(buf[7], 0);
        assert_eq!(super::copy_terminated(&mut buf, "a\0b"), 2, "stops at a NUL");
        assert_eq!(super::copy_terminated(&mut [0u8; 3], "éé"), 3, "not inside a character");
    }
}
