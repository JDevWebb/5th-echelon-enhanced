//! The controller (XInput) for the overlay's panel.
//!
//! The game reads its controller through `xinput1_3.dll`'s `XInputGetState` (an Xbox
//! controller, or any pad Steam Input presents as one, as on the Steam Deck). hudhook gives the
//! overlay the window's keys and mouse but no controller, so the overlay reads it here, once a
//! frame, and turns it into imgui's gamepad keys (the overlay's `before_render`):
//!
//! - **View + Menu** (Back + Start) together open and close the panel; while View is held the
//!   game doesn't see Menu, so the chord never pauses the game;
//! - while the panel is open the game sees the controller at rest (no buttons, sticks centred),
//!   as it gets no mouse or keys then;
//! - a button still held when the panel closes (B, A) stays hidden from the game until it's let
//!   go, so closing doesn't also crouch or shoot.
//!
//! Test clients (`test-commands`) take `pad <buttons> [ms]` in `gamebox-command` (the gamebox has
//! no controller): the buttons are pressed for that long (150 ms by default) as if from the
//! first controller, through the same path the game and the overlay read. The game itself
//! doesn't read them there: it looks for controllers through DirectInput as it starts and reads
//! XInput only for one it found (none, under Wine with no device), so what the game sees while
//! the panel is open needs a real controller to check.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use retour::static_detour;
use tracing::info;
use tracing::warn;
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::System::LibraryLoader::GetProcAddress;
use windows::Win32::System::LibraryLoader::LoadLibraryA;

/// `XINPUT_GAMEPAD`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Gamepad {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

/// `XINPUT_STATE`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct State {
    pub packet: u32,
    pub pad: Gamepad,
}

/// `XINPUT_GAMEPAD_*`.
pub mod button {
    pub const UP: u16 = 0x0001;
    pub const DOWN: u16 = 0x0002;
    pub const LEFT: u16 = 0x0004;
    pub const RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const LB: u16 = 0x0100;
    pub const RB: u16 = 0x0200;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
    /// View + Menu: the panel's chord.
    pub const CHORD: u16 = BACK | START;
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
/// How far the left stick goes before it counts as the d-pad (of 32767).
const STICK_AS_DPAD: i16 = 16000;
/// How often the other three controller slots are looked at when the one in use isn't
/// connected (a disconnected slot is slow to ask on Windows).
const RESCAN: Duration = Duration::from_secs(2);

type GetState = unsafe extern "system" fn(u32, *mut State) -> u32;

static_detour! {
    static XInputGetStateHook: unsafe extern "system" fn(u32, *mut State) -> u32;
}

/// `XInputGetState` itself, when it couldn't be hooked (the overlay still reads it).
static DIRECT: OnceLock<GetState> = OnceLock::new();
static HOOKED: AtomicBool = AtomicBool::new(false);
/// The panel is open: the game sees the controller at rest.
static BLOCKED: AtomicBool = AtomicBool::new(false);
/// Buttons hidden from the game until they're let go.
static SWALLOWED: AtomicU16 = AtomicU16::new(0);
/// Buttons pressed by a test command (test clients only; 0 otherwise).
static INJECTED: AtomicU16 = AtomicU16::new(0);
/// Changes of [`INJECTED`], so the packet number moves with them.
static INJECTED_CHANGES: AtomicU32 = AtomicU32::new(0);
/// The game's reads: whether it has asked at all, and the slots it found connected (each
/// noted once in the log).
static GAME_ASKED: AtomicBool = AtomicBool::new(false);
static GAME_SAW: AtomicU16 = AtomicU16::new(0);
/// The controller slot the overlay reads, and when the others were last looked at.
static SLOT: AtomicU32 = AtomicU32::new(0);
static LAST_SCAN: Mutex<Option<Instant>> = Mutex::new(None);

/// The hooks went in (once).
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Hooks `XInputGetState`: as the add-on loads (`early`: only if the game has `xinput1_3.dll`
/// loaded already, as it imports it; nothing may be loaded then), else when the overlay starts.
/// The game looks for controllers as it starts, so early is better.
pub fn init(early: bool) {
    if INSTALLED.load(Ordering::SeqCst) {
        return;
    }
    let name = windows::core::s!("xinput1_3.dll");
    // SAFETY: looking a module up, or loading one outside the loader's lock (not early).
    let dll = unsafe {
        if early {
            GetModuleHandleA(name)
        } else {
            LoadLibraryA(name)
        }
    };
    let Ok(dll) = dll else {
        if !early {
            warn!("Controller: xinput1_3.dll couldn't be loaded: the overlay works with the mouse and keys only");
        }
        return;
    };
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        // By name, and by its ordinal (2), which is how the game imports it.
        let Some(target) = GetProcAddress(dll, windows::core::s!("XInputGetState")).or_else(|| GetProcAddress(dll, windows::core::PCSTR(2 as *const u8))) else {
            warn!("Controller: XInputGetState not found: the overlay works with the mouse and keys only");
            return;
        };
        let target: GetState = std::mem::transmute(target);
        let _ = DIRECT.set(target);
        match XInputGetStateHook.initialize(target, get_state).and_then(|h| h.enable()) {
            Ok(()) => {
                HOOKED.store(true, Ordering::SeqCst);
                info!("Controller: XInputGetState hooked (View + Menu opens the overlay)");
            }
            Err(e) => warn!("Controller: couldn't hook XInputGetState ({e:?}): the overlay reads the controller, but the game isn't held back while it's open"),
        }
    }
    #[cfg(feature = "test-commands")]
    std::thread::spawn(test_commands);
}

/// The controller as it is (the test's buttons added), from the original function.
unsafe fn read(slot: u32, state: *mut State) -> u32 {
    let mut r = if HOOKED.load(Ordering::Relaxed) {
        XInputGetStateHook.call(slot, state)
    } else if let Some(direct) = DIRECT.get() {
        direct(slot, state)
    } else {
        ERROR_DEVICE_NOT_CONNECTED
    };
    let injected = INJECTED.load(Ordering::Relaxed);
    if slot == 0 && injected != 0 && !state.is_null() {
        let st = &mut *state;
        if r != ERROR_SUCCESS {
            *st = State::default();
            r = ERROR_SUCCESS;
        }
        st.pad.buttons |= injected;
        st.packet = st.packet.wrapping_add(INJECTED_CHANGES.load(Ordering::Relaxed));
    }
    r
}

/// What the game gets: the controller, less what the overlay holds back.
fn get_state(slot: u32, state: *mut State) -> u32 {
    let r = unsafe { read(slot, state) };
    if !GAME_ASKED.swap(true, Ordering::Relaxed) {
        info!("Controller: the game reads the controllers (first ask: controller {})", slot + 1);
    }
    if r != ERROR_SUCCESS || state.is_null() {
        return r;
    }
    if slot < 4 && GAME_SAW.fetch_or(1 << slot, Ordering::Relaxed) & (1 << slot) == 0 {
        info!("Controller: the game found controller {} connected", slot + 1);
    }
    // SAFETY: the caller's state, filled just now.
    let st = unsafe { &mut *state };
    let held = st.pad.buttons;
    let swallowed = SWALLOWED.fetch_and(held, Ordering::Relaxed) & held;
    st.pad = for_game(st.pad, BLOCKED.load(Ordering::Relaxed), swallowed);
    r
}

/// The controller as the game should see it: at rest while the panel is open; otherwise
/// without the swallowed buttons, and without Menu while View is held (the panel's chord).
fn for_game(pad: Gamepad, blocked: bool, swallowed: u16) -> Gamepad {
    if blocked {
        return Gamepad::default();
    }
    let mut hide = swallowed;
    if pad.buttons & button::BACK != 0 {
        hide |= button::START;
    }
    Gamepad {
        buttons: pad.buttons & !hide,
        ..pad
    }
}

/// The buttons held now on the controller in use (the left stick counted as the d-pad), or 0.
pub fn poll() -> u16 {
    let slot = SLOT.load(Ordering::Relaxed);
    let mut state = State::default();
    // SAFETY: our own state.
    if unsafe { read(slot, &mut state) } == ERROR_SUCCESS {
        let held = buttons(&state.pad);
        SWALLOWED.fetch_and(state.pad.buttons, Ordering::Relaxed);
        return held;
    }
    // Not connected: the other slots, now and then.
    let Ok(mut last) = LAST_SCAN.lock() else { return 0 };
    if last.is_some_and(|at| at.elapsed() < RESCAN) {
        return 0;
    }
    *last = Some(Instant::now());
    for other in (0..4).filter(|s| *s != slot) {
        // SAFETY: our own state.
        if unsafe { read(other, &mut state) } == ERROR_SUCCESS {
            SLOT.store(other, Ordering::Relaxed);
            info!("Controller: reading controller {} for the overlay", other + 1);
            return buttons(&state.pad);
        }
    }
    0
}

/// The buttons, with the left stick pushed past halfway as the d-pad.
fn buttons(pad: &Gamepad) -> u16 {
    let mut b = pad.buttons;
    if pad.thumb_ly > STICK_AS_DPAD {
        b |= button::UP;
    }
    if pad.thumb_ly < -STICK_AS_DPAD {
        b |= button::DOWN;
    }
    if pad.thumb_lx < -STICK_AS_DPAD {
        b |= button::LEFT;
    }
    if pad.thumb_lx > STICK_AS_DPAD {
        b |= button::RIGHT;
    }
    b
}

/// The panel opened or closed: the game sees the controller at rest while it's open.
pub fn set_blocked(open: bool) {
    BLOCKED.store(open, Ordering::Relaxed);
}

/// Hides these buttons from the game until each is let go (the ones that just closed the panel).
pub fn swallow(buttons: u16) {
    SWALLOWED.fetch_or(buttons, Ordering::Relaxed);
}

/// A button's name for the log and the test command.
fn named(name: &str) -> Option<u16> {
    Some(match name {
        "up" => button::UP,
        "down" => button::DOWN,
        "left" => button::LEFT,
        "right" => button::RIGHT,
        "start" | "menu" => button::START,
        "back" | "view" => button::BACK,
        "lb" => button::LB,
        "rb" => button::RB,
        "a" => button::A,
        "b" => button::B,
        "x" => button::X,
        "y" => button::Y,
        _ => return None,
    })
}

/// `pad a+down [ms]`: the buttons and how long they're held (150 ms by default, at most 5 s).
fn parse_command(text: &str) -> Option<(u16, Duration)> {
    let mut words = text.trim().strip_prefix("pad ")?.split_whitespace();
    let mut pressed = 0;
    for name in words.next()?.split('+') {
        pressed |= named(&name.to_ascii_lowercase())?;
    }
    let ms = match words.next() {
        Some(ms) => ms.parse::<u64>().ok().filter(|ms| (1..=5000).contains(ms))?,
        None => 150,
    };
    words.next().is_none().then_some((pressed, Duration::from_millis(ms)))
}

#[cfg(feature = "test-commands")]
/// Test clients only: `pad <buttons> [ms]` in `gamebox-command` presses them as the first
/// controller, then the file is deleted.
fn test_commands() {
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let Some(file) = crate::LOG_PATH.get().and_then(|p| p.parent()).map(|d| d.join("gamebox-command")) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        if !text.trim().starts_with("pad ") {
            continue;
        }
        let _ = std::fs::remove_file(&file);
        let Some((pressed, held)) = parse_command(&text) else {
            warn!("Controller: test: {:?} isn't a command (pad <a+b+...> [ms]); deleted", text.trim());
            continue;
        };
        INJECTED.store(pressed, Ordering::Relaxed);
        INJECTED_CHANGES.fetch_add(1, Ordering::Relaxed);
        std::thread::sleep(held);
        INJECTED.store(0, Ordering::Relaxed);
        INJECTED_CHANGES.fetch_add(1, Ordering::Relaxed);
        info!(
            "Controller: test: pressed {} for {} ms",
            text.trim().trim_start_matches("pad ").split_whitespace().next().unwrap_or(""),
            held.as_millis()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_sees_the_controller_at_rest_while_the_panel_is_open() {
        let pad = Gamepad {
            buttons: button::A | button::DOWN,
            left_trigger: 200,
            thumb_lx: 30000,
            ..Gamepad::default()
        };
        let seen = for_game(pad, true, 0);
        assert_eq!((seen.buttons, seen.left_trigger, seen.thumb_lx), (0, 0, 0));
        let seen = for_game(pad, false, 0);
        assert_eq!((seen.buttons, seen.left_trigger, seen.thumb_lx), (button::A | button::DOWN, 200, 30000));
    }

    #[test]
    fn view_hides_menu_so_the_chord_never_pauses_the_game() {
        assert_eq!(
            for_game(
                Gamepad {
                    buttons: button::CHORD,
                    ..Gamepad::default()
                },
                false,
                0
            )
            .buttons,
            button::BACK
        );
        assert_eq!(
            for_game(
                Gamepad {
                    buttons: button::START,
                    ..Gamepad::default()
                },
                false,
                0
            )
            .buttons,
            button::START
        );
    }

    #[test]
    fn a_button_that_closed_the_panel_stays_hidden_until_let_go() {
        let pad = Gamepad {
            buttons: button::B | button::X,
            ..Gamepad::default()
        };
        assert_eq!(for_game(pad, false, button::B).buttons, button::X);
    }

    #[test]
    fn the_left_stick_counts_as_the_dpad_past_halfway() {
        let at = |lx, ly| {
            buttons(&Gamepad {
                thumb_lx: lx,
                thumb_ly: ly,
                ..Gamepad::default()
            })
        };
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(8000, -8000), 0, "a little push is nothing");
        assert_eq!(at(0, 20000), button::UP);
        assert_eq!(at(-20000, -20000), button::LEFT | button::DOWN);
        assert_eq!(at(i16::MAX, 0), button::RIGHT);
    }

    #[test]
    fn test_commands_name_buttons_and_how_long() {
        assert_eq!(parse_command("pad a"), Some((button::A, Duration::from_millis(150))));
        assert_eq!(parse_command("pad view+menu 400\n"), Some((button::CHORD, Duration::from_millis(400))));
        assert_eq!(parse_command("pad LB"), Some((button::LB, Duration::from_millis(150))));
        assert_eq!(parse_command("pad down+a 20"), Some((button::DOWN | button::A, Duration::from_millis(20))));
        assert_eq!(parse_command("pad z"), None, "no such button");
        assert_eq!(parse_command("pad a 0"), None);
        assert_eq!(parse_command("pad a 9000"), None, "at most 5 s");
        assert_eq!(parse_command("pad a 100 more"), None);
        assert_eq!(parse_command("select 4"), None);
    }
}
