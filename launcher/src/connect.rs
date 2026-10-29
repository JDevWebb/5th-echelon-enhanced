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

/// Looks for a server on this PC's networks: the ones that answered.
pub fn find_on_network() -> Result<Vec<std::net::IpAddr>, String> {
    let adapters = setup::net::adapters();
    let refs: Vec<(&str, std::net::IpAddr)> = adapters.iter().map(|(n, ip)| (n.as_str(), *ip)).collect();
    crate::services::rt()
        .block_on(crate::network::try_locate_server(None, &refs))
        .map_err(|_| "No server answered on this network.".to_string())
}

/// Where a server is: its region ("Sydney, Australia"), else its name.
pub fn place(s: &setup::directory::Listing) -> String {
    if s.region.is_empty() {
        s.name.clone()
    } else {
        s.region.clone()
    }
}
