# testbot: headless test players

Test players that speak the game's network protocol and its DLL's gRPC API, so server behaviour can be tested without the game or other people:
- **Quazal:** the auth server's LoginEx, a Kerberos ticket, the secure connection, and RMC calls.
- **Server pushes:** a bot receives the server's own requests, such as the "come in" notification for a private match.

```sh
build/build.sh bots                        # all scenarios against a fresh local server
build/build.sh bots private-match-invite   # just one
```

`build.sh bots` builds the server and the bots, starts a server in a temporary folder with `trusted_subnet = 127.0.0.0/8` (the bots connect from localhost, as players on a VPN connect from its subnet), runs the scenarios, and prints the server log if one fails.

Against a running server (from a PC that can reach it):

```sh
testbot --server 10.8.0.10 login lobby-invite
```

That registers throwaway accounts (`Host…`, `Guest…`) on that server.

| Scenario | Checks |
|---|---|
| `login` | Login chain end to end; a player shows online in friends' lists, and offline once they quit |
| `lobby-invite` | An invite reaches the friend; the friend search returns the host's lobby with an address; joining works; no push for a public lobby |
| `private-match-invite` | The search answers with the match room and the host's lobby; the guest adding itself gets the 7003 "come in" push naming itself and the match |
| `cleanup` | A host who quits takes their lobby with them |
| `trusted-subnet` | An address from the wrong adapter reaches friends as the address the server saw |
| `bad-packets` | Junk on the auth and secure ports doesn't stop logins |
| `leave-session` | A guest who leaves a lobby is no longer in it (friends' searches for them don't return it). |
| `abandon-empty` | A host who abandons their lobby (nobody left in it) ends it. |
| `duplicate-request` | A retransmitted CreateSession (the same packet twice) creates one session. |
| `lost-push` | A lost "come in" push is sent again, so the guest still gets into the match. |
| `nat-probe` | The NAT helper tells anyone the address it sees, on both of its ports, but registers only a player with their ticket who proved the address. |
| `nat-relay` | Two players, one relayed: packets reach each other through the relay, each seeing the other at its advertised address; strangers, and packets without the right tag, get nowhere. |
| `nat-public-address` | A game that still registers its local address gets the public one the NAT helper found (with the local one kept for players on its network). |
| `friends` | A friend request, its notice, accepting it, and removing the friend. |
| `block` | A block hides both players from each other and stops invites. |
| `invite-queue` | Invitations from two players both arrive (one no longer replaces the other). |
| `join-now` | Joining a friend's game without an invitation (`JoinNow`), outside a match: a friend hosting a public room is joined with an invitation marked as requested, an older invitation from someone else waiting is delivered first and as it is; a friend in someone else's party is joined through them once its host has them as a friend (not through a mere guest of it); someone not in a game, and oneself, are refused. |
| `join-now-match` | `JoinNow` into a match: a friend in a public match (hosting it with the anteroom beside it, or a guest of a stranger's) is joined into the match, requested (not when its host blocked the caller); a friend in a private match (hosted, also with the anteroom beside it, or joined) is refused, and nothing is queued. |
| `identity-login` | An identity links to an account, and signs in to it with a new password. |
| `rename` | Renaming keeps the account (friends, id) and frees the old name. |
| `direct-test` | The launcher's direct-connection test reaches this machine: behind a reverse proxy too, where the server must use the forwarded address. |
| `presence` | Online means signed in to the game service: a ticket alone (the launcher's connection test) isn't, and leaving ends it. |
| `slow-handshake` | A game far from the server (its first resend comes before the answer) sends SYN and CONNECT twice and switches to the last SYN answer's signature: it must still sign in and play. |
| `second-sign-in` | The newest sign-in wins: the same account signing in from another PC closes the first game's connection, and what it left (its lobby) goes with it; the new one works as usual. |
| `ticket-elsewhere` | A ticket works only from the address that asked for it: someone who saw it (and the CONNECT) on the way can't sign in with it from elsewhere. Needs a server on loopback, where 127.0.0.2 is another address. |
| `private-room-join` | Nobody joins a private match uninvited: not by naming themselves twice (private and public), nor for sharing some other room with the host. The host's own party follows it in, by adding itself or being added; a stranger can't be pulled in. |
| `host-migration` | A party's host leaves: the member whose game takes over (SplitSession, then AddParticipants of itself) hosts the party, with everyone left in it (the others' games make no call), and the old one ends; a second member's split of it joins the same party. The same for a public match its guests carry on. |
| `host-migration-abandon` | The game taking over a match abandons it first, then splits it: it still takes the others along once the host is out of it; a member who abandons and splits while its host is still in goes alone. |
| `host-migration-guest` | A guest splits a match while its host is still in it and signed in: only the guest moves; the others follow once the host leaves (its abandon arriving after the split) or signs out, or when their own game splits it too. |
| `party-escort` | A party's leader invited into someone else's private match takes its party along (its game sends the members after it): the party is kept, and its members who are the host's friends are let into the match, added by the match's host (who gets the push that opens their join gate) or adding themselves. Not a stranger, not a member the host doesn't know, not one before its leader was invited or after it left, and not added by the leader (only the host brings players into a private match). |
| `host-migration-claims` | Only a member still in a room takes it over: one who left it and splits it once the host has gone splits off alone. A member's claim on a room whose host then ends it (DeleteSession) and signs out hands nobody over and tells nobody it moved. |
| `party-return` | The way back of that party: the match is dissolved, the leader leaves its party and opens a new one, the member that came along migrates the old party alone. The server asks that member (not one that stayed behind, not while the match still goes on, and not one in someone else's room since) into the leader's new party with an invitation delivered as the requested join, once. |
| `party-member-leaves-for-match` | A party member (not the leader) invited into a private match leaves its party alone; when the leader leaves later, the party's other members are not moved into the session the member split off. |
| `host-migration-silent` | A host that goes quiet while a member's split waits for it: after 20 s of silence the others move to the member's room and are told (7001), and the quiet host's lobby isn't offered until it speaks again; a host that keeps talking is never taken over. |
| `host-migration-cascade` | The new host goes too: the room is taken over again, and a member's late split of any room of the chain is answered with the room it's in now. |
| `host-migration-invite` | An invitation pending when the host goes (the game's own, and one through the friend list) names the new host's room, from the new host. |
| `host-migration-host-returns` | The host's game drops and signs in again while a member's split waits: its old connection's silence lets the takeover through, though the player talks on the new one. |
| `host-migration-parties` | A public match whose players have parties of their own migrates; every party stays as it was, and nobody from a party outside the match is taken in. |
| `private-match-ends` | A private match its host ends isn't migrated: a guest's late split takes nobody along, and nobody is told it moved. |
| `party-return-stays` | After a private match, a party member still playing in it isn't called back into the leader's new party; one who left it is. |
| `friend-list-private-match` | The friend list tells a private match from an invite-only party (`private_match`). |
| `split-limits` | Splitting makes sessions: only of a session the player is in (or just left), and no more than creating them would. |
| `private-room-hidden` | A private match isn't handed to strangers: not in matchmaking, and not by searching for its host. Its own players, and the invited, still find it. |
| `station-url-schemes` | A station URL's address is checked whatever its scheme: a `udp:` URL naming someone else's address is corrected, and a host name never reaches other players. |
| `syn-flood` | A flood of SYNs that never go on to CONNECT (what forged sources look like) leaves nothing behind, so it doesn't keep anyone from signing in, not even from the same address. |
| `login-lockout` | Wrong passwords for an account from one address stop that address only: its owner, elsewhere, still signs in. (Through the loopback "proxy", so against a server on this machine.) |
| `outdated-client` | Clients older than the server allows can't sign in, and neither can the game after one tried; a current client lets it in. |

Two more need a server set up for them, and `build/build.sh bots` runs them on one: `friends-mutual` (`[friends] mode = "mutual"`: only friends are listed, and only friends invite or join) and `identity-required` (`[limits] require_identity`). `federation` needs two servers sharing a coordinator (`--other`); `build/build.sh federation-test` runs it.

Add a scenario for every server change. Model it on the real game's calls; the game's own session log (`bl-tracing.log`) shows its sequence of calls.
