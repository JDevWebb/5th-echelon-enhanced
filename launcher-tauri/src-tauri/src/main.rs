//! The launcher in Tauri: the window and the commands the page calls (commands.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

fn main() {
    launcher::logging::init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            commands::activity_action,
            commands::find_games,
            commands::choose_game,
            commands::pick_game_folder,
            commands::servers,
            commands::find_on_network,
            commands::connect,
            commands::checks,
            commands::fix,
            commands::identify,
            commands::fix_everything,
        ])
        .run(tauri::generate_context!())
        .expect("the launcher's window");
}
