# Host migration and parties: architecture

Status: design, 2026-10-22, for the DX11 build (`Blacklist_DX11_game.exe`, addresses as loaded, image base
0x400000) and the dev server on branch `fix/host-migration-client` (commits 9952cc4, 7ccd597, be4d5e0; commit
8341913 is the discarded party draft, see §4). Markers as in `5ee-research`: **R** seen live, **C** read from
the code (Ghidra output `~/Games/scbl/ghidra-jobs/ergebnisse/dx11-alles.c`, line numbers given as `L<n>`, and
`objdump`), **I** inferred, **V** guess. Every address below was checked in the decompiled text or the
disassembly on 2026-10-22.

Sections: 1 goals · 2 findings (questions A–F) · 3 architecture · 4 decisions · 5 implementation tasks ·
6 open points for live tests · 7 order and dependencies.

> **Since the merge (2026-10-24) the hooks are not configurable any more: they are always on.** The `uplay.toml`
> keys `MigrateMatchLobby`, `MigrateRunningMatch` and `PartyFollowsInvite` are gone (a file that still has them is
> read as before, the keys are ignored); `PartyStaysAfterMatch` (H3) and `TraceMemberLeave` (`member_trace.rs`) were
> removed. The match lobby migration and the running-match fallback (`migrate_lobby.rs`), `follow_unblock.rs` (with
> `follow-dup`) and the leader's party following an invitation (`party_follow.rs`, H1/H2) are installed by
> `hooks::init` on every start (not in the `patch-free` build; `follow_unblock` and `party_follow` skip DX9 with a
> warning). The design and the live reports below keep the flag names as they were at the time; the consequences for
> players with an older client are in §11, "Mixed clients".

---

## 1. Goals and non-goals

Goals (the cases the design must cover; expected behaviour in §3.4):

1. Public lobby, host leaves on purpose: the lobby carries on under a new host (already live, R 2026-10-22).
2. Public lobby, host drops hard: the same, after ~12 s of silence (already live, R).
3. Private match, host leaves on purpose: no migration (the game's own behaviour; wanted). Everyone lands alone
   in their own party ("Deine Verbindung zum Host wurde getrennt").
4. Private match, host drops hard: the same as 3 (R).
   Cases 1–4 each with and without party members in the match.
5. **A party (leader + members) invited into a third player's private match joins it together, is put in one
   team, and after the lobby is dissolved is together again in the party it started from.** The open problem.
6. The party's leader hosts a private match itself (the game's normal case): must keep working.
7. A party member (not the leader) is invited: it leaves its party alone (the game's behaviour), the party
   carries on without it.
8. A party and a public lobby: the party falls apart (the game's behaviour; migration doesn't change it).
9. Several members move along in a migration (7001 to each).
10. Single players (no party) see no change at all.

Non-goals: a host leaving a running match (the match ends for everyone; not changed); the DX9 build (the new
hook is skipped there until its patterns are found); parties larger than the game's eight seats.

---

## 2. Findings from the decompiled game

### 2.1 Objects and fields used below (C)

| Object | Where | Fields |
|---|---|---|
| `NetCore` | `[0x3383df4]` | `+0x5cc` NetServiceManager; `+0x5d0` match session (`NetGameSession*`, kind 0); `+0x5d4` party session (`NetPartySession*`, kind 1); `+0x5d8` the local player's `NetPlayerDescriptor*`; `+0x5c8` byte "private custom match" (computed in `nsNetCore::StateInitEcho::vf08` 0x008351d0 L829935–829953 as `goal+0x24 == 8 ? goal+0x10 : 0`) |
| `NetOnline` | `[0x3383df8]` | `+0x58` party controller (`nsOnlineParty` owner); `+0x4c` presence controller; `+0x48` lobby; `+0x18` match Rdv service; `+0x2c` online service |
| `NetSessionBase` (party or match) | | `+0x3fc` own `NetGameSessionDescriptor`; `+0x4ac` **PeerDescriptor of the session's host** (set from Storm's session info `+0xc8` in `FUN_00779620` L638231, and from the join endpoint in `FUN_0086a1e0` L878738/878745); `+0x428/+0x42c` server session id; `+0x554` Storm session info (host test `FUN_01f92c70`: `[+8] != -1 && [+8] == [+4]`); `+0x55c` own Storm id; `+0x564` session id (u64, used as `party_session`); `+0x584/+0x588` NetPeer list (entries: `node+8` → record, `record+10` connected flag, `record+0xc` the peer's `NetPlayerDescriptor`); `+0x5a8` private; `+0x5a9` may migrate; `+0x5b4` capacity; `+0x5de` locked; `+0x5f0` kind; `+0x610` MigrationInfo |
| `NetPlayerDescriptor` | per player; local one at `NetCore+0x5d8` | `+0x10` pid (u64); `+0x78` **`party_session`** (u64; setter `FUN_007a1a90`); `+0x8c` rank (i32). Offsets follow from the balancing code (§2.3) and the wire order in `5ee-research/protocol/session.md` §3.4 |
| party controller (`[0x3383df8+0x58]`) | | `+0x3c` goal queue; `+0x410` party size high-water mark (0 = never had a party; `L789180–789195`); `+0x414` follow stage (0 none, 1–4 "leader is starting: create/join/quick/coop", 6 FollowLeader received, 7 leader action 3); `+0x420/+0x424` member list (sentinel/first; `node+4` next, `node+8` → member record `{u64 id, +9 synced-private, +10 synced-public}`); `+0x430` the FollowLeader endpoint (`NetSessionEndpoint`); `+0x434`, `+0x438` settings flags |
| presence controller (`[0x3383df8+0x4c]`) | | `+0x4e0` endpoint of the **anteroom** (room kind 1) found by the invite search; `+0x4e4` endpoint of the **match** (room kind 0) found by it (`nsOnlinePresence::StateAcceptInviteFindSession::vf00` 0x00850d00: property 0x71 = 113 → 0 ⇒ `+0x4e4`, 1 ⇒ `+0x4e0`); `+0x4d8` inviter id |
| `NetSessionEndpoint` (0x3c0 bytes; `FUN_0084ca80` from a search result, `FUN_00778720` from a FollowLeader) | | `+0x160` two `NetGameSessionDescriptor` (0x9c each, index = kind); `+0x298` capacity (8); `+0x2a0` private; `+0x2d4` "peer form" flag; `+0x2dc/+0x2e0` session id; `+0x2e4` `PeerDescriptor`; `+0x3b5`, `+0x3b7` route flags |
| goal object (`nsOnlineMatch` current goal at `owner+0x220` when `owner+0x328 ∈ 4..6`, else `owner+0x50` when `owner+0x158 ∈ 4..6`) | | `+0` goal id; `+4` name; `+8` task; `+0x10` first parameter byte (private for CustomMatch); `+0x24` parameter kind (8 = custom-match parameters, 0xb = join parameters) |

Goal ids (C, from the name tables at `0x031126f8..` and `0x03112450..`): party `Host 0, Join 1, LeaveThenHost 2,
Leave 3, FollowLeader 4, PartyWaitSync 5, SetupVoice 6`; match `QuickMatch 1, FailureCleanup 2, CustomMatch 3,
JoinMatch 4, LeaveMatch 5, LadderMatch 6, Shutdown 7`.

### 2.2 Answers to questions A–F

| Q | Answer | Marker | Where |
|---|---|---|---|
| **A1** How the leader tells the party when it hosts | `StateNetMatchEnter::vf08` (0x0088ce50) pushes the lobby's `eGoal_Login` with callback `FUN_00884a20`; on success that calls `FUN_00879840(task, 1)` = push **`eGoal_PartyWaitSync`** to the party controller if `param==0 || I host the party` (`FUN_0078a330`). The party FSM's goal dispatch (`nsOnlineParty::StateIdle::vf02` 0x008601d0) sends goal 5 to **`StateLeaderPartyWaitSync`** on the leader; on a member it sends `MemberAction(2)` and goes to `StateSetupVoice`. | C | L899802–899840, L890610–890700, L7xx (0x008601d0 case 5) |
| **A2** The FollowLeader send | `StateLeaderPartyWaitSync::vf08` (0x00839280): timer 15 s (`this+0x38 = 15.0`); `local_11 = (goal+0x24 == 8) ? goal+0x10 : 0`; if set: the match session (`NetCore+0x5d0`) must be `InSession` **and I must be its host** (`FUN_01f92c70` at 0x008393d8); then `NetMsgParty_FollowLeader` is built by `FUN_00828500(this=buf, descriptor = session+0x3fc, peer = session+0x4ac, private = session+0x5a8)`, the party service is fetched by `NetServiceManager::vf41` (`[NetCore+0x5cc]`, vtable `+0xa4`) and the message broadcast with `FUN_007c27f0(this=service, msg, -1)`, destructor `FUN_00826c40`. Then the own member record's `+9` (private) or `+10` (public) flag is set. Message fields (`FUN_0078b360`): descriptor `+0x10`, u32 `+0xb0`, PeerDescriptor `+0xc0`, bool `+0x15c` — the wire layout in lobby.md §3 (R). | C | objdump 0x008392a8–0x00839440 (quoted in §5, task H2) |
| **A3** The wait | `StateLeaderPartyWaitSync::vf00` (0x008ae430): fails with 0x40000004 when `NetCore+0x5c8 == 0` (unless the lobby is in a special state, `FUN_007a5bc0`); counts the timer down; done (`FUN_008277a0`) when every member record has flag `+9` (private) / `+10` (public); on timeout kicks the others (`FUN_007d4080`, "Leader Kick"). | C | 0x008ae4a4–0x008ae529 |
| **A4** Member side | The party message dispatcher `FUN_00879d80` (via `NetServiceParty::vf37` 0x0087aaa0): **FollowLeader** → `FUN_00778720` copies descriptor/peer/private into the endpoint at `ctrl+0x430` (sets `+0x298 = 8`, `+0x2a0`, `+0x3b4`), `+0x414 = 6`. **LeaderAction** (action 1, value v) → `+0x414 = v`; action 2 → cancel (`+0x414 = 0` if 1..4); action 3 → `+0x414 = 7`, match `+0x5e0 = 1`. **MemberAction** (on the leader): 1 → member `+9 = 1`; 2 → `+10 = 1`; 3 → re-send FollowLeader to that member. A member reaches `StateFollowLeaderWaiting` only through **`eGoal_FollowLeader`** (`FUN_008796a0`), whose single caller is the UI task `UI::Task::PartyFollowLeaderTask::vf04` (0x018eae00, L4157299–4157305), spawned by the party screen when `+0x414` is 1..4 (I). `StateFollowLeaderWaiting::vf00` (0x00827460): stage 0 → error; 1..4 → wait; 6 → `StateFollowLeaderJoinMatch::vf08` (0x00878940) → `FUN_00877370(task, ctrl+0x430)` = push **`eGoal_JoinMatch`** with that endpoint. On success the callback `FUN_00850b60` sends `MemberAction(1)` to the leader. | C | L890731 (FUN_00879d80), 0x00827460, 0x00878940, 0x00850b60 |
| **A5** Server calls | The join from an endpoint is `eGoal_JoinSpecificSession` (`FUN_0086a1e0`, the flag setter the existing hook patches): `StateRegisterURLs` → `StateJoin`; `StateJoin::vf08` (0x0084da70) sends **nothing** for a private session (`+0x42b = 1`, waits for the 7003 gate), `JoinSession` for a public one. The **host** adds a completed peer: `FUN_00836520` (peer record complete) → `NetOnlineSessionServiceRdv::vf18` (`eGoal_PlayerJoin`) → `StatePlayerJoin::vf08` (0x008a0990) → Rdv `vf21` = **AddParticipants(session, [peer pid], private = session+0x5a8)**; skipped when the peer is me or `NetCore+0x5c8 != 0`. The follower itself calls RegisterURLs only (R, sessions.md §4). | C, R | L639100–639165, 0x00836520, 0x008a0990 |
| **A6** `party_session` | Set once, when the player's own join into a **party** (kind 1) completes: `nsGameSession::StateAttemptingToJoin::vf11` (0x00869990) L878370–878377 `FUN_007a1a90(NetCore.party+0x568, +0x564)` = the party's Storm session id. It is not touched by a match join, so every member carries its party's id into the match; the host's own descriptor carries its party id (R). | C, R | L878355–878380 |
| **B1** Teams | `nsOnlineLobby::StateBalancingTeams::vf08` (0x00896cf0): lobby state 4; on the host, teams service (`NetServiceManager::vf44`, `+0xb0`) `+0x74` mode ≠ 0 → `FUN_008938d0` → `FUN_00893720`: mode 1 → **`FUN_0088f380`**, mode 2 → `FUN_007c8e80` (plain fill-up). `vf00` waits until `FUN_007c0990` (enough players) and `FUN_007c0930` (team sizes differ by ≤ `FUN_007a75c0()`) hold. Mode source: not found (I: the rules' `teamComposition`). | C | 0x00896cf0, 0x008938d0, 0x00893720 |
| **B2** Party grouping | `FUN_0088f380` groups the lobby's player records by the u64 at **record+0x78 = `NetPlayerDescriptor.party_session`** (loop at the top: `local_4c = *(rec+0x78)`, compared with existing groups, new group otherwise); weights from `FUN_007c8f50` (rank `+0x8c` × 10 or a stored score). Groups of ≥ 2 are placed as a block on the team with room (`capacity/2 = session+0x5b4/2`), singles fill up. So **players whose descriptors carry the same `party_session` land in one team** — the host needs nothing from the server; the information comes with each joiner's descriptor over Storm. | C (I for "block") | 0x0088f380 |
| **B3** Why the invited leader isn't grouped today | The leader's descriptor keeps its *party* id; after the secede (C below) its party is a new solo session, the member's a different new one (1729 vs 1728, R 17:02:09) → no group. | C+R | boot.log 17:02:09 |
| **C1** What breaks the party when the leader accepts an invitation | The UI accept task `FUN_018dcba0` (unnamed `UI::Task`, DX11 0x018dcba0) calls **`FUN_00879b20(5)`** on the party controller (`push 5; call 0x879b20` at 0x018dcc2b, `ecx = [0x3383df8+0x58]`). `FUN_00879b20(param)`: param 5 → **`FUN_007a3ff0` = Storm `Secede` with reason "ForceMigrationSecede"** on the party session (0x007a3ff0 L687286); any other param on the leader → `NetMsgParty_LeaderAction(action 1, value param)` broadcast + member flags reset. The secede makes the leader host of a fresh session (local migration: RegisterURLs, Abandon 1725, Split → 1729, AddParticipants, R) and the members migrate the rest (Split → 1728). | C, R | L4146800–4146835, 0x0087a240 objdump |
| **C2** What the presence FSM would do without the secede | `StateAcceptInviteFindSession` fills `+0x4e4`/`+0x4e0` → `StateShutdownForInvite::vf08` (0x0087a240): **`if (match goal id != 5 (LeaveMatch) && presence+0x4e4 != 0) → keep the party (`+0x34 = 0`)`**, else push `eGoal_Leave` on the party. `StateAcceptInvite::vf08` (0x0087a2e0): with `+0x4e4` → `FUN_00877370(task, +0x4e4)` = `eGoal_JoinMatch` (the same join the party followers use), with `+0x4e0` only → `FUN_00879270` (`eGoal_Join`). So the game already has a "join the match, keep the party" path; only the UI's secede prevents it. | C | objdump 0x0087a240–0x0087a2af |
| **C3** The friends-list route | `UI::Task::JoinSessionTask::vf04` (0x018de3a0) does `FUN_00879b20(2)` (LeaderAction value 2 = "leader joins a session") and then `FUN_00877370(task, endpoint)` directly — the party is **kept** and told. It then fails at A2's host check / A3's `+0x5c8` on the leader, so the members' "following" ends in a kick (I). | C | L4147850, 0x018de3a0 |
| **C4** Uplay party invitations | `nsNetOnlineUplayPC::StateInvitingParty` runs `UPLAY_PARTY_InvitePartyToGame` (0x00808a80) = the Uplay overlay's "invite my Uplay party" and is a no-op in the hook (`hooks/src/uplay_r1_loader/party.rs`: answers success, sends nothing). `UPLAY_PARTY_RespondToGameInvite` is a stub. The `PartyGameInviteAccepted` event (`InviteAcceptEvent = "Party"` in `uplay.toml`) only changes which UI route an accepted invitation takes; no Uplay party exists on our side. So: the Uplay path is dead here and the game's own party mechanism (A) is the only one. | C | party.rs, hooks-config lib.rs L255–263 |
| **C5** How a host invites | One invitation per friend (`UPLAY_FRIENDS_InviteToGame` → server `Friends.Invite`, bound to the match room). There is no "invite the party" in our stack (C4). The host invites the leader; the party follows by the mechanism in §3. | C, R | api.rs `invite` |
| **D1** After the match | `nsOnlineMatch::StateLeaveNetworkedMatch::vf04` (0x007a7e00): result 2 → **`StateLeaveWithoutParty`** when the party session is `InSession`, else `StateDestroySession`. `StateLeaveWithoutParty::vf08` (0x00884b10) pushes **`eGoal_LeaveThenHost`** on the party; its `vf04` (0x007a7f70): result 2 → `StateDestroySession`, result 5 → **`StatePartyRestore`**. `StatePartyRestore::vf08` (0x00884b40): party `InSession` and the match goal was a private custom match (`goal+0x24 == 8 && goal+0x10`) → `FUN_00879840(task, 0)` = `eGoal_PartyWaitSync` (leader: `StateLeaderPartyWaitSync` again; members: `MemberAction(2)` + `StateSetupVoice`), else skip. | C | 0x007a7e00, 0x00884b10, 0x007a7f70, 0x00884b40 |
| **D2** When LeaveThenHost leaves | `nsOnlineParty::StateLeaveSession::vf08` (0x00878820): `if (ctrl+0x410 < 1) { if (match goal != 7 Shutdown) fail 0x80000003 }` else leave the party session (`FUN_0086a4f0`) and re-host (`StateHostSession`). `+0x410` is the party-size high-water mark, so a real party takes the **leave + host a new one** branch (R for the solo party: Abandon 1729, CreateSession 1730 at 17:02:18). Whether a party with members survives the return (members migrate the old session, leader hosts a new one → two parties) is **not decidable statically** → §6 E1. | C, V | 0x00878820, boot.log 17:02:18 |
| **D3** Why the live case needed a new party | The party was already gone (C1): the leader's "party" was the solo 1729; LeaveThenHost replaced it by 1730. | C, R | |
| **E** Storm vs server | Storm only (server blind): party messages (FollowLeader, LeaderAction, MemberAction, PartyState memento), the Storm join itself (ticket, join accept, peer connect), the kick, the team balancing and the teams memento, host election. Server: RegisterURLs, SearchSessionsWithParticipants (invite), AddParticipants (host for followers, 7003 to them), JoinSession (public), Leave/Abandon/Split/RemoveParticipants, SetSession (advertise). **The server alone cannot** make a game join anything (only an invitation or 7003 move a game), cannot send party messages, cannot set a descriptor field. **A client hook can** change the leader's decisions (secede, host check) and reuse the game's own send code. **Impossible**: a member joining a private match it has no endpoint for; grouping players without equal `party_session`. | C, R | |
| **F1** Public lobby migration and party ids | A party never enters a public lobby (R, lobby.md §4: the leader is seceded/kicks), so migrated members carry only their own ids; `party_session` is untouched by migration (A6). The teams objects (`NetServiceTeams`, among the service objects `NetServiceManager::vf52/vf53` hand to the new master, migration steps 8/9) move with the lobby (I); `StateBalancingTeams` reruns on the new host when the countdown restarts. | C, I | host-migration.md §3 |
| **F2** Private match dissolve with a party inside | Each member's match FSM runs D1 with the party session still `InSession` (the party's Storm connections survive a match, R lobby.md §5) → `LeaveWithoutParty` → D2 → the same open point as the return in case 5 (§6 E1). The server sees only the match's Leave/Delete; the party session is untouched. | C, R | |

### 2.3 The live run of 2026-10-22 17:02 read against the code (R)

Server (`~/Games/scbl-enhanced/devserver/boot.log`, Oct 22 17:02:09): `SearchSessionsWithParticipants(1005)` by
1010 → "invited room 1722 plus anteroom → 1722 (113 ⇒ 0), 1720 (113 ⇒ 1)"; then **1011 `SplitSession(1725)` → 1728**
(the member saw the leader's Storm secede) and **1010 `AbandonSession(1725)`, `SplitSession(1725)` → 1729,
`AddParticipants(1729,[1010])`, `RemoveParticipants(1729,[1011])`** (the leader's local migration after its own
secede), then 1010 `AddParticipants(1722, private [1010])` + 7003. That is C1 to the letter. After the match
(17:02:18): 1010 `LeaveSession(1722)`, `AbandonSession(1729)`, `CreateSession` → 1730 = D2's leave-then-host.

---

## 3. Architecture

### 3.1 Components and responsibilities

| Component | Owns | Changes in this design |
|---|---|---|
| **Game** (unchanged code paths) | Party messages, Storm joins, team balancing by `party_session`, the party return (`PartyRestore`/`PartyWaitSync`) | none, but relied on |
| **Client hook** (`hooks/src/party_follow.rs`, new; always on, formerly opt-in `PartyFollowsInvite`; DX11 only) | On the **leader** only: (H1) don't secede when accepting an invitation into a private match while the party has members — tell the party "I'm joining a session" instead (LeaderAction 2, what the friends-list route does); (H2) let `StateLeaderPartyWaitSync` send FollowLeader for a match the leader *joined* (not hosts): skip the host check, present the join as a private custom match to `vf08`/`vf00`. Members and the match host need **no** hook (their code paths are the original ones). | new file + 2 detours + 1 two-byte patch |
| **Server** (`dedicated_server/src/game_session.rs`, `escorts.rs` new) | Admission: a member of the invited leader's party may be added to the private match (by the match host's `AddParticipants`, or by itself) and gets the 7003 gate; a member's split of a party whose leader was only seceded from doesn't make a takeover. Already there: host migration (SplitSession takeover, 7001, silent-host takeover). | small, see §5 S1–S3 |
| **Testbot** | Scenarios for the server rules | §5 |

### 3.2 Data flow, case 5 (party into a third player's private match), numbered

Players: H = match host (mpw, private match M=1722 with anteroom A=1720), L = party leader (1010, party P=1725),
B = member (1011, in P). Hook on L (and on everyone, harmless).

1. H invites L (`Friends.Invite`, server binds it to M; delivered to L's game as `FriendsGameInviteAccepted`
   when L accepts in the overlay). **Server: unchanged.**
2. L's game: UI accept task `FUN_018dcba0` → `FUN_00879b20(5)`. **Hook H1**: L hosts P with ≥ 1 other member and
   the inviter's advertised session is invite-only → call `FUN_00879b20(2)` instead. Game: `LeaderAction(1, 2)`
   broadcast to P over Storm; no secede. (Solo players, or a public inviter: original call, nothing changes.)
3. B's game: `+0x414 = 2` → party screen spawns `PartyFollowLeaderTask` → `eGoal_FollowLeader` →
   `StateFollowLeaderWaiting` (stage 2, "following"). (I: the same UI reaction as for value 1; §6 E2.)
4. L's presence FSM: `StateAcceptInviteFindSession` → `SearchSessionsWithParticipants([H])`. **Server** (unchanged
   code): adds L to M, answers M (113=0) and A (113=1), and **(S1) notes B as an escort of L for M** (B is in L's
   party P at that moment). L: `+0x4e4 = M`, `+0x4e0 = A`.
5. `StateShutdownForInvite::vf08`: match goal ≠ LeaveMatch and `+0x4e4` set → party kept (original code).
6. `StateAcceptInvite::vf08` → `FUN_00877370(task, M endpoint)` → `eGoal_JoinMatch` → `eGoal_JoinSpecificSession`:
   Storm join of M (L's descriptor: `party_session = P`), `RegisterURLs`, `StateJoin` waits; H's game adds L
   (`AddParticipants(M, private [L])`; L also adds itself on the invitation route, R) → 7003 to L → L is in the
   lobby. **Server: unchanged** (L is invited).
7. `StateNetMatchEnter::vf08` → lobby login → `FUN_00884a20` → `FUN_00879840(task, 1)`: L hosts P →
   `eGoal_PartyWaitSync` → `StateLeaderPartyWaitSync::vf08`. **Hook H2**: the match session is `InSession`, L is
   not its host, it is private, P has members → make the original `vf08` take its send path: FollowLeader
   (descriptor of M, **H's PeerDescriptor from `M+0x4ac`**, private=1) broadcast to P; own flag set; 15 s timer.
   `vf00` runs the original wait with `NetCore+0x5c8` presented as 1.
8. B's game: FollowLeader → `+0x430 = endpoint(M, H's peer, private)`, `+0x414 = 6` → `StateFollowLeaderJoinMatch`
   → `eGoal_JoinMatch` → Storm join of M (`party_session = P`, no_invitation = 0), `RegisterURLs`, `StateJoin`
   waits for 7003. H's join check (`FUN_00859cf0`) has no party or invitation test beyond the attachment bit
   (C) → accepted.
9. H's game: B's peer complete → `eGoal_PlayerJoin` → `AddParticipants(M, private [B])`. **Server S2**: allowed
   because B is an escort of L for M (S1); B is noted as a match player; **7003 (pid_source H, ui_param_1 B,
   ui_param_2 M)** to B. (If B's own game adds itself instead — the invitation route's habit — S2 allows that
   too and notifies B.) B's gate opens; B is in the lobby. B's callback sends `MemberAction(1)` → L's `vf00`
   sees B's flag → sync done (no kick).
10. Lobby: `StateBalancingTeams` on H groups L and B by `party_session = P` → **one team** (B2).
11. Match ends / H dissolves the lobby: L and B run D1: party `InSession` → `LeaveWithoutParty` →
    `LeaveThenHost` → D2. Expected (I, to be verified §6 E1): both are back in P; if D2 instead re-hosts a new
    party on L and migrates P on B, task H3 (contingency) replaces `LeaveThenHost` by `PartyWaitSync(0)` on the
    leader. **Server**: P was never left, nothing to do.

### 3.3 State machines touched (summary)

Leader, presence: `Idle → AcceptInviteFindSession → ShutdownForInvite (keep) → AcceptInvite (JoinMatch) → Idle`.
Leader, party: `Idle → LeaderPartyWaitSync (H2) → SetupVoice → Idle`.
Member, party: `Idle → FollowLeaderWaiting (stage 2) → FollowLeaderJoinMatch → (match FSM) → … → SetupVoice → Idle`.
Both, match: `JoinMatch → NetSetupVoice → NetMatchEnter → … → LeaveNetworkedMatch → LeaveWithoutParty → PartyRestore/DestroySession`.

### 3.4 Expected behaviour per case

| Case | Behaviour | Mechanism | Server / hook change | Test |
|---|---|---|---|---|
| 1 public, host leaves | lobby migrates, members told by 7001 | existing (`split_session` takeover, `MigrateMatchLobby`) | none | existing `host-migration*`; live R |
| 2 public, host drops | same after ≥ 12 s silence | existing `resolve_silent_hosts` | none | existing; live R |
| 3 private, host leaves | dissolve, everyone alone in own party | game (`+0x5a9` cleared for private) | none | live R |
| 4 private, host drops | same | game | none | live R |
| 1–4 with party members inside | public: no party exists inside (8); private: members keep their party session, return per D1/D2 | game | none; E1 decides H3 | §6 E1 |
| 5 party → third player's private match | together, one team, back together | §3.2 | H1, H2, S1, S2 (+H3 if E1 fails) | T1–T5 below; live L1–L4 |
| 6 leader hosts private match | unchanged | H1 only on action 5; H2 only when not host | none in effect | live L0 (regression) |
| 7 member invited | member secedes (original), party carries on | game; S3 keeps the server's takeover bookkeeping right | S3 | T6 |
| 8 party + public lobby | falls apart as before (secede for a public inviter; a joined public lobby isn't private → H2 inactive) | H1's invite-only check | none | live L5 |
| 9 several members migrate | 7001 to each | existing | none | existing `host-migration` |
| 10 singles | no change: H1 needs ≥ 1 other member, H2 needs a party with members | | none | L0 |

### 3.5 Messages and notifications (exact fields)

- **7001** (existing, server → member on migration): `NotificationEvent { pid_source: new_host, ui_type: 7001, ui_param_1: new_session, ui_param_2: old_session, str_param: "", ui_param_3: 0 }`.
- **7003** (existing, server → added player of a private room): `{ pid_source: adder, ui_type: 7003, ui_param_1: player, ui_param_2: session, "" , 0 }`. For an escort added by the host: `pid_source = host`.
- **NetMsgParty_LeaderAction** (Storm, leader → members): `i8 action = 1, i8 value` (value 2 = joining a session).
- **NetMsgParty_FollowLeader** (Storm, leader → members): `NetGameSessionDescriptor` (name "", session id M, 2 bits, u64 0, bits 1 0 1 for a private match, cu64 all ones, u32 key), `u32`, `PeerDescriptor` (H's address/port as L sees it), `bool private = 1`.
- **NetMsgParty_MemberAction** (member → leader): `i8` 1 = joined (private), 2 = synced (public), 3 = resend FollowLeader.

---

## 4. Decisions and rejected alternatives

1. **Reuse the game's own party-follow (FollowLeader) instead of server-side invitations.** The game's path gives
   simultaneity (one broadcast), the right `party_session` on every member (B2, one team), and the original
   return path. The server cannot do any of this (E).
2. **Fix the leader, not the members or the host.** Only two decisions on the leader are wrong for our case: the
   UI's secede (C1) and `vf08`'s "I must host the match" (A2). Members and the match host run unchanged code.
3. **Detours with data tweaks over re-implementing the send.** H2 makes the original `vf08` take its own send
   path (goal fields presented as a private custom match, host-check jump NOPed) instead of rebuilding the
   message in Rust: fewer offsets to get wrong, and the own-flag/timer bookkeeping stays the game's.
4. **`+0x5c8` is presented as 1 only while `vf00` runs** (set before, restored after each call), because its
   other readers (`FUN_00836520` 0x00836520, `nsNetSessionRdv::StateIdle::vf02` 0x0085b3f0,
   `nsOnlineLobby::StateLogin::vf05` 0x00873f00, `nsOnlineConnection::StateOnline::vf08` 0x0086d790,
   `nsGameSession::StateAttemptingToJoin::vf04` 0x007a3dc0) gate other behaviour.
5. **H1 only for a private inviter and only with members.** The hook checks the inviter's `invite_only` from the
   friend list it already fetches (`hooks/src/api.rs`, `invite_only` per friend); otherwise the original secede
   runs, so singles and public invitations are untouched (cases 8, 10).
6. **Server admission by "escort" bookkeeping, not by friendship or party-of-host.** The server notes, when the
   invited leader's friend search is answered, who is in the leader's party *then* (this is the `board` part of
   commit 8341913 and stays); an escort may be added to that match for a few minutes. This is the one server
   mechanism; it is idempotent whether the host or the member itself calls `AddParticipants`.
7. **What goes from 8341913**: `bring_party` (invitations to members after the leader arrived — time-shifted,
   wrong `party_session`), `bring_party_back` and `Trips::back/came` (the return by invitations — the game's
   `PartyRestore` does it over Storm), the `party-follows-leader` scenario in its current form, the `SetSession`
   hook in `api.rs`. **What stays**: the `board` noting at the friend search (`board_party`,
   `Storage::hosted_parties`), renamed into `escorts.rs`.
8. **Rejected**: a static one-byte patch `push 5 → push 2` in the UI task (would change singles); re-sending 7003
   from the server on a timer (the host's `AddParticipants` is the game's own trigger, C A5); migrating private
   matches (user decision, 7ccd597).

---

## 5. Implementation tasks (in order)

Conventions: hook code in Rust, `retour::static_detour` as in `hooks/src/hooks.rs` (`hook_with_name`), addresses
through `hooks/hooks-addresses` (new `Option<Address>` fields, DX11 values below, `None` for DX9), byte patches
through `crate::game_code()` + `find_once` as in `hooks/src/migrate_lobby.rs`. All thiscall: `this` in `ecx`,
arguments pushed right to left, callee cleans (`ret n`).

### H0 Config flag

(Superseded: since the merge there is no flag, `party_follow::install` runs unconditionally.)

- `hooks/hooks-config/src/lib.rs`: `pub party_follows_invite: bool` (`PartyFollowsInvite`, serde default,
  `skip_serializing_if = Not::not`), doc as for `migrate_match_lobby`; unit test like
  `migrate_match_lobby_round_trips`. `hooks/src/hooks.rs::init`: `if config.party_follows_invite { crate::party_follow::install(addr) }`
  under `#[cfg(not(feature = "patch-free"))]`.

### H1 Leader keeps its party when accepting an invitation into a private match

Detour `FUN_00879b20` (DX11 0x00879b20; `unsafe extern "thiscall" fn(*mut c_void, i32)`, `ret 4`).
Verify the target: `objdump -d -M intel --start-address=0x879b20 --stop-address=0x879b5f …exe` must show
`55 8b ec 6a ff 68 b3 b9 65 02 … 8b f1 a1 f4 3d 38 03 33 db 3b c3 0f 84 11 01 00 00 8b 88 d4 05 00 00` and the
tail at 0x879c7b `c2 04 00`. Single call site with 5: `objdump … --start-address=0x18dcc22 --stop-address=0x18dcc32`
→ `8b 15 f8 3d 38 03 8b 4a 58 6a 05 e8 ee ce f9 fe`.

```
fn leader_action(this, action):
    if action == 5 && party_has_other_members() && inviter_is_private():
        info!("party-follow: invitation accepted as party leader - keeping the party (LeaderAction 2 instead of secede)")
        return Original(this, 2)
    Original(this, action)
```
- `party_has_other_members()`: `core = [0x3383df4]`; `party = [core+0x5d4]` nonnull, in state `InSession`
  (read the state id the way the game does at 0x839307–0x839329: `s = [party+0x10]`; if null the id is
  `[0x2cf5c50]`, else `id = *((*(vtable(s) + 0x30))(s))`, thiscall; compare with `[0x338d7a4]`), and not
  migrating (`[[party+0x610]+4] == 0`). Then `ctrl = [[0x3383df8]+0x58]`, own id `*(u64*)([core+0x5d8]+0x10)`,
  walk `node = [ctrl+0x424]` until `node == ctrl+0x420` (`next = [node+4]`, `rec = [node+8]`), count records with
  `*(u64*)rec != own id`; true when `> 0`.
- `inviter_is_private()`: the hook records the sender of the invitation it delivered as
  `FriendsGameInviteAccepted` (`hooks/src/uplay_r1_loader.rs`, `into_friend_invite_accepted(ubi_name)`): store
  `ubi_name` + time in a `static Mutex<Option<(String, Instant)>>`; look the name up in the last friend list
  (`crate::api` friend entries, field `invite_only`); true when found and `invite_only`, and the event is
  younger than 60 s. Unknown → false (original secede).
- Tests (unit): the member-count walk on a hand-built list (two records, one own); the invite-record aging.
- Risk: `+0x5d4` null before the network is up (guard every pointer; on any null → original).

### H2 The sync state sends FollowLeader for a joined private match

(a) Two-byte patch in `StateLeaderPartyWaitSync::vf08`: pattern (found once in `.text`)
`8d 8f 54 05 00 00 e8 ?? ?? ?? ?? 84 c0 74 5f 0f b6 8e a8 05 00 00` (= `lea ecx,[edi+0x554]; call FUN_01f92c70;
test al,al; je +0x5f; movzx ecx,[esi+0x5a8]`, at 0x008393d2); replace the `74 5f` (offset 13) by `90 90`.
Effect: the send no longer requires being the match's host; it still requires `local_11` and `InSession`.
Harmless when hosting (the test was true). Verify after patching by reading the bytes back, as
`migrate_lobby.rs` does. Reference disassembly (unpatched):

```
8393d2: 8d 8f 54 05 00 00   lea    ecx,[edi+0x554]
8393d8: e8 93 98 75 01      call   0x1f92c70        ; am I the host of the match session
8393dd: 84 c0               test   al,al
8393df: 74 5f               je     0x839440         ; -> patched to 90 90
8393e1: 0f b6 8e a8 05 00 00 movzx ecx,BYTE PTR [esi+0x5a8]   ; private
8393e8: 8d 86 fc 03 00 00   lea    eax,[esi+0x3fc]  ; descriptor
8393ee: 51 / 8d 90 b0 00 00 00 / 52 / 50            ; push private, push &[esi+0x4ac] (host peer), push descriptor
8393f7: 8d 8d 5c fe ff ff   lea    ecx,[ebp-0x1a4]  ; message buffer
8393fd: e8 fe f0 fe ff      call   0x828500         ; NetMsgParty_FollowLeader ctor
839409..83941c: NetServiceManager (+0x5cc) vtable +0xa4 -> party service
83941e: 6a ff / … / e8 c2 93 f8 ff  call 0x7c27f0  ; broadcast(msg, -1)
83943b: e8 00 d8 fe ff      call   0x826c40         ; dtor
```

(b) Detour `StateLeaderPartyWaitSync::vf08` (0x00839280, `fn(this)`; prologue `55 8b ec 6a ff 68 8b d0 64 02 64 a1
00 00 00 00 50 81 ec 98 01 00 00 53 56 57`):
```
fn sync_enter(this):
    if escort_case():                       # see below
        goal = current_match_goal()         # owner = [this+8]; +0x220 if [owner+0x328] in 4..=6 else +0x50 if [owner+0x158] in 4..=6 else null
        if goal: save (kind=[goal+0x24], p=[goal+0x10] byte); [goal+0x24]=8; [goal+0x10]=1
        ESCORT_SYNC.store(true)
        Original(this)
        if goal: restore
        info!("party-follow: FollowLeader sent for joined private match {id} to the party")
    else: Original(this)
```
`escort_case()`: `m = [core+0x5d0]` nonnull; its FSM state id == `[0x338d7a4]` (`nsGameSession::StateInSession`;
read it like the game: `s=[m+0x10]; id = *( (*(vtable(s)+0x30))(s) )`); `FUN_01f92c70(m+0x554) == 0` (not host;
call the game function, `thiscall`, no args); `[m+0x5a8] != 0` (private); `[m+0x5f0] == 0` (kind match);
`party_has_other_members()` from H1. The goal read (`[goal+0x24]`) is logged before/after for the live test.

(c) Detour `StateLeaderPartyWaitSync::vf00` (0x008ae430, `fn(this, dt: f32)` — the float is the first stack
argument, `ret 4`; prologue `55 8b ec 6a ff 68 d0 56 66 02 …`):
```
fn sync_tick(this, dt):
    if ESCORT_SYNC: old=[core+0x5c8]; [core+0x5c8]=1; Original(this,dt); [core+0x5c8]=old
                    if [this+0x10] != 0: ESCORT_SYNC.store(false); info!("party-follow: sync finished ({})", [this+0x10])
    else: Original(this, dt)
```
`[this+0x10]` is the state's pending transition (4 = error, set at 0x8ae4bb..; 1/3 = done). Reference:
`8ae4aa: 80 b9 c8 05 00 00 00 cmp BYTE PTR [ecx+0x5c8],0 ; 75 76 jne 0x8ae529`.

- Hook addresses: add `func_party_leader_action: Some(0x879b20)`, `func_party_sync_enter: Some(0x839280)`,
  `func_party_sync_tick: Some(0x8ae430)` to the DX11 table (`hooks/hooks-addresses/src/lib.rs` `dx11_addrs`),
  `None` in `dx9_addrs` and in `search_patterns`; `party_follow::install` skips everything (warn) when any is
  `None` or the two-byte pattern isn't found exactly once.
- Unit tests: pattern length/offset constants; `find_once` on synthetic text (as in `migrate_lobby.rs`).
- Risks: the JoinMatch goal's `+0x10` byte might be read elsewhere during `vf08` (it isn't: `vf08` is
  synchronous, C); `local_11` also makes `vf00` kick members that don't arrive within 15 s (original behaviour
  for a hosted private match, acceptable); DX9 unsupported (skipped with a warning).

### H3 (implemented, switched off: E1 showed the party doesn't survive the return, and H3 alone doesn't fix it)

(Removed at the merge, with `PartyStaysAfterMatch` and the address `func_party_leave_without_party`: the return is
the server's S4. The text below is the record of what was built and measured.)

**Status 2026-10-23, later:** H3 now sits behind `PartyStaysAfterMatch` (default off; `party_stays_after_match`), the
code is kept. Measured live (10:00): H3 ran on the leader (`the party stays: PartyWaitSync instead of
LeaveThenHost`), but the member has no hook and does not answer; both clients hung on "joining" over the lobby and
the leader removed the member after 15 s (`RemoveParticipants`). The return is done by the server instead (S4).

**Status 2026-10-23:** E1 measured live (09:28): the party leader (hook on) and its member joined the private match
together, in one team, party session 1828 kept. When the host dissolved the lobby the leader ran
`StateLeaveWithoutParty::vf08` → `LeaveThenHost` → `StateLeaveSession::vf08` and left 1828 (hosting 1832); the
member (no hook) did not abandon, saw its party host go and migrated 1828 alone (1831). Two parties. H3 is
implemented in `party_follow.rs` (`leave_without_party`, address `func_party_leave_without_party` 0x00884b10):
when the party session is `InSession` (the lock is not asked), has other members, and the current match goal is
`goal+0x24 == 8 && goal+0x10` (as `StatePartyRestore` asks), the hook makes the task, stores it at `this+0x34` and
pushes `PartyWaitSync(0)`; otherwise the original runs. Correction to the text below: the helpers take
`ecx = [NetOnline+0x58]` (the party controller), not `this`; `FUN_00879840` is `ret 8`, `FUN_00879460` `ret 4`,
`FUN_00829450` takes `ecx = [NetOnline]`. The hook runs on any client with the party, members included. Hook log:
`party-follow: match over, the party stays: PartyWaitSync instead of LeaveThenHost (N other members)`, or
`... match over, <reason> -> LeaveThenHost as usual`. Open: the goal's stage at `LeaveWithoutParty` time (if it is
outside 4..=6 the log says "no match goal" and the original runs); a non-hooked member's own `LeaveThenHost`.

Detour `nsOnlineMatch::StateLeaveWithoutParty::vf08` (0x00884b10, `fn(this)`). Its body is
`task = FUN_00829450(ecx=[0x3383df8]); [this+0x34] = task; FUN_00879460(ecx=this, task)` (push
`eGoal_LeaveThenHost`). `StatePartyRestore::vf08` (0x00884b40) does the same with `FUN_00879840(ecx=this, task, 0)`
(push `eGoal_PartyWaitSync`); both helpers take `ecx = this` (the state object, whose `+0x3c` is the goal queue)
and the task as the first stack argument. The detour: when the party session is `InSession` with other members,
do the `PartyRestore` variant instead of the original (same three calls, `FUN_00879840` in place of
`FUN_00879460`). On the leader the dispatcher then runs `StateLeaderPartyWaitSync` (H2 inactive there: the match
session is gone, so `escort_case()` is false and the original `vf08` runs with `local_11 = 0` → the public
wait: members answer with `MemberAction(2)`); on a member it sends `MemberAction(2)` and goes to `SetupVoice`.
Apply it to members too if E1 shows their `LeaveThenHost` leaving the party as well. Decide after E1.

### S1 Escorts: who may follow whom (server)

- New `dedicated_server/src/escorts.rs` from `party_trips.rs`: keep `Trips::board` as `Escorts::board(leader,
  room, companions, now)`; replace `arrive/came/back` by `allowed(&self, room, member, now) -> bool` (a companion
  of any boarding into `room` younger than `BOARDING_FOR = 10 min`) and `boarded_by(room, member) -> Option<leader>`;
  keep the bounds (`MAX_COMPANIONS 8`, `MAX_TRIPS 1024`). Delete `party_trips.rs`.
- `game_session.rs`: keep `board_party` (called from `search_sessions_with_participants` after the invitee is
  added) → `escorts::board`; delete `bring_party`, `bring_party_back`, `note_match_players` call sites of the
  old flow, and `api.rs`'s `SetSession` hook (lines 634–642 of 8341913); keep `Storage::hosted_parties`.
- Unit tests: `escorts::tests` (board once, allowed within window, not after, not for a stranger, bounded).

### S2 Admission of escorts (server)

`add_participants` (game_session.rs ~L938):
1. Self-add into a private room: `invited || party_of_host || escorts::allowed(session_id, user_id)`.
2. Adding someone else: allow when `in_party_of(t, user_id) || friends || escorts::allowed(session_id, t)`,
   never past a block (keep `blocked_blocking`).
3. Notification: `participant == user_id` → notify self when `via_invitation || escorts::allowed(...)`.
4. `session_events::joined(target, session, "party")` for escorts (the `via` chooser: escort → "party").
- Testbot `party-escort` (replaces `party-follows-leader`): Host H (anteroom + private match M), Leader L
  (party P), Member B (joined P), Stray S. H invites L; L `search_with_participants([H])` → M present. **No**
  split/abandon by L (the party is kept). L `add_participants(M, [], [L])` → 7003. Then (i) `H.add_participants(M,
  [], [B])` succeeds and B gets 7003 with `ui_param_1 == B`, `ui_param_2 == M`; (ii) `H.add_participants(M, [],
  [S])` is refused; (iii) a second run where B adds itself (`B.add_participants(M, [], [B])`) succeeds and B gets
  7003; (iv) a stranger's self-add is refused; (v) `L.search_with_participants([H])` lists H, L, B in M.
- Live criteria (server log): after L's search → `Adding 1010 to invited room 1722`, `1010 accepts an invitation
  into private match 1722 of 1005; its party: [1011]` (board); **no** `User 1010 abandons session 1725`, **no**
  `split session 1725`; `AddParticipants … session_id: 1722 … private_participant_ids: QList([1011])` (by 1005
  or 1011) followed by `Notifying 1011 about session 1722: … ui_type: 7003, ui_param_1: 1011, ui_param_2: 1722`.

### S3 A seceded member's split is not a takeover (server)

`split_session` records a `Takeover` when a member splits a session whose host is still in it. A member that
accepted an invitation elsewhere (case 7) does the same split after its Storm secede; if the leader then leaves
within 60 s, `host_gone` would move the rest into the departed member's solo session. Rule: in
`add_participants`, when `targets.contains(&user_id)` and `session_id != t.to`, drop every `Takeover` with
`t.by == user_id` (the member went somewhere else). Testbot `party-member-leaves-for-match`: H, L, B as above,
H invites **B**; B `split_session(P)` → P' (takeover noted), B `add_participants(M, [], [B])`; then L
`abandon_session(P)` → `host_gone` moves nobody (L's other member C stays in P… ends P when empty); assert C is
not in P'.

### S4 Return to the party (server)

The members who came into the match (`escorts::came`, noted when the server sees them added to it) are asked into the
leader's **new** party once the match is over. Trigger: the leader's `CreateSession` of a party (attribute `113 => 1`)
after the match: measured 09:28:18, `User 1010 leaves session 1825`, `abandons session 1828; the others in it carry on`,
`host migration of session 1828: 1011 hosts 1831`, then the leader's CreateSession (1832). The match counts as over
when its room is destroyed or the leader is no longer in it. Each member gets an invitation from the leader to the new
party, delivered as the requested join (no click, as `JoinNow`), if online and not blocked, within the invitation rate
limit; once per trip, kept at most 4 h. Log: `The match of <leader>'s party is over: <member> is asked back into party
<P> (invitation <n>, requested)`. Testbot: `party-return`. Risk: the member is still migrating its old party when the
invitation arrives.

### T Testbot bookkeeping

`tools/testbot/README.md`: replace the `party-follows-leader` row by `party-escort` and add
`party-member-leaves-for-match`; `main.rs` match arms accordingly. `build/build.sh bots party-escort`.

---

## 6. Open points only a live test can settle (smallest experiment each)

- **E1 Return to the party (D2). Measured 2026-10-23: the party does not survive (two single parties); H3 alone does not help (members have no hook); the server brings the members back (S4).** Smallest: no hook needed. L hosts a private match with B in its party (case 6),
  L ends the lobby from the menu. Watch boot.log for `AbandonSession`/`SplitSession` of P and the UI
  ("Meine Party 2/8" on both?). If P survives: H3 not needed. Variant with the hook: case 5 as in §3.2, H
  dissolves M.
- **E2 Members' UI reacts to LeaderAction value 2.** Smallest: with H1 only (H2 off), L accepts H's invitation;
  B's screen should show "following leader" (the task spawned); no member kick for 15 s is expected then
  (`vf00` errors out at `+0x5c8` without H2). If B shows nothing, the UI only reacts to value 1 → H1 sends
  `FUN_00879b20(1)` instead (one constant).
- **E3 `vf08`'s goal fields for a JoinMatch goal.** H2 logs `[goal+0x24]`/`[goal+0x10]` before tweaking; expected
  0xb/anything (I). If the original already yields `local_11 = 1`, the tweak can go.
- **E4 Who adds B.** boot.log: `AddParticipants … [1011]` — caller 1005 (host, PlayerJoin) or 1011 (self). Either
  is admitted by S2; the answer settles whether `FUN_00836520`'s `+0x5c8` short-cut ever fires on the host.
- **E5 Team placement.** Lobby screenshots of H after the countdown: L and B on one team (`StateBalancingTeams`
  mode 1). If not, read the teams service `+0x74` on H (mode 2 = fill-up ignores groups) → rules dependent.
- **E6 NAT.** With H behind NAT the FollowLeader carries H's address as L saw it; B must probe (kind 8). LAN test
  first (all 192.168.188.x), then one client via the VM.

Smallest live run order: L0 (regression, case 6 with hook on) → E2 → full case 5 (L1: both in lobby within
15 s; L2: one team; L3: H dissolves → E1; L4: server log lines of S2) → L5 (public inviter: secede as before).

---

## 7. Order and dependencies

1. **S1 → S2 → T** (server first: they are testable without a game; `build/build.sh bots party-escort`).
2. **S3** (independent, small).
3. **H0 → H1** (needs the friend-list `invite_only` lookup; test E2 live).
4. **H2** (needs H1 on the leader; test case 5 live, E3/E4/E5).
5. **E1**, then **H3** only if needed.
6. Update `docs/research/host-migration.md` §6 pointer to this file and the README's hook flag list; keep
   `MigrateMatchLobby` and `PartyFollowsInvite` independent flags. (Superseded at the merge: both are always on.)

Each hook step is one file (`hooks/src/party_follow.rs`), one address-table edit, one config field; each server
step one function group plus one scenario. Nothing here changes migration cases 1–4, 9, 10.

---

## 8. Implementation notes (2026-10-22): where the code differs from the plan

Checked against `objdump` of `Blacklist_DX11_game.exe` while implementing; the code wins over §5 where they differ.

- **Verified as written:** the prologues of 0x00879b20, 0x00839280 and 0x008ae430; the call site 0x018dcc22; the host
  test pattern of H2(a) occurs exactly once in `.text` (0x008393d2); `vf08`'s goal choice and `local_11`; the party
  member list layout and the own-record comparison (`vf08`'s tail finds the leader's own record by comparing the
  record's u64 with `[[NetCore+0x5d8]+0x10]`, so the list includes the leader and the ids are the same kind).
- **H2(c) is not enough, `vf00` reads the goal too.** `vf00` picks the same goal (0x008ae52e..0x008ae581) and takes
  `bl = (goal+0x24 == 8) ? goal+0x10 : 0` to choose which member flag it waits for (`+9` private, `+10` public); the
  members answer a private FollowLeader with `MemberAction(1)` = `+9`. With the goal as the join goal `vf00` would wait
  for `+10`, never see it and kick everybody after 15 s. The done-step it calls (`0x008277a0`) reads the goal as well.
  So `party_follow.rs` shows the goal as a private custom match (and `NetCore+0x5c8` as 1) around **both** `vf08` and
  every `vf00` call, and puts both back after each call.
- **No `ESCORT_SYNC` latch.** The detours decide per call from the game's objects (`joined_private_match`: the match
  session is running, private, kind 0, someone else hosts it (host id known), the party is running and has other
  members). Equivalent, and nothing to reset.
- **H1's `inviter_is_private`** is looked up when the party is about to be left: the hook notes the inviter's name
  when the event is handed to the game (`uplay_r1_loader.rs`, one line), and asks for a fresh friend list (2 s limit,
  game thread) only in the case the leader has members. Unknown means the original secede runs. The party test uses
  the party session's running state and its `+0x5de` locked byte (as `FUN_00879b20` itself does); the
  `[[party+0x610]+4]` "not migrating" test of §5 was dropped (unverified, and a migrating party isn't running).
- **Pattern with a wildcard** for the call offset in the H2(a) pattern; it must lie within 0x1c0 bytes of `vf08`.
  All three prologues are checked before anything is patched or hooked; any difference skips the whole feature.
- **Server:** `escorts.rs` window `BOARDING_FOR` = 10 min as planned. `add_participants` also refuses a party member
  that adds itself before its leader was invited (no boarding yet), and logs `<member> follows its party leader
  <leader> into private match <room>`. S3 drops the takeover when the member adds itself to any session other than the
  one it split off. The synchronous `Storage::add_join_request` wrapper added by 8341913 went with `bring_party`.
- **Testbot (v)** is checked after the members joined (the match lists host, leader and both members), since a search
  lists only who is in the room at that moment.
- **Not done:** the live test of H3 (the return path); the hook has no UI/launcher switch yet
  (`PartyFollowsInvite = true` in `uplay.toml`). (Later: H3 measured and removed, the hook always on, no switch.)
- **Build note:** `hook_with_name` in `hooks/src/hooks.rs` became `pub(crate)` so `party_follow.rs` can use the `hook!`
  macro. A cross build in a git worktree needs the uncommitted `WINRES_TOOLKIT_PATH` support in `hooks/build.rs` of the
  main checkout.


## 9. Case matrix (agreed with the maintainer, 2026-10-23)

Principle: in a running match any loss of the host ends in a fall back into the lobby, and from there
the lobby rules below apply ("as for a lobby migration"). Every wait for a vanished host is cut to
12 s of silence (`HOST_SILENT_AFTER`, `resolve_silent_hosts`, applies to every takeover waiting for
its host, parties included); with the game's own detection (~10 s) a hard drop is taken over after
~14 s (measured for a public lobby).

| # | Case | Wanted behaviour | Mechanism | Status |
|---|---|---|---|---|
| 1 | Public lobby, no party, host leaves (active or hard) | New host, the lobby carries on | flag setter (`migrate_lobby.rs`, always on), SplitSession = migration, 7001 to the members, 12 s rule | live (4 clients), active < 1 s, hard ~14 s |
| 2 | Public lobby with one or more parties | Migration, **and every party stays intact** | Party sessions (kind 1) are separate session objects; the migration moves the match session only | **live 2026-10-24** (leader + member found the lobby through quick match, host killed: lobby migrated to a new session with both, the party session stayed alive with both) |
| 3 | Private match, no party | Dissolved; everyone falls back into their own online lobby | The game clears the migration flag of a private session itself (second writer `0x0085a626`); host dissolves, members see `LostConnectionToHost` | live (active; hard with 3 clients) |
| 4 | Private match with a party | Players without a party: own lobby. Players with a party: back into the party lobby | Outward: hook `party_follow` (always on) + server escorts. On host loss: privacy-aware flag setter (`code_cave.rs`), `follow_unblock.rs` (releases a stale follow task, drops the second follow signal `follow-dup`), server `bring_party_back` (2 s later, repeated every 5 s, off the CreateSession thread) | **live 2026-10-24**: outward and return (both end up together in the leader's new party) |
| 5 | Running match, host drops (crash or kill; there is no regular way to leave) | Fall back into the lobby; then case 1 (public) or 3/4 (private) | `migrate_lobby.rs` with the running-match check (formerly opt-in `MigrateRunningMatch`, now always on): the engine check (`0x00779580`) answers the flag for every non-private session, so the lobby state catches the migration and the players fall back into the migrated lobby | **live 2026-10-24** (4 clients, public match running, host `kill -9`: new session with all, "Lobby wurde gewechselt"); private running match with a party is covered by case 4 |
| 6 | **A party's host leaves the party** | The party carries on under a remaining member | Original (C + R): `+0x5a9 = (kind == 1)` so the host sends a plain leave ("Shutdown called on host."), not Dissolve; the members' `0x0084d220` starts the migration; the server sees one member's SplitSession (+ AddParticipants) = the new party (`session.md` §7.1, §7.2, R 2026-10-12/17). Hard kill: the member's connection times out ~10 s, then it carries on in a party of its own (`session.md` §7.3) | **live 2026-10-24**: leader killed with two remaining members: one hosts the new party session, the other is moved along (24 s) |

Answer to the open question "host leaves a party": the game provides a **migration**, not a
dissolution, for parties (the flag is set from the kind alone); a dissolution happens only for a
match (kind 0) or when the party has no other member.

## 10. Server hardening of 2026-10-24 (after the live round)

Done in the server (unit-tested, not yet live):

- **Return to the party** asks after 2 s and again every 15 s (was 5/10 s, then 5 s; a member whose game is still busy with the first invitation takes a second one as a new accept and leaves the party it just joined, seen live with the VM). An escort also lapses
  `BACK_WITHIN` (5 min) after the leader was seen leaving the match (`escorts::left`, called from
  `leave`): a party the leader opens hours later is no return, and members in another match are not
  invited. A leave the server never saw leaves the old behaviour (up to 4 h).
- **Notification 7001** stays queued while the member has no connection (reconnect) and is retried on
  every sweep until delivered or 60 s old.
- **Cascades**: a takeover that goes through late (`host_gone`) is noted as migrated, so a member's own
  split of the old session afterwards is answered with the new session instead of "aren't in; refused".
- **Invitations and join targets** pending for a session follow its host migration (`rebind_invites`,
  `join_targets::moved`); the invitation is re-sent from the new host (an invitation holds only while its
  sender hosts the room).
- **Matchmaking** no longer offers a lobby that migrated on, nor one whose host has been silent for
  12 s (`lobby_is_dead`). A host without a connection to look at (`user_silence` is `None`) is still
  offered. `JoinSession` itself is unchanged: its response has no fields, so there is nothing to redirect;
  a stale join into a dead room fails in the game as before.
- **Same-address return**: when a host's old connection finally expires while a new one of the same user
  exists (`stale_connection_handler`), a takeover parked for its rooms goes through (`host_left_rooms`)
  before the rooms end.

### Not done, with the trade-off

**Live host, unreachable for the members (network partition of 10-12 s).** The members' Storm elects S,
S calls `SplitSession`, the host still pings the server, so the takeover is parked and lapses after 60 s;
the members wait for a 7001 that never comes. Treating the takeover as done ~12 s after the split would
fix this, but the server cannot tell this case from S3: a member that secedes on its own after accepting an
invitation also splits a session whose host is alive, and the party must not follow it. Both look the same
to the server (one `SplitSession`, host pinging, the others silent; the followers make no call of their own).
Any time rule would drag the party out of its host's session in S3, which is live-proven and more common.
A possible signal, not verified: the host's game removing the lost members (`RemoveParticipants`) while a
takeover is parked. Left open until a partition has been captured on the wire.

**Partial partition, first splitter wins.** `Takeover` is kept per session. A second member splitting the
same session while a takeover is parked is taken for a follower and moved into the first splitter's
session (`taken_over_by`); a competing splitter with a larger group would lose. Requiring a confirmation by
a second member does not work either: with two or three players there is no second member, and a follower's
own split looks the same as a competitor's. Documented, not changed.

## 11. Known limits (Bekannte Grenzen)

- A **server restart during a migration** loses everything that lives only in memory: `TAKEOVERS`,
  `MIGRATION_NOTES`, `left`, `migrated` (and the new `host_migrated`), `ESCORTS`, `join_requests`, and the
  join targets. Members of a migration in flight then wait for a 7001 that never comes; a party's return
  is not asked for.
- The hook's **follow-dup** (`hooks/src/unblock_logic.rs`) drops the second `PartyFollowLeaderTask` within
  5 s of a finished follow, which also discards a legitimate second follow signal in that window.
- An escort whose leader's leave the server never saw (host-side `RemoveParticipants` only, or a hard
  kill of the leader) is not time-bounded by `BACK_WITHIN`; it still lapses after 4 h. The party return
  therefore checks the member itself: before every try (after `PARTY_BACK_DELAY`, and again at each retry)
  `ask_back` looks whether the member is still in a live private match (`113 == 0`, type 1) that the
  leader did not host (`match_still_running`). If so, that try is skipped with the log line
  `... skipped: N is still in match session M`; the escort survives until the `PARTY_BACK_TRIES` are used
  up, so a member who leaves the match in between is asked at the next try, and a member who stays is
  never pulled out of a running match.
- Searches filter on a silent host by `user_silence`; a host that pings late (packet loss) disappears from
  the lists for the length of the silence only.
- **No switches.** Since the merge the client hooks of this design are always on; there is no `uplay.toml` key to
  turn the lobby migration, the running-match fallback, `follow_unblock`/`follow-dup` or the party following off
  (only `DisableCrashFixes` and the `patch-free` build still leave the game's code alone, the latter for all of
  them). `PartyStaysAfterMatch` (H3) and `TraceMemberLeave` were removed.

### Mixed clients

Derived from the code, **not tested live** as a mixed setup. A player whose client doesn't have these hooks (an older
release, or the original game) runs the original code paths in a lobby with players who do:

- **Lobby, host leaves (public match).** The old client's match session keeps the original flag (`kind == 1`, 0 for a
  match): it raises `LostConnectionToHost` and falls back into its own online lobby, as in the original. The new
  clients migrate the lobby among themselves (SplitSession, 7001). If the old client is the host, its game dissolves
  the lobby (Shutdown and Dissolve, DeleteSession) as in the original, so nobody migrates.
- **Running public match, host drops.** The old client is sent to its party lobby (the engine's check answers by
  kind, F8 of `running-match-host-loss.md`); the new ones fall back into the migrated lobby.
- **Party leader with an old client** accepting an invitation into someone else's private match secedes, as in the
  original: the party falls apart and the members migrate it on without the leader. The other way round needs no
  hook on the members or the match's host (H1/H2 run on the leader only), so a new leader takes old members along.
- **Return after the private match.** The server's `bring_party_back` invites the members into the leader's new
  party. A member with an old client without `follow_unblock`/`follow-dup` may be unable to accept that invitation:
  live case 15 (§12, a VM with a stale DLL) logged `Invitation accepted` three times and
  `NetResultRdvSession code=80000003` without a request to the server, while the member with the fix joined at once.

## 12. Live round of 2026-10-24 (real clients: host, guest, client2, VM)

Server at `b4b7f56`, hooks as before. `migration-test.sh` options: `HART`, `VIER`, `PARTY`, `PARTY_PUBLIC`,
`PARTY3`, `PARTY_VM`, `IM_MATCH`, `KILL_NACH`, `KILL_WER`, `RESTART_GAST`, `KASKADE`, `INVITE_VOR`, `PLAYERS`.

| # | Case | Result |
|---|---|---|
| 4 | private match with party, host leaves (active) | green, party 2092 / 2168 together again |
| 5 | running public match, 4 clients, host `kill -9` | green, lobby 2105 with all three |
| 6 | party host `kill -9`, two members left | green, new party in 23 s |
| 16 | private match with party, host `kill -9` (lobby) | green, party 2071 / 2176 |
| 1 | running **private** match with party, host `kill -9` | green, party 2090 (the match really ran: loadout screen, round clock) |
| 2, 21 | host `kill -9` at the countdown "match starts in 04" | green, lobby 2125 with all three |
| 10 | cascade: the new host is killed 20 s after the migration | green, lobby 2137 with the other two in 12 s |
| 8 | party leader killed while the party is in somebody's private running match | green: the member stays in the match (round clock running), carries on in a party of its own |
| 7 | the killed leader restarts and creates a party | found and fixed (`b4b7f56`): the member was pulled out of the running match; now `skipped: 1011 is still in match session N` in all three tries |
| 11 | open invitation before the migration | green: `1 pending invitations to session 2195 now name 2202` |
| 2 (public lobby, party) | host `kill -9` | green again with the final server: lobby 2252 with both, party 2249 intact |

Not conclusive or not run:

- **15 (party of three): solved, a stale DLL on the VM.** The VM has no launcher and kept an old
  `uplay_r1_loader.dll` (9.3 MB, no `follow_unblock`, no `follow-dup`, no quick start). Without the fix for the
  stuck follow task its client could not take the return invitation: it logged `Invitation accepted` three times
  and `NetResultRdvSession code=80000003` locally, without a request to the server, while client2 (with the
  fix) joined at the first try. Reproduced three times with the guest as leader and once with client2 as leader.
  After copying the test DLL to the VM (`afa222ea8b1c`) all three members (guest, client2, VM) were back in
  one party after the host had left the private match (party 2389). Not a server bug. `migration-test.sh` now
  warns when the VM's DLL differs from the host's. (Late invitations of a third member while the VM was still
  starting up also made it secede at once; wait until it has its own stable session.)
- **17 (leader leaves the match by menu): green, both answers.** Client2 as party leader (`LEADER=client2`,
  `PartyFollowsInvite = true`), guest as member, private match of the host. `tap esc` ends the game of any client
  that is not the host (0.4 s later, "normal exit"), and `tap down` opens the first menu item instead of
  moving the cursor, so the lobby menu is operated with the control channel's `click` (fractions of the
  window: "match verlassen" at 0.12 / 0.424, "party verlassen" at 0.31 / 0.724). The game asks "You are leaving
  the match. Do you want to leave your party or take it along?" with three answers.
  - *Take the party along* (default, Enter): leader and guest leave the match together, party 2398 stays
    intact, the host is left alone in the match. No return invitation needed.
  - *Leave the party*: only the leader leaves; the guest stays in the match. The server asks the guest back three
    times and skips each try (`skipped: 1010 is still in match session 2401`, the fix of case 7), so nobody is
    pulled out of the match.
- **6 (second match from the migrated lobby).** `players 3` crashes the game in a public SvM lobby
  (access violation in `FUN_007d72f0` = `NetServiceTeams::vf35`, which copies `param_1[0x19]` team entries
  into a service array without a bounds check). Test tool artefact; whether a real match start can trigger it
  is unknown.
- **Crash of the new host** (one run of case 5 with party and four players): null read in the package
  reader (`ULinkerLoad`, `FArchiveFileReaderLoadLinear`) three seconds after the guest took the lobby over
  while the level was being unloaded. Not reproduced in the next runs.
- **Stalled election** (one run with only three players in a lobby whose `players 4` had been set): both
  members saw `HostMigrationStart` but no `HostElected` followed; one left after 19 s, the other did nothing.
  Not reproduced; suspected to depend on the lobby being in the "waiting for players" state.
- **Dropped by the maintainer (2026-10-24): network partition and partial partition (3, 4).** The traffic of all
  players is relayed through the server, so a member losing the host while the host stays reachable for the
  others is very unlikely. The two cases of section 10 stay documented as known limits, with no further work.
- Not testable here: host back with the same address (19:
  a game restart takes longer than 60 s), round end and result screen (a round lasts 10 min), coop (13)
  and mixed populations (14), both deferred by the maintainer.
