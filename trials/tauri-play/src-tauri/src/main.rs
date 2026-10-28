//! The Tauri trial's Rust side: the window, and one command the page asks for what to show (as
//! the real launcher would hand it the server, the checks and friends). Made-up data here.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;

#[derive(Serialize)]
struct Friend {
    name: &'static str,
    activity: &'static str,
    joinable: bool,
}

#[derive(Serialize)]
struct Check {
    title: &'static str,
    detail: &'static str,
    fix: Option<&'static str>,
}

#[derive(Serialize)]
struct Snapshot {
    player: &'static str,
    server: &'static str,
    region: &'static str,
    ping_ms: u32,
    online: u32,
    ready: &'static str,
    checks: Vec<Check>,
    friends: Vec<Friend>,
    news: Vec<&'static str>,
    latest_global: (&'static str, &'static str),
    started_ms: u128,
}

/// When the process started, for the page to say how long it took to show.
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

#[tauri::command]
fn snapshot() -> Snapshot {
    Snapshot {
        player: "Kiwi",
        server: "Falkenstein, Germany",
        region: "eu1.scbl.jdevwebb.net",
        ping_ms: 38,
        online: 24,
        ready: "Ready to play · 1 note",
        checks: vec![Check { title: "Unencrypted connection", detail: "Sign-ins travel readable to this server; ask its operator to set up HTTPS.", fix: None }],
        friends: vec![
            Friend { name: "Tank", activity: "SvM Classic · Penthouse · 3 of 4", joinable: true },
            Friend { name: "Nova", activity: "In the lobby · Sydney", joinable: true },
            Friend { name: "Raven", activity: "Launcher open", joinable: false },
        ],
        news: vec!["0.4.7: chat, friends and sign-in answer straight away again on busy servers.", "0.4.6 \"Comms Check\": chat is here.", "The community network's new address."],
        latest_global: ("Raven", "@Kiwi you hosting tonight?"),
        started_ms: STARTED.get().map_or(0, |t| t.elapsed().as_millis()),
    }
}

fn main() {
    STARTED.get_or_init(std::time::Instant::now);
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![snapshot])
        .run(tauri::generate_context!())
        .expect("the trial's window");
}
