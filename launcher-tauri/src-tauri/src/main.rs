//! The launcher in Tauri: the window and the commands the page calls (commands.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::Manager;

mod chat;
mod commands;
mod feedback;
mod host;
mod intel;
mod settings;
mod support;
mod update;
#[cfg(target_os = "windows")]
mod webview2;
mod whats_new;

/// Where Microsoft offers the WebView2 runtime, for a PC without it.
#[cfg(target_os = "windows")]
const WEBVIEW2_PAGE: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

fn main() {
    // The egui launcher's test that this one starts here (Linux: every library it needs, such as
    // WebKitGTK 4.1, is there): it got this far.
    if std::env::args().nth(1).as_deref() == Some("--probe") {
        return;
    }
    launcher::logging::init();
    // The window is a web view: on Windows it needs Microsoft's WebView2 runtime (part of
    // Windows 11 and of Edge; an older Windows 10 may lack it). Without it, Microsoft's installer
    // (carried in the launcher) puts it in; if that doesn't work (offline, or blocked by the
    // PC's policy), say so and open its download page, rather than fail to open.
    #[cfg(target_os = "windows")]
    if let Err(e) = tauri::webview_version() {
        tracing::warn!("No WebView2 runtime: {e}");
        launcher::logging::show_msgbox(
            "The launcher needs Microsoft's WebView2 runtime (part of Windows 11 and Edge), which this PC doesn't have yet.\n\nPress OK to install it now with Microsoft's installer. It takes a minute and is only needed once.",
            "5th Echelon",
        );
        if !webview2::install() {
            launcher::logging::show_msgbox(
                "Microsoft's WebView2 runtime couldn't be installed (no internet connection, or this PC doesn't allow it).\n\nPress OK to open Microsoft's download page: get the Evergreen Bootstrapper, run it, then start the launcher again.",
                "5th Echelon",
            );
            let _ = std::process::Command::new("explorer").arg(WEBVIEW2_PAGE).spawn();
            return;
        }
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(commands::Launcher::new())
        .manage(feedback::Feedback::default())
        .manage(whats_new::WhatsNew::default())
        .manage(intel::Intel::default())
        .manage(support::Support::default())
        .manage(chat::Chat::default())
        .manage(host::Host::default())
        .manage(update::Update::default())
        .setup(|app| {
            feedback::start(app.handle());
            support::start(app.handle());
            chat::start(app.handle());
            update::start(app.handle());
            commands::told_once(app.handle());
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
            commands::follow_move,
            commands::squad,
            commands::join_friend,
            commands::matches,
            commands::lobbies,
            commands::choose_match_offered,
            commands::join_match,
            commands::new_match,
            commands::svm_modes,
            commands::quick_match,
            commands::cancel_new_match,
            commands::player_action,
            commands::people,
            commands::find_players,
            commands::news,
            commands::switch_to_friend,
            commands::notice,
            feedback::feedback,
            feedback::feedback_file,
            feedback::feedback_send,
            feedback::feedback_close,
            feedback::ask_feedback,
            feedback::report_sending,
            commands::open_url,
            commands::key_art,
            commands::dismiss_emulator_notice,
            commands::ask_name_review,
            commands::account_server,
            commands::account_probe,
            commands::check_name,
            commands::name_suggestions,
            commands::recent_servers,
            commands::usual_name,
            commands::use_network,
            host::host,
            host::host_start,
            host::host_stop,
            host::host_download,
            host::host_connect,
            host::host_disconnect,
            host::host_lists,
            host::host_delete,
            host::host_logs,
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
            settings::use_classic,
            update::check_update,
            update::update_banner,
            update::update_later,
            update::update_now,
            update::update_asked,
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
        .build(tauri::generate_context!())
        .expect("the launcher's window")
        .run(|app, event| {
            // A server started here stops with the launcher.
            if let tauri::RunEvent::Exit = event {
                app.state::<host::Host>().stop();
            }
        });
}
