# Failover

A network's servers all depend on its coordinator: friends across servers, the server directory the launcher browses, stats, names and updates. If the coordinator's machine goes down, every server loses those until it's back. In a **failover group**, each game server also keeps a standby coordinator, and when the coordinator is down for long enough, the next server in line takes over, with the coordinator's data as it was a second or so before.

## How it works

- **The coordinator's record decides where it runs.** Its name (e.g. `play.example.net`) is proxied through Cloudflare (orange cloud), so changing the record moves everyone at once: launchers and servers keep the same name and reach the new machine within seconds, whatever they cached. Each server of the group serves the name in Caddy with a Cloudflare Origin certificate, so any of them can take it.
- **Each server runs a standby** (`5th-echelon-standby`, the coordinator program as root). Every 15 seconds it reads the record through Cloudflare's API:
  - **pointing here:** it makes sure the coordinator runs here, and that the names that follow it (the admin UI's) point here too;
  - **pointing at another server:** it makes sure the coordinator *doesn't* run here, and asks for the coordinator through Cloudflare.
- **Taking over.** When Cloudflare answers for the coordinator with an error from its server (the machine is down, or Caddy or the coordinator on it), the standbys wait their turn: **3 minutes** for the first in line, **6** for the second, and so on, so they don't race. Then the standby:
  1. asks the other servers of the group (their `/api/info`) whether they still reach the coordinator. If one does, the problem is between this server and Cloudflare, and nothing happens;
  2. restores the coordinator from the live backup of the server it replaces (`live/NAME/coordinator` in R2, with its join token and players' reports; see [backups.md](backups.md)), and starts it;
  3. looks at the record once more (another server may have taken over meanwhile), and points it, and the admin UI's, here.
- **Standing down.** A server that was replaced and comes back sees the record pointing elsewhere and keeps its coordinator stopped. It doesn't take the coordinator back by itself: the data moved on without it. Moving the coordinator back is an admin's choice ([below](#moving-the-coordinator)).
- **What doesn't trigger it:** no answer from Cloudflare at all (this server's own network), an answer that isn't an error from the coordinator's server (a Cloudflare challenge, say), a server that still reaches the coordinator, and Cloudflare's API not answering. A coordinator that stops for an update is back well within 3 minutes, and the updater pauses the standby on its own machine meanwhile.

The game servers themselves keep running throughout: players in matches aren't affected, and sign-ins on each server work as before. While the coordinator is down, friends on other servers, the directory, stats across servers and new names wait (servers queue their changes and send them when it's back).

## Setting it up

Every server of the group needs 0.4.2 or newer (the standby is part of the coordinator program, and servers say in `/api/info` whether they reach the coordinator).

The examples use `play.example.net` for the coordinator, `metrics.example.net` for the admin UI, and three servers `eu1`, `na1` and `oceania` (in the order they take over).

**1. Backups on every server of the group.** The standbys restore the coordinator from its live backup, so it must be running on the coordinator's machine, and each standby needs the keys to read it. Set up [backups.md](backups.md) on each server, with the same R2 bucket. Leave `BACKUP_NAME` out: each server's backups are then kept under its name in the group, which is where the others look.

**2. In Cloudflare:**
- **An Origin certificate** (SSL/TLS › Origin Server › Create certificate) for the coordinator's name and the admin UI's: `play.example.net, metrics.example.net`. Save the certificate and its key on each server (e.g. `/root/origin.pem` and `/root/origin.key`, root's only).
- **SSL/TLS mode: Full (strict).**
- **The coordinator's record proxied** (orange cloud), one A record, no AAAA. Do this before step 4: with the Origin certificate, Caddy refuses everything that doesn't come through Cloudflare, and the installer stops if the name doesn't answer through Cloudflare. The coordinator's current certificate (Let's Encrypt) works with Full (strict) until then.
- **An API token** (My Profile › API Tokens › Create Token › *Edit zone DNS*), for the zone only. Under *Client IP Address Filtering*, allow only the group's servers' addresses: the token can change any record in the zone. Note the **zone ID** too (the zone's Overview page).
- **Security:** keep **Bot Fight Mode** off, and no *I'm Under Attack* mode or WAF rule that challenges the coordinator's name. Servers and launchers can't answer a challenge, and their requests would fail. Rate limiting is the coordinator's own.

**3. `/etc/5th-echelon/standby.env` on each server**, root's only (`sudo install -m 600 /dev/null /etc/5th-echelon/standby.env`, then edit it):

```sh
CLOUDFLARE_API_TOKEN=...
CLOUDFLARE_ZONE_ID=...
```

**4. Run the installer on each server**, the coordinator's first (here eu1), with the same group on every one:

```sh
sudo bash install-server.sh \
  --coordinator-domain play.example.net \
  --coordinator-cert /root/origin.pem --coordinator-key /root/origin.key \
  --metrics-domain metrics.example.net \
  --metrics-cert /root/origin.pem --metrics-key /root/origin.key \
  --standby eu1=eu1.example.net,na1=na1.example.net,oceania=oceania.example.net
```

Each entry is a server's name in the group (its backup name) and its `--domain`; the installer finds itself in the list by its domain. Leave out `--metrics-domain` and its certificate if the admin UI shouldn't move with the coordinator. Later runs keep all of this.

On the coordinator's machine, the coordinator keeps running, and from now on the standby starts it at boot (when the record points there). On the others, the installer installs the coordinator and its Caddy site, but doesn't start it. A server that already joined the coordinator stays joined; a new one needs `--join-token` as usual.

**5. Check** on each server:

```sh
sudo /opt/5th-echelon/coordinator standby --status
```

It shows where the record points, what Cloudflare answers for the coordinator, how long this server would wait, and whether the others reach the coordinator. `journalctl -u 5th-echelon-standby` has what the standby did.

## Moving the coordinator

To move the coordinator to a server of the group (back to eu1 after it took over elsewhere, or ahead of maintenance), run on that server:

```sh
sudo /opt/5th-echelon/coordinator standby --take-over
```

It restores the coordinator from the live backup of the server it runs on, starts it here, and points the records here. The other server stands down within 15 seconds. Changes made there in those few seconds (a friend added, a stat) can be lost, so move it when it's quiet.

## A drill

Stopping the coordinator alone doesn't test anything: the standby on its machine starts it again. To see a takeover, stop both on the coordinator's machine (here eu1):

```sh
sudo systemctl stop 5th-echelon-standby 5th-echelon-coordinator
```

After about 3 minutes, na1 takes over (`journalctl -u 5th-echelon-standby -f` there). Then start eu1's standby again (`sudo systemctl start 5th-echelon-standby`): it stands by. Move the coordinator back with `--take-over` on eu1.

## Leaving a group

On each server: `sudo systemctl disable --now 5th-echelon-standby`, and delete `/etc/5th-echelon/standby`, `standby.conf` and `standby.env`. On the server the coordinator runs on, run the installer again: the coordinator starts at boot as before. On the others, also delete `/etc/5th-echelon/coordinator-domain` and run the installer with `--coordinator https://play.example.net`, so they only join it; the installer refuses to start a new, empty coordinator on a server that already belongs to one.
