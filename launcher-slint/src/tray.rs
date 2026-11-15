//! The launcher's icon in the Windows tray, when it starts with Windows in the tray (Settings ›
//! Display › Starting, autostart.rs): no window until the icon is clicked; its menu opens the
//! launcher or quits it. A hidden message window on a thread of its own receives the icon's
//! clicks, and hands them to the launcher's event loop.

use std::sync::atomic::AtomicIsize;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

use windows::core::w;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::Shell_NotifyIconW;
use windows::Win32::UI::Shell::NIF_ICON;
use windows::Win32::UI::Shell::NIF_MESSAGE;
use windows::Win32::UI::Shell::NIF_TIP;
use windows::Win32::UI::Shell::NIM_ADD;
use windows::Win32::UI::Shell::NIM_DELETE;
use windows::Win32::UI::Shell::NOTIFYICONDATAW;
use windows::Win32::UI::WindowsAndMessaging as wm;

/// What the icon asks of the launcher.
#[derive(Debug, Clone, Copy)]
pub enum Asked {
    Open,
    Quit,
}

/// The launcher's answer to the icon (on its event loop).
static ASKED: OnceLock<Box<dyn Fn(Asked) + Send + Sync>> = OnceLock::new();
/// The hidden window, to take the icon away when the launcher closes.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// Explorer's "the taskbar came back" message: the icon is added again then.
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

/// The icon's own message to the hidden window.
const ICON_MESSAGE: u32 = wm::WM_APP + 1;
/// Another start of the launcher asking this one to show its window.
const OPEN_MESSAGE: u32 = wm::WM_APP + 2;
/// The hidden window's class, which another start finds it by.
const CLASS: PCWSTR = w!("FifthEchelonTray");
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;

/// Puts the icon in the tray; `asked` runs on the thread that gets the icon's clicks.
pub fn start(asked: impl Fn(Asked) + Send + Sync + 'static) {
    if ASKED.set(Box::new(asked)).is_err() {
        return;
    }
    let started = std::thread::Builder::new().name("tray".into()).spawn(|| {
        if let Err(e) = run() {
            tracing::warn!("Tray: no icon ({e})");
        }
    });
    if let Err(e) = started {
        tracing::warn!("Tray: no icon ({e})");
    }
}

/// Whether a launcher already in the tray took this start over: asked to show its window, so
/// this one ends (opened from its shortcut while it waited in the tray, it would be a second).
pub fn hand_over_to_running() -> bool {
    // SAFETY: looks for the tray's window by its class, and posts it a message.
    unsafe {
        let hwnd = wm::FindWindowW(CLASS, None);
        hwnd.0 != 0 && wm::PostMessageW(hwnd, OPEN_MESSAGE, WPARAM(0), LPARAM(0)).is_ok()
    }
}

/// Takes the icon away (the launcher is closing).
pub fn stop() {
    let hwnd = WINDOW.swap(0, Ordering::SeqCst);
    if hwnd != 0 {
        // SAFETY: a message to the tray's own window; it removes the icon and ends its thread.
        unsafe {
            let _ = wm::PostMessageW(HWND(hwnd), wm::WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: u32::try_from(std::mem::size_of::<NOTIFYICONDATAW>()).unwrap_or_default(),
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: ICON_MESSAGE,
        ..Default::default()
    };
    // SAFETY: the program's own icon (build.rs puts it first), else Windows' plain one.
    data.hIcon = unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        wm::LoadIconW(module, PCWSTR(1 as *const u16))
            .or_else(|_| wm::LoadIconW(None, wm::IDI_APPLICATION))
            .unwrap_or_default()
    };
    let tip: Vec<u16> = "5th Echelon Enhanced".encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data
}

fn add(hwnd: HWND) -> bool {
    // SAFETY: the icon data names our own window.
    unsafe { Shell_NotifyIconW(NIM_ADD, &icon_data(hwnd)).as_bool() }
}

fn run() -> Result<(), String> {
    // SAFETY: a window class and a message-only window of our own, and their message loop.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = CLASS;
        let wc = wm::WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if wm::RegisterClassW(&wc) == 0 {
            return Err("its window class wasn't registered".into());
        }
        let _ = TASKBAR_CREATED.set(wm::RegisterWindowMessageW(w!("TaskbarCreated")));
        // A hidden top-level window (not message-only: Explorer's TaskbarCreated is broadcast).
        let hwnd = wm::CreateWindowExW(
            wm::WINDOW_EX_STYLE::default(),
            class,
            w!("5th Echelon Enhanced"),
            wm::WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            instance,
            None,
        );
        if hwnd.0 == 0 {
            return Err("its window wasn't made".into());
        }
        WINDOW.store(hwnd.0, Ordering::SeqCst);
        if !add(hwnd) {
            tracing::warn!("Tray: Windows didn't take the icon (it's added when the taskbar comes back)");
        }
        let mut msg = wm::MSG::default();
        while wm::GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = wm::TranslateMessage(&msg);
            wm::DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn ask(what: Asked) {
    if let Some(f) = ASKED.get() {
        f(what);
    }
}

/// The icon's menu: Open, and Quit.
unsafe fn menu(hwnd: HWND) {
    let Ok(menu) = wm::CreatePopupMenu() else { return };
    let _ = wm::AppendMenuW(menu, wm::MF_STRING, MENU_OPEN, w!("Open 5th Echelon Enhanced"));
    let _ = wm::AppendMenuW(menu, wm::MF_SEPARATOR, 0, None);
    let _ = wm::AppendMenuW(menu, wm::MF_STRING, MENU_QUIT, w!("Quit"));
    let _ = wm::SetMenuDefaultItem(menu, u32::try_from(MENU_OPEN).unwrap_or_default(), 0);
    let mut at = POINT::default();
    let _ = wm::GetCursorPos(&mut at);
    // In front first, or the menu stays open when the player clicks elsewhere.
    let _ = wm::SetForegroundWindow(hwnd);
    let chosen = wm::TrackPopupMenu(menu, wm::TPM_RETURNCMD | wm::TPM_RIGHTBUTTON | wm::TPM_BOTTOMALIGN, at.x, at.y, 0, hwnd, None);
    let _ = wm::DestroyMenu(menu);
    match usize::try_from(chosen.0).unwrap_or_default() {
        MENU_OPEN => ask(Asked::Open),
        MENU_QUIT => ask(Asked::Quit),
        _ => {}
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if TASKBAR_CREATED.get().is_some_and(|m| *m == msg) {
        add(hwnd);
        return LRESULT(0);
    }
    match msg {
        OPEN_MESSAGE => {
            ask(Asked::Open);
            LRESULT(0)
        }
        ICON_MESSAGE => {
            match u32::try_from(lparam.0 & 0xffff).unwrap_or_default() {
                wm::WM_LBUTTONUP | wm::WM_LBUTTONDBLCLK => ask(Asked::Open),
                wm::WM_RBUTTONUP | wm::WM_CONTEXTMENU => menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        wm::WM_CLOSE => {
            let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
            let _ = wm::DestroyWindow(hwnd);
            LRESULT(0)
        }
        wm::WM_DESTROY => {
            wm::PostQuitMessage(0);
            LRESULT(0)
        }
        _ => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
