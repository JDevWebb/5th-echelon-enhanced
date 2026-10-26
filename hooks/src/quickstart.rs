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
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use hooks_addresses::quick_start::Sites;
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
/// The online list's ids (`txt_MP_QM_Ladders`): 1 multiplayer (Spies vs Mercs), 2 Co-op.
const ITEM_SVM: u32 = 1;
const ITEM_COOP: u32 = 2;
/// How many ticks the entry must stand in the list before it's selected.
const STABLE_TICKS: u32 = 30;
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
/// Ticks the wanted entry has stood in the list.
static TARGET_SEEN: AtomicU32 = AtomicU32::new(0);
/// The online list's ids last seen (for the log).
static LAST_IDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Nothing more to do: the lobby was chosen, or the quick start gave up. Every hook then only
/// calls the game's own function, so a later return to the title (a quit, a lost connection)
/// is the game's alone.
fn done() -> bool {
    MODE_CHOSEN.load(Ordering::Relaxed) || GAVE_UP.load(Ordering::Relaxed)
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
    crate::writemem(e.video_gate_jne as *mut u8, &patch);
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
    if !ONLINE_START_PENDING.load(Ordering::Relaxed) {
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
    if PRESS_SENT.load(Ordering::Relaxed) && !done() && smi_state() == Some(SMI_STATE_ONLINE) {
        let target = match LOBBY.get() {
            Some(QuickStartLobby::Coop) => ITEM_COOP,
            _ => ITEM_SVM,
        };
        // SAFETY: main thread, live SMIMatchmaking; every pointer is checked readable.
        unsafe { choose_lobby(this as usize, target) };
    }
    // SAFETY: the original, unchanged.
    unsafe { MatchmakingTickHook.call(this, dt) }
}

unsafe fn choose_lobby(mm: usize, target: u32) {
    if read_u32(mm + MM_ACTIVE).map(|v| v & 0xff) != Some(1) {
        return;
    }
    let Some(list) = read_u32(mm + MM_LIST).filter(|&l| l != 0).map(|l| l as usize) else {
        return;
    };
    if read_u32(list + LIST_PAGE) != Some(0) {
        return;
    }
    let Some(col) = read_u32(list + LIST_COLUMNS).and_then(|c| read_u32(c as usize)).filter(|&c| c != 0).map(|c| c as usize) else {
        return;
    };
    let count = read_u32(col + COL_COUNT).unwrap_or(0).min(32) as usize;
    let Some(items) = read_u32(col + COL_ITEMS).filter(|&i| i != 0).map(|i| i as usize) else {
        return;
    };
    if !readable(items, count * 4) {
        return;
    }
    let e = exe();
    let ids: Vec<u32> = (0..count).map(|i| item_id(e, col, *((items + i * 4) as *const u32))).collect();
    if let Ok(mut last) = LAST_IDS.lock() {
        if *last != ids {
            info!("quickstart: +{} ms online list: ids {ids:?}", ms());
            last.clone_from(&ids);
        }
    }
    let Some(pos) = ids.iter().position(|&id| id == target) else {
        TARGET_SEEN.store(0, Ordering::Relaxed);
        return;
    };
    if TARGET_SEEN.fetch_add(1, Ordering::Relaxed) + 1 < STABLE_TICKS {
        return;
    }
    MODE_CHOSEN.store(true, Ordering::Relaxed);
    *((col + COL_FOCUS) as *mut u32) = pos as u32;
    if e.dx9 {
        let accept: unsafe extern "thiscall" fn(usize) = std::mem::transmute(e.mm_select);
        accept(mm);
    } else {
        let accept: unsafe extern "thiscall" fn(usize, u32) = std::mem::transmute(e.mm_select);
        accept(mm, 0);
    }
    info!(
        "quickstart: +{} ms {} selected (entry {pos})",
        ms(),
        if target == ITEM_COOP { "Co-op" } else { "Spies vs Mercs" }
    );
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
    if MODE_CHOSEN.load(Ordering::Relaxed) {
        info!("quickstart: +{} ms done, in the lobby", ms());
    } else if GAVE_UP.load(Ordering::Relaxed) {
        info!("quickstart: online play wasn't available; the game went on without the quick start");
    } else {
        warn!(
            "quickstart: the lobby wasn't reached in 5 minutes (Start pressed: {}, online startup asked: {}, menu state {smi:x?}) -> the game goes on as it is",
            PRESS_SENT.load(Ordering::Relaxed),
            ONLINE_ASKED.load(Ordering::Relaxed)
        );
    }
}
