# Host migration and parties: architecture

Status: design, 2026-10-22, from PR #8 (MPW1412), for the DX11 build (`Blacklist_DX11_game.exe`, addresses as
loaded, image base 0x400000) and the dev server. Markers: **R** seen live, **C** read from the code (decompiled
code and disassembly), **I** inferred, **V** guess. Every address below was checked in the decompiled code or the
disassembly on 2026-10-22.

Sections: 1 goals · 2 findings (questions A–F) · 3 architecture · 4 decisions · 5 implementation tasks ·
6 open points for live tests · 7 order and dependencies · 8 implementation notes · 9 case matrix ·
10 server rules · 11 known limits · 12 live round.

> **How it ships: one setting, on by default (0.5.0).** The three client hooks of this design go together:
> `migrate_lobby.rs` (the match lobby migration and the running-match fallback), `party_follow.rs` (the
> leader's party following an invitation, H1/H2) and `follow_unblock.rs` (with `follow-dup`). They're on unless a
> player turns them off in the launcher under **Settings › Game › Host migration**, stored as
> `DisableHostMigration = true` in `uplay.toml`. (During 0.5.0's development they were a beta, `[Beta] HostMigration`;
> that key is ignored now.)
>
> - **DX9 and DX11, all or nothing.** On any build where one piece isn't found, none of the three is
>   installed (one warning in the log). Not in the `patch-free` build.
> - **Found by signature.** The game's addresses are found by signature (`hooks/hooks-addresses`, reported by
>   `check_crash_fixes`), not taken from a fixed table. The addresses in this document are where those signatures
>   land in the DX11 build.
> - The old opt-in keys `MigrateMatchLobby`, `MigrateRunningMatch` and `PartyFollowsInvite` are gone (a file that
>   still has them is read as before, the keys are ignored); `PartyStaysAfterMatch` (H3) and `TraceMemberLeave`
>   (`member_trace.rs`) were removed. The design and the live reports below keep those names where they describe
>   what was run at the time. Players without it: §11, "Mixed clients".

---

## 1. Goals and non-goals

Goals (the cases the design must cover; expected behaviour in §3.4):

1. Public lobby, host leaves on purpose: the lobby carries on under a new host (live, R 2026-10-22).
2. Public lobby, host drops hard: the same, after the host has been silent for 20 s (live, R).
3. Private match, host leaves on purpose: no migration (the game's own behaviour; wanted). Everyone lands alone
   in their own party ("Your connection to the host was lost").
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

Non-goals: a host leaving a running match (the match ends for everyone; not changed, but see §9 case 5 for a
running public match whose host drops); the DX9 build (nothing is installed there); parties larger than the
game's eight seats.

---

## 2. Findings from the decompiled game

### 2.1 Objects and fields used below (C)

| Object | Where | Fields |
|---|---|---|
| `NetCore` | `[0x3383df4]` | `+0x5cc` NetServiceManager; `+0x5d0` match session (`NetGameSession*`, kind 0); `+0x5d4` party session (`NetPartySession*`, kind 1); `+0x5d8` the local player's `NetPlayerDescriptor*`; `+0x5c8` byte "private custom match" (computed in `nsNetCore::StateInitEcho::vf08` 0x008351d0 as `goal+0x24 == 8 ? goal+0x10 : 0`) |
| `NetOnline` | `[0x3383df8]` | `+0x58` party controller (`nsOnlineParty` owner); `+0x4c` presence controller; `+0x48` lobby; `+0x18` match Rdv service; `+0x2c` online service |
| `NetSessionBase` (party or match) | | `+0x3fc` own `NetGameSessionDescriptor`; `+0x4ac` **PeerDescriptor of the session's host** (set from Storm's session info `+0xc8` in `FUN_00779620`, and from the join endpoint in `FUN_0086a1e0`); `+0x428/+0x42c` server session id; `+0x554` Storm session info (host test `FUN_01f92c70`: `[+8] != -1 && [+8] == [+4]`); `+0x55c` own Storm id; `+0x564` session id (u64, used as `party_session`); `+0x584/+0x588` NetPeer list (entries: `node+8` → record, `record+10` connected flag, `record+0xc` the peer's `NetPlayerDescriptor`); `+0x5a8` private; `+0x5a9` may migrate; `+0x5b4` capacity; `+0x5de` locked; `+0x5f0` kind; `+0x610` MigrationInfo |
| `NetPlayerDescriptor` | per player; local one at `NetCore+0x5d8` | `+0x10` pid (u64); `+0x78` **`party_session`** (u64; setter `FUN_007a1a90`); `+0x8c` rank (i32). Offsets follow from the balancing code (§2.2 B2) and the wire order of the player descriptor |
| party controller (`[0x3383df8+0x58]`) | | `+0x3c` goal queue; `+0x410` party size high-water mark (0 = never had a party); `+0x414` follow stage (0 none, 1–4 "leader is starting: create/join/quick/coop", 6 FollowLeader received, 7 leader action 3); `+0x420/+0x424` member list (sentinel/first; `node+4` next, `node+8` → member record `{u64 id, +9 synced-private, +10 synced-public}`); `+0x430` the FollowLeader endpoint (`NetSessionEndpoint`); `+0x434`, `+0x438` settings flags |
| presence controller (`[0x3383df8+0x4c]`) | | `+0x4e0` endpoint of the **anteroom** (room kind 1) found by the invite search; `+0x4e4` endpoint of the **match** (room kind 0) found by it (`nsOnlinePresence::StateAcceptInviteFindSession::vf00` 0x00850d00: property 0x71 = 113 → 0 ⇒ `+0x4e4`, 1 ⇒ `+0x4e0`); `+0x4d8` inviter id |
| `NetSessionEndpoint` (0x3c0 bytes; `FUN_0084ca80` from a search result, `FUN_00778720` from a FollowLeader) | | `+0x160` two `NetGameSessionDescriptor` (0x9c each, index = kind); `+0x298` capacity (8); `+0x2a0` private; `+0x2d4` "peer form" flag; `+0x2dc/+0x2e0` session id; `+0x2e4` `PeerDescriptor`; `+0x3b5`, `+0x3b7` route flags |
| goal object (`nsOnlineMatch` current goal at `owner+0x220` when `owner+0x328 ∈ 4..6`, else `owner+0x50` when `owner+0x158 ∈ 4..6`) | | `+0` goal id; `+4` name; `+8` task; `+0x10` first parameter byte (private for CustomMatch); `+0x24` parameter kind (8 = custom-match parameters, 0xb = join parameters) |

Goal ids (C, from the name tables at `0x031126f8..` and `0x03112450..`): party `Host 0, Join 1, LeaveThenHost 2,
Leave 3, FollowLeader 4, PartyWaitSync 5, SetupVoice 6`; match `QuickMatch 1, FailureCleanup 2, CustomMatch 3,
JoinMatch 4, LeaveMatch 5, LadderMatch 6, Shutdown 7`.

### 2.2 Answers to questions A–F

| Q | Answer | Marker | Where |
|---|---|---|---|
| **A1** How the leader tells the party when it hosts | `StateNetMatchEnter::vf08` (0x0088ce50) pushes the lobby's `eGoal_Login` with callback `FUN_00884a20`; on success that calls `FUN_00879840(task, 1)` = push **`eGoal_PartyWaitSync`** to the party controller if `param==0 || I host the party` (`FUN_0078a330`). The party FSM's goal dispatch (`nsOnlineParty::StateIdle::vf02` 0x008601d0) sends goal 5 to **`StateLeaderPartyWaitSync`** on the leader; on a member it sends `MemberAction(2)` and goes to `StateSetupVoice`. | C | 0x0088ce50, 0x00884a20, 0x008601d0 |
| **A2** The FollowLeader send | `StateLeaderPartyWaitSync::vf08` (0x00839280): timer 15 s (`this+0x38 = 15.0`); `local_11 = (goal+0x24 == 8) ? goal+0x10 : 0`; if set: the match session (`NetCore+0x5d0`) must be `InSession` **and I must be its host** (`FUN_01f92c70` at 0x008393d8); then `NetMsgParty_FollowLeader` is built by `FUN_00828500(this=buf, descriptor = session+0x3fc, peer = session+0x4ac, private = session+0x5a8)`, the party service is fetched by `NetServiceManager::vf41` (`[NetCore+0x5cc]`, vtable `+0xa4`) and the message broadcast with `FUN_007c27f0(this=service, msg, -1)`, destructor `FUN_00826c40`. Then the own member record's `+9` (private) or `+10` (public) flag is set. Message fields (`FUN_0078b360`): descriptor `+0x10`, u32 `+0xb0`, PeerDescriptor `+0xc0`, bool `+0x15c` (wire layout in §3.5). | C | 0x00839280 |
| **A3** The wait | `StateLeaderPartyWaitSync::vf00` (0x008ae430): fails with 0x40000004 when `NetCore+0x5c8 == 0` (unless the lobby is in a special state, `FUN_007a5bc0`); counts the timer down; done (`FUN_008277a0`) when every member record has flag `+9` (private) / `+10` (public); on timeout kicks the others (`FUN_007d4080`, "Leader Kick"). | C | 0x008ae430 |
| **A4** Member side | The party message dispatcher `FUN_00879d80` (via `NetServiceParty::vf37` 0x0087aaa0): **FollowLeader** → `FUN_00778720` copies descriptor/peer/private into the endpoint at `ctrl+0x430` (sets `+0x298 = 8`, `+0x2a0`, `+0x3b4`), `+0x414 = 6`. **LeaderAction** (action 1, value v) → `+0x414 = v`; action 2 → cancel (`+0x414 = 0` if 1..4); action 3 → `+0x414 = 7`, match `+0x5e0 = 1` (the leader's "I leave the match, come along", see `member-stuck-after-private-match.md` F6). **MemberAction** (on the leader): 1 → member `+9 = 1`; 2 → `+10 = 1`; 3 → re-send FollowLeader to that member. A member reaches `StateFollowLeaderWaiting` only through **`eGoal_FollowLeader`** (`FUN_008796a0`), whose single caller is the UI task `UI::Task::PartyFollowLeaderTask::vf04` (0x018eae00), spawned by the party screen when `+0x414` is 1..4 (I). `StateFollowLeaderWaiting::vf00` (0x00827460): stage 0 → error; 1..4 → wait; 6 → `StateFollowLeaderJoinMatch::vf08` (0x00878940) → `FUN_00877370(task, ctrl+0x430)` = push **`eGoal_JoinMatch`** with that endpoint. On success the callback `FUN_00850b60` sends `MemberAction(1)` to the leader. (Measured later: a member can also be pulled along by Storm's `FollowHost` event, `member-stuck-after-private-match.md` §12.) | C | 0x00879d80, 0x00827460, 0x00878940, 0x00850b60 |
| **A5** Server calls | The join from an endpoint is `eGoal_JoinSpecificSession` (`FUN_0086a1e0`, the flag setter the lobby hook patches): `StateRegisterURLs` → `StateJoin`; `StateJoin::vf08` (0x0084da70) sends **nothing** for a private session (`+0x42b = 1`, waits for the 7003 gate), `JoinSession` for a public one. The **host** adds a completed peer: `FUN_00836520` (peer record complete) → `NetOnlineSessionServiceRdv::vf18` (`eGoal_PlayerJoin`) → `StatePlayerJoin::vf08` (0x008a0990) → Rdv `vf21` = **AddParticipants(session, [peer pid], private = session+0x5a8)**; skipped when the peer is me or `NetCore+0x5c8 != 0`. The follower itself calls RegisterURLs only (R). | C, R | 0x00836520, 0x008a0990 |
| **A6** `party_session` | Set once, when the player's own join into a **party** (kind 1) completes: `nsGameSession::StateAttemptingToJoin::vf11` (0x00869990) `FUN_007a1a90(NetCore.party+0x568, +0x564)` = the party's Storm session id. It is not touched by a match join, so every member carries its party's id into the match; the host's own descriptor carries its party id (R). | C, R | 0x00869990 |
| **B1** Teams | `nsOnlineLobby::StateBalancingTeams::vf08` (0x00896cf0): lobby state 4; on the host, teams service (`NetServiceManager::vf44`, `+0xb0`) `+0x74` mode ≠ 0 → `FUN_008938d0` → `FUN_00893720`: mode 1 → **`FUN_0088f380`**, mode 2 → `FUN_007c8e80` (plain fill-up). `vf00` waits until `FUN_007c0990` (enough players) and `FUN_007c0930` (team sizes differ by ≤ `FUN_007a75c0()`) hold. Mode source: not found (I: the rules' `teamComposition`). | C | 0x00896cf0, 0x008938d0, 0x00893720 |
| **B2** Party grouping | `FUN_0088f380` groups the lobby's player records by the u64 at **record+0x78 = `NetPlayerDescriptor.party_session`** (loop at the top: `local_4c = *(rec+0x78)`, compared with existing groups, new group otherwise); weights from `FUN_007c8f50` (rank `+0x8c` × 10 or a stored score). Groups of ≥ 2 are placed as a block on the team with room (`capacity/2 = session+0x5b4/2`), singles fill up. So **players whose descriptors carry the same `party_session` land in one team** — the host needs nothing from the server; the information comes with each joiner's descriptor over Storm. | C (I for "block") | 0x0088f380 |
| **B3** Why the invited leader isn't grouped today | The leader's descriptor keeps its *party* id; after the secede (C below) its party is a new solo session, the member's a different new one → no group. | C+R | server log, 22 Oct |
| **C1** What breaks the party when the leader accepts an invitation | The UI accept task `FUN_018dcba0` (unnamed `UI::Task`, DX11 0x018dcba0) calls **`FUN_00879b20(5)`** on the party controller (`push 5; call 0x879b20` at 0x018dcc2b, `ecx = [0x3383df8+0x58]`). `FUN_00879b20(param)`: param 5 → **`FUN_007a3ff0` = Storm `Secede` with reason "ForceMigrationSecede"** on the party session (0x007a3ff0); any other param on the leader → `NetMsgParty_LeaderAction(action 1, value param)` broadcast + member flags reset. The secede makes the leader host of a fresh session (local migration: RegisterURLs, Abandon, Split, AddParticipants, R) and the members migrate the rest (Split). | C, R | 0x018dcba0, 0x00879b20, 0x007a3ff0 |
| **C2** What the presence FSM would do without the secede | `StateAcceptInviteFindSession` fills `+0x4e4`/`+0x4e0` → `StateShutdownForInvite::vf08` (0x0087a240): **if (match goal id != 5 (LeaveMatch) && presence+0x4e4 != 0) → keep the party (`+0x34 = 0`)**, else push `eGoal_Leave` on the party. `StateAcceptInvite::vf08` (0x0087a2e0): with `+0x4e4` → `FUN_00877370(task, +0x4e4)` = `eGoal_JoinMatch` (the same join the party followers use), with `+0x4e0` only → `FUN_00879270` (`eGoal_Join`). So the game already has a "join the match, keep the party" path; only the UI's secede prevents it. | C | 0x0087a240, 0x0087a2e0 |
| **C3** The friends-list route | `UI::Task::JoinSessionTask::vf04` (0x018de3a0) does `FUN_00879b20(2)` (LeaderAction value 2 = "leader joins a session") and then `FUN_00877370(task, endpoint)` directly — the party is **kept** and told. It then fails at A2's host check / A3's `+0x5c8` on the leader, so the members' "following" ends in a kick (I). | C | 0x018de3a0 |
| **C4** Uplay party invitations | `nsNetOnlineUplayPC::StateInvitingParty` runs `UPLAY_PARTY_InvitePartyToGame` (0x00808a80) = the Uplay overlay's "invite my Uplay party" and is a no-op in the hook (`hooks/src/uplay_r1_loader/party.rs`: answers success, sends nothing). `UPLAY_PARTY_RespondToGameInvite` is a stub. The `PartyGameInviteAccepted` event (`InviteAcceptEvent = "Party"` in `uplay.toml`) only changes which UI route an accepted invitation takes; no Uplay party exists on our side. So: the Uplay path is dead here and the game's own party mechanism (A) is the only one. | C | `party.rs`, `hooks-config` |
| **C5** How a host invites | One invitation per friend (`UPLAY_FRIENDS_InviteToGame` → server `Friends.Invite`, bound to the match room). There is no "invite the party" in our stack (C4). The host invites the leader; the party follows by the mechanism in §3. | C, R | `api.rs` `invite` |
| **D1** After the match | `nsOnlineMatch::StateLeaveNetworkedMatch::vf04` (0x007a7e00): result 2 → **`StateLeaveWithoutParty`** when the party session is `InSession`, else `StateDestroySession`. `StateLeaveWithoutParty::vf08` (0x00884b10) pushes **`eGoal_LeaveThenHost`** on the party; its `vf04` (0x007a7f70): result 2 → `StateDestroySession`, result 5 → **`StatePartyRestore`**. `StatePartyRestore::vf08` (0x00884b40): party `InSession` and the match goal was a private custom match (`goal+0x24 == 8 && goal+0x10`) → `FUN_00879840(task, 0)` = `eGoal_PartyWaitSync` (leader: `StateLeaderPartyWaitSync` again; members: `MemberAction(2)` + `StateSetupVoice`), else skip. | C | 0x007a7e00, 0x00884b10, 0x007a7f70, 0x00884b40 |
| **D2** When LeaveThenHost leaves | `nsOnlineParty::StateLeaveSession::vf08` (0x00878820): `if (ctrl+0x410 < 1) { if (match goal != 7 Shutdown) fail 0x80000003 }` else leave the party session (`FUN_0086a4f0`) and re-host (`StateHostSession`). `+0x410` is the party-size high-water mark, so a real party takes the **leave + host a new one** branch (R for the solo party: Abandon, then CreateSession). Whether a party with members survives the return (members migrate the old session, leader hosts a new one → two parties) is **not decidable statically** → §6 E1. | C, V | 0x00878820 |
| **D3** Why the live case needed a new party | The party was already gone (C1): the leader's "party" was the solo session after the secede; LeaveThenHost replaced it by a new one. | C, R | |
| **E** Storm vs server | Storm only (server blind): party messages (FollowLeader, LeaderAction, MemberAction, PartyState memento), the Storm join itself (ticket, join accept, peer connect), the kick, the team balancing and the teams memento, host election. Server: RegisterURLs, SearchSessionsWithParticipants (invite), AddParticipants (host for followers, 7003 to them), JoinSession (public), Leave/Abandon/Split/RemoveParticipants, SetSession (advertise). **The server alone cannot** make a game join anything (only an invitation or 7003 move a game), cannot send party messages, cannot set a descriptor field. **A client hook can** change the leader's decisions (secede, host check) and reuse the game's own send code. **Impossible**: a member joining a private match it has no endpoint for; grouping players without equal `party_session`. | C, R | |
| **F1** Public lobby migration and party ids | A party never enters a public lobby together (R: for a public inviter the leader secedes, or the members are kicked), so migrated members carry only their own ids; `party_session` is untouched by migration (A6). The teams objects (`NetServiceTeams`, among the service objects `NetServiceManager::vf52/vf53` hand to the new master, migration steps 8/9) move with the lobby (I); `StateBalancingTeams` reruns on the new host when the countdown restarts. | C, I | `docs/research/host-migration.md` §3 |
| **F2** Private match dissolve with a party inside | Each member's match FSM runs D1 with the party session still `InSession` (the party's Storm connections survive a match, R) → `LeaveWithoutParty` → D2 → the same open point as the return in case 5 (§6 E1). The server sees only the match's Leave/Delete; the party session is untouched. | C, R | |

### 2.3 The live run of 2026-10-22 read against the code (R)

Server log of the run: the leader's `SearchSessionsWithParticipants` for the host → "invited room M plus anteroom →
M (113 ⇒ 0), A (113 ⇒ 1)"; then **the member's `SplitSession(P)`** (the member saw the leader's Storm secede) and
**the leader's `AbandonSession(P)`, `SplitSession(P)` → P', `AddParticipants(P', [leader])`,
`RemoveParticipants(P', [member])`** (the leader's local migration after its own secede), then the leader's
`AddParticipants(M, private [leader])` + 7003. That is C1 to the letter. After the match: the leader's
`LeaveSession(M)`, `AbandonSession(P')`, `CreateSession` → a new party = D2's leave-then-host.

---

## 3. Architecture

### 3.1 Components and responsibilities

| Component | Owns | Changes in this design |
|---|---|---|
| **Game** (unchanged code paths) | Party messages, Storm joins, team balancing by `party_session`, the party return (`PartyRestore`/`PartyWaitSync`) | none, but relied on |
| **Client hook** (`hooks/src/party_follow.rs`; part of host migration; DX9 and DX11) | On the **leader** only: (H1) don't secede when accepting an invitation into a private match while the party has members — tell the party "I'm joining a session" instead (LeaderAction 2, what the friends-list route does); (H2) let `StateLeaderPartyWaitSync` send FollowLeader for a match the leader *joined* (not hosts): skip the host check, present the join as a private custom match to `vf08`/`vf00`. Members and the match host need **no** hook (their code paths are the original ones). | new file + 2 detours + 1 two-byte patch, applied around the hook's own call only |
| **Server** (`dedicated_server/src/game_session.rs`, `escorts.rs`) | Admission: a member of the invited leader's party may be added to the private match (by the match host's `AddParticipants`, or by itself) and gets the 7003 gate (rules in §10); a member's split of a party whose leader was only seceded from doesn't make a takeover. Already there: host migration (SplitSession takeover, 7001, silent-host takeover). | small, see §5 S1–S4 |
| **Testbot** | Scenarios for the server rules | §5 |

### 3.2 Data flow, case 5 (party into a third player's private match), numbered

Players: H = match host (private match M with anteroom A), L = party leader (party P), B = member (in P). Hook on L
(and on everyone, harmless).

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
   because B is an escort of L for M (S1) and passes the checks of §10; B is noted as a match player; **7003
   (pid_source H, ui_param_1 B, ui_param_2 M)** to B. (If B's own game adds itself instead — the invitation route's
   habit — S2 allows that too and notifies B.) B's gate opens; B is in the lobby. B's callback sends
   `MemberAction(1)` → L's `vf00` sees B's flag → sync done (no kick).
10. Lobby: `StateBalancingTeams` on H groups L and B by `party_session = P` → **one team** (B2).
11. Match ends / H dissolves the lobby: L and B run D1: party `InSession` → `LeaveWithoutParty` →
    `LeaveThenHost` → D2. Expected (I, to be verified §6 E1): both are back in P. (Measured: they are not; the
    server brings the members back, S4.)

### 3.3 State machines touched (summary)

Leader, presence: `Idle → AcceptInviteFindSession → ShutdownForInvite (keep) → AcceptInvite (JoinMatch) → Idle`.
Leader, party: `Idle → LeaderPartyWaitSync (H2) → SetupVoice → Idle`.
Member, party: `Idle → FollowLeaderWaiting (stage 2) → FollowLeaderJoinMatch → (match FSM) → … → SetupVoice → Idle`.
Both, match: `JoinMatch → NetSetupVoice → NetMatchEnter → … → LeaveNetworkedMatch → LeaveWithoutParty → PartyRestore/DestroySession`.

### 3.4 Expected behaviour per case

| Case | Behaviour | Mechanism | Server / hook change | Test |
|---|---|---|---|---|
| 1 public, host leaves | lobby migrates, members told by 7001 | existing (`split_session` takeover, lobby migration hook) | none | existing `host-migration*`; live R |
| 2 public, host drops | same after 20 s of silence | existing `resolve_silent_hosts` | none | existing; live R |
| 3 private, host leaves | dissolve, everyone alone in own party | game (`+0x5a9` 0 for private) | none | live R |
| 4 private, host drops | same | game | none | live R |
| 1–4 with party members inside | public: no party exists inside (8); private: members keep their party session, return per D1/D2 | game | none; E1 decides H3 | §6 E1 |
| 5 party → third player's private match | together, one team, back together | §3.2 | H1, H2, S1, S2, S4 | testbot below; live L1–L4 |
| 6 leader hosts private match | unchanged | H1 only on action 5; H2 only when not host | none in effect | live L0 (regression) |
| 7 member invited | member secedes (original), party carries on | game; S3 keeps the server's takeover bookkeeping right | S3 | testbot |
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
   path (goal fields presented as a private custom match, host-check jump NOPed for that call) instead of
   rebuilding the message in Rust: fewer offsets to get wrong, and the own-flag/timer bookkeeping stays the game's.
4. **`+0x5c8` is presented as 1 only while `vf00` runs** (set before, restored after each call), because its
   other readers (`FUN_00836520` 0x00836520, `nsNetSessionRdv::StateIdle::vf02` 0x0085b3f0,
   `nsOnlineLobby::StateLogin::vf05` 0x00873f00, `nsOnlineConnection::StateOnline::vf08` 0x0086d790,
   `nsGameSession::StateAttemptingToJoin::vf04` 0x007a3dc0) gate other behaviour.
5. **H1 only for a private inviter and only with members.** The hook checks the inviter's `invite_only` from the
   friend list it already fetches (`hooks/src/api.rs`, `invite_only` per friend); otherwise the original secede
   runs, so singles and public invitations are untouched (cases 8, 10).
6. **Server admission by "escort" bookkeeping, checked against friendship, blocks and the party.** The server
   notes, when the invited leader's friend search is answered, who is in the leader's party *then* (`board`); an
   escort may be added to that match for a few minutes, under the rules of §10. This is the one server mechanism;
   it is idempotent whether the host or the member itself calls `AddParticipants`.
7. **What went from the first party draft**: `bring_party` (invitations to members after the leader arrived —
   time-shifted, wrong `party_session`), the first `Trips::back/came` (the return by invitations, before E1 showed
   it is needed; it came back as S4), the `party-follows-leader` scenario in its first form, the `SetSession` hook
   in `api.rs`. **What stayed**: the `board` noting at the friend search (`board_party`, `Storage::hosted_parties`),
   renamed into `escorts.rs`.
8. **Rejected**: a static one-byte patch `push 5 → push 2` in the UI task (would change singles); re-sending 7003
   from the server on a timer (the host's `AddParticipants` is the game's own trigger, A5); migrating private
   matches (maintainer decision).

---

## 5. Implementation tasks (in order)

Conventions: hook code in Rust, `retour::static_detour` as in `hooks/src/hooks.rs` (`hook_with_name`), addresses
found by signature through `hooks/hooks-addresses` (both builds; `FollowDelegate` and the facade check have a form for
each, since DX9 compiles them differently), byte patches through
`crate::game_code()` + `find_once` as in `hooks/src/migrate_lobby.rs`. All thiscall: `this` in `ecx`, arguments
pushed right to left, callee cleans (`ret n`).

### H0 Switch

Planned as a `PartyFollowsInvite` key in `uplay.toml`. Superseded: `party_follow::install` runs unless host
migration is turned off (`DisableHostMigration = true`), together with the other two hooks of this design, and only
when all three can be installed.

### H1 Leader keeps its party when accepting an invitation into a private match

Detour `FUN_00879b20` (DX11 0x00879b20; `unsafe extern "thiscall" fn(*mut c_void, i32)`, `ret 4`). Its DX11
bytes start `55 8b ec 6a ff 68 b3 b9 65 02 … 8b f1 a1 f4 3d 38 03 33 db 3b c3 0f 84 11 01 00 00 8b 88 d4 05 00 00`
and it ends in `c2 04 00` at 0x879c7b. The single call site with 5 (at 0x018dcc22) is
`8b 15 f8 3d 38 03 8b 4a 58 6a 05 e8 ee ce f9 fe`.

```
fn leader_action(this, action):
    if action == 5 && party_has_other_members() && inviter_is_private():
        info!("party-follow: invitation accepted as party leader - keeping the party (LeaderAction 2 instead of secede)")
        return Original(this, 2)
    Original(this, action)
```
- `party_has_other_members()`: `core = [0x3383df4]`; `party = [core+0x5d4]` nonnull, in state `InSession`
  (read the state id the way the game does at 0x839307: `s = [party+0x10]`; if null the id is
  `[0x2cf5c50]`, else `id = *((*(vtable(s) + 0x30))(s))`, thiscall; compare with `[0x338d7a4]`), and not
  migrating. Then `ctrl = [[0x3383df8]+0x58]`, own id `*(u64*)([core+0x5d8]+0x10)`, walk `node = [ctrl+0x424]`
  until `node == ctrl+0x420` (`next = [node+4]`, `rec = [node+8]`), count records with `*(u64*)rec != own id`;
  true when `> 0`.
- `inviter_is_private()`: the hook records the sender of the invitation it delivered as
  `FriendsGameInviteAccepted` (`hooks/src/uplay_r1_loader.rs`, `into_friend_invite_accepted(ubi_name)`): store
  `ubi_name` + time in a `static Mutex<Option<(String, Instant)>>`; look the name up in the last friend list
  (`crate::api` friend entries, field `invite_only`); true when found and `invite_only`, and the event is
  younger than 60 s. Unknown → false (original secede).
- Tests (unit): the member-count walk on a hand-built list (two records, one own); the invite-record aging.
- Risk: `+0x5d4` null before the network is up (guard every pointer; on any null → original).

### H2 The sync state sends FollowLeader for a joined private match

(a) Two-byte patch in `StateLeaderPartyWaitSync::vf08`: signature (found once in `.text`, within 0x1c0 bytes of
`vf08`) `8d 8f 54 05 00 00 e8 ?? ?? ?? ?? 84 c0 74 5f 0f b6 8e a8 05 00 00` (= `lea ecx,[edi+0x554]; call
FUN_01f92c70; test al,al; je +0x5f; movzx ecx,[esi+0x5a8]`, at 0x008393d2); the `74 5f` (offset 13) becomes
`90 90`. Effect: the send no longer requires being the match's host; it still requires `local_11` and `InSession`.
After the `je` come the FollowLeader constructor (`0x828500`, with descriptor `+0x3fc`, host peer `+0x4ac` and
private `+0x5a8`), the party service lookup and the broadcast (`0x7c27f0`).

**The patch is not permanent.** It is written just before the hook's own call of the original `vf08` for a joined
private match (the `joined_private_match` case below) and put back right after that call, both checked by reading
the bytes back, as `migrate_lobby.rs` does. Every other run of `vf08` (the leader hosting its own match, a public
match) sees the original bytes.

(b) Detour `StateLeaderPartyWaitSync::vf08` (0x00839280, `fn(this)`):
```
fn sync_enter(this):
    if joined_private_match():                 # see below
        goal = current_match_goal()            # owner = [this+8]; +0x220 if [owner+0x328] in 4..=6 else +0x50 if [owner+0x158] in 4..=6 else null
        if goal: save (kind=[goal+0x24], p=[goal+0x10] byte); [goal+0x24]=8; [goal+0x10]=1
        patch the host test (a); Original(this); restore the host test
        if goal: restore
        info!("party-follow: FollowLeader sent for joined private match {id} to the party")
    else: Original(this)
```
`joined_private_match()`: `m = [core+0x5d0]` nonnull; its FSM state id == `[0x338d7a4]` (`nsGameSession::StateInSession`;
read it like the game: `s=[m+0x10]; id = *( (*(vtable(s)+0x30))(s) )`); `FUN_01f92c70(m+0x554) == 0` (not host;
call the game function, `thiscall`, no args); `[m+0x5a8] != 0` (private); `[m+0x5f0] == 0` (kind match);
`party_has_other_members()` from H1. The goal read (`[goal+0x24]`) is logged before/after for the live test.

(c) Detour `StateLeaderPartyWaitSync::vf00` (0x008ae430, `fn(this, dt: f32)` — the float is the first stack
argument, `ret 4`):
```
fn sync_tick(this, dt):
    if joined_private_match(): old=[core+0x5c8]; [core+0x5c8]=1; Original(this,dt); [core+0x5c8]=old
    else: Original(this, dt)
```
`[this+0x10]` is the state's pending transition (4 = error, 1/3 = done). The `+0x5c8` test in `vf00` is
`cmp byte [ecx+0x5c8],0` at 0x008ae4aa.

- Addresses: `hooks_addresses::host_migration::locate` finds the three functions, the host test and the
  globals by signature (`check_crash_fixes` reports them); `host_migration::install` reports a miss and, as for
  every piece of it, installs none of the three hooks when anything isn't found exactly once.
- Unit tests: the signatures' wildcards and offsets, and a build missing a piece (`hooks-addresses`).
- Risks: the JoinMatch goal's `+0x10` byte might be read elsewhere during `vf08` (it isn't: `vf08` is
  synchronous, C); `local_11` also makes `vf00` kick members that don't arrive within 15 s (original behaviour
  for a hosted private match, acceptable); DX9 unsupported.

### H3 (built, measured, removed: E1 showed the party doesn't survive the return, and H3 alone doesn't fix it)

Removed at the merge, with `PartyStaysAfterMatch` and its address: the return is the server's S4. What was built
and measured:

- **E1 measured live (23 Oct, morning):** the party leader (hook on) and its member joined the private match
  together, in one team, party session kept. When the host dissolved the lobby the leader ran
  `StateLeaveWithoutParty::vf08` → `LeaveThenHost` → `StateLeaveSession::vf08` and left the party (hosting a new
  one); the member (no hook) did not abandon, saw its party host go and migrated the old party alone. Two parties.
- **H3** detoured `nsOnlineMatch::StateLeaveWithoutParty::vf08` (0x00884b10, `fn(this)`; body `task =
  FUN_00829450(ecx=[0x3383df8]); [this+0x34] = task; FUN_00879460(ecx=[NetOnline+0x58], task)`, push
  `eGoal_LeaveThenHost`). When the party session was `InSession`, had other members, and the current match goal
  was `goal+0x24 == 8 && goal+0x10` (as `StatePartyRestore` asks), the hook pushed `PartyWaitSync(0)` instead
  (`FUN_00879840`, `ret 8`, in place of `FUN_00879460`, `ret 4`), as `StatePartyRestore::vf08` (0x00884b40) does.
- **Measured with H3 (later that morning):** H3 ran on the leader, but the member has no hook and does not answer;
  both clients hung on "joining" over the lobby and the leader removed the member after 15 s
  (`RemoveParticipants`). Hence S4.

### S1 Escorts: who may follow whom (server)

- `dedicated_server/src/escorts.rs`: `Escorts::board(leader, room, companions, now)`; `allowed(&self, room, member,
  now) -> bool` (a companion of any boarding into `room` younger than `BOARDING_FOR = 10 min`) and
  `boarded_by(room, member) -> Option<leader>`; bounds `MAX_COMPANIONS 8`, `MAX_TRIPS 1024`.
- `game_session.rs`: `board_party` (called from `search_sessions_with_participants` after the invitee is
  added) → `escorts::board`; `Storage::hosted_parties`.
- Unit tests: `escorts::tests` (board once, allowed within window, not after, not for a stranger, bounded).

### S2 Admission of escorts (server)

`add_participants`:
1. Self-add into a private room: `invited || party_of_host || escort` (an escort under the rules of §10).
2. Adding someone else: allow when `in_party_of(t, user_id) || friends || escort`, never past a block (keep
   `blocked_blocking`). An escort may be added only by the match's host or by itself.
3. Notification: `participant == user_id` → notify self when `via_invitation || escort`.
4. `session_events::joined(target, session, "party")` for escorts (the `via` chooser: escort → "party").
- Testbot `party-escort`: Host H (anteroom + private match M), Leader L (party P), Member B (joined P), Stray S.
  H invites L; L `search_with_participants([H])` → M present. **No** split/abandon by L (the party is kept). L
  `add_participants(M, [], [L])` → 7003. Then (i) `H.add_participants(M, [], [B])` succeeds and B gets 7003 with
  `ui_param_1 == B`, `ui_param_2 == M`; (ii) `H.add_participants(M, [], [S])` is refused; (iii) a second run where
  B adds itself (`B.add_participants(M, [], [B])`) succeeds and B gets 7003; (iv) a stranger's self-add is refused;
  (v) `L.search_with_participants([H])` lists H, L, B in M.
- Live criteria (server log): after L's search → `Adding L to invited room M`, `L accepts an invitation into
  private match M of H; its party: [B]` (board); **no** `User L abandons session P`, **no** `split session P`;
  `AddParticipants … session_id: M … private_participant_ids: [B]` (by H or B) followed by `Notifying B about
  session M: … ui_type: 7003, ui_param_1: B, ui_param_2: M`.

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
after the match (measured: the leader leaves the match, abandons its party, a member hosts the old party on, then the
leader's `CreateSession`). The match counts as over when its room is destroyed or the leader is no longer in it. Each
member gets an invitation from the leader to the new party, delivered as the requested join (no click, as `JoinNow`), if
online and not blocked, within the invitation rate limit; once per trip. The tries and the skips are in §10 and §11.
Log: `The match of <leader>'s party is over: <member> is asked back into party <P> (invitation <n>, requested)`.
Testbot: `party-return`.

### T Testbot bookkeeping

`tools/testbot/README.md`: `party-escort`, `party-member-leaves-for-match`, `party-return`; `main.rs` match arms
accordingly. `build/build.sh bots party-escort`.

---

## 6. Open points only a live test can settle (smallest experiment each)

- **E1 Return to the party (D2). Measured 2026-10-23: the party does not survive (two single parties); H3 alone does
  not help (members have no hook); the server brings the members back (S4).** Smallest: no hook needed. L hosts a
  private match with B in its party (case 6), L ends the lobby from the menu. Watch the server log for
  `AbandonSession`/`SplitSession` of P and the UI ("My Party 2/8" on both?). Variant with the hook: case 5 as in
  §3.2, H dissolves M.
- **E2 Members' UI reacts to LeaderAction value 2.** Smallest: with H1 only (H2 off), L accepts H's invitation;
  B's screen should show "following leader" (the task spawned); no member kick for 15 s is expected then
  (`vf00` errors out at `+0x5c8` without H2). If B shows nothing, the UI only reacts to value 1 → H1 sends
  `FUN_00879b20(1)` instead (one constant).
- **E3 `vf08`'s goal fields for a JoinMatch goal.** H2 logs `[goal+0x24]`/`[goal+0x10]` before tweaking; expected
  0xb/anything (I). If the original already yields `local_11 = 1`, the tweak can go.
- **E4 Who adds B.** Server log: `AddParticipants … [B]` — caller H (host, PlayerJoin) or B (self). Either is
  admitted by S2; the answer settles whether `FUN_00836520`'s `+0x5c8` short-cut ever fires on the host.
- **E5 Team placement.** Lobby screenshots of H after the countdown: L and B on one team (`StateBalancingTeams`
  mode 1). If not, read the teams service `+0x74` on H (mode 2 = fill-up ignores groups) → rules dependent.
- **E6 NAT.** With H behind NAT the FollowLeader carries H's address as L saw it; B must probe (kind 8). Test
  without NAT first, then with one client behind NAT.

Smallest live run order: L0 (regression, case 6 with hook on) → E2 → full case 5 (L1: both in lobby within
15 s; L2: one team; L3: H dissolves → E1; L4: server log lines of S2) → L5 (public inviter: secede as before).

---

## 7. Order and dependencies

1. **S1 → S2 → T** (server first: they are testable without a game; `build/build.sh bots party-escort`).
2. **S3** (independent, small).
3. **H0 → H1** (needs the friend-list `invite_only` lookup; test E2 live).
4. **H2** (needs H1 on the leader; test case 5 live, E3/E4/E5).
5. **E1**, then **H3** only if needed (it was built, did not help, and went; S4 instead).
6. Update `docs/research/host-migration.md` §6 pointer to this file.

Each hook step is one file (`hooks/src/party_follow.rs`) and its signatures; each server step one function group
plus one scenario. Nothing here changes migration cases 1–4, 9, 10.

---

## 8. Implementation notes (2026-10-22): where the code differs from the plan

Checked against the disassembly of `Blacklist_DX11_game.exe` while implementing; the code wins over §5 where they differ.

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
- **No latch.** The detours decide per call from the game's objects (`joined_private_match`: the match session is
  running, private, kind 0, someone else hosts it (host id known), the party is running and has other members).
  Equivalent, and nothing to reset.
- **H1's `inviter_is_private`** is looked up when the party is about to be left: the hook notes the inviter's name
  when the event is handed to the game (`uplay_r1_loader.rs`, one line), and asks for a fresh friend list (2 s limit,
  game thread) only in the case the leader has members. Unknown means the original secede runs. The party test uses
  the party session's running state and its `+0x5de` locked byte (as `FUN_00879b20` itself does); a separate
  "not migrating" test was dropped (unverified, and a migrating party isn't running).
- **Signatures, all or nothing.** The H2(a) pattern has a wildcard for the call offset and must lie within 0x1c0
  bytes of `vf08`. Every signature is found before anything is patched or hooked; any miss skips all of it
  (all three hooks).
- **Server:** `escorts.rs` window `BOARDING_FOR` = 10 min as planned. `add_participants` also refuses a party member
  that adds itself before its leader was invited (no boarding yet), and logs `<member> follows its party leader
  <leader> into private match <room>`. S3 drops the takeover when the member adds itself to any session other than the
  one it split off.
- **Testbot (v)** is checked after the members joined (the match lists host, leader and both members), since a search
  lists only who is in the room at that moment.
- **Build note:** `hook_with_name` in `hooks/src/hooks.rs` became `pub(crate)` so `party_follow.rs` can use the `hook!`
  macro.

## 9. Case matrix (agreed with the maintainer, 2026-10-23)

Principle: in a running match any loss of the host ends in a fall back into the lobby, and from there
the lobby rules below apply ("as for a lobby migration"). Every wait for a vanished host is cut to
20 s of silence, two missed pings (`HOST_SILENT_AFTER`, `resolve_silent_hosts`, applies to every takeover
waiting for its host, parties included). With the earlier 12 s limit and the game's own detection (~10 s) a hard
drop was taken over after ~14 s (measured for a public lobby); with 20 s expect a little over 20 s.

All of these need host migration on in every player's game (0.5.0 or later, not turned off) (§11, "Mixed clients").

| # | Case | Wanted behaviour | Mechanism | Status |
|---|---|---|---|---|
| 1 | Public lobby, no party, host leaves (active or hard) | New host, the lobby carries on | flag setter (`migrate_lobby.rs`), SplitSession = migration, 7001 to the members, silence rule | live (4 clients), active < 1 s, hard ~14 s (12 s rule) |
| 2 | Public lobby with one or more parties | Migration, **and every party stays intact** | Party sessions (kind 1) are separate session objects; the migration moves the match session only | **live 2026-10-24** (leader + member found the lobby through quick match, host killed: lobby migrated to a new session with both, the party session stayed alive with both) |
| 3 | Private match, no party | Dissolved; everyone falls back into their own online lobby | A private match session gets `+0x5a9 = 0` (the privacy-aware setter, as in the original game); host dissolves, members see `LostConnectionToHost` | live (active; hard with 3 clients) |
| 4 | Private match with a party | Players without a party: own lobby. Players with a party: back into the party lobby | Outward: hook `party_follow` + server escorts. On host loss: privacy-aware flag setter (`code_cave.rs`), `follow_unblock.rs` (releases a stale follow task, drops the second follow signal `follow-dup`), server `bring_party_back` (S4: 2 s later, then every 15 s, three tries, off the CreateSession thread) | **live 2026-10-24**: outward and return (both end up together in the leader's new party) |
| 5 | Running match, host drops (crash or kill; there is no regular way to leave) | Fall back into the lobby; then case 1 (public) or 3/4 (private) | `migrate_lobby.rs` with the running-match check: the engine check (`0x00779580`) answers the flag for every non-private session, so the lobby state catches the migration and the players fall back into the migrated lobby (`running-match-host-loss.md`) | **live 2026-10-24** (4 clients, public match running, host `kill -9`: new session with all, "Lobby has changed"); private running match with a party is covered by case 4 |
| 6 | **A party's host leaves the party** | The party carries on under a remaining member | Original (C + R): `+0x5a9 = (kind == 1)` so the host sends a plain leave ("Shutdown called on host."), not Dissolve; the members' `0x0084d220` starts the migration; the server sees one member's SplitSession (+ AddParticipants) = the new party (R 2026-10-12/17). Hard kill: the member's connection times out ~10 s, then it carries on in a party of its own | **live 2026-10-24**: leader killed with two remaining members: one hosts the new party session, the other is moved along (24 s) |

Answer to the open question "host leaves a party": the game provides a **migration**, not a
dissolution, for parties (the flag is set from the kind alone); a dissolution happens only for a
match (kind 0) or when the party has no other member.

## 10. Server rules (hardening of 2026-10-24 and after)

Takeovers (`split_session`, `resolve_silent_hosts`):

- **Who may take over.** Only a member still in the room, or one that abandoned it in the last few seconds (the
  abandon variant), can take a session over. Anyone else's split is not a takeover.
- **The first claim wins.** The first member's claim on a session holds; a later split of the same session doesn't
  overwrite it and is taken for a follower (moved into the first splitter's session, `taken_over_by`).
- **A silent host.** A host counts as silent after 20 s without a ping (two missed pings, `HOST_SILENT_AFTER`); a
  parked takeover then goes through. Matchmaking stops offering a lobby that migrated on, or one whose host is
  silent (`lobby_is_dead`). The silence is that of the connection that made the room (`room_silence`): a host
  whose game dropped and signed in again talks on its new connection, but the rooms the old one made have
  nobody behind them, so their takeovers go through after 20 s instead of lapsing when the old connection
  expires a minute later. A room whose host has no connection to look at is still offered.
- **Along the chain.** A late split of a room that moved more than once (its new host gone too) is answered
  with the room the player is in now (up to eight moves).
  `JoinSession` itself is unchanged: its response has no fields, so there is nothing to redirect; a stale join into
  a dead room fails in the game as before.
- **The host deletes the session.** A pending takeover is cleared: a dissolve is not a migration.
- **Cascades**: a takeover that goes through late (`host_gone`) is noted as migrated, so a member's own split of the
  old session afterwards is answered with the new session instead of "aren't in; refused".
- **Notification 7001** stays queued while the member has no connection (reconnect) and is retried on every sweep
  until delivered or 60 s old.
- **Invitations and join targets** pending for a session follow its host migration (`rebind_invites`,
  `join_targets::moved`); the invitation is re-sent from the new host (an invitation holds only while its sender
  hosts the room).
- **Same-address return**: when a host's old connection finally expires while a new one of the same user exists
  (`stale_connection_handler`), a takeover parked for its rooms goes through (`host_left_rooms`) before the rooms end.

Parties into a private match (`add_participants`, `escorts.rs`):

- A party's members may follow their leader into a private match only if all of these hold: they were in the
  leader's party when the leader's invitation was answered (the boarding, `BOARDING_FOR` = 10 min), they are
  **still in the leader's party**, the **leader is in the match**, they are **friends of the match's host** and
  **neither has blocked the other**. Only the match's host or the member itself may add them.
- **Return to the party** (S4) asks after 2 s and again every 15 s, three tries (a member whose game is still busy
  with the first invitation takes a second one as a new accept and leaves the party it just joined, seen live). A
  member who is now in someone else's room is skipped. An escort also lapses `BACK_WITHIN` (5 min) after the leader
  was seen leaving the match (`escorts::left`, called from `leave`): a party the leader opens hours later is no
  return, and members in another match are not invited. A leave the server never saw leaves the old behaviour (up
  to 4 h).

### Not done, with the trade-off

**Live host, unreachable for the members (network partition of 10–20 s).** The members' Storm elects S,
S calls `SplitSession`, the host still pings the server, so the takeover is parked and lapses after 60 s;
the members wait for a 7001 that never comes. Treating the takeover as done shortly after the split would
fix this, but the server cannot tell this case from S3: a member that secedes on its own after accepting an
invitation also splits a session whose host is alive, and the party must not follow it. Both look the same
to the server (one `SplitSession`, host pinging, the others silent; the followers make no call of their own).
Any time rule would drag the party out of its host's session in S3, which is live-proven and more common.
A possible signal, not verified: the host's game removing the lost members (`RemoveParticipants`) while a
takeover is parked. Left open until a partition has been captured on the wire.

**Partial partition, first splitter wins.** A second member splitting the same session while a takeover is parked
is taken for a follower and moved into the first splitter's session; a competing splitter with a larger group
would lose. Requiring a confirmation by a second member does not work either: with two or three players there is no
second member, and a follower's own split looks the same as a competitor's. Documented, not changed.

## 11. Known limits

- A **server restart during a migration** loses everything that lives only in memory: `TAKEOVERS`,
  `MIGRATION_NOTES`, `left`, `migrated` (and `host_migrated`), `ESCORTS`, `join_requests`, and the
  join targets. Members of a migration in flight then wait for a 7001 that never comes; a party's return
  is not asked for.
- The hook's **follow-dup** (`hooks/src/unblock_logic.rs`) drops the second `PartyFollowLeaderTask` within
  5 s of a finished follow, which also discards a legitimate second follow signal in that window.
- An escort whose leader's leave the server never saw (host-side `RemoveParticipants` only, or a hard
  kill of the leader) is not time-bounded by `BACK_WITHIN`; it still lapses after 4 h. The party return
  therefore checks the member itself: before every try (after `PARTY_BACK_DELAY`, and again at each retry)
  `ask_back` looks whether the member is still in a live private match (`113 == 0`, type 1) that the
  leader did not host (`match_still_running`), or in someone else's room. If so, that try is skipped with the
  log line `... skipped: N is still in match session M`; the escort survives until the `PARTY_BACK_TRIES` are used
  up, so a member who leaves the match in between is asked at the next try, and a member who stays is
  never pulled out of a running match.
- Searches filter on a silent host by its room's connection (`room_silence`); a host that pings late (packet loss) disappears from
  the lists for the length of the silence only.
- **One switch for all three.** The client hooks of this design go on and off together with the "Host migration"
  setting. There is no separate switch for the lobby migration, the running-match fallback, `follow_unblock`/`follow-dup`
  or the party following. `DisableCrashFixes` and the `patch-free` build still leave the game's code alone, the
  latter for all of them. On DX9, or when any piece isn't found, none of the three is installed.

### Mixed clients

Derived from the code, **not tested live** as a mixed setup. A player without the hooks (host migration off, DX9, an
older release, or the original game) runs the original code paths in a lobby with players who have them:

- **Lobby, host leaves (public match).** That player's match session keeps the original flag (`kind == 1`, 0 for a
  match): it raises `LostConnectionToHost` and falls back into its own online lobby, as in the original. The players
  with host migration migrate the lobby among themselves (SplitSession, 7001). If the host is the one without it, its game
  dissolves the lobby (Shutdown and Dissolve, DeleteSession) as in the original, so nobody migrates.
- **Running public match, host drops.** The player without it is sent to its party lobby (the engine's check
  answers by kind, F8 of `running-match-host-loss.md`); the others fall back into the migrated lobby.
- **Party leader without it** accepting an invitation into someone else's private match secedes, as in the
  original: the party falls apart and the members migrate it on without the leader. The other way round needs no
  hook on the members or the match's host (H1/H2 run on the leader only), so a leader with it takes members
  without it along.
- **Return after the private match.** The server's `bring_party_back` invites the members into the leader's new
  party. A member without `follow_unblock`/`follow-dup` may be unable to accept that invitation: in live case 15
  (§12, one test client had an outdated DLL) it logged `Invitation accepted` three times and
  `NetResultRdvSession code=80000003` without a request to the server, while the member with the fix joined at once.

## 12. Live round of 2026-10-24 (four real clients)

Server and hooks as of that day.

| # | Case | Result |
|---|---|---|
| 4 | private match with party, host leaves (active) | green, party together again |
| 5 | running public match, 4 clients, host `kill -9` | green, migrated lobby with all three |
| 6 | party host `kill -9`, two members left | green, new party in 23 s |
| 16 | private match with party, host `kill -9` (lobby) | green, party together again |
| 1 | running **private** match with party, host `kill -9` | green, party together again (the match really ran: loadout screen, round clock) |
| 2, 21 | host `kill -9` at the countdown "match starts in 04" | green, migrated lobby with all three |
| 10 | cascade: the new host is killed 20 s after the migration | green, lobby with the other two in 12 s |
| 8 | party leader killed while the party is in somebody's private running match | green: the member stays in the match (round clock running), carries on in a party of its own |
| 7 | the killed leader restarts and creates a party | found and fixed: the member was pulled out of the running match; now `skipped: <member> is still in match session N` in all three tries |
| 11 | open invitation before the migration | green: `1 pending invitations to session M now name M'` |
| 2 (public lobby, party) | host `kill -9` | green again with the final server: migrated lobby with both, party intact |

Not conclusive or not run:

- **15 (party of three): solved, one test client had an outdated DLL** (without `follow_unblock` and `follow-dup`).
  Without the fix for the stuck follow task that client could not take the return invitation: it logged `Invitation
  accepted` three times and `NetResultRdvSession code=80000003` locally, without a request to the server, while the
  member with the fix joined at the first try. Reproduced three times with one leader and once with another. With the
  current DLL on all clients, all three members were back in one party after the host had left the private match.
  Not a server bug. (Late invitations of a third member while its game was still starting up also made it secede at
  once; wait until it has its own stable session.)
- **17 (leader leaves the match by menu): green, both answers.** Party leader and member in the host's private
  match. The game asks "You are leaving the match. Do you want to leave your party or take it along?" with three
  answers.
  - *Take the party along* (default): leader and member leave the match together, the party stays intact, the host
    is left alone in the match. No return invitation needed.
  - *Leave the party*: only the leader leaves; the member stays in the match. The server asks the member back three
    times and skips each try (`skipped: … is still in match session …`, the fix of case 7), so nobody is pulled out
    of the match.
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
  others is very unlikely. The two cases of §10 stay documented as known limits, with no further work.
- Not testable here: host back with the same address (19: a game restart takes longer than 60 s), round end and
  result screen (a round lasts 10 min), coop (13) and mixed populations (14), both deferred by the maintainer.
