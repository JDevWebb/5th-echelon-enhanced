//! The Support screen: the player's conversation with the community network's admins
//! (`setup::support`), signed with their identity. They write, with the game's or the
//! launcher's log if they choose (private details hidden first, as reports do); the admins'
//! answers show here, and a notice says when one came. The overlay says so in the game too
//! (through the player's server).

use std::time::Duration;

use setup::support::Thread;

use crate::prefs::Prefs;

/// Where support lives: the community network's coordinator.
/// How often the launcher looks for answers: while the screen shows, and otherwise.
pub const LOOK_WHILE_OPEN: Duration = Duration::from_secs(60);
pub const LOOK_EVERY: Duration = Duration::from_secs(10 * 60);

/// A message as sent, to send again.
#[derive(Debug, Clone)]
pub struct Draft {
    pub text: String,
    pub game_log: bool,
    pub launcher_log: bool,
    pub game_dir: Option<std::path::PathBuf>,
    pub server: Option<String>,
    pub name: String,
}

pub fn read(read: bool) -> Result<(Thread, bool), String> {
    let identity = crate::roadmap::load_identity()?;
    let url = setup::support::mine_url(setup::directory::community(), &identity, identity::now(), read);
    crate::services::rt().block_on(async {
        let resp = crate::roadmap::client()?
            .get(url)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(crate::roadmap::unreachable)?;
        let body = crate::roadmap::body(resp).await?;
        let thread = setup::support::parse_thread(&body).map_err(|_| "The project's server sent an answer the launcher can't read.".to_string())?;
        Ok((thread, read))
    })
}

pub fn send(draft: &Draft) -> Result<(), String> {
    let identity = crate::roadmap::load_identity()?;
    // The files, as reports send them: private details hidden, logs cut to their end.
    let files = if draft.game_log || draft.launcher_log {
        let keep: Vec<std::net::Ipv4Addr> = draft
            .server
            .as_deref()
            .map(setup::net::resolve)
            .into_iter()
            .flatten()
            .filter_map(|ip| match ip {
                std::net::IpAddr::V4(v4) => Some(v4),
                std::net::IpAddr::V6(_) => None,
            })
            .collect();
        let private = setup::feedback::Private::of_this_pc(keep).showing(&draft.name);
        let launcher_log = if draft.launcher_log { crate::logging::log_path() } else { None };
        let mut files = match &draft.game_dir {
            Some(dir) => setup::feedback::prepare(dir, launcher_log.as_deref(), &[], &private),
            // No game folder found yet: the launcher's log alone.
            None => launcher_log
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| setup::feedback::attach("launcher.log", &String::from_utf8_lossy(&b), &private).ok())
                .into_iter()
                .collect(),
        };
        if !draft.game_log {
            files.retain(|f| !f.name.starts_with("bl-"));
        }
        files
    } else {
        Vec::new()
    };
    let sender = setup::support::Sender {
        name: &draft.name,
        server: draft.server.as_deref(),
        launcher: env!("FE_RELEASE"),
    };
    let json = setup::support::message_body(setup::directory::community(), &identity, &sender, &draft.text, &files, identity::now());
    crate::services::rt().block_on(async {
        // Files take a while on a slow line: longer than a read.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .post(format!("{}/v1/support", setup::directory::community()))
            .header("content-type", "application/json")
            .body(json.to_string())
            .send()
            .await
            .map_err(crate::roadmap::unreachable)?;
        crate::roadmap::body(resp).await.map(|_| ())
    })
}

/// Whether Support is offered: on the community network, once there's an identity.
pub fn available() -> bool {
    Prefs::directory().is_some_and(|d| Prefs::is_community(&d)) && crate::roadmap::has_identity()
}

/// The Support screen.
/// The help pages beside the conversation.
pub const MANUAL: [(&str, &str); 3] = [
    ("Troubleshooting", "https://github.com/JDevWebb/5th-echelon-enhanced/wiki/Troubleshooting"),
    ("Error messages", "https://github.com/JDevWebb/5th-echelon-enhanced/wiki/Error-Messages"),
    ("Antivirus warnings", "https://github.com/JDevWebb/5th-echelon-enhanced/wiki/Antivirus-Warnings"),
];
