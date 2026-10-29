//! The launcher in Tauri: the window and the commands the page calls (commands.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

fn main() {
    launcher::logging::init();
    tauri::Builder::default()
        .manage(commands::Launcher::new())
        .invoke_handler(tauri::generate_handler![
            commands::info,
            commands::set_play_mode,
            commands::play,
            commands::questions,
            commands::diagnostics_example,
            commands::answer_diagnostics,
            commands::answer_options,
            commands::activity,
            commands::dismiss,
        ])
        .run(tauri::generate_context!())
        .expect("the launcher's window");
}
