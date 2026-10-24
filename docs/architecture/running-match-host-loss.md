# Host loss in a running public match: fall back into the migrated lobby

Status: analysis, 2026-10-23, DX11 build (`Blacklist_DX11_game.exe`, addresses as loaded, image base 0x400000),
branch `fix/host-migration-client`. Case 5 of `host-and-party-migration.md` §9. Markers as there: **C** read from
the decompiled text (`~/Games/scbl/ghidra-jobs/ergebnisse/dx11-alles.c`, `L<n>`) or `objdump`, **R** seen live,
**I** inferred, **V** guess. No live test was run for this document; every R is quoted from earlier logs or docs.

Goal (maintainer, 2026-10-23): in a running **public** match whose host drops (crash or kill; there is no regular
leave inside a match), the remaining players fall back into the lobby, and the lobby carries on as in a lobby
migration (new host, same lobby, next match starts from it as usual). A **private** match dissolves as today. The
running round is not migrated.

Sections: 1 summary · 2 findings (F1–F14) · 3 design · 4 tasks · 5 test rig · 6 open points.

> **Since the merge (2026-10-24) the hooks are not configurable any more: they are always on.** The `uplay.toml`
> keys `MigrateMatchLobby`, `MigrateRunningMatch` and `PartyFollowsInvite` are gone (a file that still has them is
> read as before, the keys are ignored); `PartyStaysAfterMatch` (H3) and `TraceMemberLeave` (`member_trace.rs`) were
> removed. The design below puts the running-match fallback behind `MigrateRunningMatch` (default off);
> `hooks::init` now always calls `migrate_match_lobbies(true)`, so the public variant of the check
> (`CHECK_PATCH_PUBLIC`) is the one applied. A client without this release's hooks still behaves as in the original
> (F8), see `host-and-party-migration.md` §11, "Mixed clients".

---

## 1. Summary

**Feasible: yes, probably with little code, because the game already contains the wanted behaviour and only two
of our own decisions switch it off.** The engine has a complete, never-enabled "host migration during a match" path:

1. A member's game notices the host is gone (`0x0084d220`) and starts the session migration instead of raising
   `LostConnectionToHost` when the session's "may migrate" flag `+0x5a9` is set. The `MigrateMatchLobby` hook
   already sets that flag for match sessions (public and private alike).
2. Step 1 of the migration driver (`0x0089e650`) sends the session-action event **3 ("migration started")** to the
   session's listeners. The lobby controller's listener (`0x007a72f0`) answers it, when the lobby FSM is in
   `StateMatch` or `StateLoadingMatch`, with the online error **`HostMigrationDisruption`** (kind 0xb, category 0).
3. A category-0 error makes `nsOnlineLobby::StateMatch` end the match (`EndMatch`) and go to **`StateLoadingLobby`**
   (category ≥ 1 would go to `StateError` → `Logout`). `StateLoadingLobby::vf00` explicitly **waits while the
   match session is migrating** and then follows the (new) host's published lobby state (2/4/5/9) into
   `WaitingForMatch` and so on. That is "fall back into the lobby, then as for a lobby migration".
4. What blocks it today is the **engine's host-loss check `0x00779580`** (`+0x610 && +0x5a9`), read by the level
   teardown (`0x00646a50`, `UGameEngine::vf160` 0x0065efc0) when the client's connection to the host breaks: the
   hook (`hooks/src/migrate_lobby.rs`, `CHECK_PATCH`) rewrites it to "parties only", so a match session answers
   "no" and `LostConnectionToHost` (kind 4, **category 1**) is raised. Category 1 outranks category 0 in the
   error slot (`0x007a56e0`), so `HostMigrationDisruption` is overridden, the lobby goes to `StateError`, logs out,
   and everyone lands in their party lobby (today's R).

So the core change is one byte-level decision: let `0x00779580` answer "may migrate" for a **public** match
(`+0x5a8 == 0`) and keep "no" for a private one. The flag setter patch stays. The server needs nothing new for
the session (SplitSession by the new host = takeover, 12 s silence rule, 7001); it does not know lobby states and
does not need to. What is **not** proven statically and needs one live experiment first (§5, §6): (a) whether the
second engine site `0x0064cb60` (the map load itself) still raises `LostConnectionToHost` while the category-0
error is pending, (b) whether the new host's `StateLoadingLobby` issues the lobby level load when it becomes host
only after it entered that state, (c) how the member's world is torn down and the lobby map loaded without a host.
If the experiment shows a hang, the fallback is a server-side re-assembly (§3.4) on top of today's behaviour.

Correction to earlier research: the "second writer of `+0x5a9` at `0x0085a626` that clears it for a private
session" is a self-copy (`mov al,[edi+0x5a9]; mov [edi+0x5a9],al` in the join check `0x00859cf0`, C). It changes
nothing. Why private matches do not migrate today is therefore not settled by the code read here (§6 E6); the
design below gates on `+0x5a8` itself and does not rely on that writer.

---

## 2. Findings

### 2.1 Objects and fields (C)

| Object | Fields used below |
|---|---|
| `NetCore` `[0x3383df4]` | `+0x5cc` NetServiceManager (`vf +0xac` lobby service object, `vf +0xb4(storm id)` a peer's client object); `+0x5d0` match session; `+0x5d4` party session |
| `NetOnline` `[0x3383df8]` | `+0xc` error slot object: `+0x10` = pending `NetOnlineError*` (null = none); `+0x154` match-end reason for tracking (3 = error); `+0x48` lobby controller; `+0xb8/+0xc0` peer client list (`[rec]+0xac` a peer's load state) |
| `NetOnlineError` (heap, 0x14 bytes) | vtable `+0` clone, **`+4` kind id**, `+8` name; object `+4` **category**, `+8` severity (both copied from the 3-dword descriptor the raiser builds: `{vtable, category, severity}`) |
| `NetSessionBase` (match or party) | `+0x10` FSM state (`vf +0x30` → state id; `[0x338d7a4]` = `nsGameSession::StateInSession`); `+0x558` host's storm id; `+0x55c` own storm id; `+0x584/+0x588` peer list; `+0x58c` connected peers; `+0x594` settings (`+0x10` lobby kind, `+0x1c` min players, `+0x20` max players, `+0x28` mode); **`+0x5a8` private**; **`+0x5a9` may migrate**; `+0x5e0` "lost" latch; `+0x5e1` report lost host as error; `+0x5f0` kind (0 match, 1 party); `+0x610` MigrationInfo (`+4` step, 0 = none) |
| lobby controller `NetOnlineLobbyController` (`[NetOnline+0x48]`) | `+0x10` FSM state; `+0x3c` goal queue; **`+0x40c` = the match session** (set in `FUN_00875490` from `NetCore+0x5d0`, L907202; the earlier guess "party session" is wrong); `+0x404` `Listener<NetEventSessionAction>` (its callback is `0x007a72f0`); `+0x414` lobby state when no session; `+0x42c` settings when no session; `+0x484` "intermission" latch |
| lobby service object (`NetServiceManager vf+0xac`) | `+0x84` the synchronised lobby state (field 0 of the lobby-state memento, lobby.md §3) |
| peer client object (`vf+0xb4(storm id)`) | `+0x64` own load stage (5 match, 6 loading lobby, 7 lobby loaded); `+0x78/+0x7b/+0x7c` sync flags for the level barriers |

Lobby state ids (C, `vf12` of each state): Idle `0x0338dafc`, Login `db04`, Logout `db0c`, LobbyTransitionSync
`db14`, WaitingForMatch `db1c`, WaitingForPlayers `db24`, BalancingTeams `db2c`, Countdown `db34`, LoadingMatch
`db3c`, **Match `db44`**, MatchTransitionSync `db4c`, **LoadingLobby `db54`**, Intermission `db5c`, **Error `db64`**,
Migrating `db6c`. Synchronised lobby state numbers as in lobby.md §3 (7 match, 8 loading lobby, 10 migrating).

Online error kinds (C, `vf01` of each error class): PlatformServiceConnectionLost 1, RendezvousConnectionLost 2,
NetworkGenericOpError 3, **LostConnectionToHost 4**, LostConnectionToSession 5, IncompatableDataVersion 7,
MissingMap 8, **NotEnoughPlayers 9**, PartnerDisconnected 10, **HostMigrationDisruption 0xb**,
**JoinInProgressDisruption 0xc**.

### 2.2 Findings table

| # | Finding | Marker | Where |
|---|---|---|---|
| **F1** The error slot and its priority | `FUN_007a56e0(this = NetOnline+0xc, desc)`: sets `NetOnline+0x154 = 3`; if no error is pending, clones the descriptor into `+0x10`; if one is pending it is **replaced only when the new category is higher, or equal with a higher severity**; otherwise the new error is dropped. `LostConnectionToHost` is raised everywhere with `{category 1, severity 2}`, `HostMigrationDisruption` and `NotEnoughPlayers` with `{0, 2}` / `{0, 0}`. The only clearer is this replacement itself (`FUN_00780050`), so a pending error stays until something higher comes or the slot is reset elsewhere. `FUN_00780020(this = NetOnline+0xc, k)` = "the pending error's kind is k". | C | L688660–688700 (0x007a56e0), L645829 (0x00780020), descriptors at L418196, L690395 |
| **F2** Member notices the host is gone | `FUN_0084d220` (from `NetSessionBase::vf02`'s event switch, L905214): while `InSession` and not yet "lost" (`+0x5e0`): if `+0x610 && +0x5a9` → `MigrationInfo+0 = kind`, `+4 = 1` (step 1), remember the event's peer; **no error**. Otherwise `LostConnectionToHost` (category 1) when `+0x5e1`, and `+0x5e0 = 1`. Two sibling handlers: `FUN_00882b40` (a peer left; raises the error only when the peer is the host **and `MigrationInfo+4 == 0`**, L898189) and `FUN_007f10c0` (session state event, raises it with `+0x5e1`, no migration test, L766077). | C | L852164–852225, L898140–898220, L766040–766110 |
| **F3** The migration driver tells the lobby | `FUN_0089e650` (per frame, `MigrationInfo+4`): step 1 builds `NetEventSessionAction {session, action = 3}` (`mov ebx,3` at 0x89e83e; event at 0x89e8d4) and hands it to every listener (`FUN_007d8200`); step 12 ("done") sends action **4** (0x89ec4c–0x89ec61). A migration error on a kind-0 session ends in `LostConnectionToHost` (category 1) with `+0x5e1`, on a party it is retried. | C | objdump 0x89e83e–0x89e8e5, 0x89ec4c–0x89ec61; L922080–922130 |
| **F4** The lobby's listener raises `HostMigrationDisruption` | `FUN_007a72f0(this = lobby+0x404, ev)`: for a **match** session (`[ev.session+0x5f0] == 0`) and **action 3**: if the lobby FSM is in `StateMatch` (`0x0338db44`) or its pending state is `StateLoadingMatch` (`0x0338db3c`) → `FUN_007a56e0(HostMigrationDisruption, category 0, severity 2)`. Returns 1 for a match event, 0 for a party event. This is the only raiser of that error. | C | L690371–690400; objdump 0x7a72f0–0x7a737f (quoted in §3.2) |
| **F5** How `StateMatch` ends | `StateMatch::vf00` (0x0080ac30): once its 1 s timer ran out and the session is not migrating (`FUN_007a70d0`), with the world and game info present: `FUN_007f21c0(...)`, then **`FUN_007a7510()` = "no error pending"** (it also raises `PartnerDisconnected`/`NotEnoughPlayers` when `session+0x58c < 2`). With an error pending and the game info's `vf+0x508() == 0`: `flag = (pending category == 2) || pending kind == NotEnoughPlayers`; game info `vf+0x4f4(1, flag)` (EndMatch, I) and the state finishes with result 2 (`FUN_00775e30`). Without an error it finishes when the game info's "match over" bit (`+0x310 & 1`) is set. `StateMatch::vf04` (0x007a6f40) on result 2: **pending error with category 0 → `StateLoadingLobby`; any other category → `StateError`**; no error → `StateLoadingLobby`. `StateError::vf02` (0x0085fe00): result 1 → `StateLogout`. | C | L791552–791625, L690146–690165, L690528–690560 |
| **F6** `StateLoadingLobby` tolerates a migration | `vf08` (0x00884460): publishes lobby state 8 (host only, I), own client object `+0x64 = 6`, listeners; **only when I am the host** (`FUN_01f92c70`): the playlist rotation and the level load command (`FUN_00875a60`, `FUN_00800250`). `vf00` (0x008257a0): **returns early while the match session is migrating** (`FUN_007a70d0`); then `+0x64 = 7`; as host waits until no peer is in load stage 4..6 (`FUN_007c0a00`), as member waits for the host's published state 2/4/5/9 (`FUN_007a7620`); result 3. `vf04` (0x0085fd80): result 2 → `Intermission` if settings `+0x10 == 1`, else `WaitingForPlayers`; result 1 → `Logout`. | C | L899524–899640, L816181–816260, L869770–869800 |
| **F7** `StateMigrating` and who enters it | `FUN_007a7620` (lobby status): **10 while the match session (`NetCore+0x5d0`) is `InSession` and `MigrationInfo+4 != 0`**, else the lobby object's `+0x84`. `StateMigrating::vf08` (0x0080aed0) remembers `+0x84`, publishes 10; `vf00` (0x00874c90) waits while `lobby+0x40c` migrates, then pushes `eGoal_Run`; `vf02` (0x0085fe20): result 2 → `Intermission` if the remembered state was 9, else **`WaitingForPlayers`**; result 1 → `Logout`. Entered from a member's `WaitingForMatch::vf04` on status 10 (0x007c0600) and from the host-side states. `StateMatch`, `StateLoadingMatch`, `StateLoadingLobby` do **not** enter it; they rely on F4–F6 instead. | C | L690596–690625, L791686, L887196–887250, L869832–869850, L716320–716370 |
| **F8** The engine's host-loss check and its readers | `FUN_00779580(this = match session)` = `+0x610 != 0 && +0x5a9` (19 bytes, quoted in `migrate_lobby.rs`). Readers: `FUN_00646a50` (level teardown with net sync, 0x646bc8) and `UGameEngine::vf160` (0x661b00): on a **client** (`[world+0x440] == 2`), when the match session is `InSession` and the teardown's sync barriers failed: `if (may_migrate) → nothing; else if (pending kind == JoinInProgressDisruption 0xc) → nothing; else LostConnectionToHost {1,2}`. Not `InSession` → `LostConnectionToSession {1,2}`. Then the world is torn down regardless (`LAB_00646c43`: `FUN_005c2d80`, progress string, `FUN_005318b0`). | C | L418080–418230; objdump 0x646b8a–0x646c43, 0x661ae7–0x661b7b |
| **F9** The map load has its own raisers | `FUN_0064cb60` (3430 bytes, the fiber-switching map load): three sites after the level sync barriers 6/7, 14 and 3/10/4 (`FUN_0088d6d0`) that raise `LostConnectionToHost {1,2}` on a client when the session is `InSession` and the pending kind is **not 0xc** — **these do not consult `0x00779580`**. `FUN_0088d6d0` returns 3 (failed) at once while **any** error is pending (`[NetOnline+0x10] != 0`, L907720) and when the lobby's session is not `InSession`; otherwise it loops `FUN_0088c760` (per-peer sync flags) with a 60 s cap, after which the host kicks "Not ready quickly enough" and a member raises `LostConnectionToHost` for barrier 1. | C | L422100–422420; objdump 0x64ce66–0x64d24c (barrier ids); L907620–907900 |
| **F10** What the hook does today | `migrate_lobby.rs`: (1) the setter in `txt_eGoal_JoinSpecificSession` 0x0086a1e0 (`sete dl` → `mov dl,1`): **every** session gets `+0x5a9 = 1`, the host's own too (R: the host of a public lobby leaves with a plain leave and the lobby migrates); (2) `CHECK_PATCH` rewrites `0x00779580` to `+0x5a9 && kind == 1`. So in a running public match: F2 starts the migration (no error from `0x0084d220`, none from `0x00882b40` either because `MigrationInfo+4 != 0`), F3/F4 raise `HostMigrationDisruption {0,2}`, but F8 raises `LostConnectionToHost {1,2}` which **replaces** it (F1) → F5 sends the lobby to `StateError` → `Logout`; the match FSM cleans up and leaves the session; the member lands in its party lobby with "Deine Verbindung zum Host wurde getrennt". | C, R | migrate_lobby.rs; §9 case 5 of host-and-party-migration.md |
| **F11** Private flag | `+0x5a8` is written once on the join path, `FUN_0086a1e0`: `mov [esi+0x5a8],al` with `al = endpoint+0x2a0` (L878761); the host's session gets it from the custom-match goal (I: `NetCore+0x5c8`, see host-and-party-migration.md §2.1). The engine check receives the session in `ecx`, so `[ecx+0x5a8]` is readable right there. The "second writer" `0x0085a626` is `this->+0x5a9 = this->+0x5a9` inside the join check `0x00859cf0` (L864287ff): a no-op. | C | objdump 0x85a607–0x85a63e; L864287 |
| **F12** Parties in a public match | A party never enters a public lobby (lobby.md §4, R). A member's party session (kind 1) is a separate object; F2/F4 run per session and F4 ignores party events (`+0x5f0 != 0` → return 0). So a party member in a public match keeps its party session untouched by the match's migration (same as §9 case 2). | C, R | F4 |
| **F13** Server | `split_session`: a non-host's split of a live session = takeover when the host is out or offline; when the host is still "in" (a crashed host's connection is live until it expires) the split is recorded as a `Takeover` and goes through after `HOST_SILENT_AFTER` = 12 s (`resolve_silent_hosts` → `host_gone` → `move_members`, 7001 to each moved member). The server stores no lobby state; `migrate_game_session` copies the attributes. Nothing in the server depends on "match running". | C | game_session.rs L104–247, L1441–1606 |
| **F14** Min players | `FUN_007c0990` ("enough players"): counts peers with load state ∉ {0, 8} and compares with `settings+0x1c` (min players), except mode 5 (co-op, > 1). The 6 of "Min=6" is the mode's lobby setting carried in the session settings object `session+0x594` (I: from the mode's rules, no writer found in the networking code; the overlay's "Match" pane and the control channel write `+0x594+0x1c/+0x20` directly). The control channel's `players N` sets min and max together; its comment warns that lowering only the minimum crashed the host with a null call. | C, R | L716563 (0x007c0990), hooks/src/overlay.rs L1339–1378, main checkout hooks/src/control.rs L244–273 |

### 2.3 Timeline today vs. wanted (I, from F1–F10)

```
host killed
 ~10 s   Storm drops the host's connection on every member
         ├─ 0x0084d220: +0x5a9 set → MigrationInfo step 1   (no error)
         ├─ driver step 1 → action 3 → lobby listener → HostMigrationDisruption {0,2}  (F3, F4)
         └─ engine: client connection to the host is gone → level teardown / map load (F8, F9)
              today: 0x00779580 (hook: parties only) → LostConnectionToHost {1,2} replaces the {0,2}
                     → StateMatch → StateError → Logout → party lobby; the match FSM leaves the session;
                       the migration is abandoned before SplitSession (server sees only LeaveSession)
              wanted: 0x00779580 → "may migrate" for a public match → no category-1 error
                     → StateMatch ends the match, StateLoadingLobby waits for the migration
         driver steps 2–5: Storm election, PlatformMigration, ServerMigration (new host: SplitSession,
                           AddParticipants) → server: takeover now or after 12 s silence, 7001 to the rest
         steps 6–12: descriptor, object hand-over (lobby/teams/level objects), restart, action 4
 then    new host's lobby FSM: LoadingLobby (host) → WaitingForPlayers (publishes 2)
         members: LoadingLobby → see state 2 → WaitingForMatch → the lobby as after a lobby migration
```

---

## 3. Design and recommendation

### 3.1 Principle

Reuse the game's own in-match migration path (F2–F7) and remove the one override that defeats it (F8/F10). Do not
re-implement the fall-back in Rust; do not migrate the world. Gate on the privacy byte so a private match keeps
today's behaviour. Keep everything behind the existing `MigrateMatchLobby` flag plus one new opt-in
(`MigrateRunningMatch`, default off until measured), so the current lobby-only behaviour is untouched when off.

### 3.2 Hook change H5: the engine check answers "may migrate" for a public match

Replace `CHECK_PATCH` in `hooks/src/migrate_lobby.rs` (19 bytes, in place, same site found once in `.text`):

```
; original 0x00779580 (CHECK):            ; wanted (CHECK_PATCH_PUBLIC), 19 bytes:
83 b9 10 06 00 00 00  cmp [ecx+0x610],0   8a 81 a9 05 00 00     mov al,[ecx+0x5a9]   ; the flag
74 07                 je .no              80 b9 a8 05 00 00 00  cmp byte [ecx+0x5a8],0 ; private?
8a 81 a9 05 00 00     mov al,[ecx+0x5a9]  74 02                 je .ret               ; public: flag as is
c3                    ret                 32 c0                 xor al,al             ; private: no
32 c0            .no: xor al,al           c3               .ret: ret
c3                    ret                 cc                    int3 (padding)
```

Bytes: `8A 81 A9 05 00 00 80 B9 A8 05 00 00 00 74 02 32 C0 C3 CC` (6 + 7 + 2 + 2 + 1 + 1 = 19). A party session
(kind 1) is not private (`+0x5a8 = 0`, the join path copies the endpoint's privacy, F11), so it keeps "may migrate" as
in the original; a public match gets it (the setter patch made `+0x5a9 = 1`); a private match gets "no" (today's
dissolve). The `+0x610` test is dropped as the current patch already does (the object lives as long as the session).
When `MigrateRunningMatch` is off, the current `CHECK_PATCH` stays. Verify after writing by reading back, as now.

Why gate here and not in `0x0084d220`: the detector must keep starting the migration (it already does with the
setter patch), otherwise F3/F4 never fire. The engine check is the only place that turns the category-0 error into a
category-1 one (F8). Why not also patch F9's sites: they are reached only through the map load, after the match was
ended by F5; whether they fire is the main open point (E1) and is measured before anything else is written.

### 3.3 Contingencies, decided by the experiment in §5

- **C1** F9 raises `LostConnectionToHost` during the lobby map load (E1 fails with the error dialog, lobby in
  `StateError`). Fix: a detour on `FUN_007a56e0` (0x007a56e0; prologue to verify: first bytes of the function,
  `thiscall`, one stack argument, `ret 4`) that drops a `LostConnectionToHost` descriptor (`[desc] == 0x029a0364`)
  while `HostMigrationDisruption` is pending (`[[NetOnline+0x10]]` vtable `== 0x029bb144`) and the match session is a
  public one that is migrating (`[NetCore+0x5d0]`: `+0x5a8 == 0`, `[+0x610]+4 != 0`). Log one line per drop. This
  keeps the category-0 error in the slot; nothing else reads the dropped error. Risk: low (only one error kind, only
  while migrating).
- **C2** The new host's `StateLoadingLobby` never issues the lobby load because it entered `vf08` before it was host
  (F6; E2 shows members stuck at "loading lobby", state 8, no level messages). Fix: a detour on
  `nsOnlineLobby::StateLoadingLobby::vf00` (0x008257a0, `fn(this)`, `ret 0`): after the original returns, when the
  session just finished migrating (`MigrationInfo+4 == 0` now, was ≠ 0 at the previous call) and I am the host
  (`FUN_01f92c70(session+0x554)`), call the original `vf08` (0x00884460) once more so its host branch runs. Risk:
  medium (re-running the enter handler publishes state 8 again and resets `+0x34`; acceptable, verify by E2).
- **C3** The members' world is not torn down / the lobby map never loads without a host (E3). Fix candidates, in
  order: let the engine's own connection-loss travel happen (it does today, F8's teardown runs regardless of the
  check); if the engine waits on the 60 s barrier (F9), shorten it by having C1's detour also present the barrier as
  failed-but-silent. If none holds: **fallback §3.4**.
- **C4** The Storm election fails during the teardown (migration error → `LostConnectionToHost` from the driver, F3).
  Nothing cheap on the client; fallback §3.4.

### 3.4 Fallback: server-assisted re-assembly (if the engine path cannot be made to hold)

Keep today's behaviour (everyone to their party lobby) and bring them back together: when a public match's host goes
silent (`resolve_silent_hosts`) and no member split within 15 s, the server picks the first former member to open a
new public lobby of the same mode (it will, when it clicks "quick game"; or the hook does it for it) and sends the
other former members an invitation delivered as the requested join (`join_requests::Requested`, as `bring_party_back`
does). Reliable, no engine risk, but not "the same lobby": settings, teams and the map vote are lost, and each player
sees the disconnect dialog first. Implement only if §5 E1–E3 fail twice.

---

## 4. Task list (in order; each step is one file and testable alone)

1. **H5a config.** `hooks/hooks-config/src/lib.rs`: `pub migrate_running_match: bool` (`MigrateRunningMatch`, serde
   default false, `skip_serializing_if = Not::not`), doc: "in a running public match whose host drops, fall back into
   the lobby and migrate it; needs `MigrateMatchLobby`"; unit test as `migrate_match_lobby_round_trips`.
   `hooks/src/hooks.rs::init`: pass `config.migrate_running_match` into `migrate_match_lobbies(running: bool)`.
2. **H5b patch.** `hooks/src/migrate_lobby.rs`: add `CHECK_PATCH_PUBLIC` (bytes in §3.2), choose it when `running`;
   keep `CHECK_PATCH` otherwise; extend the read-back check and the info line ("engine check @ … answers for public
   matches too"); unit test that both patches are 19 bytes and that `CHECK_PATCH_PUBLIC[6..13]` is the
   `cmp byte [ecx+0x5a8],0` (`80 B9 A8 05 00 00 00`). Update the module doc (the "second writer" sentence is wrong,
   F11; the private case is now gated by `+0x5a8` in the check itself).
3. **Docs.** `docs/research/host-migration.md` §2.4/§4/§5: the engine check is per-privacy when
   `MigrateRunningMatch` is on; correct the `0x0085a626` claim. `host-and-party-migration.md` §9 case 5: point here.
4. **Live experiment E1–E3 (§5)** with the DLL from step 2 on all four clients. Decide C1–C3.
5. **C1/C2 detours** only if needed: new `hooks/src/running_match.rs` with `hook_with_name` detours (addresses
   `func_online_error_push: Some(0x7a56e0)`, `func_lobby_loading_lobby_tick: Some(0x8257a0)`,
   `func_lobby_loading_lobby_enter: Some(0x884460)` in `hooks/hooks-addresses` DX11 table, `None` for DX9), each
   verified by its prologue before hooking, all skipped (warn) when `MigrateRunningMatch` is off.
6. **Server (only if the experiment shows a need).** Candidates: (a) `resolve_silent_hosts` sweep interval – check it
   runs at least every 2 s so the takeover is not later than the game's 60 s barrier (F9); (b) log line "host of
   public match {id} silent while the match was running" using `note_match_player` knowledge, for the test's
   observation; (c) nothing else: SplitSession/AddParticipants/7001 are the lobby-migration calls already handled.
7. **Testbot:** no new scenario needed for the session part (`host-migration`, `host-migration-guest` cover a match's
   guests carrying on after a silent host). Add one only for §3.4 if that path is built.
8. **Fallback §3.4** only after E1–E3 failed twice.

---

## 5. Test rig

Smallest rig: the four real clients (host mpw + three guests; the VM client counts), the dev server
(`~/Games/scbl-enhanced/devserver/service.toml`, `dedicated_server-mig`), hook DLL with `MigrateMatchLobby = true`
and `MigrateRunningMatch = true` on **every** client (a member without it still raises the category-1 error, F8).

Steps:
1. Host opens a public lobby ("quick game", mode Blacklist). Status shows `Min=6 Max=8` (F14).
2. On the host, set the counts together: control channel `players 4` (or the overlay "Match" pane: both steppers;
   never only the minimum, F14). Expected: `OK Min und Max = 4`.
3. Three guests join by invitation (as for the lobby-migration runs). Expected: countdown (lobby state 5), teams
   (4), loading (6), match (7) – watch the lobby-state memento updates in the guests' bl-tracing.log
   (`LobbyUpdate` field 0).
4. In the match, kill the host's game process hard (the test session does it; e.g. SIGKILL of the host's game, not
   a menu leave – there is none in a match, lobby.md §4). Note the time.
5. Observe for 90 s.

Observation points:
- **Server log** (`boot.log`): within ~10–15 s after the kill: `User <member> splits session <M> as <M'> while its
  host is still in it` or `takes over session <M> as <M'> (host migration)`; then `Host <host> of session <M> has
  been silent; the takeover of it goes through` (≤ 12 s) and `Notifying <m> that session <M> moved to <M'> under
  <new host>: … ui_type: 7001`. **Absence** of `LeaveSession` from the members within those seconds (today's path).
- **Member bl-tracing.log**: the `migrate-match-lobby:` line at start with "public matches too"; Storm events of the
  migration (candidacy, election) as in `~/Games/scbl/logs/migration-20261022-*/`; the lobby-state updates after the
  return: expect 8 (loading lobby) then 2 (waiting for players) from the **new** host's id; no
  `LostConnectionToHost` dialog text.
- **Screen**: members go from the match to the loading screen and then to the lobby ("Spieler 3/4, warten auf
  Spieler"), with the same settings and teams list; the new host has the host controls. Failure signatures: the
  disconnect dialog (E1 failed, C1); a loading screen that never ends (E2/E3, C2/C3); a black screen or crash (C4).
- **After the return**: start another match from the migrated lobby (countdown with the 4 players; the min was
  set on the old host's session object – after the migration the new host's settings object carries whatever the
  lobby object hand-over brought, so re-apply `players 3` on the new host if the countdown does not start).

Regression runs with the flag on: (a) public lobby, host leaves in the lobby (case 1, must still migrate);
(b) private match, host killed in the match (case 3 hard: must still dissolve, members in their party lobby);
(c) party host leaves its party (case 6, unchanged).

---

## 6. Open points, with the smallest experiment each

- **E1** Does `0x0064cb60` raise `LostConnectionToHost` during the lobby map load while `HostMigrationDisruption`
  is pending (F9)? Smallest: §5 with step 2 only (no detours). Signature: dialog "Verbindung zum Host getrennt"
  although the server shows the SplitSession. Then C1.
- **E2** Does the new host issue the lobby level load when it became host after entering `StateLoadingLobby` (F6)?
  Smallest: same run; signature: members stuck on the loading screen, lobby state stays 8 or never arrives, server
  shows the takeover complete. Then C2.
- **E3** How is the match world torn down on a member without a host, and does the lobby map load (F8's teardown
  vs. F9's barriers, 60 s)? Smallest: same run; measure the time from the kill to the lobby screen; > 60 s means the
  barrier timed out (C3).
- **E4** Timing of the server takeover vs. the game: does the member's SplitSession arrive before the 12 s silence
  (then `waits_for_host` + sweep) or after? Read from boot.log timestamps; if the sweep is slower than 2 s, task 6a.
- **E5** Teams and settings after the return: are the `NetServiceTeams`/`NetServiceLobby` objects handed over
  intact (migration steps 8/9) so the lobby shows the same teams and map? Smallest: screenshot of the lobby after
  the return on every member; compare with the pre-match lobby.
- **E6** Why does a private match not migrate today although `+0x5a9 = 1` (F11 removed the assumed reason)?
  Smallest: private match, host leaves in the lobby, read boot.log for a member's SplitSession and the hook log for
  the migration driver's "migration error"; if the member never splits, the election failed (Storm) – then the
  `+0x5a8` gate in H5b is the only thing keeping private matches from the running-match path and must stay.
- **E7** `StateLoadingMatch` (state 6) at the moment of the kill: F4 covers it (pending state `0x0338db3c`), but the
  level barriers of the match load are then in flight. Smallest: kill the host during the loading screen once;
  expect the same return or a clean dissolve, never a hang.
