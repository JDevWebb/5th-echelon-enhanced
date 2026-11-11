# Backups

The installer can back up a server's databases off the machine:

- **Snapshots, to Cloudflare R2.** Every 2 hours, a copy of each database (SQLite's own backup, taken while the server runs) goes to R2, and a week of them is kept. A database can be restored as it was in the latest snapshot, or in the last one before a moment in the past week. A snapshot that hasn't changed since the last one isn't sent again (but one a day is, all the same).
- **An archive, to Backblaze B2.** Each day at about 03:30 UTC, a copy of each database, from the latest snapshot, goes to B2. Daily copies are kept for 30 days, and the copy made on the 1st of each month for 12 months.
- **Encrypted, a bucket each.** Everything is encrypted on the server before it goes up ([age](https://age-encryption.org), to a public key), and each server sends to buckets of its own with keys that reach only those. Only the machines that may restore hold the key that opens the backups. See [Encryption](#encryption) and [A bucket each](#a-bucket-each); a server set up before 0.5.0 keeps working as it was until you [upgrade it](#upgrading-from-one-shared-bucket).

What's backed up:

| What | Snapshots (R2) | Archive (B2) |
|---|---|---|
| The game server's database: accounts, friends, stats, bans | `snapshots/NAME/game/TIME.db.gz.age` | `archive/NAME/daily/DAY-game.db.gz.age` |
| The coordinator's database (where one runs): links, names, global stats, admins, server secrets | `snapshots/NAME/coordinator/TIME.db.gz.age` | `archive/NAME/daily/DAY-coordinator.db.gz.age` |
| The coordinator's join token and players' reports (with each snapshot) | `live/NAME/coordinator-files.tar.age` | `archive/NAME/daily/DAY-coordinator-files.tar.gz.age` |
| The game server's key files (its id, federation secret, NAT, API, metrics and relay-pass keys) and `service.toml` (with each snapshot) | `live/NAME/game-files.tar.age` | |

Without encryption, the names are the same without `.age`, and the live files are folders (`live/NAME/coordinator-files/`, `live/NAME/game-files/`). `NAME` is `BACKUP_NAME` if you set it, else the server's name in its [failover group](failover.md), else the game server's id (`/var/lib/5th-echelon/server-id.txt`). `TIME` is when the snapshot was taken, in UTC (`20261020T020000Z`). Monthly copies are under `archive/NAME/monthly/MONTH-…`.

**What it costs.** Each snapshot sent is one write request to R2 (a Class A operation), and listing the folder to drop snapshots over a week old, or sending the coordinator's files, is a few more. That's a couple of thousand requests a month for a server at most, well inside R2's free million. (Before 0.4.6, Litestream sent each change a second or so after it, which on a busy server went past the free requests.) Change `SNAPSHOT_HOURS` at the top of the installer to take them more or less often. Even every 15 minutes stays well inside the free requests. Buckets cost nothing by themselves on either, so one per server costs the same as one shared.

**Upgrading from Litestream (before 0.4.6).** Running the installer again stops and removes Litestream's services, settings and the folder it kept beside each database, and takes the first snapshot straight away. What Litestream sent to R2 (`live/NAME/game` and `live/NAME/coordinator`) is left there; delete those two folders in the R2 dashboard once the snapshots are there (`backup.sh status`). In a failover group, update every server together: a standby on an older release restores from Litestream's copy, which nothing updates any more.

## Why encrypted, and a bucket each

A snapshot holds the server's secrets: the coordinator's database has every member server's secret, and a game server's key files are what make it that server to the network. With one bucket and one key shared by every server (the setup before 0.5.0), root on any one server, or a `backup.env` that leaked, could read every server's secrets from the bucket, and delete every server's backups.

- **Encrypted:** the servers only hold the public key, which can seal but not open. A server, or its `backup.env`, gives nothing away of the backups in the bucket, its own included. Only the machines that restore (the standbys of a failover group, or the machine you restore on by hand) hold the key that opens them.
- **A bucket each:** each server's key reaches only its own buckets, so a server broken into can't touch another's backups. A standby that restores another server reads that server's bucket with a **read-only** key.

## Setting them up

What you create by hand is marked **(you)**; the installer and `backup.sh` do the rest.

### 1. The age key pair, once per network (you)

On a machine you trust (your own computer, or the first server), with [age](https://github.com/FiloSottile/age) installed (`sudo apt install age`):

```sh
age-keygen -o backup.age
# Public key: age1...
```

- `backup.age` is the **private key** (age calls it an identity). It opens every backup of the network. Keep a copy offline (a password manager, a USB stick in a drawer): without it, encrypted backups can't be restored at all.
- The `age1...` line (also `age-keygen -y backup.age`) is the **public key**. It's safe to share; it goes in every server's `backup.env` as `BACKUP_AGE_RECIPIENT`.

### 2. Buckets and keys, per server (you)

For each server (here `eu1`), on both services. All buckets private.

**Cloudflare R2** (the snapshots):
- R2 › Create bucket: `fe-eu1` (any name; one per server).
- R2 › Manage API tokens › Create API token: **Object Read & Write**, *Apply to specific buckets only* › `fe-eu1`. Note the access key id and secret, and the endpoint `https://ACCOUNT_ID.r2.cloudflarestorage.com`. This is eu1's own key; it goes only in eu1's `backup.env`.
- For each machine that restores eu1 (its standbys): another token, **Object Read only**, for `fe-eu1` only. This read-only key goes in that machine's `backup-peers.env` (step 4).

**Backblaze B2** (the archive):
- Buckets › Create a Bucket: `fe-eu1` (B2 bucket names are global: add something of yours), private. Set its lifecycle to **Keep only the last version of the file**: otherwise B2 keeps every file the archive's 30-day and 12-month clean-up removes, and the bucket only grows.
- App Keys › Add a New Application Key: access to `fe-eu1` only, **Read and Write**. Note the key id and key, and the bucket's S3 endpoint (Buckets › your bucket: `https://s3.REGION.backblazeb2.com`).
- If a standby should restore eu1 from its archive too (not needed for failover, which uses the snapshots): another key for `fe-eu1`, **Read Only**.

A server's own keys can still delete its own backups (it has to, to drop old ones). Against a server broken into deleting its own, keep the offline copies above, or turn on B2's Object Lock for the archive bucket with a retention shorter than 30 days.

### 3. `/etc/5th-echelon/backup.env` on each server (you)

Root's only (`sudo install -m 600 /dev/null /etc/5th-echelon/backup.env`, then edit it). One `NAME=value` a line, no quotes or spaces. Its own buckets and keys, and the public key:

```sh
BACKUP_R2_ENDPOINT=https://ACCOUNT_ID.r2.cloudflarestorage.com
BACKUP_R2_BUCKET=fe-eu1
BACKUP_R2_ACCESS_KEY_ID=...
BACKUP_R2_SECRET_ACCESS_KEY=...
BACKUP_B2_ENDPOINT=https://s3.REGION.backblazeb2.com
BACKUP_B2_BUCKET=fe-eu1-yourname
BACKUP_B2_ACCESS_KEY_ID=...
BACKUP_B2_SECRET_ACCESS_KEY=...
BACKUP_AGE_RECIPIENT=age1...
# Optional: the name this machine's backups are kept under (default: its name in its
# failover group, else its server id).
# BACKUP_NAME=eu1
```

Without the `BACKUP_B2_` lines, there are only the snapshots. `BACKUP_AGE_RECIPIENT` can list more than one public key, comma-separated (a second key kept offline, say); any of them opens the backups.

### 4. On the machines that restore: the private key, and the other servers' read-only keys (you)

Only on the machines that may restore (each standby of a [failover group](failover.md), or a machine you restore on by hand). Not on the others.

- **The private key**, as `/etc/5th-echelon/backup.age`, root's only:

  ```sh
  sudo install -m 600 -o root -g root backup.age /etc/5th-echelon/backup.age
  ```

- **`/etc/5th-echelon/backup-peers.env`**, root's only (`sudo install -m 600 /dev/null /etc/5th-echelon/backup-peers.env`): for each server this machine may restore, its bucket and the read-only key for it. Lines start `PEER_` and the server's backup name (a `-` in it written as `_`):

  ```sh
  # eu1's bucket, read only
  PEER_eu1_R2_ENDPOINT=https://ACCOUNT_ID.r2.cloudflarestorage.com
  PEER_eu1_R2_BUCKET=fe-eu1
  PEER_eu1_R2_ACCESS_KEY_ID=...
  PEER_eu1_R2_SECRET_ACCESS_KEY=...
  # Optional: eu1's archive, read only (for restore --archive --from eu1)
  # PEER_eu1_B2_ENDPOINT=https://s3.REGION.backblazeb2.com
  # PEER_eu1_B2_BUCKET=fe-eu1-yourname
  # PEER_eu1_B2_ACCESS_KEY_ID=...
  # PEER_eu1_B2_SECRET_ACCESS_KEY=...
  ```

  `restore --from eu1` reads eu1's bucket with these. A name with no entry here is read from this machine's own buckets (as with one shared bucket).

Which machine needs what, for a failover group of eu1 (the coordinator), na1 and oceania, each standing by for the coordinator:

| | `backup.env` | `backup.age` | `backup-peers.env` |
|---|---|---|---|
| eu1 | its own buckets and keys, the public key | yes (it restores when the coordinator is moved back with `--take-over`) | na1 and oceania, read-only |
| na1 | its own, the public key | yes | eu1 and oceania, read-only |
| oceania | its own, the public key | yes | eu1 and na1, read-only |
| a game server in no group | its own, the public key | no | no |

For the matchmaking server's standbys (its relays), the same: each relay has the private key and a read-only entry for the matchmaking server's bucket.

### 5. Run the installer again

`sudo bash install-server.sh` (or `--backups-only` for nothing else). It installs rclone (a pinned version, checked against its checksum), age and SQLite's command-line tool, and the snapshot and daily archive timers, and takes the first snapshot. Then:

```sh
sudo /opt/5th-echelon/backup.sh status
```

It says whether the backups are encrypted, whether the key that opens them is on this machine, and which servers this machine restores from.

## Encryption

- **What's sealed:** every snapshot, the coordinator's files and the game server's key files and settings (each as one tar), and the daily and monthly archive: everything `backup.sh` sends. It's sealed on the server, in its root-only work folder, with age to the public keys in `BACKUP_AGE_RECIPIENT` (age: X25519 and ChaCha20-Poly1305). A server needs no secret to send; nothing on it can open what it sent.
- **What opens it:** `backup.sh restore`, with `/etc/5th-echelon/backup.age`. A restore that needs it on a machine without it stops before anything changes. The opened copy is only ever in the restore's own work folder (root's, removed after).
- **The archive** goes to B2 from the latest snapshot as it is, still sealed. Where the private key is on the machine, the snapshot is opened and checked whole first; elsewhere, only that it's sealed.
- **Once a server has sent encrypted backups, it never sends unencrypted ones again:** if `BACKUP_AGE_RECIPIENT` goes missing from its `backup.env`, the snapshots and the archive stop with an error (in the journal, and in `backup.sh status`) instead of going up unencrypted. To really go back to unencrypted, delete `/var/lib/5th-echelon-backup/encrypted`.
- **Without `BACKUP_AGE_RECIPIENT`** (a server set up before 0.5.0): the backups go up unencrypted, as before, with a warning in the journal each time, in `backup.sh status` and from the installer.
- **Old copies:** restores open sealed copies and read unencrypted ones (from before encryption) alike. When the first sealed copy of the live files is up, the unencrypted folder they replace (`live/NAME/coordinator-files/`, `live/NAME/game-files/`) is deleted. Unencrypted snapshots go when they're a week old; delete the unencrypted archive (`archive/NAME/…/*.gz`, without `.age`) by hand when you no longer want it.
- **Changing the key pair:** make a new pair, put the new private key on the machines that restore beside the old (age's `-i` takes one file: put both keys in `backup.age`, one under the other), set the new public key in every `backup.env`. When the old snapshots and archives have aged out, take the old key out.

## A bucket each

- Each server sends only to its own buckets (`BACKUP_R2_BUCKET`, `BACKUP_B2_BUCKET`), with keys limited to those.
- A restore of another server (`--from NAME`) reads NAME's bucket with the read-only key for it in `backup-peers.env`; without an entry there, this machine's own buckets (so one shared bucket keeps working as it did).
- The backups are still kept under `snapshots/NAME/…` inside each bucket, as before, so a shared bucket and a bucket each look the same from inside.

## Upgrading from one shared bucket

A server set up before 0.5.0 (one bucket and one key for every server, unencrypted) keeps working as it is after the update; nothing changes until you do this. Server by server, in any order, without stopping anything:

1. **Once:** make the age key pair (step 1).
2. **For each server:** its own buckets and keys (step 2), then in its `backup.env`, change `BACKUP_R2_BUCKET`, `BACKUP_B2_BUCKET` and their keys to its own, and add `BACKUP_AGE_RECIPIENT`. Run `sudo bash install-server.sh --backups-only`: it installs age and takes a snapshot, encrypted, into the new bucket straight away, with its live files.
3. **On the machines that restore** (step 4): `backup.age`, and in `backup-peers.env` a read-only entry for each server they restore, pointing at its **new** bucket. In a failover group, do this on every standby **before** moving that server to its new bucket: until then a standby reads the old shared bucket (no entry), where the server no longer sends.
4. **When every server has moved** and a week has passed (its snapshots in the new bucket cover the week): delete the old shared bucket's `snapshots/` and `live/` in R2, and its `archive/` in B2 once you no longer want the old unencrypted archive (up to 12 months of monthly copies). Then delete the shared key in both dashboards.

To restore from the old shared bucket meanwhile (a snapshot older than the move), add a read-only entry for that server's name pointing at the old bucket to `backup-peers.env` for the restore, and remove it after.

To encrypt without changing buckets (or the other way round), do just that part: each works without the other.

## How it runs

- `backup.sh snapshot` runs every 2 hours (`5th-echelon-backup-snapshot.timer`): for each database on this machine, SQLite takes a copy as the database's own user (WAL mode means the server doesn't wait for it), the copy is checked whole, sealed, and sent to R2 if it changed. Then the coordinator's join token and reports go up, where the coordinator runs, and the game server's key files and settings.
- `backup.sh archive` runs daily (`5th-echelon-backup-archive.timer`). Both log to the journal: `journalctl -u 5th-echelon-backup-snapshot`.
- The keys are only in `backup.env` (and `backup-peers.env`, `backup.age` where they are), read by `backup.sh` as root. Each key only reaches its own bucket.
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

An encrypted copy needs `/etc/5th-echelon/backup.age` on the machine. The copy is checked first (a whole SQLite database, by SQLite's `PRAGMA integrity_check`); a copy that isn't whole changes nothing. Then the service stops, the database it had is kept beside it (`….db.before-restore-TIME`, with its WAL), the copy goes in, and the service starts again.

**A lost machine:** install on a new one with the same settings, write its `backup.env` (its own new buckets, or the lost machine's with `BACKUP_NAME` set to the lost machine's name), put `backup.age` there, and a read-only entry for the lost machine's bucket in `backup-peers.env` if it isn't this machine's. Then restore from the lost machine's backups:

```sh
sudo /opt/5th-echelon/backup.sh restore game --from OLDNAME
sudo /opt/5th-echelon/backup.sh restore coordinator --from OLDNAME
```

The coordinator's join token and reports are in the day's `coordinator-files.tar.gz.age` (B2) and in `live/NAME/coordinator-files.tar.age` (R2). `--with-files` puts them back with the coordinator's database, from R2; for a game server, its key files and settings:

```sh
sudo /opt/5th-echelon/backup.sh restore coordinator --from OLDNAME --with-files
sudo /opt/5th-echelon/backup.sh restore game --from OLDNAME --with-files
```

This is what a standby does when it takes over ([failover.md](failover.md)). On a standby, where the coordinator doesn't run, there's nothing of the coordinator's to back up: its snapshots, files and archive are the coordinator's machine's.

**A game server's settings are never restored as they are.** A server's backups are only as trustworthy as the server: whoever broke into it (or holds its bucket's key; the public key seals for anyone) could put settings in its `service.toml` for the machine that restores it, such as debug switches, trusted proxies, federation over plain HTTP or other files to serve. So `--with-files` keeps **this machine's own `service.toml`** (as its installer wrote it: its addresses, ports, Caddy, `[public]` names, limits) and takes from the backup only what makes it the same server to the network: `[role]` `kind`, `ticket_key`, `moved_to`; `[matchmaking]` `network_relays`; `[federation]` `name`, `region`, `listed`, `listed_as`, `auto_update`; `[friends]` `mode`; `[admin]` `enabled`; `[limits]` `open_registration`, `require_identity`; `[nat]` `relay`. Each is checked as the installer checks it, and a value it wouldn't write stops the restore before anything changes. One missing from the backup takes its default. The coordinator's address stays this machine's (taken from the backup only when this machine has none). Run the installer on a new machine first, so it has settings of its own to start from; anything else you set by hand in the old server's `service.toml`, set again.
