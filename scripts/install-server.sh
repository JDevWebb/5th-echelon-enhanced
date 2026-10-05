#!/usr/bin/env bash
# Installs (or updates) the 5th Echelon server on a Linux machine, e.g. a
# fresh VPS, as a systemd service, optionally behind Caddy on a domain name.
#
#   curl -fsSLO https://raw.githubusercontent.com/JDevWebb/5th-echelon-enhanced/main/scripts/install-server.sh
#   sudo bash install-server.sh
#
# Run interactively, it asks for what it needs: the domain name, and whether
# this server shares friends with others (runs a coordinator, or joins one).
# The full guide is docs/deploying.md.
#
# Works on Debian, Ubuntu, Fedora, RHEL-likes (Rocky, Alma, CentOS Stream),
# Arch and Manjaro, on x86_64 with systemd. Run it again to
# update: settings, accounts and keys are kept.
#
# With a domain name (asked for, or --domain), Caddy serves the game's
# config, content and the launcher's API on port 80 under that name, so
# port 80 can be shared with other sites and players type the name. It
# also serves the launcher's API over HTTPS on port 443, so passwords and
# sign-ins never travel readable.
#
# Downloads are checked against the release's SHA256SUMS, and SHA256SUMS
# against the release key's signature (needs OpenSSL 3).
#
# Options:
#   --domain NAME         serve through Caddy as NAME (an A record for this
#                         server); asked for when run interactively
#   --no-caddy            no domain name, no Caddy: the server takes the
#                         ports itself
#   --public-address IP   the address players' game traffic reaches
#                         (default: detected)
#   --version X.Y.Z       a release (default: the latest)
#   --binary FILE         install this dedicated_server-linux-x86_64 instead
#                         of downloading one
#   --relay auto|all|off  who plays through the server's relay (default: auto)
#   --friends mutual|everyone
#                         who is on a player's friend list: only friends,
#                         who alone can invite (mutual, the default for a new
#                         install: a public server), or every player
#                         (everyone: a group that all know each other)
#   --alias NAME          another name (or IP) players reach this server by;
#                         repeat for more. Caddy serves it too, and identity
#                         signatures made for it are accepted
#   --admin, --no-admin   turn the admin API on or off (manage the server
#                         through an SSH tunnel; see docs/deploying.md)
#   --closed-registration, --open-registration
#                         stop or allow new accounts (existing ones still
#                         sign in)
#   --server-name NAME    the server's name in a server directory (default:
#                         the domain)
#   --region NAME         where the server is, for the directory, e.g. Sydney
#   --coordinator URL     share friends with other servers through this
#                         coordinator, and appear in its server directory
#   --join-token TOKEN    the coordinator's join token, from its operator
#                         (visible to other users in ps; prefer the file)
#   --join-token-file FILE
#                         read the join token from FILE
#   --unlisted, --listed  stay out of (or appear in) the server directory
#   --coordinator-domain NAME
#                         also run a coordinator here, at https://NAME (an A
#                         record for this server, TCP 443 open); this server
#                         joins it, and other servers can too
#   --coordinator-only    with --coordinator-domain: only the coordinator
#                         (and Caddy), no game server on this machine
#   --coordinator-binary FILE
#                         install this coordinator-linux-x86_64 instead of
#                         downloading one
#   --metrics-domain NAME the coordinator's admin web UI (metrics, servers,
#                         updates) at https://NAME, only through Cloudflare
#                         (the record proxied, orange cloud); needs
#                         --coordinator-domain. Then --add-admin
#   --cloudflare-origin-pull
#                         with --metrics-domain: also require Cloudflare's
#                         client certificate (Authenticated Origin Pulls,
#                         turned on in Cloudflare too)
#   --metrics-cert FILE, --metrics-key FILE
#                         with --metrics-domain: a Cloudflare Origin CA
#                         certificate and its key (SSL/TLS > Origin Server)
#                         instead of Let's Encrypt, which can't validate a
#                         proxied name reliably. Kept for later runs
#   --coordinator-cert FILE, --coordinator-key FILE
#                         with --coordinator-domain: a Cloudflare Origin CA
#                         certificate and its key for it. The coordinator is
#                         then reached only through Cloudflare (its record
#                         proxied, orange cloud). Kept for later runs
#   --standby NAME=HOST,NAME=HOST,...
#                         a failover group (docs/failover.md): each server
#                         listed runs a standby coordinator, and they take
#                         over in this order if the coordinator goes down.
#                         NAME is a server's backup name, HOST its --domain.
#                         Needs --coordinator-domain (the same on each) with
#                         --coordinator-cert, backups, and
#                         /etc/5th-echelon/standby.env. Kept for later runs
#   --add-admin NAME      print a one-time setup link for a new admin of the
#                         admin UI
#   --reset-admin NAME    for an admin who lost their second factor: a new
#                         setup link (their sign-in is cleared)
#   --admin-open-access   clear the admin UI's sign-in restrictions (when
#                         every admin is locked out)
#   --no-auto-update, --auto-update
#                         don't (or do) install the releases the coordinator
#                         rolls out (on by default; a coordinator delists
#                         servers that don't)
#   --no-https-api        don't serve the launcher's API over HTTPS
#   --allow-unsigned      install a release without a valid signature
#                         (older releases); the checksum is still checked
#   --no-firewall         don't open ports in ufw or firewalld
#   --yes                 don't ask; go on past warnings (e.g. DNS not
#                         pointing here yet)
#   --force               install even though a port is already in use
#   --status              show what's installed and whether it's working
#   --show-join-token     print this machine's coordinator join token
#   --rotate-join-token   make a new join token (servers that joined keep
#                         working)
#   Backups (live to Cloudflare R2, a daily archive to Backblaze B2) turn on when
#   /etc/5th-echelon/backup.env exists; see docs/backups.md.
#   --uninstall           remove the service and program (keeps the data)
#   --purge               with --uninstall: also delete the data
#   --no-systemd          only install files (containers, testing)
#   -h, --help
set -euo pipefail
# Files for other users (Caddy reads its config, systemd its units) must be
# readable whatever umask this was started with; secrets set their own modes.
umask 022

REPO="JDevWebb/5th-echelon-enhanced"
ASSET="dedicated_server-linux-x86_64"
SERVICE="5th-echelon"
USER_NAME="echelon"
PROGRAM_DIR="/opt/5th-echelon"
STATE_DIR="/var/lib/5th-echelon"
UNIT="/etc/systemd/system/${SERVICE}.service"
CONFIG="$STATE_DIR/service.toml"
CADDYFILE="/etc/caddy/Caddyfile"
CADDY_SITE="/etc/caddy/5th-echelon.caddy"
BEGIN_MARK="# >>> 5th Echelon (managed by install-server.sh; keep other sites outside this block)"
END_MARK="# <<< 5th Echelon"
UDP_PORTS="21126-21129"
SYSCTL_FILE="/etc/sysctl.d/90-5th-echelon.conf"
# The root updater for the releases a coordinator rolls out (see install_updater).
UPDATER="$PROGRAM_DIR/update.sh"
UPDATE_PATH_UNIT="/etc/systemd/system/5th-echelon-update.path"
UPDATE_SERVICE_UNIT="/etc/systemd/system/5th-echelon-update.service"
CADDY_DROPIN="/etc/systemd/system/caddy.service.d/10-5th-echelon-hardening.conf"
COORD_SERVICE="5th-echelon-coordinator"
COORD_UNIT="/etc/systemd/system/${COORD_SERVICE}.service"
COORD_ASSET="coordinator-linux-x86_64"
# The coordinator runs as a user of its own, with its data outside the game
# server's folder: the game server (as its user) can't read its join token,
# admins' sign-ins or database, nor plant links for root to follow there.
COORD_USER="echelon-coord"
COORD_DIR="/var/lib/5th-echelon-coordinator"
# Where earlier installs kept it (run as the game server's user); moved on
# the next run.
OLD_COORD_DIR="$STATE_DIR/coordinator"
# The coordinator listens on a loopback address of its own, which the game
# server's sandbox can't reach (IPAddressDeny), so the game server can't
# talk to it as the local proxy would (X-Forwarded-For, the admin UI's
# client address).
COORD_ADDR="127.0.0.2"
# The installer's own records, owned by root and outside the folders the
# services can write: which coordinator domain runs here, which firewall
# rules it added.
ETC_DIR="/etc/5th-echelon"
FIREWALL_RECORD="$ETC_DIR/firewall-rules"
# The updater's status (root's; the services read it, and can't change it).
UPDATE_DIR="/var/lib/5th-echelon-update"
# The release key (its public half), which signs each release's SHA256SUMS.
RELEASE_KEY_PEM="-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEANX9q9hOdzlNhVlSPEtmjJbbdJyOktGgQkPw4ep2KCLI=
-----END PUBLIC KEY-----"
# A failover group (see install_standby and docs/failover.md): its servers, as given; the
# standby's settings; and Cloudflare's API token and zone, root's only (the operator writes it).
STANDBY_LIST="$ETC_DIR/standby"
STANDBY_CONF="$ETC_DIR/standby.conf"
STANDBY_ENV="$ETC_DIR/standby.env"
STANDBY_SERVICE="5th-echelon-standby"
STANDBY_UNIT="/etc/systemd/system/$STANDBY_SERVICE.service"
# The coordinator's Cloudflare Origin CA certificate, when its record is proxied.
COORD_CERT="/etc/caddy/5th-echelon-coordinator.crt"
COORD_KEY="/etc/caddy/5th-echelon-coordinator.key"
# Backups (see install_backups and docs/backups.md): on when this file exists. Root's only.
BACKUP_ENV="$ETC_DIR/backup.env"
BACKUP_SCRIPT="$PROGRAM_DIR/backup.sh"
BACKUP_STATE="/var/lib/5th-echelon-backup"
# Litestream (the live copy to R2) and rclone (the archive to B2): pinned, with their SHA-256.
LITESTREAM_VERSION="0.5.17"
LITESTREAM_SHA256_amd64="cfb371176d164437ae869f8351cfde49bd1804ae71c61923f75c9cba9c9c006d"
LITESTREAM_SHA256_arm64="f8ca4a050095c1efbda2c4365172e61bf9d955ea0d9ac42f448b52e51819baa5"
RCLONE_VERSION="1.75.1"
RCLONE_SHA256_amd64="982b5aa772841168f8e380f139e9e787b2a105403e32b94da8676a0e1c0a13ab"
RCLONE_SHA256_arm64="03f2504174034b6d004152ed7369251c9a9ec1f7e0836eda420f5c7a5ec0dff9"
# Caddy's static build, when there's no package: pinned, with its SHA-512.
CADDY_VERSION="2.11.7"
CADDY_SHA512_amd64="a7a433a1b133efc3c8d10eb0b99d52a24b5ef5c322dc77f5282182b1c0402139ab83f3a99f0c52409df77d20123fb0b523edad8a66d8f5e49136197bf61ef0e7"
CADDY_SHA512_arm64="3db36ba90c7a6e8dda40ee3dd71fa08844c76b5fb08f61b31e5e78d2ed38e71c51dc7baed875e50d1ca1279196e84302967237386ae87c91ae9f2aaceada682e"

domain="" no_caddy=0 public_address="" version="latest" binary="" relay=""
# What to tell the operator at the end (added to along the way).
notes=()
firewall=1 yes=0 force=0 uninstall=0 purge=0 use_systemd=1
friends="" server_name="" region="" coordinator="" join_token="" coord_domain="" coord_binary=""
https_api=1 allow_unsigned=0 coord_only=0 admin="" registration="" listed="" command=""
metrics_cert="" metrics_key="" metrics_domain="" origin_pull=0 admin_name="" auto_update="" release_version=""
coord_cert="" coord_key="" standby="" standby_me=""
aliases=()
# Only HTTPS, and TLS 1.2 or newer, for every download, and none larger
# than a release's binaries could be (SMALL: for listings, checksums and
# signatures).
CURL=(curl --proto '=https' --tlsv1.2 --connect-timeout 20 --max-filesize 268435456)
SMALL=(--max-filesize 1048576 --max-time 60)

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0; }
# A question on the terminal, even when the script is piped into bash.
ask() {
  local answer=""
  if [ "$yes" -eq 0 ] && [ -r /dev/tty ] && { : </dev/tty; } 2>/dev/null; then
    read -r -p "$1" answer </dev/tty || true
  fi
  printf '%s' "$answer"
}
# Whether there's someone to ask.
interactive() { [ "$yes" -eq 0 ] && [ -r /dev/tty ] && { : </dev/tty; } 2>/dev/null; }
# A question whose answer isn't shown (a join token).
ask_secret() {
  local answer=""
  if interactive; then
    read -r -s -p "$1" answer </dev/tty || true
    echo >/dev/tty
  fi
  printf '%s' "$answer"
}
confirm() {
  [ "$yes" -eq 1 ] && return 0
  case "$(ask "$1 [y/N] ")" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

while [ $# -gt 0 ]; do
  case "$1" in
    --domain) domain="${2:?}"; shift ;;
    --no-caddy) no_caddy=1 ;;
    --public-address) public_address="${2:?}"; shift ;;
    --version) version="${2:?}"; shift ;;
    --binary) binary="${2:?}"; shift ;;
    --relay) relay="${2:?}"; shift ;;
    --friends) friends="${2:?}"; shift ;;
    --server-name) server_name="${2:?}"; shift ;;
    --region) region="${2:?}"; shift ;;
    --coordinator) coordinator="${2:?}"; shift ;;
    --join-token) join_token="${2:?}"; shift ;;
    --join-token-file) join_token="$(tr -d '[:space:]' < "${2:?}")" || die "can't read $2"; shift ;;
    --no-https-api) https_api=0 ;;
    --metrics-domain) metrics_domain="${2:?}"; shift ;;
    --cloudflare-origin-pull) origin_pull=1 ;;
    --metrics-cert) metrics_cert="${2:?}"; shift ;;
    --metrics-key) metrics_key="${2:?}"; shift ;;
    --coordinator-cert) coord_cert="${2:?}"; shift ;;
    --coordinator-key) coord_key="${2:?}"; shift ;;
    --standby) standby="${2:?}"; shift ;;
    --add-admin) command=add-admin; admin_name="${2:?}"; shift ;;
    --reset-admin) command=reset-admin; admin_name="${2:?}"; shift ;;
    --admin-open-access) command="admin-open-access" ;;
    --no-auto-update) auto_update=false ;;
    --auto-update) auto_update=true ;;
    --allow-unsigned) allow_unsigned=1 ;;
    --coordinator-domain) coord_domain="${2:?}"; shift ;;
    --coordinator-binary) coord_binary="${2:?}"; shift ;;
    --coordinator-only) coord_only=1 ;;
    --alias) aliases+=("${2:?}"); shift ;;
    --admin) admin=true ;;
    --no-admin) admin=false ;;
    --closed-registration) registration=false ;;
    --open-registration) registration=true ;;
    --unlisted) listed=false ;;
    --listed) listed=true ;;
    --status) command=status ;;
    --show-join-token) command=show-token ;;
    --rotate-join-token) command=rotate-token ;;
    --no-firewall) firewall=0 ;;
    --yes|-y) yes=1 ;;
    --force) force=1 ;;
    --uninstall) uninstall=1 ;;
    --purge) purge=1 ;;
    --no-systemd) use_systemd=0 ;;
    -h|--help) usage ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done

[ "$(id -u)" -eq 0 ] || die "run as root (sudo bash $0)"

# --- Commands that only look (or rotate the token) -------------------------

# Everything that goes into service.toml, Caddy's config or a URL is
# checked first: no quotes, newlines or anything else that could change
# what's written.
DOMAIN_RE='^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$'
COORD_RE='^https://([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}(:[0-9]{1,5})?$'
IPV4_RE='^[0-9]{1,3}(\.[0-9]{1,3}){3}$'
NAME_RE="^[A-Za-z0-9][A-Za-z0-9 ._(),'-]{0,63}\$"
BOOL_RE='^(true|false)$'

# service.toml as it is, read without following a link: the server's user
# can write its folder.
config_text() { dd if="$CONFIG" iflag=nofollow status=none 2>/dev/null || true; }
# A value from service.toml: `value section key` (quotes stripped).
value() { config_text | sed -n "/^\\[$1\\]\$/,/^\\[/ s/^$2 = \"\\{0,1\\}\\([^\"]*\\)\"\\{0,1\\}\$/\\1/p" | head -1; }
# A value from service.toml only if it matches REGEX (empty otherwise):
# `checked_value section key regex`. The server (as its own user) can change
# the file, so whatever this script reads back from it and uses again, in
# the file, in Caddy's config or in a command it runs as root, is one of
# the values it would have written itself.
checked_value() {
  local v
  v="$(value "$1" "$2")"
  if [[ "$v" =~ $3 ]]; then printf '%s' "$v"; fi
}
# Text for the terminal, without control characters.
printable() { tr -d '\000-\037\177'; }

# An earlier install's coordinator that hasn't moved yet (the next run moves it).
if [ -n "$command" ] && [ ! -d "$COORD_DIR" ] && [ -d "$OLD_COORD_DIR" ] && [ ! -L "$OLD_COORD_DIR" ]; then
  COORD_DIR="$OLD_COORD_DIR" COORD_USER="$USER_NAME" COORD_ADDR="127.0.0.1"
fi

if [ "$command" = status ]; then
  printf '%-30s %s\n' "Service" "State"
  for s in "$SERVICE" "$COORD_SERVICE" "$STANDBY_SERVICE" caddy; do
    if systemctl cat "$s" >/dev/null 2>&1; then printf '%-30s %s\n' "$s" "$(systemctl is-active "$s" 2>/dev/null || true)"; fi
  done
  if [ -f "$CONFIG" ]; then
    host="$(checked_value public host "$DOMAIN_RE")"
    echo
    info="$(curl -fsS --max-time 3 ${host:+-H "Host: $host"} http://127.0.0.1/api/info 2>/dev/null || true)"
    if [ -n "$info" ]; then
      echo "Server:        $(printf '%s' "$info" | grep -o '"version":"[^"]*"' | cut -d'"' -f4 | printable)${host:+ at $host}"
      case "$info" in *'"api_tls":443'*) echo "API:           HTTPS (https://$host) and plain" ;; *) echo "API:           plain only (no api_tls)" ;; esac
    else
      echo "Server:        doesn't answer /api/info on this machine"
    fi
    echo "Friend lists:  $(value friends mode | printable)"
    echo "Accounts:      $( [ "$(value limits open_registration)" = false ] && echo "closed to new players" || echo "open")"
    echo "Identities:    $( [ "$(value limits require_identity)" = true ] && echo "required for every account" || echo "optional (password-only accounts allowed)")"
    echo "Admin API:     $( [ "$(value admin enabled)" = true ] && echo "on (SSH tunnel to 127.0.0.1:50051; key in $STATE_DIR/admin-key.txt)" || echo "off")"
    coord="$(value federation coordinator | printable)"
    if [ -n "$coord" ]; then
      echo "Shares friends: through $coord$( [ -s "$STATE_DIR/federation.key" ] && echo " (joined)" || echo " (not joined yet)")"
      journalctl -u "$SERVICE" --since "-1h" --no-pager 2>/dev/null | grep -o 'Federation:.*' | tail -1 | sed 's/^/                /' || true
    else
      echo "Shares friends: no"
    fi
  fi
  if [ -f "$UPDATE_PATH_UNIT" ]; then
    echo "Updates:       installs the coordinator's rollouts ($(systemctl is-active 5th-echelon-update.path 2>/dev/null || true)); runs $(head -c 64 "$PROGRAM_DIR/release" 2>/dev/null | printable || echo "an unrecorded release")$( [ -s "$ETC_DIR/min-release" ] && echo ", never older than $(head -c 64 "$ETC_DIR/min-release" | printable)")"
    last="$(head -c 1024 "$UPDATE_DIR/update-status.json" 2>/dev/null | head -1 | printable || true)"
    [ -z "$last" ] || echo "               last: $last"
    checked="$(head -c 1024 "$UPDATE_DIR/verify.json" 2>/dev/null | head -1 | printable || true)"
    echo "Programs:      ${checked:-not checked yet (checked against the signed release at each start)}"
  else
    echo "Updates:       by hand (no updater installed)"
  fi
  if [ -s "$ETC_DIR/metrics-domain" ]; then echo "Admin UI:      https://$(cat "$ETC_DIR/metrics-domain")$( [ -f "$ETC_DIR/metrics-origin-pull" ] && echo " (Cloudflare client certificate required)")"; fi
  if [ -s "$ETC_DIR/coordinator-domain" ]; then
    echo
    cinfo="$(curl -fsS --max-time 3 "http://$COORD_ADDR:8700/v1/info" 2>/dev/null | head -c 300 | printable || true)"
    echo "Coordinator:   https://$(cat "$ETC_DIR/coordinator-domain") ${cinfo:+($cinfo)}"
    listed_now="$(curl -fsS --max-time 3 "http://$COORD_ADDR:8700/v1/servers" 2>/dev/null | grep -o '"id":' | wc -l || true)"
    echo "Directory:     ${listed_now:-0} server(s) seen in the last 2 minutes"
  fi
  if [ -s "$STANDBY_CONF" ]; then
    echo
    echo "Failover:      a standby in $(head -c 2048 "$STANDBY_LIST" 2>/dev/null | printable) (docs/failover.md)"
    if [ -f "$STANDBY_ENV" ]; then "$PROGRAM_DIR/coordinator" standby --config "$STANDBY_CONF" --status 2>&1 | tr -d '\000-\011\013-\037\177' | sed 's/^/               /' || true; fi
  fi
  exit 0
fi
if [ "$command" = show-token ] || [ "$command" = rotate-token ]; then
  [ -f "$COORD_DIR/join-token.txt" ] || die "no coordinator is installed here"
  [ ! -L "$COORD_DIR/join-token.txt" ] || die "$COORD_DIR/join-token.txt isn't a plain file"
  if [ "$command" = rotate-token ]; then
    runuser -u "$COORD_USER" -- "$PROGRAM_DIR/coordinator" --data "$COORD_DIR" new-token >/dev/null
    systemctl restart "$COORD_SERVICE" 2>/dev/null || true
    say "Made a new join token; servers that already joined keep working."
  fi
  echo "Join token (keep it private; give it to the operators of servers joining):"
  head -c 256 "$COORD_DIR/join-token.txt" | printable; echo
  exit 0
fi

if [ "$command" = add-admin ] || [ "$command" = reset-admin ] || [ "$command" = admin-open-access ]; then
  [ -x "$PROGRAM_DIR/coordinator" ] && [ -d "$COORD_DIR" ] || die "no coordinator is installed here"
  [ -s "$ETC_DIR/metrics-domain" ] || warn "the admin UI isn't on yet: run this script again with --metrics-domain NAME"
  [ -z "$admin_name" ] || [[ "$admin_name" =~ ^[A-Za-z0-9._-]{2,32}$ ]] || die "admin names are 2 to 32 letters, digits, . _ and -"
  case "$command" in
    add-admin) runuser -u "$COORD_USER" -- "$PROGRAM_DIR/coordinator" --data "$COORD_DIR" admin add "$admin_name" ;;
    reset-admin) runuser -u "$COORD_USER" -- "$PROGRAM_DIR/coordinator" --data "$COORD_DIR" admin reset "$admin_name" ;;
    admin-open-access) runuser -u "$COORD_USER" -- "$PROGRAM_DIR/coordinator" --data "$COORD_DIR" admin open-access ;;
  esac
  exit 0
fi

# An earlier install that runs only a coordinator stays that way.
if [ "$uninstall" -eq 0 ] && [ -f "$ETC_DIR/coordinator-only" ] && [ -z "$domain" ]; then coord_only=1; fi
case "$relay" in ""|auto|all|off) ;; *) die "--relay is auto, all or off" ;; esac
[ -z "$domain" ] || [ "$no_caddy" -eq 0 ] || die "--domain and --no-caddy don't go together"
case "$friends" in ""|mutual|everyone) ;; *) die "--friends is mutual or everyone" ;; esac
# Updating a machine that runs a coordinator keeps it (and its Caddy site).
if [ -z "$coord_domain" ] && [ -z "$coordinator" ]; then
  if [ -s "$ETC_DIR/coordinator-domain" ]; then
    coord_domain="$(tr -d '[:space:]' < "$ETC_DIR/coordinator-domain")"
  elif [ -f "$OLD_COORD_DIR/domain" ] && [ ! -L "$OLD_COORD_DIR/domain" ]; then
    # Where installs before 0.4 kept it (checked below like --coordinator-domain).
    coord_domain="$(head -c 256 "$OLD_COORD_DIR/domain" | tr -d '[:space:]')"
  fi
fi
# The admin UI stays on (and its Cloudflare client certificate required) once set up.
if [ -z "$metrics_domain" ] && [ -s "$ETC_DIR/metrics-domain" ]; then metrics_domain="$(head -c 256 "$ETC_DIR/metrics-domain" | tr -d '[:space:]')"; fi
if [ -f "$ETC_DIR/metrics-origin-pull" ]; then origin_pull=1; fi
[ -z "$metrics_domain" ] || [ -n "$coord_domain" ] || die "--metrics-domain is the coordinator's admin UI: it needs --coordinator-domain (a coordinator on this machine)"
[ "$origin_pull" -eq 0 ] || [ -n "$metrics_domain" ] || die "--cloudflare-origin-pull goes with --metrics-domain"
if [ -n "$metrics_cert" ] || [ -n "$metrics_key" ]; then
  [ -n "$metrics_cert" ] && [ -n "$metrics_key" ] || die "--metrics-cert and --metrics-key go together"
fi
[ -z "$metrics_cert" ] || [ -n "$metrics_domain" ] || die "--metrics-cert goes with --metrics-domain"
if [ -n "$coord_cert" ] || [ -n "$coord_key" ]; then
  [ -n "$coord_cert" ] && [ -n "$coord_key" ] || die "--coordinator-cert and --coordinator-key go together"
  [ -n "$coord_domain" ] || die "--coordinator-cert goes with --coordinator-domain"
fi
# A failover group stays once joined (docs/failover.md says how to leave one).
if [ -z "$standby" ] && [ -s "$STANDBY_LIST" ]; then standby="$(head -c 2048 "$STANDBY_LIST" | tr -d '[:space:]')"; fi
if [ -n "$standby" ]; then
  standby="${standby,,}"
  [ -n "$coord_domain" ] || die "--standby needs --coordinator-domain: the coordinator's name, the same on every server of the group"
  [ "$coord_only" -eq 0 ] || die "--standby is for game servers (each one asks the others whether they reach the coordinator)"
  [ "$no_caddy" -eq 0 ] && [ "$https_api" -eq 1 ] || die "--standby needs the API over HTTPS (no --no-caddy or --no-https-api): the standbys ask each other's /api/info over HTTPS"
  [ -n "$coord_cert" ] || [ -f "$COORD_CERT" ] || die "--standby needs --coordinator-cert and --coordinator-key: the coordinator's record is proxied through Cloudflare, so it moves for everyone at once (docs/failover.md)"
  standby_names=" "
  IFS=, read -r -a standby_entries <<< "$standby"
  [ "${#standby_entries[@]}" -ge 2 ] || die "--standby lists two servers or more: NAME=HOST,NAME=HOST"
  for entry in "${standby_entries[@]}"; do
    [[ "$entry" =~ ^[a-z0-9-]{1,64}=.+$ ]] && [[ "${entry#*=}" =~ $DOMAIN_RE ]] || die "--standby: \"$entry\" isn't NAME=HOST (a backup name of letters, digits and -, then the server's domain)"
    [[ "$standby_names" != *" ${entry%%=*} "* ]] || die "--standby: ${entry%%=*} is in the list twice"
    standby_names="$standby_names${entry%%=*} "
  done
fi
if [ "$coord_only" -eq 1 ]; then
  [ -n "$coord_domain" ] || die "--coordinator-only needs --coordinator-domain"
  [ -z "$domain" ] && [ "$no_caddy" -eq 0 ] && [ -z "$coordinator" ] || die "--coordinator-only runs no game server: leave out --domain, --no-caddy and --coordinator"
fi
[ -z "$coord_domain" ] || [ "$no_caddy" -eq 0 ] || die "--coordinator-domain needs Caddy (for HTTPS); leave out --no-caddy"
[ -z "$coord_domain" ] || [ -z "$coordinator" ] || die "--coordinator-domain runs a coordinator here; leave out --coordinator"
[ -z "$coordinator" ] || [ -n "$join_token" ] || [ -f "$STATE_DIR/federation.key" ] || die "--coordinator needs --join-token (from the coordinator's operator)"
# Everything from the command line is checked (the patterns are above).
if [ -n "$coordinator" ]; then
  coordinator="${coordinator%/}"
  [[ "$coordinator" =~ $COORD_RE ]] \
    || die "--coordinator is an https:// address with no path, e.g. https://coordinator.example.com"
fi
[ -z "$join_token" ] || [[ "$join_token" =~ ^[A-Z2-7]{16,128}$ ]] || die "that isn't a join token (letters A-Z and digits 2-7)"
[ -z "$server_name" ] || [[ "$server_name" =~ $NAME_RE ]] || die "--server-name is up to 64 letters, digits, spaces and . _ ( ) , ' -"
[ -z "$region" ] || [[ "$region" =~ $NAME_RE ]] || die "--region is up to 64 letters, digits, spaces and . _ ( ) , ' -"
[ "$version" = latest ] || [[ "$version" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?$ ]] || die "--version is a release number, e.g. 0.4.0"
coord_domain="${coord_domain,,}"
[ -z "$coord_domain" ] || [[ "$coord_domain" =~ $DOMAIN_RE ]] || die "\"$coord_domain\" isn't a domain name"
metrics_domain="${metrics_domain,,}"
[ -z "$metrics_domain" ] || [[ "$metrics_domain" =~ $DOMAIN_RE ]] || die "\"$metrics_domain\" isn't a domain name"
[ -z "$metrics_domain" ] || [ "$metrics_domain" != "$coord_domain" ] || die "the admin UI needs a name of its own (proxied through Cloudflare), not the coordinator's"
for i in "${!aliases[@]}"; do
  aliases[i]="${aliases[i],,}"
  [[ "${aliases[i]}" =~ $DOMAIN_RE ]] || [[ "${aliases[i]}" =~ $IPV4_RE ]] || die "--alias ${aliases[i]} isn't a domain name or IPv4 address"
done

# --- The system ---------------------------------------------------------

[ -r /etc/os-release ] || die "can't tell which Linux this is (no /etc/os-release)"
# shellcheck disable=SC1091
. /etc/os-release
os_id="${ID:-linux}" os_like="${ID_LIKE:-}" os_name="${PRETTY_NAME:-$os_id}"

family=""
for id in $os_id $os_like; do
  case "$id" in
    debian|ubuntu) family=debian; break ;;
    fedora|rhel|centos|rocky|almalinux) family=fedora; break ;;
    arch) family=arch; break ;;
    alpine) die "Alpine uses musl, and the server is built for glibc. Use the Docker image instead (ghcr.io/jdevwebb/5th-echelon-server)." ;;
  esac
done
[ -n "$family" ] || die "$os_name isn't supported by this script (Debian, Ubuntu, Fedora, RHEL-likes, Arch). The Docker image runs anywhere."
# Fedora itself has Caddy; its RHEL relatives get it from Caddy's COPR.
rhel_like=0
if [ "$family" = fedora ] && [ "$os_id" != fedora ]; then rhel_like=1; fi

if [ "$uninstall" -eq 0 ]; then
  arch="$(uname -m)"
  [ "$arch" = x86_64 ] || die "the server is built for x86_64, and this machine is $arch. Build it from source (see the README) or use a different VPS."
fi
if [ "$use_systemd" -eq 1 ] && [ ! -d /run/systemd/system ]; then
  die "this machine doesn't run systemd. Use the Docker image, or --no-systemd to only install the files."
fi

say "$os_name ($family), $(uname -m)"

# The PIDs of every running copy of the installed server: the program a process
# runs, or, where /proc/*/exe names an emulator instead (qemu-user) or can't be read,
# the program its command line starts with. Only those exactly: a shell or an editor whose command
# line mentions the server's path isn't the server.
server_pids() {
  local p exe a0 a1 bin="$PROGRAM_DIR/dedicated_server"
  for p in /proc/[0-9]*; do
    [ "${p#/proc/}" = "$$" ] && continue
    # Unreadable without ptrace rights over the process (another user's, in a container):
    # then the command line, read exactly, decides.
    exe="$(readlink "$p/exe" 2>/dev/null)" || exe=""
    case "$exe" in
      "$bin"|"$bin (deleted)") echo "${p#/proc/}"; continue ;;
      ""|*/qemu-*) ;;
      *) continue ;;
    esac
    a0="" a1=""
    { IFS= read -r -d '' a0 && IFS= read -r -d '' a1; } < "$p/cmdline" 2>/dev/null || true
    if [ "${a0:-}" = "$bin" ] || { [[ "${a0:-}" == *qemu-* ]] && [ "${a1:-}" = "$bin" ]; }; then echo "${p#/proc/}"; fi
  done
}

# Stops every running copy of the installed server (not the service).
stop_strays() {
  local pids
  pids="$(server_pids)"
  [ -z "$pids" ] && return 0
  # shellcheck disable=SC2086
  kill $pids 2>/dev/null || true
  for _ in $(seq 25); do
    [ -z "$(server_pids)" ] && return 0
    sleep 0.2
  done
  # shellcheck disable=SC2046
  kill -9 $(server_pids) 2>/dev/null || true
}

# Stops every process of the game server's user (whatever its program).
stop_user_processes() {
  local uid p
  uid="$(id -u "$USER_NAME" 2>/dev/null)" || return 0
  for p in /proc/[0-9]*; do
    if [ "$(stat -c %u "$p" 2>/dev/null)" = "$uid" ]; then kill -9 "${p#/proc/}" 2>/dev/null || true; fi
  done
}

# Replaces the installer's block in a Caddyfile (between the markers) with
# the file $2 (or nothing), keeping everything else.
replace_managed() {
  local file="$1" with="$2" tmp line skipping=0
  # Without the end marker, everything after the start would go: stop instead.
  if ! sed -n "/^$(printf '%s' "$BEGIN_MARK" | sed 's/[][\\/.*^$]/\\&/g')\$/,\$p" "$file" | grep -qxF "$END_MARK"; then
    die "$file has the line \"$BEGIN_MARK\" but not \"$END_MARK\" after it; put the end marker back after the 5th Echelon block and run this again"
  fi
  tmp="$(mktemp)"
  while IFS= read -r line || [ -n "$line" ]; do
    if [ "$line" = "$BEGIN_MARK" ]; then
      skipping=1
      if [ -n "$with" ]; then cat "$with" >> "$tmp"; fi
      continue
    fi
    if [ "$skipping" -eq 1 ]; then
      [ "$line" = "$END_MARK" ] && skipping=0
      continue
    fi
    printf '%s\n' "$line" >> "$tmp"
  done < "$file"
  cat "$tmp" > "$file"
  rm -f "$tmp"
}

# --- Uninstall ----------------------------------------------------------

if [ "$uninstall" -eq 1 ]; then
  if [ "$use_systemd" -eq 1 ]; then
    systemctl disable --now "$SERVICE" 2>/dev/null || true
    systemctl disable --now "$COORD_SERVICE" 2>/dev/null || true
    systemctl disable --now 5th-echelon-update.path 2>/dev/null || true
    for u in 5th-echelon-backup.service 5th-echelon-coordinator-backup.service 5th-echelon-backup-archive.timer 5th-echelon-backup-files.timer; do
      systemctl disable --now "$u" 2>/dev/null || true
      rm -f "/etc/systemd/system/$u" "/etc/systemd/system/${u%.timer}.service"
    done
    rm -f "$BACKUP_SCRIPT" "$ETC_DIR"/litestream-*.yml
    systemctl disable --now "$STANDBY_SERVICE" 2>/dev/null || true
    rm -f "$STANDBY_UNIT" "$STANDBY_CONF" "$STANDBY_LIST"
    rm -f "$UNIT" "$COORD_UNIT" "$UPDATE_PATH_UNIT" "$UPDATE_SERVICE_UNIT" "$CADDY_DROPIN"
    systemctl daemon-reload
  fi
  stop_strays
  if [ -f "$CADDY_SITE" ] || grep -qxF "$BEGIN_MARK" "$CADDYFILE" 2>/dev/null; then
    rm -f "$CADDY_SITE"
    sed -i '\|^import /etc/caddy/5th-echelon.caddy$|d' "$CADDYFILE" 2>/dev/null || true
    if grep -qxF "$BEGIN_MARK" "$CADDYFILE" 2>/dev/null; then
      replace_managed "$CADDYFILE" ""
      # Nothing of anyone else's left: put back what was there before.
      if ! grep -vE '^[[:space:]]*(#|$)' "$CADDYFILE" | grep -q .; then
        if [ -f "$CADDYFILE.before-5th-echelon" ]; then mv "$CADDYFILE.before-5th-echelon" "$CADDYFILE"; else rm -f "$CADDYFILE"; fi
      fi
    fi
    systemctl reload-or-restart caddy 2>/dev/null || true
    say "Removed the server's Caddy site (Caddy itself stays installed)."
  fi
  # The firewall rules this installer added.
  if [ -s "$FIREWALL_RECORD" ]; then
    while read -r tool rule; do
      case "$tool" in
        ufw) command -v ufw >/dev/null && ufw delete allow "$rule" >/dev/null 2>&1 || true ;;
        firewalld) command -v firewall-cmd >/dev/null && firewall-cmd --permanent --remove-port="$rule" >/dev/null 2>&1 || true ;;
      esac
    done < "$FIREWALL_RECORD"
    if command -v firewall-cmd >/dev/null && grep -q '^firewalld ' "$FIREWALL_RECORD"; then firewall-cmd --reload >/dev/null 2>&1 || true; fi
    say "Removed the firewall rules the installer added."
  fi
  rm -rf "$PROGRAM_DIR" "$ETC_DIR" "$UPDATE_DIR"
  rm -f "$SYSCTL_FILE"
  if [ "$purge" -eq 1 ]; then
    rm -rf "$STATE_DIR" "$COORD_DIR"
    userdel "$USER_NAME" 2>/dev/null || true
    userdel "$COORD_USER" 2>/dev/null || true
    say "Removed the server and its data."
  else
    say "Removed the server. Its data (accounts, settings, keys) is still in $STATE_DIR$( [ -d "$COORD_DIR" ] && echo " and $COORD_DIR"); --purge deletes it."
  fi
  exit 0
fi

# --- Packages -----------------------------------------------------------

install_packages() {
  say "Installing packages: $*"
  case "$family" in
    # Waits for the package lock (automatic updates on a fresh VPS) instead of failing.
    debian) DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=600 update -qq && DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=600 install -y -qq "$@" >/dev/null ;;
    fedora) if command -v dnf >/dev/null; then dnf install -y -q "$@" >/dev/null; else yum install -y -q "$@" >/dev/null; fi ;;
    arch) pacman -Sy --noconfirm --needed "$@" >/dev/null ;;
  esac
}

need=()
command -v curl >/dev/null || need+=(curl)
[ -e /etc/ssl/certs/ca-certificates.crt ] || [ -e /etc/pki/tls/certs/ca-bundle.crt ] || [ -e /etc/ssl/ca-bundle.pem ] || need+=(ca-certificates)
command -v sha256sum >/dev/null || need+=(coreutils)
if { [ "$coord_only" -eq 0 ] && [ -z "$binary" ]; } || { [ -n "$coord_domain" ] && [ -z "$coord_binary" ]; }; then
  command -v openssl >/dev/null || need+=(openssl)
fi
command -v ss >/dev/null || case "$family" in debian|arch) need+=(iproute2) ;; fedora) need+=(iproute) ;; esac
command -v useradd >/dev/null || case "$family" in debian) need+=(passwd) ;; fedora) need+=(shadow-utils) ;; arch) need+=(shadow) ;; esac
command -v runuser >/dev/null || need+=(util-linux)
command -v getent >/dev/null || case "$family" in debian) need+=(libc-bin) ;; *) need+=(glibc) ;; esac
[ "${#need[@]}" -eq 0 ] || install_packages "${need[@]}"

# --- Public address -----------------------------------------------------

if [ -z "$public_address" ]; then
  for url in https://api.ipify.org https://ipv4.icanhazip.com https://ifconfig.me/ip; do
    public_address="$("${CURL[@]}" -4 -fsS --max-time 5 "$url" 2>/dev/null | tr -d '[:space:]')" || true
    [[ "$public_address" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] && break
    public_address=""
  done
  [ -n "$public_address" ] || die "couldn't find this machine's public address; pass --public-address"
  say "Public address: $public_address (detected; --public-address overrides it)"
fi
[[ "$public_address" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--public-address must be an IPv4 address"

# --- Domain name (Caddy) ------------------------------------------------

# Checks that NAME points at this server (IPv4), and that no IPv6 record
# sends certificate checks or players elsewhere.
check_dns() {
  local name="$1" resolved v6
  resolved="$(getent ahostsv4 "$name" 2>/dev/null | cut -d' ' -f1 | sort -u | tr '\n' ' ' || true)"
  if [ -z "$resolved" ]; then
    warn "$name doesn't resolve yet. Add an A record for it pointing at $public_address."
    confirm "Go on anyway?" || die "stopped; run this again once $name points here"
  elif [[ " $resolved" != *" $public_address "* ]]; then
    warn "$name points at ${resolved% }, not at this server ($public_address)."
    echo "  On Cloudflare, set the record to \"DNS only\" (grey cloud): the game's UDP traffic and"
    echo "  its plain HTTP on port 80 can't go through Cloudflare's proxy." >&2
    confirm "Go on anyway?" || die "stopped; fix the A record, or pass --public-address if $public_address is wrong"
  else
    say "$name points here"
  fi
  v6="$(getent ahostsv6 "$name" 2>/dev/null | cut -d' ' -f1 | grep -v '^::ffff:' | sort -u | tr '\n' ' ' || true)"
  if [ -n "$v6" ]; then
    warn "$name also has an IPv6 (AAAA) record (${v6% }). The server and launcher use IPv4, and Caddy's certificate check may try IPv6: remove the AAAA record unless it's this server."
  fi
}

if [ "$coord_only" -eq 0 ]; then

# A domain from an earlier install is kept unless told otherwise.
if [ -z "$domain" ] && [ "$no_caddy" -eq 0 ] && [ -f "$CONFIG" ]; then
  domain="$(checked_value public host "$DOMAIN_RE")"
  if [ -n "$domain" ]; then say "Domain name: $domain (from the earlier install)"; fi
fi
if [ -z "$domain" ] && [ "$no_caddy" -eq 0 ] && [ "$yes" -eq 0 ]; then
  echo
  echo "Players can reach the server by a domain name through Caddy, which then"
  echo "serves the game's web parts on port 80 (shared with any other sites)."
  echo "Point an A record at $public_address first."
  domain="$(ask "Domain name for this server (empty: no domain, no Caddy): ")"
  domain="${domain,,}"
fi
[ -n "$domain" ] || no_caddy=1
# Without Caddy, the server's own port 50051 (plain HTTP/2, every address)
# would carry the admin API and its key.
if [ "$no_caddy" -eq 1 ] && [ "$admin" = true ]; then
  die "--admin needs Caddy (a domain name): without it, the admin API and its key would travel unencrypted on port 50051, open to every address. Use --domain, or manage accounts on the machine itself"
fi
if [ "$no_caddy" -eq 0 ]; then
  [[ "$domain" =~ $DOMAIN_RE ]] || die "\"$domain\" isn't a domain name"
  # The game keeps its server's name where onlineconfigservice.ubi.com was: 27 characters.
  [ "${#domain}" -le 27 ] || die "\"$domain\" is ${#domain} characters; the game takes at most 27 (e.g. eu1.example.com). Players can't start the game with a longer one"
  check_dns "$domain"
fi
fi

# --- Sharing friends (asked on a first install) --------------------------

if [ "$coord_only" -eq 0 ] && [ "$no_caddy" -eq 0 ] && [ -z "$coordinator" ] && [ -z "$coord_domain" ] \
  && ! grep -q '^coordinator = ' "$CONFIG" 2>/dev/null && interactive; then
  echo
  echo "Servers can share friends through a coordinator: friends follow players"
  echo "between them, and they're listed together in a server directory."
  echo "  1) not now: this server on its own"
  echo "  2) run a coordinator here too (the first server of a group)"
  echo "  3) join a group's coordinator (you need its address and join token)"
  case "$(ask "Choose 1, 2 or 3 [1]: ")" in
    2)
      echo "The coordinator needs its own name, e.g. coord.${domain#*.}, with an A record pointing at $public_address."
      coord_domain="$(ask "Coordinator's domain name: ")"
      coord_domain="${coord_domain,,}"
      [[ "$coord_domain" =~ $DOMAIN_RE ]] || die "\"$coord_domain\" isn't a domain name"
      check_dns "$coord_domain"
      coord_dns_checked=1
      ;;
    3)
      coordinator="$(ask "Coordinator's address (https://...): ")"
      coordinator="${coordinator%/}"
      [[ "$coordinator" =~ $COORD_RE ]] || die "that isn't an https:// address with no path"
      join_token="$(ask_secret "Join token (from the coordinator's operator; not shown): " | tr -d '[:space:]')"
      [[ "$join_token" =~ ^[A-Z2-7]{16,128}$ ]] || die "that isn't a join token (letters A-Z and digits 2-7)"
      ;;
  esac
  if [ -n "$coordinator$coord_domain" ]; then
    [ -n "$server_name" ] || server_name="$(ask "Name in the server directory [$domain]: ")"
    [ -z "$server_name" ] || [[ "$server_name" =~ $NAME_RE ]] || die "the name is up to 64 letters, digits, spaces and . _ ( ) , ' -"
    [ -n "$region" ] || region="$(ask "Region, e.g. Sydney (optional): ")"
    [ -z "$region" ] || [[ "$region" =~ $NAME_RE ]] || die "the region is up to 64 letters, digits, spaces and . _ ( ) , ' -"
  fi
fi
# A coordinator behind Cloudflare (an Origin certificate): its record must be proxied before
# Caddy refuses everyone but Cloudflare, or nobody reaches it.
check_proxied() {
  local name="$1"
  if curl -sS -o /dev/null -D - --max-time 10 "https://$name/v1/info" 2>/dev/null | grep -qi '^cf-ray:'; then
    say "$name is proxied through Cloudflare"
  elif [ "$force" -eq 1 ]; then
    warn "$name doesn't answer through Cloudflare; going on (--force)"
  else
    die "$name doesn't answer through Cloudflare. Make its record proxied (orange cloud) first, with SSL/TLS set to Full (strict): with --coordinator-cert, Caddy refuses everything that doesn't come through Cloudflare (docs/failover.md). --force goes on anyway"
  fi
}
if [ -n "$coord_domain" ] && [ "${coord_dns_checked:-0}" -eq 0 ]; then
  if [ -n "$coord_cert" ] || [ -f "$COORD_CERT" ]; then check_proxied "$coord_domain"; else check_dns "$coord_domain"; fi
fi
# This server in its failover group: the one listed by its domain.
if [ -n "$standby" ]; then
  for entry in "${standby_entries[@]}"; do
    if [ "${entry#*=}" = "$domain" ]; then standby_me="${entry%%=*}"; fi
  done
  [ -n "$standby_me" ] || die "--standby doesn't list this server ($domain): each server is NAME=its --domain"
fi

# --- The program --------------------------------------------------------

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Checks the release's SHA256SUMS against its signature (SHA256SUMS.sig,
# base32, by the release key): whoever can change a release on GitHub
# still can't change what this installs. The signature covers the version
# too (the tag, without its v), so an old release published again under a
# new tag doesn't verify.
verify_release() {
  local base="$1" release="$2"
  if ! "${CURL[@]}" -fsSL --retry 3 "${SMALL[@]}" -o "$work/SHA256SUMS.sig" "$base/SHA256SUMS.sig" 2>/dev/null; then
    [ "$allow_unsigned" -eq 1 ] || die "this release isn't signed (no SHA256SUMS.sig). --allow-unsigned installs it anyway, checked by its checksum only"
    warn "the release isn't signed; installing it on its checksum alone (--allow-unsigned)"
    return 0
  fi
  printf '%s\n' "$RELEASE_KEY_PEM" > "$work/release.pem"
  { printf '5th-echelon/release/v2\n%s\n' "$release"; cat "$work/SHA256SUMS"; } > "$work/signed"
  # base32 without padding: pad it for coreutils.
  local sig
  sig="$(tr -d '[:space:]' < "$work/SHA256SUMS.sig")"
  while [ $(( ${#sig} % 8 )) -ne 0 ]; do sig="$sig="; done
  printf '%s' "$sig" | base32 -d > "$work/signature" 2>/dev/null || die "the release's signature is malformed"
  if openssl pkeyutl -verify -pubin -inkey "$work/release.pem" -rawin -in "$work/signed" -sigfile "$work/signature" >/dev/null 2>&1; then
    say "Release signature verified"
  elif ! openssl pkeyutl -help 2>&1 | grep -q -- '-rawin'; then
    [ "$allow_unsigned" -eq 1 ] || die "this system's OpenSSL ($(openssl version)) can't check the release's signature (it needs OpenSSL 3). --allow-unsigned installs on the checksum alone"
    warn "OpenSSL can't check the signature here; installing on the checksum alone (--allow-unsigned)"
  else
    die "the release's signature doesn't match: SHA256SUMS wasn't signed by the release key as release $release. Not installing it"
  fi
}

# Which release to download: the one asked for, or the latest one's tag
# (from GitHub's API, or else where /releases/latest leads). Downloads then
# come from that tag, and its signature must name that version.
resolve_version() {
  [ -z "$release_version" ] || return 0
  if [ "$version" = latest ]; then
    version="$("${CURL[@]}" -fsSL --retry 3 "${SMALL[@]}" "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d'"' -f4 || true)"
    if [ -z "$version" ]; then
      version="$("${CURL[@]}" -fsSI --retry 3 "${SMALL[@]}" -o /dev/null -w '%{redirect_url}' "https://github.com/$REPO/releases/latest" 2>/dev/null || true)"
      version="${version##*/tag/}"
    fi
    [[ "${version#v}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?$ ]] || die "couldn't find which release is the latest (GitHub didn't answer); pass --version"
  fi
  release_version="${version#v}"
  base="https://github.com/$REPO/releases/download/v$release_version"
}

if [ "$coord_only" -eq 1 ]; then
  :
elif [ -n "$binary" ]; then
  [ -f "$binary" ] || die "$binary doesn't exist"
  cp "$binary" "$work/$ASSET"
  say "Using $binary"
else
  resolve_version
  say "Downloading the server ($version)"
  "${CURL[@]}" -fsSL --retry 3 -o "$work/$ASSET" "$base/$ASSET" \
    || die "couldn't download $base/$ASSET (is there a release yet? --binary installs a file you have)"
  "${CURL[@]}" -fsSL --retry 3 "${SMALL[@]}" -o "$work/SHA256SUMS" "$base/SHA256SUMS" || die "couldn't download the release's SHA256SUMS"
  verify_release "$base" "$release_version"
  (cd "$work" && grep " $ASSET\$" SHA256SUMS | sha256sum -c --quiet -) || die "the download doesn't match the release's checksum"
  say "Checksum verified"
fi
[ "$coord_only" -eq 1 ] || chmod 755 "$work/$ASSET"
chmod 711 "$work"
if [ -n "$coord_domain" ]; then
  if [ -n "$coord_binary" ]; then
    [ -f "$coord_binary" ] || die "$coord_binary doesn't exist"
    cp "$coord_binary" "$work/$COORD_ASSET"
    say "Using $coord_binary"
  elif [ -n "$binary" ]; then
    die "with --binary, also give --coordinator-binary (the coordinator of the same build)"
  else
    say "Downloading the coordinator ($version)"
    if [ ! -f "$work/SHA256SUMS" ]; then
      resolve_version
      "${CURL[@]}" -fsSL --retry 3 "${SMALL[@]}" -o "$work/SHA256SUMS" "$base/SHA256SUMS" || die "couldn't download the release's SHA256SUMS"
      verify_release "$base" "$release_version"
    fi
    "${CURL[@]}" -fsSL --retry 3 -o "$work/$COORD_ASSET" "$base/$COORD_ASSET" || die "couldn't download $base/$COORD_ASSET"
    (cd "$work" && grep " $COORD_ASSET\$" SHA256SUMS | sha256sum -c --quiet -) || die "the coordinator doesn't match the release's checksum"
  fi
  chmod 755 "$work/$COORD_ASSET"
fi
# Whether it runs here at all (a C library too old for it, say).
# As nobody: nothing new runs as root.
probe="$work/$ASSET"
[ "$coord_only" -eq 0 ] || probe="$work/$COORD_ASSET"
if ! out="$(cd / && runuser -u nobody -- "$probe" --help 2>&1)"; then
  case "$out" in
    *GLIBC*) die "$os_name's C library is too old for the server (it needs glibc 2.34: Ubuntu 22.04+, Debian 12+, RHEL/Rocky/Alma 9+, Fedora 35+). Use a newer system, or the Docker image.
$out" ;;
    *) die "the server doesn't run on this system:
$out" ;;
  esac
fi

# --- Ports --------------------------------------------------------------

updating=0
if [ -f "$COORD_UNIT" ] && [ "$coord_only" -eq 1 ]; then updating=1; fi
if [ "$use_systemd" -eq 1 ] && [ -f "$UNIT" ]; then
  updating=1
  say "Stopping the installed server"
  systemctl stop "$SERVICE" 2>/dev/null || true
fi
stop_strays
# An earlier install's coordinator runs as the game server's user, from its
# folder: it stops while it moves (below).
if [ "$use_systemd" -eq 1 ] && [ -f "$COORD_UNIT" ] && grep -qx "User=$USER_NAME" "$COORD_UNIT"; then
  systemctl stop "$COORD_SERVICE" 2>/dev/null || true
fi
# Nothing runs as the game server's user while this script (as root)
# changes files in its folder.
stop_user_processes

port_owner() { ss -Hlnp "$1" "sport = :$2" 2>/dev/null | grep -o 'users:(("[^"]*"' | head -1 | cut -d'"' -f2 || true; }
busy=()
if [ "$coord_only" -eq 1 ]; then
  tcp_check=()
  [ -f "$COORD_UNIT" ] || tcp_check=(8700)
  owner="$(port_owner -t 80)"
  if [ -n "$owner" ] && [ "$owner" != caddy ]; then busy+=("80/tcp (held by $owner; Caddy needs it)"); fi
elif [ "$no_caddy" -eq 1 ]; then
  tcp_check=(80 8000 50051)
else
  tcp_check=(8000 8080 50051)
  owner="$(port_owner -t 80)"
  if [ -n "$owner" ] && [ "$owner" != caddy ]; then
    busy+=("80/tcp (held by $owner; Caddy needs it: stop $owner, or add the site from docs/reverse-proxy.md to it and use --no-caddy)")
  fi
fi
for p in "${tcp_check[@]}"; do
  owner="$(port_owner -t "$p")"
  if [ -n "$owner" ]; then busy+=("$p/tcp ($owner)"); fi
done
game_udp=(21126 21127 21128 21129)
[ "$coord_only" -eq 0 ] || game_udp=()
for p in "${game_udp[@]}"; do
  owner="$(port_owner -u "$p")"
  if [ -n "$owner" ]; then busy+=("$p/udp ($owner)"); fi
done
if [ "${#busy[@]}" -gt 0 ]; then
  msg="ports already in use: ${busy[*]}"
  if [ "$force" -eq 1 ]; then warn "$msg"; else die "$msg (--force installs anyway)"; fi
fi

# --- Install ------------------------------------------------------------

# A system user for a service: `system_user NAME HOME`.
system_user() {
  id "$1" >/dev/null 2>&1 && return 0
  say "Creating the system user $1"
  useradd --system --home-dir "$2" --no-create-home --shell /usr/sbin/nologin "$1" 2>/dev/null \
    || useradd --system --home-dir "$2" --no-create-home --shell /sbin/nologin "$1"
}
system_user "$USER_NAME" "$STATE_DIR"
[ -z "$coord_domain" ] || system_user "$COORD_USER" "$COORD_DIR"

say "Installing to $PROGRAM_DIR (program) and $STATE_DIR (settings, accounts, keys)"
install -d -m 755 "$PROGRAM_DIR"
install -d -m 755 "$ETC_DIR"
install -d -m 750 -o "$USER_NAME" -g "$USER_NAME" "$STATE_DIR"
chmod 750 "$STATE_DIR"
if [ "$coord_only" -eq 1 ]; then
  touch "$ETC_DIR/coordinator-only"
else
  rm -f "$ETC_DIR/coordinator-only"
  install -m 755 "$work/$ASSET" "$PROGRAM_DIR/dedicated_server"
  # The server keeps data/ next to its program.
  install -d -m 750 -o "$USER_NAME" -g "$USER_NAME" "$PROGRAM_DIR/data"
  chmod 750 "$PROGRAM_DIR/data"
fi
# The service owns its folder, so what's in it could be a link planted to
# make this script (as root) write elsewhere: refuse that.
for f in "$CONFIG" "$STATE_DIR/federation.key" "$STATE_DIR/admin-key.txt"; do
  if [ -L "$f" ] || { [ -e "$f" ] && [ ! -f "$f" ]; }; then die "$f isn't a plain file; move it away and run this again"; fi
done
if command -v restorecon >/dev/null; then restorecon -R "$PROGRAM_DIR" "$STATE_DIR" 2>/dev/null || true; fi

# --- The game server's settings --------------------------------------------

if [ "$coord_only" -eq 0 ]; then
first_install=0
# The first start writes the default settings.
if [ ! -f "$CONFIG" ]; then
  first_install=1
  say "Writing the default settings"
  cd "$STATE_DIR"
  runuser -u "$USER_NAME" -- sh -c 'umask 077; exec "$0" "$@"' "$PROGRAM_DIR/dedicated_server" --public-address "$public_address" >/dev/null 2>&1 &
  generator=$!
  for _ in $(seq 50); do [ -s "$CONFIG" ] && break; sleep 0.2; done
  sleep 0.5
  stop_strays
  kill "$generator" 2>/dev/null || true
  wait "$generator" 2>/dev/null || true
  cd /
  [ -s "$CONFIG" ] || die "the server didn't start (run $PROGRAM_DIR/dedicated_server by hand in $STATE_DIR to see why)"
fi

# Refuses to go on when service.toml isn't a plain file (a link the
# server's user planted, to have root write elsewhere).
plain_config() {
  if [ -L "$CONFIG" ] || [ ! -f "$CONFIG" ]; then die "$CONFIG isn't a plain file; move it away and run this again"; fi
}
# Sets `key = value` in [section] of service.toml (`toml_set section key
# value`); toml_put also adds the key (after the section's heading), or the
# section, when missing. Values are this script's own or checked ones; they
# reach awk through its environment, never as part of a program, and the
# file is rewritten in place, so it stays the server's.
toml_edit() {
  local tmp="$work/service.toml.new"
  plain_config
  MODE="$1" SECTION="$2" KEY="$3" VALUE="$4" awk '
    BEGIN { head = "[" ENVIRON["SECTION"] "]"; line = ENVIRON["KEY"] " = " ENVIRON["VALUE"]; prefix = ENVIRON["KEY"] " =" }
    FNR == 1 { pass++; in_section = 0 }
    /^\[/ { in_section = ($0 == head) }
    pass == 1 { if ($0 == head) has_section = 1; if (in_section && index($0, prefix) == 1) has_key = 1; next }
    in_section && index($0, prefix) == 1 { print line; next }
    { print }
    $0 == head && !has_key && ENVIRON["MODE"] == "add" { print line }
    END { if (!has_section && ENVIRON["MODE"] == "add") printf "\n%s\n%s\n", head, line }
  ' "$CONFIG" "$CONFIG" > "$tmp"
  cat "$tmp" > "$CONFIG"
  rm -f "$tmp"
}
toml_set() { toml_edit set "$@"; }
toml_put() { toml_edit add "$@"; }
if [ -n "$relay" ]; then
  toml_set nat relay "\"$relay\""
  say "Relay: $relay"
fi
# [public] (the host name and ports players use) is rewritten below; its
# aliases stay unless new ones are given.
if [ "${#aliases[@]}" -gt 0 ]; then
  aliases_line="[$(printf '"%s", ' "${aliases[@]}" | sed 's/, $//')]"
else
  aliases_line="$(config_text | sed -n '/^\[public\]$/,/^\[/ s/^aliases = \(\[.*\]\)$/\1/p' | head -1)"
  [[ "$aliases_line" =~ ^\[(\"[a-z0-9.-]+\"(,\ )?)*\]$ ]] || aliases_line=""
  # Only names and addresses, checked as --alias checks them (they go into
  # Caddy's config too).
  kept=()
  while IFS= read -r a; do
    if [[ "$a" =~ $DOMAIN_RE ]] || [[ "$a" =~ $IPV4_RE ]]; then kept+=("$a"); elif [ -n "$a" ]; then warn "dropped the alias \"$a\" from $CONFIG: it isn't a domain name or IPv4 address"; fi
  done < <(printf '%s' "$aliases_line" | grep -o '"[^"]*"' | tr -d '"')
  aliases=("${kept[@]}")
  aliases_line=""
  if [ "${#aliases[@]}" -gt 0 ]; then aliases_line="[$(printf '"%s", ' "${aliases[@]}" | sed 's/, $//')]"; fi
fi
# The API over HTTPS stays on while Caddy has its certificate for the same
# name (checked again once Caddy runs).
tls_kept=0
if [ "$no_caddy" -eq 0 ] && [ "$https_api" -eq 1 ] && [ "$(checked_value public api_tls '^443$')" = 443 ] \
  && [ "$(checked_value public host "$DOMAIN_RE")" = "$domain" ]; then
  tls_kept=1
fi
plain_config
sed -i '/^\[public\]$/,/^\[/{/^\[public\]$/d;/^\[/!d}' "$CONFIG"
if [ "$no_caddy" -eq 0 ]; then
  say "Settings for Caddy: the web parts and API on this machine only; players use $domain"
  sed -i -E 's|^api_server = .*|api_server = "127.0.0.1:50051"|' "$CONFIG"
  toml_set service.onlineconfig listen '"127.0.0.1:8080"'
  toml_set service.content listen '"127.0.0.1:8000"'
  printf '\n[public]\nhost = "%s"\napi = 80\ncontent = 80\n' "$domain" >> "$CONFIG"
  [ "$tls_kept" -eq 0 ] || printf 'api_tls = 443\n' >> "$CONFIG"
  [ -z "$aliases_line" ] || printf 'aliases = %s\n' "$aliases_line" >> "$CONFIG"
  # Passwords and sign-ins only over HTTPS while the API has it; without
  # it, none could sign in.
  toml_put limits require_tls_for_credentials "$( [ "$tls_kept" -eq 1 ] && echo true || echo false )"
else
  sed -i -E 's|^api_server = "127\.0\.0\.1:|api_server = "0.0.0.0:|' "$CONFIG"
  toml_set service.onlineconfig listen '"0.0.0.0:80"'
  toml_set service.content listen '"0.0.0.0:8000"'
  [ -z "$aliases_line" ] || printf '\n[public]\naliases = %s\n' "$aliases_line" >> "$CONFIG"
fi
[ -z "$aliases_line" ] || say "Also reached as: ${aliases[*]}"
# Friend lists: only friends on a new (public) server; an update keeps the setting.
if [ -z "$friends" ]; then
  if [ "$first_install" -eq 1 ]; then friends=mutual; else friends="$(checked_value friends mode '^(mutual|everyone)$')"; fi
  if [ -z "$friends" ]; then
    [ "$first_install" -eq 1 ] || [ -z "$(value friends mode)" ] || warn "[friends] mode in $CONFIG is neither mutual nor everyone; set to mutual"
    friends=mutual
  fi
fi
toml_put friends mode "\"$friends\""
say "Friend lists: $friends"
if [ "$no_caddy" -eq 1 ] && [ -z "$admin" ] && [ "$(value admin enabled)" = true ]; then
  warn "turned the admin API off: without Caddy, it and its key would travel unencrypted on port 50051, open to every address"
  admin=false
fi
if [ -n "$admin" ]; then toml_put admin enabled "$admin"; fi
if [ -n "$registration" ]; then toml_put limits open_registration "$registration"; fi
# Every account linked to a player identity (the launcher finds it with the player's key),
# unless the operator turned it off.
if [ "$first_install" -eq 1 ] || [ -z "$(value limits require_identity)" ]; then toml_put limits require_identity true; fi
say "Admin API: $( [ "$(value admin enabled)" = true ] && echo "on (through an SSH tunnel only)" || echo off)"
say "New accounts: $( [ "$(value limits open_registration)" = false ] && echo closed || echo open)"
say "Accounts need a player identity: $( [ "$(value limits require_identity)" = true ] && echo yes || echo no)"
fi

# Earlier installs kept the coordinator's data in the game server's folder,
# and ran it as the game server's user: it moves to a folder of its own, for
# a user of its own (both stopped by now). Only a plain folder of plain
# files and folders moves: the game server's user could have planted links
# in it, for root to follow.
migrate_coordinator() {
  if [ ! -e "$OLD_COORD_DIR" ] && [ ! -L "$OLD_COORD_DIR" ]; then return 0; fi
  if [ -L "$OLD_COORD_DIR" ] || [ ! -d "$OLD_COORD_DIR" ]; then die "$OLD_COORD_DIR isn't a plain folder; move it away and run this again"; fi
  if [ -e "$COORD_DIR" ] || [ -L "$COORD_DIR" ]; then
    die "both $OLD_COORD_DIR (an earlier install's coordinator) and $COORD_DIR exist; move away the one that isn't in use and run this again"
  fi
  local odd
  odd="$(find "$OLD_COORD_DIR" -mindepth 1 \( -type l -o \( ! -type f ! -type d \) -o \( -type f -links +1 \) \) -print -quit)"
  [ -z "$odd" ] || die "$odd isn't a plain file or folder (a link, say); move it away and run this again"
  say "Moving the coordinator's data to $COORD_DIR (run as $COORD_USER)"
  mv -T "$OLD_COORD_DIR" "$COORD_DIR"
  # The old updater's files (it now keeps its status elsewhere).
  rm -f "$COORD_DIR/update-status.json" "$COORD_DIR/update-request" "$COORD_DIR/update-request.tmp"
  chown -R -h -P "$COORD_USER:$COORD_USER" "$COORD_DIR"
}

# A coordinator on this machine: its own service, behind Caddy on HTTPS.
if [ -n "$coord_domain" ]; then
  say "Installing the coordinator for https://$coord_domain"
  install -m 755 "$work/$COORD_ASSET" "$PROGRAM_DIR/coordinator"
  migrate_coordinator
  install -d -m 700 -o "$COORD_USER" -g "$COORD_USER" "$COORD_DIR"
  echo "$coord_domain" > "$ETC_DIR/coordinator-domain"
  if [ -n "$metrics_domain" ]; then echo "$metrics_domain" > "$ETC_DIR/metrics-domain"; else rm -f "$ETC_DIR/metrics-domain"; fi
  if [ "$origin_pull" -eq 1 ]; then touch "$ETC_DIR/metrics-origin-pull"; else rm -f "$ETC_DIR/metrics-origin-pull"; fi
  coord_args="--listen $COORD_ADDR:8700 --data $COORD_DIR"
  # The admin UI on its own port, for Caddy to serve at the metrics name.
  if [ -n "$metrics_domain" ]; then coord_args="$coord_args --admin-listen $COORD_ADDR:8701 --admin-origin https://$metrics_domain"; fi
  rm -f "$COORD_DIR/domain"
  if [ "$use_systemd" -eq 1 ]; then
    cat > "$COORD_UNIT" <<UNIT
[Unit]
Description=5th Echelon coordinator (friends across servers, server directory)
Documentation=https://github.com/$REPO
After=network-online.target
Wants=network-online.target

[Service]
User=$COORD_USER
Group=$COORD_USER
WorkingDirectory=$COORD_DIR
ExecStartPre=-+$UPDATER --verify
ExecStart=$PROGRAM_DIR/coordinator $coord_args
Restart=always
RestartSec=3
CapabilityBoundingSet=
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=$COORD_DIR
InaccessiblePaths=-$STATE_DIR
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
ProtectProc=invisible
ProcSubset=pid
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged
LockPersonality=yes
RemoveIPC=yes
UMask=0077

[Install]
WantedBy=multi-user.target
UNIT
    systemctl daemon-reload
    if [ -n "$standby" ] && ! systemctl is-active --quiet "$COORD_SERVICE"; then
      # A standby: its coordinator starts when the coordinator's record points here, and
      # never on its own (an empty database would have none of the servers' secrets).
      systemctl disable "$COORD_SERVICE" >/dev/null 2>&1 || true
      say "The coordinator here is a standby: $STANDBY_SERVICE starts it if it takes over"
    else
      # A new, empty coordinator on a server that already shares friends through one would
      # split the group (e.g. a standby that left its failover group).
      if [ -z "$standby" ] && [ ! -f "$COORD_DIR/coordinator.db" ] && [ -s "$STATE_DIR/federation.key" ] && ! systemctl is-active --quiet "$COORD_SERVICE"; then
        die "this server already shares friends through a coordinator, and none runs here: a new, empty one would split the group. In a failover group, give --standby (docs/failover.md); to only join, remove $ETC_DIR/coordinator-domain and run this again with --coordinator https://$coord_domain"
      fi
      # In a failover group, the standby starts it at boot (if the record points here).
      if [ -n "$standby" ] && [ -f "$STANDBY_ENV" ]; then
        systemctl disable "$COORD_SERVICE" >/dev/null 2>&1 || true
      else
        systemctl enable "$COORD_SERVICE" >/dev/null 2>&1
      fi
      systemctl restart "$COORD_SERVICE"
      for _ in $(seq 40); do [ -s "$COORD_DIR/join-token.txt" ] && break; sleep 0.25; done
      [ -s "$COORD_DIR/join-token.txt" ] || { journalctl -u "$COORD_SERVICE" -n 20 --no-pager >&2 || true; die "the coordinator didn't start (its log is above)"; }
    fi
  else
    ( cd "$COORD_DIR" && runuser -u "$COORD_USER" -- "$PROGRAM_DIR/coordinator" --listen "$COORD_ADDR:8700" --data "$COORD_DIR" >/dev/null 2>&1 & sleep 1; kill $! 2>/dev/null || true )
  fi
  coordinator="https://$coord_domain"
  [ ! -L "$COORD_DIR/join-token.txt" ] || die "$COORD_DIR/join-token.txt isn't a plain file"
  if [ -s "$COORD_DIR/join-token.txt" ] || [ -z "$standby" ]; then
    join_token="$(head -c 256 "$COORD_DIR/join-token.txt" | tr -d '[:space:]')"
    [[ "$join_token" =~ ^[A-Z2-7]{16,128}$ ]] || die "the coordinator's join token isn't one"
  else
    # A standby that never ran the coordinator joins it like any server.
    [ -n "$join_token" ] || [ -f "$STATE_DIR/federation.key" ] || die "this standby's server hasn't joined the coordinator yet: give --join-token (from $coordinator's machine: $0 --show-join-token)"
  fi
fi

if [ "$coord_only" -eq 0 ]; then
# [federation]: rewritten when a coordinator is given, kept otherwise.
if [ -n "$coordinator" ]; then
  # A name or region set before (by hand, or an earlier run) stays unless given again.
  server_name="${server_name:-$(checked_value federation name "$NAME_RE")}"
  region="${region:-$(checked_value federation region "$NAME_RE")}"
  if [ -z "$listed" ]; then listed="$(checked_value federation listed "$BOOL_RE")"; fi
  if [ -z "$auto_update" ]; then auto_update="$(checked_value federation auto_update "$BOOL_RE")"; fi
  sed -i '/^\[federation\]$/,/^\[/{/^\[federation\]$/d;/^\[/!d}' "$CONFIG"
  {
    printf '\n[federation]\ncoordinator = "%s"\n' "$coordinator"
    [ -z "$join_token" ] || printf 'join_token = "%s"\n' "$join_token"
    printf 'name = "%s"\n' "${server_name:-${domain:-$public_address}}"
    [ -z "$region" ] || printf 'region = "%s"\n' "$region"
    [ "$listed" != false ] || printf 'listed = false\n'
    [ "$auto_update" != false ] || printf 'auto_update = false\n'
  } >> "$CONFIG"
  say "Sharing friends through $coordinator$( [ "$listed" = false ] && echo ", not listed in the directory")"
elif [ "$listed" = false ] || [ "$listed" = true ]; then
  warn "--listed and --unlisted only matter with a coordinator; ignored"
fi
if [ -z "$coordinator" ] && [ -n "$auto_update" ] && grep -q '^\[federation\]$' "$CONFIG"; then toml_put federation auto_update "$auto_update"; fi
plain_config
chown -h "$USER_NAME:$USER_NAME" "$CONFIG"
# It can hold the join token.
chmod 600 "$CONFIG"

# The relay queues bursts of game traffic in 4 MB socket buffers; Linux
# caps them at about 208 KB unless allowed more.
cat > "$SYSCTL_FILE" <<'SYSCTL'
# 5th Echelon: room for bursts of relayed game traffic (UDP).
net.core.rmem_max = 4194304
net.core.wmem_max = 4194304
SYSCTL
if command -v sysctl >/dev/null && sysctl -q -p "$SYSCTL_FILE" >/dev/null 2>&1; then
  say "Allowed 4 MB UDP buffers for the relay"
else
  warn "couldn't raise the UDP buffer limits now (a container?); $SYSCTL_FILE applies them at the next boot"
fi

if [ "$use_systemd" -eq 1 ]; then
  say "Installing the systemd service $SERVICE"
  cat > "$UNIT" <<UNIT
[Unit]
Description=5th Echelon server (Splinter Cell: Blacklist)
Documentation=https://github.com/$REPO
After=network-online.target
Wants=network-online.target

[Service]
User=$USER_NAME
Group=$USER_NAME
WorkingDirectory=$STATE_DIR
Environment=FE_PUBLIC_ADDRESS=$public_address
# As root, before it starts: whether the program is the release installed here (never
# stops it starting; see the updater's --verify).
ExecStartPre=-+$UPDATER --verify
ExecStart=$PROGRAM_DIR/dedicated_server
Restart=always
RestartSec=3
# Port 80 without running as root.
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=$STATE_DIR $PROGRAM_DIR/data
InaccessiblePaths=-$COORD_DIR
# Not the coordinator's loopback address: only Caddy talks to it there.
IPAddressDeny=$COORD_ADDR
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
ProtectProc=invisible
# No ProcSubset=pid: the server reports the machine's memory, CPU and load to its
# coordinator from /proc/meminfo, /proc/stat and /proc/loadavg.
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged
LockPersonality=yes
RemoveIPC=yes
UMask=0077

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable "$SERVICE" >/dev/null 2>&1
  systemctl restart "$SERVICE"
  up=0
  for _ in $(seq 40); do
    if systemctl is-active --quiet "$SERVICE" && (exec 3<>/dev/tcp/127.0.0.1/50051) 2>/dev/null; then
      # Still up a moment later (not crashing and restarting).
      sleep 2
      if systemctl is-active --quiet "$SERVICE"; then up=1; break; fi
    fi
    sleep 0.5
  done
  if [ "$up" -eq 0 ]; then
    journalctl -u "$SERVICE" -n 30 --no-pager >&2 || true
    die "the server didn't come up; the log is above"
  fi
  say "The server is running"
fi
fi # the game server

# --- Updates ------------------------------------------------------------

# The root service that installs the releases a coordinator rolls out. The
# server and the coordinator run unprivileged and can't change their own
# programs: they write the version they were asked for to update-request in
# their folder, and this updater (started by a systemd path unit when one
# appears) downloads that release from GitHub, checks SHA256SUMS against the
# release key's signature (which names the version) and the binaries
# against SHA256SUMS, swaps them in and restarts the services. If they don't
# come back healthy, the previous binaries go back. It records what happened
# in $UPDATE_DIR/update-status.json (root's; the services only read it),
# which the server reports to the coordinator.
#
# Downgrades: it never installs a release older than the one running,
# except the one it replaced (a rollback, from the copy it kept), within
# ROLLBACK_DAYS of the update, and never older than $ETC_DIR/min-release:
# the release the installer last installed, raised to the one each update
# replaced. It does nothing while the installed release isn't known (a
# --binary install whose version couldn't be read).
install_updater() {
  {
    echo '#!/usr/bin/env bash'
    echo '# Written by install-server.sh: installs the release a coordinator rolls out.'
    echo 'set -euo pipefail'
    echo 'umask 022'
    printf 'REPO=%q\nPROGRAM_DIR=%q\nSTATE_DIR=%q\nCOORD_DIR=%q\nCOORD_ADDR=%q\nSERVICE=%q\nCOORD_SERVICE=%q\nSTANDBY_SERVICE=%q\nCONFIG=%q\nETC_DIR=%q\nUPDATE_DIR=%q\n' \
      "$REPO" "$PROGRAM_DIR" "$STATE_DIR" "$COORD_DIR" "$COORD_ADDR" "$SERVICE" "$COORD_SERVICE" "$STANDBY_SERVICE" "$CONFIG" "$ETC_DIR" "$UPDATE_DIR"
    printf 'RELEASE_KEY_PEM=%q\n' "$RELEASE_KEY_PEM"
    cat <<'UPDATER'
CURL=(curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --connect-timeout 20 --max-time 600 --max-filesize 268435456)
SMALL=(--max-filesize 1048576 --max-time 60)
VERSION_RE='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'
# How long after an update the release it replaced can still be asked for.
ROLLBACK_DAYS=14
# Whether SHA256SUMS in folder $1 carries the release key's signature as release $2
# (SHA256SUMS.sig beside it).
release_signed() {
  local dir="$1" version="$2" sig
  [ -s "$dir/SHA256SUMS" ] && [ -s "$dir/SHA256SUMS.sig" ] || return 1
  printf '%s\n' "$RELEASE_KEY_PEM" > "$dir/release.pem"
  { printf '5th-echelon/release/v2\n%s\n' "$version"; cat "$dir/SHA256SUMS"; } > "$dir/signed"
  sig="$(tr -d '[:space:]' < "$dir/SHA256SUMS.sig")"
  while [ $(( ${#sig} % 8 )) -ne 0 ]; do sig="$sig="; done
  printf '%s' "$sig" | base32 -d > "$dir/signature" 2>/dev/null \
    && openssl pkeyutl -verify -pubin -inkey "$dir/release.pem" -rawin -in "$dir/signed" -sigfile "$dir/signature" >/dev/null 2>&1
}
asset() { case "$1" in dedicated_server) echo dedicated_server-linux-x86_64 ;; coordinator) echo coordinator-linux-x86_64 ;; esac; }

# --verify (before the server or coordinator starts, as root): whether the programs here
# are the release installed here, byte for byte, against its SHA256SUMS as signed by the
# release key (kept from the install in $UPDATE_DIR/sums, or downloaded once). Recorded in
# $UPDATE_DIR/verify.json, which the server reports; it never stops a service starting.
if [ "${1:-}" = --verify ]; then
  install -d -m 755 "$UPDATE_DIR"
  verified() {
    local detail
    detail="$(printf '%s' "${3:-}" | tr -d '"\\\000-\037\177' | head -c 300)"
    printf '{"state":"%s","version":"%s","at":%s,"detail":"%s"}\n' "$1" "$2" "$(date +%s)" "$detail" > "$UPDATE_DIR/verify.json.new"
    chmod 644 "$UPDATE_DIR/verify.json.new" && mv -f "$UPDATE_DIR/verify.json.new" "$UPDATE_DIR/verify.json"
    echo "verify: $1 $2${3:+: $3}"
  }
  release="$(head -c 64 "$PROGRAM_DIR/release" 2>/dev/null | head -1 || true)"
  if ! [[ "$release" =~ $VERSION_RE ]]; then
    verified unverified unknown "not installed from a release (a --binary install)"
    exit 0
  fi
  check="$(mktemp -d)"
  trap 'rm -rf "$check"' EXIT
  sums="$UPDATE_DIR/sums/$release"
  if [ -s "$sums/SHA256SUMS" ] && [ -s "$sums/SHA256SUMS.sig" ]; then
    cp "$sums/SHA256SUMS" "$sums/SHA256SUMS.sig" "$check/"
  else
    base="https://github.com/$REPO/releases/download/v$release"
    if ! "${CURL[@]}" "${SMALL[@]}" --max-time 20 -o "$check/SHA256SUMS" "$base/SHA256SUMS" 2>/dev/null \
      || ! "${CURL[@]}" "${SMALL[@]}" --max-time 20 -o "$check/SHA256SUMS.sig" "$base/SHA256SUMS.sig" 2>/dev/null; then
      verified unverified "$release" "couldn't download release $release's signed SHA256SUMS to check against"
      exit 0
    fi
  fi
  if ! release_signed "$check" "$release"; then
    verified mismatch "$release" "the SHA256SUMS for $release isn't signed by the release key"
    exit 0
  fi
  install -d -m 755 "$sums" && cp "$check/SHA256SUMS" "$check/SHA256SUMS.sig" "$sums/"
  checked=() bad=()
  for p in dedicated_server coordinator; do
    [ -e "$PROGRAM_DIR/$p" ] || continue
    want="$(grep " $(asset "$p")\$" "$check/SHA256SUMS" | head -1 | cut -d' ' -f1 || true)"
    got="$(sha256sum "$PROGRAM_DIR/$p" | cut -d' ' -f1)"
    if [ -n "$want" ] && [ "$want" = "$got" ]; then checked+=("$p"); else bad+=("$p"); fi
  done
  if [ "${#bad[@]}" -gt 0 ]; then
    printf -v list '%s, ' "${bad[@]}"
    verified mismatch "$release" "${list%, } doesn't match release $release"
  elif [ "${#checked[@]}" -gt 0 ]; then
    verified verified "$release" "${checked[*]}"
  else
    verified unverified "$release" "no program to check"
  fi
  exit 0
fi
# A version that failed isn't tried again for this long (each try restarts the services).
RETRY_AFTER=3600
install -d -m 755 "$UPDATE_DIR"
exec 9>"$UPDATE_DIR/lock"
flock -n 9 || exit 0
work="$(mktemp -d)"
# Set while the services are stopped for an update: if anything fails then (set -e),
# they're started again and the failure recorded, rather than left down.
stopped=0
on_exit() {
  if [ "$stopped" -eq 1 ]; then
    restart_all || true
    status failed "${wanted:-?}" "the update stopped part way; the services were started again"
  fi
  rm -rf "$work"
}
trap on_exit EXIT
now="$(date +%s)"

current="$(head -c 64 "$PROGRAM_DIR/release" 2>/dev/null | head -1 || true)"
[[ "$current" =~ $VERSION_RE ]] || current=unknown

# A file of root's, replaced whole.
put() { printf '%s\n' "$2" > "$1.new" && chmod 644 "$1.new" && mv -f "$1.new" "$1"; }

# What happened, for the server (and coordinator) to report.
status() {
  local state="$1" version="$2" error="${3:-}"
  error="$(printf '%s' "$error" | tr -d '"\\\000-\037\177' | head -c 300)"
  put "$UPDATE_DIR/update-status.json" "$(printf '{"state":"%s","version":"%s","from":"%s","at":%s,"error":"%s"}' "$state" "$version" "$current" "$(date +%s)" "$error")"
  echo "update: $state $version${error:+: $error}"
}

# The version asked for: the start of a plain file, read without following
# a link (the services own their folders, and could swap in a link, a pipe
# or a folder), and then removed, whatever it is.
wanted=""
for f in "$STATE_DIR/update-request" "$COORD_DIR/update-request"; do
  if [ -e "$f" ] || [ -L "$f" ]; then
    v="$(dd if="$f" bs=64 count=1 iflag=nofollow,nonblock status=none 2>/dev/null | head -1 | tr -d '[:space:]' || true)"
    if [[ "$v" =~ $VERSION_RE ]]; then wanted="$v"; fi
    rm -rf -- "$f" 2>/dev/null || true
  fi
done
[ -n "$wanted" ] || exit 0
[ "$wanted" != "$current" ] || exit 0

# Not the same version again soon after it failed.
last="$(head -c 1024 "$UPDATE_DIR/update-status.json" 2>/dev/null || true)"
if [[ "$last" == *"\"version\":\"$wanted\""* ]] && [[ "$last" =~ \"state\":\"(failed|rolled-back)\" ]] && [[ "$last" =~ \"at\":([0-9]+) ]] \
  && [ $(( now - BASH_REMATCH[1] )) -lt "$RETRY_AFTER" ]; then
  exit 0
fi

if [ "$current" = unknown ]; then
  status failed "$wanted" "the release installed here isn't known (a --binary install?); run install-server.sh to install a release"
  exit 0
fi

# Release order, with pre-releases before their release (1.0.0-rc.1 < 1.0.0).
older() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "${1/-/\~}" "${2/-/\~}" | sort -V | head -1)" = "${1/-/\~}" ]; }
parts=()
[ -x "$PROGRAM_DIR/dedicated_server" ] && parts+=(dedicated_server)
[ -x "$PROGRAM_DIR/coordinator" ] && parts+=(coordinator)
[ "${#parts[@]}" -gt 0 ] || exit 0
# In a failover group, the coordinator runs on one server; the others keep its program up
# to date, but don't start it.
coord_running=0
if systemctl is-active --quiet "$COORD_SERVICE" 2>/dev/null; then coord_running=1; fi

# The oldest release this machine may run (root's; the installer sets it).
min="$(head -c 64 "$ETC_DIR/min-release" 2>/dev/null | head -1 || true)"
[[ "$min" =~ $VERSION_RE ]] || min="$current"
if older "$wanted" "$min"; then
  status failed "$wanted" "won't install a release older than $min"
  exit 0
fi
rollback=0
if older "$wanted" "$current"; then
  # Only back to the release the last update replaced, from the copy it
  # kept, and only for a while after it.
  until="$(head -c 32 "$ETC_DIR/rollback-until" 2>/dev/null | head -1 || true)"
  [[ "$until" =~ ^[0-9]+$ ]] || until=0
  if [ "$(head -c 64 "$PROGRAM_DIR/previous/release" 2>/dev/null | head -1)" != "$wanted" ]; then
    status failed "$wanted" "won't install a release older than $current (only the one before, kept here)"
    exit 0
  fi
  if [ "$now" -ge "$until" ]; then
    status failed "$wanted" "$current has run too long to go back to $wanted (a rollback is for $ROLLBACK_DAYS days after an update)"
    exit 0
  fi
  rollback=1
fi
status updating "$wanted"

# Each service's database: its folder's, as the services are set up here.
databases() {
  for p in "${parts[@]}"; do
    case "$p" in dedicated_server) echo "$STATE_DIR/5th-echelon.db" ;; coordinator) echo "$COORD_DIR/coordinator.db" ;; esac
  done
}
stop_all() {
  # The standby (failover) would start the coordinator again part way.
  systemctl stop "$STANDBY_SERVICE" 2>/dev/null || true
  for p in "${parts[@]}"; do
    case "$p" in dedicated_server) systemctl stop "$SERVICE" ;; coordinator) systemctl stop "$COORD_SERVICE" ;; esac
  done
}
restart_all() {
  for p in "${parts[@]}"; do
    case "$p" in dedicated_server) systemctl restart "$SERVICE" ;; coordinator) [ "$coord_running" -eq 0 ] || systemctl restart "$COORD_SERVICE" ;; esac
  done
  if systemctl is-enabled --quiet "$STANDBY_SERVICE" 2>/dev/null; then systemctl start "$STANDBY_SERVICE" || true; fi
}
# Copies each database (and its WAL) into $UPDATE_DIR/databases, root's
# only. Links aren't followed: the services own their folders.
save_databases() {
  local db f
  rm -rf "$UPDATE_DIR/databases.new"
  install -d -m 700 "$UPDATE_DIR/databases.new" || return 1
  while read -r db; do
    for f in "$db" "$db-wal" "$db-shm"; do
      if [ -f "$f" ] && [ ! -L "$f" ]; then
        install -d -m 700 "$UPDATE_DIR/databases.new$(dirname "$f")" && cp -p "$f" "$UPDATE_DIR/databases.new$f" || return 1
      fi
    done
  done < <(databases)
  rm -rf "$UPDATE_DIR/databases"
  mv "$UPDATE_DIR/databases.new" "$UPDATE_DIR/databases"
}
# Puts the copies back, with the services stopped.
restore_databases() {
  local db f
  [ -d "$UPDATE_DIR/databases" ] || return 0
  while read -r db; do
    [ -f "$UPDATE_DIR/databases$db" ] || continue
    for f in "$db" "$db-wal" "$db-shm"; do
      if [ -f "$UPDATE_DIR/databases$f" ]; then
        # Beside it first, then over it: the database is never missing, and a link
        # there is replaced, not followed.
        rm -f -- "$f.restoring"
        cp -p "$UPDATE_DIR/databases$f" "$f.restoring" && mv -f -- "$f.restoring" "$f" || return 1
      else
        rm -f -- "$f"
      fi
    done
  done < <(databases)
}

if [ "$rollback" -eq 0 ]; then
  base="https://github.com/$REPO/releases/download/v$wanted"
  if ! "${CURL[@]}" "${SMALL[@]}" -o "$work/SHA256SUMS" "$base/SHA256SUMS" || ! "${CURL[@]}" "${SMALL[@]}" -o "$work/SHA256SUMS.sig" "$base/SHA256SUMS.sig"; then
    status failed "$wanted" "couldn't download the release's SHA256SUMS and signature"
    exit 0
  fi
  if ! release_signed "$work" "$wanted"; then
    status failed "$wanted" "the release isn't signed by the release key as $wanted"
    exit 0
  fi
  # Kept for --verify, which checks the programs against them at each start.
  install -d -m 755 "$UPDATE_DIR/sums/$wanted" && cp "$work/SHA256SUMS" "$work/SHA256SUMS.sig" "$UPDATE_DIR/sums/$wanted/"
  for p in "${parts[@]}"; do
    a="$(asset "$p")"
    if ! "${CURL[@]}" -o "$work/$a" "$base/$a" || ! (cd "$work" && grep " $a\$" SHA256SUMS | sha256sum -c --quiet -); then
      status failed "$wanted" "$a didn't download, or doesn't match its checksum"
      exit 0
    fi
    chmod 755 "$work/$a"
  done
  # The databases as they are now, with the services stopped: the new release
  # may migrate them, and if it doesn't come back healthy they go back with
  # the release before (which may not open them migrated). Kept until the
  # next update, in $UPDATE_DIR/databases.
  stopped=1
  stop_all
  if ! save_databases; then
    restart_all || true
    stopped=0
    status failed "$wanted" "couldn't copy the databases before updating (is the disk full?)"
    exit 0
  fi
  # Keep what runs now, for a rollback.
  rm -rf "$PROGRAM_DIR/previous.new"
  install -d -m 755 "$PROGRAM_DIR/previous.new"
  for p in "${parts[@]}"; do cp -p "$PROGRAM_DIR/$p" "$PROGRAM_DIR/previous.new/$p"; done
  echo "$current" > "$PROGRAM_DIR/previous.new/release"
  # The copy kept until now stays aside until this update is healthy: if it isn't, it's
  # put back, so a failed update doesn't take away going back to it.
  rm -rf "$PROGRAM_DIR/previous.kept"
  if [ -d "$PROGRAM_DIR/previous" ]; then mv "$PROGRAM_DIR/previous" "$PROGRAM_DIR/previous.kept"; fi
  mv "$PROGRAM_DIR/previous.new" "$PROGRAM_DIR/previous"
  for p in "${parts[@]}"; do install -m 755 "$work/$(asset "$p")" "$PROGRAM_DIR/$p.new" && mv -f "$PROGRAM_DIR/$p.new" "$PROGRAM_DIR/$p"; done
else
  # Swap the kept copy and what runs now.
  install -d -m 755 "$work/now"
  for p in "${parts[@]}"; do
    [ -x "$PROGRAM_DIR/previous/$p" ] || { status failed "$wanted" "the kept copy has no $p"; exit 0; }
    cp -p "$PROGRAM_DIR/$p" "$work/now/$p"
  done
  for p in "${parts[@]}"; do install -m 755 "$PROGRAM_DIR/previous/$p" "$PROGRAM_DIR/$p.new" && mv -f "$PROGRAM_DIR/$p.new" "$PROGRAM_DIR/$p"; done
  for p in "${parts[@]}"; do cp -p "$work/now/$p" "$PROGRAM_DIR/previous/$p"; done
  echo "$current" > "$PROGRAM_DIR/previous/release"
fi
echo "$wanted" > "$PROGRAM_DIR/release"

# Restart, and wait for each to answer with the new release.
healthy() {
  local host
  for p in "${parts[@]}"; do
    case "$p" in
      dedicated_server)
        systemctl is-active --quiet "$SERVICE" || return 1
        host="$(dd if="$CONFIG" iflag=nofollow status=none 2>/dev/null | sed -n '/^\[public\]$/,/^\[/ s/^host = "\([a-z0-9.-]*\)"$/\1/p' | head -1)"
        curl -fsS --max-time 3 ${host:+-H "Host: $host"} http://127.0.0.1/api/info 2>/dev/null | grep -q "\"version\":\"$wanted\"" || return 1 ;;
      coordinator)
        [ "$coord_running" -eq 1 ] || continue
        systemctl is-active --quiet "$COORD_SERVICE" || return 1
        curl -fsS --max-time 3 "http://$COORD_ADDR:8700/v1/info" 2>/dev/null | grep -q "\"version\":\"$wanted\"" || return 1 ;;
    esac
  done
}
restart_all || true
stopped=0
ok=0
for _ in $(seq 45); do
  sleep 2
  if healthy; then ok=1; break; fi
done
if [ "$ok" -eq 1 ]; then
  if [ "$rollback" -eq 0 ]; then
    # Never below the release this one replaced; back to it only for a while.
    put "$ETC_DIR/min-release" "$current"
    put "$ETC_DIR/rollback-until" "$(( now + ROLLBACK_DAYS * 86400 ))"
  else
    put "$ETC_DIR/rollback-until" 0
  fi
  rm -rf "$PROGRAM_DIR/previous.kept"
  # Sums for this release and the one kept for a rollback; no others.
  for d in "$UPDATE_DIR"/sums/*; do
    case "${d##*/}" in "$wanted" | "$current") ;; *) rm -rf -- "$d" ;; esac
  done
  status done "$wanted"
  exit 0
fi
# Not healthy: what ran before goes back (and, after a rollback, the kept copy too).
for p in "${parts[@]}"; do
  cp -p "$PROGRAM_DIR/$p" "$work/tried-$p"
  install -m 755 "$PROGRAM_DIR/previous/$p" "$PROGRAM_DIR/$p.new" && mv -f "$PROGRAM_DIR/$p.new" "$PROGRAM_DIR/$p"
  if [ "$rollback" -eq 1 ]; then cp -p "$work/tried-$p" "$PROGRAM_DIR/previous/$p"; fi
done
if [ "$rollback" -eq 1 ]; then echo "$wanted" > "$PROGRAM_DIR/previous/release"; fi
# After an update (not a rollback), the copy kept before it goes back in place.
if [ "$rollback" -eq 0 ] && [ -d "$PROGRAM_DIR/previous.kept" ]; then
  rm -rf "$PROGRAM_DIR/previous"
  mv "$PROGRAM_DIR/previous.kept" "$PROGRAM_DIR/previous"
fi
echo "$current" > "$PROGRAM_DIR/release"
# After an update (not a rollback), the databases as they were before it.
restored=""
if [ "$rollback" -eq 0 ]; then
  if stop_all && restore_databases; then
    restored=", with the databases as they were before it"
  else
    restored="; the databases couldn't be put back (copies in $UPDATE_DIR/databases)"
  fi
fi
restart_all || true
status rolled-back "$wanted" "the new release didn't come back healthy within 90 seconds; $current is back$restored"
UPDATER
  } > "$work/update.sh"
  install -m 755 "$work/update.sh" "$UPDATER"
  install -d -m 755 "$ETC_DIR" "$UPDATE_DIR"
  # The release installed now, and the oldest one the updater may go back
  # to: this one, the operator's choice. A --binary install records the
  # version the server (or coordinator) reports, or "unknown", which the
  # updater won't update from.
  local installed="$release_version"
  if [ -z "$installed" ] && { [ -n "$binary" ] || [ -n "$coord_binary" ]; }; then
    installed="$(running_version)"
    if [[ "$installed" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
      say "The installed build reports release $installed"
    else
      installed=unknown
      warn "couldn't tell which release the installed build is; the updater won't install rollouts until a release is installed with this script"
    fi
  fi
  if [ -n "$installed" ]; then
    echo "$installed" > "$PROGRAM_DIR/release"
    if [ "$installed" != unknown ]; then
      echo "$installed" > "$ETC_DIR/min-release"
      rm -f "$ETC_DIR/rollback-until"
    fi
  fi
  [ -f "$PROGRAM_DIR/release" ] || echo unknown > "$PROGRAM_DIR/release"
  # The signed sums just checked, for the updater's --verify at each start.
  if [ -n "$release_version" ] && [ -s "$work/SHA256SUMS" ] && [ -s "$work/SHA256SUMS.sig" ]; then
    install -d -m 755 "$UPDATE_DIR/sums/$release_version"
    cp "$work/SHA256SUMS" "$work/SHA256SUMS.sig" "$UPDATE_DIR/sums/$release_version/"
  fi
  # Where earlier updaters wrote their status, in the services' folders (the
  # server reads that only when root's file isn't there).
  rm -f "$STATE_DIR/update-status.json"
  cat > "$UPDATE_SERVICE_UNIT" <<UNIT
[Unit]
Description=5th Echelon updater (installs the release the coordinator rolls out)
Documentation=https://github.com/$REPO
# However often the services ask: each run reads and removes the requests,
# and doesn't try a version that just failed again for an hour.
StartLimitIntervalSec=0

[Service]
Type=oneshot
ExecStart=$UPDATER
TimeoutStartSec=20min
# Root, but only what an update needs: its program, records and status,
# the services' requests; no home folders, devices or kernel settings.
ProtectSystem=strict
ReadWritePaths=$PROGRAM_DIR $ETC_DIR $UPDATE_DIR
ReadWritePaths=-$STATE_DIR -$COORD_DIR
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
NoNewPrivileges=yes
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
SystemCallArchitectures=native
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
UNIT
  cat > "$UPDATE_PATH_UNIT" <<UNIT
[Unit]
Description=5th Echelon updater: watches for update requests

[Path]
PathExists=$STATE_DIR/update-request
PathExists=$COORD_DIR/update-request
Unit=5th-echelon-update.service

[Install]
WantedBy=paths.target
UNIT
  systemctl daemon-reload
  systemctl enable --now 5th-echelon-update.path >/dev/null 2>&1
  say "Installed the updater: the coordinator's signed releases are installed as they're rolled out$( [ "$auto_update" = false ] && echo " (off for this server: --no-auto-update)")"
}
# Backups: the live copy of each database to R2 (Litestream, seconds behind) and a daily
# archive to B2 (30 days, plus the 1st of each month for 12). Only with $BACKUP_ENV, which
# the operator writes (docs/backups.md); its keys never go anywhere else.
install_backups() {
  if [ ! -f "$BACKUP_ENV" ]; then
    notes+=("Backups are off: write $BACKUP_ENV (R2 for the live copy, B2 for the archive; see docs/backups.md), then run this script again.")
    return 0
  fi
  [ "$(stat -c '%u %a' "$BACKUP_ENV")" = "0 600" ] || { chown root:root "$BACKUP_ENV"; chmod 600 "$BACKUP_ENV"; }
  local arch=amd64 work
  [ "$(uname -m)" = aarch64 ] && arch=arm64
  work="$(mktemp -d)"
  if [ "$(/usr/local/bin/litestream version 2>/dev/null | tr -d v)" != "$LITESTREAM_VERSION" ]; then
    local tarball want
    tarball="litestream-$LITESTREAM_VERSION-linux-$( [ "$arch" = arm64 ] && echo arm64 || echo x86_64 ).tar.gz"
    "${CURL[@]}" -fsSL --retry 3 -o "$work/$tarball" "https://github.com/benbjohnson/litestream/releases/download/v$LITESTREAM_VERSION/$tarball" \
      || die "couldn't download Litestream $LITESTREAM_VERSION"
    if [ "$arch" = arm64 ]; then want="$LITESTREAM_SHA256_arm64"; else want="$LITESTREAM_SHA256_amd64"; fi
    printf '%s  %s\n' "$want" "$work/$tarball" | sha256sum -c --quiet - || die "the Litestream download doesn't match its pinned checksum"
    tar -xzf "$work/$tarball" -C "$work" litestream
    install -m 755 "$work/litestream" /usr/local/bin/litestream
  fi
  if [ "$(/usr/local/bin/rclone version 2>/dev/null | head -1 | awk '{print $2}' | tr -d v)" != "$RCLONE_VERSION" ]; then
    local zip want
    zip="rclone-v$RCLONE_VERSION-linux-$arch.zip"
    "${CURL[@]}" -fsSL --retry 3 -o "$work/$zip" "https://github.com/rclone/rclone/releases/download/v$RCLONE_VERSION/$zip" \
      || die "couldn't download rclone $RCLONE_VERSION"
    if [ "$arch" = arm64 ]; then want="$RCLONE_SHA256_arm64"; else want="$RCLONE_SHA256_amd64"; fi
    printf '%s  %s\n' "$want" "$work/$zip" | sha256sum -c --quiet - || die "the rclone download doesn't match its pinned checksum"
    command -v unzip >/dev/null || install_packages unzip
    unzip -q -j -o "$work/$zip" "rclone-v$RCLONE_VERSION-linux-$arch/rclone" -d "$work"
    install -m 755 "$work/rclone" /usr/local/bin/rclone
  fi
  rm -rf "$work"
  install -d -m 700 "$BACKUP_STATE"
  # Each database's backup runs as its service's user (what Litestream writes beside the
  # database stays theirs), stops and starts with it (PartOf, WantedBy: the updater's
  # stop, restore and start take it along), and reads only its own folder.
  local name db user dir svc unit cfg units=()
  for name in game coordinator; do
    case "$name" in
      game) db="$STATE_DIR/5th-echelon.db"; user="$USER_NAME"; dir="$STATE_DIR"; svc="$SERVICE"; unit="5th-echelon-backup.service" ;;
      coordinator) db="$COORD_DIR/coordinator.db"; user="$COORD_USER"; dir="$COORD_DIR"; svc="$COORD_SERVICE"; unit="5th-echelon-coordinator-backup.service" ;;
    esac
    cfg="$ETC_DIR/litestream-$name.yml"
    # Litestream needs the database in WAL mode, which the services set from 0.4.2 on (the
    # header's read and write versions are 2): never switched underneath a running service.
    if [ -f "$db" ] && [ "$(od -An -tu1 -j18 -N2 "$db" | tr -s ' ')" != " 2 2" ]; then
      notes+=("The $name database isn't in WAL mode yet (a release before 0.4.2 runs here), so its live backup waits: run this script again after the update.")
      continue
    fi
    if [ ! -f "/etc/systemd/system/$svc.service" ]; then
      systemctl disable --now "$unit" 2>/dev/null || true
      rm -f "/etc/systemd/system/$unit" "$cfg"
      continue
    fi
    # No secrets here: Litestream fills in ${...} from the unit's environment ($BACKUP_ENV).
    cat > "$cfg.new" <<CFG
# Written by install-server.sh: the live copy of $db (docs/backups.md).
snapshot:
  interval: 24h
  retention: 168h
dbs:
  - path: $db
    replica:
      type: s3
      endpoint: \${BACKUP_R2_ENDPOINT}
      region: auto
      bucket: \${BACKUP_R2_BUCKET}
      path: live/\${BACKUP_NAME}/$name
      access-key-id: \${BACKUP_R2_ACCESS_KEY_ID}
      secret-access-key: \${BACKUP_R2_SECRET_ACCESS_KEY}
CFG
    chmod 644 "$cfg.new" && mv -f "$cfg.new" "$cfg"
    cat > "/etc/systemd/system/$unit.new" <<UNIT
# Written by install-server.sh: the live copy of $db to R2 (docs/backups.md).
[Unit]
Description=5th Echelon: live backup of $db
After=$svc.service network-online.target
Wants=network-online.target
PartOf=$svc.service
ConditionPathExists=$BACKUP_ENV

[Service]
User=$user
Group=$user
EnvironmentFile=$BACKUP_ENV
Environment=BACKUP_NAME=$(backup_name)
ExecStart=/usr/local/bin/litestream replicate -config $cfg
Restart=always
RestartSec=10
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=$dir
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
SystemCallArchitectures=native

[Install]
WantedBy=$svc.service
UNIT
    chmod 644 "/etc/systemd/system/$unit.new" && mv -f "/etc/systemd/system/$unit.new" "/etc/systemd/system/$unit"
    units+=("$unit")
  done
  write_backup_script
  local t
  for t in archive files; do
    cat > "/etc/systemd/system/5th-echelon-backup-$t.service" <<UNIT
# Written by install-server.sh (docs/backups.md).
[Unit]
Description=5th Echelon: $( [ "$t" = archive ] && echo "daily archive to B2" || echo "the coordinator's join token and reports to R2" )
ConditionPathExists=$BACKUP_ENV
[Service]
Type=oneshot
ExecStart=$BACKUP_SCRIPT $t
UNIT
    cat > "/etc/systemd/system/5th-echelon-backup-$t.timer" <<UNIT
# Written by install-server.sh (docs/backups.md).
[Unit]
Description=5th Echelon: $( [ "$t" = archive ] && echo "daily archive to B2" || echo "the coordinator's files to R2, hourly" )
[Timer]
OnCalendar=$( [ "$t" = archive ] && echo "*-*-* 03:30:00" || echo hourly )
RandomizedDelaySec=$( [ "$t" = archive ] && echo 30m || echo 5m )
Persistent=true
[Install]
WantedBy=timers.target
UNIT
    chmod 644 "/etc/systemd/system/5th-echelon-backup-$t".{service,timer}
  done
  systemctl daemon-reload
  for unit in "${units[@]}"; do
    systemctl enable "$unit" >/dev/null 2>&1
    systemctl restart "$unit"
  done
  systemctl enable --now 5th-echelon-backup-archive.timer >/dev/null 2>&1
  if [ -f "/etc/systemd/system/$COORD_SERVICE.service" ]; then
    systemctl enable --now 5th-echelon-backup-files.timer >/dev/null 2>&1
  else
    systemctl disable --now 5th-echelon-backup-files.timer >/dev/null 2>&1 || true
  fi
  say "Backups: live to R2 (${#units[@]} database(s), as $(backup_name)), daily archive to B2. Check: $BACKUP_SCRIPT status"
}
# What this machine's backups are kept under: BACKUP_NAME in $BACKUP_ENV, else its name in
# a failover group (where the other standbys restore the coordinator from), else the game
# server's id, else the host name.
backup_name() {
  local n
  n="$(sed -n 's/^BACKUP_NAME=//p' "$BACKUP_ENV" 2>/dev/null | tr -d "\"' " | head -1)"
  [ -n "$n" ] || n="$standby_me"
  [ -n "$n" ] || n="$(head -c 64 "$STATE_DIR/server-id.txt" 2>/dev/null | tr -cd 'a-z0-9-')"
  [ -n "$n" ] || n="$(hostname -s | tr -cd 'a-z0-9-')"
  printf '%s' "$n"
}
# The backup helper (archive, files, status, restore), root's.
write_backup_script() {
  {
    echo '#!/usr/bin/env bash'
    echo '# Written by install-server.sh: backups (docs/backups.md).'
    printf 'BACKUP_ENV=%q\nBACKUP_STATE=%q\nETC_DIR=%q\nSTATE_DIR=%q\nCOORD_DIR=%q\nSERVICE=%q\nCOORD_SERVICE=%q\nBACKUP_NAME_DEFAULT=%q\n' \
      "$BACKUP_ENV" "$BACKUP_STATE" "$ETC_DIR" "$STATE_DIR" "$COORD_DIR" "$SERVICE" "$COORD_SERVICE" "$(backup_name)"
    cat <<'BACKUP'
# Usage:
#   backup.sh status                    what's backed up, and the last archive
#   backup.sh archive                   the daily archive to B2 (its timer runs it)
#   backup.sh files                     the coordinator's join token and reports to R2 (hourly)
#   backup.sh restore game|coordinator [--time 2026-10-05T03:00:00Z] [--archive 2026-10-05|2026-10]
#                     [--from NAME]     put a copy back: the live copy (now, or as it was at
#                     [--with-files]    --time), or an archive (a day, or a month's), from this
#                                       machine's backups or another's (--from); the
#                                       coordinator's join token and reports too (--with-files,
#                                       from the live copy)
set -euo pipefail
umask 077
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
[ "$(id -u)" -eq 0 ] || die "run as root"
[ -f "$BACKUP_ENV" ] || die "no $BACKUP_ENV: backups are off (docs/backups.md)"
[ "$(stat -c '%u %a' "$BACKUP_ENV")" = "0 600" ] || die "$BACKUP_ENV must be root's, mode 600"
set -a
# shellcheck disable=SC1090
. "$BACKUP_ENV"
set +a
BACKUP_NAME="${BACKUP_NAME:-$BACKUP_NAME_DEFAULT}"
for v in BACKUP_R2_ENDPOINT BACKUP_R2_BUCKET BACKUP_R2_ACCESS_KEY_ID BACKUP_R2_SECRET_ACCESS_KEY; do
  [ -n "${!v:-}" ] || die "$v isn't set in $BACKUP_ENV"
done
has_b2() { [ -n "${BACKUP_B2_ENDPOINT:-}" ] && [ -n "${BACKUP_B2_BUCKET:-}" ] && [ -n "${BACKUP_B2_ACCESS_KEY_ID:-}" ] && [ -n "${BACKUP_B2_SECRET_ACCESS_KEY:-}" ]; }
# rclone's remotes, from the environment (nothing written to disk).
export RCLONE_CONFIG_R2_TYPE=s3 RCLONE_CONFIG_R2_PROVIDER=Cloudflare RCLONE_CONFIG_R2_ENDPOINT="$BACKUP_R2_ENDPOINT" \
  RCLONE_CONFIG_R2_ACCESS_KEY_ID="$BACKUP_R2_ACCESS_KEY_ID" RCLONE_CONFIG_R2_SECRET_ACCESS_KEY="$BACKUP_R2_SECRET_ACCESS_KEY" \
  RCLONE_CONFIG_R2_NO_CHECK_BUCKET=true RCLONE_CONFIG_R2_ACL=private
if has_b2; then
  export RCLONE_CONFIG_B2_TYPE=s3 RCLONE_CONFIG_B2_PROVIDER=Other RCLONE_CONFIG_B2_ENDPOINT="$BACKUP_B2_ENDPOINT" \
    RCLONE_CONFIG_B2_ACCESS_KEY_ID="$BACKUP_B2_ACCESS_KEY_ID" RCLONE_CONFIG_B2_SECRET_ACCESS_KEY="$BACKUP_B2_SECRET_ACCESS_KEY" \
    RCLONE_CONFIG_B2_NO_CHECK_BUCKET=true
fi
RCLONE=(/usr/local/bin/rclone --config /dev/null --retries 3 --low-level-retries 5)
database() { case "$1" in game) echo "$STATE_DIR/5th-echelon.db" ;; coordinator) echo "$COORD_DIR/coordinator.db" ;; *) die "game or coordinator, not $1" ;; esac; }
service() { case "$1" in game) echo "$SERVICE" ;; coordinator) echo "$COORD_SERVICE" ;; esac; }
here() { [ -f "/etc/systemd/system/$(service "$1").service" ]; }
# Whether it runs here: a standby coordinator (docs/failover.md) has nothing of its own to
# back up.
runs() { here "$1" && { [ "$1" != coordinator ] || systemctl is-active --quiet "$COORD_SERVICE"; }; }
# Restores the live copy kept under NAME (this machine's, or another's) to FILE, as it is
# now or as it was at TIME.
restore_live() {
  local what="$1" from="$2" out="$3" time="${4:-}" cfg
  cfg="$(mktemp)"
  cat > "$cfg" <<CFG
dbs:
  - path: $(database "$what")
    replica:
      type: s3
      endpoint: \${BACKUP_R2_ENDPOINT}
      region: auto
      bucket: \${BACKUP_R2_BUCKET}
      path: live/$from/$what
      access-key-id: \${BACKUP_R2_ACCESS_KEY_ID}
      secret-access-key: \${BACKUP_R2_SECRET_ACCESS_KEY}
CFG
  /usr/local/bin/litestream restore -config "$cfg" ${time:+-timestamp "$time"} -o "$out" "$(database "$what")"
  rm -f "$cfg"
}
# Whether FILE is a whole SQLite database (Python's check, when Python is there).
sound() {
  [ -s "$1" ] && [ "$(head -c 15 "$1")" = "SQLite format 3" ] || return 1
  command -v python3 >/dev/null || return 0
  python3 - "$1" <<'PY'
import sqlite3, sys
c = sqlite3.connect("file:" + sys.argv[1] + "?mode=ro", uri=True)
sys.exit(0 if c.execute("PRAGMA integrity_check").fetchone()[0] == "ok" else 1)
PY
}

case "${1:-status}" in
  status)
    echo "Backups of $BACKUP_NAME: live to $BACKUP_R2_BUCKET (R2)$(has_b2 && echo ", archive to $BACKUP_B2_BUCKET (B2)" || echo ", no archive (B2 isn't set)")"
    for what in game coordinator; do
      here "$what" || continue
      case "$what" in game) unit=5th-echelon-backup.service ;; coordinator) unit=5th-echelon-coordinator-backup.service ;; esac
      echo "  $what: $(systemctl is-active "$unit" 2>/dev/null || true) ($unit)"
    done
    echo "  last archive: $(cat "$BACKUP_STATE/last-archive" 2>/dev/null || echo never)"
    echo "  last files:   $(cat "$BACKUP_STATE/last-files" 2>/dev/null || echo never)"
    ;;
  archive)
    has_b2 || die "B2 isn't set in $BACKUP_ENV: no archive"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    day="$(date -u +%F)"
    month="$(date -u +%Y-%m)"
    base="B2:$BACKUP_B2_BUCKET/archive/$BACKUP_NAME"
    # From the live copy, not the database: nothing touches the running service, and each
    # day proves the live copy restores.
    for what in game coordinator; do
      runs "$what" || continue
      restore_live "$what" "$BACKUP_NAME" "$work/$what.db"
      sound "$work/$what.db" || die "the live copy of $what didn't restore whole"
      gzip -9 "$work/$what.db"
      "${RCLONE[@]}" copyto "$work/$what.db.gz" "$base/daily/$day-$what.db.gz"
      if [ "$(date -u +%d)" = 01 ]; then
        "${RCLONE[@]}" copyto "$work/$what.db.gz" "$base/monthly/$month-$what.db.gz"
      fi
    done
    files=()
    for f in join-token.txt reports; do [ -e "$COORD_DIR/$f" ] && files+=("$f"); done
    if runs coordinator && [ "${#files[@]}" -gt 0 ]; then
      tar -czf "$work/coordinator-files.tar.gz" -C "$COORD_DIR" "${files[@]}"
      "${RCLONE[@]}" copyto "$work/coordinator-files.tar.gz" "$base/daily/$day-coordinator-files.tar.gz"
      if [ "$(date -u +%d)" = 01 ]; then
        "${RCLONE[@]}" copyto "$work/coordinator-files.tar.gz" "$base/monthly/$month-coordinator-files.tar.gz"
      fi
    fi
    "${RCLONE[@]}" delete --min-age 31d "$base/daily"
    "${RCLONE[@]}" delete --min-age 366d "$base/monthly"
    date -u +%FT%TZ > "$BACKUP_STATE/last-archive"
    echo "Archived $BACKUP_NAME for $day"
    ;;
  files)
    runs coordinator || exit 0
    for f in join-token.txt; do
      [ -f "$COORD_DIR/$f" ] && "${RCLONE[@]}" copyto "$COORD_DIR/$f" "R2:$BACKUP_R2_BUCKET/live/$BACKUP_NAME/coordinator-files/$f"
    done
    if [ -d "$COORD_DIR/reports" ]; then
      "${RCLONE[@]}" sync "$COORD_DIR/reports" "R2:$BACKUP_R2_BUCKET/live/$BACKUP_NAME/coordinator-files/reports"
    fi
    date -u +%FT%TZ > "$BACKUP_STATE/last-files"
    ;;
  restore)
    what="${2:-}"; shift 2 || die "restore game|coordinator"
    time="" archive="" from="$BACKUP_NAME" with_files=0
    while [ $# -gt 0 ]; do
      case "$1" in
        --time) time="${2:?}"; shift ;;
        --archive) archive="${2:?}"; shift ;;
        --from) from="${2:?}"; shift ;;
        --with-files) with_files=1 ;;
        *) die "unknown option $1" ;;
      esac
      shift
    done
    [[ "$from" =~ ^[a-z0-9-]{1,64}$ ]] || die "--from: a backup name (letters, digits, -)"
    db="$(database "$what")"
    here "$what" || die "there's no $what service on this machine"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    if [ -n "$archive" ]; then
      has_b2 || die "B2 isn't set in $BACKUP_ENV"
      if [[ "$archive" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then kind=daily
      elif [[ "$archive" =~ ^[0-9]{4}-[0-9]{2}$ ]]; then kind=monthly
      else die "--archive: a day (2026-10-05) or a month (2026-10)"; fi
      "${RCLONE[@]}" copyto "B2:$BACKUP_B2_BUCKET/archive/$from/$kind/$archive-$what.db.gz" "$work/restored.db.gz" 2>/dev/null || true
      [ -s "$work/restored.db.gz" ] || die "there's no $kind archive $archive of $from's $what; nothing changed"
      gunzip "$work/restored.db.gz"
    else
      restore_live "$what" "$from" "$work/restored.db" "$time"
    fi
    sound "$work/restored.db" || die "the copy isn't a whole database; nothing changed"
    if [ "$with_files" -eq 1 ]; then
      [ "$what" = coordinator ] && [ -z "$archive" ] || die "--with-files is the coordinator's, from the live copy (an archive's are in its coordinator-files.tar.gz)"
      install -d -m 700 "$work/files"
      "${RCLONE[@]}" copy "R2:$BACKUP_R2_BUCKET/live/$from/coordinator-files" "$work/files"
      [ -s "$work/files/join-token.txt" ] || die "there's no join token in $from's live copy; nothing changed"
    fi
    svc="$(service "$what")"
    echo "Stopping $svc to put the copy in place…"
    systemctl stop "$svc"
    stamp="$(date -u +%Y%m%dT%H%M%SZ)-$$"
    # The database as it was, kept beside it; its WAL goes with it, never onto the copy.
    for f in "$db" "$db-wal" "$db-shm"; do
      if [ -f "$f" ]; then mv -f -- "$f" "$f.before-restore-$stamp"; fi
    done
    owner="$(stat -c %U "$(dirname "$db")")" group="$(stat -c %G "$(dirname "$db")")"
    install -m 600 -o "$owner" -g "$group" "$work/restored.db" "$db"
    if [ "$with_files" -eq 1 ]; then
      install -m 600 -o "$owner" -g "$group" "$work/files/join-token.txt" "$COORD_DIR/join-token.txt"
      if [ -d "$work/files/reports" ]; then
        rm -rf "$COORD_DIR/reports.restoring"
        cp -r "$work/files/reports" "$COORD_DIR/reports.restoring"
        chown -R "$owner:$group" "$COORD_DIR/reports.restoring"
        chmod -R go-rwx "$COORD_DIR/reports.restoring"
        if [ -d "$COORD_DIR/reports" ]; then mv -f -- "$COORD_DIR/reports" "$COORD_DIR/reports.before-restore-$stamp"; fi
        mv "$COORD_DIR/reports.restoring" "$COORD_DIR/reports"
      fi
    fi
    systemctl start "$svc"
    echo "Restored $what from $( [ -n "$archive" ] && echo "the $kind archive $archive" || echo "the live copy${time:+ as at $time}" ) of $from$( [ "$with_files" -eq 1 ] && echo ", with its join token and reports")."
    echo "The one it replaced: $db.before-restore-$stamp"
    ;;
  *) die "status, archive, files or restore (see the top of $0)" ;;
esac
BACKUP
  } > "$BACKUP_SCRIPT.new"
  chmod 700 "$BACKUP_SCRIPT.new"
  mv -f "$BACKUP_SCRIPT.new" "$BACKUP_SCRIPT"
}
# The version the installed server (or, alone, the coordinator) reports.
running_version() {
  local info
  if [ "$coord_only" -eq 0 ]; then
    info="$(curl -fsS --max-time 3 ${domain:+-H "Host: $domain"} "http://127.0.0.1:$( [ "$no_caddy" -eq 1 ] && echo 80 || echo 8080 )/api/info" 2>/dev/null || true)"
  else
    info="$(curl -fsS --max-time 3 "http://$COORD_ADDR:8700/v1/info" 2>/dev/null || true)"
  fi
  printf '%s' "$info" | grep -o '"version":"[^"]*"' | head -1 | cut -d'"' -f4
}
if [ "$use_systemd" -eq 1 ]; then install_updater; fi
if [ "$use_systemd" -eq 1 ]; then install_backups; fi

# A failover group's standby (docs/failover.md): the coordinator binary, as root, watching
# the coordinator's record through Cloudflare's API.
install_standby() {
  [ -n "$standby" ] || return 0
  if [ "$(backup_name)" != "$standby_me" ]; then
    warn "BACKUP_NAME in $BACKUP_ENV is $(backup_name), not $standby_me: the other standbys restore the coordinator from live/$standby_me. Remove BACKUP_NAME (or make it $standby_me)"
  fi
  printf '%s\n' "$standby" > "$STANDBY_LIST"
  {
    echo "# Written by install-server.sh: this server's failover group (docs/failover.md)."
    echo "# The coordinator's record first; the records that move with it after."
    echo "record $coord_domain"
    [ -z "$metrics_domain" ] || echo "record $metrics_domain"
    echo "me $standby_me"
    echo "address $public_address"
    echo "# The group, in the order they take over: backup name, then game server."
    for entry in "${standby_entries[@]}"; do echo "server ${entry%%=*} ${entry#*=}"; done
    echo "secrets $STANDBY_ENV"
    echo "service $COORD_SERVICE"
    echo "local http://$COORD_ADDR:8700"
    echo "database $COORD_DIR/coordinator.db"
    echo "backup $BACKUP_SCRIPT"
  } > "$STANDBY_CONF.new"
  chmod 644 "$STANDBY_CONF.new" && mv -f "$STANDBY_CONF.new" "$STANDBY_CONF"
  cat > "$STANDBY_UNIT.new" <<UNIT
# Written by install-server.sh (docs/failover.md).
[Unit]
Description=5th Echelon: standby coordinator (runs it here when its record points here; takes over if it's down)
Documentation=https://github.com/$REPO/blob/main/docs/failover.md
After=network-online.target
Wants=network-online.target
ConditionPathExists=$STANDBY_ENV

[Service]
ExecStart=$PROGRAM_DIR/coordinator standby --config $STANDBY_CONF
Restart=always
RestartSec=10
StateDirectory=5th-echelon-standby
StateDirectoryMode=0700
NoNewPrivileges=yes
ProtectSystem=full
ProtectHome=yes
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
LockPersonality=yes

[Install]
WantedBy=multi-user.target
UNIT
  chmod 644 "$STANDBY_UNIT.new" && mv -f "$STANDBY_UNIT.new" "$STANDBY_UNIT"
  systemctl daemon-reload
  systemctl enable "$STANDBY_SERVICE" >/dev/null 2>&1
  [ -f "$BACKUP_ENV" ] || notes+=("Failover needs backups: the standbys restore the coordinator from its live backup. Write $BACKUP_ENV (docs/backups.md) and run this again.")
  if [ -f "$STANDBY_ENV" ]; then
    [ "$(stat -c '%u %a' "$STANDBY_ENV")" = "0 600" ] || die "$STANDBY_ENV must be root's, mode 600"
    systemctl restart "$STANDBY_SERVICE"
    say "Standby coordinator: $standby_me, in a group of$standby_names(in that order). Check: $PROGRAM_DIR/coordinator standby --status"
  else
    notes+=("The standby waits for $STANDBY_ENV (Cloudflare's API token and zone; docs/failover.md). Until then nothing takes over, and the coordinator runs where it runs now.")
  fi
}
if [ "$use_systemd" -eq 1 ]; then install_standby; fi

# --- Firewall -----------------------------------------------------------

# Before Caddy: its certificate check needs ports 80 and 443 reachable.

if [ "$coord_only" -eq 1 ]; then tcp_ports=(80); elif [ "$no_caddy" -eq 0 ]; then tcp_ports=(80); else tcp_ports=(80 8000 50051); fi
if [ -n "$coord_domain" ] || { [ "$no_caddy" -eq 0 ] && [ "$https_api" -eq 1 ]; }; then tcp_ports+=(443); fi
opened=""
if [ "$firewall" -eq 1 ]; then
  # Each rule is recorded, so --uninstall removes what it added (and only that).
  record_rule() { grep -qxF "$1 $2" "$FIREWALL_RECORD" 2>/dev/null || echo "$1 $2" >> "$FIREWALL_RECORD"; }
  if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q "Status: active"; then
    for p in "${tcp_ports[@]}"; do ufw allow "$p/tcp" >/dev/null; record_rule ufw "$p/tcp"; done
    if [ "$coord_only" -eq 0 ]; then
      ufw allow "${UDP_PORTS/-/:}/udp" >/dev/null
      record_rule ufw "${UDP_PORTS/-/:}/udp"
    fi
    opened="ufw"
  elif command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
    for p in "${tcp_ports[@]}"; do firewall-cmd --permanent --add-port="$p/tcp" >/dev/null; record_rule firewalld "$p/tcp"; done
    if [ "$coord_only" -eq 0 ]; then
      firewall-cmd --permanent --add-port="$UDP_PORTS/udp" >/dev/null
      record_rule firewalld "$UDP_PORTS/udp"
    fi
    firewall-cmd --reload >/dev/null
    opened="firewalld"
  fi
fi

# --- Caddy --------------------------------------------------------------

install_caddy() {
  if command -v caddy >/dev/null; then return 0; fi
  say "Installing Caddy"
  case "$family" in
    debian)
      install_packages gnupg debian-keyring debian-archive-keyring apt-transport-https 2>/dev/null || install_packages gnupg
      "${CURL[@]}" -1sLf https://dl.cloudsmith.io/public/caddy/stable/gpg.key | gpg --batch --yes --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
      "${CURL[@]}" -1sLf https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt > /etc/apt/sources.list.d/caddy-stable.list
      if ! install_packages caddy; then
        # A repository apt can't verify (its key expired, say) would break every
        # later apt update on this machine: take it away again.
        warn "Caddy's package repository didn't work; removing it, and using Caddy's own build instead"
        rm -f /etc/apt/sources.list.d/caddy-stable.list /usr/share/keyrings/caddy-stable-archive-keyring.gpg
        DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=600 update -qq >/dev/null 2>&1 || true
        return 1
      fi ;;
    fedora)
      if [ "$rhel_like" -eq 1 ]; then
        install_packages 'dnf-command(copr)' && dnf copr enable -y -q @caddy/caddy >/dev/null
      fi
      install_packages caddy ;;
    arch) install_packages caddy ;;
  esac
}

# Caddy's own build, for systems without a package.
install_caddy_static() {
  say "Installing Caddy (the official static build)"
  local caddy_arch=amd64 want
  [ "$(uname -m)" = aarch64 ] && caddy_arch=arm64
  local tarball="caddy_${CADDY_VERSION}_linux_${caddy_arch}.tar.gz"
  "${CURL[@]}" -fsSL --retry 3 -o "$work/$tarball" "https://github.com/caddyserver/caddy/releases/download/v$CADDY_VERSION/$tarball" \
    || die "couldn't download Caddy $CADDY_VERSION"
  if [ "$caddy_arch" = arm64 ]; then want="$CADDY_SHA512_arm64"; else want="$CADDY_SHA512_amd64"; fi
  printf '%s  %s\n' "$want" "$work/$tarball" | sha512sum -c --quiet - || die "the Caddy download doesn't match its pinned checksum"
  tar -xzf "$work/$tarball" -C "$work" caddy
  install -m 755 "$work/caddy" /usr/bin/caddy
  # This script's own Caddy: later runs keep it up to date (see caddy_is_ours).
  install -d -m 755 "$ETC_DIR"
  touch "$ETC_DIR/caddy-static"
  if ! id caddy >/dev/null 2>&1; then
    useradd --system --home-dir /var/lib/caddy --create-home --shell /usr/sbin/nologin caddy 2>/dev/null \
      || useradd --system --home-dir /var/lib/caddy --create-home --shell /sbin/nologin caddy
  fi
  install -d -m 755 /etc/caddy
  cat > /etc/systemd/system/caddy.service <<'UNIT'
[Unit]
Description=Caddy
Documentation=https://caddyserver.com/docs/
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
User=caddy
Group=caddy
ExecStart=/usr/bin/caddy run --environ --config /etc/caddy/Caddyfile
ExecReload=/usr/bin/caddy reload --config /etc/caddy/Caddyfile --force
TimeoutStopSec=5s
LimitNOFILE=1048576
PrivateTmp=true
ProtectSystem=full
AmbientCapabilities=CAP_NET_ADMIN CAP_NET_BIND_SERVICE

[Install]
WantedBy=multi-user.target
UNIT
  if [ "$use_systemd" -eq 1 ]; then systemctl daemon-reload; fi
}

# Restarts the server after a change to its settings, and waits for it.
restart_server() {
  systemctl restart "$SERVICE"
  # Back up before anything below talks to it.
  for _ in $(seq 40); do
    if (exec 3<>/dev/tcp/127.0.0.1/50051) 2>/dev/null; then break; fi
    sleep 0.5
  done
}

# The site's routes, for http:// and https:// alike. The admin API
# (accounts and games) is never served to the internet: manage the server
# on the machine itself (or through an SSH tunnel to 127.0.0.1:50051).
site_routes() {
  cat <<'SITE'
	# The API takes messages up to 8 MB: a player's report with its logs is
	# the largest (6 MB of them, compressed). Nothing else is near it.
	request_body {
		max_size 8MB
	}
	@admin path /users.UsersAdmin/* /games.GamesAdmin/*
	handle @admin {
		respond 403
	}
	@grpc header Content-Type application/grpc*
	handle @grpc {
		reverse_proxy h2c://127.0.0.1:50051 {
			flush_interval -1
		}
	}
	handle /mp_balancing.ini {
		reverse_proxy 127.0.0.1:8000
	}
	handle {
		reverse_proxy 127.0.0.1:8080
	}
SITE
}

site_block() {
  if [ -n "$domain" ]; then
    # The domain, and any aliases that are names (an IP can't have a certificate).
    local names=("$domain") a http_sites https_sites
    for a in "${aliases[@]}"; do [[ "$a" =~ $IPV4_RE ]] || names+=("$a"); done
    http_sites="$(printf 'http://%s, ' "${names[@]}" | sed 's/, $//')"
    https_sites="$(printf 'https://%s, ' "${names[@]}" | sed 's/, $//')"
    echo "# 5th Echelon: the game's online config and community API, its content,"
    echo "# and the launcher's API (gRPC). http:// on purpose: the game can only"
    echo "# speak plain HTTP on port 80, so this name must never redirect to HTTPS."
    echo "$http_sites {"
    site_routes
    echo "}"
    if [ "$https_api" -eq 1 ]; then
      echo
      echo "# The same over HTTPS (Caddy gets the certificate): launchers that"
      echo "# see api_tls in /api/info use this, so nothing travels readable."
      echo "$https_sites {"
      site_routes
      echo "}"
    fi
  fi
  if [ -n "$coord_domain" ] && [ -f "$COORD_CERT" ]; then
    cat <<SITE

# The 5th Echelon coordinator: friends across servers and the server
# directory, only through Cloudflare (the record proxied, so it can move
# between the servers of a failover group at once; docs/failover.md). Any
# other address is refused but this machine's, and the client's address is
# Cloudflare's CF-Connecting-IP.
$coord_domain {
	tls $COORD_CERT $COORD_KEY
	@direct not remote_ip $(cloudflare_ranges) 127.0.0.0/8 ::1
	abort @direct
	reverse_proxy $COORD_ADDR:8700 {
		header_up X-Forwarded-For {http.request.header.CF-Connecting-IP}
	}
}
SITE
  elif [ -n "$coord_domain" ]; then
    cat <<SITE

# The 5th Echelon coordinator: friends across servers and the server
# directory, over HTTPS (Caddy gets the certificate).
$coord_domain {
	reverse_proxy $COORD_ADDR:8700
}
SITE
  fi
  if [ -n "$metrics_domain" ]; then
    cat <<SITE

# The coordinator's admin UI, only through Cloudflare (the record proxied):
# any other address is refused, and the client's address and country are
# Cloudflare's. (Any Cloudflare account can proxy to this address, past
# this zone's rules: the UI's own sign-in still applies.)
$metrics_domain {
$(metrics_tls)
	@direct not remote_ip $(cloudflare_ranges)
	abort @direct
	reverse_proxy $COORD_ADDR:8701 {
		header_up X-Admin-Client-IP {http.request.header.CF-Connecting-IP}
		header_up X-Admin-Country {http.request.header.CF-IPCountry}
	}
}
SITE
  fi
}

# Caddy in a sandbox, like the server's own units: read-only system, its own
# data folder, no new privileges, only network system calls it needs, and the
# admin socket in /run/caddy (root and Caddy only). The package's unit is
# left alone; this adds to it.
caddy_unit_changed=0
harden_caddy() {
  local caddy_home want
  caddy_home="$(getent passwd caddy | cut -d: -f6)"
  caddy_home="${caddy_home:-/var/lib/caddy}"
  want="$(cat <<UNIT
# Written by install-server.sh: Caddy in a sandbox.
[Service]
# Started again if it stops on its own (a crash took the sites down once).
Restart=on-failure
RestartSec=2s
RuntimeDirectory=caddy
RuntimeDirectoryMode=0750
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=$caddy_home
ReadWritePaths=-/var/log/caddy
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
ProtectProc=invisible
ProcSubset=pid
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged
CapabilityBoundingSet=CAP_NET_BIND_SERVICE CAP_NET_ADMIN
UNIT
)"
  if [ "$(cat "$CADDY_DROPIN" 2>/dev/null)" != "$want" ]; then
    install -d -m 755 "$(dirname "$CADDY_DROPIN")"
    printf '%s\n' "$want" > "$CADDY_DROPIN"
    chmod 644 "$CADDY_DROPIN"
    systemctl daemon-reload
    caddy_unit_changed=1
  fi
  # The admin API moves to the socket: a reload can't reach it at the old address.
  if ss -Hltn 'sport = :2019' 2>/dev/null | grep -q .; then caddy_unit_changed=1; fi
}

# Cloudflare's address ranges (fetched once per run; the list below if that fails).
CF_FALLBACK="173.245.48.0/20 103.21.244.0/22 103.22.200.0/22 103.31.4.0/22 141.101.64.0/18 108.162.192.0/18 190.93.240.0/20 188.114.96.0/20 197.234.240.0/22 198.41.128.0/17 162.158.0.0/15 104.16.0.0/13 104.24.0.0/14 172.64.0.0/13 131.0.72.0/22 2400:cb00::/32 2606:4700::/32 2803:f800::/32 2405:b500::/32 2405:8100::/32 2a06:98c0::/29 2c0f:f248::/32"
cloudflare_ranges() {
  if [ -z "${CF_RANGES:-}" ]; then
    local fetched
    fetched="$({ "${CURL[@]}" -fsSL --max-time 10 https://www.cloudflare.com/ips-v4; echo; "${CURL[@]}" -fsSL --max-time 10 https://www.cloudflare.com/ips-v6; } 2>/dev/null \
      | grep -E '^([0-9]{1,3}(\.[0-9]{1,3}){3}|[0-9a-f:]+:[0-9a-f:]*)/[0-9]{1,3}$' | tr '\n' ' ' || true)"
    if [ "$(printf '%s' "$fetched" | wc -w)" -ge 10 ]; then CF_RANGES="${fetched% }"; else CF_RANGES="$CF_FALLBACK"; fi
  fi
  printf '%s' "$CF_RANGES"
}

# With --cloudflare-origin-pull, TLS needs Cloudflare's client certificate.
# With an Origin CA certificate, Caddy uses it instead of asking Let's Encrypt.
metrics_tls() {
  [ "$origin_pull" -eq 1 ] || [ -f "$METRICS_CERT" ] || return 0
  printf '\ttls'
  [ ! -f "$METRICS_CERT" ] || printf ' %s %s' "$METRICS_CERT" "$METRICS_KEY"
  printf ' {\n'
  [ "$origin_pull" -eq 0 ] || printf '\t\tclient_auth {\n\t\t\tmode require_and_verify\n\t\t\ttrust_pool file %s\n\t\t}\n' "$ORIGIN_PULL_CA"
  printf '\t}\n'
}
# How long Caddy waits for a request's headers and body, and keeps an idle
# connection (for every site on the port). A report's logs, a few MB, can take
# well over 30 s from the other side of the world (NZ to Canada did, and the
# report failed); headers must still come at once.
caddy_timeouts() {
  printf '\t\ttimeouts {\n\t\t\tread_header 10s\n\t\t\tread_body 3m\n\t\t\tidle 3m\n\t\t}\n'
}
METRICS_CERT="/etc/caddy/5th-echelon-metrics.crt"
METRICS_KEY="/etc/caddy/5th-echelon-metrics.key"
ORIGIN_PULL_CA="/etc/caddy/cloudflare-origin-pull-ca.pem"

if [ "$no_caddy" -eq 0 ]; then
  install_caddy || true
  command -v caddy >/dev/null || install_caddy_static
  # A packaged Caddy updates with the system; Caddy's own build that this script put in
  # place doesn't, so an older one than this script's is replaced. Anyone else's (built
  # with plugins, say) is left alone.
  caddy_is_ours() {
    [ "$(command -v caddy)" = /usr/bin/caddy ] || return 1
    { dpkg -S /usr/bin/caddy || rpm -qf /usr/bin/caddy || pacman -Qo /usr/bin/caddy; } >/dev/null 2>&1 && return 1
    [ -f "$ETC_DIR/caddy-static" ] || grep -qxF 'ExecStart=/usr/bin/caddy run --environ --config /etc/caddy/Caddyfile' /etc/systemd/system/caddy.service 2>/dev/null
  }
  if caddy_is_ours; then
    have="$(caddy version 2>/dev/null | cut -d' ' -f1 | tr -d v)"
    if [[ "$have" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && [ "$have" != "$CADDY_VERSION" ] && [ "$(printf '%s\n%s\n' "$have" "$CADDY_VERSION" | sort -V | head -1)" = "$have" ]; then
      say "Updating Caddy $have to $CADDY_VERSION"
      install_caddy_static
      caddy_unit_changed=1
    fi
  fi
  caddy version >/dev/null 2>&1 || die "Caddy was installed but doesn't run ($(command -v caddy)); install Caddy 2.6+ yourself and run this again"
  caddy_version="$(caddy version | cut -d' ' -f1)"
  case "$caddy_version" in
    v2.[0-5].*) die "Caddy $caddy_version is too old (2.6 or newer speaks the launcher's gRPC); update it and run this again" ;;
  esac
  install -d -m 755 /etc/caddy
  if [ -n "$metrics_cert" ]; then
    openssl x509 -noout -in "$metrics_cert" 2>/dev/null || die "$metrics_cert isn't a certificate (PEM)"
    openssl pkey -noout -in "$metrics_key" 2>/dev/null || die "$metrics_key isn't a private key (PEM)"
    [ "$(openssl x509 -noout -pubkey -in "$metrics_cert" | openssl sha256)" = "$(openssl pkey -pubout -in "$metrics_key" | openssl sha256)" ] \
      || die "$metrics_key isn't the key of $metrics_cert"
    openssl x509 -noout -checkhost "$metrics_domain" -in "$metrics_cert" | grep -q 'does match' || die "$metrics_cert isn't for $metrics_domain"
    install -m 644 "$metrics_cert" "$METRICS_CERT"
    # Caddy reads the key; nobody else may.
    install -m 640 -o root -g "$(id -gn caddy 2>/dev/null || echo root)" "$metrics_key" "$METRICS_KEY"
    say "Using the Origin CA certificate for $metrics_domain"
  fi
  if [ -n "$coord_cert" ]; then
    openssl x509 -noout -in "$coord_cert" 2>/dev/null || die "$coord_cert isn't a certificate (PEM)"
    openssl pkey -noout -in "$coord_key" 2>/dev/null || die "$coord_key isn't a private key (PEM)"
    [ "$(openssl x509 -noout -pubkey -in "$coord_cert" | openssl sha256)" = "$(openssl pkey -pubout -in "$coord_key" | openssl sha256)" ] \
      || die "$coord_key isn't the key of $coord_cert"
    openssl x509 -noout -checkhost "$coord_domain" -in "$coord_cert" | grep -q 'does match' || die "$coord_cert isn't for $coord_domain"
    install -m 644 "$coord_cert" "$COORD_CERT"
    install -m 640 -o root -g "$(id -gn caddy 2>/dev/null || echo root)" "$coord_key" "$COORD_KEY"
    say "Using the Origin CA certificate for $coord_domain"
  fi
  if [ "$origin_pull" -eq 1 ]; then
    case "$caddy_version" in v2.[6-7].*) die "--cloudflare-origin-pull needs Caddy 2.8 or newer (this is $caddy_version)" ;; esac
    "${CURL[@]}" -fsSL --retry 3 -o "$work/origin-pull-ca.pem" https://developers.cloudflare.com/ssl/static/authenticated_origin_pull_ca.pem \
      || die "couldn't download Cloudflare's origin pull certificate"
    openssl x509 -noout -in "$work/origin-pull-ca.pem" 2>/dev/null || die "Cloudflare's origin pull certificate isn't a certificate"
    install -m 644 "$work/origin-pull-ca.pem" "$ORIGIN_PULL_CA"
  fi
  # The installer's block: the global option for the launcher's API and
  # the site, between markers so updates leave everything else alone.
  managed="$work/managed.caddy"
  {
    echo "$BEGIN_MARK"
    echo "{"
    echo "	# Caddy's admin API on a socket only root and Caddy can open, not on"
    echo "	# localhost:2019, where any local process could rewrite the config."
    echo "	admin unix//run/caddy/admin.sock"
    echo "	# The launcher and overlay speak gRPC without TLS (h2c). Requests"
    echo "	# must arrive in time: slow ones can't hold the server's connections."
    echo "	servers :80 {"
    echo "		protocols h1 h2 h2c"
    caddy_timeouts
    echo "	}"
    if [ -n "$coord_domain$metrics_domain" ] || { [ -n "$domain" ] && [ "$https_api" -eq 1 ]; }; then
      echo "	servers :443 {"
      caddy_timeouts
      echo "	}"
    fi
    echo "}"
    echo
    site_block
    echo "$END_MARK"
  } > "$managed"
  if grep -qxF "$BEGIN_MARK" "$CADDYFILE" 2>/dev/null; then
    replace_managed "$CADDYFILE" "$managed"
    say "Updated the 5th Echelon block in $CADDYFILE"
  elif [ ! -s "$CADDYFILE" ] || ! grep -vE '^[[:space:]]*(#|$)' "$CADDYFILE" | grep -qvE '^[[:space:]]*(:80( \{)?|root \* /usr/share/caddy|file_server|\{|\})[[:space:]]*$'; then
    # Empty, or the package's example site: replaced (and kept aside).
    if [ -s "$CADDYFILE" ]; then cp "$CADDYFILE" "$CADDYFILE.before-5th-echelon"; fi
    { cat "$managed"; echo; echo "# Other sites can go here, below the block."; } > "$CADDYFILE"
    say "Wrote $CADDYFILE"
  else
    # Someone else's Caddyfile: the site goes in its own file, imported.
    site_block > "$CADDY_SITE"
    grep -qxF "import $CADDY_SITE" "$CADDYFILE" || printf '\nimport %s\n' "$CADDY_SITE" >> "$CADDYFILE"
    say "Added the site as $CADDY_SITE, imported by your $CADDYFILE"
  fi
  # Caddy runs as its own user and must be able to read them (an existing file
  # keeps its mode when rewritten).
  chmod 644 "$CADDYFILE"
  if [ -f "$CADDY_SITE" ]; then chmod 644 "$CADDY_SITE"; fi
  if ! caddy validate --config "$CADDYFILE" --adapter caddyfile >/dev/null 2>&1; then
    caddy validate --config "$CADDYFILE" --adapter caddyfile >&2 || true
    die "Caddy doesn't accept $CADDYFILE (the error is above)"
  fi
  # SELinux: let Caddy connect to the server on this machine.
  if command -v getenforce >/dev/null && [ "$(getenforce 2>/dev/null)" = Enforcing ] && command -v setsebool >/dev/null; then
    setsebool -P httpd_can_network_connect 1 || true
  fi
  if [ "$use_systemd" -eq 1 ]; then
    harden_caddy
    systemctl enable caddy >/dev/null 2>&1
    # A reload goes through the admin API; when its address changed, or the
    # sandbox did, Caddy needs a restart instead.
    if [ "$caddy_unit_changed" -eq 1 ] || ! systemctl reload caddy 2>/dev/null; then systemctl restart caddy; fi
  fi
  if [ "$use_systemd" -eq 1 ] && [ -n "$coord_domain" ] && [ -n "$standby" ] && ! systemctl is-active --quiet "$COORD_SERVICE"; then
    say "Caddy serves $coord_domain here too, for when this standby takes over"
  elif [ "$use_systemd" -eq 1 ] && [ -n "$coord_domain" ]; then
    coord_ok=0
    # An Origin CA certificate is only trusted by Cloudflare: not checked here.
    coord_insecure=""; [ ! -f "$COORD_CERT" ] || coord_insecure=-k
    for _ in $(seq 60); do
      if curl -fsS $coord_insecure --max-time 3 --resolve "$coord_domain:443:127.0.0.1" "https://$coord_domain/v1/info" >/dev/null 2>&1; then coord_ok=1; break; fi
      sleep 1
    done
    if [ "$coord_ok" -eq 1 ]; then
      say "The coordinator answers at https://$coord_domain"
    else
      if [ -f "$COORD_CERT" ]; then
        notes+=("The coordinator doesn't answer at https://$coord_domain on this machine; see journalctl -u $COORD_SERVICE -u caddy")
      else
        notes+=("Caddy has no certificate for $coord_domain yet, so other servers can't reach the coordinator. Check its A record (DNS only, not proxied) and that TCP 80 and 443 are open, then run this script again.")
      fi
    fi
  fi
  if [ "$use_systemd" -eq 1 ] && [ -n "$metrics_domain" ]; then
    if curl -fsS --max-time 3 -o /dev/null "http://$COORD_ADDR:8701/" 2>/dev/null; then
      say "The admin UI is served at https://$metrics_domain (through Cloudflare only)"
      notes+=("In Cloudflare: make $metrics_domain's A record proxied (orange cloud) and set SSL/TLS to Full (strict). Then add yourself: $0 --add-admin YOURNAME")
    else
      notes+=("The coordinator's admin UI doesn't answer on $COORD_ADDR:8701; see journalctl -u $COORD_SERVICE")
    fi
  fi
  if [ "$use_systemd" -eq 1 ] && [ -n "$domain" ]; then
    ok=0
    for _ in $(seq 30); do
      if curl -fsS --max-time 2 -H "Host: $domain" http://127.0.0.1/api/info 2>/dev/null | grep -q '"api":80'; then ok=1; break; fi
      sleep 0.5
    done
    if [ "$ok" -eq 0 ]; then
      journalctl -u caddy -n 20 --no-pager >&2 || true
      die "Caddy doesn't pass requests on to the server (its log is above)"
    fi
    say "Caddy serves $domain"
    # The API over HTTPS, once Caddy has its certificate: then launchers are told to use it.
    if [ "$https_api" -eq 1 ]; then
      tls_ok=0
      for _ in $(seq 60); do
        if curl -fsS --max-time 3 --resolve "$domain:443:127.0.0.1" "https://$domain/api/info" >/dev/null 2>&1; then tls_ok=1; break; fi
        sleep 1
      done
      if [ "$tls_ok" -eq 1 ] && [ "$tls_kept" -eq 0 ]; then
        plain_config
        sed -i '/^\[public\]$/,/^\[/ { /^api_tls = /d; s/^content = 80$/content = 80\napi_tls = 443/ }' "$CONFIG"
        toml_put limits require_tls_for_credentials true
        restart_server
      elif [ "$tls_ok" -eq 0 ] && [ "$tls_kept" -eq 1 ]; then
        plain_config
        sed -i '/^\[public\]$/,/^\[/ { /^api_tls = /d }' "$CONFIG"
        toml_put limits require_tls_for_credentials false
        restart_server
      fi
      if [ "$tls_ok" -eq 1 ]; then
        say "The launcher's API is served over HTTPS too (https://$domain); passwords and sign-ins only over HTTPS"
      else
        journalctl -u caddy -n 20 --no-pager >&2 || true
        notes+=("Caddy has no certificate for $domain yet (its log is above), so launchers keep using the unencrypted API (and passwords travel readable). Check the A record and that TCP 80 and 443 are open, then run this script again.")
      fi
    fi
    # The launcher's API: gRPC over plain HTTP/2 (h2c) through Caddy.
    grpc_ok=0
    for _ in $(seq 10); do
      if curl -fsS --max-time 3 --http2-prior-knowledge -o /dev/null -X POST -H "Host: $domain" -H "Content-Type: application/grpc" \
        http://127.0.0.1/users.Users/Login 2>/dev/null; then grpc_ok=1; break; fi
      sleep 1
    done
    if [ "$grpc_ok" -eq 0 ]; then
      notes+=("Caddy doesn't pass the launcher's API (gRPC without TLS) through. Add this to the global options block (the { } at the top) of $CADDYFILE, then run 'systemctl reload caddy':
      servers :80 {
          protocols h1 h2 h2c
      }")
    fi
  fi
fi

# --- Summary ------------------------------------------------------------

if [ "$coord_only" -eq 1 ]; then
  echo
  echo "$( [ "$updating" -eq 1 ] && echo Updated || echo Installed ) the 5th Echelon coordinator (no game server on this machine)."
  echo
  echo "  Coordinator:    https://$coord_domain   (log: journalctl -u $COORD_SERVICE -f)"
  echo "  Status:         $0 --status"
  echo "  Join token:     $0 --show-join-token   (new one: --rotate-join-token)"
  echo "  Update:         run this script again.   Remove: $0 --uninstall"
  echo
  echo "  Open TCP 80 and 443 on every firewall in front of it (and keep SSH, TCP 22)."
  if [ -n "$opened" ]; then echo "  This machine's $opened now allows them."; fi
  echo
  echo "  Servers join it with:"
  echo "    --coordinator https://$coord_domain --join-token-file token.txt"
  for note in "${notes[@]}"; do
    echo
    warn "$note"
  done
  exit 0
fi

info_host=()
if [ -n "$domain" ]; then info_host=(-H "Host: $domain"); fi
version_line="$(curl -fsS --max-time 3 "${info_host[@]}" http://127.0.0.1/api/info 2>/dev/null | grep -o '"version":"[^"]*"' | cut -d'"' -f4 || true)"
address="${domain:-$public_address}"
action="Installed"
if [ "$updating" -eq 1 ]; then action="Updated"; fi

echo
echo "$action the 5th Echelon server${version_line:+ $version_line}."
echo
echo "  Players join:   $address   (launcher: Join a server)"
echo "  Settings:       $CONFIG   (then: systemctl restart $SERVICE)"
echo "  Server log:     journalctl -u $SERVICE -f"
if [ "$no_caddy" -eq 0 ]; then echo "  Caddy:          $CADDYFILE   (log: journalctl -u caddy -f)"; fi
echo "  Status:         $0 --status"
echo "  Update:         run this script again.   Remove: $0 --uninstall"
echo
echo "  Open these on every firewall in front of this server (your VPS"
echo "  provider's firewall or security group, a router's port forwards):"
echo
if [ "$no_caddy" -eq 0 ]; then
  echo "    TCP 80           Caddy: the game's config and content, the launcher's API"
else
  echo "    TCP 80           the game's online config (the game always uses port 80)"
  echo "    TCP 8000         content (multiplayer balancing)"
  echo "    TCP 50051        accounts, friends and invites (launcher and overlay)"
fi
if [ "$no_caddy" -eq 0 ] && [ "$https_api" -eq 1 ]; then
  echo "    TCP 443          Caddy: the launcher's API over HTTPS${coord_domain:+, and the coordinator}"
elif [ -n "$coord_domain" ]; then
  echo "    TCP 443          the coordinator (HTTPS)"
fi
echo "    UDP 21126        game login"
echo "    UDP 21127        game service"
echo "    UDP 21128-21129  internet play (public addresses and the relay)"
echo
ssh_port_now="$(sshd -T 2>/dev/null | awk '$1 == "port" {print $2}' | tail -1)"
echo "  Keep SSH (TCP ${ssh_port_now:-22}) open as well. Nothing else is needed."
if [ -n "$opened" ]; then
  echo "  This machine's $opened now allows them."
else
  echo "  There's no active ufw or firewalld here, so no rules were added on this machine."
fi
if [ "$no_caddy" -eq 0 ]; then
  echo
  echo "  Check from your own PC:  curl http://$domain/api/info"
fi
if [ -n "$coord_domain" ]; then
  echo
  echo "  Coordinator:    https://$coord_domain   (log: journalctl -u $COORD_SERVICE -f)"
  echo "  Other servers join it with its join token (keep it private):"
  echo "    sudo bash $0 --show-join-token     # copy it to the other server as token.txt, then there:"
  echo "    --coordinator https://$coord_domain --join-token-file token.txt"
elif [ -n "$coordinator" ]; then
  echo
  echo "  Friends are shared through $coordinator (log: journalctl -u $SERVICE | grep Federation)"
fi
for note in "${notes[@]}"; do
  echo
  warn "$note"
done
