# Moving a network to global matchmaking, all at once

A network of regional servers (each its own accounts, matchmaking and relay) becomes one
**matchmaking server** that every player signs in to, with the other servers as **relays** near
their players. Players keep their accounts: each server's are merged into the matchmaking
server's by identity, and launchers move their players by themselves. Done in one update
window, after a full rehearsal on temporary servers.

The example: three regional servers, `eu`, `na` and `oceania`, with the coordinator on `eu`.
`na` becomes the matchmaking server at `play.example.net`, the one server every player signs in
to (the launcher pings the relays and has the game use the nearest); `eu` and `oceania` become
relays (`eu` keeps the coordinator, at `coord.example.net`). Players can still type any other
server's address, as now.

## The names

| Name | Points at | Cloudflare |
|---|---|---|
| `play.example.net` | the matchmaking server (`na`); its standby moves it | DNS only, TTL 60 s |
| `coord.example.net` | the coordinator (`eu`) | DNS only (proxied once its failover is set up) |
| `na`, `eu`, `oceania.example.net` | each machine, never moved | DNS only |

The matchmaking server's name has to be DNS only: the game's UDP and plain port-80 traffic can't
pass Cloudflare's proxy. If players were handed the coordinator's name until now, give the
coordinator a new one-level name of its own (one level, so it can be proxied later with
Cloudflare's free certificate) and keep the old one as an alias (`--coordinator-alias`) until
launchers have moved; the matchmaking server then takes the name players know.

The directory lists the matchmaking server by its record (`[federation] listed_as`, which the
installer sets from `--mm-record` on the matchmaking server), so a player set up from the
directory is set up on `play.example.net`, which follows it in a failover. A launcher set up on
`na`'s own name moves to the record by itself (the directory lists `na`'s name among the
matchmaking server's other names, only for the server the coordinator's operator named).

## What it needs

- Every server on the same release (0.5.0 or later) and **backups on** on each ([backups.md](backups.md)): encrypted, and on each relay (the matchmaking server's standbys) the key that opens them and a read-only key to the matchmaking server's bucket ([backups.md, step 4](backups.md#4-on-the-machines-that-restore-the-private-key-and-the-other-servers-read-only-keys-you)).
- Every account linked to an identity (`SELECT COUNT(*) FROM users WHERE ubi_id IS NOT NULL AND
  global_id IS NULL` is 0 on each server); an account without one is moved under its name, but
  its launcher can't follow by itself.
- **The matchmaking server's name:** an A record, **DNS only** (grey cloud), **TTL 60 s**,
  pointing at the matchmaking server. Players and servers use this name; its standby moves it.
- **The matchmaking server listed in the network's directory** (it is, as a member of the coordinator): a launcher follows a move only to a server its network's directory lists, read from the coordinator over HTTPS.
- Cloudflare's API token and zone in `/etc/5th-echelon/standby.env` on each server (as for the
  coordinator's failover: [failover.md](failover.md)).

## 1. The rehearsal (temporary servers)

Four small VPSes: `coord`, `mm`, `relay-a`, `relay-b`, in the regions you'll use, with test
names in your zone (`coord.test.example.net`, ...). The matchmaking server's test name is DNS
only, TTL 60. Then:

1. **Install them** as they'll be, with the release candidate:
   - coord: `--coordinator-domain coord.test... --coordinator-only --matchmaking-server <mm's id, once mm is installed>`;
   - mm: `--domain mm.test... --matchmaking --mm-record match.test... --mm-standby mm=mm.test...,relay-a=relay-a.test...,relay-b=relay-b.test...`;
   - relays: `--domain relay-x.test... --role relay --ticket-key <mm's key> --mm-record match.test... --mm-standby` (the same list).
2. **Copies of the production databases**, taken with SQLite's own backup while the servers run
   (`sudo -u echelon sqlite3 /var/lib/5th-echelon/5th-echelon.db ".backup /tmp/x/copy.db"`), or
   from their latest snapshots (`backup.sh restore game --from <name>` into a scratch folder):
   eu's, na's and oceania's. This is player data on temporary machines: keep it to the
   rehearsal and wipe the VPSes after.
3. **The merge**, as on the night: on mm, with its server stopped, **delete its database's
   `5th-echelon.db-wal` and `-shm`** (as root: `/var/lib/5th-echelon` is the server's only), put
   na's copy in place as its database, then
   `cd /var/lib/5th-echelon && sudo -u echelon /opt/5th-echelon/dedicated_server --merge-accounts /tmp/eu.db --merge-accounts /tmp/oceania.db`.
   It prints what it moved; check: accounts = na's + eu's + oceania's − people on more than
   one, without each server's two sample accounts (ids 1 and 105, which aren't moved); names
   renamed (expected none); friends, requests and blocks between the same people before and
   after (none lost); uploads = distinct (person, kind) across the three (the newer kept).
   Then `copy-links` for each merged server on the coordinator, as on the night (step 3 there),
   and the same count check.
   **The old database's `-wal` and `-shm` matter:** left beside a database copied in under the
   same name, SQLite replays the old journal over it on open, and the merge goes into the old
   database (a rehearsal did this once: about a fifth of the accounts missing).
4. **Load:** test players at production volume against mm, relayed at both relays
   (`build/build.sh load` with `SERVER=mm...`), with the Sessions page watched.
5. **Real games:** Windows PCs (from NZ and further), a Steam Deck: sign-in, quick match
   across the relays, the Matches tab, joins, host migration, a long session. Note sign-in,
   search and join times from each region.
6. **The automatic move:** a launcher set up on relay-a's name (as an eu player's is) starts,
   is told relay-a moved to match.test..., and signs in there with its identity.
7. **A failover drill:** stop mm (`systemctl stop 5th-echelon-mm-standby 5th-echelon`); within
   about 3 minutes relay-a takes over (its log: "Took over from mm"), match.test... points at it,
   and games sign in there. Then move it back (`coordinator standby --config
   /etc/5th-echelon/mm-standby.conf --take-over` on mm, after a fresh `backup.sh snapshot` on
   relay-a).
8. **A full rollback**, as on the night (below), timed.
9. **Wipe** the VPSes.

Go ahead only when every step above passed; note what it took and how long.


## 2. The update window

Book it ([update windows](operations.md)), long enough for the merge and checks (an hour).
Announce it in the launcher's news and Discord.

1. **The release first, as usual:** every server and launcher on the new release. **eu first,
   with the coordinator's new name**, right after publishing: the release's installer with
   `--coordinator-domain coord.example.net --coordinator-alias play.example.net` (its record
   made beforehand, DNS only to eu). Caddy gets a certificate for each name, and the coordinator
   answers to both, so launchers that saved the old name carry on. Check
   `curl https://coord.example.net/v1/info`, and that the coordinator's own switches are still on
   (`systemctl cat 5th-echelon-coordinator`). Then every member server with
   `--coordinator https://coord.example.net` as it updates (its join is kept).
2. **Stop new sign-ins on eu and oceania** (their game service stops), then let their
   outboxes empty (`SELECT COUNT(*) FROM stats_outbox`, `report_outbox`, `federation_outbox`
   all 0; the merge refuses otherwise) and take a fresh snapshot of each (`backup.sh snapshot`).
3. **Merge:** copy eu's and oceania's databases to na (from their snapshots, or SQLite's
   `.backup`: never a plain copy of a database in use); stop na's game service; take a
   snapshot of na first; run `--merge-accounts` with both, against na's own database in place
   (nothing copied over it); start it again. The rehearsal's merge took a second. **Then their
   players' links:** on the coordinator (eu), first save na's as they are, for the rollback:
   `sudo -u echelon-coord /opt/5th-echelon/coordinator --data /var/lib/5th-echelon-coordinator
   save-links <na's id> /var/lib/5th-echelon-coordinator/na-links-before-merge.json` (it won't
   write over a file that's there); then `... copy-links <eu's id> <na's id>`, and the same for
   oceania's id (it prints any left out: na has that identity or name linked otherwise; check each).
   A link keeps the name it had on eu or oceania: an account the merge renamed (it prints them;
   none expected) needs its link's name put right by hand.
   A link is the player's signed word, made once when they first link: na can't make them again,
   and without them every moved player's friends fail ("that player isn't linked on this
   server"). Check: na's accounts with an identity
   (`SELECT COUNT(*) FROM users WHERE global_id IS NOT NULL`) against the coordinator's links for
   na (`SELECT COUNT(*) FROM links WHERE server_id = '<na's id>'`): the same, less any left out.
4. **na becomes the matchmaking server:** the installer with `--matchmaking --mm-record
   play.example.net --mm-standby na=...,eu=...,oceania=... --server-name "<the network's name>"`.
   It makes its relay-pass key as it starts and logs it ("Relay passes are signed with the key
   ..."): the relays need it, so na goes before them.
5. **Move the name:** in Cloudflare, `play.example.net`'s A record to na, **DNS only**, TTL 60 s.
6. **Turn eu and oceania into relays:** the installer with `--role relay --ticket-key <na's key>
   --mm-record play.example.net --mm-standby` (the same list). On eu the same run drops the old
   name from the coordinator and names the matchmaking server: add `--coordinator-alias -`
   (without it the installer keeps the alias it remembers) and `--matchmaking-server <na's id>`.
   Do eu's in this one run: any run of the installer starts the service it installs, so a run
   of its own to drop the alias would open eu's game to sign-ins again in the middle of the
   window. Their own databases stay as they are (the rollback's), and their `/api/info` now
   says `moved_to`.
7. **Check:** test players at `play.example.net` (`testbot --server play.example.net --info`,
   and `cross-relay` through each relay); the directory lists one server, `play.example.net`,
   with `na.example.net` in its `also`, and its `helpers` the relays too (a relay is offered once
   the coordinator's host check has seen it answer at its address: within minutes of the
   upgrade); a launcher on each old server (eu, oceania and
   na's own name) moves by itself; the Sessions page shows sign-ins from every region; relays'
   `/api/info` say `matchmaking_seen`.

### Rollback (decide its triggers before the night)

Triggers, for example: sign-ins failing for more than a few players for 15 minutes; joins failing
well above the usual rate for 30 minutes; the matchmaking server's CPU or memory near its limit.

1. Stop the matchmaking standbys (`systemctl disable --now 5th-echelon-mm-standby`) everywhere.
2. eu and oceania: the installer with `--role game` (and without `--mm-record`): their own
   databases, untouched since step 2, come back; `moved_to` goes. `play.example.net` stays on
   na: launchers that moved keep signing in there.
3. na: restore its snapshot from before the merge (`backup.sh restore game --time <then>`).
   Then its links: on the coordinator, `... restore-links
   /var/lib/5th-echelon-coordinator/na-links-before-merge.json` (the file step 3 saved). The
   links `copy-links` gave na go (players from eu and oceania would otherwise be linked on na
   without an account there: the rehearsal had 529 links for na's 268 identities), links made on
   na since the save stay (players who signed up meanwhile), and it prints how many of each.
4. Launchers that moved stay on na until their players switch back (Servers): say so in the news.
   Accounts made on na meanwhile stay there.

## After

- Archive the old regional databases; purge their rosters from the coordinator after a grace period.
- Watch the Sessions page for a week: sign-in and join times by region, relayed traffic, far rooms.
