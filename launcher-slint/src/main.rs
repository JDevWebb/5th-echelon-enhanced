//! The 5th Echelon launcher in Slint: the Tauri launcher's design with the project's own brand,
//! in one native window (ui.rs), on the egui launcher's library (core.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod core;
mod shots;
mod ui;

fn main() {
    // Every screen in both looks at several sizes, on made-up data (build.sh slint-shots).
    let args: Vec<String> = std::env::args().collect();
    if let Some(at) = args.iter().position(|a| a == "--shots") {
        let dir = args.get(at + 1).map_or_else(|| "shots".into(), std::path::PathBuf::from);
        if let Err(e) = shots::run(dir) {
            eprintln!("shots: {e}");
            std::process::exit(1);
        }
        return;
    }
    launcher::logging::init();
    tracing::info!("5th Echelon launcher (Slint) {}", env!("FE_RELEASE"));
    // OpenGL (femtovg) where there's OpenGL 2 or later; Slint falls back to its software renderer
    // where there isn't (a virtual machine without its graphics driver), or FE_RENDERER=software.
    if std::env::var("FE_RENDERER").as_deref() == Ok("software") && std::env::var_os("SLINT_BACKEND").is_none() {
        std::env::set_var("SLINT_BACKEND", "winit-software");
    }
    if let Err(e) = ui::run() {
        tracing::error!("the launcher's window failed: {e}");
        launcher::logging::show_msgbox(
            &format!("The launcher couldn't open its window: {e}\n\nIf this is a virtual machine, turn on its 3D acceleration, or install its graphics driver."),
            "5th Echelon",
        );
        std::process::exit(1);
    }
}
