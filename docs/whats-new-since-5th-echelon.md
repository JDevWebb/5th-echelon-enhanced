<!-- Moved from the README (2026-11-10), unchanged. -->
# What's new since 5th Echelon

Compared with upstream [5th Echelon 0.2.5](https://github.com/unixoide/5th-echelon/releases/tag/v0.2.5):

**Launcher**
- Rewritten around a new `setup` library, for Windows and Linux (Steam, Flatpak, Steam Deck, Lutris, Heroic). Since 0.5.0 one native window in [Slint](https://slint.dev), the same on Windows, Linux and the Steam Deck (Game Mode too), in a dark and a light look, with 5th Echelon Enhanced's own mark; the in-game overlay follows its look.
- A Play button that says whether you're ready and why not, a Friends page (requests, finding players, blocks), and **Choose a match...** to pick an open public match or start a new one.
- A guided first-run setup, and a Status card with fixes. **Connect** finds your account with your identity, and asks for a name only when you have none on that server.
- Typing a network's address (a coordinator) sets you up on its best server by ping; the server menu moves you to another.
- Adapter pinning that works (upstream's saved a value that never matched an adapter).
- Server management, and verified updates from this project's releases.
- Settings are never silently reset: an unreadable file is kept as `uplay.toml.broken`, and outside changes are merged rather than overwritten.
- No sample accounts; DirectX 11 by default.
- A window you can resize, maximise or make full screen (F11), with everything scaled to fit it, and your own size on top (Settings › Display, or Ctrl + / Ctrl −).

**Internet play**
- Players no longer need a LAN or VPN:
  - the client answers the game's own "what's my public address?" request with the address the server's NAT helper sees, so the game advertises it and punches through NAT itself;
  - it asks the router to forward the match port (UPnP, then NAT-PMP);
  - the server relays matches for players whose router can't forward a port to the game (no UPnP, carrier-grade or symmetric NAT);
  - the server corrects the address a game registers, if the game didn't take the public one.
- The findings behind it are in [docs/research/nat-traversal.md](research/nat-traversal.md).

**Friends**
- Real friend lists instead of "everyone on the server":
  - requests, blocking and player search in the overlay;
  - friends-only lists and invites for public servers.
- An identity per player that links their accounts across servers, and signs them in on a new PC.
- A coordinator that shares friends between servers, reserves names across them, and keeps a server directory; the launcher adopts it on joining a server that names it over HTTPS, pings every server and preselects the best.
- Every account is linked to its player's identity: the launcher finds your account on a server by itself, and asks for a name only when you have none there yet. Servers can require it (`[limits] require_identity`, on by default with the installer).
- Renaming, with the account id kept, and warnings in the overlay about players using a friend's name on another server.
- Each server gets its own random password (upstream reused one everywhere); on Windows it's saved encrypted.
- See [docs/friends.md](friends.md).

**Running a network**
- **Automatic updates:**
  - the coordinator rolls out each release signed with the release key to its servers: one first, watched for 10 minutes, then the rest when they're quiet;
  - each machine's root updater checks the signature again, keeps the previous binaries, and puts them back if the new release doesn't come back healthy;
  - admins can pause, pin, halt or roll back.
- **Global matchmaking:** one matchmaking server for a network, with relays near each region's players (`--role relay`); the launcher picks the nearest relay, and relays pass packets between each other for players the matchmaking server put together, with passes bound to each game's session key.
- **Membership is kept current:** servers that don't install updates, or still run an old release a day after a rollout, leave the directory.
- **Metrics from every server:** players, cities (DB-IP, looked up on the server; no addresses sent), what's being played, sign-ins, load, bandwidth and relayed traffic. Pings come from the coordinator and from players' launchers.
- **An admin UI:**
  - the views: overview, servers, bandwidth, players and map, playlists, network, alerts, players' reports, updates, security and audit log, updated live;
  - sign-in: passkeys, an authenticator app or recovery codes;
  - address and country restrictions;
  - served only through Cloudflare.
- See [docs/operations.md](operations.md).

**Security**
- A security audit of the server, the client, the launcher, the coordinator and the installer, and every finding fixed:
  - **client:** the strings and friend data a server sends can no longer overflow the game's memory; the friend list no longer blocks or leaks; a server can't join you to a match without your click (`AllowForceJoin`, off);
  - **game protocol:** decompression, fragment and connection caps; replies go only to a connection's own address; the login proof is checked before anything else; private matches need an invite; the game's own invites follow friends-only mode and blocks;
  - **accounts and API:** login limits per account and per IPv6 /64, tokens that end when the password changes, the same answer for unknown users and wrong passwords, blocks that stay invisible, activity shown to friends only, reserved names;
  - **identity:** signatures name the server you actually connected to, and a key login signs the password it sets, so a malicious server can't replay them elsewhere;
  - **NAT helper:** registrations need a ticket from your login, replies carry a cookie, relayed packets a tag, with caps per address and a packet rate limit;
  - **coordinator:** a server can't take over another's place, typed and checked listings, rate limits, `remove-server` and `new-token`.
- A third review attacked a copy of a whole network (a coordinator and servers, as the installer sets them up), then real servers on the internet, and every way it found to take a server down, lock players out or reach them through the relay is fixed: connection and message caps, limits on connections that haven't signed in, a relay only between players the server put together, a directory checked against what answers at each address, and the server's Docker image signed with each release. The fixes were tested in real games across two networks.
- **The launcher's API over HTTPS** where the server has a domain; the admin API is never reachable from the internet, and the launcher warns before sending an admin key unencrypted.
- **Signed releases:** the launcher and the installer install only releases whose checksums carry the release key's signature; downloads are size-capped; CI pins its actions and attests every download's provenance.
- **Installer and Docker:** inputs checked, the join token never printed, sandboxed services (Caddy too, with its admin API off the network), Caddy from its own package repository or a static build pinned by checksum (updated on reruns), the server image without root.
- **The admin UI:**
  - passwords with Argon2id;
  - TOTP and passkeys (WebAuthn, with user verification);
  - single-use recovery codes, and lockouts after failed sign-ins;
  - a fresh second factor for sensitive changes;
  - CSP, CSRF and origin checks;
  - an audit log.
- **Hosts:** `harden-host.sh`, for SSH, fail2ban, the kernel, and automatic updates (Caddy's included) with reboots at a quiet hour.

**Invites and matches**
- **Host migration** (from [#8](https://github.com/JDevWebb/5th-echelon-enhanced/pull/8) by Matthias Walther): a match lobby carries on under another player when its host leaves, and a party's leader brings the party into a friend's private match and back. Part of every game, with no switch; DirectX 9 and 11.
- Searches leave out matches whose round has started (the host's game says so), matches with someone you blocked, and, where a server asks for it, far hosts while a near one matches.
- Invites into private matches, from [#123](https://github.com/unixoide/5th-echelon/pull/123) by Matthias Walther, with follow-up fixes:
  - pushes are resent until acknowledged;
  - duplicate packets are answered, not handled twice.
- Leaving and abandoning a session removes the player, and empty sessions end.
- Private matches (co-op and Spies vs Mercs) stay out of matchmaking: a room with only private seats counts as invite-only, whatever the game announces, so Find Teammate no longer drops a stranger into someone's private co-op match.
- Co-op loads over the relay: it carries the game's largest packets (up to 1,472 bytes), which co-op sends while a mission loads.
- The `trusted_subnet` address fix for VPN play.

**Fair play**
- Bans hold for the identity on every server of a network, and for the PC (hashed hardware ids, salted per server, so one server's hashes mean nothing to another).
- Report-only checks for the admins: stats far from the network's normal, round settings changed mid-round, hosts who keep ending matches soon after they start, lag-switch rhythms at the relay, the game's own files against everyone else's (hashed at sign-in), the game's clocks against Windows' own (speed hacks), the game's code against its executable, and other players' games seeing someone move faster than the game allows (counted only from independent witnesses).
- **Report a player** from the overlay, and match recordings the admins can replay (top down, in 3D among the map's walls, and whole matches from every player's recording).

**Server stability and security**
- Crashes fixed:
  - bad packets can no longer take the server down;
  - a failed service restarts the process instead of leaving it half-alive;
  - matchmaking no longer panics on odd data.
- Logins survive restarts (the token keys are kept).
- Plain logins can no longer sign in as someone else; sample accounts are disabled (including the third one upstream left with its sample password).
- Names are unique whatever their case, the server sets account ids, API tokens expire after 30 days, and deleting an account with a pending invite works.
- Presence works; memory no longer grows without bound; the HTTP servers handle connections in parallel, with timeouts.
- Rate limits, expiring tickets, session owner checks, and a constant-time admin key check.
- An opt-in admin API setting, and gRPC reflection off by default.

**Game client**
- Works under Wine and Proton:
  - the network check Wine lacks is emulated ([#128](https://github.com/unixoide/5th-echelon/pull/128) by Thiago);
  - it starts without Ubisoft Connect there;
  - its network runtime uses two threads instead of one per CPU core.
- Signs in again on its own when the server says it's signed out.
- A "data version mismatch" means the two copies' game data differs, almost always because of a mod that changes the game's files (a loose `PEC.ini` in `src\SYSTEM`, say). Copies with the same data play together, mods or not; the launcher warns when yours is modified. Steam and Ubisoft Connect copies, in any language, have the same data.
- Crash fixes in the hook.
- `uplay.override.toml`, for tools that set up the game (see below).
- A redesigned overlay.

**Tooling**
- Docker builds for Linux and Windows (`build/build.sh`), and a Docker image for the server.
- A Linux installer for servers and coordinators (`scripts/install-server.sh`), a host hardening script (`scripts/harden-host.sh`), a [deployment guide](deploying.md) and an [operations guide](operations.md).
- Load tests (`build/build.sh load`, [results](load-testing.md)): 1,000 players on two cores.
- Headless test players (`tools/testbot`) that play through logins, lobby and private-match invites, internet play and the relay, friends, blocks, identities and renames, and packet loss against a real server; `proxy-test` runs them behind Caddy, and `federation-test` runs two servers sharing friends through a coordinator, and checks their metrics reach it.

## Setting up the game from another tool

Tools that set up the game (an installer, a VPN client, a community's own app) can write `uplay.override.toml` next to `uplay.toml`. It sets:
- the server (`ConfigServer`, `ApiServer`);
- the account (`[User]`);
- the network pin (`[Networking]`);
- optionally `[Managed] By = "Tool name"` and `HelpText`.

The hook applies it on every start, even if the launcher has never run. With `[Managed]` set, the launcher shows the install as managed by that tool and doesn't change its settings.
