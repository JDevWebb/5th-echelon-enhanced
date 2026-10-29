//! The launcher in Tauri: the window and the commands the page calls (commands.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chat;
mod commands;
mod feedback;
mod intel;
mod settings;
mod support;
mod whats_new;

fn main() {
    launcher::logging::init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(commands::Launcher::new())
        .manage(feedback::Feedback::default())
        .manage(whats_new::WhatsNew::default())
        .manage(intel::Intel::default())
        .manage(support::Support::default())
        .manage(chat::Chat::default())
        .setup(|app| {
            feedback::start(app.handle());
            support::start(app.handle());
            chat::start(app.handle());
            Ok(())
        })
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
            commands::switch_server,
            commands::squad,
            commands::join_friend,
            commands::player_action,
            commands::news,
            commands::switch_to_friend,
            commands::notice,
            feedback::feedback,
            feedback::feedback_file,
            feedback::feedback_send,
            feedback::feedback_close,
            feedback::ask_feedback,
            commands::open_url,
            commands::key_art,
            commands::account_server,
            commands::account_probe,
            commands::check_name,
            commands::name_suggestions,
            commands::recent_servers,
            commands::usual_name,
            commands::use_network,
            settings::settings,
            settings::set,
            settings::profile_password,
            settings::act,
            settings::connection_test,
            settings::identity,
            settings::export_identity,
            settings::import_identity,
            settings::set_directory,
            settings::my_reports,
            settings::check_update,
            chat::chat,
            chat::chat_glance,
            chat::chat_check,
            chat::chat_send,
            chat::chat_delete,
            chat::chat_report,
            chat::chat_act,
            support::support,
            support::support_check,
            support::support_send,
            intel::roadmap,
            intel::suggestion_form,
            intel::check_suggestion,
            intel::suggest,
            intel::my_suggestions,
            whats_new::whats_new,
            whats_new::whats_new_again,
            whats_new::whats_new_seen,
            whats_new::use_community_network,
        ])
        .run(tauri::generate_context!())
        .expect("the launcher's window");
}
