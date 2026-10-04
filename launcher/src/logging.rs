//! Initializes the logging framework and sets up a panic handler.
//!
//! This module configures `tracing-subscriber` for structured logging and
//! sets a custom panic hook to display panic information in a message box,
//! ensuring that users are notified of critical errors.

use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::Layer as _;

/// Shows a Windows message box with the specified message and caption.
#[cfg(target_os = "windows")]
pub fn show_msgbox(msg: &str, caption: &str) {
    use std::ffi::CString;

    use windows::core::PCSTR;
    use windows::Win32::UI::WindowsAndMessaging::MessageBoxA;
    use windows::Win32::UI::WindowsAndMessaging::MB_OK;

    let msg = CString::new(msg).unwrap_or_default();
    let caption = CString::new(caption).unwrap_or_default();
    unsafe {
        MessageBoxA(None, PCSTR(msg.as_ptr().cast::<u8>()), PCSTR(caption.as_ptr().cast::<u8>()), MB_OK);
    }
}

/// Elsewhere (development builds), stderr is enough.
#[cfg(not(target_os = "windows"))]
pub fn show_msgbox(_msg: &str, _caption: &str) {}

/// Attaches the process to the parent console, enabling console output.
fn enable_console() {
    #[cfg(target_os = "windows")]
    unsafe {
        // This allows the launcher to print to the console when run from a terminal.
        let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

/// Sets a custom panic hook to display panic information in a message box.
fn catch_panics() {
    std::panic::set_hook(Box::new(|panic_info| {
        let mut expl = String::new();

        // Extract the panic message.
        let message = match (panic_info.payload().downcast_ref::<&str>(), panic_info.payload().downcast_ref::<String>()) {
            (Some(s), _) => Some((*s).to_string()),
            (_, Some(s)) => Some(s.to_string()),
            (None, None) => None,
        };

        let cause = match message {
            Some(m) => m,
            None => "Unknown".into(),
        };

        // Get the location of the panic.
        match panic_info.location() {
            Some(location) => expl.push_str(&format!("Panic occurred in file '{}' at line {}", location.file(), location.line())),
            None => expl.push_str("Panic location unknown."),
        }
        let msg = format!("{}\n{}", expl, cause);
        eprintln!("PANIC: {msg}");

        // A background task's panic ends that task: the screen waiting on it shows an error.
        if crate::task::in_task() {
            tracing::error!("A background task panicked: {msg}");
            return;
        }

        // Show the panic information in a message box.
        show_msgbox(&msg, "PANIC");

        std::process::exit(1);
    }));
}

/// The launcher's log file (in its data folder), for players' problem reports
/// (feedback.rs); the run before's is kept beside it.
pub fn log_path() -> Option<std::path::PathBuf> {
    setup::app_data_dir().map(|d| d.join("launcher.log"))
}

/// Initializes the logging and panic handling for the application: warnings and errors to
/// the console, and everything from info up to [`log_path`].
pub fn init() {
    enable_console();
    catch_panics();

    let file = log_path().and_then(|path| {
        let _ = std::fs::create_dir_all(path.parent()?);
        let _ = std::fs::rename(&path, path.with_file_name("launcher.prev.log"));
        std::fs::File::create(&path).ok()
    });
    let file_layer = file.map(|f| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(f))
            .with_filter(tracing_subscriber::filter::LevelFilter::INFO)
    });
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer().with_filter(
                tracing_subscriber::EnvFilter::builder()
                    .with_default_directive(tracing_subscriber::filter::LevelFilter::WARN.into())
                    .from_env_lossy(),
            ),
        )
        .with(file_layer)
        .init();
}
