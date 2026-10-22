# Backups

The installer can back up a server's databases off the machine:

- **Snapshots, to Cloudflare R2.** Every 2 hours, a copy of each database (SQLite's own backup, taken while the server runs) goes to R2, and a week of them is kept. A database can be restored as it was in the latest snapshot, or in the last one before a moment in the past week. A snapshot that hasn't changed since the last one isn't sent again (but one a day is, all the same).
- **An archive, to Backblaze B2.** Each day at about 03:30 UTC, a copy of each database, restored from the latest snapshot (which proves the snapshots restore), goes to B2. Daily copies are kept for 30 days, and the copy made on the 1st of each month for 12 months.

What's backed up:

| What | Snapshots (R2) | Archive (B2) |
|---|---|---|
| The game server's database: accounts, friends, stats, bans | `snapshots/NAME/game/TIME.db.gz` | `archive/NAME/daily/DAY-game.db.gz` |
| The coordinator's database (where one runs): links, names, global stats, admins, server secrets | `snapshots/NAME/coordinator/TIME.db.gz` | `archive/NAME/daily/DAY-coordinator.db.gz` |
| The coordinator's join token and players' reports (with each snapshot) | `live/NAME/coordinator-files/` | `archive/NAME/daily/DAY-coordinator-files.tar.gz` |

`NAME` is `BACKUP_NAME` if you set it, else the server's name in its [failover group](failover.md), else the game server's id (`/var/lib/5th-echelon/server-id.txt`). `TIME` is when the snapshot was taken, in UTC (`20261020T020000Z`). Monthly copies are under `archive/NAME/monthly/MONTH-…`.

**What it costs.** Each snapshot sent is one write request to R2 (a Class A operation), and listing the folder to drop snapshots over a week old, or sending the coordinator's files, is a few more. That's a couple of thousand requests a month for a server at most, well inside R2's free million. (Before 0.4.6, Litestream sent each change a second or so after it, which on a busy server went past the free requests.) Change `SNAPSHOT_HOURS` at the top of the installer to take them more or less often. Even every 15 minutes stays well inside the free requests.

**Upgrading from Litestream (before 0.4.6).** Running the installer again stops and removes Litestream's services, settings and the folder it kept beside each database, and takes the first snapshot straight away. What Litestream sent to R2 (`live/NAME/game` and `live/NAME/coordinator`) is left there; delete those two folders in the R2 dashboard once the snapshots are there (`backup.sh status`). In a failover group, update every server together: a standby on an older release restores from Litestream's copy, which nothing updates any more.

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

Without the `BACKUP_B2_` lines, there are only the snapshots.

**4. Run the installer again** (`sudo bash install-server.sh`). It installs rclone (a pinned version, checked against its checksum) and SQLite's command-line tool, and the snapshot and daily archive timers, and takes the first snapshot. Then:

```sh
sudo /opt/5th-echelon/backup.sh status
```

## How it runs

- `backup.sh snapshot` runs every 2 hours (`5th-echelon-backup-snapshot.timer`): for each database on this machine, SQLite takes a copy as the database's own user (WAL mode means the server doesn't wait for it), the copy is checked whole, and sent to R2 if it changed. Then the coordinator's join token and reports go up, where the coordinator runs.
- `backup.sh archive` runs daily (`5th-echelon-backup-archive.timer`). Both log to the journal: `journalctl -u 5th-echelon-backup-snapshot`.
- The keys are only in `backup.env`, read by `backup.sh` as root. Each key only reaches its own bucket.
- A snapshot and a restore never run at once. Run `sudo /opt/5th-echelon/backup.sh snapshot` by hand before anything risky (or before moving the coordinator, [failover.md](failover.md#moving-the-coordinator)).

## Restoring

```sh
# The game server's database as it is in the latest snapshot:
sudo /opt/5th-echelon/backup.sh restore game
# As it was in the last snapshot at or before a moment in the last 7 days (UTC):
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

The coordinator's join token and reports are in the day's `coordinator-files.tar.gz` (B2) and under `live/NAME/coordinator-files/` (R2). `--with-files` puts them back with the coordinator's database, from R2:

```sh
sudo /opt/5th-echelon/backup.sh restore coordinator --from OLDNAME --with-files
```

This is what a standby does when it takes over ([failover.md](failover.md)). On a standby, where the coordinator doesn't run, there's nothing of the coordinator's to back up: its snapshots, files and archive are the coordinator's machine's.
