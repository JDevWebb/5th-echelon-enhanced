# Match recordings in R2

From 0.5.6 a network can keep its players' match recordings in a Cloudflare R2 bucket for 30
days, for reviewing reported matches. Without it, each server keeps the recordings its players
send for a week, and the admin UI's replay viewer asks that server for one.

## What happens

- A player's game sends each finished match recording to their server (as before).
- With a key, the server puts it in R2 at `recordings/<YYYY>/<MM>/<id>.fer`, with its note
  (`<id>.json`) beside it. Uploads are retried every minute. A recording here is then a cache,
  deleted two days after it went up. One that hasn't gone up within a day is an alert on the
  admin UI ("hasn't put a recording in R2").
- Recordings already on the server when it first starts with a key go up too.
- The coordinator, with its own read-only key, fetches a recording from R2 when an admin opens
  it. It still asks the player's server when it has no key, or R2 doesn't have it.
- The bucket deletes each recording 30 days after it went up (a lifecycle rule).

## Setting it up

1. **A bucket**, for example `fe-recordings`. Add a lifecycle rule on the prefix `recordings/`:
   delete objects 30 days after upload.
2. **A key for each server** that takes recordings (the matchmaking server, or any game
   server): an R2 API token with Object Read & Write on that bucket only.
3. **A read-only key for the coordinator**: Object Read only, on that bucket only.
4. On each machine, as root, write `/etc/5th-echelon/recordings-r2.env` (mode 600):

   ```
   FE_RECORDINGS_R2_ENDPOINT=https://<account id>.r2.cloudflarestorage.com
   FE_RECORDINGS_R2_BUCKET=fe-recordings
   FE_RECORDINGS_R2_KEY_ID=<the token's access key id>
   FE_RECORDINGS_R2_SECRET=<the token's secret access key>
   ```

5. Run `install-server.sh` again (or add `EnvironmentFile=-/etc/5th-echelon/recordings-r2.env`
   to the units by hand), then restart the services. The server's log says
   `Recordings: kept in R2 (...)`, and the coordinator's
   `recordings: read from R2 (...)`.

The services read the file through systemd, as root, so it can stay readable by root only.

## Cost

About 65 MB of recordings a day at today's play, so about 2 GB held at a time, and about 1,100
uploads a day. Both are inside R2's free allowance (10 GB stored, a million writes a month).
