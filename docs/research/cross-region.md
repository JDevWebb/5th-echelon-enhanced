# Cross-region play: players on different servers in one match

Research, 2 October 2026: can a player on the EU server and a friend on the Oceania server play together, by the servers passing data between them? Paths are relative to the repository root; RE notes are in the separate blacklist-re notes (`findings.md`, `nat/peers.md`, `nat/traversal.md`).

## Recommendation

1. **First: cross-server invites that end in an assisted switch.** The coordinator forwards an invite between identities on different servers. The friend sees "Kiwi invites you on EU", and the launcher moves them to that server and restarts the game. Both players are then on one server, so the invite, join and NAT code that already works is used as is. No game RE needed. Days of work, almost no risk.
2. **Then, for the community network: one matchmaking server, with the regional servers as relays and NAT helpers only.** Matches already run peer to peer; the regional server only matters for the relay and the NAT helper, and matchmaking calls don't need low latency. With one session store, the per-server ids never meet. The work is mostly separating the NAT helper and relay from the matchmaking server. Both servers are ours, so concentrating matchmaking is fine.
3. **Last, and only for networks with several operators: session federation.** Servers mirror each other's sessions. It needs ids that never collide between servers (each server allocating from its own range), and RE showing whether ids travel peer to peer or inside the 496-byte session blob, which no server-side translation could fix. Highest risk: it doubles the most fragile flow in the project (the private-match invite) across two servers that sync with a delay.

Separately worth one experiment: **switching servers without restarting the game** (see "Follow the friend").

## What ties a joining player to the host's server today

Everything is keyed by ids local to one server, and all of them collide between servers:
- a **pid** is `users.id`, an SQLite autoincrement;
- an **RVCID** comes from a per-process counter that starts at `0x3AAA_AAAA` on every server (`quazal/src/prudp.rs`, `next_conn_id`);
- **session ids** are autoincrement too.

**Host enters multiplayer**
- `RegisterURLs` (`game_session.rs`): the game registers `prudp:/address=A;port=13000;RVCID=…;type=2`. The server corrects the address with the NAT helper's (`nat_helper::advertised_for`) and `only_reachable_addresses`, which allows only the observed address or this server's relay. Stored per pid.
- `CreateSession` for the anteroom (113=1), and for a private match room (113=0, `3=>0;4=>8`), then `AddParticipants` for the host.
- `UPLAY_USER_SetGameSession` (`hooks/src/uplay_r1_loader/user.rs`) sends `Friends.SetSession` (`api.rs`), which stores the session id, invite_only and a 496-byte opaque blob per pid.

**Invite**
- `UPLAY_FRIENDS_InviteToGame` → `Friends.Invite` (`api.rs`). The receiver's account id resolves to a **local** pid; relation and mode checks apply; the invite is bound to the host's current room, preferring the private match room.
- The game's own `SendInvitation`/`AcceptInvitation` (`game_session.rs`) is a second path, also keyed by pids.

**Delivery and accept**
- The guest's DLL polls `Misc.Event` and hands the game `FriendsGameInviteAccepted(username)` through `UPLAY_GetNextEvent`. The event carries only the inviter's name.
- The game then finds the inviter in `UPLAY_FRIENDS_GetFriendList`. The entry must carry the inviter's **pid**, **session_id** and **496-byte session_data**; without the blob the game opens a session of its own.

**Search**
- `SearchSessionsWithParticipants([host pid])` (`game_session.rs`). Before answering, the server **adds the caller to the invited room**; without that the game never starts the join. It answers with the invited room plus the host's anteroom (`rooms_for_friend_search`). Each result carries `host_pid`, `host_urls` (with the host's RVCID) and participant pids.
- Or `GameSessionEx.SearchSessions` query 8 (`game_session_ex.rs`), which answers with the bound room and opens a private seat.

**Join**
- Public room: `AddParticipants` for itself, then `JoinSession`, which consumes the invite.
- Private match: `AbandonSession` and `SplitSession` on its own lobby, then `AddParticipants` for itself, and no `JoinSession` (`StateJoin::vf08` skips it).
- **Server→client push:** NotificationEvent `ui_type 7003`, with `ui_param_1` the **guest's pid** and `ui_param_2` the **session id**, sent through `client_by_user_id`. The game checks both against its own player and session objects (`vf28` 0x007BDFB0, `vf01` 0x0077DA30). Without it, `StateJoin::vf00` waits forever.
- A host taking a party into a match adds the guests' pids itself.

**Peer to peer**
- The game turns participants' station URLs into Storm PeerDescriptors (public address, local address, RVCID).
- `RequestProbeInitiationExt`: the server finds the target by **RVCID in its own registry** (`nat_traversal.rs`), checks they share a session, and pushes `InitiateProbe`.
- **Relay:** the hook wraps only for *its own* server's relay range (`hooks/src/hooks/nat.rs`; the helper host is the game's server), and the helper routes only between players registered with **it** (`nat_helper.rs`).
- **Address echo** (`main.rs`, `handle_user_packet`) answers from this server's NAT table.

So the server holds sessions, participants, station URLs, both kinds of invite, advertised sessions and blobs, the client registry (pid and RVCID to connection) and the NAT/relay table. It pushes 7003 and `InitiateProbe`. Every one of these uses its own pids, RVCIDs and session ids.

## Options

### a. Follow the friend (switch servers)

What exists: the coordinator's "elsewhere" data, and the launcher's "Switch to this server" (disabled while the game runs), which finds the account by identity.

**With a restart (recommended first)**
- A coordinator route forwards an invite notice from one identity to another: friends only, never across a block, rate-limited like `rate_limit::invites`.
- The guest's overlay shows "Kiwi invites you on EU (restarts the game)".
- The launcher waits for the game to close, switches (key login, `uplay.toml`) and starts it again.
- On arrival, the host invites again, or the coordinator has the host's server re-issue the invite for the guest's linked account there.
- Small, low risk; the only cost is a restart of about a minute.

**Without a restart: unknown, one experiment settles it.**
- The server address is patched once, when the DLL starts (`hooks/src/lib.rs`, `patch_url`: rodata, or `g_cfg_client+0x24` on the heap). Credentials come from `UPLAY_USER_GetUsername/GetPassword`, which the DLL controls.
- If the game fetches its online config again, and asks for credentials again, when it re-enters online after losing the connection, the DLL could switch it: repatch the host name, swap the credentials, the API base and the NAT helper, then force a disconnect.
- **Experiment:** at the multiplayer menu, block UDP to the server or stop a local server, and check in `bl-tracing.log` whether re-entering online calls GetOnlineConfig again or only LoginEx to the cached address. If only LoginEx, redirect the auth and secure addresses in the `sendto`/`recvfrom` hooks instead, the technique `nat.rs` already uses.

### b. Session federation

Servers mirror sessions for linked identities, through the coordinator or directly. What would have to be carried across:

| Item | Today | Federated |
|---|---|---|
| Friend list entry (pid, session id, 496-byte blob) | local tables | a shadow account for the foreign host, a mirrored session id, the host's blob |
| Invite binding | `api.rs` | forwarded, bound to the mirror |
| Search / query 8 | local, adds the caller first | answered from the mirror; the add has to happen on the host's server **before** the reply |
| AddParticipants / JoinSession / Split / Leave | local | forwarded to the host's server as the guest's shadow there |
| 7003 push | `client_by_user_id` | host's server → guest's server, translating the pid and session id |
| RequestProbeInitiationExt → InitiateProbe | `client_by_connection_id` | routed by RVCID to the owning server, both ways |
| Relay | one helper | relay to relay, or the hook registers with foreign relays |
| Party route (host adds guest pids) | local | the host's server must know a foreign pid |

**Can translation tables fix the id clashes?** Only for fields the servers see. The game passes ids around itself: a Storm PeerDescriptor carries the RVCID, Storm join messages may carry pids, and the blob may hold the session id or host pid. If ids travel peer to peer, the host's game would send the guest's *home* pid to the host's server, where it means a different player.

The robust fix is ids that never collide: the coordinator gives each server a range (for example `pid = server_index << 24 | n`), and connection and session ids use the same range. Existing accounts would need renumbering (tables reference `users(id)`), so that means a careful migration; tickets and tokens expire anyway. With non-colliding ids a shadow account can reuse the same pid, and only membership needs mirroring.

Risks: sync delay against the game's tight, order-sensitive flow (add before search, 7003 at the right moment), session state owned twice, and unknown id fields in Storm and the blob. Large (weeks), and every failure is a silent hang in the game.

### c. One matchmaking server, regional relays

What separates the current code from this:
- **The NAT helper runs inside the game server.** `nat_helper::start`, the `TABLE` static, `advertised_for` in RegisterURLs and the echo all read it directly. Needed: a helper-only mode, and a way for the matchmaking server to learn each player's helper address (reported by the helpers over signed HTTPS, or trusted from what the hook makes the game advertise).
- **Only one relay's addresses are allowed.** `only_reachable_addresses` and the probe check accept a single `relay_ip`; they'd take the network's list.
- **The DLL's helper is always the game's server** (`nat.rs`). Needed: a separate NAT server setting, picked by the launcher's existing ping ranking, which already pings each server's NAT helper.
- **The hook wraps only for its own relay.** Needed: registering with any network relay on demand, with network-wide or coordinator-signed relay tickets instead of the per-server `ticket_for` HMAC, or relay-to-relay forwarding.
- **The directory** already publishes each server's NAT port; it would also need the relay port range.

Costs: matchmaking and menu traffic would cross about 300 ms round trips for Oceania players, which should be fine for RMC but needs measuring. One point of failure. Quick Match would mix regions unless the server prefers sessions in the player's region (tagged by their relay). Accounts move over on their own: identities and network-wide names already let a switch find or create the account.

Medium effort, low to medium risk, and nothing in the invite and join flow changes.

### d. Other ideas

- **Several front ends, one shared session store:** regional servers keep their logins, while sessions, participants and URLs live in one shared database, and pushes go to whichever front end holds the connection. Option c without moving logins, but it means moving storage off local SQLite. Too big for now.
- **Party leader's server:** when friends group up, everyone uses the leader's server. That's option a, for a whole group.

## Plan

**M1: forwarded invites with an assisted switch**
- Coordinator route `POST /v1/invites` (sender identity, receiver identity), pulled alongside `relations`.
- An overlay notice for the guest; the launcher switches and restarts the game.
- On arrival, the guest's server lets the pending invite through, or the host invites again.
- Test with `build/build.sh federation-test`: bot A on server 1 invites friend B on server 2; B's poll names the host's server; B signs in on server 1 with its identity; then the existing `lobby-invite` and `private-match-invite` steps pass on server 1. Then two PCs, one on each server.

**M2: the switching-without-restart experiment.** If it works, M1's switch happens inside the game.

**M3: separate the NAT helper and relay (start of c)**
- A helper-only mode, the DLL's NAT server setting, a relay address list in the directory and in `only_reachable_addresses`, network-wide relay tickets, and registering with foreign relays in the hook.
- Testbot: `nat_register` against two helpers. A relayed packet reaches a player on the other relay; an unregistered sender is dropped; per-player limits hold; a federation relay address in RegisterURLs is kept, an unknown public address rewritten.

**M4: latency check, then consolidate**
- Run `build.sh bots` with `tc netem delay 150ms` both ways, and watch for RMC timeouts and retransmits.
- Then point the community address at one matchmaking server, with Oceania as relay and helper only.
- Test a private match between an Oceania and an EU player, each through their own relay.

**M5 (optional): session federation.** Only after the RE below, and after ids from per-server ranges have shipped.

**RE still needed**
1. What's in the 496-byte blob? Compare stored `advertised_sessions` blobs (from a database copy) with known session ids, pids and IPs. Needed for b.
2. The Storm join handshake: which player ids travel peer to peer, and does the host's game call the server with the joiner's pid (AddParticipants on the party route, `ReadStatsByPlayers`, `GetClanInfoByPid`)? Needed for b.
3. Does a failed Quazal NAT probe block a join? `traversal.md` §4 never found who reads +0x4c. Experiment: drop `InitiateProbe` on a test server and join with two directly reachable players.
4. Does the game ask for tickets for other players (`RequestTicket` with another player as target)? `ticket.rs` doesn't log successful requests; add a log line on a test server. Another server couldn't seal a ticket for a foreign pid.
5. What does the game do when it loses the online connection, and does it fetch its config again? Needed for switching without a restart.

## Security

- **Member servers acting for players they don't own (b).** The coordinator already accepts statements only about identities linked on the calling server (`set_online`, `require_linked`). Federated sessions should follow the same rule:
  - only identities online on the server publishing them;
  - mirrored only on demand, for a friend of the host with a pending invite, never across a block;
  - forwarded pushes (7003, InitiateProbe) only for sessions the target actually joined, with rate limits.
- **Address abuse.** A dishonest member could put any address into station URLs, making other players' games send traffic there (reflection). Foreign URLs should be limited to network relay addresses or addresses a helper signed; `only_reachable_addresses` relies on this server's own observation, which another server can't check.
- **Relay abuse (b and c).** A network-wide relay ticket must not let one compromised member register any name on every relay. Use per-player tickets signed by the player's home server, checked against keys from the coordinator. Keep today's per-player limits (`relay_kbps_per_player`, 600 packets a second), cap foreign registrations per relay, forward only between two registered players, and authenticate relay-to-relay links both ways.
- **c puts trust in one operator.** Fine for the community network, where both servers are ours. It doesn't suit networks run by different people, which is the only real reason to build b.

## Unknowns

- PC almost certainly uses Quazal NAT rather than Storm punch (`traversal.md`).
- Whether probes are needed for a join, what the blob and Storm join messages contain, and whether the game re-fetches its config after a disconnect are all unknown; each has an experiment above.
- The recommendation doesn't depend on any of them: M1, M3 and M4 work whatever they turn out to be.
