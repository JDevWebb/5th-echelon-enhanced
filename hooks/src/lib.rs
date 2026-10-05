#![feature(unboxed_closures, tuple_trait, c_variadic, once_cell_try, mapped_lock_guards)]
#![deny(clippy::pedantic)]

use std::ffi::c_char;
use std::ffi::CStr;
use std::ffi::CString;
use std::ffi::OsString;
use std::fs::File;
use std::os::raw::c_void;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::path::PathBuf;

use addresses::Addresses;
use hooks_config as config;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::instrument;
use tracing::level_filters::LevelFilter;
use tracing::warn;
use windows::core::PCSTR;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::System::SystemServices::DLL_PROCESS_DETACH;
use windows::Win32::System::SystemServices::DLL_THREAD_ATTACH;
use windows::Win32::System::SystemServices::DLL_THREAD_DETACH;
use windows::Win32::UI::WindowsAndMessaging::MessageBoxA;
use windows::Win32::UI::WindowsAndMessaging::MB_OK;

mod addresses;
mod api;
mod community;
mod dataversion;
mod diagnostics;
mod dll_utils;
mod hooks;
mod macros;
mod overlay;
mod overlay_input;
mod uplay_r1_loader;

use macros::fatal_error;

unsafe fn writemem(ptr: *mut u8, data: &[u8]) {
    if let Ok(_handle) = region::protect_with_handle(ptr, data.len(), region::Protection::READ_WRITE) {
        std::ptr::copy(data.as_ptr(), ptr, data.len());
    }
}

/// Overwrites the C string at `ptr` with `new`, but only if it fits the space
/// the old one had: its bytes plus the zero padding after it. The game's own
/// name is `onlineconfigservice.ubi.com` (27 characters); anything longer would
/// run into whatever follows it.
unsafe fn write_c_string_in_place(ptr: *mut u8, new: &CString) -> bool {
    const MAX_SCAN: usize = 256;
    let mut len = 0;
    while len < MAX_SCAN && *ptr.add(len) != 0 {
        len += 1;
    }
    let mut room = len;
    while room < len + 64 && *ptr.add(room) == 0 {
        room += 1;
    }
    // `room` counts the old string and the zeros after it; the new one needs its NUL too.
    let needed = new.as_bytes_with_nul().len();
    if needed > room {
        error!("The server address {new:?} is too long for the game: at most {} characters here", room.saturating_sub(1));
        return false;
    }
    writemem(ptr, new.as_bytes_with_nul());
    true
}

/// The host's data version check, in a game's join handling: `mov ecx,[global]`,
/// `mov edx,[eax+0x20]` (the joiner's data version), `mov ecx,[ecx+0x70]`,
/// `cmp edx,[ecx+0x418]` (the host's own), then `jz` to accept; otherwise the joiner
/// is refused with DATA_VERSION_MISMATCH. The same bytes in every build of both
/// executables (Ubisoft and Steam DX11, DX9).
const DATA_VERSION_CHECK: [Option<u8>; 19] = [
    Some(0x8B),
    Some(0x0D),
    None,
    None,
    None,
    None, // mov ecx,[global]
    Some(0x8B),
    Some(0x50),
    Some(0x20), // mov edx,[eax+0x20]
    Some(0x8B),
    Some(0x49),
    Some(0x70), // mov ecx,[ecx+0x70]
    Some(0x3B),
    Some(0x91),
    Some(0x18),
    Some(0x04),
    Some(0x00),
    Some(0x00), // cmp edx,[ecx+0x418]
    Some(0x74), // jz accept
];

/// The game's code: its executable's `.text` section in memory.
unsafe fn game_code() -> Option<&'static [u8]> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleA;
    let base = GetModuleHandleA(windows::core::PCSTR::null()).ok()?.0 as *const u8;
    let pe = base.add(std::ptr::read_unaligned(base.add(0x3c).cast::<u32>()) as usize);
    let sections = u16::from_le_bytes([*pe.add(6), *pe.add(7)]) as usize;
    let optional = u16::from_le_bytes([*pe.add(20), *pe.add(21)]) as usize;
    let mut header = pe.add(24 + optional);
    for _ in 0..sections {
        if std::slice::from_raw_parts(header, 8).starts_with(b".text") {
            let size = std::ptr::read_unaligned(header.add(8).cast::<u32>()) as usize;
            let rva = std::ptr::read_unaligned(header.add(12).cast::<u32>()) as usize;
            return Some(std::slice::from_raw_parts(base.add(rva), size));
        }
        header = header.add(40);
    }
    None
}

/// Where the host's data version check starts, if it's found exactly once in the game's
/// code (an unknown build is left alone).
unsafe fn data_version_check() -> Option<*mut u8> {
    let Some(code) = game_code() else {
        warn!("Data version check: the game's code wasn't found");
        return None;
    };
    let matches = |at: &[u8]| at.iter().zip(DATA_VERSION_CHECK).all(|(b, want)| want.is_none_or(|w| *b == w));
    let found: Vec<usize> = code.windows(DATA_VERSION_CHECK.len()).enumerate().filter(|(_, w)| matches(w)).map(|(i, _)| i).collect();
    let [at] = found[..] else {
        warn!("Data version check: found {} times, not once", found.len());
        return None;
    };
    Some(code.as_ptr().add(at).cast_mut())
}

/// Lets a game whose data differs join this one's matches (`AllowDataMismatch`, off by
/// default): turns the host's data version check's `jz` into a `jmp`. Only where the check is
/// found exactly once, so an unknown build is left as it is.
unsafe fn allow_data_mismatch(check: *mut u8) {
    let jz = check.add(DATA_VERSION_CHECK.len() - 1);
    writemem(jz, &[0xEB]);
    info!("Data version check: off at {jz:?} (AllowDataMismatch), so players whose game data differs can join this game's lobbies");
}

unsafe fn patch_url(new_server: &str, addrs: &Addresses) {
    let Ok(new_server_cstr) = CString::new(new_server) else {
        fatal_error!("Invalid config_server");
    };
    let g_cfg_client = addrs.global_onlineconfig_client as *mut *mut *mut u8;
    if g_cfg_client.is_null() {
        fatal_error!(
            "Couldn't adjust config server";
            "Couldn't adjust config server: g_cfg_client is null"
        );
    }
    let g_cfg_client = *g_cfg_client;
    info!("Global onlineconfig client: {:?}", g_cfg_client);
    if g_cfg_client.is_null() {
        info!("Patching rodata with {}", new_server);
        if !write_c_string_in_place(addrs.onlineconfig_url as *mut u8, &new_server_cstr) {
            fatal_error!("The server address is too long for the game; use its IP address or a shorter name (at most 27 characters)");
        }
        return;
    }
    let hostname_ptr = g_cfg_client.add(0x24);
    info!("->hostname: {:?}", hostname_ptr);
    if hostname_ptr.is_null() {
        fatal_error!(
            "Couldn't adjust config server";
            "Couldn't adjust config server: g_cfg_client->hostname is null"
        );
    }
    let hostname_ptr = *hostname_ptr;
    info!("hostname: {:?}", hostname_ptr);
    if hostname_ptr.is_null() {
        fatal_error!(
            "Couldn't adjust config server";
            "Couldn't adjust config server: *g_cfg_client->hostname is null"
        );
    }
    info!("Patching heap with {}", new_server);
    if !write_c_string_in_place(hostname_ptr, &new_server_cstr) {
        fatal_error!("The server address is too long for the game; use its IP address or a shorter name (at most 27 characters)");
    }
}

extern "C" {
    #[cfg(all(target_family = "windows", target_env = "msvc"))]
    fn snprintf(buffer: *mut c_char, count: usize, format: *const c_char, ...) -> i32;
}

unsafe extern "C" fn debug_print(fmt: *const c_char, args: ...) {
    #![allow(clippy::cast_sign_loss)]
    let mut buf: Vec<u8> = vec![0; 4096];

    let mut required = snprintf(buf.as_mut_ptr().cast(), buf.len() - 1, fmt, args.clone()) as usize;
    if required > buf.len() {
        buf.reserve(required);
        required = snprintf(buf.as_mut_ptr().cast(), buf.len() - 1, fmt, args) as usize;
    }

    let formatted = CStr::from_bytes_with_nul(&buf[..=required]);

    panic!("{formatted:?}");
}

unsafe fn enable_debug_print(addrs: &Addresses) {
    let Some((func_ptr, ref to_nop)) = addrs.debug_print else {
        return;
    };

    let target = func_ptr as *mut unsafe extern "C" fn(*const c_char, ...);

    *target = debug_print;

    for addr in to_nop {
        writemem(addr.start as *mut u8, &vec![0xccu8; addr.end - addr.start]);
        // writemem(addr.start as *mut u8, &vec![0x90u8; addr.end - addr.start]);
    }
}

#[instrument]
fn check_orig_library(hmodule: Option<HMODULE>) {
    let myself = get_dll_path(hmodule).expect("couldn't find myself");
    let orig = myself.with_file_name("uplay_r1_loader.orig.dll");

    debug!("Myself: {:?}", myself);
    debug!("Original library path: {:?}", orig);
    if !orig.exists() {
        fatal_error!("Original library not found at {orig:?}");
    }

    let my_info = dll_utils::get_dll_info(myself).expect("couldn't parse myself");
    let orig_info = dll_utils::get_dll_info(orig).expect("couldn't parse uplay_r1_loader.orig.dll");
    info!("My info: {:?}", my_info);
    info!("Orig info: {:?}", orig_info);

    if matches!(orig_info.company.as_deref(), Some("5th Echelon")) {
        fatal_error!("uplay_r1_loader.orig.dll is invalid: Not Ubisoft version");
    }
}

#[instrument(skip_all)]
fn init(hmodule: Option<HMODULE>) {
    let dir = get_target_dir(hmodule);
    community::set_folder(dir.clone());
    let reload_handle = init_log(&dir);
    check_orig_library(hmodule);
    if let Some(cmdline) = get_arguments() {
        info!("Cmdline: {}", cmdline);
    }
    let game_dir = dir.clone();
    let path = config::get_config_path(dir);
    info!("Config path={:?}", path);
    let config = match config::get_or_load(path) {
        Err(e) => {
            fatal_error!("Config couldn't be loaded: {e}");
        }
        Ok(cfg) => cfg,
    };

    let level = match config.logging.level {
        config::LogLevel::Trace => tracing_subscriber::filter::LevelFilter::TRACE,
        config::LogLevel::Debug => tracing_subscriber::filter::LevelFilter::DEBUG,
        config::LogLevel::Info => tracing_subscriber::filter::LevelFilter::INFO,
        config::LogLevel::Warning => tracing_subscriber::filter::LevelFilter::WARN,
        config::LogLevel::Error => tracing_subscriber::filter::LevelFilter::ERROR,
    };
    reload_handle.reload(tracing_subscriber::EnvFilter::default().add_directive(level.into())).unwrap();
    diagnostics::start(config);

    let addr = addresses::get();

    unsafe {
        hooks::init(config, &addr);
        if let Some(config_server) = config.config_server.as_ref() {
            #[cfg(not(feature = "patch-free"))]
            patch_url(config_server, &addr);
        } else {
            info!("Keeping original config server");
        }

        if !config.internal_command_line.is_empty() {
            if let Some(ptr) = addr.unreal_commandline {
                let ptr = ptr as *mut u8;
                let count = if config.internal_command_line.len() > 11 {
                    11
                } else {
                    config.internal_command_line.len()
                };
                ptr.copy_from_nonoverlapping(config.internal_command_line.as_ptr(), count);
            } else {
                error!("Can't set command line. Address is missing");
            }
        }

        enable_debug_print(&addr);
        // Found once, before it's patched (the patch changes the bytes it's found by).
        #[cfg(not(feature = "patch-free"))]
        match data_version_check() {
            Some(check) => {
                if config.allow_data_mismatch {
                    allow_data_mismatch(check);
                }
                // What the data version is made of, to compare two players' (dataversion.rs):
                // only when asked for (Advanced › Hooks); otherwise last time's files go.
                if config.log_data_version {
                    dataversion::write_when_ready(check as usize, game_dir.clone());
                } else {
                    config::remove_data_version_files(&game_dir);
                }
            }
            None => warn!("Data version check: left as it is"),
        }
    }

    // needs to be done in a separate thread, otherwise it'll block indefinitely
    std::thread::Builder::new()
        .name(String::from("login-thread"))
        .spawn(move || {
            // try to login once. relogins are attempted by the update thread later on
            match config.user.secret() {
                Some(password) => {
                    let _ = api::login(&config.user.username, &password);
                }
                // Encrypted for another Windows user or PC (a copied game folder).
                None => tracing::error!("The saved password can't be read here; set up the server again in the launcher"),
            }
        })
        .unwrap();
}

fn deinit(hmodule: Option<HMODULE>) {
    // disable tracing as TLS is already destroyed at this point.
    // No tracing calls/instruments are allowed before this point!!
    let _guard = tracing::dispatcher::set_default(&tracing::dispatcher::Dispatch::none());
    let dir = get_target_dir(hmodule);
    let path = config::get_config_path(dir);
    info!("Config path={:?}", path);
    let config = match config::get_or_load(path) {
        Err(e) => {
            fatal_error!("Config couldn't be loaded: {e}");
        }
        Ok(cfg) => cfg,
    };
    unsafe { hooks::deinit(config) };
}

struct FileWriter(PathBuf);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for FileWriter {
    type Writer = Box<dyn std::io::Write>;

    fn make_writer(&'a self) -> Self::Writer {
        // A log that can't be written (read-only game folder) must not crash
        // the game.
        match File::options().create(true).append(true).open(&self.0) {
            Ok(f) => Box::new(f),
            Err(_) => Box::new(std::io::sink()),
        }
    }
}

fn show_msgbox(msg: &str, caption: &str) {
    let msg = CString::new(msg).unwrap();
    let caption = CString::new(caption).unwrap();
    unsafe {
        MessageBoxA(None, PCSTR(msg.as_ptr().cast::<u8>()), PCSTR(caption.as_ptr().cast::<u8>()), MB_OK);
    }
}

fn get_executable(hinst: HMODULE) -> Option<PathBuf> {
    let mut path = vec![0u16; 4096];
    let sz = unsafe { GetModuleFileNameW(hinst, &mut path) } as usize;
    let err = std::io::Error::last_os_error();
    if err.raw_os_error().unwrap_or_default() != 0 {
        return None;
    };
    let path = OsString::from_wide(&path[..sz]);
    // cannot fail
    Some(PathBuf::from(path))
}

fn get_arguments() -> Option<String> {
    let cmdline = unsafe { windows::Win32::System::Environment::GetCommandLineW().to_string() };
    cmdline.ok()
}

fn get_dll_path(hinst: Option<HMODULE>) -> anyhow::Result<PathBuf> {
    hinst.ok_or(anyhow::anyhow!("no hinst")).and_then(|hinst| {
        let mut path = vec![0u16; 4096];
        let sz = unsafe { GetModuleFileNameW(hinst, &mut path) } as usize;
        let err = std::io::Error::last_os_error();
        if err.raw_os_error().unwrap_or_default() != 0 {
            return Err(err.into());
        };
        let path = OsString::from_wide(&path[..sz]);
        // cannot fail
        let path = PathBuf::from(path);
        Ok(path)
    })
}

fn get_target_dir(hinst: Option<HMODULE>) -> PathBuf {
    get_dll_path(hinst).map_or_else(
        |_| std::env::current_dir().unwrap_or_default(),
        |mut p| {
            p.pop();
            p
        },
    )
}

type ReconfigurableLogger = tracing_subscriber::reload::Handle<
    tracing_subscriber::EnvFilter,
    tracing_subscriber::layer::Layered<
        tracing_subscriber::fmt::Layer<tracing_subscriber::Registry, tracing_subscriber::fmt::format::DefaultFields, tracing_subscriber::fmt::format::Format, FileWriter>,
        tracing_subscriber::Registry,
    >,
>;

fn init_log(target_dir: &Path) -> ReconfigurableLogger {
    let path = target_dir.join("bl-tracing.log");
    // Keep the previous run's log: that's usually the one with the problem.
    let _ = std::fs::rename(&path, target_dir.join("bl-tracing.prev.log"));
    let subscriber_builder = tracing_subscriber::FmtSubscriber::builder()
        .with_writer(FileWriter(path))
        .with_ansi(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_line_number(true)
        .with_env_filter(tracing_subscriber::EnvFilter::builder().with_default_directive(LevelFilter::INFO.into()).from_env_lossy())
        .with_filter_reloading();
    let reload_handle = subscriber_builder.reload_handle();
    {
        use tracing_subscriber::layer::SubscriberExt as _;
        use tracing_subscriber::util::SubscriberInitExt as _;
        // The log file, and the lines the server gets (diagnostics.rs).
        subscriber_builder.finish().with(diagnostics::DiagnosticsLayer).init();
    }
    tracing::event!(tracing::Level::INFO, "attaching");

    std::panic::set_hook(Box::new(|panic_info| {
        let mut expl = String::new();

        let message = match (panic_info.payload().downcast_ref::<&str>(), panic_info.payload().downcast_ref::<String>()) {
            (Some(s), _) => Some((*s).to_string()),
            (_, Some(s)) => Some(s.to_string()),
            (None, None) => None,
        };

        let cause = match message {
            Some(m) => m,
            None => "Unknown".into(),
        };

        match panic_info.location() {
            Some(location) => {
                expl.push_str(&format!("Panic occurred in file '{}' at line {}", location.file(), location.line()));
            }
            None => expl.push_str("Panic location unknown."),
        }
        let msg = format!("{expl}\n{cause}");

        tracing::error!("{}", msg);

        show_msgbox(&msg, "PANIC");

        unsafe {
            std::arch::asm!("int3");
        }

        std::process::exit(1);
    }));
    reload_handle
}

#[no_mangle]
unsafe extern "system" fn DllMain(hinst: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    match reason {
        DLL_PROCESS_ATTACH => init(Some(hinst)),
        DLL_PROCESS_DETACH => deinit(Some(hinst)),
        DLL_THREAD_ATTACH | DLL_THREAD_DETACH => {}
        _ => {
            fatal_error!("Unexpected reason: {reason}");
        }
    };
    BOOL::from(true)
}
