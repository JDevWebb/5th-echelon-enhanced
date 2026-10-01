# Running a network: updates, metrics and the admin UI

A coordinator does more than share friends. It keeps every server in its network on the latest signed release, and gathers what each server sees into an admin web UI: players and where they are, what's being played, pings, load and bandwidth.

## Automatic updates

### How a release reaches the servers

1. **The coordinator finds it.**
   - It asks GitHub for the latest release every 10 minutes.
   - It only takes a release whose `SHA256SUMS` carries the release key's signature, and which has a server build.
   - An unsigned release is never rolled out.
2. **One server goes first (the canary).**
   - The coordinator picks the quietest server that installs updates, and its next heartbeat answer names the release.
   - The canary must run the release and keep reporting in for 10 minutes.
   - If it doesn't come back, or its updater rolls back, the rollout halts there.
3. **The rest follow.**
   - Each server updates when it has no players on.
   - Two hours into the stage, a server updates even with players on.
4. **The coordinator updates itself** once the canary has proved the release, when the installer's updater is on its machine.

### On each machine

The server and the coordinator run unprivileged, and can't change their own programs. The installer adds a root updater:

- `/opt/5th-echelon/update.sh`;
- `5th-echelon-update.path`, a systemd unit that starts it when a request appears.

**When a server is told to update:**

1. The server writes the version to `update-request` in its folder.
2. The updater checks the version is a release number (nothing else is read).
3. It downloads that release from GitHub, and checks `SHA256SUMS` against the release key and the binaries against `SHA256SUMS`.
4. It keeps the running binaries in `/opt/5th-echelon/previous/`, swaps in the new ones and restarts the services.
5. It waits up to 90 seconds for the server (and coordinator) to answer with the new version. If they don't, the previous binaries go back.
6. It records the result in `update-status.json`, which the server reports to the coordinator.

**Downgrades:**
- The updater never installs a release older than the one running, except the one it kept in `previous/`, which is how a rollback works.
- So a coordinator can only ever have servers install a release signed with the release key: no newer than what's on GitHub, and no older than the one they replaced.

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
| Check GitHub now | Look for a new release now |

## Metrics

**Every minute, each server sends its coordinator:**
- players online, in a match and in a lobby;
- every live session's mode (Spies vs Mercs or co-op), room (match or lobby), map and game mode;
- online players per city;
- sign-ins, failed sign-ins, new accounts and relayed traffic;
- the machine's CPU, memory, load, disk, network traffic, and the server process's own CPU and memory.

**Locations:**
- Each server looks up its players' addresses in [DB-IP](https://db-ip.com)'s free city database, on its own machine. It downloads the database to `geoip/` and refreshes it monthly.
- Only city counts are sent: addresses never leave the server.

**Pings:**
- The coordinator measures its round trip to each server every minute.
- Launchers report their own ping to each server when they look at the directory. The coordinator notes which city a report came from and keeps nothing else about it.

**History:**
- Samples are kept for a week.
- Hourly summaries are kept for 400 days, for the 30-day and one-year views.

The game reports maps and game modes as numbers. Name them under **Playlists** as you identify them; the names apply everywhere.

## The admin UI

### Turning it on

The admin UI is served by the coordinator on its own port (127.0.0.1:8701), at a name of its own, and only through Cloudflare.

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
4. **Optional: Authenticated Origin Pulls.**
   - Turn it on in Cloudflare: **SSL/TLS › Origin Server**.
   - Run the installer with `--cloudflare-origin-pull`. Caddy then refuses any TLS connection without Cloudflare's client certificate.
6. **Add yourself:**

   ```sh
   sudo bash install-server.sh --add-admin yourname
   ```

   This prints a one-time setup link. It works for 24 hours.
7. **Set up your sign-in.** Open the link, choose a password, and add a passkey (recommended) or an authenticator app. Save the recovery codes it shows you.

**Caddy refuses requests that don't come from Cloudflare's addresses**, so the protection can't be bypassed by reaching the server directly. The client's address and country are Cloudflare's (`CF-Connecting-IP`, `CF-IPCountry`).

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
- removing servers.

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

### Privacy

**What the admin UI shows:**
- counts, cities, and the names of admins;
- never players' names, addresses or accounts.

**What's stored:**
- Players' locations are city counts.
- Launchers' ping reports keep only the city and the round trip.

IP geolocation by [DB-IP](https://db-ip.com), CC BY 4.0. The world map is [Natural Earth](https://www.naturalearthdata.com) (public domain), via [world-atlas](https://github.com/topojson/world-atlas) (ISC).
