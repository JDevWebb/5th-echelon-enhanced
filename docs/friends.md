# Friends, identities and sharing friends between servers

Blacklist has one friend list, and it's whatever the Uplay layer hands it. Upstream's server gave every player every account on the server. That's fine for a LAN or a group that all know each other, but not for a public server with hundreds of strangers.

This fork has real friend lists, blocking, and an identity that carries friends between servers that share a coordinator.

## Friend lists

**Players add friends in the overlay** (<kbd>F5</kbd>):
- **Friends:**
  - requests waiting for your answer (Accept, Decline, Block);
  - your friends, with what they're playing (Invite, Remove, Block);
  - your requests waiting for theirs (Cancel);
  - who you've blocked (Unblock).
- **Find players:** search by part of a name, or see who's online; Add friend, Block, or Invite.
- A friend request, or an accepted one, shows up as a notice in the game. The Friends tab counts the requests waiting.

**The server decides who is on the game's own friend list**, `[friends] mode` in `service.toml`:

| Mode | The game's friend list | Who can invite you |
|---|---|---|
| `everyone` (the default) | every player on the server, as before | anyone you haven't blocked |
| `mutual` | only your friends | only your friends |

The Linux installer sets `mutual`, since a server on a VPS is usually public. Friend requests and blocks work in both modes; in `everyone` mode, friends are simply a shorter list in the overlay.

The game only reads its friend list and sends invites. It never calls Uplay's other friend functions (`IsFriend`, `RequestFriendship`, `AddToBlackList`, …), so friends are managed in the overlay rather than the game's menus.

### Blocking

Blocking someone:
- ends any friendship or request between you, and removes their waiting invites and notices;
- hides each of you from the other: friend lists, searches, and who's online;
- stops invites either way.

The blocked player isn't told. Their friend requests look sent, but never arrive.

### Invites

- **Limits:** a player can send 20 invites a minute, and have 5 waiting from different players at once. A new invite from someone replaces their older one. It no longer pushes out other people's invites, as it used to.
- **Joining:** when you accept, the server answers the game's search with the room of the player you accepted, even if others invited you too.

## Accounts and names

- **Names are unique whatever the case:** "Kiwi" and "kiwi" are one name. When a server updates, any accounts whose names differ only in case keep the oldest name, and the others get `-<id>` added.
- **The server sets each account's id** (the game's "Ubisoft id"), so no one can register with someone else's.
- **New names** are 1 to 32 letters, digits, `_`, `-` and `.`.
- **Sign-in tokens** for the launcher and the overlay expire after 30 days. The game signs in again on its own.

### Passwords

- **Each server gets its own password.** The launcher makes a random one for every server you join. Upstream reused your password on every server you tried, so every server operator saw it.
- **On Windows**, the saved password in `uplay.toml` is encrypted for your Windows user, with Windows' own data protection (DPAPI). The file has `ProtectedPassword` instead of `Password`. Only your Windows account on that PC can read it; copied to another PC, it doesn't work there.
- **On Linux**, and with the Windows launcher under Wine, the password stays readable in `uplay.toml`, and the file is readable only by your user. Wine prefixes don't share DPAPI keys, so encrypting it wouldn't survive.

## Your identity

The launcher makes you an identity the first time you join a server that supports one. It's an Ed25519 key, kept in the launcher's folder: `%APPDATA%\5th-Echelon\identity.key` on Windows (encrypted for your Windows user), `~/.config/5th-Echelon/identity.key` on Linux (readable by you only). Its public half, shown as e.g. `K7QF-2M9D`, is your id across servers.

The launcher uses it for two things:

1. **Linking.** It links each account you make to your identity by signing "this account on this server is me". The server checks the signature. That lets friends follow you between servers (see below).
2. **Signing in without a password.** On a new PC, or when a password stops working, the launcher signs in to your linked accounts with the key and gives each a new password. Each signature names the server, the account and the time, and works once, within five minutes of when it was made.

**To move to another PC:**
1. On the old PC: **Settings › Identity and friends › Copy to move it to another PC**.
2. On the new PC: paste it under **Import**.
3. Join each server again with the name you had there. The launcher signs in to your existing account there.

Whoever has your identity can sign in as you on every server you've linked, so keep the copied text private.

## Sharing friends between servers

Servers can share friends through a **coordinator**: a small service that one person runs for a group of servers.

When two players are friends on one server, have both linked their identities, and then both play on another server of the same group, they're friends there too, without asking again. Blocks travel the same way. Each server keeps its own accounts; only the links between them and your identity are shared.

### What the coordinator knows

- The member servers: each server's id, its secret (only a hash is kept), and its directory entry.
- For each linked account: your identity's public key, the server, and your name there.
- Friendships and blocks between identities.

It never sees passwords, invites, matches, or who is online.

### What it trusts

- **Servers** join with a join token from whoever runs the coordinator. They can only act for players linked on them, and only between two players both linked there.
- **Links** carry the player's own signature, which the coordinator checks too. A server can't claim an identity for a player who never used it.
- **Changes** are applied in order, and the latest wins.

A dishonest member server could still make friendships or blocks between players who are linked on it. Only give the join token to servers you trust.

### How changes travel

- A friendship or block made on a server is queued in its database and sent to the coordinator, so an outage only delays it.
- Each server asks for the friends of its players:
  - when they link;
  - once a minute while they're online;
  - when they open their friends list.
- Friend requests don't travel, only answered ones. A block wins over a friendship everywhere.

### The server directory

Every member server appears in the coordinator's **server directory**, unless `listed = false`, with its name, region, address and players online. Launchers show it under **Browse servers** on the Play screen. They measure their ping to each server through its NAT helper, the path game traffic takes, and suggest the nearest, busiest one.

A launcher learns the directory from the first server it joins that uses a coordinator; you can also set it in **Settings › Identity and friends**.

### Running a coordinator

**With the Linux installer**, on the same VPS as a server:

```sh
sudo bash install-server.sh --domain blacklist.example.com --coordinator-domain coordinator.example.com
```

- It installs `coordinator` as a second service (`5th-echelon-coordinator`), behind Caddy on HTTPS. Caddy gets the certificate, so TCP 443 must be open and the domain's A record must point at the VPS.
- The server on that VPS joins it.
- The summary prints the join token for other servers:
  ```sh
  sudo bash install-server.sh --domain other.example.com --coordinator https://coordinator.example.com --join-token <token> --region Sydney
  ```

**By hand:**
1. Run `coordinator --listen 127.0.0.1:8700 --data /var/lib/coordinator` behind any reverse proxy with HTTPS.
2. The join token is in `join-token.txt` in the data folder. Delete the file to make a new one; servers that already joined keep working.

**On a server**, `[federation]` in `service.toml` (see [server-settings.md](server-settings.md)):

```toml
[federation]
coordinator = "https://coordinator.example.com"
join_token = "..."        # only needed until it has joined
name = "Kiwi Ops"         # the name in the directory
region = "Sydney"
listed = true
```

The server keeps its credentials in `federation.key` once it has joined. Its log has a `Federation:` line for joining and anything that fails.

### The coordinator's API

| Call | Who | What |
|---|---|---|
| `POST /v1/join` `{token, server_id}` | a new server | joins; answers `{secret}` |
| `POST /v1/heartbeat` | a member (Bearer secret) | its directory entry |
| `GET /v1/servers` | anyone | the directory: servers seen in the last 2 minutes |
| `POST /v1/changes` `{changes: [...]}` | a member | links, unlinks, friendships and blocks, in order; one result each |
| `GET /v1/relations/<identity>` | a member | a player's friends and blocks, for a player linked on that server |
| `GET /v1/info` | anyone | name, version, number of servers |

## Testing

- **`build/build.sh bots`:**
  - on a default server: the `friends`, `block`, `invite-queue` and `identity-login` scenarios;
  - on a server in `mutual` mode: `friends-mutual`.
- **`build/build.sh federation-test`:** a coordinator and two servers in containers.
  - Both servers must appear in the directory.
  - A friendship made on the first server must reach the second.
  - A block on the second must reach the first.
