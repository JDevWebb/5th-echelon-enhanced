//! The Settings screen: everything the automatic setup decides, for players
//! who want to decide it themselves, plus saves, connection tests and the
//! hook's switches.

use std::time::Duration;

use crate::activity::Activities as Notices;
use crate::prefs::Game;

/// Each check's name, how it went, and what to say about it.
pub type TestResults = Vec<(&'static str, setup::diagnose::Status, Option<String>)>;

/// The parts of Settings, one shown at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Game,
    Network,
    Servers,
    Identity,
    Notifications,
    Save,
    Client,
    Advanced,
    Display,
    Feedback,
    About,
}

impl Section {
    pub const ALL: [Self; 11] = [
        Self::Game,
        Self::Network,
        Self::Servers,
        Self::Identity,
        Self::Notifications,
        Self::Save,
        Self::Client,
        Self::Advanced,
        Self::Display,
        Self::Feedback,
        Self::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Game => "Game",
            Self::Network => "Network",
            Self::Servers => "Servers and accounts",
            Self::Identity => "Identity and friends",
            Self::Notifications => "Notifications",
            Self::Save => "Save game",
            Self::Client => "5th Echelon client",
            Self::Advanced => "Advanced",
            Self::Display => "Display",
            Self::Feedback => "Feedback",
            Self::About => "About",
        }
    }
}

/// Pins the game to a network adapter (None: automatic, the one that reaches the server), for
/// the current server's account and the game's settings.
pub fn set_adapter(game: &mut Game, notices: &mut Notices, adapter: Option<String>) {
    game.update(notices, |c| {
        if let Some(mut p) = c.current_profile().cloned() {
            p.adapter = adapter.clone();
            c.upsert_profile(p.clone());
            c.apply_profile(&p);
        } else {
            c.hook_config.networking.adapter = adapter.clone();
        }
        if adapter.is_none() {
            c.hook_config.networking.require_adapter = false;
        }
    });
}

/// The adapter choice that pins nothing.
pub const AUTOMATIC: &str = "Automatic (the one that reaches the server)";

/// What the game last ran with, beside the Renderer setting (the 0.5.0 plan's P3), and whether
/// it isn't what was chosen: from the add-on's note of its last start (`bl-last-run.txt`).
pub fn renderer_note(game_dir: &std::path::Path, chosen: setup::game::GameVersion) -> Option<(String, bool)> {
    let last = hooks_config::last_run::read(game_dir)?;
    let ran = if last.dx9() { "DirectX 9" } else { "DirectX 11" };
    let wanted_dx9 = chosen == setup::game::GameVersion::SplinterCellBlacklistDx9;
    if last.dx9() == wanted_dx9 {
        return Some((format!("The game last ran with {ran}."), false));
    }
    let why = if last.launcher {
        "the Renderer changed since: it applies from the next start."
    } else if last.steam {
        "Steam started it and chose that itself (its Play menu or launch options). Pick the other in Steam's Play menu, or start the game with Play here and Start through Steam off."
    } else {
        "it was started from somewhere else (a shortcut or Ubisoft Connect), which chooses its own. Start it with Play here for the Renderer to hold."
    };
    Some((format!("The game last ran with {ran}: {why}"), !last.launcher))
}

/// The "Start without Ubisoft Connect" switch (the 0.5.0 plan's P4): Some(true) where it's
/// offered, Some(false) where it will be once a start through Ubisoft Connect worked on this copy
/// (so it's for the copy the player owns), None where it means nothing (Wine and Proton start
/// without it already).
/// Whether Settings › Game shows "Start without Ubisoft Connect": on Windows (not under Wine,
/// where the game always starts without it). On by default from 0.5.0, so it's offered
/// whether or not a start through Ubisoft Connect worked (it was offered only then).
pub fn ubisoft_connect_offer(_game_dir: &std::path::Path, _on: bool) -> Option<bool> {
    (cfg!(target_os = "windows") && !hooks_config::running_under_wine()).then_some(true)
}

pub const WITHOUT_UBISOFT_CONNECT: &str = "Start without Ubisoft Connect";
pub const WITHOUT_UBISOFT_CONNECT_ON: &str = "On by default: the game starts without opening Ubisoft Connect. Ubisoft's own overlay isn't there; everything 5th Echelon does works the same. Turn it off to start through Ubisoft Connect.";
/// Said if it's asked for where there's no choice: under Wine the game always starts without it.
pub const WITHOUT_UBISOFT_CONNECT_LATER: &str = "On Linux and the Steam Deck the game always starts without Ubisoft Connect.";

#[cfg(test)]
mod renderer_note_tests {
    use setup::game::GameVersion;

    use super::renderer_note;

    fn note(text: &str, chosen: GameVersion) -> Option<(String, bool)> {
        let dir = std::env::temp_dir().join(format!("fes-renderer-note-{}-{}", std::process::id(), text.len()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(hooks_config::last_run::FILE), text).unwrap();
        let n = renderer_note(&dir, chosen);
        let _ = std::fs::remove_dir_all(&dir);
        n
    }

    #[test]
    fn says_what_ran_and_why_it_differs() {
        let (text, warn) = note("exe=Blacklist_game.exe\nsteam=false\nlauncher=true\n", GameVersion::SplinterCellBlacklistDx9).unwrap();
        assert_eq!((text.as_str(), warn), ("The game last ran with DirectX 9.", false));
        let (text, warn) = note("exe=Blacklist_DX11_game.exe\nsteam=true\nlauncher=false\n", GameVersion::SplinterCellBlacklistDx9).unwrap();
        assert!(warn && text.starts_with("The game last ran with DirectX 11: Steam started it"), "{text}");
        let (_, warn) = note("exe=Blacklist_DX11_game.exe\nsteam=false\nlauncher=true\n\n", GameVersion::SplinterCellBlacklistDx9).unwrap();
        assert!(!warn, "changed since, not a problem");
    }
}

/// What a beta feature's switch says, and its hover text, in the order of
/// `hooks_config::Beta::NAMES`.
pub const BETA_FEATURES: [(&str, &str); 3] = [
    (
        "Mouse turning the same at any frame rate",
        "Above 60 fps the game turns less for the same mouse movement, so turning feels slow and players cap their frame rate. \
         This scales the mouse's turn by the frame rate, so it turns as it does at 60 fps. Only the mouse: controllers are left as they are.",
    ),
    (
        "No camera shake",
        "The camera no longer shakes from explosions, hits and the like. Only your view: nothing changes for the other players.",
    ),
    (
        "Wider field of view",
        "You see 10 degrees more to each side than the game's own view. Only your view: nothing changes for the other players.",
    ),
];

/// Makes `identity` this PC's, keeping the one it replaces: whether it did.
pub fn replace_identity(identity: &setup::player_identity::Identity, notices: &mut Notices) -> bool {
    match setup::player_identity::replace(identity) {
        Ok(kept) => {
            let kept = kept.map(|p| format!(" The one it replaced is kept in {}.", p.display())).unwrap_or_default();
            notices.info(format!(
                "This PC now uses the identity {}. Press Connect on each server again to sign in with it.{kept}",
                identity::short(&identity.global_id())
            ));
            // The checklist signs in again with the new identity's accounts.
            crate::flow::forget_account_check();
            true
        }
        Err(e) => {
            notices.error(e.to_string());
            false
        }
    }
}

/// The game service's answer to the test's sign-in. The server lets a name sign in there only
/// after its game client signed in to the API in the last day (the launcher's sign-in doesn't
/// vouch for the game: servers since 0.3.156), so a player who hasn't played today is refused
/// with the right password: with the account's sign-in fine just before, that's the server
/// answering as it should, not a failure (the Surface, 16 Nov 2026).
fn game_sign_in_status(result: Result<(), String>, account_ok: bool) -> (setup::diagnose::Status, Option<String>) {
    use setup::diagnose::Status;
    match result {
        Ok(()) => (Status::Ok, None),
        Err(e) if account_ok && e.contains("AccessDenied") => (
            Status::Ok,
            Some("The server answers; it lets the game itself sign in once the game has started and checked in".into()),
        ),
        Err(e) => (Status::Fail, Some(e)),
    }
}

/// What "How players reach this PC" says when the game's port is held.
const PORT_HELD: &str = "Not tested: another program holds the game's match port (see below)";

/// Whether a test's error is the game's port being held by another program (Windows' 10048,
/// WSAEADDRINUSE; elsewhere "address in use").
fn port_in_use(e: &str) -> bool {
    e.contains("10048") || e.to_ascii_lowercase().contains("address in use") || e.contains("AddrInUse")
}

pub fn run_tests(profile: &setup::config::Profile, game_nat_port: Option<u16>, nat_mode: hooks_config::NatMode, game_dir: &std::path::Path) -> TestResults {
    use crate::network;
    let rt = crate::services::rt();
    let api = profile.api_server_url().to_string();
    let secret = profile.user.secret().unwrap_or_default();
    let (user, pass) = (profile.user.username.as_str(), secret.as_str());
    let t = Duration::from_secs(8);
    let run = |f: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), network::Error>> + Send + '_>>| -> Result<(), String> {
        // The timer must be made inside the runtime.
        match rt.block_on(async { tokio::time::timeout(t, f).await }) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("no answer in time".into()),
        }
    };
    use setup::diagnose::Status;
    let pass_fail = |r: Result<(), String>| match r {
        Ok(()) => (Status::Ok, None),
        Err(e) => (Status::Fail, Some(e)),
    };
    let mut results: TestResults = Vec::new();
    let mut add = |name, (status, note): (Status, Option<String>)| results.push((name, status, note));
    add("Config server (port 80)", pass_fail(run(Box::pin(network::test_cfg_server(&profile.server)))));
    let api_ok = run(Box::pin(network::test_login(api.clone(), user, pass)));
    let account_ok = api_ok.is_ok();
    add("API and account", pass_fail(api_ok));
    let quazal = run(Box::pin(network::test_quazal_login(&profile.server, profile.login_port(), user, pass)));
    add("Game service sign-in", game_sign_in_status(quazal, account_ok));
    // Only a router that forwards UDP 13000 (or no router) lets the server's packet in
    // unasked. If it doesn't, ask the router for the mapping the game asks for while it
    // runs (UPnP, then NAT-PMP), try again, and take the mapping away. The outcome is
    // worded like the overlay's line in game: without a port open, the server relays
    // this PC's matches (Automatic), or other players may not get through (LAN or VPN only).
    let timed_out = |e: &str| e.contains("in time") || e.contains("Deadline");
    let no_port = |why: String| match nat_mode {
        hooks_config::NatMode::Off => (
            Status::Warn,
            Some(format!(
                "Direct, no router port open ({why}): others may not be able to join you. Internet play is set to LAN or VPN only"
            )),
        ),
        _ => (
            Status::Ok,
            Some(format!(
                "Through the server's relay: no router port open ({why}), so matches go through the server, which adds a little delay"
            )),
        ),
    };
    let direct = match run(Box::pin(network::test_p2p(api.clone(), user, pass))) {
        Ok(()) => (Status::Ok, Some("Direct: reachable without any router set-up".into())),
        Err(e) if timed_out(&e) => {
            let pinned = profile
                .adapter
                .as_deref()
                .and_then(|name| setup::net::adapter_ip(name, &setup::net::adapters()))
                .and_then(|ip| match ip {
                    std::net::IpAddr::V4(v4) => Some(v4),
                    std::net::IpAddr::V6(_) => None,
                });
            match portmap::map(nat_proto::STORM_PORT, pinned, 120, "5th Echelon connection test") {
                Err(why) => no_port(format!("the router didn't forward one: {why}")),
                Ok(mapping) => {
                    let (public, how) = (mapping.public, mapping.how);
                    let outcome = if public.port() != nat_proto::STORM_PORT {
                        no_port(format!("the router offered {public} ({how}), not port {}; another PC may have it", nat_proto::STORM_PORT))
                    } else {
                        match run(Box::pin(network::test_p2p(api, user, pass))) {
                            Ok(()) => (Status::Ok, Some(format!("Direct, router port opened ({public}, {how}); the game opens it while it runs"))),
                            // The game reports this mapping, so the server treats the PC as
                            // reachable and doesn't relay it.
                            Err(e) if timed_out(&e) => (
                                Status::Warn,
                                Some(format!(
                                    "Direct, router port opened ({public}, {how}), but the server's packet still didn't arrive: a firewall on this PC, or another router in front. Others may not be able to join you; set Internet play to Always through the server"
                                )),
                            ),
                            Err(e) if port_in_use(&e) => (Status::Warn, Some(PORT_HELD.into())),
                            Err(e) => (Status::Fail, Some(e)),
                        }
                    };
                    mapping.remove();
                    outcome
                }
            }
        }
        // The game's port held by another program (or the game): it can't be tested, and the
        // port's own line below says why (a raw "only one usage of each socket address" read as
        // a failure of its own: the Surface, 16 Nov 2026).
        Err(e) if port_in_use(&e) => (Status::Warn, Some(PORT_HELD.into())),
        Err(e) => (Status::Fail, Some(e)),
    };
    let direct = match (nat_mode, direct) {
        (hooks_config::NatMode::Relay, (Status::Ok | Status::Warn, _)) => {
            (Status::Ok, Some("Through the server's relay: Internet play is set to Always through the server".into()))
        }
        (_, outcome) => outcome,
    };
    add("How players reach this PC", direct);
    // The helper the game will use: a relay of the network's when the server offers them
    // (relays.rs), else the server's own.
    let relay = crate::relays::pick_for(&profile.server);
    let (nat_host, nat_port) = match &relay {
        Some(r) => (r.host.clone(), r.port),
        None => (profile.server.clone(), game_nat_port.unwrap_or(nat_proto::DEFAULT_PORT)),
    };
    let nat = match rt.block_on(async { tokio::time::timeout(t, network::test_nat_helper(&nat_host, nat_port)).await }) {
        Ok(Ok(check)) => match check.symmetric {
            Some(true) => (
                Status::Warn,
                Some(format!(
                    "reachable; this PC is {} to the server, and the router changes ports per destination, so matches go through the server's relay",
                    check.observed
                )),
            ),
            _ => (Status::Ok, None),
        },
        Ok(Err(e)) => (Status::Fail, Some(format!("{e} (UDP {nat_port}-{}): only LAN or VPN play works", nat_port + 1))),
        Err(_) => (Status::Fail, Some(format!("no answer (UDP {nat_port}-{}): only LAN or VPN play works", nat_port + 1))),
    };
    let nat = match (&relay, nat) {
        (Some(r), (status, why)) => (status, Some(format!("the relay {}{}", r.name, why.map(|w| format!(": {w}")).unwrap_or_default()))),
        (None, outcome) => outcome,
    };
    add("Internet play helper", nat);
    // 0.5.2's J3: what on this PC gets in the game's way. A VPN adapter the game may use (several
    // support replies were Radmin VPN or Hamachi), and Storm's port taken by another program.
    if !hooks_config::running_under_wine() {
        let vpns = vpn_adapters(&setup::net::adapters());
        add(
            "VPN adapters",
            if vpns.is_empty() {
                (Status::Ok, None)
            } else {
                (
                    Status::Warn,
                    Some(format!(
                        "{} on: the game may use it for matches, and players outside it can't reach you then. Turn it off while you play, or pick your network adapter in Settings › Network",
                        vpns.join(", ")
                    )),
                )
            },
        );
    }
    add("Game's match port (UDP 13000)", storm_port_free());
    if cfg!(target_os = "windows") && !hooks_config::running_under_wine() {
        add(FIREWALL_TEST, firewall_says(game_firewall_rules(game_dir).as_deref()));
    }
    results
}

/// The inbound Windows Firewall rules for the two executables of the game in `game_dir` (only
/// that copy's: another install's rule lets in another program, and made the line read Ok for a
/// copy with none, 2026-11-15), as PowerShell's firewall cmdlets give them (their values aren't
/// translated, unlike netsh's): JSON of `{p: program, e: "True"|"False", a: "Allow"|"Block"}`.
/// None when it couldn't be read.
#[cfg(target_os = "windows")]
fn game_firewall_rules(game_dir: &std::path::Path) -> Option<String> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = firewall_rules_script(game_dir)?;
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(target_os = "windows"))]
fn game_firewall_rules(_game_dir: &std::path::Path) -> Option<String> {
    None
}

/// The PowerShell that lists the inbound rules for the executables of the game in `game_dir`
/// (a rule's program may be written with variables, %ProgramFiles%: matched expanded, any
/// case). None when neither executable is there.
pub fn firewall_rules_script(game_dir: &std::path::Path) -> Option<String> {
    let exes: Vec<String> = setup::game::GameVersion::ALL
        .iter()
        .map(|v| v.full_path(game_dir))
        .filter(|p| p.is_file())
        .map(|p| p.display().to_string())
        .collect();
    if exes.is_empty() {
        return None;
    }
    // PowerShell single-quoted strings: a quote is written twice.
    let list = exes.iter().map(|e| format!("'{}'", e.replace('\'', "''"))).collect::<Vec<_>>().join(",");
    Some(format!(
        "$exes = @({list}); Get-NetFirewallApplicationFilter | Where-Object {{ $exes -contains [Environment]::ExpandEnvironmentVariables($_.Program) }} | \
         ForEach-Object {{ $f = $_; Get-NetFirewallRule -AssociatedNetFirewallApplicationFilter $f | Where-Object {{ \"$($_.Direction)\" -eq 'Inbound' }} | \
         ForEach-Object {{ [pscustomobject]@{{ p = $f.Program; e = \"$($_.Enabled)\"; a = \"$($_.Action)\" }} }} }} | ConvertTo-Json -Compress"
    ))
}

/// The connection test's Windows Firewall line for the game in `game_dir`, read again now
/// (after "Allow the game").
pub fn firewall_now(game_dir: &std::path::Path) -> (setup::diagnose::Status, Option<String>) {
    firewall_says(game_firewall_rules(game_dir).as_deref())
}

/// The connection test's name for the Windows Firewall line, which "Allow the game" fixes.
pub const FIREWALL_TEST: &str = "Windows Firewall";

/// "Allow the game" (0.5.2's J3, the user's choice of 14 Nov): with one UAC prompt, inbound
/// Windows Firewall rules allowing the game's executables in `game_dir` (both, where there),
/// named "5th Echelon Enhanced: <exe>", and the inbound rules blocking them turned off (not
/// deleted: Windows Security still lists them; a block beats an allow, so they must go). A rule's
/// program can be written with variables (%ProgramFiles%): matched expanded. Ok when it was done; the player saying No to the
/// prompt is an error.
#[cfg(target_os = "windows")]
pub fn allow_game_through_firewall(game_dir: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = allow_script(game_dir).ok_or("The game's programs weren't found in its folder.")?;
    // The script goes to the elevated PowerShell encoded (UTF-16LE, base64): no quoting to get
    // wrong between the two shells. -PassThru gives its exit code.
    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>(),
    );
    let outer = format!(
        "try {{ $p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','{encoded}'; exit $p.ExitCode }} catch {{ exit 1223 }}"
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &outer])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("PowerShell didn't start: {e}"))?;
    match status.code() {
        Some(0) => Ok(()),
        // ERROR_CANCELLED: the UAC prompt was answered No.
        Some(1223) => Err("Windows asked for permission and it wasn't given: nothing was changed.".into()),
        code => Err(format!("Windows Firewall didn't take the rules (code {}).", code.unwrap_or(-1))),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn allow_game_through_firewall(_game_dir: &std::path::Path) -> Result<(), String> {
    Err("Only on Windows.".into())
}

/// The elevated script for [`allow_game_through_firewall`]: for each of the game's executables
/// found, its blocking inbound rules turned off and our allow rule made again. None when neither
/// is there.
pub fn allow_script(game_dir: &std::path::Path) -> Option<String> {
    let exes: Vec<String> = setup::game::GameVersion::ALL
        .iter()
        .map(|v| v.full_path(game_dir))
        .filter(|p| p.is_file())
        .map(|p| p.display().to_string())
        .collect();
    if exes.is_empty() {
        return None;
    }
    // PowerShell single-quoted strings: a quote is written twice.
    let list = exes.iter().map(|e| format!("'{}'", e.replace('\'', "''"))).collect::<Vec<_>>().join(",");
    Some(format!(
        "$ErrorActionPreference = 'Stop'\n\
         foreach ($exe in @({list})) {{\n\
           $name = '5th Echelon Enhanced: ' + (Split-Path $exe -Leaf)\n\
           Get-NetFirewallApplicationFilter | Where-Object {{ [Environment]::ExpandEnvironmentVariables($_.Program) -eq $exe }} | ForEach-Object {{ Get-NetFirewallRule -AssociatedNetFirewallApplicationFilter $_ }} | \
             Where-Object {{ \"$($_.Direction)\" -eq 'Inbound' -and \"$($_.Action)\" -eq 'Block' -and \"$($_.Enabled)\" -eq 'True' }} | Disable-NetFirewallRule\n\
           Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue | Remove-NetFirewallRule\n\
           New-NetFirewallRule -DisplayName $name -Direction Inbound -Program $exe -Action Allow -Profile Any | Out-Null\n\
         }}\n"
    ))
}

/// What the game's inbound firewall rules say (0.5.2's J3): a rule blocking it (what Windows
/// writes when its "Allow access" prompt is dismissed), or no rule letting it in, keeps other
/// players out even with the router's port open.
pub fn firewall_says(rules: Option<&str>) -> (setup::diagnose::Status, Option<String>) {
    use setup::diagnose::Status;
    let Some(text) = rules else {
        return (Status::Warn, Some("couldn't read its rules; a firewall on this PC may still block the game".into()));
    };
    let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap_or(serde_json::Value::Null);
    let list: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(_) => vec![value],
        _ => Vec::new(),
    };
    let enabled: Vec<&serde_json::Value> = list.iter().filter(|r| r["e"] == "True").collect();
    let exe = |r: &serde_json::Value| r["p"].as_str().and_then(|p| p.rsplit(['\\', '/']).next()).unwrap_or("the game").to_string();
    if let Some(block) = enabled.iter().find(|r| r["a"] == "Block") {
        return (
            Status::Warn,
            Some(format!(
                "a rule blocks {} coming in (Windows writes one when its \"Allow access\" prompt is dismissed): other players can't reach you. Allow the game here, or in Windows Security › Firewall › Allow an app through firewall",
                exe(block)
            )),
        );
    }
    if !enabled.iter().any(|r| r["a"] == "Allow") {
        return (
            Status::Warn,
            Some("no rule lets the game in: if Windows asks when the game starts, choose Allow access, or Allow the game here".into()),
        );
    }
    (Status::Ok, None)
}

/// The adapters that look like a VPN's (by name, or by the address ranges the common ones give):
/// their names, with the VPN's. 0.5.2's J3.
pub fn vpn_adapters(adapters: &[(String, std::net::IpAddr)]) -> Vec<String> {
    const NAMES: [(&str, &str); 8] = [
        ("radmin", "Radmin VPN"),
        ("hamachi", "Hamachi"),
        ("zerotier", "ZeroTier"),
        ("tailscale", "Tailscale"),
        ("wireguard", "WireGuard"),
        ("nordlynx", "NordVPN"),
        ("openvpn", "OpenVPN"),
        ("proton", "Proton VPN"),
    ];
    adapters
        .iter()
        .filter_map(|(name, ip)| {
            let lower = name.to_lowercase();
            let by_name = NAMES.iter().find(|(key, _)| lower.contains(key)).map(|(_, vpn)| *vpn);
            let by_range = match ip {
                std::net::IpAddr::V4(v4) => match v4.octets() {
                    [26, ..] => Some("Radmin VPN"),
                    [25, ..] => Some("Hamachi"),
                    _ => None,
                },
                std::net::IpAddr::V6(_) => None,
            };
            by_name.or(by_range).map(|vpn| {
                if name.to_lowercase().contains(&vpn.to_lowercase()) {
                    name.clone()
                } else {
                    format!("{vpn} ({name})")
                }
            })
        })
        .collect()
}

/// Whether UDP 13000 (Storm's port, which the game binds) is free on this PC while the game
/// isn't running: another program holding it keeps the game's matches off it. 0.5.2's J3.
fn storm_port_free() -> (setup::diagnose::Status, Option<String>) {
    use setup::diagnose::Status;
    match std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, nat_proto::STORM_PORT)) {
        Ok(_) => (Status::Ok, None),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => (
            Status::Warn,
            Some("another program holds it (or the game is running): close it, or the game's matches can't use their port".into()),
        ),
        Err(e) => (Status::Warn, Some(format!("couldn't check: {e}"))),
    }
}

#[cfg(test)]
mod vpn_tests {
    use setup::diagnose::Status;

    use super::firewall_says;
    use super::game_sign_in_status;

    /// J3: the game's inbound firewall rules, as PowerShell lists them.
    #[test]
    fn the_firewall_rules_are_read() {
        let allow = r#"[{"p":"C:\\Games\\Blacklist_DX11_game.exe","e":"True","a":"Allow"}]"#;
        assert_eq!(firewall_says(Some(allow)).0, Status::Ok);
        let block = r#"[{"p":"C:\\Games\\Blacklist_DX11_game.exe","e":"True","a":"Allow"},{"p":"C:\\Games\\Blacklist_game.exe","e":"True","a":"Block"}]"#;
        let (status, why) = firewall_says(Some(block));
        assert_eq!(status, Status::Warn);
        assert!(why.unwrap().contains("blocks Blacklist_game.exe"));
        let single_off = r#"{"p":"C:\\Games\\Blacklist_DX11_game.exe","e":"False","a":"Allow"}"#;
        assert!(
            firewall_says(Some(single_off)).1.unwrap().starts_with("no rule lets the game in"),
            "a lone rule, switched off"
        );
        assert!(firewall_says(Some("")).1.unwrap().starts_with("no rule"), "none at all");
        assert_eq!(firewall_says(None).0, Status::Warn);
    }

    /// J3's "Allow the game": the elevated script names each executable there, quotes escaped.
    #[test]
    fn the_allow_script_covers_the_games_programs() {
        let dir = std::env::temp_dir().join(format!("fe-allow-test-{}-o'brien", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(super::allow_script(&dir).is_none(), "no game there");
        std::fs::write(dir.join("Blacklist_DX11_game.exe"), b"").unwrap();
        let script = super::allow_script(&dir).unwrap();
        assert!(script.contains("o''brien"), "a quote in the path is doubled: {script}");
        assert!(
            script.contains("Blacklist_DX11_game.exe'") && !script.contains("Blacklist_game.exe'"),
            "only what's there: {script}"
        );
        assert!(script.contains("New-NetFirewallRule") && script.contains("Disable-NetFirewallRule") && !script.contains("Remove-NetFirewallRule -Action"));
        std::fs::write(dir.join("Blacklist_game.exe"), b"").unwrap();
        assert!(super::allow_script(&dir).unwrap().contains("Blacklist_game.exe'"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The firewall line reads the rules of this folder's game only, not another install's.
    #[test]
    fn a_held_port_isnt_a_failure_of_the_reach_test() {
        assert!(super::port_in_use(
            "I/O error: Only one usage of each socket address (protocol/network address/port) is normally permitted. (os error 10048)"
        ));
        assert!(super::port_in_use("Address in use (os error 98)"));
        assert!(!super::port_in_use("no answer in time"));
    }

    #[test]
    fn a_refused_game_sign_in_after_the_accounts_is_the_server_answering() {
        let denied = || Err::<(), String>("RMC error: AccessDenied".into());
        assert_eq!(game_sign_in_status(denied(), true).0, Status::Ok, "the game hasn't checked in today");
        assert_eq!(game_sign_in_status(denied(), false).0, Status::Fail, "the account's sign-in failed too");
        assert_eq!(game_sign_in_status(Err("no answer in time".into()), true).0, Status::Fail);
        assert_eq!(game_sign_in_status(Ok(()), true), (Status::Ok, None));
    }

    #[test]
    fn the_firewall_line_reads_this_copys_rules_only() {
        let dir = std::env::temp_dir().join(format!("fe-rules-test-{}-o'neil", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(super::firewall_rules_script(&dir).is_none(), "no game there");
        std::fs::write(dir.join("Blacklist_DX11_game.exe"), b"").unwrap();
        let script = super::firewall_rules_script(&dir).unwrap();
        let exe = dir.join("Blacklist_DX11_game.exe").display().to_string().replace('\'', "''");
        assert!(script.contains(&format!("'{exe}'")), "this copy's program, quotes doubled: {script}");
        assert!(
            script.contains("-contains [Environment]::ExpandEnvironmentVariables($_.Program)"),
            "matched by path: {script}"
        );
        assert!(!script.contains("-like '*Blacklist*"), "not every install's: {script}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::vpn_adapters;

    /// J3: VPN adapters by name or by the ranges Radmin VPN and Hamachi give.
    #[test]
    fn vpn_adapters_are_named() {
        use std::net::IpAddr;
        let a = |name: &str, ip: &str| (name.to_string(), ip.parse::<IpAddr>().unwrap());
        let found = vpn_adapters(&[
            a("Ethernet", "192.168.1.20"),
            a("Radmin VPN", "26.12.0.9"),
            a("Ethernet 3", "25.1.2.3"),
            a("Tailscale", "100.101.102.103"),
            a("Wi-Fi", "10.0.0.5"),
        ]);
        assert_eq!(found, vec!["Radmin VPN", "Hamachi (Ethernet 3)", "Tailscale"]);
    }
}
