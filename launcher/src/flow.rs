//! What the Play screen runs in the background: gathering the checklist's
//! facts, the automatic setup, and single fixes. Each works on its own copy
//! of the settings file; the UI reloads it afterwards.

use std::net::IpAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use setup::account;
use setup::config::Config;
use setup::config::Profile;
use setup::diagnose::AccountFact;
use setup::diagnose::Facts;
use setup::game::GameVersion;
use setup::install;
use setup::net;
use setup::save;

use crate::services::Accounts;

/// Whether the hook knows the game executable (it needs the addresses of
/// the functions it patches, per game build).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    Supported,
    Unsupported(String),
    Unknown(String),
}

pub fn support(game_dir: &Path, version: GameVersion) -> Support {
    match hooks_addresses::get_from_path(&version.full_path(game_dir)) {
        Ok(_) => Support::Supported,
        Err(e @ hooks_addresses::Error::BinaryMismatch(..)) => Support::Unsupported(e.to_string()),
        Err(e) => Support::Unknown(e.to_string()),
    }
}

/// Looks for the addresses in an unsupported executable and saves them next
/// to the game (`5th-echelon-addresses.json`), keeping any found before.
pub fn identify(game_dir: &Path, version: GameVersion) -> anyhow::Result<()> {
    let exe = version.full_path(game_dir);
    let addresses = hooks_addresses::search_patterns(&exe)?;
    let hash = hooks_addresses::hash_file(&exe)?;
    let mut custom = hooks_addresses::load_custom_addresses(game_dir);
    let mut dx9 = custom.remove("blacklist_game.exe").unwrap_or_default();
    let mut dx11 = custom.remove("blacklist_dx11_game.exe").unwrap_or_default();
    match version {
        GameVersion::SplinterCellBlacklistDx9 => dx9.insert(hash, addresses),
        GameVersion::SplinterCellBlacklistDx11 => dx11.insert(hash, addresses),
    };
    hooks_addresses::save_addresses(game_dir, dx9, dx11);
    Ok(())
}

/// Everything the checklist needs, gathered off the UI thread.
pub fn gather(game_dir: &Path, cfg: &Config, bundled: Option<&[u8]>) -> (Facts, Support) {
    let mut facts = Facts {
        game_dir: Some(game_dir.to_path_buf()),
        client: bundled.map(|dll| install::client_state(game_dir, dll)),
        ..Facts::default()
    };
    if let Some(profile) = cfg.current_profile().filter(|p| !p.server.is_empty()) {
        let ip = net::resolve(&profile.server);
        facts.server = Some((profile.server.clone(), ip));
        if let Some(ip) = ip {
            let t = Duration::from_secs(2);
            facts.api_port = Some(profile.api_port());
            facts.server_ports = Some((net::port_open(ip, profile.api_port(), t), net::port_open(ip, setup::CONFIG_PORT, t)));
            facts.account = Some(if !profile.has_account() {
                AccountFact::None
            } else {
                let accounts = Accounts::new(profile.api_server_url().to_string());
                match account::AccountService::login(&accounts, &profile.user.username, &profile.user.secret().unwrap_or_default()) {
                    Ok(()) => AccountFact::Ok(profile.user.username.clone()),
                    Err(e @ (account::AccountError::WrongPassword | account::AccountError::NotFound)) => AccountFact::Refused(e.to_string()),
                    Err(e) => AccountFact::Unknown(e.to_string()),
                }
            });
            let adapters = net::adapters();
            facts.route_adapter = net::adapter_for_server(ip, &adapters);
            facts.pinned = cfg.hook_config.networking.adapter.clone();
            facts.pinned_ip = facts.pinned.as_deref().and_then(|p| net::adapter_ip(p, &adapters));
        }
    }
    facts.save = save::save_path(&cfg.hook_config.save, game_dir).map(|p| save::check(&p));
    facts.log = setup::diagnose::read_log(game_dir);
    facts.wine = wine_facts(game_dir);
    let version = setup::game::pick_version(game_dir, cfg.default_game).unwrap_or(cfg.default_game);
    (facts, support(game_dir, version))
}

/// What the player asked the setup for.
#[derive(Debug, Clone)]
pub struct Plan {
    pub game_dir: PathBuf,
    /// The server, as typed.
    pub server: String,
    /// The name for a new account, used only when the player's identity has
    /// none on this server. None: the setup stops and asks (see
    /// [`Done::NeedsName`]).
    pub new_name: Option<String>,
}

/// How a setup ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    Ready,
    /// The player has no account on `server` (the one picked, for a
    /// network): they choose a name, and the setup runs again with it.
    NeedsName {
        server: String,
        suggested: String,
    },
}

/// Progress lines for the UI.
pub type Log = Arc<Mutex<Vec<String>>>;

fn say(log: &Log, line: impl Into<String>) {
    log.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(line.into());
}

/// The automatic setup: install the client, check the server, set up the
/// account, pin the adapter, make a save, and save it all as the player's
/// profile. Stops at the first step that fails, saying why.
/// When `address` is a network (a coordinator's https address, e.g. the
/// community's play.scbl.jdevwebb.net) rather than a game server: uses its
/// directory from now on, pings its servers, and returns the best one's host.
/// None for anything else (a server, an IP address, a LAN host).
pub fn pick_from_network(address: &str, log: &Log) -> Result<Option<String>, String> {
    let address = address.trim();
    if address.parse::<std::net::IpAddr>().is_ok() || !net::valid_host(address) || !address.contains('.') {
        return Ok(None);
    }
    let url = format!("https://{address}");
    let rt = crate::services::rt();
    if !rt.block_on(crate::network::is_coordinator(&url)) {
        return Ok(None);
    }
    say(log, format!("{address} is a network of servers: finding the best one for you…"));
    crate::app::Prefs::set_directory(Some(url.clone()));
    let servers = rt.block_on(crate::network::server_directory(&url))?;
    let best = setup::directory::best(&servers).ok_or_else(|| format!("{address} lists no servers right now; try again in a minute."))?;
    let (listing, ping) = &servers[best];
    let region = if listing.region.is_empty() { String::new() } else { format!(" ({})", listing.region) };
    let ping = ping.map_or_else(|| "no ping".to_string(), |ms| format!("{ms} ms"));
    say(log, format!("Best for you: {}{region}, {ping}, at {}.", listing.name, listing.host));
    Ok(Some(listing.host.clone()))
}

pub fn run_setup(plan: &Plan, bundled: Option<&[u8]>, log: &Log) -> Result<Done, String> {
    let dir = &plan.game_dir;

    if let Some(dll) = bundled {
        if install::client_state(dir, dll) != install::ClientState::Installed {
            say(log, "Installing the 5th Echelon client…");
            if setup::game::game_running() {
                return Err("Close Splinter Cell: Blacklist first; the client can't be installed while it runs.".into());
            }
            install::install(dir, dll).map_err(|e| e.to_string())?;
        }
    }

    if !net::valid_host(&plan.server) {
        return Err(format!("\"{}\" isn't a server address. Type a host name or IP address, without a port.", plan.server));
    }
    say(log, format!("Looking up {}…", plan.server));
    let ip = net::resolve(&plan.server).ok_or_else(|| format!("\"{}\" isn't an address this PC can find.", plan.server))?;

    let mut profile = Config::load(dir).profiles.iter().find(|p| p.server == plan.server).cloned().unwrap_or_else(|| Profile {
        name: plan.server.clone(),
        server: plan.server.clone(),
        ..Default::default()
    });
    // A server behind a proxy, or with remapped ports, says which it uses.
    let info = setup::server_info::fetch(&plan.server, Duration::from_secs(4));
    if let Some(ports) = info.as_ref().and_then(|i| i.ports) {
        profile.use_ports(&ports);
        if profile.api_server_url.is_some() || profile.login_port.is_some() || profile.nat_port.is_some() {
            say(log, format!("The server uses API port {}, game port {}.", ports.api, ports.login));
        }
    }
    // HTTPS that doesn't answer (the port closed on a firewall in front): the plain API.
    if let Some(ports) = info.as_ref().and_then(|i| i.ports).filter(|p| p.api_tls.is_some()) {
        if !net::port_open(ip, profile.api_port(), Duration::from_secs(4)) {
            profile.use_ports(&setup::server_info::Ports { api_tls: None, ..ports });
            say(log, "The server's HTTPS port doesn't answer; using its unencrypted API.");
        }
    }
    let api_port = profile.api_port();
    if !net::port_open(ip, api_port, Duration::from_secs(4)) {
        return Err(format!(
            "{ip} doesn't answer on port {api_port}. Check the server is running and you're connected to its network."
        ));
    }

    // A server that shares friends through a coordinator also has its server directory:
    // used from now on, unless the player already chose one.
    if let Some(coordinator) = info.as_ref().and_then(|i| i.coordinator.clone()) {
        if crate::app::Prefs::adopt_directory(&coordinator) {
            say(log, format!("Using the server directory at {coordinator}."));
        }
    }

    say(log, "Looking for your account…");
    // Every account is linked to the player's identity: it finds their account here, from any
    // PC, and carries friends between servers.
    // Signatures name the server as the player typed it, never what the server says it is:
    // a server claiming to be another can't reuse them there.
    if !info.as_ref().is_some_and(|i| i.features.iter().any(|f| f == "identity-login")) {
        return Err(format!(
            "{} can't find accounts by player identity, which this launcher needs. Ask its operator to update the server.",
            plan.server
        ));
    }
    let host = identity::host_key(&plan.server);
    let identity = Arc::new(setup::player_identity::load_or_create().map_err(|e| format!("Your identity couldn't be loaded: {e}."))?);
    let accounts = Accounts {
        api: profile.api_server_url().to_string(),
        identity: Some((Arc::clone(&identity), host.clone())),
    };
    // Only this server's own password is ever sent to it.
    let saved_secret = profile.user.secret();
    let saved = saved_secret.as_deref().filter(|_| profile.has_account()).map(|p| (profile.user.username.as_str(), p));
    let found = account::find_account(&accounts, saved).map_err(|e| format!("Couldn't look for your account: {e}."))?;
    let (username, password, how) = match (found, &plan.new_name) {
        (Some(found), _) => found,
        (None, Some(name)) => {
            say(log, format!("Creating your account {}…", account::account_name(name)));
            let (u, p) = account::create_account(&accounts, name).map_err(|e| match e {
                account::AccountError::Taken => format!("Someone already has the name {}. Choose another.", account::account_name(name)),
                e => format!("Couldn't create your account: {e}."),
            })?;
            (u, p, account::Outcome::Created)
        }
        (None, None) => {
            // The name the player goes by elsewhere, or on this PC.
            let suggested = Config::load(dir)
                .profiles
                .iter()
                .map(|p| p.user.username.clone())
                .find(|u| !u.is_empty())
                .unwrap_or_else(windows_user);
            say(log, format!("You don't have an account on {} yet.", plan.server));
            return Ok(Done::NeedsName {
                server: plan.server.clone(),
                suggested,
            });
        }
    };
    say(
        log,
        match how {
            account::Outcome::Existing => format!("Signed in as {username}."),
            account::Outcome::Recovered => format!("Found your account {username} with your identity."),
            account::Outcome::Created => format!("Created your account {username}."),
        },
    );
    // An account from before identities were required: linked now (a no-op when it is).
    if how == account::Outcome::Existing {
        let linked = crate::services::rt().block_on(async {
            tokio::time::timeout(
                Duration::from_secs(8),
                crate::network::link_identity(profile.api_server_url().to_string(), &identity, &host, &username, &password),
            )
            .await
        });
        match linked {
            Ok(Ok(())) => {}
            Ok(Err(crate::network::Error::Rpc(status))) if status.code() == tonic::Code::AlreadyExists => {
                return Err(format!(
                    "{username} is linked to another identity. Import that identity in Settings to use this account here."
                ))
            }
            Ok(Err(e)) => return Err(format!("Couldn't link {username} to your identity: {e}.")),
            Err(_) => return Err("Couldn't link your account to your identity: the server didn't answer in time.".into()),
        }
    }
    // The account id the server gave the account (the name, unless a renamed account still
    // had it); the game uses it until it has signed in.
    let account_id = crate::services::rt()
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(6),
                crate::network::account_id(profile.api_server_url().to_string(), &username, &password),
            )
            .await
        })
        .ok()
        .and_then(Result::ok)
        .unwrap_or_else(|| username.clone());
    let mut user = hooks_config::User {
        username: username.clone(),
        password: String::new(),
        protected_password: String::new(),
        cd_keys: profile.user.cd_keys.clone(),
        account_id,
    };
    user.set_secret(&password);
    profile.user = user;

    let adapters = net::adapters();
    profile.adapter = net::adapter_for_server(ip, &adapters);
    match &profile.adapter {
        Some(a) => say(log, format!("Playing over \"{a}\".")),
        None => say(log, "No network adapter to pin."),
    }

    let cfg = Config::load(dir);
    if let Some(path) = save::save_path(&cfg.hook_config.save, dir) {
        match save::check(&path) {
            save::SaveState::Missing => {
                save::create_rank5(&path).map_err(|e| format!("Couldn't create a save: {e}"))?;
                say(log, "Created a rank 5 save.");
            }
            s if s.below_rank5() => {
                save::raise_to_rank5(&path).map_err(|e| format!("Couldn't raise the save to rank 5: {e}"))?;
                say(log, "Raised your save to rank 5 (the old one is backed up).");
            }
            _ => {}
        }
    }

    let mut cfg = cfg;
    cfg.update(|c| {
        c.upsert_profile(profile.clone());
        c.apply_profile(&profile);
    })
    .map_err(|e| format!("Couldn't save the settings: {e}"))?;
    say(
        log,
        format!(
            "Ready. You're {} on {}, linked to your identity ({}).",
            profile.user.username,
            plan.server,
            identity::short(&identity.global_id())
        ),
    );
    Ok(Done::Ready)
}

/// Renames the account on the profile named `profile_name`: signed with the
/// player's identity when the server has one, reserved across servers
/// sharing friends. Updates the profile (and the game's settings, if it's
/// the current one).
pub fn rename(game_dir: &Path, profile_name: &str, new_name: &str) -> Result<String, String> {
    let mut cfg = Config::load(game_dir);
    let profile = cfg.profile(profile_name).cloned().ok_or("That server isn't set up any more.")?;
    let password = profile.user.secret().ok_or("The saved password can't be read here; set up the server again.")?;
    let new_name = new_name.trim().to_string();
    let info = setup::server_info::fetch(&profile.server, Duration::from_secs(4));
    if !info.is_some_and(|i| i.features.iter().any(|f| f == "rename")) {
        return Err("This server can't rename accounts.".into());
    }
    let host = identity::host_key(&profile.server);
    let identity = setup::player_identity::load().ok().flatten();
    let renamed = crate::services::rt()
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(10),
                crate::network::rename(
                    profile.api_server_url().to_string(),
                    &profile.user.username,
                    &password,
                    &new_name,
                    identity.as_ref().map(|i| (i, host.as_str())),
                ),
            )
            .await
        })
        .map_err(|_| "The server didn't answer in time.".to_string())?
        .map_err(|e| match e {
            crate::network::Error::Rpc(status) => status.message().to_string(),
            e => e.to_string(),
        })?;
    let current = cfg.current_profile().is_some_and(|p| p.name == profile.name);
    cfg.update(|c| {
        let mut p = profile.clone();
        // The account id stays: only the name changes.
        p.user.username = renamed.clone();
        c.upsert_profile(p.clone());
        if current {
            c.apply_profile(&p);
        }
    })
    .map_err(|e| format!("Renamed to {renamed}, but the settings couldn't be saved: {e}"))?;
    Ok(format!("You're {renamed} now. Restart the game if it's running."))
}

/// Pins the adapter the current server is reached through.
pub fn pin_adapter(game_dir: &Path) -> Result<String, String> {
    let mut cfg = Config::load(game_dir);
    let profile = cfg.current_profile().cloned().ok_or("Choose a server first.")?;
    let ip: IpAddr = net::resolve(&profile.server).ok_or("The server's address can't be found.")?;
    let adapter = net::adapter_for_server(ip, &net::adapters()).ok_or("No adapter on this PC reaches the server.")?;
    cfg.update(|c| {
        let p = Profile {
            adapter: Some(adapter.clone()),
            ..profile
        };
        c.upsert_profile(p.clone());
        c.apply_profile(&p);
    })
    .map_err(|e| e.to_string())?;
    Ok(format!("Pinned \"{adapter}\"."))
}

/// Creates or raises the save to rank 5.
pub fn fix_save(game_dir: &Path) -> Result<String, String> {
    let cfg = Config::load(game_dir);
    let path = save::save_path(&cfg.hook_config.save, game_dir).ok_or("The save folder can't be found.")?;
    match save::check(&path) {
        save::SaveState::Missing => save::create_rank5(&path).map(|_| "Created a rank 5 save.".to_string()),
        _ => save::raise_to_rank5(&path).map(|_| "Raised the save to rank 5; the old one is backed up.".to_string()),
    }
    .map_err(|e| e.to_string())
}

/// Installs (or updates) the client.
pub fn install_client(game_dir: &Path, bundled: Option<&[u8]>) -> Result<String, String> {
    let dll = bundled.ok_or("This build doesn't carry the client.")?;
    install::install(game_dir, dll).map_err(|e| e.to_string())?;
    Ok("5th Echelon is installed.".into())
}

/// The player's user name, as a default account name.
pub fn windows_user() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default()
}

/// The Wine or Proton side of the checklist, when there is one.
pub fn wine_facts(game_dir: &Path) -> Option<setup::diagnose::WineFacts> {
    if cfg!(target_os = "windows") {
        // A Windows launcher under Wine starts the game itself, in its own
        // prefix, and sets the CPU topology when needed.
        return hooks_config::running_under_wine().then_some(setup::diagnose::WineFacts {
            steam: false,
            prefix_ready: true,
            launch_options: None,
        });
    }
    setup::wine::prefix_for(game_dir).map(|p| setup::diagnose::WineFacts {
        steam: p.steam,
        prefix_ready: p.ready(),
        launch_options: if p.steam { setup::wine::steam_launch_options() } else { None },
    })
}

/// Opens a folder in the file manager.
pub fn open_folder(dir: &Path) {
    let program = if cfg!(target_os = "windows") { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}
