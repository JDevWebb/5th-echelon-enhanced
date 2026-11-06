//! Quick start: from the process start straight into the online lobby of Spies vs
//! Mercs or Co-op, with no key press, invitation or dialog (opt-in, `QuickStartLobby`). Once the
//! lobby is chosen (or the quick start gives up) every hook only calls the game's own function.
//! Off for a start the launcher made for a friend's Join (its join request waiting).
//!
//! Runtime changes only; no file is modified. `install` runs from `hooks::init` (in `DllMain`),
//! before the game's own code starts.
//!
//! Both executables, `Blacklist_DX11_game.exe` and `Blacklist_game.exe` (DX9): every function
//! and global used is found by signature (`hooks_addresses::quick_start`, which
//! `check_crash_fixes` also runs on an executable on disk). Unless all of them are found, the
//! module changes nothing. The addresses below are DX11's.
//!
//! 1. **Boot videos** (disclaimer, logos, title background): the video gate in the Bink player
//!    (`0x42e8c0`) reads the command line with `strstr`; it has a built-in skip branch
//!    (`0x42e9ed`: empties the list and clears the "video playing" flag, so the renderer doesn't
//!    wait for the end). The `jne` after its `menu` check (`0x42e98d`) is made a `jmp` into that
//!    branch.
//! 2. **Title screen**: once the title is ready and online presence is up, the title's tick
//!    presses Start the way `UI::SceneStart::vf88`'s invitation branch does (the "Start pressed"
//!    flag, state 2 -> 3, assigning the controller).
//! 3. **Online menu**: the travel to the Paladin hub that follows the press is suppressed, and
//!    the game's own `UI::Task::OnlineMultiplayerStartupTask` is started instead, on the UI task
//!    runner once it's free (`0x19c75d0`). Its callback queues the menu state machine's command
//!    0x2a (title -> online menu), as the game does from the hub. Should the online startup
//!    fail (no server), the game shows its own dialog and the suppressed hub travel is done after
//!    all: the game goes on as it would have without the quick start.
//!    Still on the title 30 s after that, with the task runner free, it gives up the same way.
//! 4. **Lobby**: in the online menu (`UI::SMIMatchmaking`'s tick), once the entry for Spies vs
//!    Mercs (id 1) or Co-op (id 2) has stood in the list for a moment, it gets the focus and the
//!    menu's own "select" handler is called.
//!
//! Everything is logged with the milliseconds since the DLL was loaded (`quickstart:`), with
//! each state change of the title screen and the menu state machine.

use std::ffi::c_void;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use hooks_addresses::quick_start::Sites;
use hooks_config::navigation;
use hooks_config::navigation::Action;
use hooks_config::navigation::Goal;
use hooks_config::navigation::MenuView;
use hooks_config::navigation::Navigator;
use hooks_config::QuickStartLobby;
use retour::static_detour;
use tracing::info;
use tracing::warn;

static_detour! {
    static SceneStartTickHook: unsafe extern "thiscall" fn(*mut c_void, u32);
    static SmiStepHook: unsafe extern "thiscall" fn(*mut c_void);
    static HubTravelHook: unsafe extern "thiscall" fn(*mut c_void);
    static HubTravelHook9: unsafe extern "stdcall" fn(*mut c_void);
    static MatchmakingTickHook: unsafe extern "thiscall" fn(*mut c_void, f32);
    static OnlineResultHook: unsafe extern "thiscall" fn(*mut c_void, i32);
}

static SITES: OnceLock<Sites> = OnceLock::new();

/// Where everything is (the hooks are only installed once it's all found).
fn exe() -> &'static Sites {
    SITES.get().expect("quickstart: hooked before its sites were found")
}

/// The UI task runner's running task at `+0x30` (0: free; otherwise a new task is ignored).
const TASK_RUNNER_CURRENT: usize = 0x30;
/// A menu command that does nothing in the title state 0x9a.
const SMI_CMD_NOOP_TITLE: u32 = 0x7b;
/// `UI::SMIMatchmaking`: active (u8), its list. The list: page (`+0x250`), columns (`+0x264`). A
/// column: focus (`+0x240`), count (`+0x290`), entries (`+0x2a4`), id map (`+0x28c`). The same in
/// both executables.
const MM_ACTIVE: usize = 0x2e8;
const MM_LIST: usize = 0x2a0;
const LIST_PAGE: usize = 0x250;
const LIST_COLUMNS: usize = 0x264;
const COL_FOCUS: usize = 0x240;
const COL_COUNT: usize = 0x290;
const COL_ITEMS: usize = 0x2a4;
const COL_ID_MAP: usize = 0x28c;
const NODE_ID: usize = 0x40;
/// Presence `+0x4d4`: a controller index (only valid with an invitation pending).
const PRESENCE_CONTROLLER: usize = 0x4d4;
/// `UI::SMI`: state, command queue and its length.
const SMI_STATE: usize = 0x2b8;
const SMI_QUEUE: usize = 0x2c8;
const SMI_QUEUE_LEN: usize = 0x2cc;
const SMI_STATE_TITLE: u32 = 0x9a;
/// The online menu (`SMIPartyLobby`/`SMIMatchmaking`).
const SMI_STATE_ONLINE: u32 = 0x56;
/// `UI::SceneStart`: title ready (u8), state (u32). `NetOnline+0x4c`: the presence.
const SS_READY: usize = 0x55d;
const SS_STATE: usize = 0x568;
const ONLINE_PRESENCE: usize = 0x4c;

static START: OnceLock<Instant> = OnceLock::new();
static LOBBY: OnceLock<QuickStartLobby> = OnceLock::new();
/// The title screen is ready (`SceneStart+0x55d`).
static TITLE_READY: AtomicBool = AtomicBool::new(false);
/// The last `this` of the title screen (from its tick).
static SCENE_START: AtomicUsize = AtomicUsize::new(0);
/// The game saw Start pressed (logged once).
static PRESS_SEEN: AtomicBool = AtomicBool::new(false);
/// Start was pressed by this module.
static PRESS_SENT: AtomicBool = AtomicBool::new(false);
/// The online startup was asked for (instead of the hub).
static ONLINE_ASKED: AtomicBool = AtomicBool::new(false);
/// The first hub travel after the press was suppressed.
static HUB_SUPPRESSED: AtomicBool = AtomicBool::new(false);
/// The online startup task is to be started (on the main thread, when the runner is free).
static ONLINE_START_PENDING: AtomicBool = AtomicBool::new(false);
/// The online startup task this module started hasn't answered yet.
static ONLINE_STARTED: AtomicBool = AtomicBool::new(false);
/// The lobby was chosen: nothing more to do.
static MODE_CHOSEN: AtomicBool = AtomicBool::new(false);
/// The online startup failed: the game goes on as without the quick start.
static GAVE_UP: AtomicBool = AtomicBool::new(false);
/// When the hub travel (or, DX11, the menu's hub command) was first suppressed for the online
/// startup.
static SUPPRESSED_AT: OnceLock<Instant> = OnceLock::new();
/// Still on the title this long after the hub travel was suppressed, with the UI task runner
/// free (the startup finished without leading anywhere, or never started), the quick start
/// gives up and the hub travel is done after all.
const TITLE_GIVE_UP: Duration = Duration::from_secs(30);
/// How long the menu has to leave the title after the online startup succeeded before the quick
/// start may give up.
const AFTER_ONLINE_DONE: Duration = Duration::from_secs(10);
/// When this module's online startup succeeded.
static ONLINE_DONE_AT: OnceLock<Instant> = OnceLock::new();
/// The online list's ids last seen (for the log).
static LAST_IDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
#[cfg(feature = "test-commands")]
/// `test_commands` (the gamebox's copies): see [`test_command`].
static TEST_COMMANDS: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-commands")]
/// What the menus showed last with `test_commands`: (active, page, ids), for the log.
static TEST_LAST: Mutex<Option<(u32, u32, Vec<u32>)>> = Mutex::new(None);
#[cfg(feature = "test-commands")]
/// When the command file was last looked for.
static TEST_LOOKED: Mutex<Option<Instant>> = Mutex::new(None);

/// What's left to do in the online menus (`hooks_config::navigation`): the lobby, then the
/// launcher's New or Quick match.
static NAV: Mutex<Option<Navigator>> = Mutex::new(None);
/// Steps are left in [`NAV`] (read every frame by the overlay, without the lock).
static NAV_BUSY: AtomicBool = AtomicBool::new(false);
/// The quick start is finding its way to the lobby (from the title until the lobby is chosen,
/// it gives up, or its 5 minutes are over): the player's input is held back meanwhile.
static NAVIGATING: AtomicBool = AtomicBool::new(false);
/// Esc: 0 not pressed, 1 the player stopped the navigation (the title still to be let go, if
/// it was held), 2 done.
static ABORT: AtomicU8 = AtomicU8::new(0);

/// The game is being taken through its menus for the player (to the lobby, or a New or Quick
/// match's matchmaking): a click or key now would fight it, so the overlay holds the player's
/// input back (`overlay_input`), says why, and Esc stops it ([`abort`]).
pub fn navigating() -> bool {
    ABORT.load(Ordering::Relaxed) == 0 && (NAVIGATING.load(Ordering::Relaxed) || NAV_BUSY.load(Ordering::Relaxed))
}

/// The player stopped the navigation with Esc.
pub fn aborted() -> bool {
    ABORT.load(Ordering::Relaxed) != 0
}

/// Esc while navigating (the overlay, any thread): everything left is dropped and the game is
/// the player's from where it is. At the title, before the online startup began, the travel to
/// the hub held back for it is let go on the title's next tick, as if there had been no quick
/// start; a startup under way finishes into the online menu; in the menus it stops there. The
/// launcher's request, if not read yet, is deleted. `false`: nothing was under way.
pub fn abort() -> bool {
    if !navigating() {
        return false;
    }
    ABORT.store(1, Ordering::Relaxed);
    if let Ok(mut nav) = NAV.lock() {
        if let Some(nav) = nav.as_mut() {
            nav.abort();
        }
    }
    NAV_BUSY.store(false, Ordering::Relaxed);
    NAVIGATING.store(false, Ordering::Relaxed);
    if let Some(path) = crate::community::folder().map(|d| d.join(hooks_config::new_match_request::FILE)) {
        let _ = std::fs::remove_file(path);
    }
    info!("quickstart: +{} ms stopped by the player (Esc)", ms());
    true
}

/// Nothing more to do: the lobby was chosen, or the quick start gave up. Every hook then only
/// calls the game's own function, so a later return to the title (a quit, a lost connection)
/// is the game's alone.
fn done() -> bool {
    MODE_CHOSEN.load(Ordering::Relaxed) || GAVE_UP.load(Ordering::Relaxed) || aborted()
}

fn ms() -> u128 {
    START.get().map_or(0, |s| s.elapsed().as_millis())
}

fn readable(at: usize, len: usize) -> bool {
    at >= 0x1_0000 && region::query_range(at as *const u8, len).is_ok_and(|mut r| r.all(|r| r.is_ok_and(|r| r.is_readable())))
}

fn read_u32(at: usize) -> Option<u32> {
    // SAFETY: checked readable just before.
    readable(at, 4).then(|| unsafe { std::ptr::read_volatile(at as *const u32) })
}

unsafe fn bytes_at(at: usize, expected: &[u8]) -> bool {
    readable(at, expected.len()) && std::slice::from_raw_parts(at as *const u8, expected.len()) == expected
}

/// Online presence is up (`NetOnline+0x4c`).
fn presence_ready() -> bool {
    read_u32(exe().net_online)
        .filter(|&o| o != 0)
        .and_then(|o| read_u32(o as usize + ONLINE_PRESENCE))
        .is_some_and(|p| p != 0)
}

fn scene_state() -> Option<u32> {
    match SCENE_START.load(Ordering::Relaxed) {
        0 => None,
        this => read_u32(this + SS_STATE),
    }
}

fn smi_state() -> Option<u32> {
    read_u32(exe().smi).filter(|&s| s != 0).and_then(|s| read_u32(s as usize + SMI_STATE))
}

/// Installs everything (from `hooks::init`, in `DllMain`, before the game's code runs). An
/// executable where not everything is found is left untouched.
pub unsafe fn install(lobby: QuickStartLobby) {
    let _ = START.set(Instant::now());
    let _ = LOBBY.set(lobby);
    #[cfg(feature = "test-commands")]
    if hooks_config::get().is_some_and(|c| c.test_commands) {
        TEST_COMMANDS.store(true, Ordering::Relaxed);
        info!("quickstart: test commands on (gamebox-command beside the log)");
        let _ = std::thread::Builder::new().name("fe-test-commands".into()).spawn(lobby_commands);
    }
    // Started for a friend's Join (the launcher's join request waiting beside uplay.toml): the
    // join is what the player asked for, not a lobby. Left for `community` to take.
    if join_request_waiting() {
        info!("quickstart: a join request is waiting -> off for this start");
        return;
    }

    let Some(image) = crate::game_image() else {
        warn!("quickstart: the game's sections weren't found -> nothing changed");
        return;
    };
    let sites = match hooks_addresses::quick_start::locate(&image) {
        Ok(sites) => sites,
        Err(why) => {
            warn!("quickstart: {why} -> nothing changed (unknown build?)");
            return;
        }
    };
    let e = SITES.get_or_init(|| sites);

    // 1. Boot videos: the gate's `jne` becomes a `jmp` to its own skip branch.
    let rel = (e.video_skip as u32).wrapping_sub(e.video_gate_jne as u32 + 5);
    let mut patch = [0xE9, 0, 0, 0, 0, 0x90];
    patch[1..5].copy_from_slice(&rel.to_le_bytes());
    let _ = crate::writemem(e.video_gate_jne as *mut u8, &patch);
    if bytes_at(e.video_gate_jne, &patch) {
        info!("quickstart: {}: boot videos skipped (video gate @ {:#x} -> {:#x})", e.name, e.video_gate_jne, e.video_skip);
    } else {
        warn!("quickstart: the video gate patch didn't take");
    }

    // 2.-4. Hooks.
    crate::hooks::hook!(SceneStartTickHook, Some(e.scenestart_tick), scene_start_tick);
    if let Some(at) = e.smi_step {
        crate::hooks::hook!(SmiStepHook, Some(at), smi_step);
    }
    if e.dx9 {
        crate::hooks::hook!(HubTravelHook9, Some(e.hub_travel), hub_travel9);
    } else {
        crate::hooks::hook!(HubTravelHook, Some(e.hub_travel), hub_travel);
    }
    crate::hooks::hook!(OnlineResultHook, Some(e.smi_push_online), online_result);
    crate::hooks::hook!(MatchmakingTickHook, Some(e.mm_tick), matchmaking_tick);

    // Logs the states on a thread of its own (it starts after DllMain).
    if let Ok(mut nav) = NAV.lock() {
        *nav = Some(Navigator::to_lobby(matches!(lobby, QuickStartLobby::Coop)));
    }
    NAV_BUSY.store(true, Ordering::Relaxed);
    NAVIGATING.store(true, Ordering::Relaxed);
    let _ = std::thread::Builder::new().name(String::from("quickstart")).spawn(watch);
    info!("quickstart: {} on, lobby {lobby:?}", e.name);
}

/// Whether the launcher's join request for this start is waiting and still good (read, not
/// taken: `community` takes it).
fn join_request_waiting() -> bool {
    let Some(path) = crate::community::folder().map(|d| d.join(hooks_config::join_request::FILE)) else {
        return false;
    };
    let Ok(bytes) = std::fs::read(path) else { return false };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    hooks_config::join_request::read(&bytes, now).is_ok()
}

/// The title screen's tick (main thread): presses Start once the title is ready and online
/// presence is up, and starts the online startup when it's been asked for.
fn scene_start_tick(this: *mut c_void, a1: u32) {
    if !this.is_null() && ABORT.compare_exchange(1, 2, Ordering::Relaxed, Ordering::Relaxed).is_ok() && let_title_go() {
        // SAFETY: main thread, the live title screen the travel was held back for; the
        // originals, unchanged.
        unsafe {
            if exe().dx9 {
                HubTravelHook9.call(this);
            } else {
                HubTravelHook.call(this);
            }
            return SceneStartTickHook.call(this, a1);
        }
    }
    if done() {
        // SAFETY: the original, unchanged.
        return unsafe { SceneStartTickHook.call(this, a1) };
    }
    SCENE_START.store(this as usize, Ordering::Relaxed);
    if !this.is_null() && title_given_up() {
        warn!(
            "quickstart: +{} ms still on the title {} s after the online startup was asked for -> to the hub, as without the quick start",
            ms(),
            TITLE_GIVE_UP.as_secs()
        );
        GAVE_UP.store(true, Ordering::Relaxed);
        // SAFETY: main thread, the live title screen the travel was suppressed for; the
        // original, unchanged. Then this tick as the game's.
        unsafe {
            if exe().dx9 {
                HubTravelHook9.call(this);
            } else {
                HubTravelHook.call(this);
            }
            return SceneStartTickHook.call(this, a1);
        }
    }
    if !this.is_null() {
        // SAFETY: `this` is the live SceneStart the game is ticking right now; `pressed` is a
        // global of the recognised executable.
        unsafe {
            TITLE_READY.store(*((this as usize + SS_READY) as *const u8) != 0, Ordering::Relaxed);
            if !PRESS_SEEN.load(Ordering::Relaxed) && *(exe().pressed as *const u8) != 0 {
                PRESS_SEEN.store(true, Ordering::Relaxed);
                info!("quickstart: +{} ms title: Start is pressed", ms());
            }
        }
    }
    if !this.is_null() && !PRESS_SENT.load(Ordering::Relaxed) && TITLE_READY.load(Ordering::Relaxed) && smi_state() == Some(SMI_STATE_TITLE) && presence_ready() {
        PRESS_SENT.store(true, Ordering::Relaxed);
        // SAFETY: main thread, live SceneStart; the same calls as vf88's invitation branch.
        unsafe { press_start(this as usize) };
    }
    // DX9 has no menu step hook: the online startup starts from here (in DX11 from either).
    if let Some(smi) = read_u32(exe().smi).filter(|&s| s != 0) {
        pump_online_start(smi as usize);
    }
    // SAFETY: the original, unchanged.
    unsafe { SceneStartTickHook.call(this, a1) }
}

/// The hub was first kept from the title [`TITLE_GIVE_UP`] ago, the game is still on the title and the
/// UI task runner is free: nothing more is coming.
fn title_given_up() -> bool {
    SUPPRESSED_AT.get().is_some_and(|at| at.elapsed() >= TITLE_GIVE_UP)
        // Not just after the startup succeeded: the task runner is free a few frames before the
        // menu leaves the title, so a startup that took about TITLE_GIVE_UP was sent to the
        // hub as it finished (the gamebox, 2026-11-05: "online startup done -> online menu",
        // then 3 ms later "still on the title 30 s after ...").
        && ONLINE_DONE_AT.get().is_none_or(|at| at.elapsed() >= AFTER_ONLINE_DONE)
        && smi_state() == Some(SMI_STATE_TITLE)
        && read_u32(exe().task_runner).filter(|&r| r != 0).and_then(|r| read_u32(r as usize + TASK_RUNNER_CURRENT)) == Some(0)
}

/// Presses Start as `UI::SceneStart::vf88`'s invitation branch does: "Start pressed" = 1, state
/// 2 -> 3, assign the controller (`player = engine->vf1f0()`), `engine[+0x44]->vf94()->vf148(idx)`.
unsafe fn press_start(scene: usize) {
    let get_engine: unsafe extern "C" fn() -> *mut c_void = std::mem::transmute(exe().get_engine);
    let engine = get_engine();
    if engine.is_null() {
        warn!("quickstart: no engine -> Start not pressed");
        return;
    }
    let vcall = |obj: *mut c_void, off: usize| -> usize { *((*(obj as *const usize)) as *const usize).add(off / 4) };
    let get_player: unsafe extern "thiscall" fn(*mut c_void) -> *mut c_void = std::mem::transmute(vcall(engine, 0x1f0));
    let player = get_player(engine);
    let presence = read_u32(exe().net_online).and_then(|o| read_u32(o as usize + ONLINE_PRESENCE)).unwrap_or(0) as usize;
    // Presence+0x4d4 is a controller index only with an invitation pending; without one it can
    // hold anything, so only a small value is taken, otherwise controller 0.
    let raw = read_u32(presence + PRESENCE_CONTROLLER);
    let idx = raw.filter(|&i| i < 4).unwrap_or(0);
    *(exe().pressed as *mut u8) = 1;
    if read_u32(scene + SS_STATE) == Some(2) {
        *((scene + SS_STATE) as *mut u32) = 3;
    }
    let assign: unsafe extern "thiscall" fn(*mut c_void, u32, u32) = std::mem::transmute(exe().assign_controller);
    assign(player, idx, 0);
    let ui = *((engine as usize + 0x44) as *const *mut c_void);
    if !ui.is_null() {
        let get94: unsafe extern "thiscall" fn(*mut c_void) -> *mut c_void = std::mem::transmute(vcall(ui, 0x94));
        let o = get94(ui);
        if !o.is_null() {
            let set148: unsafe extern "thiscall" fn(*mut c_void, u32) = std::mem::transmute(vcall(o, 0x148));
            set148(o, idx);
        }
    }
    info!("quickstart: +{} ms Start pressed (controller {idx})", ms());
}

/// After Esc at the title: the travel to the hub was held back for an online startup that
/// hasn't begun (and now won't), so it's to be done after all. A startup under way is left to
/// finish (into the online menu).
fn let_title_go() -> bool {
    let held = SUPPRESSED_AT.get().is_some() && !ONLINE_STARTED.load(Ordering::Relaxed) && ONLINE_DONE_AT.get().is_none();
    ONLINE_START_PENDING.store(false, Ordering::Relaxed);
    if held {
        info!("quickstart: +{} ms stopped at the title -> to the hub, as without the quick start", ms());
    }
    held
}

/// Asks for the online startup, once.
fn ask_online_startup() {
    if !ONLINE_ASKED.swap(true, Ordering::Relaxed) {
        ONLINE_START_PENDING.store(true, Ordering::Relaxed);
    }
}

/// The travel to the hub (main thread): the first one after our press is suppressed and the online
/// startup asked for instead. `true`: suppressed.
fn suppress_hub_travel() -> bool {
    if !done() && PRESS_SENT.load(Ordering::Relaxed) && !HUB_SUPPRESSED.swap(true, Ordering::Relaxed) {
        let _ = SUPPRESSED_AT.set(Instant::now());
        ask_online_startup();
        info!("quickstart: +{} ms hub travel suppressed -> online startup (menu state {:x?})", ms(), smi_state());
        return true;
    }
    false
}

/// DX11, thiscall(SceneStart).
fn hub_travel(this: *mut c_void) {
    if !suppress_hub_travel() {
        // SAFETY: the original, unchanged.
        unsafe { HubTravelHook.call(this) }
    }
}

/// DX9, stdcall(SceneStart).
fn hub_travel9(this: *mut c_void) {
    if !suppress_hub_travel() {
        // SAFETY: the original, unchanged.
        unsafe { HubTravelHook9.call(this) }
    }
}

/// Starts the game's online startup task (main thread) once it's asked for and the UI task
/// runner is free.
fn pump_online_start(smi: usize) {
    if !ONLINE_START_PENDING.load(Ordering::Relaxed) || aborted() {
        return;
    }
    let e = exe();
    let Some(runner) = read_u32(e.task_runner).filter(|&r| r != 0).map(|r| r as usize) else {
        return;
    };
    if read_u32(runner + TASK_RUNNER_CURRENT) != Some(0) {
        return;
    }
    ONLINE_START_PENDING.store(false, Ordering::Relaxed);
    ONLINE_STARTED.store(true, Ordering::Relaxed);
    info!("quickstart: +{} ms online startup task started (menu state {:x?})", ms(), smi_state());
    // SAFETY: a game function on the main thread; the receiver is only stored in the task's
    // callback, which doesn't use it. DX11's takes two unused stack arguments (`ret 8`), DX9's
    // none.
    unsafe {
        if e.dx9 {
            let f: unsafe extern "thiscall" fn(usize) -> u8 = std::mem::transmute(e.online_start);
            f(smi);
        } else {
            let f: unsafe extern "thiscall" fn(usize, u32, u32) -> u8 = std::mem::transmute(e.online_start);
            f(smi, 0, 0);
        }
    }
}

/// The online startup task's callback (main thread). For the task this module started: on
/// failure the game shows its own dialog, and the hub travel suppressed for it is done after all,
/// so the game goes on as it would have without the quick start. The game's own later online
/// startups pass untouched.
fn online_result(this: *mut c_void, result: i32) {
    // SAFETY: the original, unchanged.
    unsafe { OnlineResultHook.call(this, result) };
    if !ONLINE_STARTED.swap(false, Ordering::Relaxed) {
        return;
    }
    if result == 0 {
        let _ = ONLINE_DONE_AT.set(Instant::now());
        info!("quickstart: +{} ms online startup done -> online menu", ms());
        return;
    }
    let scene = SCENE_START.load(Ordering::Relaxed);
    if !HUB_SUPPRESSED.load(Ordering::Relaxed) || scene == 0 || !readable(scene, SS_STATE + 4) {
        warn!("quickstart: +{} ms online startup failed ({result})", ms());
        return;
    }
    GAVE_UP.store(true, Ordering::Relaxed);
    warn!("quickstart: +{} ms online startup failed ({result}) -> to the hub, as without the quick start", ms());
    // SAFETY: main thread, the title screen the travel was suppressed for (still live: the title
    // ticks until the travel); the original, unchanged.
    unsafe {
        if exe().dx9 {
            HubTravelHook9.call(scene as *mut c_void);
        } else {
            HubTravelHook.call(scene as *mut c_void);
        }
    }
}

/// The online menu's tick (main thread): once the wanted entry stands in the list, focus it and
/// call the menu's own "select".
fn matchmaking_tick(this: *mut c_void, dt: f32) {
    #[cfg(feature = "test-commands")]
    if done() && TEST_COMMANDS.load(Ordering::Relaxed) {
        // SAFETY: main thread, live SMIMatchmaking; every pointer is checked readable.
        unsafe { test_command(this as usize) };
    }
    if PRESS_SENT.load(Ordering::Relaxed) && NAV_BUSY.load(Ordering::Relaxed) && !aborted() {
        // SAFETY: main thread, live SMIMatchmaking; every pointer is checked readable.
        unsafe { navigate(this as usize) };
    }
    // SAFETY: the original, unchanged.
    unsafe { MatchmakingTickHook.call(this, dt) }
}

/// One tick of the online menus' steps ([`NAV`]): reads the menu, and selects what the
/// navigator says. The lobby is taken in the online menu only; once it is, the launcher's New
/// or Quick match request (if any) adds its matchmaking steps.
unsafe fn navigate(mm: usize) {
    let Ok(mut guard) = NAV.lock() else { return };
    let Some(nav) = guard.as_mut() else { return };
    let Some(goal) = nav.current() else {
        NAV_BUSY.store(false, Ordering::Relaxed);
        return;
    };
    if matches!(goal, Goal::Lobby { .. }) && smi_state() != Some(SMI_STATE_ONLINE) {
        return;
    }
    let list = read_list(mm);
    if let (Some((_, ids, _, page)), Ok(mut last)) = (&list, LAST_IDS.lock()) {
        if *last != *ids {
            info!("quickstart: +{} ms menu page {page}: ids {ids:?}", ms());
            last.clone_from(ids);
        }
    }
    let view = list.as_ref().map(|(col, ids, active, page)| MenuView {
        ids: ids.clone(),
        active: *active == 1,
        page: *page,
        focus: read_u32(col + COL_FOCUS),
    });
    match nav.tick(Instant::now(), view.as_ref()) {
        Action::Idle | Action::Wait => {}
        Action::GiveUp(goal) => {
            warn!(
                "quickstart: +{} ms {goal:?} not reached in {} s -> given up, the game is the player's",
                ms(),
                navigation::STEP_GIVE_UP.as_secs()
            );
        }
        Action::Select { goal, pos, id, fallback } => {
            let Some((col, ..)) = list else { return };
            if let Goal::Lobby { coop } = goal {
                MODE_CHOSEN.store(true, Ordering::Relaxed);
                if let Some(asked) = take_matchmaking_request() {
                    nav.then_matchmaking(coop, asked.playlist);
                }
            }
            if fallback {
                warn!("quickstart: +{} ms {goal:?}: that mode isn't in the game's list; the focused one ({id}) instead", ms());
            }
            select_entry(mm, col, pos);
            info!("quickstart: +{} ms {goal:?}: selected id {id} (entry {pos})", ms());
        }
    }
    NAV_BUSY.store(nav.busy(), Ordering::Relaxed);
}

/// Focuses entry `pos` of the menu's column and calls the menu's own "select", as a click does.
unsafe fn select_entry(mm: usize, col: usize, pos: usize) {
    *((col + COL_FOCUS) as *mut u32) = pos as u32;
    let e = exe();
    if e.dx9 {
        let accept: unsafe extern "thiscall" fn(usize) = std::mem::transmute(e.mm_select);
        accept(mm);
    } else {
        let accept: unsafe extern "thiscall" fn(usize, u32) = std::mem::transmute(e.mm_select);
        accept(mm, 0);
    }
}

/// The launcher's New or Quick match request beside the game (`hooks_config::new_match_request`),
/// read and deleted, if it's there and still good.
fn take_matchmaking_request() -> Option<hooks_config::new_match_request::Accepted> {
    let path = crate::community::folder()?.join(hooks_config::new_match_request::FILE);
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    match hooks_config::new_match_request::read(&bytes, now) {
        Ok(asked) => {
            info!(
                "quickstart: +{} ms the launcher's {} ({} s ago, mode {:?}): starting matchmaking",
                ms(),
                if asked.quick { "Quick match" } else { "New match" },
                asked.age.as_secs(),
                asked.playlist
            );
            Some(asked)
        }
        Err(e) => {
            warn!("quickstart: the launcher's matchmaking request isn't taken ({e:?})");
            None
        }
    }
}

/// The menu's list on its current page: its column, the entries' ids, and the menu's active
/// flag and the page (for the log).
unsafe fn read_list(mm: usize) -> Option<(usize, Vec<u32>, u32, u32)> {
    let active = read_u32(mm + MM_ACTIVE)? & 0xff;
    let list = read_u32(mm + MM_LIST).filter(|&l| l != 0)? as usize;
    let page = read_u32(list + LIST_PAGE)?;
    // One column per page (the online menu is page 0; Spies vs Mercs' own menu page 1).
    let col = read_u32(list + LIST_COLUMNS)
        .and_then(|c| read_u32(c as usize + page.min(15) as usize * 4))
        .filter(|&c| c != 0)? as usize;
    let count = read_u32(col + COL_COUNT).unwrap_or(0).min(32) as usize;
    let items = read_u32(col + COL_ITEMS).filter(|&i| i != 0)? as usize;
    if !readable(items, count * 4) {
        return None;
    }
    let e = exe();
    let ids = (0..count).map(|i| item_id(e, col, *((items + i * 4) as *const u32))).collect();
    Some((col, ids, active, page))
}

#[cfg(feature = "test-commands")]
/// Test clients only (`test_commands`): logs each new state of the online menus' list, and
/// once a second looks for `gamebox-command` beside the game's log. `select <id>` focuses the
/// entry with that id and calls the menu's own "select", as the quick start does, then the
/// file is deleted; an id not in the list leaves it for a later tick (the menu still loading).
unsafe fn test_command(mm: usize) {
    let Some((col, ids, active, page)) = read_list(mm) else { return };
    if let Ok(mut last) = TEST_LAST.lock() {
        let now = (active, page, ids.clone());
        if last.as_ref() != Some(&now) {
            info!("quickstart: +{} ms test: menu active {active}, page {page}, ids {ids:?}", ms());
            *last = Some(now);
        }
    }
    {
        let Ok(mut looked) = TEST_LOOKED.lock() else { return };
        if looked.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
            return;
        }
        *looked = Some(Instant::now());
    }
    let Some(file) = crate::LOG_PATH.get().and_then(|p| p.parent()).map(|d| d.join("gamebox-command")) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&file) else { return };
    if text.trim().starts_with("players ")
        || text.trim().starts_with("setting ")
        || text.trim() == "pawns"
        || text.trim() == "offsets"
        || text.trim() == "poke"
        || text.trim() == "end-round"
    {
        return;
    }
    let Some(id) = text.trim().strip_prefix("select ").and_then(|v| v.trim().parse::<u32>().ok()) else {
        warn!("quickstart: test: {:?} isn't a command (select <id>); deleted", text.trim());
        let _ = std::fs::remove_file(&file);
        return;
    };
    if active != 1 {
        return;
    }
    let Some(pos) = ids.iter().position(|&i| i == id) else { return };
    let _ = std::fs::remove_file(&file);
    *((col + COL_FOCUS) as *mut u32) = pos as u32;
    let e = exe();
    if e.dx9 {
        let accept: unsafe extern "thiscall" fn(usize) = std::mem::transmute(e.mm_select);
        accept(mm);
    } else {
        let accept: unsafe extern "thiscall" fn(usize, u32) = std::mem::transmute(e.mm_select);
        accept(mm, 0);
    }
    info!("quickstart: +{} ms test: selected id {id} (entry {pos} of {ids:?})", ms());
}

#[cfg(feature = "test-commands")]
/// Test clients only (`test_commands`): the commands for the lobby the player is in, which the
/// menus' tick doesn't see (it runs only in the online menus). `players <n>` sets the lobby's
/// players needed to start, as the overlay's stepper does (the same field, in the lobby's game
/// session), so a public match starts with the copies there are; the file is then deleted.
/// Outside a lobby it waits. Other commands are left to [`test_command`].
fn lobby_commands() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Some(file) = crate::LOG_PATH.get().and_then(|p| p.parent()).map(|d| d.join("gamebox-command")) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let Some(n) = text
            .trim()
            .strip_prefix("players ")
            .and_then(|v| v.trim().parse::<i32>().ok())
            .filter(|n| (1..=16).contains(n))
        else {
            continue;
        };
        // SAFETY: the overlay's own accessor; null outside a lobby.
        let Some(min) = (unsafe { crate::overlay::get_min_players_var().as_mut() }) else {
            continue;
        };
        let before = *min;
        *min = n;
        let _ = std::fs::remove_file(&file);
        info!("quickstart: +{} ms test: players needed to start {before} -> {n}", ms());
    }
}

/// A list entry's id, through the game's own lookup.
unsafe fn item_id(e: &Sites, col: usize, handle: u32) -> u32 {
    if e.dx9 {
        let find: unsafe extern "fastcall" fn(usize, *const u32) -> usize = std::mem::transmute(e.list_item_id);
        let node = find(col + COL_ID_MAP, &handle);
        if node == 0 {
            0
        } else {
            read_u32(node + NODE_ID).unwrap_or(0)
        }
    } else {
        let get_id: unsafe extern "thiscall" fn(usize, u32) -> u32 = std::mem::transmute(e.list_item_id);
        get_id(col, handle)
    }
}

/// One step of the menu state machine (DX11, main thread). Should the title still want the hub
/// after our press (commands 0x3d/0x47 in state 0x9a), the command is made one that does nothing
/// there, and the online startup asked for instead.
fn smi_step(this: *mut c_void) {
    if done() {
        // SAFETY: the original, unchanged.
        return unsafe { SmiStepHook.call(this) };
    }
    let smi = this as usize;
    let queued = read_u32(smi + SMI_QUEUE_LEN).unwrap_or(0) > 0;
    let cmd = if queued { read_u32(smi + SMI_QUEUE).and_then(|q| read_u32(q as usize)) } else { None };
    if PRESS_SENT.load(Ordering::Relaxed) && read_u32(smi + SMI_STATE) == Some(SMI_STATE_TITLE) && matches!(cmd, Some(0x3d | 0x47)) {
        if let Some(q) = read_u32(smi + SMI_QUEUE) {
            // SAFETY: the head of the command array (its length checked above), main thread.
            unsafe { *(q as usize as *mut u32) = SMI_CMD_NOOP_TITLE };
            let _ = SUPPRESSED_AT.set(Instant::now());
            ask_online_startup();
            info!("quickstart: +{} ms menu command {:#x} (hub) on the title dropped -> online startup", ms(), cmd.unwrap_or(0));
        }
    }
    pump_online_start(smi);
    // SAFETY: the original, unchanged.
    unsafe { SmiStepHook.call(this) };
}

/// Logs each state change until the lobby is chosen (at most a few minutes), then stops.
fn watch() {
    let until = Instant::now() + Duration::from_secs(300);
    let (mut scene, mut smi, mut presence) = (None, None, false);
    while !done() && Instant::now() < until {
        let now = scene_state();
        if now != scene {
            info!("quickstart: +{} ms title state {scene:?} -> {now:?}", ms());
            scene = now;
        }
        let now = smi_state();
        if now != smi {
            info!("quickstart: +{} ms menu state {smi:x?} -> {now:x?}", ms());
            smi = now;
        }
        let now = presence_ready();
        if now != presence {
            info!("quickstart: +{} ms online presence {}", ms(), if now { "up" } else { "gone" });
            presence = now;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    NAVIGATING.store(false, Ordering::Relaxed);
    let timed_out = !done();
    if timed_out {
        // Five minutes without the lobby: the game goes on as it is, the steps dropped.
        GAVE_UP.store(true, Ordering::Relaxed);
        if let Ok(mut nav) = NAV.lock() {
            if let Some(nav) = nav.as_mut() {
                nav.abort();
            }
        }
        NAV_BUSY.store(false, Ordering::Relaxed);
    }
    if aborted() {
        info!("quickstart: +{} ms stopped by the player", ms());
    } else if MODE_CHOSEN.load(Ordering::Relaxed) {
        info!("quickstart: +{} ms done, in the lobby", ms());
    } else if GAVE_UP.load(Ordering::Relaxed) && !timed_out {
        info!("quickstart: online play wasn't available; the game went on without the quick start");
    } else {
        warn!(
            "quickstart: the lobby wasn't reached in 5 minutes (Start pressed: {}, online startup asked: {}, menu state {smi:x?}) -> the game goes on as it is",
            PRESS_SENT.load(Ordering::Relaxed),
            ONLINE_ASKED.load(Ordering::Relaxed)
        );
    }
}
