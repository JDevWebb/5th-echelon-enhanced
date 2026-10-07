# Roadmap

What's being worked on next, roughly in order. Plans change as testing and the game's own code turn things up; the [issues](https://github.com/JDevWebb/5th-echelon-enhanced/issues) have the details, and ideas are welcome there.

## Next

### Code signing for Windows

A signed `launcher.exe` and client DLL, so antivirus programs stop guessing (see [Antivirus warnings](../README.md#antivirus-warnings)). Until then, each release is sent to Microsoft for review.

### Global matchmaking

One matchmaking server for everyone, relays kept near players, hosts ranked by ping: the second step of [cross-region play](#cross-region-play) below.

### Coordinator failover

Moving the community network's coordinator to a one-level name, so Cloudflare can proxy it, then turning [failover](failover.md) on.

### When a public match's host leaves

When the host of a public match leaves, the match ends for everyone else, since the 0.4.0 rule on who may take a session over doesn't count the players who joined from a search. Next: count them, and find out whether the game hands hosting over.

## Cross-region play

Players on different regional servers (EU, North America, Oceania) can't join each other's matches yet: each server's ids for players and sessions are its own. The research is in [research/cross-region.md](research/cross-region.md). The plan, in three steps:

1. **Cross-server invites, with an assisted switch.** An invite from a friend on another server reaches you through the coordinator; accepting it moves you to their server and starts the game there. Both players then play on one server, with everything that already works. Small, low risk, and the first to come.
2. **One matchmaking server for the community network**, with the regional servers kept for what needs to be close: the relay and the internet-play helper. Matches already run peer to peer, and matchmaking doesn't need a low ping, so everyone in the network could find everyone. Most of the work is splitting the relay and helper from matchmaking.
3. **Session sharing between independent networks**, last and only if there's a need: servers run by different people mirroring each other's sessions. The hardest and riskiest of the three.

Also worth an experiment: switching servers without restarting the game.

## Game features the server doesn't have yet

- **Uploading content** (`UserStorage`) works from 0.4.3: what the game uploads is its ShadowNet companion snapshot (loadouts, owned items, purchases, challenge progress), which it never reads back. Servers keep each player's latest for their admins; what to do with it for players (a profile, challenge leaderboards) is open.

## Running servers

- **The admin UI:** more reports as operators ask for them; the bandwidth, players and alerts reports are new.
- **More regions** for the community network, as players turn up from further away.

## On hold

Planned, and waiting until players need them.

### Settings of our own, apart from the original launcher's

The launcher and the original 5th Echelon launcher both keep their settings in `uplay.toml` in the game's folder, which the game's client reads too. Each one resets the other's: this launcher backs up a file it didn't write (`uplay.toml.old`) and starts over, and the original rewrites the file in its own format. Players who switch between the two lose their settings each time. For now: use one launcher.

The fix, if players ask for it:
- **A file of our own,** say `5th-echelon-enhanced.toml`, for this launcher and its client. The name is in one place (`hooks-config`).
- **Moving over:** the first start copies this launcher's settings out of `uplay.toml` into the new file, and puts the original launcher's backup (`uplay.toml.old`, when it's the original's) back as `uplay.toml`, so its profiles return.
- **Then:** no more starting over for the original's files (`fresh_start`), and the docs that name `uplay.toml` (`AllowDataMismatch` among them) updated.
- **What stays shared:** the game loads one online DLL (`uplay_r1_loader.dll`), and each launcher installs its own. This one replaces it every time it starts; started from the original launcher, the game may run this one's client with this one's settings.

## Done recently

- **0.4.3:** a security review of the whole network, attacking a copy of it and then real servers, and every finding fixed: caps on the API's and the coordinator's connections and messages, limits on game connections that haven't signed in, a relay that carries only the players the server put together, and a directory that lists a server only where it answers as itself. Tested in real games across two networks, co-op and Spies vs Mercs, relayed and direct, over half-hour matches.
- **0.4.3:** Support in the launcher (the community network's), replies to reports, the overlay's key yours to choose, the 15 minutes of the game's log before a problem, and in the admin UI a **Live** page and how each stay ended.
- **0.4.3:** the server's Docker image signed with each release (`scripts/verify-image.sh`).
- Friends list that keeps up: a friend added mid-session shows in the game's own friends list without a restart (the client tells the game the list changed, as Ubisoft's service did). Confirmed in real games.
- Signed releases: since 0.4.0, every release's checksums carry the release key's signature, and launchers and servers install releases on their own ([Releases and CI](../README.md#releases-and-ci)).
- Stats and leaderboards: the server keeps the stats the game writes after each match and mission (Spies vs Mercs per mode, weapon and gadget, medals, ladders, solo and co-op missions), added up as the game's stats configuration says, and answers the game's leaderboards: solo and co-op high scores and best times, Spies vs Mercs total score and the ladders, overall, around you and among your friends. Leaderboards are global: every server sends the stats to the coordinator, which ranks each player once across the network (by their identity), so your stats follow you between servers. Servers answer the game from what the coordinator last sent, and from their own stats while it can't be reached.
- A VPN on the player's PC (Radmin VPN): when the game offers the VPN's address for connecting, the server uses the address the player connected from instead of refusing the game's request.
- Co-op over the relay: missions failed to load when a player was relayed, because the relay dropped the game's largest packets. Confirmed in games with one and with both players relayed, in co-op and Spies vs Mercs.
- Private matches stay private: Find Teammate offered private co-op matches to anyone, friends or not. Public matchmaking still finds public rooms, between friends and strangers alike, direct or relayed.
- Hosts without a router port forward can be invited to: they go through the relay.
- The launcher notes when a VPN sits between you and the server, which adds delay and sends your matches through the relay.
- A third community server, in North America (Beauharnois, Canada).
- Servers refuse outdated clients, and the launcher updates itself when one does (release builds), and checks every 4 hours.
- The admin UI rebuilt with live updates, bandwidth and players reports, and alerts to Discord or Slack.
- A redesigned launcher: guided setup, server news, online friends, and the server list with pings.
