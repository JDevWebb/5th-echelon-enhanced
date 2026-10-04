# Server settings added by 5th Echelon Enhanced

These go in the server's `service.toml`, next to upstream's settings. Each is off, or at its safest, unless you set it; the NAT helper for internet play is on.

## `[community_api]`: the JSON API on port 80

The config server on port 80 also answers a small JSON API under `/api/`, for launchers, overlays and tools. The game never asks for these paths, and every other path still returns its online config.

```toml
[community_api]
info = true        # GET /api/info: name, version, features, and GET /api/news: the server's news (on by default)
presence = false   # GET /api/presence: registered players, who's online, what they're playing
accounts = false   # POST /api/register and /api/login, for tools (register is refused with [limits] require_identity)
unhandled = false  # GET /api/unhandled: game calls this server couldn't answer
```

- **presence** shares every registered username and what each online player is doing. Turn it on only for a community whose players expect that.
- **accounts** lets anyone who can reach port 80 create an account.
  - Registering and logging in are rate-limited per address (see `[limits]`).
  - Usernames are 1–32 characters. Passwords are 8–63 characters for a new account (the game's limit); signing in takes up to 128.
  - Wrong passwords and unknown users get the same answer.
- **unhandled** is for development: it lists the game's RMC calls the server has no handler for, most frequent first.

`/api/info` only lists `presence` and `accounts` in its features when they're on. Any part that's off answers `404`.

## `trusted_subnet`: fix addresses from the wrong adapter

Matches run peer to peer, over the address the game registers with the server. The game takes that address from whichever network adapter it picked. When that's the wrong one (the home LAN instead of the VPN everyone plays over), nobody can join that player.

Put the subnet your players connect from in the **secure** service's settings, next to `storage_host`:

```toml
[service.sc_bl_secure.settings]
storage_host = "..."
trusted_subnet = "10.8.0.0/16"
```

A player connecting from inside that subnet then has every other address it advertises replaced by the one the server saw it connect from. Players from outside the subnet, or servers without the setting, keep upstream's behaviour.

## `[nat]`: internet play without a VPN

On by default. Players need UDP `21128` and `21129` to reach the server (the port in `listen` and the one after it).

```toml
[nat]
enabled = true
listen = "0.0.0.0:21128"
relay = "auto"               # "auto", "all" or "off"
relay_ports = [40000, 40999]
relay_kbps_per_player = 2048
# public_address = "203.0.113.10"   (written by --public-address)
```

How it works:
- The client in each game probes the helper **from the game's match port** (UDP 13000). The helper answers with the address it saw, and the client tells the game to advertise it. The game's own NAT probing, which this server already forwards, then gets through most routers.
- Probing the second port shows whether a router gives each destination its own port (symmetric NAT, common with carrier-grade NAT). Direct connections fail with those.
- If the game still registers its local address, the server replaces it with the one the helper saw, keeping the local one for players on the same network.

**`relay`** decides who plays through the server:
- **`auto`:** every player whose router has no port mapping for the game (UPnP or NAT-PMP), and anyone with symmetric NAT or who chose **Always through the server**. Without a mapping nobody can start a connection to a player, so they could join others but nobody could join them. Players with a mapping connect directly, and a LAN party with the server on the same network relays nobody.
- **`all`:** every player. The most reliable, but every match goes through the server.
- **`off`:** nobody; those players can't join others over the internet.

Relayed players advertise an address on this server with a port from **`relay_ports`**. Nothing listens on those ports: the client wraps the packets for the helper's port. So they need no firewall rule, but they must not overlap other services' ports.

The relay carries game packets of up to 1,472 bytes, the largest a 1,500-byte link carries, so every packet the game sends. Wrapping adds 20 bytes, so the largest ones (a co-op mission sends packets of about 1,460 bytes while it loads) travel as two IP fragments. Until 0.3.175 the limit was 1,400 bytes, and a co-op mission with a relayed player often failed to load. Each relayed player uses roughly 20–60 KB/s in each direction during a match. **`relay_kbps_per_player`** caps it, and packets over the cap are dropped.

The helper only relays between players who probed it, so it can't be used to send traffic elsewhere. Probes are padded so an answer is never bigger than the question.

**`public_address`** is the address relay addresses use. It's written by `--public-address` / `FE_PUBLIC_ADDRESS`; without it, the secure service's address is used.

In Docker, the helper needs to see players' real addresses: Docker on Linux keeps them; Docker Desktop doesn't.

## `[public]`: what players connect to

For a server behind a reverse proxy, or with its ports forwarded to other numbers. The full guide, with a Caddy example, is [reverse-proxy.md](reverse-proxy.md).

```toml
[public]
host = "blacklist.example.com"   # content downloads are addressed to it
api = 80                         # the gRPC API, e.g. through Caddy
# api_tls = 443                  # the gRPC API over HTTPS (the installer sets it once Caddy has a certificate)
content = 80
# login = 21126                  # game login (UDP)
# secure = 21127                 # game service (UDP)
# nat = 21128                    # NAT helper (UDP; the next port too)
proxies = ["172.17.0.0/16"]      # proxies whose X-Forwarded-For is believed
# aliases = ["bl.example.com"]   # other names players reach this server by
```

Without this section, nothing changes. With it:
- the server hands out these ports, and for any left unset, the port its service listens on: the login port in the online config, the game service in tickets, content downloads, and relay addresses;
- `GET /api/info` reports them as `ports` (and `host`), and the launcher's **Connect** stores them for the player;
- `X-Forwarded-For` is believed from the listed proxies (and always from a proxy on this machine), so the rate limits count each player, not the proxy.
- With **`api_tls`**, launchers and overlays use `https://host[:api_tls]` for the API instead, so passwords and sign-in tokens never travel readable. The launcher asks `https://host/api/info` first (then the `api_tls` port), and only that answer can name a coordinator. Once a server has answered over HTTPS, the launcher never moves that player back to plain HTTP: if HTTPS stops answering, Connect fails with a message instead. A server that never had HTTPS for that player falls back to the plain API when the HTTPS port doesn't answer, and the launcher's Status card then warns that the connection is unencrypted (not for servers on the player's own network).
- Identity signatures name the host the player typed. The server accepts signatures for its `host`, its public address and its **`aliases`**, and no others, so a signature made for another server can't be replayed here.

## `[limits]`: rate limits on accounts and logins

```toml
[limits]
failed_logins_per_10_minutes = 30  # per address, over every login route
logins_per_10_minutes = 120        # per address, failed or not
registrations_per_hour = 20        # per address
open_registration = true           # false: no new accounts (only existing ones sign in)
require_identity = false           # true: every account is linked to a player identity
require_tls_for_credentials = false  # true: passwords and sign-ins only over HTTPS (or from a LAN)
```

- With `require_identity = true` (the installer's default), accounts are only made through the launcher, linked to the player's identity: the launcher finds a player's account by their identity, so they never type a password, and accounts can't be unlinked. `/api/register` and password-only registrations are refused.

- The login limits cover the game's own login, the launcher's (gRPC) and the community API. `failed_logins_per_10_minutes` counts failures: players sharing one address sign in often, and only password guessing fails a lot. `logins_per_10_minutes` counts every login, because each one checks a password, which takes the server real work; raise it if many players sign in from one address (a big LAN party playing on a server elsewhere).
- When many logins at once keep the server checking passwords for more than a few seconds, further ones are answered "busy, try again" instead of waiting in line.
- Players behind one address (a LAN party, a household) share the registration budget, so raise it if a big group sets up at once.
- Requests from the server's own machine (loopback) are never limited.
- Failed logins also count per account and address: 10 in 10 minutes stop that address trying that account, without locking its owner out elsewhere. After 50 failures for one account in 10 minutes from anywhere, only addresses that have signed in to it before (since the server started) may try it until the failures age out. IPv6 addresses count per /64.
- With **`require_tls_for_credentials`**, the launcher's Login, Register, KeyLogin, LinkIdentity and Rename, and the community API's `/api/login` and `/api/register`, are refused when they arrive unencrypted. Through a reverse proxy, the proxy's `X-Forwarded-Proto` header says how the player connected (Caddy sets it; only proxies on this machine or listed in `[public] proxies` are believed). Without a proxy, only this machine and private networks (192.168/16, 10/8, 172.16/12, 100.64/10, IPv6 unique-local and link-local) are let through, since the API itself speaks only plain HTTP/2. Turn it on only when the API is served over HTTPS (`[public] api_tls`) and the players' launchers use it. The Linux installer turns it on together with `api_tls` (and off when Caddy has no certificate).
- `FE_MAX_CONNECTIONS_PER_IP` (environment, default 256) caps the game connections from one address.

## `[admin]`: the admin API

The admin API lists and deletes accounts and games, over the gRPC port (50051). The launcher's Host screen uses it.

```toml
[admin]
enabled = true
```

- It's always on when the launcher starts the server (`--launcher`). Otherwise `enabled` turns it on.
- The key is written to `admin-key.txt` next to the database, readable only by the server's user. Paste it into the launcher's "Manage a server".
- Anyone with the key can delete accounts: keep it private, and don't expose port 50051 more widely than you need to.
- The installer's Caddy never passes the admin services on (they answer 403 there). Manage such a server from the machine itself, or through an SSH tunnel: `ssh -L 50051:127.0.0.1:50051 you@server`, then connect the launcher to `localhost`.
- The launcher warns before sending the key over plain `http://` to a machine that isn't on a private network.

## `[friends]`: who is on a player's friend list

```toml
[friends]
mode = "everyone"   # or "mutual"
```

- **everyone** (the default): every player on the server is on the game's friend list, and anyone can invite anyone they haven't blocked. This is how it worked before friend lists; it suits a LAN or a group that all know each other.
- **mutual**: only friends are on it, and only friends can invite. The Linux installer sets this, for public servers.

Friend requests, blocks and player search work in both modes, in the overlay. See [friends.md](friends.md).

## `[clients]`: refusing outdated launchers and game clients

```toml
[clients]
# minimum_version = "0.3.156"   # unset: this server's own release; "off": any client
```

The launcher and the game's client DLL are one release (the launcher carries the DLL and replaces any other version in the game folder when it checks the game). Both say their release when they sign in to the API, and the server refuses one older than `minimum_version`, or one that doesn't say (every client from before this check). The player sees what to do: "Update the launcher", or start the game from the launcher.

The game's own sign-in carries no version, so the server lets it in only for an account that a current launcher or client DLL signed in to in the last 24 hours, with no outdated one trying since. The client DLL signs in as the game starts, so a game with an old DLL is refused even if its player has a new launcher.

Unset, the minimum is the server's own release: when a server updates, players need a client at least as new. Set an older version to let clients that are still compatible keep playing after a server-only update. `/api/info` reports the minimum as `minimum_client`.

## `[federation]`: sharing friends with other servers

```toml
[federation]
coordinator = "https://coordinator.example.com"
join_token = "..."   # from the coordinator's operator; only needed until joined
name = "Kiwi Ops"    # in the server directory (default: the public host)
region = "Sydney"
listed = true        # false: share friends, but stay out of the directory
auto_update = true   # install the releases the coordinator rolls out (see operations.md)
# allow_http = false # only for tests: allow an http:// coordinator (the server's secret travels readable)
```

Off until `coordinator` is set. It must be `https://` (or on this machine) unless `allow_http` is on. The server then:
- joins with the token, and keeps its credentials in `federation.key`;
- sends friendships and blocks between players who linked their identity;
- pulls their friends from other servers;
- lists itself in the coordinator's server directory;
- reports its metrics (players, cities, activity, load, traffic) every minute;
- installs the releases the coordinator rolls out, through the installer's updater. With `auto_update = false`, or without the updater, the coordinator leaves it out of its directory once it falls behind. See [operations.md](operations.md).

`server-id.txt` holds this server's id, which players sign into their identity links; don't change it. See [friends.md](friends.md) for what is shared and how to run a coordinator.

## `[debug]` switches

```toml
[debug]
grpc_reflection = false       # list every API call to tools like grpcurl
session_owner_checks = true   # only a session's host and participants may change it
```

- With `session_owner_checks`, other players may still add or remove themselves (joining and leaving), but not change someone else's session.
- Turn it off only to rule it out when joins fail, and report what you saw.

## Login tickets

Tickets from the game's login are valid for 24 hours. The server checks this only when the game connects, right after login, so it never ends a game in progress.

The launcher's and overlay's sign-in tokens (the gRPC API) are valid for 30 days; the game and the overlay sign in again on their own.
