# Deploying servers and a coordinator

How to put 5th Echelon servers on the internet with the Linux installer: one server on its own, a group of servers sharing friends through a coordinator, or a coordinator on a machine of its own.

The examples use `play.example.com` for a game server and `coord.example.com` for a coordinator. The [community network](../README.md#the-community-server) runs a coordinator at `play.scbl.jdevwebb.net`, with regional servers such as `eu1.scbl.jdevwebb.net` and `oceania.scbl.jdevwebb.net`.

**A game server's name is at most 27 characters** (e.g. `eu1.example.com`). The game keeps it where its own `onlineconfigservice.ubi.com` was, so it can't start with a longer one; the installer refuses it.

## Choose a shape

| Shape | Machines | For |
|---|---|---|
| **One server** | 1 | A group that plays on one server |
| **Server and coordinator together** | 1 | The first server of a group; others can join later |
| **Join a group** | 1 per server | A new server in an existing group, e.g. the community network |
| **Coordinator on its own** | 1, plus the servers | A group of servers with no "main" one |

A coordinator shares friends and blocks between its servers, reserves each player's name across them, and lists them in the server directory the launcher browses. See [friends.md](friends.md) for what it knows and trusts.

**Give a group one address.** Players can type a coordinator's address in the launcher as well as a server's: the launcher then pings every server in the directory and sets them up on the best. So a group can hand out its coordinator's name (say `play.example.com`) and give each server a regional one (`eu.example.com`, `oceania.example.com`). The community network does exactly this.

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

```sh
curl -fsSLO https://raw.githubusercontent.com/JDevWebb/5th-echelon-enhanced/main/scripts/install-server.sh
```

It downloads the latest release and checks it against the release's `SHA256SUMS` and the release key's signature. To install a build of your own instead, copy it next to the script and add `--binary ./dedicated_server-linux-x86_64` (and `--coordinator-binary ./coordinator-linux-x86_64` for a coordinator).

## One server, or a server with a coordinator

Run it and answer the questions:

```sh
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

You need the coordinator's address and its **join token**, from whoever runs it. To join the community network, [open an issue](https://github.com/JDevWebb/5th-echelon-enhanced/issues/new?template=add-server.yml).

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

## A coordinator on its own

For a group with no main server:

```sh
sudo bash install-server.sh --yes --coordinator-only --coordinator-domain coord.example.com
```

It installs the coordinator and Caddy, and no game server. Servers then join it as above.

## Renaming servers or the coordinator

Run the installer again with the new names (`--domain`, `--coordinator-domain`), after their A records point at the machine. Accounts, friends and settings stay. A server whose coordinator has a new address joins it again by itself, with the secret it already has. Players set up on an old name press **Connect** again with the new one, or with the network's address.

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
4. In the launcher: **Server › Manage a server**, address `localhost`, and the key.

## Updating

**Servers in a network update themselves.** Their coordinator rolls out each signed release, one server first, then the rest. The installer's updater installs it, checking the release key's signature, and puts the previous release back if the new one doesn't come back healthy. See [operations.md](operations.md). `--no-auto-update` turns this off for a server, but a coordinator then leaves it out of its directory.

**To update by hand**, run the installer again. It keeps the settings, accounts and keys, and updates the server, the coordinator and the Caddy site. A release that isn't signed by the release key is refused.

`--uninstall` removes the services, programs, Caddy site, updater and the firewall rules the installer added, and keeps the data; add `--purge` to delete that too.

## The admin UI

A coordinator has an admin web UI at a name of its own, served only through Cloudflare. It shows:
- the network's players and where they are;
- what's being played;
- pings, load and bandwidth;
- the update rollout.

Turn it on with `--metrics-domain NAME` and an Origin CA certificate, and add yourself with `--add-admin NAME`. [operations.md](operations.md) has the steps, the Cloudflare settings, and sign-in with passkeys or an authenticator app.

## Hardening the machine

The installer hardens what it installs:
- each service runs in a systemd sandbox, Caddy included;
- Caddy's admin API listens on a root-only socket;
- the firewall opens only the game's ports.

`scripts/harden-host.sh` does the rest of a Debian or Ubuntu host:
- installs updates, and lets unattended upgrades reboot at a quiet hour;
- SSH: keys only, named users, no forwarding but local tunnels, modern algorithms; it then checks with `sshd -T` that sshd really uses them, and warns about any that an earlier file overrides;
- fail2ban for SSH, never banning the address you ran it from (`--ignore-ip` adds others);
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
| "this release isn't signed" | An older release; `--allow-unsigned` installs it on its checksum alone |
| `--status` shows the server delisted, or the updater failed | See **Updates** in the admin UI, or `cat /var/lib/5th-echelon-update/update-status.json`. The previous release is kept in `/opt/5th-echelon/previous/` |
| SSH stopped answering after `harden-host.sh` | Wait 10 minutes (the change undoes itself unless confirmed), or use your provider's console. After moving SSH, connect with `-p` and the new port |

Logs: `journalctl -u 5th-echelon -f`, `journalctl -u 5th-echelon-coordinator -f`, `journalctl -u caddy -f`.
