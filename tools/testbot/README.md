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

Add a scenario for every server change. Model it on the real game's calls; the game's own session log (`bl-tracing.log`) shows its sequence of calls.
