# A party member stays in the lobby of a dissolved private match

Status: analysis, 2026-10-23, DX11 build (`Blacklist_DX11_game.exe`, addresses as loaded, image base 0x400000),
branch `fix/host-migration-client`. No live test was run for this document. Markers as in
`host-and-party-migration.md`: **C** read from the decompiled text
(`~/Games/scbl/ghidra-jobs/ergebnisse/dx11-alles-benannt.c`) or `objdump`, **R** seen in the logs of
`~/Games/scbl/logs/migration-20261023-195439/` (and the earlier runs of 23.10. 09:23-10:02), **I** inferred,
**V** guess.

> **Since the merge (2026-10-24) the hooks are not configurable any more: they are always on.** The `uplay.toml`
> keys `MigrateMatchLobby`, `MigrateRunningMatch` and `PartyFollowsInvite` are gone (a file that still has them is
> read as before, the keys are ignored); `PartyStaysAfterMatch` (H3) and `TraceMemberLeave` (`member_trace.rs`) were
> removed. `follow_unblock.rs` (with `follow-dup`) is installed unconditionally on the DX11 build and sets its own
> observer hooks on `PartyFollowLeaderTask::vf04` and `FollowDelegate` (there is no `member_trace` to share them
> with); H3's `leave_without_party` detour (H7 below) is gone. The measurement sections below describe the flags as
> they were at the time.

## 1. Summary

- **Case 4** (private match, party leader L and member B inside, host H leaves on purpose): L leaves the match and
  hosts a new party; B never leaves the match session. The server never sees `LeaveSession`/`AbandonSession` of the
  match from B (R, also in the runs at 09:28, 09:45, 09:54 and 10:00, so it does not come from the escort invitation
  that the server delivers later).
- Both clients get the same stimulus: H's Storm leave packet carries the reason text **"Shutdown and Dissolve called
  on host"** (bit-shifted ASCII in the `recvfrom` payload, identical in both logs, R) and `storm::echo::session::
  outEvent::Leave` for the match session follows. L reacts within 5.5 ms (`NetResultLobby 0x80000002` = lobby logout,
  then `LeaveSession 1881`); B's main thread does nothing visible (R).
- **The game never leaves a match by itself from the net code.** Leaving is always a goal (`eGoal_LeaveMatch` 5 or
  `eGoal_Shutdown` 7) pushed on the match controller, or the match FSM's own failure conversion. What triggers it
  for B is unknown (§5); what we know is that the trigger that fired on L did not fire on B.
- The party's designed way to take a member out of a match (`LeaderAction(3)`, §3 F6) is only sent when the leader
  leaves by the menu entry, not on the host-loss path L took. So B has to rely on its own host-loss path, like any guest.
- **The "Versuch, Spielsitzung beizutreten ..." mask is not the cause.** It is on L's screen *before* the host
  leaves too (`haenger/20261023-200218-migration-vorher-gast.png`, R). It is the leftover join spinner
  (`LocID_SPIN_Joining`), see F8.
- Server messages cannot help: the only notifications the game acts on are 7001 and 7003 (F10).
- Smallest robust remedy: a **member-side watchdog in the hooks** that pushes `eGoal_Shutdown` (`0x00878680`) when
  the match session's host has said goodbye and no leave goal has started within ~1.5 s (§6). Cheap measurement
  points to confirm the cause first: §7.

## 2. Observation (R)

Timeline 18:02:27 UTC (20:02:27 server time), client2 = B, gast = L:

| Time | B (client2) | L (gast) |
|---|---|---|
| .2403 / .2336 | `recvfrom` H: Leave packet ("Shutdown and Dissolve called on host") | same payload |
| .2505 / .2531 | `session::outEvent::Leave` (0x80010021 = match), `ReplaceGoneMaster`, `MasterGone`, `Gone` | same |
| .2588 | nothing | `NetResultLobby 0x80000002`, `SetGameSession`, Storm `inEvent::Leave`, `SessionDeleted` |
| .305 / .311 | nothing | `LeaveSession 1881` (server: "User 1010 leaves session 1881") |
| .331-.412 | party session 0x80010004: `HostMigrationStart`, `PeerGone`, `HostMigrationEnd`; B hosts 1887 (`SplitSession`, `AddParticipants`, `RemoveParticipants`) | `AbandonSession 1885`, `CreateSession 1888` |
| .414 / .520 | server: "The match of 1010's party is over: 1011 is asked back into party 1888", invitation delivered | - |
| .5215 | `FriendsGameInviteAccepted(player_client_1)`; no `LookupPrincipalIds`, no `SearchSessionsWithParticipants`, no SMI command (compare 18:01:04 and 18:01:45: `LookupPrincipalIds` 15 ms after the event) | - |
| 35.25 | `MasterGone`/`Gone` again for the match objects (8 s later) | - |

B's match session got no `HostMigrationStart` (only the party session did). Screenshots after the leave are
useless as state evidence: L's X11 photo at +10 s still shows the lobby with the same spinner although its network
left at +0.3 s.

## 3. Findings

| # | Finding | Marker | Where |
|---|---|---|---|
| **F1** `0x0084d220` is not a host-gone check | The session event dispatcher `NetSessionBase::vf02` (0x0088a440) sends `PeerGone` to `0x00882b40`, `session::outEvent::Leave` (type test `0x007afb50`) to **`0x007f10c0`** and `HostMigrationStart` (type test `0x007afd90`) to `0x0084d220`. So `0x0084d220` only reacts to a migration Storm already decided on. For the match session B got none; for the party session it fired (+0x5a9, MigrationInfo step 1, no error). | C, R | L905130-905280 (alles.c), `0x007afd90` |
| **F2** Events are dropped once the latch is set | `nsGameSession::StateInSession::vf11` (0x00869c60): `if (session+0x5e0 != 0) return handled`. The latch `+0x5e0` is set by `0x007f10c0`, `0x00882b40`, `0x0084d220` (non-migration branch), `txt_LeavingSession` (0x0086a4f0), the member's `LeaderAction(3)` handler (`0x00879d80`) and `nsOnlineMatch::StateIdle::vf02` case 5/7 (0x008ade20). | C | |
| **F3** What `Leave` does | `0x007f10c0`: not `InSession` -> latch only. Else with `reason = event+0x2c`: `== [0x034c38c8]` -> sub 1; `== [0x0338d7bc]` (the reason the leaver writes in `StateDestroySession::vf08` 0x00876680 branch 1 / `txt_LeavingSession`, "leader leaves a custom match with its party") -> look the host's id up in the **party session's** peer lists (`mov edi,[NetCore+0x5d4]` at 0x007f1117, calls 0x007bd990 at 0x007f1133 and 0x007bdc70 at 0x007f1143): found -> **no error**, latch only; `== [0x0338d7c4]` -> sub 3 when found; else sub 0. Then, if latch == 0 and `+0x5e1 != 0`: `LostConnectionToHost {category 1, severity 2, sub}` through `0x007a56e0`. For our case H is in neither B's nor L's party session, so the error branch is taken on both (I; same as the guests of runs 1-3). | C, I | objdump 0x007f10c0-0x007f1162 |
| **F4** Error gate | `0x007a56e0` drops the error unless the byte `[NetOnline+0xc+category]` is set; the three bytes are set once at startup (`nsMultiplayerStartup::StateStartup::vf05` 0x00828e60) and cleared at shutdown (`StateShutdown::vf08` 0x00887010). Not dynamic. | C | |
| **F5** Nothing in the net code leaves the match | Leaving needs goal 5 (`txt_eGoal_LeaveMatch` 0x008783d0) or 7 (`0x00878680`, `ecx = [[0x3383df8]+0x50]`, one stack argument, `ret 4`; call site 0x018dd7a2). Pushers: `UI::Task::LeaveMatchTask::vf04` (0x018de590), the UI facade `txt_Migration` (0x018dd6f0, case 1/4 push goal 7), `nsMultiplayerStartup` (0x0087b8d0, 0x00887010), the match FSM's own failure conversion `nsOnlineMatch::StateNetMatchEnter::vf06` (0x00860050: builds `eGoal_FailureCleanup` and switches to `StateLeaveNetworkedMatch`, id 0x0338dbc4). `StateNetMatchEnter::vf05` (0x0080ca70) rejects goals 1/3/4/6 (0x80000007) while in the lobby. | C | |
| **F6** The party follows its leader out only by `LeaderAction(3)` | `LeaveMatch`'s parameter block is `{kind 8, byte param2}` (0x008783d0). `LeaveMatchTask::vf04` passes `param2 = (task+0x20 == 0)` when the match controller's `+0x408 == 2`, else 0. The menu entry creates `LeaveMatchTask(0,0)` -> param2 = 1; the facade's host-loss route creates `LeaveMatchTask(1,4)` (0x018dd6f0 case 2/3, ctor 0x0183dce0) -> param2 = 0. `StateDestroySession::vf08` (0x00876680): goal kind 8 with byte != 0 and "I host my party" -> `0x007d3f60` = `NetMsgParty_LeaderAction(3)` (built by 0x007c0ca0) broadcast to the party, reason 0x0338d7bc. A member's `0x00879d80` sets `party+0x414 = 7` and latches the match session; the facade then (error == 0 branch, `facade+0x30 == 2`) runs `LeaveMatchTask(0,0)` for it (`txt_Migration` case 7). So when L leaves through the host-loss route no `LeaderAction(3)` goes out. | C | 0x00876680, 0x007d3f60, 0x00879d80, 0x018eab60, 0x018dd6f0 |
| **F7** L did not use the UI route either | `LeaveMatchTask::vf04` replaces the stale spinner by "Disconnecting"/"Leave lobby"; L's photo 10 s later still shows "Joining". So L's exit came from game logic that pushed a leave goal (F5, most likely the failure conversion), not from the facade. The facade object is `[0x344d668]` (+0x30 mode, +0x34 state, +0x38 phase), its modal gate `[[0x344d640]+0x30]`. | C, R, I | 0x018eab60, 0x018de590 |
| **F8** The mask | `LocID_SPIN_Joining` is created by `JoinSessionTask::vf04` (0x018de3a0), the quick-match tasks, the invitation route (`0x019c7af0` -> `0x018dcba0`, which also calls `0x00879b20(5)` and `0x0087a650` = presence `eGoal_AcceptInvite`) and, for a member following its leader, `PartyFollowLeaderTask::vf07` (0x018eae60) -> `0x018dee70` (stage 2/6 -> "Joining") on the progress event 0x20000005 from `StateFollowLeaderWaiting::vf00` (0x00827460). B's follow finished normally: SMI commands 0x12 and 0x2 at 18:01:54.9 are `0x01886720` + `0x01880340`. The spinner is closed by the task's `vf05` (`FUN_01898c30(DAT_031d0550)`); it stays on both clients. | C, R | |
| **F9** After the leave | `StateLeaveNetworkedMatch::vf04` (0x007a7e00) result 2 -> `StateLeaveWithoutParty` when the party session is `InSession` (L and B alike) -> `eGoal_LeaveThenHost` (0x00884b10) -> `nsOnlineParty::StateLeaveSession::vf08` (0x00878820): leaves the party session and re-hosts (goal 7 passes the `+0x410 < 1` check). A member that does leave therefore ends in a new party of its own, not in L's. | C | |
| **F10** No server message leaves a match | `NotificationHandler::vf01` (0x0077da30) dispatches `ui_type 7xxx`; `NetOnlineSessionServiceRdv::vf27` (0x0077db80) accepts sub-type 3 and, only with `+0x429` set, 1. Only 7001 (migration) and 7003 (join gate). | C | |
| **F11** Facade is one-at-a-time | `UI::FacadeScreen::vf03` (0x018eab60) starts new work only when `+0x34 == 0`; later stages wait for `[[0x344d640]+0x30] == 0`, `DAT_03383d7e == 0` and the UI-ready test. Detection order: pending error (cat 1 -> state 2/3, cat 0 -> 5/6), else (needs `+0x30 == 2`) pending invitation (1/4), `party+0x414 != 0` (7), party/match migrating (5/6). | C | |
| **F12** B's party migrated while its match did not | Becoming the party's host (`FUN_01f92c70` true) does not touch the match session; the match-side facade/FSM state is unchanged. | C, R | |

## 4. What differs between L and B

Same: the Leave packet and event, the code path up to the error (F3), `MigrateMatchLobby` (both hooks installed).
Different: L runs `party_follow` (H1/H2), B is a member (`PartyFollowLeaderTask`, party migration after L's
abandon), L is party host, B is not until 18:02:27.4. The unknown is the trigger that made the match FSM leave on L.

## 5. Candidate causes for B (ranked, all I/V)

1. **No leave goal is started on B although the stimulus is the same.** The decompile does not show a member-only
   branch before the goal is pushed, so the cause is in state that differs at run time: the latch `+0x5e0` already
   set (F2: all events then ignored, `StateLeaveNetworkedMatch` never reached), `+0x5e1 == 0` (no error raised), the
   facade busy (`+0x34 != 0`, F11), or the match FSM not receptive. B is the only client with a finished
   `PartyFollowLeaderTask` and a party migration right after the Leave; both touch the same facade and party state.
2. The reason/host lookup of F3 takes the silent branch on B only (host id found in B's party session lists). Unlikely
   (H is in neither party), but the one place in the Leave path that reads the party at all.
3. The match session is no longer `InSession` on B (F3, first branch: latch only, no error). The lobby login
   result `0x80000001` at 18:01:53.155 says it was.

## 6. Proposal

**H6, member watchdog (hooks, opt-in next to `MigrateMatchLobby`).** Independent of which of §5 is true.

1. Observer detour on `0x007f10c0` (thiscall, `ecx` = session, one stack argument, `ret 4`; prologue
   `55 8B EC 83 EC 1C 56 8B F1`). It runs on the **Storm thread** (the handler lines are logged on ThreadId 20/22),
   so it only records `(session pointer, time)` when `*(session+0x5f0) == 0` (match) and calls the original.
2. A main-thread tick: hook `NetFiniteStateMachine::vf03` (0x00868a90, thiscall(float)) and filter
   `ecx == [[0x3383df8]+0x50]` (the match controller). When the record is older than 1.5 s and
   `[NetCore+0x5d0] == recorded session`, the session is `InSession` (state id test as in `party_follow.rs`),
   no leave goal is running (`0x007a1cd0`-style test: neither `+0x328` nor `+0x158` in 4..6), the session is not
   migrating (`[[session+0x610]+4] == 0`) and no category-0 error is pending (`[[0x3383df8]+0x10]`): call
   `0x00878680` with `ecx = [[0x3383df8]+0x50]`, argument 0, once per session, then clear the record.
3. Effect: the game's own `Shutdown` path (the one the facade uses for an accepted invitation, F5):
   `StateLeaveNetworkedMatch` -> logout -> `LeaveSession 1881` -> `LeaveWithoutParty` -> `LeaveThenHost` (F9).

Risks: (a) never call the goal push from the Storm thread; (b) a late native leave arriving after the push: guarded
by the goal-slot test, the worst case is a second goal answered with `0x80000012`; (c) the UI is not told (no
dialog, the stale spinner stays) exactly like L's screen today; (d) B ends in a fresh party (F9), so the server
must bring it into L's party **after** B's `LeaveSession(match)`; today the invitation (escorts) is delivered at
L's `CreateSession` (18:02:27.5), 0.2 s after the Leave, while B is still in the match. Server change **S4**:
trigger the invitation on B's `LeaveSession` of the match (or repeat it every ~5 s until `JoinSession 1888`);
(e) public matches: the guards (migrating, category-0 error) keep it out of the lobby-migration path.

**H7, optional, leader side.** In the existing `leave_without_party` detour (`party_follow.rs`) call `0x007d3f60`
(thiscall on the party controller `[[0x3383df8]+0x58]`) before the original: members get `LeaderAction(3)` and, if the
facade works for them, leave through the game's own follow-out path (F6). Depends on the facade (F11); do not rely
on it alone.

## 7. Cheap measurements that decide §5 (no code change to the game flow)

Trace lines on both clients, same build: `0x007f10c0` entry/exit (`[esi+0x5e0]`, `[esi+0x5e1]`, `[event+0x2c]`,
eax of 0x007bd990/0x007bdc70, `[[0x3383df8]+0x10]` and its `+4`); calls of `0x008783d0`, `0x00878680`, `0x00876680`,
`0x008763d0`, `0x00860050`; the facade fields `[0x344d668]+0x30/+0x34/+0x38` and `[[0x344d640]+0x30]` at the Leave
and 0.3 s later; `party+0x414` (`[[0x3383df8]+0x58]+0x414`). The first call that fires on L and not on B is the answer.

## 8. Case 5 (running public match, `MigrateRunningMatch`)

Not the same cause: the migration path of case 5 (Storm `HostMigrationStart` -> `0x0084d220` ->
`HostMigrationDisruption` -> `StateMatch`/`StateLoadingLobby`) runs in the lobby FSM, not in the facade or in the
Leave/dissolve path, and a public match has no party migrating beside it. What carries over: (1) the fall-back
`LostConnectionToHost` route (today's behaviour when the migration does not start) ends in the same "someone must push
a leave goal" step, and a private running match with a party has the B situation; (2) correction for
`running-match-host-loss.md` F2: `0x0084d220` is the `HostMigrationStart` handler, not the host-loss detector; whether
Storm raises `HostMigrationStart` for the match depends on the host's leave (a dissolve, as here, raises none).
H6 would also cover the fall-back of case 5 if its guards stay in place.

## 9. Corrections to earlier text

- `host-and-party-migration.md` / `running-match-host-loss.md`: `0x0084d220` is the `HostMigrationStart` handler (F1).
- "Why the mask": it is not caused by the invitation; it is on the screen before the leave (F8).
- `host-and-party-migration.md` A4 lists `LeaderAction` action 3 without a meaning; it is the leader's "I leave the
  match, come along" broadcast (F6).

## 10. Quick-start run 2026-10-23 (21:29 local): why `PartyFollowsInvite` did not keep the party

Question: with the guest started through the new quick-start direct route (`QuickStartLobby=svm`, SMI state 0x56 = ONLINE
menu, leader of a party with client2) the hook `leader_action` (0x00879b20) logged nothing and the party was left (Storm
`Secede`), unlike the old route (HQ -> fake invitation -> ONLINE menu -> invitation accepted). Markers as above: **R** seen
in logs, **C** read from the decompile (`dx11-alles-benannt.c`), **I** inferred.

### 10.1 The route the accept takes from SMI 0x56 (C, R)

| Step | What | Evidence |
|---|---|---|
| 1 | Uplay event `FriendsGameInviteAccepted` reaches the game (`UPLAY_GetNextEvent`); the game itself asks the friend list (`UPLAY_FRIENDS_GetFriendList`, 19:29:54.031) | R guest log; server `Friendlist request from 1010` 21:29:54.032 |
| 2 | `UI::FacadeScreen::vf03` (0x018eab60): `NetOnline+0x10 == 0` (no match goal), facade `+0x30 == 2`, pending invitation (`FUN_0183a6b0` -> `FUN_007a8840`, list count `0x338dc54`) -> `+0x34 = 1`, `+0x38 = 1`; next tick `FUN_018dd6f0` ("Migration" task, case 1) | C, L4159694 / L4159740..4159760 |
| 3 | Case 1, else branch: `FUN_00878680(0)` = **`eGoal_Shutdown`** (match goal id 7, queued at `owner+0x3c`; this is the `member-trace` line at 54.035) followed by `FUN_0183d430` = SMI command **0x3f** | C, L4149817; R 54.035 |
| 4 | SMI step `FUN_0192bb60`: pre-handler `FUN_018f6260(0x3f)` sets next state 0x50 when the state is not 0x3e (state 0x56 -> 0x50). The state-0x50 handler in `txt_combinedops` (0x01996980, L4302377) pushes command **0x48** when an invitation is pending (else 0xa); table: 0x50 + 0x48 -> 0x4f; 0x4f + 0x43 -> 0x3e | C; R 54.035 / 54.039 / 55.885 |
| 5 | State **0x3e** = `UI::SceneInvite` (I: from the transition and the timing, the vtable was not looked up): `SceneInvite::vf02` (0x01a00490) -> `FUN_019c7af0` ("Joining" spinner) -> `FUN_018dcba0` -> **`FUN_00879b20(5)`** (call at 0x018dcc2d) -> `txt_ForceMigrationSecede` (0x007a3ff0) | C; R `storm::echo::session::inEvent::Secede` 55.8907, 5 ms after SMI 0x43 |

Findings:

- `txt_ForceMigrationSecede` (0x007a3ff0) has exactly one caller, `0x879bf0` inside `FUN_00879b20`. A `Secede` therefore proves
  that `FUN_00879b20(5)` ran, and the hook (detour on that address, installed: log line 19:28:49) ran with it. The route is
  the same as the old one from step 5 on; the "new route" differs only in steps 2..4 (Shutdown, 0x3f, 0x48, 0x43), none of which
  touches the party.
- `eGoal_Shutdown` @ 0x878680 is a *match* goal on the match owner (same `ecx` 0x24991fa8 as `eGoal_LeaveMatch` @ 0x8783d0 in
  the same log, id 7 in the match goal table, not the party goals 0..6). It is not what leaves the party. It is the facade's
  "clean up an idle match owner before joining" step.
- The hook was silent because it **decided "not private" without logging**: `leader_action` evaluates
  `action == 5 && leads_members() && inviter_is_private()`. A friend-list lookup happened at 21:29:55.888
  (`Friendlist request from 1010`, 2 ms before the Secede) with no matching `UPLAY_FRIENDS_GetFriendList` line in the guest
  log, so it came from the hook's `list_friends_within`. That call is only reached when `leads_members()` is true, so the
  party was seen as led with members. The only silent exit left is `in_private_session(...) == false`
  (`f.session_id != 0 && f.invite_only` for the friend `mpw`).
- The server data agrees: the host (1005) advertised its session **1892 with `invite_only=false`** at 21:28:30 and never again
  `true` (old run: `1879 invite_only=true` re-announced together with the lobby `UpdateSession`, 19:55:22 / 19:55:39). The
  match room the invitation was bound to (1893) was created with `Property 3 = 4, 4 = 0, 105 = 0` (a matchmaking-style
  lobby); the earlier private custom lobbies have `3 = 0, 4 = 8, 105 = 1`. The test script itself reported
  "Host in der oeffentlichen Lobby: PUBLOBBY, OK Min=4 Max=4" (old run: `PRIVLOBBY ... Min=1 Max=8`). **I**: in this run the
  host was in a *public* lobby although `ZIEL=privat` was requested; by design (H1, case 8) the party is then left as usual.

Conclusion (I, strongly supported): no hook change is needed for the route; the quick-start host did not create a private
custom match. Re-test with a host that really is in a private custom match (Min=1/Max=8, host re-announces
`invite_only=true`).

### 10.2 Decisive log line (no behaviour change)

In `inviter_is_private`, replace the silent `Ok(friends) => in_private_session(&friends, &from)` by

```rust
Ok(friends) => {
    let hit = friends.iter().find(|f| f.id == from || f.username.eq_ignore_ascii_case(&from));
    info!("party-follow: inviter {from}: session {:?}, invite_only {:?} -> {}", hit.map(|f| f.session_id), hit.map(|f| f.invite_only), if in_private_session(&friends, &from) { "private" } else { "not private, the party is left as usual" });
    in_private_session(&friends, &from)
}
```

Also worth one line at the top of `leader_action` for `action == ACTION_SECEDE`: `info!("party-follow: leader action 5, leads_members={}", leads_members(&Game))`. With both, the log says
whether the hook ran, whether the party was seen, and what the friend list said, so a silent decision cannot happen again.

### 10.3 If the inviter's flag proves unreliable (V, not needed now)

The friend list's `invite_only` is the host's last `UPLAY_USER_SetGameSession` flag, not a property of the match room. If a
real private host ever leaves it `false`, the alternative is to decide on the room the invitation is bound to (server side,
`Binding invitation to match room N`), which the client only learns after the accept task's search (`StateAcceptInviteFindSession`
0x00850d00), i.e. after `FUN_00879b20(5)` has run. Deferring the decision to after the search would need the secede
postponed (a larger hook); keep the friend-list rule unless the log line above shows a real private host reported as public.

## 11. Setter is privacy-aware

Measured in a private match with a party: the party member's private match session had `+0x5a8 = 1` (private) and
`+0x5a9 = 1` (may migrate), the leader's `+0x5a9 = 0`. The flat `mov dl,1` in the flag setter (`0x0086a1e0`) had given
the private match session the flag, so after the host left the member tried to migrate instead of leaving. The setter
now computes `(kind == 1) or (not private)` through a code cave (`hooks/src/migrate_lobby.rs`, `hooks/src/code_cave.rs`):
a private match session gets 0 as in the original game, a party 1, a public match 1. No later writer is needed, and a
private session never carries the flag, not even through a party follow-up join. The engine's host-loss check
(`0x00779580`) is unchanged. Patching is all-or-nothing: if no 16-byte `0xCC` run is found or a read-back differs, no
patch (the check included) is applied and a warning is logged.

## 12. Measurement 2026-10-23 22:13 (case 4, private match with party, `member-trace` on): the member's UI facade is the blocker

Markers as above: **R** log (`~/Games/scbl/logs/migration-20261023-221320/`, UTC), **C** decompile/`objdump`
(`dx11-alles-benannt.c`, `Blacklist_DX11_game.exe`), **I** inferred, **V** guess. No code was changed and nothing was run for this section.

### 12.1 Observation (R)

| | leader L (`gast`) | member B (`client2`) |
|---|---|---|
| facade at the Leave (20:16:24.920) | mode30=2 state34=0 phase38=0 gate30=0 | mode30=2 **state34=7 phase38=2 gate30=0x2cd3ccb8**, party414=0; same +300 ms |
| after the Leave | main thread: `eGoal_LeaveMatch` 2.5 ms later, SMI command **0x73**, `NetResultLobby 0x80000002`, `StateDestroySession` | nothing (no goal, no SMI command) |
| earlier | 51.39 `leader_action(2)` instead of Secede (H1); lobby login 53.004; **no** `party-follow: ... FollowLeader sent` line (H2 never engaged) | party-message dispatcher `0x879d80` called at 45.871, 51.406, 53.014; `outEvent::FollowHost` 52.082 + `AcceptFollowHost` 52.086 (no dispatcher call in between); JoinSession 52.936; lobby login 53.009; SMI 0x12 + 0x2 at 54.908; nothing after |

### 12.2 Answer 1: what facade state 7 / phase 2 is, and why it blocks the host-loss exit (C)

* `UI::FacadeScreen::vf03` (`0x018eab60`, thiscall(float), `ret 4`; prologue `55 8B EC 56 8B F1 83 7E 30 00`) is a one-job-at-a-time
  machine. **Only in state 0** it looks for work: pending error `[NetOnline+0x10]` (`[err+4]`: 0 -> state 5, 1 -> 2, 2 -> 3), else (mode30==2)
  pending invitation (1/4), else `[[NetOnline+0x58]+0x414] != 0` (call `0x78a390`, ecx = party controller) -> **state 7**, then migration checks (5/6);
  all set phase 1. **In any other state it never looks at the error**; it only advances the current state's phase, and only when the task
  slot `[[0x344d640]+0x30]` (= `gate30`) is empty (`0x18eabbf`).
* Phase 1 runs `txt_Migration` (`0x018dd6f0`), which first writes phase 2. Case 7 reads `party+0x414`: 0 -> reset 0/0 (no task); 7 (`LeaderAction(3)`)
  -> `LeaveMatchTask(0,0)`; 1..6 -> **`PartyFollowLeaderTask`** (ctor `0x183e180`, delegate = `0x01886720`), submitted with `0x183ec20` into the
  runner `[0x344d640]` (`UI::FacadeTask`, tick `0x18599d0`) whose slot `+0x30` takes exactly one task. Phase 2 of state 7 has no handler of its own:
  **the state ends only when that task finishes and its delegate `0x01886720(result)` writes state=phase=0** (`mov [ecx+0x34],0; mov [ecx+0x38],0`).
  `gate30` is therefore **the running facade task** (0x2cd3ccb8 = the heap clone of `PartyFollowLeaderTask`, see `0x1848f40`), not a modal dialog.
* The host-loss reaction is the facade, nothing else (resolves §5 candidate 1 and corrects F7): the Leave handler `0x7f10c0` stores
  `LostConnectionToHost` through `0x7a56e0` in `NetOnline+0x10` (and `NetOnline+0x154=3`); in state 0 the facade turns it into state 2, then
  `txt_Migration` case 2/3: match session `InSession` (`0x789510`) -> **`LeaveMatchTask(1,4)`** (ctor `0x183dce0`, delegate `0x183d430`) whose `vf04`
  (`0x18de590`) sends SMI command 0x73 (kind 4), replaces the spinner by "Disconnecting" and pushes `eGoal_LeaveMatch` (`0x8783d0`). L shows exactly
  this (R: `eGoal_LeaveMatch` 2.5 ms after the Leave on the main thread, SMI 0x73). On B the error sits in `NetOnline+0x10` unread: state 7 is not
  state 0, and a new task could not even be submitted (`0x183ec20` and the inline submitters at `0x189ba70`, `0x189bbe0` check `slot == 0`).

How the follow task ends in the game (C):

| step | where |
|---|---|
| spinner | `PartyFollowLeaderTask::vf04` `0x18eae00` (`0x879b20(6)` is a no-op for a member, `0x18dee70` picks the text from `party+0x414`: 1 create, 2/6 "Joining", 3/4 "Searching"), then `eGoal_FollowLeader` (`0x8796a0`); progress event 0x20000005 -> `vf07` `0x18eae60` -> `0x18dee70` |
| `party+0x414` set | `0x879d80`: FollowLeader message -> endpoint at `ctrl+0x430`, 414=6 (**sole sender: `StateLeaderPartyWaitSync::vf08` `0x839280` via `0x828500`**); LeaderAction action 1 -> 414=value (byte `msg+0x2c`), action 2 -> cancel (414 1..4 -> 0), action 3 -> 414=7 + match latch `+0x5e0`. **Also the Storm event `FollowHost` on the party session: `0x85aaa0` (stores FollowHostInfo at `session+0x60c`) -> listener `NetOnlinePartyController::vf00` `0x7d4110` (ecx = `ctrl+0x408`) copies it to `ctrl+0x430` and sets 414=6, exactly like FollowLeader** (public flow; trigger semantics I) |
| goal | `StateFollowLeaderWaiting::vf00` `0x827460`: party session not `InSession`/migrating -> 414:=0, result 0x40000002; 414==0 -> result 0x40000000 (cancel); 1..4 -> progress; **6 -> progress, 414:=0 (`0x7a86b0`), `StateFollowLeaderJoinMatch::vf08` `0x878940`** -> `eGoal_JoinMatch` (`0x877370`, goal id 4) with callback `0x850b60` |
| success | `0x850b60`: JoinMatch result bit 31 -> `MemberAction(1)` to the leader (`0x7c1b40(1)`, `0x7c2970`) and state result 0x80000003; else "JoinMatch failed", 0x40000000. The JoinMatch result comes when `StateNetMatchEnter` is done: lobby login (`0x88ce50` -> `0x875490`, cb `0x884a20`) -> `PartyWaitSync(task,1)` (`0x879840`: **for a non-leader it returns 0x80000003 at once**) -> cb `0x826260` -> `0x80cb20` |
| task end | `FacadeBaseTask::vf03` `0x184f1d0` polls the goal result (flags & 0xd0000000) -> `vf06` `0x183e250` (task+0x10 = 0 success / 1 bit 30 / 2 bit 28, task+0x18=1), done flag `+0x19` after >= 2.0 s; runner `0x18599d0` -> `0x1859950`: `vf05` `0x18dedc0` (not success: `0x879c80` + "Join Failed" dialog unless code 0x40000002; always closes `DAT_031d0550`), then delegate `0x1886720`: state=phase=0 and, for result 0, `NetOnline+0x150:=3`, SMI 0x12, `0x1880340` (SMI 0x2/0x21) |

Normal use: public quick match with a party: leader's UI calls `0x879b20(v)` (v 3/4/2 = searching/creating/joining) while it searches, B shows
the spinner; when the leader is in, B gets 414=6 (FollowHost, or FollowLeader for a private hosted match), joins once, the goal completes, the task ends.
**`vf05` dereferences its argument: forcing a task to "done" while its goal has no result object would crash (`0x1859950` passes `holder+0x1c`).**

### 12.3 Answer 2: what is different in our path (I/V, ranked; the log cannot decide it)

Fact: the leader's state is clean (never in 7: the leader gets no 414), B's is not. B was pulled in by Storm `FollowHost` (R), not by H2 (no H2 line on L).
What the log does not show is whether the first follow task ended:

* **I1 (second follow after the first one ended).** SMI 0x12+0x2 at 54.908 would be the delegate `0x1886720(0)` of the first task. A later
  `party+0x414 != 0` (candidate: the third dispatcher call at 53.014, 8 ms after the leader's lobby login and 1 ms after a party `MasterBroadcast`;
  FollowLeader (`0x839280` is the only broadcaster at lobby login) or LeaderAction(1,v)) arrives while the first goal is in its JoinMatch state, where nobody
  consumes it; after the first task ends the facade (state 0) sees 414 != 0 -> state 7 again -> task 2 -> `Waiting::vf00` consumes it (414:=0, matches
  `party414=0`) -> `eGoal_JoinMatch` for the match B is already in -> never answered (V; `StateNetMatchEnter::vf05` `0x80ca70` would reject goal 4 with
  0x80000007, which `0x850b60` would take as success, so "never answered" needs the goal to sit in the queue; not followed further).
  Prediction: facade 7 -> 0 -> 7, 414 != 0 for ~2 s, no second MemberAction at L (R: L got only one message, 53.019).
* **I2 (the first task never ended).** The same SMI pair is `UI::FacadeLobby::vf01` (`0x1894400`, calls the same `0x1880340`); the delegate never ran.
  Prediction: facade stays 7/2 from ~51.4 on. Unclear why the goal would not complete although B sent `MemberAction(1)` at ~53.015 (L received at 53.019).
* Not the cause: `LeaderAction(3)` (latch `+0x5e0` was 0, 414 never 7), host-loss detection (the error is raised: latch 0, `err5e1`=1 at the Leave).
* Open: `nsOnlineLobby::StateMatch::vf04` (`0x7a6f40`) also reads the pending error (type 0 -> transition `0x338db54`, else `0x338db64`); effect not followed.

### 12.4 Answer 3: smallest intervention

Independent of I1/I2 (the block itself is proven in 12.2). **R-B (recommended, member, main thread): detour `0x018eab60` and orphan the stuck task.**

1. Trigger (all at once, seen on two consecutive ticks ~50 ms apart): `facade+0x34==7 && +0x38==2`, `[NetOnline+0x10] != 0` with `[err+4]==1` (host lost), match session
   `InSession` (as `0x789510`), `party+0x414==0`, `[0x344d640]+0x30 != 0` and its vtable is `PartyFollowLeaderTask`'s (read it from a live task; not looked up here).
2. Action: `[[0x344d640]+0x30] = 0` (**do not free the task**: the pending FollowLeader goal still writes `task+0x1c` through `0x183d550` on progress events,
   so freeing could be a use-after-free; leaking 0x20 bytes is the safe choice), `facade+0x34 = 0; facade+0x38 = 0`; then call the original. The same tick's
   state-0 path picks the error up, `txt_Migration` runs `LeaveMatchTask(1,4)` (it closes the stale "Joining" spinner `DAT_031d0550` itself) and the game's own
   `eGoal_LeaveMatch` follows, then F9 as for L.
3. Convention: thiscall(float) with one stack float, `ret 4`; the Storm thread (Leave handler, ThreadId 20/22) must not touch the facade, so only the main-thread tick acts.
4. Risks: (a) the pending `FollowLeader` goal stays in the party FSM until `LeaveThenHost` preempts it (F9), its late completion has no task to report to (the orphan is
   still allocated; the delegate `0x1886720` is not called, so no SMI 0x12/0x2); (b) a legitimate follow running while an unrelated host-loss error is pending is
   cut off (acceptable: the match is being left anyway); (c) a stale `party+0x414` (I1) is not cleared: add `0x7a86b0` (thiscall on the controller) after the reset,
   so the facade does not re-enter state 7 on the next tick; (d) the leader-side effect of F9 (B ends in its own party; server escort rule S4) is unchanged.

R-A (prevent, only after the log below decides): I1 -> in the `0x879d80` detour drop a FollowLeader/LeaderAction(1,v) message when the match session is
`InSession` and its host is the one the message names (compare the PeerDescriptor id); the game cannot follow into a match it is in anyway. I2 -> no cheap prevention known.
Not recommended: pushing `eGoal_Shutdown` (H6 of §6) alone; it leaves state 7/2 and the "Joining" spinner on the member's screen after the leave.

**Live decision log (observe-only, same style as `member_trace.rs`):** (1) a 250 ms sampler thread for 90 s after the first `0x18eae00`: `state34/phase38/[gate30]/414/[NetOnline+0x10]/[err+4]`
(decides I1/I2: does the state return to 0?); (2) in the `0x879d80` detour log message kind (type word `0x338ec9c` FollowLeader, `0x338ecc4` LeaderAction, else), action byte `msg+0x1c`,
value `msg+0x2c`, and 414 before/after; (3) `0x7d4110` (thiscall ecx=`ctrl+0x408`, 1 stack arg `{session,type}`, `ret 4`): session kind, type, 414 before/after;
(4) `0x18eae00` (thiscall, no args, prologue `55 8B EC 83 EC 08 A1 F8 3D 38 03`), `0x1886720` (thiscall, 1 arg, `ret 4`, prologue `55 8B EC 33 C0 89 41 34`): entry with result;
(5) `0x850b60` (thiscall, 2 args, prologue `55 8B EC 6A FF 68 A0 2B 65 02`): result code `[arg2+0x14]`. Every line should carry the elapsed ms since the lobby login.

### 12.5 Answer 4: reset on the Leave itself

A reset of `+0x34/+0x38` alone does **not** help: the next phase-1 handler waits for `gate30 == 0` (`0x18eabbf`) and `0x183ec20` will not submit a new task while the slot is
occupied, so the slot must be orphaned too (12.4). It must not be done from the Leave handler (`0x7f10c0`, Storm thread): the facade runs on the main thread, so set a flag there and act in the
`0x18eab60` tick. A timer-less variant "reset on Leave" also fires when no host loss follows (any Leave with latch 0), so R-B's condition (pending category-1 error) is the safer trigger.

### 12.6 Corrections

* F7: L's exit **is** the facade route (`LeaveMatchTask(1,4)`, SMI 0x73), not "game logic"; §5 candidate 1 (facade busy) is confirmed, and refined: the error is raised on B too but never read.
* F8 / §10: "SMI 0x12 + 0x2 = `0x01886720` + `0x01880340`" is not exclusive; `FacadeLobby::vf01` (`0x1894400`) emits the same pair, so it does not prove the follow task ended.
* `host-and-party-migration.md` A4 assumes members only follow through FollowLeader; in this run B followed through Storm `FollowHost` (`0x85aaa0` -> `0x7d4110`) and H2 did not engage on L.

## 13. Measurement 2026-10-23 22:40 (client2): two follow signals for one join; R-B implemented

**R** (`migration-20261023-224027/bl-tracing-client2.log`, UTC 20:43:01-20:43:35). For one and the same match join the member gets **two**
follow signals. First `LeaderAction(1, value 2)` sets `party414=2` and starts `PartyFollowLeaderTask` #1 (UI facade state34=7 / phase38=2);
Storm `FollowHost` sets 414=6, the join runs (join-cb result `0x80000007`, i.e. "already in"), the `FollowDelegate` (`0x1886720`) is called with result 0 and
task #1 ends at 20:43:05.105. 7 ms later `PartyFollowLeaderTask` #2 starts, because a `FollowLeader` message (`0x879d80`, value `0x34`) had set 414 to 6 again
in between (this is candidate I1 of 12.3). Task #2 pushes a `JoinMatch` for a match the member is already in; it is never answered. The facade stays in
state34=7 / phase38=2 with `gate30` = task #2, so it never reads the pending host-loss error (`[[0x3383df8]+0x10]`, `[err+4]==1` at 20:43:35.309),
and the match is never left. On the leader the facade is in state 0 and `eGoal_LeaveMatch` runs.

**Intervention** (`hooks/src/follow_unblock.rs`, decision in `hooks/src/unblock_logic.rs`; part of `MigrateMatchLobby`, log prefix `follow-unblock:`): R-B of 12.4.
`UI::FacadeScreen::vf03` (`0x18eab60`) is detoured (prologue checked, DX9 skipped). A passive observer on `PartyFollowLeaderTask::vf04` (`0x18eae00`; when
`TraceMemberLeave` owns that hook, `member_trace` feeds the same function) stores the task's vtable. Before the original tick, when on two consecutive ticks
state34==7, phase38==2, `party414==0`, a category-1 error is pending and the task slot `[[0x344d640]+0x30]` holds an object with the stored vtable, the slot and
`facade+0x34/+0x38` are set to 0 and the original runs: in state 0 it reads the error and starts `LeaveMatchTask(1,4)` as on the leader. The task is not freed
(its pending goal may still write to it, 12.4). One firing per error (the counter is reset). Risks: the orphaned task and the stale goal stay in memory
until the next party transition; the `0x7a86b0` reset of 12.4 (c) is not done because `party414==0` is part of the trigger; the task vtable is only known after the first
follow task of the session, which is always the case when one can be stuck. Not run live yet.

## 14. Analysis 2026-10-23 23:36 run (client2 after `follow-unblock`): the match goal queue is blocked by a stale JoinMatch

Markers as above (**C** decompile/`objdump`, **R** log, **I** inferred, **V** guess). Log: `~/Games/scbl/logs/migration-20261023-233552/`
(`bl-tracing-client2.log` = member B, `bl-tracing-gast.log` = leader L, UTC 21:39:05). Analysis only; nothing was run or changed.

### 14.1 Observation (R)

| UTC 21:39:05 | B (client2) | L (gast) |
|---|---|---|
| .2229 | Leave handler: latch5e0 0->1, err5e1=1, facade 2/7/2 (state34=7, phase38=2), gate30 = follow task | same handler, facade 2/0/0 |
| .2240 | `follow-unblock` releases the task (slot and 34/38 := 0) | - |
| .2257 / .2259 | `eGoal_LeaveMatch` (0x8783d0), SMI 0x73 | `eGoal_LeaveMatch` |
| +0.7 ms | **nothing** (main thread keeps ticking: party controller `0x7d4110` at .2564 and .3379) | `fn_8763d0` (0x8763d0), `NetResultLobby 0x80000002`, `StateDestroySession::vf08`, `LeaveSession` at .2370 |
| .2555 - .3373 | party session `HostMigrationStart` ... `HostMigrationEnd` (82 ms), `SplitSession`, `AddParticipants`, `RemoveParticipants` | `AbandonSession`, `CreateSession` |

* The block exists **before** B's party migration starts: no `fn_8763d0` in the ticks between .2257 and .2555 (the leader needs 0.7 ms). The party
  migration ends at .3373, 100 s before the sampler stops. No `LostConnection` event, no `HostMigrationStart` for the match session on B.
* Follow history of B (R): `LeaderAction(1,2)` 31.30 -> task #1 (`vf04` 31.30); `FollowHost` 32.27 (414=6); `JoinSession` RMC 33.12; lobby login 33.158; **join-cb
  0x80000007 at 33.164**; `FollowLeader` message (value 0x34) **33.175** (414=6 again); `FollowDelegate(0)` 34.80 (task #1 ends); **task #2 `vf04` 34.802**,
  progress event 0x20000005 at 36.30 (`StateFollowLeaderWaiting::vf00` consumed 414=6, started `StateFollowLeaderJoinMatch`). **No second join-cb and no RMC
  message at all from 34.8 to the Leave.** Same pattern (join-cb=1, `vf04`=2, unblock=1, `eGoal_LeaveMatch`=1, `fn_8763d0`=0) in runs 225949, 232928 and 233552.

### 14.2 How goals are processed (C)

* `FUN_00849f80` (0x849f80) queues a goal record in a doubly linked list at `owner+0x3c` (newest at `+0x3c`, oldest at `+0x40`, count `+0x44`);
  `eGoal_LeaveMatch`/`eGoal_JoinMatch` do this on the match controller (`ecx` = `[[0x3383df8]+0x50]`), `eGoal_JoinSpecificSession` on the **session** object.
* Consumer `FUN_00858fd0` (0x858fd0), called from `NetFiniteStateMachine::vf03` (0x868a90) every tick: it looks **only at the oldest node** and calls the current
  state's `vf02(goal, ctx)`. Node status (`node+0x110`): 3 = done (node removed), 4/5 = in progress (return), anything else = nothing happens and the same node is
  offered again on the next tick. There is no skipping: **head-of-line blocking**.
* In the match FSM (`nsOnlineMatch`) **only `StateIdle` has a goal handler** (`vf02` 0x8ade20; I dumped the vtables: StateJoinMatch, QosEndpoints, CreateNetworked,
  NetSetupVoice, NetMatchEnter, LeaveNetworked, DestroySession ... all point to the empty default `NetFiniteState::vf02` 0x775c00, `ret 8`). In Idle:
  goal 4 (JoinMatch) -> StateJoinMatch (id 0x338db9c, no precondition); goal 5/7 (LeaveMatch/Shutdown) -> StateLeaveNetworkedMatch (id 0x338dbc4, vf08 =
  `fn_8763d0`) if the match session is `InSession` (`0x7796e0`), else result 0x80000012.
* `StateJoinMatch::vf08` (0x875f20) -> `0x875df0` -> `txt_eGoal_JoinSpecificSession` (0x86a1e0, call at 0x875f03 with `ecx = [NetCore+0x5d0]` = the **match
  session**): queues goal kind 1 on the session's own FSM (timeout field 30.0f = 0x41f00000 in the record). `StateJoinMatch::vf00` (0x875f50) only polls the task
  result (`task+0x1c`), no timeout of its own. Before queuing, 0x86a1e0 overwrites `session+0x5a8/0x5a9/0x5b4/0x5d8` and the descriptors at `+0x3fc/+0x454/+0x4ac`
  with the candidate's values (the same function holds the privacy-aware setter patched by `migrate_lobby.rs`; I: this explains `private5a8=1 migrate5a9=0` on B).
* Session FSM (`nsGameSession`): `StateNone::vf02` (0x8596a0) handles goal kinds 0, 1, 2; **`StateInSession::vf02` (0x859a10) handles only kind 2** (LeavingSession
  -> 0x338d77c). A kind-1 goal queued while the session is already `InSession` is therefore never dispatched and never answered. It sits at the head of the session
  queue and also blocks every later goal of that session, including the kind-2 `txt_LeavingSession` (0x86a4f0) that ends in `LeaveSession`.
* Timeouts: `NetFiniteStateMachine::vf03` decrements `ctx+0x104` by `dt`, but only for a context in status 4..6 (an active goal). A goal that no state handles never
  gets a context, so its 30 s never run (R: > 100 s without change). Goals JoinMatch, LeaveMatch and Shutdown carry FLT_MAX (0x7f7fffff). I did not find the expiry check.

### 14.3 Answer 1: where B hangs (C for the mechanism, I for "this is what happened")

1. The second `PartyFollowLeaderTask` (34.80) runs `eGoal_FollowLeader`; with 414=6 the party FSM goes to `StateFollowLeaderJoinMatch::vf08` (0x878940), which calls
   `eGoal_JoinMatch` (0x877370, cb 0x850b60) for a match B is already in.
2. The match FSM is in Idle, accepts goal 4 and enters **StateJoinMatch**; its task queues `JoinSpecificSession` on the match session whose FSM is in **InSession** ->
   never handled (no RMC, no cb: R). The match FSM now waits in `StateJoinMatch::vf00` for a result that cannot come.
3. `follow-unblock` frees the facade, the facade runs `LeaveMatchTask(1,4)` and pushes `eGoal_LeaveMatch` (R), but it is queued **behind** the unfinished JoinMatch
   goal in the match queue and the match FSM is not in Idle -> `fn_8763d0` and `StateDestroySession` never run. The facade (state 2/2) waits for the result callback
   `0x183d550` of that goal -> the "Verbindung ... wird getrennt" mask stays (I). Invitations are not read (facade not in state 0).
4. L has no follow task (no 414), its match FSM is in Idle and its facade in state 0, so the same goal runs in the first tick (R: 0.7 ms).

Not verified live: the state ids and queue contents (see the decision log in 14.5); the evidence is the code path plus the identical log pattern in three runs. Alternative that gives
the same symptom (V): the match FSM did not return to Idle after the first JoinMatch; the fix below would then be insufficient; the decision log in 14.5 tells them apart.

### 14.4 Answer 3: party migration is not the cause (I, strong)

The block starts before the party migration (14.1), the migration is over after 82 ms, the leader migrates its party at the same moment without any problem, no
`LostConnection` event appears, and the match and party FSMs have separate goal queues/contexts. Suppressing B's `SplitSession` is neither needed nor safe: the
client calls it as the elected party host (Storm `HostElected` .2575); a refusal gives `NetResultRdvSession` errors on the party session, i.e. a second failure path, and
the server's S4 "ask B back into party" logic needs the split session (1966 -> 1969). Do not do it.

### 14.5 Answer 2: smallest intervention

Calling `fn_8763d0`/`StateDestroySession::vf08` directly is not an option: they are state-entry methods, the FSM transition (`FUN_00858ed0`, needs ctx status 2) is not
done, and even then the session-level leave would stay behind the stale kind-1 goal. Cleaning the queues from outside (unlink the head nodes, write a result into
`task+0x1c`) needs a NetResult object and is the riskiest option (V). **Prevent the second follow instead (H8, recommended):**

* Where: the existing `follow_unblock::facade_tick` (main thread, before the original). Condition on one tick: `facade+0x34 == 0 && +0x38 == 0`, gate slot empty, no pending
  `NetOnline+0x10` error, `party414` in 1..=6, the match session `[0x3383df4]+0x5d0` is `InSession` (state-id compare as `0x7796e0`: `[[s+0x10]+0x30]()` returns a pointer to the
  id, compare with the dword at 0x338d7a4), and a follow task has ended with result 0 for **this** match session (remember the session pointer and id pair `+0x428/+0x42c`
  in the `FollowDelegate` 0x1886720 observer; the id offsets are an assumption, I). Action: `party414 := 0` (`0x7a86b0`: thiscall on `[[0x3383df8]+0x58]`, no args,
  plain `ret`; equivalent to `mov [ecx+0x414],0`), log once. The facade then never starts task #2, no `FollowLeader`/`JoinMatch` goal is pushed, and the normal
  `LeaveMatchTask` route (as on L) works with or without `follow-unblock`.
* Risks: a legitimate second follow (leader moves to a different match while B sits in one) is only dropped if the session id pair matches, so keep that check; the
  `FollowLeader` message that the leader sends at every lobby login is the usual source of the duplicate (`0x839280`), the same condition covers `FollowHost`; the stale
  state from an earlier duplicate that already happened is not repaired (keep R-B as a safety net, but it cannot complete the leave once a kind-1 goal is queued).
* Alternative prevention (more invasive): detour `0x86a1e0` and refuse kind 1 on an `InSession` session; this also needs a result for `StateJoinMatch`'s task (V).

Decision log to add (observe-only, main thread, prefix `follow-dup:`): at `facade_tick` entry once per change of `(state34, party414, match-session state id)`; at `0x877370`,
`0x875f20`, `0x86a1e0`, `0x859a10` entry (thiscall; prologue of 0x877370 `55 8B EC 6A FF 68 A6 B1 65 02`, of 0x86a1e0 `55 8B EC 6A FF 68 F8 82 65 02`, of 0x859a10
`55 8B EC 8B 55 08 8B 02 83 E8 02`; 0x875f20 starts `A1 50 5C CF 02 56 8B F1`): goal kind, `ecx`, session state id. Sampler (250 ms, from the Leave on): match controller state id
(`[[0x3383df8]+0x50]` -> `[+0x10]` -> vtable slot 12), queue count `[ctrl+0x44]`, head goal kind `[[ctrl+0x40]+8]` and status `[[ctrl+0x40]+0x110]`; the same four values for the
match session (`[0x3383df4]+0x5d0`). Prediction if 14.3 is right: match state id 0x338db9c, match queue 2 (head kind 4, then 5), session state = InSession id, session
queue head kind 1, count >= 1. If the match state id is 0x338db7c (Idle) with a non-empty queue, 14.3 is wrong and the head goal's kind says what blocks.

### 14.6 Open points

* Whether the match FSM really returns to Idle after the first JoinMatch (the leader's behaviour says yes; not read in the code).
* The party FSM keeps the stale `eGoal_FollowLeader`/`StateFollowLeaderJoinMatch` goal after the leave; F9 (`LeaveThenHost`) has to preempt it. Not followed.
* Server side (S4, invitation of B into L's new party) is independent of this; it only matters once B really leaves the match.

### 14.7 Status 2026-10-23: H8 implemented (not yet tested live)

`follow_unblock::drop_second_follow` runs at the start of `facade_tick` (main thread, before the original). If the facade is idle (`+0x34 == 0`, `+0x38 == 0`, task slot
empty), `party414` is in 1..=6 and a `FollowDelegate` (0x1886720) with result 0 ended less than 5 s ago (`note_follow_done`, fed by the `member_trace` observer or by its own
hook when `trace_member_leave` is off), `party414` is set to 0 by a checked memory write and `follow-dup: second follow dropped (party414 was N, first follow ended X ms ago)`
is logged once. Decision logic: `unblock_logic::should_drop_second_follow` (unit tests). R-B stays as a safety net. Deviation from 14.5: the match-session id-pair check is
**not** done (the 5 s window replaces it), so a legitimate second follow within 5 s after the first would be dropped as well. Expected in the next run: the `follow-dup:` line, no
second `PartyFollowLeaderTask::vf04`, `fn_8763d0` right after `eGoal_LeaveMatch`.
