//! What the Play screen runs in the background: gathering the checklist's
//! facts, the automatic setup, and single fixes. Each works on its own copy
//! of the settings file; the UI reloads it afterwards.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

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

/// The longest server name the game takes: the length of the name it came with,
/// `onlineconfigservice.ubi.com`.
const MAX_SERVER_NAME: usize = 27;

/// Everything the checklist needs, gathered off the UI thread.
pub fn gather(game_dir: &Path, cfg: &Config, bundled: Option<&[u8]>) -> (Facts, Support) {
    let mut facts = Facts {
        game_dir: Some(game_dir.to_path_buf()),
        client: bundled.map(|dll| install::client_state(game_dir, dll)),
        ..Facts::default()
    };
    // The client is part of the launcher's release: another version (an older one, as servers
    // refuse those) is replaced at once. It stays as it is while the game has it open.
    if let (Some(install::ClientState::Different), Some(dll)) = (facts.client, bundled) {
        if install::install(game_dir, dll).is_ok() {
            facts.client = Some(install::ClientState::Installed);
        }
    }
    if let Some(profile) = cfg.current_profile().filter(|p| !p.server.is_empty()) {
        let ip = net::resolve(&profile.server);
        facts.server = Some((profile.server.clone(), ip));
        if let Some(ip) = ip {
            let t = Duration::from_secs(2);
            facts.api_port = Some(profile.api_port());
            facts.unencrypted = profile.unencrypted() && !net::is_local_server(&profile.server, ip);
            facts.server_ports = Some((net::port_open(ip, profile.api_port(), t), net::port_open(ip, setup::CONFIG_PORT, t)));
            facts.account = Some(if !profile.has_account() {
                AccountFact::None
            } else {
                let url = profile.api_server_url().to_string();
                let secret = profile.user.secret().unwrap_or_default();
                checked_account(&url, &profile.user.username, &secret, || {
                    let accounts = Accounts::new(url.clone());
                    match account::AccountService::login(&accounts, &profile.user.username, &secret) {
                        Ok(()) => AccountFact::Ok(profile.user.username.clone()),
                        Err(e @ (account::AccountError::WrongPassword | account::AccountError::NotFound)) => AccountFact::Refused(e.to_string()),
                        Err(account::AccountError::Outdated(why)) => AccountFact::Outdated(why),
                        Err(e) => AccountFact::Unknown(e.to_string()),
                    }
                })
            });
            let adapters = net::adapters();
            facts.route_adapter = net::adapter_for_server(ip, &adapters);
            facts.route_ip = net::local_ip_towards(ip);
            facts.pinned = cfg.hook_config.networking.adapter.clone();
            facts.pinned_ip = facts.pinned.as_deref().and_then(|p| net::adapter_ip(p, &adapters));
            facts.require_adapter = cfg.hook_config.networking.require_adapter;
        }
    }
    facts.save = save::save_path(&cfg.hook_config.save, game_dir).map(|p| save::check(&p));
    facts.log = setup::diagnose::read_log(game_dir);
    facts.wine = wine_facts(game_dir);
    let version = setup::game::pick_version(game_dir, cfg.default_game).unwrap_or(cfg.default_game);
    (facts, support(game_dir, version))
}

/// How long an account check holds. The checklist refreshes every 30 s, and
/// signing in each time cost the server a password hash per player per
/// refresh (and a refused one counts towards its lockout).
const ACCOUNT_CHECK_HOLDS: Duration = Duration::from_secs(10 * 60);

/// The last account check: the server, name and a hash of the password it
/// was made with, when, and what it found.
static LAST_ACCOUNT_CHECK: Mutex<Option<(String, Instant, AccountFact)>> = Mutex::new(None);

/// Forgets the last account check, so the next one signs in: after a setup,
/// which may have made or changed the account.
pub fn forget_account_check() {
    *LAST_ACCOUNT_CHECK.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// `check`'s answer for this server, name and password, reusing one from the
/// last [`ACCOUNT_CHECK_HOLDS`]. A changed server, name or password checks
/// again at once; so does a check that couldn't reach the server.
fn checked_account(url: &str, username: &str, secret: &str, check: impl FnOnce() -> AccountFact) -> AccountFact {
    use sha2::Digest as _;
    let key = format!("{url}\n{username}\n{:x}", sha2::Sha256::digest(secret.as_bytes()));
    let mut last = LAST_ACCOUNT_CHECK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((k, at, fact)) = last.as_ref() {
        if *k == key && at.elapsed() < ACCOUNT_CHECK_HOLDS {
            return fact.clone();
        }
    }
    let fact = check();
    *last = (!matches!(fact, AccountFact::Unknown(_))).then(|| (key, Instant::now(), fact.clone()));
    fact
}

/// The API token for the current server's account, kept with the server,
/// name and password hash it's for: the friends list below is fetched every
/// refresh, and signing in for each would cost the server a password hash.
static API_TOKEN: Mutex<Option<(String, String)>> = Mutex::new(None);

/// A friend who's online on this server.
#[derive(Debug, Clone, Default)]
pub struct OnlineFriend {
    pub name: String,
    /// What they're doing ("Spies vs Mercs · in a lobby"), if the server says.
    pub activity: Option<String>,
}

/// The current account's friends, as the home screen shows them.
#[derive(Debug, Clone, Default)]
pub struct Friends {
    /// Online here, online first by name.
    pub online: Vec<OnlineFriend>,
    pub offline: usize,
    /// Friend requests waiting for this player's answer.
    pub requests: usize,
    /// Friends playing on other servers sharing friends, with the server.
    pub elsewhere: Vec<server_api::friends::FriendElsewhere>,
}

/// The current account's friends: who's online here and what they're
/// playing, and who's on other servers sharing friends. Empty without an
/// account.
pub fn friends(cfg: &Config) -> Result<Friends, String> {
    use sha2::Digest as _;
    let Some(profile) = cfg.current_profile().filter(|p| !p.server.is_empty() && p.has_account()) else {
        return Ok(Friends::default());
    };
    let url = profile.api_server_url().to_string();
    let username = profile.user.username.clone();
    let secret = profile.user.secret().unwrap_or_default();
    let key = format!("{url}\n{username}\n{:x}", sha2::Sha256::digest(secret.as_bytes()));
    let kept = API_TOKEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .filter(|(k, _)| *k == key)
        .map(|(_, t)| t.clone());
    let rt = crate::services::rt();
    let fetch = |token: String| rt.block_on(crate::network::relationships(url.clone(), &token));
    let result = match kept {
        Some(token) => match fetch(token) {
            // Signed out (a new password elsewhere, or expired): sign in once more.
            Err(crate::network::Error::InvalidPassword) => None,
            other => Some(other),
        },
        None => None,
    };
    let result = match result {
        Some(result) => result,
        None => {
            let token = rt.block_on(crate::network::sign_in(url.clone(), &username, &secret)).map_err(|e| e.to_string())?;
            *API_TOKEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((key, token.clone()));
            fetch(token)
        }
    };
    // Shown on the Play screen: one line each, cut short, whatever the server sends.
    let clip = hooks_config::text::clip;
    let lists = result.map_err(|e| e.to_string())?;
    let mut online: Vec<OnlineFriend> = lists
        .friends
        .iter()
        .filter(|f| f.is_online)
        .take(100)
        .map(|f| OnlineFriend {
            name: clip(&f.username, 32),
            activity: f.activity.as_ref().map(describe),
        })
        .collect();
    online.sort_by_key(|f| f.name.to_lowercase());
    Ok(Friends {
        offline: lists.friends.iter().filter(|f| !f.is_online).count(),
        online,
        requests: lists.requests_received.len(),
        elsewhere: lists
            .elsewhere
            .into_iter()
            .take(100)
            .map(|f| server_api::friends::FriendElsewhere {
                username: clip(&f.username, 32),
                server: clip(&f.server, 64),
                region: clip(&f.region, 32),
                host: f.host.trim().to_string(),
            })
            .collect(),
    })
}

/// "Spies vs Mercs · in a match with Tank", as the overlay says it.
fn describe(a: &server_api::friends::Activity) -> String {
    let clip = hooks_config::text::clip;
    let mode = match a.mode.as_str() {
        "svm" => String::from("Spies vs Mercs"),
        "coop" => String::from("Co-op"),
        other => clip(other, 32),
    };
    let with = a.with.iter().take(4).map(|n| clip(n, 32)).collect::<Vec<_>>().join(", ");
    let room = match (a.room.as_str(), with.is_empty()) {
        ("match", true) => String::from("in a match"),
        ("match", false) => format!("in a match with {with}"),
        (_, true) => String::from("in a lobby"),
        (_, false) => format!("in a lobby with {with}"),
    };
    clip(&format!("{mode} · {room}"), 96)
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
    /// The server came from another server (a friend's, or the directory's),
    /// not from the player: it must be on the internet, never this PC or its
    /// network.
    pub public_only: bool,
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

/// What `server` says about itself (`/api/info`), and whether it said so over HTTPS.
///
/// HTTPS is asked first: on 443, then on the port the plain answer gives the HTTPS API.
/// The plain answer could come from anyone on the way, so it's only taken as it is from a
/// server that never had HTTPS here (`had_https`). From one that did, it's taken only
/// when it still offers its API over HTTPS (whose certificate is checked when used);
/// otherwise this fails rather than switch the player to plain HTTP.
fn server_info(server: &str, had_https: bool) -> Result<(Option<setup::server_info::ServerInfo>, bool), String> {
    let rt = crate::services::rt();
    if let Some(info) = rt.block_on(crate::network::server_info_tls(server, 443)) {
        return Ok((Some(info), true));
    }
    let plain = setup::server_info::fetch(server, Duration::from_secs(4));
    let api_tls = plain.as_ref().and_then(|i| i.ports).and_then(|p| p.api_tls);
    if let Some(info) = api_tls.filter(|p| *p != 443).and_then(|port| rt.block_on(crate::network::server_info_tls(server, port))) {
        return Ok((Some(info), true));
    }
    if had_https && api_tls.is_none() {
        return Err(format!(
            "{server} answered over HTTPS before but doesn't now, so your password isn't sent to it unencrypted. Try again later. If its operator turned HTTPS off, remove {server} under Settings › Servers and accounts, then connect again."
        ));
    }
    Ok((plain, false))
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
    // The game keeps the server's name where `onlineconfigservice.ubi.com` was, so it can't
    // start with a longer one (see `hooks::patch_url`).
    if plan.server.len() > MAX_SERVER_NAME {
        return Err(format!(
            "\"{}\" is {} characters, and the game takes at most {MAX_SERVER_NAME}. Use the server's IP address, or ask its operator for a shorter name.",
            plan.server,
            plan.server.len()
        ));
    }
    say(log, format!("Looking up {}…", plan.server));
    let ip = net::resolve(&plan.server).ok_or_else(|| format!("\"{}\" isn't an address this PC can find.", plan.server))?;
    if plan.public_only && !net::is_public(ip) {
        return Err(format!(
            "{} is at {ip}, which isn't an address on the internet. Servers can only send you to public servers; type it yourself if you meant it.",
            plan.server
        ));
    }

    let mut profile = Config::load(dir).profiles.iter().find(|p| p.server == plan.server).cloned().unwrap_or_else(|| Profile {
        name: plan.server.clone(),
        server: plan.server.clone(),
        ..Default::default()
    });
    // A server behind a proxy, or with remapped ports, says which it uses; over HTTPS when it can.
    let (info, over_tls) = server_info(&plan.server, profile.https)?;
    if let Some(ports) = info.as_ref().and_then(|i| i.ports) {
        profile.use_ports(&ports);
        if profile.api_server_url.is_some() || profile.login_port.is_some() || profile.nat_port.is_some() {
            say(log, format!("The server uses API port {}, game port {}.", ports.api, ports.login));
        }
    }
    // An HTTPS API that doesn't answer (the port closed on a firewall in front): the plain
    // one, but only on a server that never had HTTPS here, and never quietly (the checklist
    // keeps warning).
    if let Some(ports) = info.as_ref().and_then(|i| i.ports).filter(|p| p.api_tls.is_some()) {
        if !net::port_open(ip, profile.api_port(), Duration::from_secs(4)) {
            if profile.https {
                return Err(format!(
                    "{}'s HTTPS API doesn't answer, and it had one before, so your password isn't sent to it unencrypted. Try again later.",
                    plan.server
                ));
            }
            profile.use_ports(&setup::server_info::Ports { api_tls: None, ..ports });
            say(log, "The server's HTTPS port doesn't answer; using its unencrypted API, so your password travels readable.");
        }
    }
    if profile.https && profile.unencrypted() {
        // Said over HTTPS (see `server_info`), so it's the server's own word.
        say(log, "The server no longer offers its API over HTTPS.");
    }
    let api_port = profile.api_port();
    if !net::port_open(ip, api_port, Duration::from_secs(4)) {
        return Err(format!(
            "{ip} doesn't answer on port {api_port}. Check the server is running and you're connected to its network."
        ));
    }

    // A server that shares friends through a coordinator also has its server directory:
    // used from now on, unless the player already chose one. Only when the server said so
    // over HTTPS: a plain answer could name any directory.
    if let Some(coordinator) = info.as_ref().and_then(|i| i.coordinator.clone()).filter(|c| setup::directory::valid_coordinator(c)) {
        let shown = hooks_config::text::clip(&coordinator, 120);
        if !over_tls {
            say(
                log,
                format!("The server names the server directory {shown}, but not over HTTPS, so it isn't used. If you trust it, set it in Settings."),
            );
        } else if crate::app::Prefs::adopt_directory(&coordinator) {
            say(log, format!("Using the server directory at {shown}."));
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

    // Nothing is pinned: the game uses the adapter that reaches the server each time it
    // starts. A pin chosen in Settings stays.
    match (&profile.adapter, net::adapter_for_server(ip, &net::adapters())) {
        (Some(pinned), _) => say(log, format!("Playing over \"{pinned}\" (pinned in Settings).")),
        (None, Some(route)) => say(log, format!("Playing over \"{route}\".")),
        (None, None) => {}
    }

    let cfg = Config::load(dir);
    if let Some(path) = save::save_path(&cfg.hook_config.save, dir) {
        let prepared = save::prepare(&path, Some(dir)).map_err(|e| format!("Couldn't get your save ready: {e}"))?;
        if let save::Prepared::Imported { from, .. } = &prepared {
            say(log, format!("Found your Ubisoft Connect save ({}).", from.display()));
        }
        if let Some(done) = prepared.describe() {
            say(log, done);
        }
    }

    // HTTPS from now on, once the API answered there.
    profile.https = !profile.unencrypted();
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
    let password = profile.user.secret().ok_or("The saved password can't be read here; press Connect on the server again.")?;
    let new_name = new_name.trim().to_string();
    let (info, _) = server_info(&profile.server, profile.https)?;
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

/// Unpins the adapter: the game uses whichever reaches the server, each time it starts.
pub fn auto_adapter(game_dir: &Path) -> Result<String, String> {
    let mut cfg = Config::load(game_dir);
    cfg.update(|c| {
        if let Some(mut p) = c.current_profile().cloned() {
            p.adapter = None;
            c.upsert_profile(p.clone());
            c.apply_profile(&p);
        }
        c.hook_config.networking.adapter = None;
        c.hook_config.networking.require_adapter = false;
    })
    .map_err(|e| e.to_string())?;
    Ok("The game now uses the adapter that reaches the server.".into())
}

/// Creates or raises the save to rank 5.
pub fn fix_save(game_dir: &Path) -> Result<String, String> {
    let cfg = Config::load(game_dir);
    let path = save::save_path(&cfg.hook_config.save, game_dir).ok_or("The save folder can't be found.")?;
    save::prepare(&path, Some(game_dir))
        .map(|p| p.describe().unwrap_or("Your save is ready.").to_string())
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
