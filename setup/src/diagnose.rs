//! The setup checklist: what's ready, what isn't, and which automatic fix
//! applies. Built from facts the launcher gathers ([`Facts`]) and the hook's
//! log of the last game session.

use std::net::IpAddr;
use std::path::Path;
use std::time::SystemTime;

use crate::install::ClientState;
use crate::save::SaveState;

/// The hook's log of the current (or last) game session, in the game folder.
pub const LOG_FILE: &str = "bl-tracing.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

/// What the launcher can do about a failed check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    FindGame,
    InstallClient,
    ChooseServer,
    SetUpAccount,
    PinAdapter,
    CreateSave,
    RaiseSave,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: &'static str,
    pub status: Status,
    pub title: String,
    pub detail: String,
    pub fix: Option<Fix>,
}

impl Check {
    fn new(id: &'static str, status: Status, title: impl Into<String>, detail: impl Into<String>, fix: Option<Fix>) -> Self {
        Self {
            id,
            status,
            title: title.into(),
            detail: detail.into(),
            fix,
        }
    }
}

/// What the account check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountFact {
    None,
    Ok(String),
    Refused(String),
    Unknown(String),
}

/// Everything the checklist looks at. The launcher fills it in (some of it
/// takes network calls) and calls [`checklist`].
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub game_dir: Option<std::path::PathBuf>,
    pub client: Option<ClientState>,
    /// The chosen server, as typed, and what it resolved to.
    pub server: Option<(String, Option<IpAddr>)>,
    /// Whether the server's API and config (80) ports answered.
    pub server_ports: Option<(bool, bool)>,
    /// The server's API port (50051 unless the server says otherwise).
    pub api_port: Option<u16>,
    pub account: Option<AccountFact>,
    /// The pinned adapter, its current address, and the adapter the server
    /// is routed through.
    pub pinned: Option<String>,
    pub pinned_ip: Option<IpAddr>,
    pub route_adapter: Option<String>,
    pub save: Option<SaveState>,
    pub log: Option<LogFacts>,
    /// Set when the game runs under Wine or Proton.
    pub wine: Option<WineFacts>,
}

/// The Wine or Proton side of things (Linux, Steam Deck).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineFacts {
    /// Steam's Proton (not Lutris, Heroic or plain Wine).
    pub steam: bool,
    /// Whether the prefix exists yet (Proton makes it on the first run).
    pub prefix_ready: bool,
    /// Steam launch options this CPU needs, if any (see `wine::steam_launch_options`).
    pub launch_options: Option<String>,
}

/// What the hook logged in the last game session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFacts {
    pub started: Option<SystemTime>,
    /// "Enforcing <ip> for networking": the address the game used.
    pub enforcing: Option<IpAddr>,
    /// The pinned adapter wasn't there.
    pub adapter_missing: Option<String>,
    pub hook_errors: Vec<String>,
    pub panicked: bool,
    pub attached: bool,
}

pub fn parse_log(text: &str) -> LogFacts {
    let mut facts = LogFacts::default();
    for line in text.lines() {
        if let Some(rest) = line.split("Enforcing ").nth(1) {
            facts.enforcing = rest.split_whitespace().next().and_then(|ip| ip.parse().ok()).or(facts.enforcing);
        }
        if let Some(rest) = line.split("No adapter ").nth(1) {
            facts.adapter_missing = rest.split('"').nth(1).map(str::to_string);
        }
        if line.contains("Address for hook") && line.contains("missing") || line.contains("Hook ") && line.contains(" failed") {
            if facts.hook_errors.len() < 5 {
                facts.hook_errors.push(line.trim().to_string());
            }
        }
        facts.panicked |= line.contains("panicked at");
        facts.attached |= line.contains("attaching");
    }
    facts
}

pub fn read_log(game_dir: &Path) -> Option<LogFacts> {
    let path = game_dir.join(LOG_FILE);
    let text = std::fs::read(&path).ok()?;
    let mut facts = parse_log(&String::from_utf8_lossy(&text));
    facts.started = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    Some(facts)
}

/// The checklist, in the order the setup runs.
pub fn checklist(f: &Facts) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(_) = &f.game_dir else {
        checks.push(Check::new("game", Status::Fail, "Game not found", "Find Splinter Cell: Blacklist, or choose its folder.", Some(Fix::FindGame)));
        return checks;
    };
    checks.push(Check::new("game", Status::Ok, "Game found", "", None));

    if let Some(wine) = &f.wine {
        if !wine.prefix_ready {
            let from = if wine.steam { "Steam" } else { "Lutris, Heroic or Wine" };
            checks.push(Check::new(
                "prefix",
                Status::Fail,
                "Start the game once first",
                format!("Start Splinter Cell: Blacklist once from {from} and quit it, so its Wine files exist; then check again."),
                None,
            ));
        }
        if let (true, Some(options)) = (wine.steam, &wine.launch_options) {
            checks.push(Check::new(
                "cpu",
                Status::Warn,
                "Set the game's Steam launch options",
                format!(
                    "This CPU has more than {} threads, and the game can freeze at start under Proton. In Steam: Splinter Cell: Blacklist › Properties › Launch options: {options}",
                    crate::wine::MAX_THREADS
                ),
                None,
            ));
        }
    }

    checks.push(match f.client {
        Some(ClientState::Installed) => Check::new("client", Status::Ok, "5th Echelon installed", "", None),
        Some(ClientState::Different) => Check::new("client", Status::Fail, "Update 5th Echelon", "A different version is installed.", Some(Fix::InstallClient)),
        Some(ClientState::NotInstalled) => Check::new("client", Status::Fail, "Install 5th Echelon", "The game still has Ubisoft's online DLL.", Some(Fix::InstallClient)),
        Some(ClientState::NoGameDll) | None => Check::new(
            "client",
            Status::Fail,
            "Game files incomplete",
            "uplay_r1_loader.dll is missing. Verify the game's files in Steam or Ubisoft Connect.",
            None,
        ),
    });

    let server_ip = match &f.server {
        None => {
            checks.push(Check::new("server", Status::Fail, "Choose a server", "", Some(Fix::ChooseServer)));
            None
        }
        Some((name, None)) => {
            checks.push(Check::new("server", Status::Fail, "Server not found", format!("\"{name}\" doesn't resolve to an address."), Some(Fix::ChooseServer)));
            None
        }
        Some((name, Some(ip))) => {
            checks.push(match f.server_ports {
                Some((true, true)) | None => Check::new("server", Status::Ok, format!("Server {name}"), "", None),
                Some((api, config)) => {
                    let api_port = f.api_port.unwrap_or(crate::API_PORT).to_string();
                    let down: Vec<&str> = [(!api).then_some(api_port.as_str()), (!config).then_some("80")].into_iter().flatten().collect();
                    Check::new(
                        "server",
                        Status::Fail,
                        "Server not answering",
                        format!("{ip} doesn't answer on port {}. Check it's running and that you're connected to it.", down.join(" or ")),
                        None,
                    )
                }
            });
            Some(*ip)
        }
    };

    if server_ip.is_some() {
        checks.push(match &f.account {
            Some(AccountFact::Ok(name)) => Check::new("account", Status::Ok, format!("Signed in as {name}"), "", None),
            Some(AccountFact::Refused(why)) => Check::new("account", Status::Fail, "Account refused", why.clone(), Some(Fix::SetUpAccount)),
            Some(AccountFact::Unknown(why)) => Check::new("account", Status::Warn, "Couldn't check the account", why.clone(), None),
            Some(AccountFact::None) | None => Check::new("account", Status::Fail, "Set up an account", "", Some(Fix::SetUpAccount)),
        });
    }

    if let Some(server_ip) = server_ip.filter(|ip| !ip.is_loopback()) {
        checks.push(match (&f.pinned, &f.route_adapter) {
            (Some(pinned), _) if f.pinned_ip.is_none() => Check::new(
                "network",
                Status::Fail,
                "Network adapter missing",
                format!("\"{pinned}\" isn't connected. Connect it (e.g. turn your VPN on), or pick the adapter again."),
                Some(Fix::PinAdapter),
            ),
            (Some(pinned), Some(route)) if !hooks_config::adapter_name_matches(route, pinned) => Check::new(
                "network",
                Status::Fail,
                "Wrong network adapter",
                format!("The game is pinned to \"{pinned}\", but {server_ip} is reached through \"{route}\"."),
                Some(Fix::PinAdapter),
            ),
            (Some(pinned), _) => Check::new("network", Status::Ok, format!("Playing over \"{pinned}\""), "", None),
            (None, Some(route)) => Check::new(
                "network",
                Status::Fail,
                "Network adapter not set",
                format!("Pin \"{route}\" so other players can join you."),
                Some(Fix::PinAdapter),
            ),
            (None, None) => Check::new("network", Status::Warn, "Network adapter not set", "Couldn't tell which adapter reaches the server.", None),
        });
    }

    // Without the Wine prefix there's nowhere for a save yet ("prefix" above).
    let prefix_missing = f.wine.as_ref().is_some_and(|w| !w.prefix_ready);
    if !prefix_missing {
        checks.push(save_check(f));
    }

    if let Some(log) = &f.log {
        if let Some(adapter) = &log.adapter_missing {
            checks.push(Check::new(
                "session",
                Status::Fail,
                "Last game: adapter missing",
                format!("The game couldn't find \"{adapter}\"."),
                Some(Fix::PinAdapter),
            ));
        } else if let (Some(used), Some(pinned)) = (log.enforcing, f.pinned_ip) {
            if used != pinned {
                checks.push(Check::new(
                    "session",
                    Status::Warn,
                    "Last game used another address",
                    format!("It played over {used}; the pinned adapter is now {pinned}. Start the game again."),
                    None,
                ));
            }
        }
        if !log.hook_errors.is_empty() {
            checks.push(Check::new("hooks", Status::Warn, "Last game: some hooks failed", log.hook_errors.join("\n"), None));
        }
        if log.panicked {
            checks.push(Check::new("crash", Status::Warn, "Last game: the client crashed", format!("See {LOG_FILE} in the game folder."), None));
        }
    }
    checks
}

fn save_check(f: &Facts) -> Check {
    match &f.save {
        Some(SaveState::Ok { xp }) if *xp < crate::save::RANK5_XP => Check::new(
            "save",
            Status::Fail,
            "Save below rank 5",
            "Co-op and Spies vs Mercs need rank 5.",
            Some(Fix::RaiseSave),
        ),
        Some(SaveState::Ok { .. }) => Check::new("save", Status::Ok, "Save game ready", "", None),
        Some(SaveState::Unreadable) => Check::new("save", Status::Warn, "Save game not recognised", "It's left as it is.", None),
        Some(SaveState::Missing) | None => Check::new("save", Status::Fail, "No save game yet", "A rank 5 save unlocks co-op and Spies vs Mercs.", Some(Fix::CreateSave)),
    }
}

/// Whether the player can press Play: nothing failed.
pub fn ready(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.status != Status::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = r#"2026-09-30T10:00:00Z  INFO hooks: attaching
2026-09-30T10:00:01Z  WARN hooks_config: No adapter "Game VPN" found
2026-09-30T10:00:01Z  INFO hooks_config: Enforcing 10.8.1.2 for networking
2026-09-30T10:00:02Z ERROR hooks::hooks: Hook GetAdaptersInfo failed: NotFound
thread '<unnamed>' panicked at hooks/src/overlay.rs:10:5"#;

    #[test]
    fn reads_the_hooks_log() {
        let f = parse_log(LOG);
        assert!(f.attached && f.panicked);
        assert_eq!(f.enforcing, Some(IpAddr::from([10, 8, 1, 2])));
        assert_eq!(f.adapter_missing.as_deref(), Some("Game VPN"));
        assert_eq!(f.hook_errors.len(), 1);
    }

    fn ready_facts() -> Facts {
        Facts {
            game_dir: Some("C:/Game".into()),
            client: Some(ClientState::Installed),
            server: Some(("10.8.0.10".into(), Some(IpAddr::from([10, 8, 0, 10])))),
            server_ports: Some((true, true)),
            api_port: None,
            account: Some(AccountFact::Ok("Kiwi".into())),
            pinned: Some("Game VPN".into()),
            pinned_ip: Some(IpAddr::from([10, 8, 1, 2])),
            route_adapter: Some("Game VPN".into()),
            save: Some(SaveState::Ok { xp: 6600 }),
            log: None,
            wine: None,
        }
    }

    #[test]
    fn a_ready_setup() {
        let checks = checklist(&ready_facts());
        assert!(ready(&checks), "{checks:#?}");
        assert_eq!(checks.iter().map(|c| c.id).collect::<Vec<_>>(), ["game", "client", "server", "account", "network", "save"]);
    }

    #[test]
    fn each_problem_has_its_fix() {
        let fix = |f: Facts, id: &str| checklist(&f).into_iter().find(|c| c.id == id).map(|c| (c.status, c.fix));
        assert_eq!(fix(Facts::default(), "game"), Some((Status::Fail, Some(Fix::FindGame))));
        let f = Facts { client: Some(ClientState::Different), ..ready_facts() };
        assert_eq!(fix(f, "client"), Some((Status::Fail, Some(Fix::InstallClient))));
        let f = Facts { server: None, ..ready_facts() };
        assert_eq!(fix(f.clone(), "server"), Some((Status::Fail, Some(Fix::ChooseServer))));
        assert_eq!(fix(f, "account"), None, "no account check without a server");
        let f = Facts { account: Some(AccountFact::Refused("wrong password".into())), ..ready_facts() };
        assert_eq!(fix(f, "account"), Some((Status::Fail, Some(Fix::SetUpAccount))));
        let f = Facts { route_adapter: Some("Ethernet".into()), ..ready_facts() };
        assert_eq!(fix(f, "network"), Some((Status::Fail, Some(Fix::PinAdapter))));
        let f = Facts { pinned: None, pinned_ip: None, ..ready_facts() };
        assert_eq!(fix(f, "network"), Some((Status::Fail, Some(Fix::PinAdapter))));
        let f = Facts { save: Some(SaveState::Ok { xp: 10 }), ..ready_facts() };
        assert_eq!(fix(f, "save"), Some((Status::Fail, Some(Fix::RaiseSave))));
        let f = Facts { server_ports: Some((true, false)), ..ready_facts() };
        assert_eq!(fix(f, "server"), Some((Status::Fail, None)));
        let f = Facts {
            server: Some(("localhost".into(), Some(IpAddr::from([127, 0, 0, 1])))),
            pinned: None,
            pinned_ip: None,
            ..ready_facts()
        };
        assert!(ready(&checklist(&f)), "a server on this PC needs no pin");
    }

    #[test]
    fn a_moved_api_port_is_named() {
        let f = Facts {
            server_ports: Some((false, true)),
            api_port: Some(8443),
            ..ready_facts()
        };
        let server = checklist(&f).into_iter().find(|c| c.id == "server").unwrap();
        assert!(server.detail.contains("port 8443"), "{}", server.detail);
    }

    #[test]
    fn proton_needs_its_prefix_and_maybe_launch_options() {
        let wine = |prefix_ready, launch_options: Option<&str>| Facts {
            wine: Some(WineFacts {
                steam: true,
                prefix_ready,
                launch_options: launch_options.map(String::from),
            }),
            ..ready_facts()
        };
        let ids = |f: Facts| checklist(&f).into_iter().filter(|c| c.status != Status::Ok).map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids(wine(true, None)), Vec::<&str>::new());
        assert_eq!(ids(wine(false, None)), ["prefix"]);
        assert!(!ready(&checklist(&wine(false, None))));
        assert_eq!(ids(wine(true, Some("WINE_CPU_TOPOLOGY=16:0 %command%"))), ["cpu"]);
        assert!(ready(&checklist(&wine(true, Some("x")))), "a warning, not a failure");
    }
}
