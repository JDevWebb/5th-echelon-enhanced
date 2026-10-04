# Running a network: updates, metrics and the admin UI

A coordinator does more than share friends. It keeps every server in its network on the latest signed release, and gathers what each server sees into an admin web UI: players and where they are, what's being played, pings, load and bandwidth.

## Automatic updates

### How a release reaches the servers

1. **The coordinator finds it.**
   - It asks GitHub for the latest release every 10 minutes.
   - It only takes a release whose `SHA256SUMS` carries the release key's signature for that release's version (its tag), and which has a server build.
   - An unsigned release is never rolled out.
2. **One server goes first (the canary).**
   - The coordinator picks the quietest server that installs updates, and its next heartbeat answer names the release.
   - The canary must run the release and keep reporting in for 10 minutes.
   - If it doesn't come back, or its updater rolls back, the rollout halts there.
3. **The rest follow.**
   - Each server updates when it has no players on.
   - Two hours into the stage, a server updates even with players on.
4. **The coordinator updates itself** once the canary has proved the release, when the installer's updater is on its machine. If that updater fails or rolls back, the coordinator doesn't ask for that release again for 6 hours (it reads `update-status.json`), so a bad release doesn't restart its machine every 30 seconds.

**What's rolled out on its own:** a release newer than the one being rolled out (with none yet, not older than the coordinator's own), and at most one major version past it. Anything else is recorded, and an admin can roll it out (Roll this out). The signature names the version, so an old signed release published again under a new tag doesn't verify.

### On each machine

The server and the coordinator run unprivileged, and can't change their own programs. The installer adds a root updater:

- `/opt/5th-echelon/update.sh`;
- `5th-echelon-update.path`, a systemd unit that starts it when a request appears.

**When a server is told to update:**

1. The server writes the version to `update-request` in its folder.
2. The updater reads the start of that file (never through a link, a pipe or a folder, which it just removes) and checks it's a release number. It does nothing while the installed release isn't known (a `--binary` install whose version it couldn't read), and doesn't try a version that just failed again for an hour.
3. It downloads that release from GitHub, and checks `SHA256SUMS` against the release key (the signature must name that version) and the binaries against `SHA256SUMS`.
4. It keeps the running binaries in `/opt/5th-echelon/previous/`, swaps in the new ones and restarts the services.
5. It waits up to 90 seconds for the server (and coordinator) to answer with the new version. If they don't, the previous binaries go back.
6. It records the result in `/var/lib/5th-echelon-update/update-status.json` (root's; the server and coordinator only read it), which the server reports to the coordinator.

The updater runs as root in a sandbox: it can only write its program folder, its records and status, and the services' requests.

**Downgrades:**
- The updater never installs a release older than the one running, except the one it kept in `previous/`, which is how a rollback works, and only for 14 days after the update.
- It never goes below `/etc/5th-echelon/min-release` (root's): the release the installer last installed, raised to the one each update replaced.
- So a coordinator (or a compromised server) can only ever have a machine install a release signed with the release key: no newer than what's on GitHub, and at most one step back, soon after an update.

### Keeping up is part of membership

A member server drops out of the server directory, and players' launchers stop offering it, when:

- it doesn't install updates (`[federation] auto_update = false`, or no updater, e.g. not installed with the installer); or
- it still runs an older release a day after a rollout finished.

**Being delisted:**
- It keeps sharing friends, and players who already use it can still play there.
- It's listed again as soon as it runs the network's release.

**Turning updates off or on:**
- `install-server.sh --no-auto-update` turns them off for a server; `--auto-update` turns them back on.
- Under Docker, update the image and the directory follows.

### In the admin UI

**Updates** shows:
- the rollout's stage;
- each server's version, its updater's last result, and whether it's listed;
- every signed release seen.

**Actions:**

| Action | What it does |
|---|---|
| Pause / Resume | Stop or restart telling servers to update; they keep what they run |
| Pin / Unpin | A pinned rollout ignores new releases until unpinned |
| Skip the canary | Every server updates now |
| Halt | Stop the rollout where it is |
| Roll back to … | Every server returns to the release before (from its kept copy, so no download), and the rollout is pinned there |
| Roll this out | Roll out an earlier signed release, with a canary, and pin it |
| Check GitHub now | Look for a new release now (and says why one isn't rolled out on its own) |

## Metrics

**Every minute, each server sends its coordinator:**
- players online, in a match and in a lobby;
- every live session's mode (Spies vs Mercs or co-op), room (match or lobby), map and game mode;
- online players per city;
- sign-ins, failed sign-ins, new accounts, failed joins, matches started and relayed traffic;
- the matches that finished since the last report: mode, map, game mode, start and end, the most players at once, and whether it was private;
- the machine's CPU, memory, load, disk, network traffic, and the server process's own CPU and memory.

**Every 10 seconds, a lighter pulse:** players online, in a match and in a lobby; the sign-in, account and match counters; and network traffic. It feeds the live figures and the Live events panel. Older coordinators answer 404, and the server tries again ten minutes later.

**Players and play sessions:** at start, every 5 minutes (what changed) and every 6 hours (the whole roster), each server sends its accounts (name, identity, created, last seen, online, time played, sessions, matches, ban) and its play sessions (sign-in to sign-out). Up to 2,000 players and 5,000 sessions a request; a whole roster that fits in one request also removes accounts deleted on the server. Older coordinators answer 404.

**Who played (older servers):** with each minute's metrics, each server sends a short id for every player online. The id is an HMAC of the player's account number under a key only that server has (`metrics.key`, made on first start), cut to 16 hex characters: the coordinator can count distinct players, new ones and returning ones, but can't tell who they are or match them across servers.

**Locations:**
- Each server looks up its players' addresses in [DB-IP](https://db-ip.com)'s free city database, on its own machine. It downloads the database to `geoip/` and refreshes it monthly.
- Only city counts are sent: addresses never leave the server.

**Pings:**
- The coordinator measures its round trip to each server every minute.
- Launchers report their own ping to each server when they look at the directory. The coordinator notes which city a report came from and keeps nothing else about it. It counts one report per launcher address and server every 10 minutes, so no one address can skew a city's figures.

**History:**
- Samples are kept for a week.
- Hourly summaries are kept for 400 days, for the 30-day and one-year views. They keep each hour's bytes in and out, relayed bytes, peak rate and five-minute rates, so the bandwidth report stays exact after the samples are gone.
- Players per day (by anonymised id, with minutes played), play sessions, finished matches, admins' player actions and resolved alerts are kept for 400 days too.
- Players' global stats (for the leaderboards) are kept for good, until an admin removes someone's.
- The live points and events (from the pulses) are kept a day, so restarting the coordinator keeps the live view.

### Reports

- **Bandwidth:** data in, out and relayed per hour, day or month; totals, the peak, and the 95th percentile of five-minute rates (what burstable plans bill); a row per day (per month over a year), and a CSV export. Set each server's monthly allowance in TB to see how far through it the month is and where it's heading.
- **Players & map › Report & map:** players per period, daily average, new and returning players, time played per player per day, the peak; sign-ins and failed joins per day; a heatmap of when people play, in your time zone; median pings by city; the live map. Time played comes from play sessions for servers that send them (from the first whole day they did), else from a sample each minute.
- **Players & map › Matches:** finished matches per day by mode, matches started, their average length and players, private and public, by mode, game mode and map (name maps as you identify them).

### Players

**Players & map › Accounts** lists every server's accounts: search by name, account number or identity; filter by server, online or banned; sort by last seen, time played, name or when they joined. An account's identity is the same on every server, so the list shows where else the player has an account.

Pick a player for their sessions, time played each day of the last 30, their other accounts and what admins did to them, and to act:

| Action | What it does |
|---|---|
| Kick | Signs them out; they can sign in again |
| Ban | Signs them out and refuses sign-in for 1, 7 or 30 days, or for good, with a reason they see. Optionally every account of theirs |
| Unban | Lifts a ban |
| Reset password | Their server makes a temporary password, shown once to the admin who asked, to give them privately. The coordinator forgets it once shown (or after an hour) |
| Rename | A new name (3 to 24 letters, digits, _ - .), not one another player holds on the network; friends on other servers see it |
| Delete | Removes the account for good (type the name to confirm) |

The server carries an action out when it next sends its pulse (within 10 seconds) and says how it went; the page follows it. An action the server doesn't answer within an hour expires. Every action is in the audit log; ban, reset password, rename and delete ask for your second factor again if the last was more than 10 minutes ago.

### Sessions

**Sessions** shows each player's time on the servers over the last 6 hours to 7 days, on one server or all: a row per player with when they were online, the rooms they were in (a party, or a co-op or Spies vs Mercs match; hatched when private, faded when alone; hover for whose it was and who else was in it), and marks for searches, stats written (a mission or match played to its end) and problems. Click a row, or a problem, for that player's events in order. Server trouble (alerts, and times a server's API didn't answer the coordinator's pings) is shaded behind the rows.

Above it, **Problems** lists what went wrong, worst first in each level:

| Level | What |
|---|---|
| Problem | A player couldn't sign in (each reason and how often: an outdated client, too many attempts, banned...); a join that failed, with the game's code; a player who dropped out of a match: their relayed traffic fell, then they left the match without it ending (no stats written); a game nobody could reach: it signed in but never registered for online play (NAT helper), or its registration lapsed while it was still signed in (joins with it fail with CONNECTION_FAILED until it registers again; restarting the game does) |
| Warning | A request the server failed to answer; an invitation that never reached the player (offline, or out of the online menus); an invitation with no room to join; a game that went quiet in a match with others still in it (a crash, or their connection lost) |
| Note | A player refused who then got in; two players who searched for the same mode on different servers within five minutes and both found nobody; a public match its host waited in alone for a minute or more |

Servers record these as they happen and send them to the coordinator every few seconds. They keep them until the coordinator has them, so an outage loses none, and repeat events (an outdated client retrying its sign-in every few seconds) come as one, with a count. The coordinator keeps them 30 days. The relay sees only relayed players' traffic, so a dropout between two players connected directly shows only as their leaving.

### Leaderboards

Servers forward every stat the game writes to the coordinator, which keeps each person's stats across the network: one entry per identity, however many servers they play on (see [friends.md](friends.md#the-coordinators-api)). The stats are kept for good.

**Players & map › Leaderboards** shows the top 100 of a global leaderboard: pick the leaderboard (solo and co-op high scores and best times, Spies vs Mercs total score, the ladders' time played, wins, deaths from above, takedowns, kills, total score and objectives) and the mission, game mode or ladder; each shows how many it ranks. Best times read m:ss, time played in hours.

**Remove stats** (on a row) removes everything the network keeps for that person, on every leaderboard: a cheater's, say. It asks for your second factor again if the last was more than 10 minutes ago, and is in the audit log. It can't be undone. Their servers keep their own copy, and games they play afterwards count again, so ban them too if they shouldn't come back.

### Alerts

The coordinator checks every minute. An alert opens when its condition starts, updates while it lasts, and resolves itself when it ends:

| Alert | When |
|---|---|
| Offline | No heartbeat for 3 minutes (servers seen in the last week) |
| CPU | Over 90% in at least 3 samples within 5 minutes |
| Memory | Over 90% |
| Disk | Under 10% free |
| Sign-ins refused | 30 or more in 10 minutes |
| Traffic allowance | 80% or 100% of the month's allowance, or on course to pass it |
| Update | A server's update failed or was rolled back, or a rollout halted |

Open alerts show on the Overview and as a badge on **Alerts**. To have them posted to a chat, paste a Discord or Slack incoming webhook under **Alerts**: it must be https and reach a public address. Changing it asks for your second factor, and **Send a test** checks it. Players' reports can go there too (see [Player reports](#player-reports)).

The game reports maps and game modes as numbers. Name them under **Playlists** as you identify them; the names apply everywhere.

## Player reports

After a game session with problems (a failed join, a relayed connection, a crash, a refused sign-in, a different version of the game, a disconnect), and now and then after a normal one, the launcher asks the player how it went. They can rate it, tick what went wrong (couldn't join, lag, crash, connection, game version, sign-in, other) and say what happened, and choose whether to attach their logs. The launcher sends the report to the server they played on, which adds its own log lines about that player and a summary of their recent session, and forwards it to its coordinator (`POST /v1/reports`, see [friends.md](friends.md#the-coordinators-api)).

**What's in a report:**
- the player's account number, name and identity on that server;
- their rating, the problems they ticked and their comment (up to 2,000 characters);
- why the launcher asked (e.g. `failed_join`, `relayed`, `panic`, `exit_code`, `routine`);
- the launcher's and client's versions, the game's build, Windows version and language;
- the server's summary of their session, and its log lines about them;
- the logs they chose to attach: up to 8 files, 4 MB each.

**Redaction happens on the player's PC:** the launcher redacts the logs before they leave it, and attaches them only if the player agrees: home folders (`C:\Users\<name>`, `/home/<name>`) become `<user>`, the PC's and the Windows account's names become `<private>`, and public IPv4 addresses other than the server's lose their last two numbers (`122.58.x.x`). LAN addresses stay (they show players on one network). The player can read every file before sending. What's attached: the client's log and the one before it, `bl-dataversion.txt`, the launcher's log, the launcher's checklist and the server's summary of the session; each is cut to its last 3 MB. The coordinator stores what it's sent as it is; it checks sizes and names, and that each file decompresses to what it says.

**Where it's kept:**
- The report in the coordinator's database; the files on disk in its data folder (`reports/<report id>/<name>.gz`, gzip as received; `/var/lib/5th-echelon-coordinator` with the installer).
- Reports and their files are kept 90 days, removed with the hourly rollup.
- The files take at most 2 GB: past that, the oldest reports' files go first, and their reports stay (the page says the files were removed).
- Each server may send 120 new reports an hour.

**In the admin UI, under Reports:** the open reports (or resolved, or all), filtered by server or problem, or searched by player, comment, identity or report id; the badge on **Reports** counts the open ones. A report shows what the player said, why they were asked, their client, a link to their account under **Players**, the server's summary and log lines, each file (view it in the page, the first 2 MB, with a search; or download it), and the player's other reports (the same account, or the same identity on any server). **Resolve** or **Reopen** it with a note; **Delete** removes it and its files (it asks for your second factor again if the last was more than 10 minutes ago). All three are in the audit log.

**Alerts:** with the alert webhook set (see Alerts), each new report is posted to it, e.g. "📝 Report from ijsman5530 on eu1: couldn't join, lag — "couldn't join my friend"", with a link to it. Under **Alerts › Player reports**: post the reports rated bad or with problems ticked (the default), all of them, or none. Past 5 in a minute, the rest wait and go together in one message ("7 new reports …") a minute later. A player's words can't ping anyone: mentions are broken up.

## The admin UI

### Turning it on

The admin UI is served by the coordinator on its own port (127.0.0.2:8701, a loopback address the game server's sandbox can't reach), at a name of its own, and only through Cloudflare.

1. **Choose a name one level below your domain**, e.g. `scbl-metrics.example.com`. Cloudflare's free edge certificate covers `example.com` and `*.example.com` only, so a name like `metrics.scbl.example.com` fails with a TLS error (unless you pay for Advanced Certificate Manager). The community network uses `scbl-metrics.jdevwebb.net`.
2. **Add the DNS record, proxied.** In Cloudflare, add an A record for the name pointing at the coordinator's machine, **Proxied** (orange cloud). Under **SSL/TLS**, set the mode to **Full (strict)**.
3. **Make an Origin CA certificate.**
   - In Cloudflare, go to **SSL/TLS › Origin Server › Create Certificate**. Keep "Generate private key and CSR with Cloudflare", add the name (or `*.example.com`), and choose PEM.
   - Save the certificate and the private key as two files, and copy them to the server yourself (`scp origin.pem origin.key root@server:`).
   - The key is a secret: never paste it anywhere else.

   Let's Encrypt can't reliably certify a proxied name: its TLS-ALPN challenge never passes through Cloudflare. The Origin CA certificate is free, lasts up to 15 years, and is trusted by Cloudflare, the only client this site accepts.
4. **Run the installer** on the coordinator's machine with the name and the certificate:

   ```sh
   sudo bash install-server.sh --metrics-domain scbl-metrics.example.com \
     --metrics-cert origin.pem --metrics-key origin.key
   rm origin.key origin.pem
   ```

   - **Checks:** that the certificate covers the name and matches the key.
   - **What it keeps:** its own copy under `/etc/caddy`, with the key readable by Caddy only, which later runs reuse.
5. **Optional extra gates:**
   - **Authenticated Origin Pulls:** turn it on in Cloudflare (**SSL/TLS › Origin Server**), and run the installer with `--cloudflare-origin-pull`. Caddy then refuses any TLS connection without Cloudflare's client certificate.
   - **Cloudflare WAF rules, or a [Cloudflare Access](https://developers.cloudflare.com/cloudflare-one/policies/access/) policy:** a second check before the UI's own sign-in.
6. **Add yourself:**

   ```sh
   sudo bash install-server.sh --add-admin yourname
   ```

   This prints a one-time setup link. It works for 24 hours.
7. **Set up your sign-in.** Open the link, choose a password, and add a passkey (recommended) or an authenticator app. Save the recovery codes it shows you.

**Cloudflare leaves the pages as they are:** the admin UI sends `Cache-Control: no-store, no-transform`, so Cloudflare neither caches it nor adds scripts of its own (Web Analytics, bot detection, Rocket Loader), which the UI's content security policy would block.

**Caddy refuses requests that don't come from Cloudflare's addresses**, so nobody reaches the UI straight from the internet. That's Cloudflare as a whole, though, not your zone: someone with a Cloudflare account of their own can point a proxied name at the server's address, and their requests come from Cloudflare's addresses too, past your zone's WAF rules and Access policy (Authenticated Origin Pulls with Cloudflare's shared certificate doesn't tell zones apart either). The UI's own sign-in (password plus a passkey or authenticator app) is what keeps them out. The client's address and country are Cloudflare's (`CF-Connecting-IP`, `CF-IPCountry`), and are only as trustworthy as that.

### Signing in

**What signs an admin in:**
- A password (12 characters or more, Argon2id) and a second factor:
  - a passkey (Touch ID, Windows Hello, a security key or a phone);
  - an authenticator app code (each code works once);
  - a recovery code (ten, each works once).
- Or a passkey alone: it verifies the admin with a PIN or biometric, so it's two factors on its own.

**Every account always keeps a second factor**, and a new admin must add one before seeing anything.

**Sessions:**
- They end after 30 minutes idle, 12 hours in all, or when the session's country changes.
- Sessions are listed under **Security**, and any can be ended.

**Sensitive changes ask for a second factor again** if the last was more than 10 minutes ago:
- adding or resetting admins;
- sign-in restrictions;
- rollbacks;
- removing servers;
- banning, renaming or deleting a player, and resetting their password;
- removing a person's stats.

**Failed sign-ins:**
- Five in a row lock the account for 15 minutes, doubling after each further failure.
- Sign-in attempts are also limited per address.

**Everything is in the audit log** for 400 days: sign-ins, failures and changes.

### Where admins may sign in from

Under **Security**, limit the admin UI to address ranges (e.g. your home's address) and countries.

- Every request is checked, the sign-in page too. A request from anywhere else gets a refusal and nothing more.
- Rules that would lock out the admin saving them are refused.

### When something's wrong

| Problem | What to do |
|---|---|
| Every admin locked out by the restrictions | `sudo bash install-server.sh --admin-open-access` clears them |
| An admin lost their second factor | Another admin uses **Reset sign-in**, or on the machine: `sudo bash install-server.sh --reset-admin NAME`. Either gives a new setup link |
| Cloudflare shows error 525 | Caddy has no certificate for the name: install an Origin CA certificate (step 3) |
| A TLS error before Cloudflare answers | The name is two levels deep, which Cloudflare's free certificate doesn't cover: use a one-level name (step 1) |
| `ERR_HTTP2_PROTOCOL_ERROR` right after switching the record to proxied | Your computer still has the server's own address cached, and Caddy drops anything that isn't from Cloudflare. Flush your DNS cache (macOS: `sudo dscacheutil -flushcache; sudo killall -HUP mDNSResponder`) |
| "is an admin already" from `--add-admin` | They were added before: `--reset-admin NAME` gives a new setup link |

### Live updates

The dashboards keep themselves current: the page holds a WebSocket to the coordinator (`/api/live`) and changes in place, with no reloading.
- **Overview and server status:** sent within a couple of seconds of a server's heartbeat (every 30 seconds) or metrics (every minute).
- **Charts:** fetch their newest points when a server reports new numbers.
- **Audit log and Admin activity:** new entries appear as they happen.
- **Live figures and Live events:** players online and bandwidth now, from every server's 10-second pulse; events like "6 players signed in" or "a match started" are counts only.
- **Alerts:** appear and resolve as the coordinator raises them.
- **Reports:** new players' reports appear in the list, and the badge on **Reports** follows.

The badge at the top right says whether it's live. The connection uses the same sign-in as the rest of the admin UI: it ends when the session does, and only the admin UI's own site may open it (the browser's `Origin` is checked). Behind Caddy and Cloudflare, WebSockets pass through as they are.

### Privacy

**What the admin UI shows:**
- counts, cities, and the names of admins;
- each server's accounts: names, identities, when they played, bans; never players' addresses or passwords.
- players' reports: what they said, and their logs and the server's.

**What's stored:**
- Players' locations are city counts.
- Each server's accounts and play sessions, as it reports them (see Metrics), so admins can manage players.
- Players' session events (sign-ins, rooms, searches, joins, invitations, problems), with their names but no addresses, for 30 days on the coordinator (a few days on each server).
- The per-minute "who played" ids are a per-server keyed hash of the account number, with minutes played per day; the reports count players per server, so someone playing on two servers counts twice.
- Launchers' ping reports keep only the city and the round trip.
- Players' reports, with the logs they chose to attach (redacted on their PC) and their server's log lines about them, for 90 days (see [Player reports](#player-reports)).

IP geolocation by [DB-IP](https://db-ip.com), CC BY 4.0. The world map is [Natural Earth](https://www.naturalearthdata.com) (public domain), via [world-atlas](https://github.com/topojson/world-atlas) (ISC).
