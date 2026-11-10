# Deploying servers and a coordinator

How to put 5th Echelon servers on the internet with the Linux installer: one server on its own, a group of servers sharing friends through a coordinator, or a coordinator on a machine of its own.

The examples use `play.example.com` for a game server and `coord.example.com` for a coordinator. The [community network](../README.md#the-community-server) is one of these: a coordinator, one matchmaking server and relays near its players.

**A game server's name is at most 27 characters** (e.g. `eu1.example.com`). The game keeps it where its own `onlineconfigservice.ubi.com` was, so it can't start with a longer one; the installer refuses it.

## Choose a shape

| Shape | Machines | For |
|---|---|---|
| **One server** | 1 | A group that plays on one server |
| **Server and coordinator together** | 1 | The first server of a group; others can join later |
| **Join a group** | 1 per server | A new server in a group you run, or one whose coordinator you trust |
| **Coordinator on its own** | 1, plus the servers | A group of servers with no "main" one |
| **One matchmaking server with relays** | 1 coordinator, 1 matchmaking server, 1 relay per region | A network whose players should all find each other, wherever they are ([below](#one-matchmaking-server-with-relays)) |

A coordinator shares friends and blocks between its servers, reserves each player's name across them, and lists them in the server directory the launcher browses. See [friends.md](friends.md) for what it knows and trusts.

**Give a group one address.** Players can type a coordinator's address in the launcher as well as a server's: the launcher then pings every server in the directory and sets them up on the best. So a group can hand out its coordinator's name (say `play.example.com`) and give each server a regional one (`eu.example.com`, `oceania.example.com`). A network with one matchmaking server hands out that server's name instead.

## What a server needs

**The machine:**
- Linux on x86_64 with systemd: Debian 12+, Ubuntu 22.04+, Fedora, Rocky/Alma 9+, or Arch.
- **2 vCPUs and 2–4 GB of memory** are plenty for a busy server. By the [load test](load-testing.md), 1,000 players need under 100 MB; one core handles 500, two handle 1,000.
- **Bandwidth is the real cost.** Matches run peer to peer, but players whose routers can't be reached are relayed through the server: about 7 MB/s each way at 1,000 players with 20% relayed, 3 MB/s at 500. Real servers are busy a few hours a day. A plan with 20 TB a month covers 1,000 players in matches around the clock if only outgoing traffic is counted, or half that if both directions are.

**DNS:** an A record for each name, pointing at the machine's IPv4 address.
- On **Cloudflare**, set the game server and coordinator records to **DNS only** (grey cloud), not proxied:
  - the game's UDP traffic and its plain HTTP on port 80 can't go through Cloudflare's proxy;
  - Cloudflare's free certificate doesn't cover names two levels deep like `play.scbl.example.com`.

  Caddy on the server gets its own certificates.
- The one exception is a coordinator's [admin UI](operations.md#the-admin-ui): its name is proxied, one level deep, with a Cloudflare Origin CA certificate.
- Don't add an AAAA (IPv6) record unless it points at the same machine.

**Your provider's firewall** (security group), for every server:

| Port | For |
|---|---|
| TCP 22 | SSH (or the port you [move it to](#hardening-the-machine)) |
| TCP 80 | The game's config and content, and the launcher's API without encryption |
| TCP 443 | The launcher's API over HTTPS, and a coordinator |
| UDP 21126 | Game login |
| UDP 21127 | Game service |
| UDP 21128–21129 | Internet play: public addresses and the relay |

A machine that only runs a coordinator needs SSH, and TCP 80 and 443.

## Get the installer

The installer runs as root, so take it from a release and check it first. Each release has `install-server.sh` among its downloads, listed in its `SHA256SUMS`, and `SHA256SUMS.sig` is the release key's signature of that release's version and `SHA256SUMS`. The block below, pasted into a shell on the server:

1. finds the latest release and downloads `install-server.sh`, `SHA256SUMS` and `SHA256SUMS.sig` from it, into `~/5th-echelon-installer`;
2. checks the signature with OpenSSL against the release key. What's signed isn't a file as published: it's the line `5th-echelon/release/v2`, the version (`0.5.0`, the tag without its `v`) on a line of its own, then `SHA256SUMS`. The signature is base32 without padding, so it's padded and decoded first (the installer and the servers' updater check releases the same way);
3. checks the installer against `SHA256SUMS`;
4. runs it, only if everything above passed.

Any failure stops it with an error, and nothing runs. It needs curl, OpenSSL 3, base32 and sha256sum (Debian 12+, Ubuntu 22.04+ and the others above have them).

```sh
(
  set -eu
  mkdir -p ~/5th-echelon-installer
  cd ~/5th-echelon-installer
  repo=https://github.com/JDevWebb/5th-echelon-enhanced
  tag=$(curl -fsSI --proto '=https' -o /dev/null -w '%{redirect_url}' "$repo/releases/latest")
  version=${tag##*/tag/v}
  printf '%s\n' "$version" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?'
  rm -f install-server.sh SHA256SUMS SHA256SUMS.sig
  for f in install-server.sh SHA256SUMS SHA256SUMS.sig; do
    curl -fsSLO --proto '=https' "$repo/releases/download/v$version/$f"
  done
  printf '%s\n' '-----BEGIN PUBLIC KEY-----' \
    'MCowBQYDK2VwAyEANX9q9hOdzlNhVlSPEtmjJbbdJyOktGgQkPw4ep2KCLI=' \
    '-----END PUBLIC KEY-----' > release.pem
  { printf '5th-echelon/release/v2\n%s\n' "$version"; cat SHA256SUMS; } > signed
  sig=$(tr -d '[:space:]' < SHA256SUMS.sig)
  while [ $(( ${#sig} % 8 )) -ne 0 ]; do sig="$sig="; done
  printf '%s' "$sig" | base32 -d > signature
  openssl pkeyutl -verify -pubin -inkey release.pem -rawin -in signed -sigfile signature
  grep -Eqx '[0-9a-f]{64}  install-server\.sh' SHA256SUMS
  sha256sum -c --ignore-missing --strict SHA256SUMS
  echo "install-server.sh is release $version's, signed by the release key."
  sudo bash install-server.sh
)
```

(The whole block is one subshell with `set -e` so that any failing line stops it; don't add `&&` or `||` after its closing parenthesis, which would turn `set -e` off inside it.)

It prints `Signature Verified Successfully` and `install-server.sh: OK` before the installer starts. Releases before 0.5.0 don't list the installer, so the block stops at the `grep` for them.

**The release key** (Ed25519), the same one the installer (`RELEASE_KEY_PEM`), the launcher and the servers check releases with (`RELEASE_KEYS` in `identity/src/lib.rs` holds its raw 32 bytes, in base32):

```
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEANX9q9hOdzlNhVlSPEtmjJbbdJyOktGgQkPw4ep2KCLI=
-----END PUBLIC KEY-----
```

Its SHA-256 fingerprint (of the key in DER form) is `0d9aa66595918e8fa5b6d79fc6470b53defde525a719546c5e0440aa992b234a`; `openssl pkey -pubin -in release.pem -outform DER | sha256sum` prints it. Compare it with the key on another copy of this page (the repository's, a clone of your own) if you'd rather not trust one download for both the key and the check.

Run as root, the installer then downloads the release's server (and coordinator) and checks them against the same signed `SHA256SUMS`. It also says whether it's that release's own `install-server.sh`, and warns when it isn't. To install a build of your own instead, copy it next to the script and add `--binary ./dedicated_server-linux-x86_64` (and `--coordinator-binary ./coordinator-linux-x86_64` for a coordinator).

## One server, or a server with a coordinator

The block above ends by running the installer; answer its questions. To run it again later, or with options, it's in `~/5th-echelon-installer` (get it again with the block when there's a new release):

```sh
cd ~/5th-echelon-installer
sudo bash install-server.sh
```

1. **Domain name:** `play.example.com`. Caddy then serves the game's web parts and the launcher's API on it, over HTTP for the game and HTTPS for the launcher.
2. **Sharing friends:**
   - `1` keeps the server on its own;
   - `2` runs a coordinator here too, at a second name (`coord.example.com`);
   - `3` joins a group's coordinator (see below).
3. **Name and region** for the server directory, e.g. `Kiwi Ops` and `Sydney`.

The same without questions:

```sh
sudo bash install-server.sh --yes \
  --domain play.example.com \
  --coordinator-domain coord.example.com \
  --server-name "Kiwi Ops" --region Sydney
```

The installer:
- installs the server and the coordinator as sandboxed systemd services, each running as its own user (`echelon`, `echelon-coord`) with its own folder (`/var/lib/5th-echelon`, `/var/lib/5th-echelon-coordinator`). Earlier installs kept the coordinator in `/var/lib/5th-echelon/coordinator`, run as `echelon`; the next run moves it;
- installs Caddy, which gets a certificate for each name;
- serves the launcher's API over HTTPS once the certificate works, tells launchers to use it, and then takes passwords and sign-ins only over HTTPS (`[limits] require_tls_for_credentials`; turned off again on a run where Caddy has no certificate, so players can still sign in). The game's own plain HTTP on port 80 is unchanged;
- has Caddy refuse request bodies over 4 MB on the game's sites, and requests whose headers or body don't arrive within 10 or 30 seconds (on ports 80 and 443, for every site there);
- joins the server to its coordinator;
- opens the ports in ufw or firewalld, if either is active, and lists the ones to open in your provider's firewall.

**Check it:**

```sh
sudo bash install-server.sh --status
curl http://play.example.com/api/info       # "ports" includes "api_tls":443
curl https://coord.example.com/v1/servers   # the directory: your server within a minute or two
```

## Join a group

You need the coordinator's address and its **join token**, from whoever runs it.

Put the token in a file only you can read, then:

```sh
sudo bash install-server.sh --yes \
  --domain other.example.com \
  --coordinator https://coord.example.com \
  --join-token-file token.txt \
  --server-name "Other" --region Perth
shred -u token.txt
```

Or run it without `--yes` and choose `3`; it asks for the token without showing it. The token is only needed once; the server then keeps its own credentials in `federation.key`. `--status` shows whether it has joined.

## One matchmaking server with relays

From 0.5.0. Every player signs in to one **matchmaking server**, so every search finds the whole network; **relays** near each region's players (a NAT helper and relay alone: no game services, never a place to play) carry matches when a router can't be reached. Matches still run between the players' games, and matchmaking doesn't need a low ping. The launcher pings the relays the matchmaking server offers and has the game use the nearest.

All with the installer, on servers that share a coordinator:

| Machine | Options |
|---|---|
| **The coordinator** | `--matchmaking-server <the matchmaking server's id>` (its `server-id.txt`, once it's installed) |
| **The matchmaking server** | `--matchmaking`; for its standby, `--mm-record <its own DNS-only name>` and `--mm-standby <the group: itself, then the relays>` |
| **Each relay** | `--role relay --ticket-key <the matchmaking server's relay-pass key>` (logged at its start), and the same `--mm-record` and `--mm-standby` list |

The matchmaking server signs a relay pass at each sign-in, which relays take with its key; relays forward to each other only between players the matchmaking server put together. Each relay can stand by for the matchmaking server ([failover.md](failover.md)). Moving an existing network of regional servers onto one, accounts, friends and links included, in one update window: [cutover.md](cutover.md).

## A coordinator on its own

For a group with no main server:

```sh
sudo bash install-server.sh --yes --coordinator-only --coordinator-domain coord.example.com
```

It installs the coordinator and Caddy, and no game server. Servers then join it as above.

## Renaming servers or the coordinator

Run the installer again with the new names (`--domain`, `--coordinator-domain`), after their A records point at the machine. Accounts, friends and settings stay. A server whose coordinator has a new address joins it again by itself, with the secret it already has. Players set up on an old name press **Connect** again with the new one, or with the network's address.

A coordinator can keep answering to its old name while launchers and servers move: add `--coordinator-alias <old name>` (from 0.4.6; `--coordinator-alias -` drops it later). Servers moving to the new name keep their membership as long as the coordinator lists it as its own, which it does with both names set. The steps, failover included, are in [Moving a coordinator to a new name](failover.md#moving-a-coordinator-to-a-new-name).

## The join token

On the coordinator's machine:

```sh
sudo bash install-server.sh --show-join-token     # print it, to pass on privately
sudo bash install-server.sh --rotate-join-token   # a new one; servers that joined keep working
```

Anyone with the token can add a server to the group, and a member server can make friendships and blocks between players linked on it. Give it only to people you trust, over a private channel, never in a public issue or chat. To remove a server:

```sh
sudo -u echelon-coord /opt/5th-echelon/coordinator --data /var/lib/5th-echelon-coordinator remove-server <id>
```

Its id is in the directory (`/v1/servers`) and in that server's `server-id.txt`. Rotate the token afterwards if it could join again.

## Settings

The installer's options for the usual settings. Each is kept on later runs unless given again.

| Option | Setting |
|---|---|
| `--friends mutual` or `everyone` | Only friends on the game's friend list, and only friends invite (`mutual`, the default); or every player |
| `--closed-registration`, `--open-registration` | Stop or allow new accounts |
| `--admin`, `--no-admin` | The admin API, for managing players and games (see below). Needs Caddy: without a domain, the admin API would travel unencrypted on port 50051, so the installer refuses `--admin` and turns it off |
| `--unlisted`, `--listed` | Stay out of the server directory, or appear in it |
| `--alias NAME` | Another name (or IP) players reach the server by; repeat for more |
| `--relay auto`, `all` or `off` | Who plays through the server's relay |
| `--no-https-api` | Don't serve the launcher's API over HTTPS |
| `--no-auto-update`, `--auto-update` | Don't (or do) install the releases the coordinator rolls out. On by default; a network delists servers that don't |
| `--metrics-domain NAME` (`--metrics-cert`, `--metrics-key`) | A coordinator's admin UI, at NAME, with a Cloudflare Origin CA certificate; see [operations.md](operations.md) |

New installs also require every account to be linked to a player identity (`[limits] require_identity`): the launcher finds a player's account by their identity, so nobody types a password, and accounts can't be made without one.

Everything else is in `/var/lib/5th-echelon/service.toml` (see [server-settings.md](server-settings.md)); restart with `systemctl restart 5th-echelon` after changing it.

## Managing players and games

The admin API is never reachable from the internet; Caddy refuses it. Use an SSH tunnel:

1. `sudo bash install-server.sh --admin`
2. `sudo cat /var/lib/5th-echelon/admin-key.txt`
3. From your PC: `ssh -L 50051:127.0.0.1:50051 you@play.example.com` (add `-p 28622` if you [moved SSH](#hardening-the-machine))
4. In the launcher: **Host › Manage a server**, address `localhost`, and the key.

## Updating

**Servers in a network update themselves.** Their coordinator rolls out each signed release, one server first, then the rest. The installer's updater installs it, checking the release key's signature, and puts the previous release back if the new one doesn't come back healthy, with the databases as they were before the update (it copies them first, into `/var/lib/5th-echelon-update/databases`). See [operations.md](operations.md). `--no-auto-update` turns this off for a server, but a coordinator then leaves it out of its directory.

**To update by hand**, run the installer again. It keeps the settings, accounts and keys, and updates the server, the coordinator and the Caddy site, and Caddy itself when it's the installer's own static build (a packaged Caddy updates with the system). A release that isn't signed by the release key is refused.

`--uninstall` removes the services, programs, Caddy site, updater and the firewall rules the installer added, and keeps the data; add `--purge` to delete that too.

## Backups

The databases (accounts, friends and stats; and the coordinator's links, names and admins) can be backed up off the machine: a snapshot to Cloudflare R2 every 2 hours (a week of them), and a daily archive to Backblaze B2 (30 days, and 12 monthly copies). Write `/etc/5th-echelon/backup.env` with the buckets' keys and run the installer again; `/opt/5th-echelon/backup.sh` restores. See **[backups.md](backups.md)**.

## Failover

If the coordinator's machine goes down, every server of its network loses friends across servers, the directory and stats until it's back. In a failover group, each game server keeps a standby coordinator, and the next in line takes over after 3 minutes, from the coordinator's latest snapshot, by moving its record (proxied through Cloudflare). Give each server `--standby NAME=HOST,...` with an Origin certificate for the coordinator's name. See **[failover.md](failover.md)**.

## The admin UI

A coordinator has an admin web UI at a name of its own, served only through Cloudflare. It shows:
- the network's players and where they are;
- what's being played;
- pings, load and bandwidth, live;
- bandwidth over time, with monthly allowances, the 95th percentile and a CSV export;
- players over time, play time and when people play, and the matches played;
- the players on every server, by name, with their play time, and actions on them (kick, ban, a new password, rename, delete);
- alerts: a server offline, CPU, memory, disk, refused sign-ins, the traffic allowance, failed updates; optionally posted to Discord or Slack;
- the update rollout.

Turn it on with `--metrics-domain NAME` and an Origin CA certificate, and add yourself with `--add-admin NAME`. [operations.md](operations.md) has the steps, the Cloudflare settings, and sign-in with passkeys or an authenticator app.

## Hardening the machine

The installer hardens what it installs:
- each service runs in a systemd sandbox, Caddy included;
- Caddy's admin API listens on a root-only socket;
- if ufw or firewalld is already on, it opens only the game's ports there.

Neither script turns a firewall on. Use your provider's firewall, or turn on ufw before installing (SSH first, so you keep your login):

```sh
sudo ufw default deny incoming && sudo ufw allow 22/tcp && sudo ufw --force enable
```

`scripts/harden-host.sh` does the rest of a Debian or Ubuntu host:
- installs updates, and lets unattended upgrades reboot at a quiet hour; they include Caddy from its own repository, not only the distribution's;
- SSH: keys only, named users, no forwarding but local tunnels, modern algorithms; it then checks with `sshd -T` that sshd really uses them, and warns about any that an earlier file overrides;
- fail2ban for SSH, never banning the address you ran it from (`--ignore-ip` adds others; give yours with it when running the script without a terminal, e.g. through `ssh host 'sudo bash …'`, where it can't tell);
- kernel hardening, no core dumps, and no services a server doesn't need.

```sh
sudo bash harden-host.sh --ssh-users "you" --reboot-time 04:30
```

**Confirming the SSH change:**
1. Log in from a new terminal to check it works.
2. Run `sudo bash harden-host.sh --confirm-ssh` within 10 minutes.

Without that, the SSH change undoes itself, so a mistake can't lock you out.

**Moving SSH to another port** (`--ssh-port 28622`, say) stops most automated scans. Key-only login and fail2ban are what keep attackers out, but it keeps the logs quiet. Port 22 stays open alongside the new one until you log in on the new port and run `--confirm-ssh` there; it refuses to close 22 while no login on the new port is open, so you can't get cut off in between. Open the new port in your provider's firewall too, if it has one.

## When something's wrong

| What you see | What to do |
|---|---|
| "doesn't resolve yet", or "points at … not at this server" | Fix the A record. On Cloudflare, switch it to DNS only |
| "Caddy has no certificate for …" | The A record must point here, and TCP 80 and 443 must be open in your provider's firewall. Run the installer again once they are |
| `--status` says "not joined yet" | The coordinator must answer at its `https://` address, and the token must be current. The line under it shows the last error |
| "ports already in use" | Another program holds a port; stop it, or add `--force` |
| "this release isn't signed" | Releases are signed: a missing signature can mean someone changed the release on GitHub. Pick a signed release; `--allow-unsigned` (its checksum alone, which whoever changed it could change too) only for one you know about |
| `--status` shows the server delisted, or the updater failed | See **Releases** in the admin UI, or `cat /var/lib/5th-echelon-update/update-status.json`. The previous release is kept in `/opt/5th-echelon/previous/` |
| SSH stopped answering after `harden-host.sh` | Wait 10 minutes (the change undoes itself unless confirmed), or use your provider's console. After moving SSH, connect with `-p` and the new port |

Logs: `journalctl -u 5th-echelon -f`, `journalctl -u 5th-echelon-coordinator -f`, `journalctl -u caddy -f`.
