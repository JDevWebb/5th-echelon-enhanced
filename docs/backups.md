# Backups

The installer can back up a server's databases off the machine:

- **Live, to Cloudflare R2.** [Litestream](https://litestream.io) sends each database's changes to R2 a second or so after they happen, with a full snapshot each day and a week of history. A database can be restored as it is now, or as it was at any moment in the last 7 days.
- **An archive, to Backblaze B2.** Each day at about 03:30 UTC, a copy of each database, restored from the live copy (which proves the live copy restores), goes to B2. Daily copies are kept for 30 days, and the copy made on the 1st of each month for 12 months.

What's backed up:

| What | Live (R2) | Archive (B2) |
|---|---|---|
| The game server's database: accounts, friends, stats, bans | `live/NAME/game` | `archive/NAME/daily/DAY-game.db.gz` |
| The coordinator's database (where one runs): links, names, global stats, admins, server secrets | `live/NAME/coordinator` | `archive/NAME/daily/DAY-coordinator.db.gz` |
| The coordinator's join token and players' reports (hourly) | `live/NAME/coordinator-files/` | `archive/NAME/daily/DAY-coordinator-files.tar.gz` |

`NAME` is the game server's id (`/var/lib/5th-echelon/server-id.txt`) unless you set `BACKUP_NAME`. Monthly copies are under `archive/NAME/monthly/MONTH-…`.

Backups need the databases in SQLite's WAL mode, which the server and coordinator use from 0.4.2. On an older release, the installer says so and starts the live copy once the server is updated and the installer runs again.

## Setting them up

**1. Buckets.** One bucket on R2 and one on B2 (the same name works, e.g. `5th-echelon-enhanced`). Both private.
- On B2, set the bucket's lifecycle to **Keep only the last version of the file**. Otherwise B2 keeps every file the archive's 30-day and 12-month clean-up removes, and the bucket only grows.

**2. Keys, limited to their bucket.**
- **R2:** R2 › Manage API tokens › Create, with **Object Read & Write**, applied to that bucket only. Note the access key id and secret, and the endpoint: `https://ACCOUNT_ID.r2.cloudflarestorage.com`.
- **B2:** App Keys › Add a New Application Key, with access to that bucket only, read and write. Note the key id and key, and the bucket's S3 endpoint (Buckets › your bucket: `https://s3.REGION.backblazeb2.com`).

**3. `/etc/5th-echelon/backup.env` on each server**, root's only (`sudo install -m 600 /dev/null /etc/5th-echelon/backup.env`, then edit it). One `NAME=value` a line, no quotes or spaces:

```sh
BACKUP_R2_ENDPOINT=https://ACCOUNT_ID.r2.cloudflarestorage.com
BACKUP_R2_BUCKET=5th-echelon-enhanced
BACKUP_R2_ACCESS_KEY_ID=...
BACKUP_R2_SECRET_ACCESS_KEY=...
BACKUP_B2_ENDPOINT=https://s3.REGION.backblazeb2.com
BACKUP_B2_BUCKET=5th-echelon-enhanced
BACKUP_B2_ACCESS_KEY_ID=...
BACKUP_B2_SECRET_ACCESS_KEY=...
# Optional: the name this machine's backups are kept under (default: its server id).
# BACKUP_NAME=eu1
```

Without the `BACKUP_B2_` lines, there's only the live copy.

**4. Run the installer again** (`sudo bash install-server.sh`). It installs Litestream and rclone (pinned versions, checked against their checksums), a backup service for each database, and the daily and hourly timers. Then:

```sh
sudo /opt/5th-echelon/backup.sh status
```

## How it runs

- Each database's live copy is a service of its own (`5th-echelon-backup`, `5th-echelon-coordinator-backup`). It runs as the database's own user, sandboxed to its folder, and stops and starts with its server: when the updater stops the server to install a release, or puts the databases back after a failed one, the backup stops with it and starts again after.
- The keys are only in `backup.env`. Systemd reads it as root and passes it to the backup services, so the backup processes (and so their database's user) have them. Each key only reaches its own bucket.
- `backup.sh archive` runs daily (`5th-echelon-backup-archive.timer`) and `backup.sh files` hourly on the coordinator's machine (`5th-echelon-backup-files.timer`). Both log to the journal: `journalctl -u 5th-echelon-backup-archive`.

## Restoring

```sh
# The game server's database as it is in the live copy now:
sudo /opt/5th-echelon/backup.sh restore game
# As it was at a moment in the last 7 days (UTC):
sudo /opt/5th-echelon/backup.sh restore game --time 2026-10-05T03:00:00Z
# From the archive: a day's copy, or a month's:
sudo /opt/5th-echelon/backup.sh restore game --archive 2026-10-01
sudo /opt/5th-echelon/backup.sh restore coordinator --archive 2026-09
```

The copy is checked first (a whole SQLite database, and `PRAGMA integrity_check` when Python is there); a copy that isn't whole changes nothing. Then the service stops, the database it had is kept beside it (`….db.before-restore-TIME`, with its WAL), the copy goes in, and the service starts again.

**A lost machine:** install on a new one with the same settings, write the same `backup.env` (with `BACKUP_NAME` set to the lost machine's name, or keep it and use `--from`), then restore from the lost machine's backups:

```sh
sudo /opt/5th-echelon/backup.sh restore game --from OLDNAME
sudo /opt/5th-echelon/backup.sh restore coordinator --from OLDNAME
```

The coordinator's join token and reports are in the day's `coordinator-files.tar.gz` (B2) and under `live/NAME/coordinator-files/` (R2).
