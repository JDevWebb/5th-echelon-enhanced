# Roadmap

What's being worked on next, roughly in order. Plans change as testing and the game's own code turn things up; the [issues](https://github.com/JDevWebb/5th-echelon-enhanced/issues) have the details, and ideas are welcome there.

## Next

### The first signed release

The launcher, the client and the servers update themselves from signed GitHub releases, but every build so far has been a development build, handed out by hand.
- **Signed releases:** each release's checksums signed with the release key, so launchers and servers install it on their own ([Releases and CI](../README.md#releases-and-ci)).
- **Code signing for Windows:** a signed `launcher.exe` and client DLL, so antivirus programs stop guessing (see [Antivirus warnings](../README.md#antivirus-warnings)). Until then, each release is sent to Microsoft for review.

### Friends list that keeps up: confirming in the game

The game asks for its friends list once, when it reaches the online menu, so a friend added mid-session only appeared after a restart. The client now tells the game the list changed, the way Ubisoft's service did, and the game fetches it again ([friends.md](friends.md)). It's in the code; next is confirming it in real games.

## Cross-region play

Players on different regional servers (EU, North America, Oceania) can't join each other's matches yet: each server's ids for players and sessions are its own. The research is in [research/cross-region.md](research/cross-region.md). The plan, in three steps:

1. **Cross-server invites, with an assisted switch.** An invite from a friend on another server reaches you through the coordinator; accepting it moves you to their server and starts the game there. Both players then play on one server, with everything that already works. Small, low risk, and the first to come.
2. **One matchmaking server for the community network**, with the regional servers kept for what needs to be close: the relay and the internet-play helper. Matches already run peer to peer, and matchmaking doesn't need a low ping, so everyone in the network could find everyone. Most of the work is splitting the relay and helper from matchmaking.
3. **Session sharing between independent networks**, last and only if there's a need: servers run by different people mirroring each other's sessions. The hardest and riskiest of the three.

Also worth an experiment: switching servers without restarting the game.

## Game features the server doesn't have yet

The game calls a few services the server doesn't implement, seen in the community servers' logs:
- **Leaderboards by player** (`PlayerStats.ReadLeaderboardsByPlayers`): your friends' standings.
- **Uploading content** (`UserStorage.SaveContentAndGetUploadInfo`).

Each needs the game's side worked out first, from the protocol and the game's code, without shipping anything of the game's.

## Running servers

- **The admin UI:** more reports as operators ask for them; the bandwidth, players and alerts reports are new.
- **More regions** for the community network, as players turn up from further away.

## Done recently

- A third community server, in North America (Beauharnois, Canada).
- Servers refuse outdated clients, and the launcher updates itself when one does (release builds), and checks every 4 hours.
- The admin UI rebuilt with live updates, bandwidth and players reports, and alerts to Discord or Slack.
- A redesigned launcher: guided setup, server news, online friends, and the server list with pings.
