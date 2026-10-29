//! The launcher in Tauri: the window and the commands the page calls (commands.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![commands::info])
        .run(tauri::generate_context!())
        .expect("the launcher's window");
}
