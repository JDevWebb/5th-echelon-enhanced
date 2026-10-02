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
- **Renaming:** **Settings › Servers and accounts › Rename** in the launcher. The account id stays, so friends, blocks and invites carry on, and the old name is free again.
- **New names** are 1 to 32 letters, digits, `_`, `-` and `.`.
- **Sign-in tokens** for the launcher and the overlay expire after 30 days. The game signs in again on its own.

### Passwords

- **Each server gets its own password.** The launcher makes a random one for every server you join. Upstream reused your password on every server you tried, so every server operator saw it.
- **On Windows**, the saved password in `uplay.toml` is encrypted for your Windows user, with Windows' own data protection (DPAPI). The file has `ProtectedPassword` instead of `Password`. Only your Windows account on that PC can read it; copied to another PC, it doesn't work there.
- **On Linux**, and with the Windows launcher under Wine, the password stays readable in `uplay.toml`, and the file is readable only by your user. Wine prefixes don't share DPAPI keys, so encrypting it wouldn't survive.

## Your identity

The launcher makes you an identity the first time you join a server. Every account is linked to one. It's an Ed25519 key, kept in the launcher's folder: `%APPDATA%\5th-Echelon\identity.key` on Windows (encrypted for your Windows user), `~/.config/5th-Echelon/identity.key` on Linux (readable by you only). Its public half, shown as e.g. `K7QF-2M9D`, is your id across servers.

The launcher uses it for two things:

1. **Linking.** It links each account you make to your identity by signing "this account on this server is me". The server checks the signature. That lets friends follow you between servers (see below).
   - It isn't optional: the join form has no username, password or link option. An account made before this is linked the next time you connect.
   - A server you join can change your friends on the servers that share friends with it, so join servers you trust.
   - Servers with `[limits] require_identity = true` (the installer's default) refuse accounts without an identity, and unlinking.
2. **Finding your account.** When you connect, the launcher signs in with the key to whichever account on that server is linked to it, and gives it a new password. You never type a name or password for an account you have; the launcher asks for a name only when your identity has no account there yet. Each signature names the server and the time, and works once, within five minutes of when it was made. Only the identity's holder learns whether it has an account on a server.

**To move to another PC:**
1. On the old PC: **Settings › Identity and friends › Copy to move it to another PC**.
2. On the new PC, **before connecting to any server**: paste it under **Import** and press **Use this identity**.
3. Connect to each server. The launcher finds your existing account there by itself.

Import first: connecting first makes the new PC an identity of its own and an account under it. Your usual name is your old identity's, so that account gets another (`Kiwi2`). Importing afterwards still brings your real accounts back, but the extra one stays on the server.

**Replacing an identity:** if the PC already has a different identity, Import asks before replacing it, as accounts made with an identity can only be signed in to with it. The replaced one is kept beside the new one, as `identity.<its id>.key` in the launcher's folder (encrypted as before). To go back, copy it over `identity.key`.

Whoever has your identity can sign in as you on every server you've linked, so keep the copied text private.

## Sharing friends between servers

Servers can share friends through a **coordinator**: a small service that one person runs for a group of servers.

When two players are friends on one server, have both linked their identities, and then both play on another server of the same group, they're friends there too, without asking again. Blocks travel the same way. Each server keeps its own accounts; only the links between them and your identity are shared.

### One name per player across the group

Names are unique on each server, so two strangers could both be "Kiwi" on different servers. Within a group of servers that share a coordinator, that can't happen:

- **The coordinator reserves names.**
  - A name belongs to the identity that first used it, on any server in the group, whatever the case.
  - The launcher makes each account with your identity's signature, so the server claims the name for you before making the account.
  - On another server of the group, a name someone else holds is refused ("That name belongs to another player on the servers sharing friends"). The launcher then picks the next free one (`Kiwi2`), as it does for a name taken on that server.
- **Accounts made without an identity** (an older launcher, the community API) can only take names nobody has reserved.
- **Clashes from before** (two servers that had a "Kiwi" each, then joined one coordinator): the first to link keeps the name. The other account keeps working, but is flagged. Its owner sees a note in the overlay's Friends tab asking them to rename, and anyone looking them up sees "another player has this name on other servers". Renaming clears it.
- **A rename claims the new name first**, and the old one is released once nothing of yours uses it.
- **If the coordinator is down**, new accounts are still made, and their names are claimed when their links go through. A clash found then is flagged, as above.

Servers outside the group can reuse any name; friends don't travel there. The overlay still warns you:

- It remembers your friends' identities from every server you play on, in `5th-echelon-known-friends.json` next to `uplay.toml`.
- Anyone with a friend's name who isn't that friend (another identity, or none) shows as **Not your friend Kiwi from play.example.org**: in search results, friend requests, invites and invite notifications.
- Find players shows each player's identity (e.g. `ID K7QF-2M9D`, "name reserved"), or "no identity".

### What the coordinator knows

- The member servers: each server's id, its secret (only a hash is kept), and its directory entry.
- For each linked account: your identity's public key, the server, and your name there.
- Which identity holds each name.
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

Every member server appears in the coordinator's **server directory**, unless `listed = false`, with its name, region, address and players online. Launchers ping each server through its NAT helper, the path game traffic takes, and rank them: the lowest ping first, and among servers within 15 ms of it, the busiest.

You can also type a coordinator's address where you'd type a server's (the community's is `play.scbl.jdevwebb.net`): the launcher uses its directory, pings every server, and sets you up on the best one.

The first server you join that uses a coordinator brings its directory: the launcher uses it from then on. A directory you set yourself, in **Settings › Identity and friends**, is never replaced.

- **The join form** pings the directory's servers as soon as it opens, and preselects the best (unless you've typed a server).
- **Once you've joined,** the server card lists the network's servers with your ping to each. **Switch** sets you up on another in one click: your identity signs you in there, with the same name, and your friends follow.

- Only `https://` directories are used.
- A directory entry must be a public host name or address. Entries for private addresses (your own network) are skipped, as are more than 200 entries.
- Each server's host is shown next to its name, so you can see where Choose takes you.

### Running a coordinator

**With the Linux installer**, on the same VPS as a server:

```sh
sudo bash install-server.sh --domain blacklist.example.com --coordinator-domain coordinator.example.com
```

- It installs `coordinator` as a second service (`5th-echelon-coordinator`), behind Caddy on HTTPS. Caddy gets the certificate, so TCP 443 must be open and the domain's A record must point at the VPS.
- The server on that VPS joins it.
- The join token for other servers is in `/var/lib/5th-echelon/coordinator/join-token.txt` (the summary says where; it doesn't print it). Copy it to the other server as a file, then:
  ```sh
  sudo bash install-server.sh --domain other.example.com --coordinator https://coordinator.example.com --join-token-file token.txt --region Sydney
  ```

**By hand:**
1. Run `coordinator --listen 127.0.0.1:8700 --data /var/lib/coordinator` behind any reverse proxy with HTTPS.
2. The join token is in `join-token.txt` in the data folder. `coordinator --data <folder> new-token` makes a new one (restart the coordinator afterwards); servers that already joined keep working.
3. `coordinator --data <folder> remove-server <id>` removes a member server, its links, and the names only it used. Rotate the token too if it could join again.

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
| `POST /v1/names/claim` `{name, global_id, time, signature}` | a member | reserves a name for a player (their link signature for that server); `409` if someone else has it |
| `GET /v1/names/<name>` | a member | whether a name is reserved (`{claimed}`) |
| `GET /v1/info` | anyone | name, version, number of servers |

## Testing

- **`build/build.sh bots`:**
  - on a default server: the `friends`, `block`, `invite-queue`, `identity-login` and `rename` scenarios;
  - on a server in `mutual` mode: `friends-mutual`.
- **`build/build.sh federation-test`:** a coordinator and two servers in containers.
  - Both servers must appear in the directory.
  - A friendship made on the first server must reach the second.
  - A name reserved on the first is refused to anyone else on the second.
  - An older account with that name is flagged, and renaming clears the flag.
  - A block on the second must reach the first.
