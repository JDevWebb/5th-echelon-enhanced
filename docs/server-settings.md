# Server settings added by this fork

These go in the server's `service.toml`, next to upstream's settings. Both are off, or at their safest, unless you set them.

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
