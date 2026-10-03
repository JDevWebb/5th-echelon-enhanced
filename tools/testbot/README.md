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
| `identity-login` | An identity links to an account, and signs in to it with a new password. |
| `rename` | Renaming keeps the account (friends, id) and frees the old name. |
| `direct-test` | The launcher's direct-connection test reaches this machine: behind a reverse proxy too, where the server must use the forwarded address. |
| `presence` | Online means signed in to the game service: a ticket alone (the launcher's connection test) isn't, and leaving ends it. |
| `slow-handshake` | A game far from the server (its first resend comes before the answer) sends SYN and CONNECT twice and switches to the last SYN answer's signature: it must still sign in and play. |
| `second-sign-in` | The newest sign-in wins: the same account signing in from another PC closes the first game's connection, and what it left (its lobby) goes with it; the new one works as usual. |
| `ticket-elsewhere` | A ticket works only from the address that asked for it: someone who saw it (and the CONNECT) on the way can't sign in with it from elsewhere. Needs a server on loopback, where 127.0.0.2 is another address. |
| `private-room-join` | Nobody joins a private match uninvited: not by naming themselves twice (private and public), nor for sharing some other room with the host. The host's own party follows it in, by adding itself or being added; a stranger can't be pulled in. |
| `split-limits` | Splitting makes sessions: only of a session the player is in (or just left), and no more than creating them would. |
| `private-room-hidden` | A private match isn't handed to strangers: not in matchmaking, and not by searching for its host. Its own players, and the invited, still find it. |
| `station-url-schemes` | A station URL's address is checked whatever its scheme: a `udp:` URL naming someone else's address is corrected, and a host name never reaches other players. |
| `syn-flood` | A flood of SYNs that never go on to CONNECT (what forged sources look like) leaves nothing behind, so it doesn't keep anyone from signing in, not even from the same address. |
| `login-lockout` | Wrong passwords for an account from one address stop that address only: its owner, elsewhere, still signs in. (Through the loopback "proxy", so against a server on this machine.) |
| `outdated-client` | Clients older than the server allows can't sign in, and neither can the game after one tried; a current client lets it in. |

Two more need a server set up for them, and `build/build.sh bots` runs them on one: `friends-mutual` (`[friends] mode = "mutual"`: only friends are listed, and only friends invite) and `identity-required` (`[limits] require_identity`). `federation` needs two servers sharing a coordinator (`--other`); `build/build.sh federation-test` runs it.

Add a scenario for every server change. Model it on the real game's calls; the game's own session log (`bl-tracing.log`) shows its sequence of calls.
