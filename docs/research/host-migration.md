# Host migration: party yes, match lobby no - and why

Research, 19 October 2026, on the DX11 build (`Blacklist_DX11_game.exe`, addresses as loaded). Markers:
**C** read from the code (Ghidra output and disassembly), **R** seen live, **I** inferred. No decompiled code here,
only what it does, in our words.

The question: when the host of a party leaves, the party carries on under a new host. When the host of a
match lobby (private or public) leaves, the lobby is dissolved and everybody is out. Is that the server, Storm,
or the game's own choice, and what would it take for a match lobby to carry on?

Short answer: it is the game's own choice, made from one flag per session object. The migration machinery
itself (election, server calls, Storm object hand-over, the lobby's own "migrating" state) is generic and
already knows about match lobbies. `hooks/src/migrate_lobby.rs` sets that flag for match sessions too. It was
written as an opt-in (`MigrateMatchLobby` in `uplay.toml`, and `MigrateRunningMatch` for §5.2), the names used
below.

> **Since the merge (2026-10-24) the hooks are not configurable any more: they are always on.** The `uplay.toml`
> keys `MigrateMatchLobby`, `MigrateRunningMatch` and `PartyFollowsInvite` are gone (a file that still has them is
> read as before, the keys are ignored); `PartyStaysAfterMatch` (H3) and `TraceMemberLeave` (`member_trace.rs`) were
> removed. What that means for players with an older client: `docs/architecture/host-and-party-migration.md` §11,
> "Mixed clients".

## 1. The session object and its fields (C)

Both the party (`NetPartySession`) and the match (`NetGameSession`) derive from `NetSessionBase`
(constructor `0x00869d90`). The fields that matter:

| Offset | What it is |
|---|---|
| `+0x3fc` | the session's `NetGameSessionDescriptor` (Storm `SessionDescriptor` plus a `u32` key) |
| `+0x550` | the Storm session handle (`-1`: none) |
| `+0x554` | the Storm session-info block; its host test is `0x01f92c70` (host id equals own id) |
| `+0x55c` | this peer's own id, compared with the elected host |
| `+0x5a9` | **may migrate**: written in `0x0086a1e0` as "kind is 1" |
| `+0x5e1` | report a lost host as an error (`LostConnectionToHost`) |
| `+0x5f0` | the kind: **0 = match** (`NetGameSession`), **1 = party** (`NetPartySession`); also the index of the session's online service (`NetOnlineSessionServiceRdv`, one per kind) |
| `+0x610` | the `MigrationInfo` object (0xe8 bytes, allocated in the constructor, so never null while the session lives); its `+0` is the kind, `+4` the migration step (0 = none), `+0x20` the elected host |
| `+0x618` | the pending callback of an asynchronous migration step |

So "`+0x610` missing" never happens in practice; the one flag that decides is `+0x5a9`, and it is set from the
kind alone. The only writer of `+0x5a9` in the networking code is `0x0086a1e0` (at `0x0086a319..0x0086a33c`:
compare the kind with 1, `sete`, store) (C).

## 2. Where the flag is read

1. **The host leaving** (`nsGameSession::StateLeavingNetSession` on-enter, `0x007bd700`): on a client it always
   sends a plain Storm leave ("Shutdown called on client."). On the host it sends **Dissolve** ("Shutdown and
   Dissolve called on host") unless `+0x610` is set **and** `+0x5a9` is set; then a plain leave ("Shutdown called
   on host."), which lets the others elect a new host (C). R (2026-10-12): a private match host's game dissolved
   the match, its party it only left, and the party migrated.
2. **The host leaving, server side** (`nsGameSession::StateLeavingOnlineSession` on-enter, `0x008363f0`): passes
   `+0x5a9` as the flag of the online service's Exit goal (`NetOnlineSessionServiceRdv::vf13`, `eGoal_Exit`). The
   service's idle state (`nsNetSessionRdv::StateIdle`, result handler `0x0085b3f0`) then goes to
   - `StateLeave` (`LeaveSession`) when this game didn't create the server session (the Rdv flag `+0x42c` clear),
   - `StateAbandon` (`AbandonSession`) when it did and the flag is set: the session stays for the others,
   - `StateDelete` (`DeleteSession`) when it did and the flag is clear (C).
   That is why a party host abandons and a match host deletes.
3. **A member noticing the host is gone** (`0x0084d220`, called from `NetSessionBase::vf02` for a Storm session
   event): while the session is in `nsGameSession::StateInSession`, it starts a migration (`MigrationInfo`: kind,
   step 1, the event's peer) when `+0x5a9` is set; otherwise, with `+0x5e1`, it raises `LostConnectionToHost` and
   the member leaves (C). The flag must therefore be set on **every** member's game, not only the host's.
4. **The engine's own host-loss check** (`0x00779580`, "may migrate": `+0x610` set and `+0x5a9`), used by
   `UGameEngine::vf160` and `0x00646a50` when the level's net connection to the host breaks during a match: no
   migration means `LostConnectionToHost` (C). This is the in-match path, not the lobby's.

## 3. A successful party migration, step by step

The migration is driven every frame by `0x0089e650` on each member, on `MigrationInfo+4` (C unless marked):

| Step | What happens |
|---|---|
| 1 | the session's listeners are told (`0x008546d0`, `0x007d8200`), Storm is told this peer accepts host candidacy (`0x01f9f7e0`); step 2 |
| 2 | wait until Storm elected a host (`MigrationInfo+0x2c`); the elected id goes to `+0x20`; step 3 |
| 3 | `eGoal_PlatformMigration` on the online service (`NetOnlineSessionServiceRdv::vf21`), with "am I the new host"; callback `0x00836640`; step 4 |
| 4 | in the callback: on failure "Platform migration failed"; on the **new host** `eGoal_ServerMigration` (`vf22`, result event 11), callback `0x00821980`; step 5 |
| 5 | new host: the callback stores the new server session id in the descriptor (`+0x428`, read from the service) or fails with "Server migration failed"; others: wait for the new host to be reachable and for its session id; step 6 |
| 6 | Storm gets the updated descriptor (`0x01f9fa20`); step 7 (or 8) |
| 7 | wait for the new host, Storm's host set (`0x01f92c30`), the session's own vf; step 8 |
| 8, 9 | `NetServiceManager::vf52` hands every Storm service object (the lobby service with its ten agents, the player services) to the new master; `vf53` polls until each object's master is the new host (I: the lobby's settings survive this way) |
| 10, 11 | Storm restarts the session (`0x01f9f8e0`); step 12, done |
| 13 | failed (`0xd`) |

On the server this is (C for the Rdv states, R 2026-10-12 for the calls): `eGoal_ServerMigration` makes the new
host's `nsNetSessionRdv` go `StateRegisterURLs` (RegisterURLs) → `StateMigrateHost` (**SplitSession** of the old
session) → `StateMigrateHostAddParticipant` (**AddParticipants** of itself). If its Rdv flag `+0x42c` says it
already owns a server session (it created one, or it migrated before), it goes `StateRegisterURLs` →
**`StateAbandon`** (AbandonSession) → `StateMigrateHost` → `StateMigrateHostAddParticipant`: the "abandon
variant" (C, `0x007a4c40`, `0x007a4e60`). The others' games make no server call of their own for the
migration (R 2026-10-12: only the remaining member's calls were seen). Storm itself sends nothing to the
server; it elects the new host among the peers (see 5ee-research `protocol/session.md` §8).

Also gated by the migration step (C): joins are refused with "Still migrating" while `MigrationInfo+4` is not 0
(`0x00859cf0`); a migration error on a **match** session (kind 0) ends in `LostConnectionToHost`, on a party it
is retried ("migration error", `0x0089e650`).

## 4. What a match lobby would need beyond the flag

- **The lobby state machine already has a migration state** (C): `nsOnlineLobby::StateMigrating` (state id
  `0x0338db6c`). The host-side lobby states `StateWaitingForPlayers`, `StateBalancingTeams`, `StateCountdown`
  and `StateIntermission` register it as a target, and a member in `StateWaitingForMatch` goes there when the
  synchronised lobby state is 10. On entry it publishes lobby state 10 (`0x007f1ec0(10, 0)`); each frame it
  waits while its session (`lobby+0x40c`) is migrating, and then resumes with `eGoal_Run`; its result handler
  goes back to `StateWaitingForPlayers`, or to `StateIntermission` when the lobby was in state 9. So the match
  lobby was built to survive a migration; only the session's flag keeps it from ever starting.
- **Lobby objects**: the lobby's settings live in Storm objects of the lobby service (ten agents), and step 8/9
  of the migration hands every service object to the new master (C for the hand-over, I that the lobby
  service is among them: `NetServiceManager::vf43` finds the lobby group in the same list).
- **Not handled** (C): a migration during `nsOnlineLobby::StateLobbyTransitionSync` (the game logs "The session
  is in a state of migration but we dont know about it yet and dont handle it").
- **Open** (I): the first of the descriptor's three bits (`NetSessionBase+0x478`, descriptor `+0x7c`) is set to
  1 for a match and 0 for a party when hosting (`nsGameSession::StateAttemptingToHost` on-enter, `0x0089da60`).
  Its meaning in Storm is not known. If Storm refuses to elect a host for a session with it set, the members
  will hang in step 2 and then fail (step 13, `LostConnectionToHost`); the hook leaves it alone, and the live
  test is what tells.
- **In a running match** the engine's own check (§2.4) decides; the hook keeps it as it was, so a host leaving
  a running match still ends it for everyone.

## 5. The hook (`MigrateMatchLobby`, now always on)

Two patches (plus the cave), both sites found by their exact bytes in `.text` and applied only together, or not at all (no cave found or a read-back mismatch: nothing is applied):

1. In `0x0086a1e0`, the `sete dl` / `push ebx` / `lea ecx,[ebp-0x1e0]` (10 bytes) become `call cave` / `push ebx`
   / 4x `nop`. The cave (16 bytes of `0xCC` padding between functions, the first run found in `.text` at run time,
   written before the call) computes the flag as **(kind == 1) or (not private)**: `sete dl; xor al,1; or dl,al;
   xor al,1; lea ecx,[ebp-0x1e0]; ret` (`al` is the privacy byte `[edi+0x2a0]` just stored to `+0x5a8`). A party
   and a public match may migrate; a private match session gets 0, as the original gave it. This covers §2.1 (plain
   leave instead of dissolve), §2.2 (AbandonSession instead of DeleteSession) and §2.3 (members migrate instead of
   leaving), for non-private sessions. Measured with the earlier flat `mov dl,1`: a party member's private match
   session had `+0x5a8=1` and `+0x5a9=1` and never left after the host went, while the leader's was 0.
2. `0x00779580` keeps its old answer, now read from the kind: "may migrate" only for a party, so the in-match path
   (§2.4) is unchanged. With `MigrateRunningMatch` (needs `MigrateMatchLobby`; both now always on) it answers the
   flag for every session that isn't private (`+0x5a8 == 0`) instead, so a public match whose host drops falls back
   into the lobby and migrates (`docs/architecture/running-match-host-loss.md`). The "second writer" of the flag at
   `0x0085a626` is a self-copy and changes nothing; why a private session doesn't migrate today is not settled,
   which is why the check gates on the privacy byte itself.

Every real player's game needs it (now: a client with this release's hooks); a member without it still leaves when
the host goes (§2.3), and a host without it still dissolves (§2.1). The headless bot is not a game; whether it
follows a migrated host is its own code.

## 6. The server's part

`dedicated_server/src/game_session.rs`, `split_session`: a SplitSession by someone who isn't the session's
host, of a session that still has others in it, is a host migration: the new session takes the others along
(participants and guests). Three cases:

- the splitter is still in it (the plain migration),
- the splitter abandoned it just before (the abandon variant, §3), and its host is gone from it,
- the host is still in it and online: nothing tells a migration from a guest splitting off on its own, so only
  the splitter moves, and the others follow once the host leaves the session or signs out within a minute (or
  when their own game splits the same session).

A party that is invited into someone else's private match is a different case (the leader's secede, not a
migration): see `docs/architecture/host-and-party-migration.md` (hook `party_follow`, formerly
`PartyFollowsInvite`, server `escorts.rs`).
