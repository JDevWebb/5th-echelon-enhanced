//! Starting the game from the Play button, for both interfaces: what has to be answered or set
//! first, the settings the game reads as it starts, then the start itself.

use setup::launch::Launched;

use crate::activity::Activities as Notices;
use crate::prefs::Game;
use crate::prefs::PlayMode;
use crate::prefs::Prefs;

/// Why the game wasn't started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Something to do first (a question to answer): a notice, not an error.
    First(String),
    Error(String),
}

/// Starts the game, `mode` saying whether straight into an online lobby. `name_not_allowed`:
/// the server said the player's name breaks its rules (it would keep the game out).
pub fn start(game: &mut Game, notices: &mut Notices, mode: PlayMode, name_not_allowed: bool) -> Result<Launched, Refused> {
    // The question about diagnostics is answered first (diagnostics.rs).
    if crate::diagnostics::Ask::pending(Some(game)) {
        return Err(Refused::First("First, say whether the game may send its diagnostics.".into()));
    }
    // And the new game options (new_options.rs).
    if !crate::new_options::pending(Some(game)).is_empty() {
        return Err(Refused::First("First, choose the new game options.".into()));
    }
    // The game's files are hashed for its sign-in first (game_files.rs): a few seconds the
    // first time, the Play button waiting meanwhile.
    if crate::game_files::preparing(&game.dir) {
        return Err(Refused::First(format!("{}…", crate::game_files::PREPARING)));
    }
    if setup::game::game_running() {
        return Err(Refused::Error("Splinter Cell: Blacklist is already running.".into()));
    }
    if name_not_allowed {
        return Err(Refused::Error(
            "Your name breaks this network's rules. Choose another first: Settings › Servers and accounts › Rename.".into(),
        ));
    }
    // The hook reads uplay.toml when the game starts: make sure it has the current profile.
    if game.managed.is_none() {
        if let Some(profile) = game.cfg.current_profile().cloned() {
            // The relay nearest the player, when the server is a network's matchmaking server
            // offering its relays (relays.rs); else its own helper.
            let relay = crate::relays::pick_for(&profile.server);
            // UDP blocked (or the player chose it): through the server's tunnel over HTTPS, to
            // the server's own helper (0.5.2's T). Checked at every start.
            // A relay that answered the launcher's UDP pings: UDP works, nothing to check.
            // The game's calls to the server kept losing their answers on this network (or the
            // player chose it): sign-in and the API through the nearest relay, which passes them
            // on (0.5.5's SR). Only with a relay that says it can.
            let via_relay = relay
                .as_ref()
                .and_then(|r| crate::sign_in_route::wanted(&game.dir, &profile.server).map(|why| (r, why)))
                .filter(|(r, _)| crate::sign_in_route::relay_can(&r.host));
            let tunnel = if let Some((r, why)) = via_relay {
                tracing::info!("Signing in to {} through the relay {} ({why})", profile.server, r.host);
                notices.info_long(why);
                Some(crate::sign_in_route::tunnel(
                    &profile.server,
                    profile.login_port(),
                    profile.login_port().wrapping_add(1),
                    &r.host,
                    r.port,
                ))
            } else if relay.is_some() && !Prefs::tunnel_always() {
                None
            } else {
                crate::tunnel::for_start(&profile.server)
            };
            let via = tunnel.as_ref().and_then(|t| t.via.clone());
            // The launcher's own calls go the same way (its sign-in check, friends, chat).
            crate::network::set_api_via(&profile.server, via.as_deref());
            if tunnel.is_some() && via.is_none() {
                notices.info_long(if Prefs::tunnel_always() { crate::tunnel::NOTICE_CHOSEN } else { crate::tunnel::NOTICE });
            }
            // On Automatic, a network known not to be reachable directly (J4, reach.rs): the
            // game asks for the relay at once. Never over the player's own choice.
            let start_relayed = (tunnel.is_none() && game.cfg.hook_config.networking.nat == hooks_config::NatMode::Auto)
                .then(|| crate::reach::start_relayed(&game.dir))
                .flatten();
            if let Some(why) = &start_relayed {
                tracing::info!("This network needs the relay ({why}): the game asks for it at once");
            }
            game.update(notices, |c| {
                c.apply_profile(&profile);
                c.hook_config.networking.tunnel = tunnel.clone();
                c.hook_config.networking.start_relayed = start_relayed.clone();
                c.hook_config.networking.api_via = via.clone();
                if let (Some(_), Some(relay)) = (&via, &relay) {
                    // Through the relay: its helper, as usual, and the tunnel there.
                    c.hook_config.networking.nat_host = Some(relay.host.clone());
                    c.hook_config.networking.nat_port = Some(relay.port).filter(|p| *p != nat_proto::DEFAULT_PORT);
                    c.hook_config.networking.relays = relay.ranges.clone();
                    c.hook_config.networking.relay_pings = relay.pings.clone();
                } else if let Some(t) = &tunnel {
                    c.hook_config.networking.nat_host = None;
                    c.hook_config.networking.nat_port = Some(t.helper).filter(|p| *p != nat_proto::DEFAULT_PORT);
                } else if let Some(relay) = &relay {
                    c.hook_config.networking.nat_host = Some(relay.host.clone());
                    c.hook_config.networking.nat_port = Some(relay.port).filter(|p| *p != nat_proto::DEFAULT_PORT);
                    c.hook_config.networking.relays = relay.ranges.clone();
                    c.hook_config.networking.relay_pings = relay.pings.clone();
                }
            });
        }
        // Straight into a lobby, or a plain start (which clears a lobby the launcher set before,
        // never one set by hand). The game reads it on every start, through Steam too.
        let set_before = Prefs::quick_start_set();
        let mut set = set_before;
        let mut by_hand = Prefs::quick_start_by_hand();
        game.update(notices, |c| set = mode.apply(c, set_before, &mut by_hand));
        Prefs::set_quick_start_set(set);
        Prefs::set_quick_start_by_hand(by_hand);
    }
    let Some(version) = setup::game::pick_version(&game.dir, game.cfg.default_game) else {
        return Err(Refused::Error("The game's executable is missing.".into()));
    };
    // Linux, "Brighten the picture": vkBasalt's switches for the game's process.
    let env = crate::new_options::brighten_env(game, notices);
    let launched = setup::launch::launch(&game.dir, version, Prefs::start_through_steam(), &env).map_err(|e| {
        tracing::warn!("couldn't start the game: {e}");
        Refused::Error(format!("Couldn't start the game: {e}"))
    })?;
    // Steam starts the game itself, without the launcher's environment.
    if let (Launched::Steam, [(enable, on), (config, path)]) = (&launched, env.as_slice()) {
        notices.info(format!(
            "To brighten the picture when Steam starts the game, put this in its Launch Options in Steam: {enable}={} {config}=\"{}\" %command%",
            on.to_string_lossy(),
            path.to_string_lossy()
        ));
    }
    Ok(launched)
}
