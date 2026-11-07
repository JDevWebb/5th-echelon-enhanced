# Moving a network to global matchmaking, all at once

A network of regional servers (each its own accounts, matchmaking and relay) becomes one
**matchmaking server** that every player signs in to, with the other servers as **relays** near
their players. Players keep their accounts: each server's are merged into the matchmaking
server's by identity, and launchers move their players by themselves. Done in one update
window, after a full rehearsal on temporary servers.

The community network's example: na1 becomes the matchmaking server at `scbl.jdevwebb.net`, the
one server every player signs in to (the launcher offers no choice of server on the community
network; it pings the relays and uses the nearest); eu1 and oceania become relays (eu1 keeps
the coordinator, now at `scbl-coord.jdevwebb.net`). Players can still type any other server's
address, as now.

## The names

| Name | Points at | Cloudflare |
|---|---|---|
| `scbl.jdevwebb.net` | the matchmaking server (na1); its standby moves it | DNS only, TTL 60 s |
| `scbl-coord.jdevwebb.net` | the coordinator (eu1) | as `scbl.jdevwebb.net` is now: DNS only (proxied once its failover is set up) |
| `na1`, `eu1`, `oceania.scbl.jdevwebb.net` | each machine, never moved | DNS only |

`scbl.jdevwebb.net` was the coordinator's name until 0.4.7 (DNS only to eu1, a Let's Encrypt
certificate). It becomes the matchmaking server's, which has to be DNS only (the game's UDP and
plain port-80 traffic can't pass Cloudflare's proxy), and the coordinator takes a one-level name
of its own (one level, so it can be proxied later with Cloudflare's free certificate). 0.5.0 launchers use the new
name, and take a saved old one for it. Older launchers find their update on GitHub, not through
the coordinator, and can't play on 0.5.0 servers before updating, so nothing is passed on to the
coordinator from the old name once it moves.

The directory lists the matchmaking server by its record (`[federation] listed_as`, which the
installer sets from `--mm-record` on the matchmaking server), so a player set up from the
directory is set up on `scbl.jdevwebb.net`, which follows it in a failover. A launcher set up on
na1's own name moves to the record by itself (the directory lists na1's name among the
matchmaking server's other names, only for the server the coordinator's operator named).

## What it needs

- Every server on the same release (0.5.0 or later) and **backups on** on each ([backups.md](backups.md)).
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
   eu1's, na1's and oceania's. This is player data on temporary machines: keep it to the
   rehearsal and wipe the VPSes after.
3. **The merge**, as on the night: on mm, with its server stopped, **delete its database's
   `5th-echelon.db-wal` and `-shm`** (as root: `/var/lib/5th-echelon` is the server's only), put
   na1's copy in place as its database, then
   `cd /var/lib/5th-echelon && sudo -u echelon /opt/5th-echelon/dedicated_server --merge-accounts /tmp/eu1.db --merge-accounts /tmp/oceania.db`.
   It prints what it moved; check: accounts = na1's + eu1's + oceania's − people on more than
   one, without each server's two sample accounts (ids 1 and 105, which aren't moved); names
   renamed (expected none); friends, requests and blocks between the same people before and
   after (none lost); uploads = distinct (person, kind) across the three (the newer kept).
   Then `copy-links` for each merged server on the coordinator, as on the night (step 3 there),
   and the same count check. (The 2026-11-04 rehearsal's coordinator was a new one, without
   production's links: its 446 merged accounts had none until they were put in by hand on
   2026-11-07.)
   **The old database's `-wal` and `-shm` matter:** left beside a database copied in under the
   same name, SQLite replays the old journal over it on open, and the merge goes into the old
   database (the 2026-11-04 rehearsal did this once: 357 accounts instead of 448).
4. **Load:** test players at production volume against mm, relayed at both relays
   (`build/build.sh load` with `SERVER=mm...`), with the Sessions page watched.
5. **Real games:** Windows PCs (from NZ and further), a Steam Deck: sign-in, quick match
   across the relays, the Matches tab, joins, host migration, a long session. Note sign-in,
   search and join times from each region.
6. **The automatic move:** a launcher set up on relay-a's name (as an eu1 player's is) starts,
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

1. **The release first, as usual:** every server and launcher on the new release. **eu1 first,
   with the coordinator's new name**, right after publishing (0.5.0 launchers use
   `scbl-coord.jdevwebb.net`; until it answers they fall back to their saved server list): the
   release's installer with `--coordinator-domain scbl-coord.jdevwebb.net --coordinator-alias
   scbl.jdevwebb.net --coordinator-alias play.scbl.jdevwebb.net` (its record, DNS only to eu1,
   was made on 2026-11-04). Caddy gets a certificate for each name, and the coordinator answers to
   all three. Check `curl https://scbl-coord.jdevwebb.net/v1/info`, and that its `--roadmap
   --chat` are still on (`systemctl cat 5th-echelon-coordinator`: the 0.5.0 installer knows the
   new name as the community's). Then every member server with `--coordinator
   https://scbl-coord.jdevwebb.net` as it updates (its join is kept).
2. **Stop new sign-ins on eu1 and oceania** (their game service stops), then let their
   outboxes empty (`SELECT COUNT(*) FROM stats_outbox`, `report_outbox`, `federation_outbox`
   all 0; the merge refuses otherwise) and take a fresh snapshot of each (`backup.sh snapshot`).
3. **Merge:** copy eu1's and oceania's databases to na1 (from their snapshots, or SQLite's
   `.backup`: never a plain copy of a database in use); stop na1's game service; take a
   snapshot of na1 first; run `--merge-accounts` with both, against na1's own database in place
   (nothing copied over it); start it again. The rehearsal's merge took a second. **Then their
   players' links:** on the coordinator (eu1), `sudo -u echelon-coord /opt/5th-echelon/coordinator
   --data /var/lib/5th-echelon-coordinator copy-links <eu1's id> <na1's id>`, and the same for
   oceania's id (it prints any left out: na1 has that identity or name linked otherwise; check each).
   A link keeps the name it had on eu1 or oceania: an account the merge renamed (it prints them;
   none expected) needs its link's name put right by hand.
   A link is the player's signed word, made once when they first link: na1 can't make them again,
   and without them every moved player's friends fail ("that player isn't linked on this server",
   found in the 2026-11-07 rehearsal). Check: na1's accounts with an identity
   (`SELECT COUNT(*) FROM users WHERE global_id IS NOT NULL`) against the coordinator's links for
   na1 (`SELECT COUNT(*) FROM links WHERE server_id = '<na1's id>'`): the same, less any left out.
4. **Move the name:** first, on eu1, drop it from the coordinator (the installer with
   `--coordinator-alias play.scbl.jdevwebb.net` alone); then in Cloudflare, `scbl.jdevwebb.net`'s
   A record to na1, **DNS only**, TTL 60 s.
5. **Turn eu1 and oceania into relays:** the installer with `--role relay --ticket-key <na1's key>
   --mm-record scbl.jdevwebb.net --mm-standby na1=...,eu1=...,oceania=...`. Their own
   databases stay as they are (the rollback's), and their `/api/info` now says `moved_to`.
6. **na1:** the installer with `--matchmaking --mm-record scbl.jdevwebb.net --mm-standby` (the
   same list) `--server-name "5th Echelon Community"`: it's listed by `scbl.jdevwebb.net` from
   then on. The coordinator names it the matchmaking server: the installer on eu1 with
   `--matchmaking-server <na1's id>`.
7. **Check:** test players at `scbl.jdevwebb.net` (`testbot --server scbl.jdevwebb.net --info`,
   and `cross-relay` through each relay); the directory lists one server, `scbl.jdevwebb.net`,
   with `na1.scbl.jdevwebb.net` in its `also`; a launcher on each old server (eu1, oceania and
   na1's own name) moves by itself; the Sessions page shows sign-ins from every region; relays'
   `/api/info` say `matchmaking_seen`.

### Rollback (decide its triggers before the night)

Triggers, for example: sign-ins failing for more than a few players for 15 minutes; joins failing
well above the usual rate for 30 minutes; the matchmaking server's CPU or memory near its limit.

1. Stop the matchmaking standbys (`systemctl disable --now 5th-echelon-mm-standby`) everywhere.
2. eu1 and oceania: the installer with `--role game` (and without `--mm-record`): their own
   databases, untouched since step 2, come back; `moved_to` goes. `scbl.jdevwebb.net` stays on
   na1: launchers that moved keep signing in there.
3. na1: restore its snapshot from before the merge (`backup.sh restore game --time <then>`).
4. Launchers that moved stay on na1 until their players switch back (Servers): say so in the news.
   Accounts made on na1 meanwhile stay there.

## After

- Archive the old regional databases; purge their rosters from the coordinator after a grace period.
- Watch the Sessions page for a week: sign-in and join times by region, relayed traffic, far rooms.
