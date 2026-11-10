//! Connecting the game to a server, for both interfaces: what the activity bar says while it
//! runs, the setup itself (a network's address set up on its best server), and what's kept
//! once it worked.

use crate::activity::Handle;
use crate::flow;
use crate::prefs::Game;
use crate::prefs::Prefs;

/// A setup as it was asked, to run (and run again from the activity bar's Try again).
#[derive(Debug, Clone)]
pub struct Asked {
    pub plan: flow::Plan,
    /// What the activity says while it runs, when it worked, and when it didn't.
    pub title: String,
    pub done: String,
    pub failed: String,
}

/// How a setup that ran went: how it ended, the server it set up (a network's best, for a
/// network's address), and whether a network's directory chose it.
pub type Outcome = Result<(flow::Done, String, bool), String>;

/// The setup for `server` (as typed or picked) on `game`. `new_name`: the name for a new
/// account (the name step); `public_only`: the server came from another server, not the
/// player; `directory`: the network's servers, for their names and whether `server` is one.
pub fn ask(game: &Game, server: &str, new_name: Option<String>, public_only: bool, directory: Option<&[(setup::directory::Listing, Option<u32>)]>) -> Asked {
    let server = server.trim().to_string();
    let current = game.cfg.current_profile().map(|p| p.server.clone()).filter(|s| !s.is_empty());
    let listing = directory.and_then(|d| d.iter().find(|(s, _)| s.host.eq_ignore_ascii_case(&server)));
    let shown = listing.map_or_else(|| server.clone(), |(s, _)| place(s));
    let (title, done, failed) = match current.as_deref() {
        Some(c) if c == server => (format!("Setting up {shown} again"), format!("You're set up on {shown}"), format!("Couldn't set up {shown}")),
        Some(_) => (format!("Switching to {shown}"), format!("Switched to {shown}"), format!("Couldn't switch to {shown}")),
        None => (
            format!("Connecting to {shown}"),
            format!("You're set up on {shown}"),
            format!("Couldn't connect to {shown}"),
        ),
    };
    // The name step: what failed is the account (a name refused, say), not the connection.
    let failed = if new_name.is_some() {
        format!("Couldn't create your account on {shown}")
    } else {
        failed
    };
    Asked {
        plan: flow::Plan {
            game_dir: game.dir.clone(),
            server,
            new_name,
            public_only,
            // A community server: an account is made there with the player's usual name, without asking.
            automatic: listing.is_some(),
        },
        title,
        done,
        failed,
    }
}

/// Runs the setup (on a task's thread), reporting to `log`.
pub fn run(asked: &Asked, log: &Handle) -> Outcome {
    let mut plan = asked.plan.clone();
    // A network's address: set up on its best server, which its directory named.
    let via_network = match flow::pick_from_network(&plan.server, log)? {
        Some(host) => {
            plan.server = host;
            plan.public_only = true;
            plan.automatic = true;
            true
        }
        None => false,
    };
    // A server of the network that moved its players (a relay now: global matchmaking): set up
    // where they play, rather than offered an account on a server nobody plays on.
    if !via_network {
        if let Some(to) = flow::moved_server(&plan.server) {
            flow::say(log, format!("{} moved its players to {to}: setting you up there.", plan.server));
            tracing::info!("{} moved its players to {to}: set up there instead", plan.server);
            plan.server = to;
            plan.public_only = true;
        }
    }
    flow::run_setup(&plan, crate::dll_utils::bundled(), log).map(|done| (done, plan.server.clone(), via_network))
}

/// After a setup that worked: the account is looked at afresh, and a server of the player's
/// own (not the network's, nor one a server named) is kept with the recent ones.
pub fn ready(asked: &Asked, server: &str, via_network: bool, directory: Option<&[(setup::directory::Listing, Option<u32>)]>) {
    flow::forget_account_check();
    let listed = directory.is_some_and(|d| d.iter().any(|(s, _)| s.host.eq_ignore_ascii_case(server)));
    if !via_network && !listed && !asked.plan.public_only {
        Prefs::remember_server(server, None);
    }
}

/// A recent server's line (Servers › Recent): "in use now" for the one the player is on (its
/// time is from when they last connected there from Recent, hours ago maybe), else when.
pub fn recent_when(address: &str, last_used: i64, now: i64, current: Option<&str>) -> String {
    if current.is_some_and(|c| c.trim().eq_ignore_ascii_case(address.trim())) {
        return "in use now".to_string();
    }
    format!("used {}", setup::directory::ago(now - last_used).replace("within the last hour", "just now"))
}

/// Looks for a server on this PC's networks: the ones that answered.
pub fn find_on_network() -> Result<Vec<std::net::IpAddr>, String> {
    let adapters = setup::net::adapters();
    let refs: Vec<(&str, std::net::IpAddr)> = adapters.iter().map(|(n, ip)| (n.as_str(), *ip)).collect();
    crate::services::rt()
        .block_on(crate::network::try_locate_server(None, &refs))
        .map_err(|_| "No server answered on this network.".to_string())
}

/// Who runs the server, for the server card's caption, when it isn't the community
/// network: "Self-hosted" for a server on no network, or the other network's name, with
/// a line saying so (`listed`: in the directory in use; `loaded`: the directory is known,
/// so a server missing from it really isn't there, and the label doesn't flash).
pub fn run_by(listed: bool, loaded: bool) -> Option<(String, String)> {
    let network = Prefs::directory();
    // With no network there's no list to wait for.
    let loaded = loaded || network.is_none();
    if network.as_deref().is_some_and(Prefs::is_community) && listed {
        return None;
    }
    match network {
        Some(url) if listed => {
            let host = url_host(&url).to_string();
            let why = format!("On {host}'s network, run by its admins, not by the community network.");
            Some((hooks_config::text::clip(&host, 22), why))
        }
        _ if !listed && loaded => Some((
            "Self-hosted".to_string(),
            "Run by whoever hosts it, not by the community network. Fine for your own or a friend's server.".to_string(),
        )),
        _ => None,
    }
}

/// A network as players are shown it: the community network by its address
/// ([`setup::directory::COMMUNITY_ADDRESS`]), another by its coordinator's host.
pub fn network_shown(url: &str) -> &str {
    if Prefs::is_community(url) {
        setup::directory::COMMUNITY_ADDRESS
    } else {
        url_host(url)
    }
}

/// A network's host, from its address.
pub fn url_host(url: &str) -> &str {
    url.trim_start_matches("https://").trim_end_matches('/')
}

/// The server menu's header: the network its servers are from.
pub fn menu_header(network: Option<&str>) -> String {
    match network {
        Some(url) if Prefs::is_community(url) => format!("Community network · {}", network_shown(url)),
        Some(url) => format!("Servers · {}", url_host(url)),
        None => "No network".to_string(),
    }
}

/// Where a server is: its region ("Sydney, Australia"), else its name.
pub fn place(s: &setup::directory::Listing) -> String {
    if s.region.is_empty() {
        s.name.clone()
    } else {
        s.region.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::recent_when;

    #[test]
    fn the_server_in_use_says_so_in_recent() {
        let now = 1_800_000_000;
        assert_eq!(recent_when("match.example.net", now - 6 * 3600, now, Some("MATCH.example.net")), "in use now");
        assert!(recent_when("match.example.net", now - 6 * 3600, now, Some("other.example.net")).starts_with("used "));
        assert!(recent_when("match.example.net", now - 60, now, None).ends_with("just now"));
    }
}
