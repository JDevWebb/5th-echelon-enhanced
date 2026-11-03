# Moving a network to global matchmaking, all at once

A network of regional servers (each its own accounts, matchmaking and relay) becomes one
**matchmaking server** that every player signs in to, with the other servers as **relays** near
their players. Players keep their accounts: each server's are merged into the matchmaking
server's by identity, and launchers move their players by themselves. Done in one update
window, after a full rehearsal on temporary servers.

The community network's example: na1 becomes the matchmaking server at `match.scbl.jdevwebb.net`;
eu1 and oceania become relays (eu1 keeps the coordinator).

## What it needs

- Every server on the same release (0.4.8 or later) and **backups on** on each ([backups.md](backups.md)).
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
   - coord: `--coordinator-domain coord.test... --matchmaking-server <mm's id later>`;
   - mm: `--domain mm.test... --matchmaking --mm-record match.test... --mm-standby mm=mm.test...,relay-a=relay-a.test...,relay-b=relay-b.test...`;
   - relays: `--domain relay-x.test... --role relay --ticket-key <mm's key> --mm-record match.test... --mm-standby` (the same list).
2. **Copies of the production databases** (from their latest snapshots: `backup.sh restore game
   --from <name>` into a scratch folder, or the snapshot in R2): eu1's, na1's and oceania's. This is
   player data on temporary machines: keep it to the rehearsal and wipe the VPSes after.
3. **The merge**, as on the night: on mm, with its server stopped, put na1's copy in place as
   its database, then
   `cd /var/lib/5th-echelon && sudo -u echelon /opt/5th-echelon/dedicated_server --merge-accounts /tmp/eu1.db --merge-accounts /tmp/oceania.db`.
   It prints what it moved; check: accounts = na1's + eu1's + oceania's − people on more than
   one; names renamed (expected none); a few players' friends and bans by hand.
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

1. **The release first, as usual:** every server and launcher on the new release.
2. **Stop new sign-ins on eu1 and oceania** (their game service stops), then let their
   outboxes empty (`SELECT COUNT(*) FROM stats_outbox`, `report_outbox`, `federation_outbox`
   all 0; the merge refuses otherwise) and take a fresh snapshot of each (`backup.sh snapshot`).
3. **Merge:** copy eu1's and oceania's databases to na1; stop na1's game service; take a
   snapshot of na1 first; run `--merge-accounts` with both; start it again.
4. **Turn eu1 and oceania into relays:** the installer with `--role relay --ticket-key <na1's key>
   --mm-record match.scbl.jdevwebb.net --mm-standby na1=...,eu1=...,oceania=...`. Their own
   databases stay as they are (the rollback's), and their `/api/info` now says `moved_to`.
5. **na1:** the installer with `--mm-record` and `--mm-standby` (the same list); the coordinator
   names it the matchmaking server (`--matchmaking-server <na1's id>` on eu1's coordinator).
6. **Check:** test players at match.scbl...; a launcher on each old server moves by itself; the
   Sessions page shows sign-ins from every region; relays' `/api/info` say `matchmaking_seen`.

### Rollback (decide its triggers before the night)

Triggers, for example: sign-ins failing for more than a few players for 15 minutes; joins failing
well above the usual rate for 30 minutes; the matchmaking server's CPU or memory near its limit.

1. Stop the matchmaking standbys (`systemctl disable --now 5th-echelon-mm-standby`) everywhere.
2. eu1 and oceania: the installer with `--role game` (and without `--mm-record`): their own
   databases, untouched since step 2, come back; `moved_to` goes.
3. na1: restore its snapshot from before the merge (`backup.sh restore game --time <then>`).
4. Launchers that moved stay on na1 until their players switch back (Servers): say so in the news.
   Accounts made on na1 meanwhile stay there.

## After

- Archive the old regional databases; purge their rosters from the coordinator after a grace period.
- Watch the Sessions page for a week: sign-in and join times by region, relayed traffic, far rooms.
