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
    /// The server refuses this launcher's version: update it from the releases.
    UpdateLauncher,
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
    /// The server refuses this launcher's version: what it says to do.
    Outdated(String),
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
    /// The server's API is plain HTTP, and the server isn't on this PC or
    /// its network (see `net::is_local_server`).
    pub unencrypted: bool,
    pub account: Option<AccountFact>,
    /// The pinned adapter, its current address, and the adapter the server
    /// is routed through.
    pub pinned: Option<String>,
    pub pinned_ip: Option<IpAddr>,
    pub route_adapter: Option<String>,
    /// This PC's address on the route to the server (the route adapter's).
    pub route_ip: Option<IpAddr>,
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
        Some(ClientState::Different) => Check::new("client", Status::Fail, "Update 5th Echelon", "Another version is installed. Close the game so the launcher can replace it: servers refuse other versions.", Some(Fix::InstallClient)),
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
            if f.unencrypted {
                checks.push(Check::new(
                    "encryption",
                    Status::Warn,
                    "Unencrypted connection",
                    format!("Your password and sign-ins travel readable to {name}: anyone on the way can read them. Ask its operator to set up HTTPS."),
                    None,
                ));
            }
            Some(*ip)
        }
    };

    if server_ip.is_some() {
        checks.push(match &f.account {
            Some(AccountFact::Ok(name)) => Check::new("account", Status::Ok, format!("Signed in as {name}"), "", None),
            Some(AccountFact::Refused(why)) => Check::new("account", Status::Fail, "Account refused", why.clone(), Some(Fix::SetUpAccount)),
            Some(AccountFact::Outdated(why)) => Check::new("account", Status::Fail, "Update the launcher", why.clone(), Some(Fix::UpdateLauncher)),
            Some(AccountFact::Unknown(why)) => Check::new("account", Status::Warn, "Couldn't check the account", why.clone(), None),
            Some(AccountFact::None) | None => Check::new("account", Status::Fail, "No account on this server yet", "Connect finds yours, or makes one.", Some(Fix::SetUpAccount)),
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

    // A VPN on the way to a server on the internet: it works, but adds delay and sends
    // matches through the server's relay. Only a note: it never stops anyone playing. A
    // server inside the VPN (a group's Radmin network) is what the VPN is for.
    if let (Some(server_ip), Some(route)) = (server_ip.filter(|ip| crate::net::is_public(*ip)), &f.route_adapter) {
        if let Some(vpn) = crate::net::vpn(route, f.route_ip).filter(|v| !v.holds(server_ip)) {
            let what = if vpn.virtual_lan { format!("{} (sending its traffic to the internet)", vpn.name) } else { vpn.name.to_string() };
            checks.push(Check::new(
                "vpn",
                Status::Warn,
                format!("Playing through {}", vpn.name.trim_start_matches("a ")),
                format!(
                    "Your connection to {server_ip} goes through {what}. It works, but adds delay, and your matches go through the server's relay. Turn it off while you play to connect directly, then check again."
                ),
                None,
            ));
        }
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
        // A save the launcher can't read (one changed by another tool, say) is still a save:
        // the check is only that there is one.
        Some(SaveState::Unreadable) => Check::new("save", Status::Ok, "Save game found", "", None),
        Some(SaveState::Missing) | None => Check::new(
            "save",
            Status::Fail,
            "No save game yet",
            "Your Ubisoft Connect save is brought over if there is one, else a new rank 5 save: rank 5 unlocks co-op and Spies vs Mercs.",
            Some(Fix::CreateSave),
        ),
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
            unencrypted: false,
            account: Some(AccountFact::Ok("Kiwi".into())),
            pinned: Some("Game VPN".into()),
            pinned_ip: Some(IpAddr::from([10, 8, 1, 2])),
            route_adapter: Some("Game VPN".into()),
            route_ip: Some(IpAddr::from([10, 8, 1, 2])),
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
        let f = Facts { account: Some(AccountFact::Outdated("update".into())), ..ready_facts() };
        assert_eq!(fix(f, "account"), Some((Status::Fail, Some(Fix::UpdateLauncher))));
        let f = Facts { route_adapter: Some("Ethernet".into()), ..ready_facts() };
        assert_eq!(fix(f, "network"), Some((Status::Fail, Some(Fix::PinAdapter))));
        let f = Facts { pinned: None, pinned_ip: None, ..ready_facts() };
        assert_eq!(fix(f, "network"), Some((Status::Fail, Some(Fix::PinAdapter))));
        let f = Facts { save: Some(SaveState::Unreadable), ..ready_facts() };
        assert_eq!(fix(f, "save"), Some((Status::Ok, None)), "a save it can't read is still a save");
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
    fn plain_http_to_a_public_server_is_a_lasting_warning() {
        let f = Facts { unencrypted: true, ..ready_facts() };
        let checks = checklist(&f);
        let check = checks.iter().find(|c| c.id == "encryption").unwrap();
        assert_eq!(check.status, Status::Warn);
        assert!(ready(&checks), "a warning, not a failure");
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

#[cfg(test)]
mod vpn_check_tests {
    use super::*;

    fn facts(server: [u8; 4], route: &str, route_ip: [u8; 4]) -> Facts {
        Facts {
            game_dir: Some("C:/Game".into()),
            client: Some(ClientState::Installed),
            server: Some(("play.example.com".into(), Some(IpAddr::from(server)))),
            server_ports: Some((true, true)),
            account: Some(AccountFact::Ok("Kiwi".into())),
            pinned: Some(route.into()),
            pinned_ip: Some(IpAddr::from(route_ip)),
            route_adapter: Some(route.into()),
            route_ip: Some(IpAddr::from(route_ip)),
            save: Some(SaveState::Ok { xp: 6600 }),
            ..Facts::default()
        }
    }

    #[test]
    fn a_vpn_on_the_way_to_an_internet_server_is_a_note() {
        let checks = checklist(&facts([139, 99, 171, 113], "NordLynx", [10, 5, 0, 2]));
        let vpn = checks.iter().find(|c| c.id == "vpn").expect("a note about the VPN");
        assert_eq!(vpn.status, Status::Warn);
        assert_eq!(vpn.title, "Playing through NordVPN");
        assert!(vpn.detail.contains("relay"), "{}", vpn.detail);
        assert!(ready(&checks), "a note never stops anyone playing: {checks:#?}");
        let generic = checklist(&facts([139, 99, 171, 113], "wg0", [10, 5, 0, 2]));
        assert_eq!(generic.iter().find(|c| c.id == "vpn").map(|c| c.title.as_str()), Some("Playing through VPN"));
    }

    #[test]
    fn a_server_inside_the_vpn_or_no_vpn_says_nothing() {
        // A group's server on its Radmin network: what the VPN is for.
        assert!(!checklist(&facts([26, 1, 2, 3], "Radmin VPN", [26, 4, 5, 6])).iter().any(|c| c.id == "vpn"));
        // The internet through the PC's own connection, with Radmin merely installed.
        assert!(!checklist(&facts([139, 99, 171, 113], "Ethernet", [192, 168, 1, 20])).iter().any(|c| c.id == "vpn"));
    }
}
