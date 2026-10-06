//! The mouse for the overlay's panel. In play (outside the SMI map), the game reads the
//! mouse through DirectInput to turn the camera, and keeps the Windows cursor hidden and
//! clipped. The window messages the overlay holds back don't stop DirectInput, so while
//! the panel is open:
//!
//! - the game's DirectInput reads come back empty (no camera turning, no keys);
//! - its `ClipCursor` and `SetCursorPos` calls are held back, and the cursor is free within
//!   the game's window (the overlay draws its own cursor, as the game hides the system's);
//! - when the panel closes, the clip the game last asked for is put back.

use std::ffi::c_void;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use retour::static_detour;
use tracing::info;
use tracing::warn;
use windows::core::Interface as _;
use windows::core::HRESULT;
use windows::core::PCSTR;
use windows::Win32::Devices::HumanInterfaceDevice::DirectInput8Create;
use windows::Win32::Devices::HumanInterfaceDevice::GUID_SysMouse;
use windows::Win32::Devices::HumanInterfaceDevice::IDirectInput8A;
use windows::Win32::Devices::HumanInterfaceDevice::IDirectInput8W;
use windows::Win32::Devices::HumanInterfaceDevice::IDirectInputDevice8A;
use windows::Win32::Devices::HumanInterfaceDevice::IDirectInputDevice8W;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::System::LibraryLoader::GetProcAddress;
use windows::Win32::System::LibraryLoader::LoadLibraryA;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

/// DirectInput 8, as the game uses it (`DIRECTINPUT_VERSION`).
const DIRECTINPUT_VERSION: u32 = 0x0800;

type GetDeviceState = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> HRESULT;
type GetDeviceData = unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut u32, u32) -> HRESULT;

static_detour! {
    static ClipCursorHook: unsafe extern "system" fn(*const RECT) -> BOOL;
    static SetCursorPosHook: unsafe extern "system" fn(i32, i32) -> BOOL;
    static GetDeviceStateWHook: unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> HRESULT;
    static GetDeviceDataWHook: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut u32, u32) -> HRESULT;
    static GetDeviceStateAHook: unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> HRESULT;
    static GetDeviceDataAHook: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut u32, u32) -> HRESULT;
}

/// The panel is open: the overlay has the mouse and keys.
static OPEN: AtomicBool = AtomicBool::new(false);
/// The clip the game last asked for (None: none), put back when the panel closes.
static GAME_CLIP: Mutex<Option<RECT>> = Mutex::new(None);

/// Hooks the cursor calls and DirectInput's reads. Once, when the overlay starts.
pub fn init() {
    unsafe {
        if let Ok(user32) = LoadLibraryA(windows::core::s!("user32.dll")) {
            hook(&ClipCursorHook, GetProcAddress(user32, windows::core::s!("ClipCursor")), clip_cursor, "ClipCursor");
            hook(&SetCursorPosHook, GetProcAddress(user32, windows::core::s!("SetCursorPos")), set_cursor_pos, "SetCursorPos");
        }
        if let Err(e) = hook_direct_input() {
            warn!("Overlay: DirectInput isn't held back while the panel is open: {e}");
        }
    }
}

unsafe fn hook<T: retour::Function>(
    detour: &retour::StaticDetour<T>,
    target: Option<unsafe extern "system" fn() -> isize>,
    f: impl Fn<T::Arguments, Output = T::Output> + Send + 'static,
    name: &str,
) where
    <T as retour::Function>::Arguments: std::marker::Tuple,
{
    let Some(target) = target else {
        warn!("Overlay: {name} not found");
        return;
    };
    match detour.initialize(std::mem::transmute_copy(&target), f).and_then(|h| h.enable()) {
        Ok(()) => info!("Overlay: {name} hooked"),
        Err(e) => warn!("Overlay: couldn't hook {name}: {e:?}"),
    }
}

/// DirectInput's device methods, from a mouse of our own: every device of the game shares
/// them. The wide and narrow interfaces may have their own.
unsafe fn hook_direct_input() -> windows::core::Result<()> {
    let instance = GetModuleHandleA(PCSTR::null())?;
    let mut wide: Option<IDirectInput8W> = None;
    DirectInput8Create(instance, DIRECTINPUT_VERSION, &IDirectInput8W::IID, std::ptr::addr_of_mut!(wide).cast(), None)?;
    let mut narrow: Option<IDirectInput8A> = None;
    DirectInput8Create(instance, DIRECTINPUT_VERSION, &IDirectInput8A::IID, std::ptr::addr_of_mut!(narrow).cast(), None)?;
    let mut hooked: Vec<usize> = Vec::new();
    if let Some(wide) = wide {
        let mut device: Option<IDirectInputDevice8W> = None;
        wide.CreateDevice(&GUID_SysMouse, &mut device, None)?;
        if let Some(device) = device {
            let vtable = device.vtable();
            let (state, data): (GetDeviceState, GetDeviceData) = (std::mem::transmute(vtable.GetDeviceState), std::mem::transmute(vtable.GetDeviceData));
            enable(
                &GetDeviceStateWHook,
                state,
                |d, n, b| device_state(&GetDeviceStateWHook, d, n, b),
                &mut hooked,
                "GetDeviceState (W)",
            );
            enable(
                &GetDeviceDataWHook,
                data,
                |d, n, b, c, f| device_data(&GetDeviceDataWHook, d, n, b, c, f),
                &mut hooked,
                "GetDeviceData (W)",
            );
        }
    }
    if let Some(narrow) = narrow {
        let mut device: Option<IDirectInputDevice8A> = None;
        narrow.CreateDevice(&GUID_SysMouse, &mut device, None)?;
        if let Some(device) = device {
            let vtable = device.vtable();
            let (state, data): (GetDeviceState, GetDeviceData) = (std::mem::transmute(vtable.GetDeviceState), std::mem::transmute(vtable.GetDeviceData));
            enable(
                &GetDeviceStateAHook,
                state,
                |d, n, b| device_state(&GetDeviceStateAHook, d, n, b),
                &mut hooked,
                "GetDeviceState (A)",
            );
            enable(
                &GetDeviceDataAHook,
                data,
                |d, n, b, c, f| device_data(&GetDeviceDataAHook, d, n, b, c, f),
                &mut hooked,
                "GetDeviceData (A)",
            );
        }
    }
    Ok(())
}

/// Hooks `target` unless an earlier hook took the same function (the wide and narrow
/// interfaces can share one).
unsafe fn enable<T: retour::Function + Copy>(
    detour: &retour::StaticDetour<T>,
    target: T,
    f: impl Fn<T::Arguments, Output = T::Output> + Send + 'static,
    hooked: &mut Vec<usize>,
    name: &str,
) where
    <T as retour::Function>::Arguments: std::marker::Tuple,
{
    let address = target.to_ptr() as usize;
    if hooked.contains(&address) {
        return;
    }
    match detour.initialize(target, f).and_then(|h| h.enable()) {
        Ok(()) => {
            hooked.push(address);
            info!("Overlay: {name} hooked");
        }
        Err(e) => warn!("Overlay: couldn't hook {name}: {e:?}"),
    }
}

/// Whether the window in front is the game's: the overlay's key is read from the whole
/// keyboard, so it means the overlay only then, not in a browser over a windowed game.
pub fn game_in_front() -> bool {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
    pid == std::process::id()
}

/// The panel opened or closed (checked every frame).
pub fn set_open(open: bool) {
    if OPEN.swap(open, Ordering::SeqCst) == open {
        return;
    }
    unsafe {
        if open {
            // Free within the game's window, so a click can't land on another one.
            let window = GetForegroundWindow();
            let mut client = RECT::default();
            let free = if GetClientRect(window, &mut client).is_ok() {
                let mut top_left = POINT { x: client.left, y: client.top };
                let mut bottom_right = POINT {
                    x: client.right,
                    y: client.bottom,
                };
                let _ = ClientToScreen(window, &mut top_left);
                let _ = ClientToScreen(window, &mut bottom_right);
                Some(RECT {
                    left: top_left.x,
                    top: top_left.y,
                    right: bottom_right.x,
                    bottom: bottom_right.y,
                })
            } else {
                None
            };
            original_clip(free.as_ref());
        } else {
            // The game's own clip again, but only while it's in front: another program's
            // window mustn't be fenced in.
            let clip = *GAME_CLIP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            original_clip(if game_in_front() { clip.as_ref() } else { None });
        }
    }
}

unsafe fn original_clip(rect: Option<&RECT>) {
    if ClipCursorHook.is_enabled() {
        let _ = ClipCursorHook.call(rect.map_or(std::ptr::null(), std::ptr::from_ref));
    }
}

fn clip_cursor(rect: *const RECT) -> BOOL {
    // SAFETY: the game passes a RECT or null, as ClipCursor takes.
    let wanted = unsafe { rect.as_ref().copied() };
    *GAME_CLIP.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = wanted;
    if OPEN.load(Ordering::SeqCst) {
        return BOOL(1);
    }
    unsafe { ClipCursorHook.call(rect) }
}

fn set_cursor_pos(x: i32, y: i32) -> BOOL {
    if OPEN.load(Ordering::SeqCst) {
        return BOOL(1);
    }
    unsafe { SetCursorPosHook.call(x, y) }
}

/// The device's state, as if nothing moved or was pressed while the panel is open.
fn device_state(detour: &retour::StaticDetour<GetDeviceState>, device: *mut c_void, size: u32, buffer: *mut c_void) -> HRESULT {
    let result = unsafe { detour.call(device, size, buffer) };
    if OPEN.load(Ordering::SeqCst) && result.is_ok() && !buffer.is_null() {
        // SAFETY: the caller gave a buffer of `size` bytes for the state.
        unsafe { std::ptr::write_bytes(buffer.cast::<u8>(), 0, size as usize) };
    }
    result
}

/// The device's events, none while the panel is open (they're read, so they don't pile up
/// for when it closes).
fn device_data(detour: &retour::StaticDetour<GetDeviceData>, device: *mut c_void, size: u32, data: *mut c_void, count: *mut u32, flags: u32) -> HRESULT {
    let result = unsafe { detour.call(device, size, data, count, flags) };
    if OPEN.load(Ordering::SeqCst) && result.is_ok() && !count.is_null() {
        // SAFETY: `count` is the caller's in/out item count.
        unsafe { *count = 0 };
    }
    result
}
