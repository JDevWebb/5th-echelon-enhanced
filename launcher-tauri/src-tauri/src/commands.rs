//! What the page asks the Rust side for. Read from the same files the egui launcher keeps, so
//! the two show the same game, server and account.

use std::path::PathBuf;

use serde::Serialize;

const PRODUCT: &str = "5th Echelon Enhanced";
const RELEASE: &str = env!("FE_RELEASE");

/// The launcher's own settings (`launcher.toml`), what of them this needs.
#[derive(serde::Deserialize, Default)]
struct Prefs {
    game_dir: Option<PathBuf>,
    directory: Option<String>,
    /// The player chose a directory (or none): without it, the community network's.
    #[serde(default)]
    directory_set: bool,
}

fn prefs() -> Prefs {
    setup::app_data_dir()
        .map(|d| d.join("launcher.toml"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// The release, the game folder, and the server and account the game is set up for.
#[derive(Serialize)]
pub struct Info {
    product: &'static str,
    release: &'static str,
    game_dir: Option<String>,
    server: Option<String>,
    account: Option<String>,
    /// The launcher uses the community network (Chat, Support and the Roadmap are its).
    community: bool,
}

#[tauri::command]
pub fn info() -> Info {
    let prefs = prefs();
    // The folder chosen, or the first found, as the egui launcher picks it.
    let game_dir = prefs
        .game_dir
        .filter(|d| setup::game::is_game_dir(d))
        .or_else(|| setup::game::find_game_dirs().into_iter().next());
    let profile = game_dir.as_ref().and_then(|d| setup::config::Config::load(d).current_profile().cloned());
    let directory = prefs.directory.or_else(|| (!prefs.directory_set).then(|| setup::directory::COMMUNITY.to_string()));
    let community = directory.as_deref().is_some_and(setup::directory::is_community);
    Info {
        product: PRODUCT,
        release: RELEASE,
        game_dir: game_dir.map(|d| d.display().to_string()),
        server: profile.as_ref().map(|p| p.server.clone()).filter(|s| !s.is_empty()),
        account: profile.map(|p| p.user.username).filter(|u| !u.is_empty()),
        community,
    }
}
