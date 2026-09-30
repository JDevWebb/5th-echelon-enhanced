# Server settings added by this fork

These go in the server's `service.toml`, next to upstream's settings. Each is off, or at its safest, unless you set it; the NAT helper for internet play is on.

## `[community_api]`: the JSON API on port 80

The config server on port 80 also answers a small JSON API under `/api/`, for launchers, overlays and tools. The game never asks for these paths, and every other path still returns its online config.

```toml
[community_api]
info = true        # GET /api/info: name, version, features (on by default)
presence = false   # GET /api/presence: registered players, who's online, what they're playing
accounts = false   # POST /api/register and /api/login: one-click accounts
unhandled = false  # GET /api/unhandled: game calls this server couldn't answer
```

- **presence** shares every registered username and what each online player is doing. Turn it on only for a community whose players expect that.
- **accounts** lets anyone who can reach port 80 create an account.
  - Registering and logging in are rate-limited per address (see `[limits]`).
  - Usernames are 1–32 characters, passwords 8–128.
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
- **`auto`:** players whose router can't be reached directly: symmetric NAT, a player who chose **Always through the server**, or a player on the server's own network without a router port mapping. Everyone else connects directly.
- **`all`:** every player. The most reliable, but every match goes through the server.
- **`off`:** nobody; those players can't join others over the internet.

Relayed players advertise an address on this server with a port from **`relay_ports`**. Nothing listens on those ports: the client wraps the packets for the helper's port. So they need no firewall rule, but they must not overlap other services' ports. Each relayed player uses roughly 20–60 KB/s in each direction during a match. **`relay_kbps_per_player`** caps it, and packets over the cap are dropped.

The helper only relays between players who probed it, so it can't be used to send traffic elsewhere. Probes are padded so an answer is never bigger than the question.

**`public_address`** is the address relay addresses use. It's written by `--public-address` / `FE_PUBLIC_ADDRESS`; without it, the secure service's address is used.

In Docker, the helper needs to see players' real addresses: Docker on Linux keeps them; Docker Desktop doesn't.

## `[public]`: what players connect to

For a server behind a reverse proxy, or with its ports forwarded to other numbers. The full guide, with a Caddy example, is [reverse-proxy.md](reverse-proxy.md).

```toml
[public]
host = "blacklist.example.com"   # content downloads are addressed to it
api = 80                         # the gRPC API, e.g. through Caddy
content = 80
# login = 21126                  # game login (UDP)
# secure = 21127                 # game service (UDP)
# nat = 21128                    # NAT helper (UDP; the next port too)
proxies = ["172.17.0.0/16"]      # proxies whose X-Forwarded-For is believed
```

Without this section, nothing changes. With it:
- the server hands out these ports, and for any left unset, the port its service listens on: the login port in the online config, the game service in tickets, content downloads, and relay addresses;
- `GET /api/info` reports them as `ports` (and `host`), and the launcher's **Set up** stores them for the player;
- `X-Forwarded-For` is believed from the listed proxies (and always from a proxy on this machine), so the rate limits count each player, not the proxy.

## `[limits]`: rate limits on accounts and logins

```toml
[limits]
failed_logins_per_10_minutes = 30  # per address, over every login route
registrations_per_hour = 20        # per address
```

- The login limit covers the game's own login, the launcher's (gRPC) and the community API. Only failed logins count: players sharing one address sign in often, and only password guessing fails a lot.
- Players behind one address (a LAN party, a household) share the registration budget, so raise it if a big group sets up at once.
- Requests from the server's own machine (loopback) are never limited.

## `[admin]`: the admin API

The admin API lists and deletes accounts and games, over the gRPC port (50051). The launcher's Server screen uses it.

```toml
[admin]
enabled = true
```

- It's always on when the launcher starts the server (`--launcher`). Otherwise `enabled` turns it on.
- The key is written to `admin-key.txt` next to the database, readable only by the server's user. Paste it into the launcher's "Manage a server".
- Anyone with the key can delete accounts: keep it private, and don't expose port 50051 more widely than you need to.

## `[friends]`: who is on a player's friend list

```toml
[friends]
mode = "everyone"   # or "mutual"
```

- **everyone** (the default): every player on the server is on the game's friend list, and anyone can invite anyone they haven't blocked. This is how it worked before friend lists; it suits a LAN or a group that all know each other.
- **mutual**: only friends are on it, and only friends can invite. The Linux installer sets this, for public servers.

Friend requests, blocks and player search work in both modes, in the overlay. See [friends.md](friends.md).

## `[federation]`: sharing friends with other servers

```toml
[federation]
coordinator = "https://coordinator.example.com"
join_token = "..."   # from the coordinator's operator; only needed until joined
name = "Kiwi Ops"    # in the server directory (default: the public host)
region = "Sydney"
listed = true        # false: share friends, but stay out of the directory
```

Off until `coordinator` is set. The server then:
- joins with the token, and keeps its credentials in `federation.key`;
- sends friendships and blocks between players who linked their identity;
- pulls their friends from other servers;
- lists itself in the coordinator's server directory.

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
