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
COORD_SERVICE="5th-echelon-coordinator"
COORD_UNIT="/etc/systemd/system/${COORD_SERVICE}.service"
COORD_ASSET="coordinator-linux-x86_64"
COORD_DIR="$STATE_DIR/coordinator"
# The installer's own records, owned by root and outside the folders the
# services can write: which coordinator domain runs here, which firewall
# rules it added.
ETC_DIR="/etc/5th-echelon"
FIREWALL_RECORD="$ETC_DIR/firewall-rules"
# The release key (its public half), which signs each release's SHA256SUMS.
RELEASE_KEY_PEM="-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEANX9q9hOdzlNhVlSPEtmjJbbdJyOktGgQkPw4ep2KCLI=
-----END PUBLIC KEY-----"
# Caddy's static build, when there's no package: pinned, with its SHA-512.
CADDY_VERSION="2.11.4"
CADDY_SHA512_amd64="8220d1f013b6f27510247b2360c9e0ca9f018feebd82515f07635318b34ff9777ccc8fd0b6e6f2486ce3a33fe389fbb7db12d05baa474f4587509fb4f5ebf1c9"
CADDY_SHA512_arm64="d5a7c423853c24a799765e0e8210d5c7c22a8f56ed37a3cae2fb9f58be138853c02b4efd6b59d576e6d8c7c0d30b9c1592deeaa6a536ff69bcca23b8c1ea709c"

domain="" no_caddy=0 public_address="" version="latest" binary="" relay=""
firewall=1 yes=0 force=0 uninstall=0 purge=0 use_systemd=1
friends="" server_name="" region="" coordinator="" join_token="" coord_domain="" coord_binary=""
https_api=1 allow_unsigned=0 coord_only=0 admin="" registration="" listed="" command=""
aliases=()
# Only HTTPS, and TLS 1.2 or newer, for every download.
CURL=(curl --proto '=https' --tlsv1.2)

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

# A value from service.toml: `value section key` (quotes stripped).
value() { sed -n "/^\\[$1\\]\$/,/^\\[/ s/^$2 = \"\\{0,1\\}\\([^\"]*\\)\"\\{0,1\\}\$/\\1/p" "$CONFIG" 2>/dev/null | head -1; }

if [ "$command" = status ]; then
  printf '%-30s %s\n' "Service" "State"
  for s in "$SERVICE" "$COORD_SERVICE" caddy; do
    if systemctl cat "$s" >/dev/null 2>&1; then printf '%-30s %s\n' "$s" "$(systemctl is-active "$s" 2>/dev/null || true)"; fi
  done
  if [ -f "$CONFIG" ]; then
    host="$(value public host)"
    echo
    info="$(curl -fsS --max-time 3 ${host:+-H "Host: $host"} http://127.0.0.1/api/info 2>/dev/null || true)"
    if [ -n "$info" ]; then
      echo "Server:        $(printf '%s' "$info" | grep -o '"version":"[^"]*"' | cut -d'"' -f4)${host:+ at $host}"
      case "$info" in *'"api_tls":443'*) echo "API:           HTTPS (https://$host) and plain" ;; *) echo "API:           plain only (no api_tls)" ;; esac
    else
      echo "Server:        doesn't answer /api/info on this machine"
    fi
    echo "Friend lists:  $(value friends mode)"
    echo "Accounts:      $( [ "$(value limits open_registration)" = false ] && echo "closed to new players" || echo "open")"
    echo "Admin API:     $( [ "$(value admin enabled)" = true ] && echo "on (SSH tunnel to 127.0.0.1:50051; key in $STATE_DIR/admin-key.txt)" || echo "off")"
    coord="$(value federation coordinator)"
    if [ -n "$coord" ]; then
      echo "Shares friends: through $coord$( [ -s "$STATE_DIR/federation.key" ] && echo " (joined)" || echo " (not joined yet)")"
      journalctl -u "$SERVICE" --since "-1h" --no-pager 2>/dev/null | grep -o 'Federation:.*' | tail -1 | sed 's/^/                /' || true
    else
      echo "Shares friends: no"
    fi
  fi
  if [ -s "$ETC_DIR/coordinator-domain" ]; then
    echo
    cinfo="$(curl -fsS --max-time 3 http://127.0.0.1:8700/v1/info 2>/dev/null || true)"
    echo "Coordinator:   https://$(cat "$ETC_DIR/coordinator-domain") ${cinfo:+($cinfo)}"
    listed_now="$(curl -fsS --max-time 3 http://127.0.0.1:8700/v1/servers 2>/dev/null | grep -o '"id":' | wc -l || true)"
    echo "Directory:     ${listed_now:-0} server(s) seen in the last 2 minutes"
  fi
  exit 0
fi
if [ "$command" = show-token ] || [ "$command" = rotate-token ]; then
  [ -f "$COORD_DIR/join-token.txt" ] || die "no coordinator is installed here"
  [ ! -L "$COORD_DIR/join-token.txt" ] || die "$COORD_DIR/join-token.txt isn't a plain file"
  if [ "$command" = rotate-token ]; then
    runuser -u "$USER_NAME" -- "$PROGRAM_DIR/coordinator" --data "$COORD_DIR" new-token >/dev/null
    systemctl restart "$COORD_SERVICE" 2>/dev/null || true
    say "Made a new join token; servers that already joined keep working."
  fi
  echo "Join token (keep it private; give it to the operators of servers joining):"
  cat "$COORD_DIR/join-token.txt"
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
  elif [ -f "$COORD_DIR/domain" ] && [ ! -L "$COORD_DIR/domain" ]; then
    # Where installs before 0.4 kept it (checked below like --coordinator-domain).
    coord_domain="$(head -c 256 "$COORD_DIR/domain" | tr -d '[:space:]')"
  fi
fi
if [ "$coord_only" -eq 1 ]; then
  [ -n "$coord_domain" ] || die "--coordinator-only needs --coordinator-domain"
  [ -z "$domain" ] && [ "$no_caddy" -eq 0 ] && [ -z "$coordinator" ] || die "--coordinator-only runs no game server: leave out --domain, --no-caddy and --coordinator"
fi
[ -z "$coord_domain" ] || [ "$no_caddy" -eq 0 ] || die "--coordinator-domain needs Caddy (for HTTPS); leave out --no-caddy"
[ -z "$coord_domain" ] || [ -z "$coordinator" ] || die "--coordinator-domain runs a coordinator here; leave out --coordinator"
[ -z "$coordinator" ] || [ -n "$join_token" ] || [ -f "$STATE_DIR/federation.key" ] || die "--coordinator needs --join-token (from the coordinator's operator)"
# Everything that goes into service.toml or a URL is checked first: no
# quotes, newlines or anything else that could change what's written.
DOMAIN_RE='^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$'
COORD_RE='^https://([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}(:[0-9]{1,5})?$'
if [ -n "$coordinator" ]; then
  coordinator="${coordinator%/}"
  [[ "$coordinator" =~ $COORD_RE ]] \
    || die "--coordinator is an https:// address with no path, e.g. https://coordinator.example.com"
fi
[ -z "$join_token" ] || [[ "$join_token" =~ ^[A-Z2-7]{16,128}$ ]] || die "that isn't a join token (letters A-Z and digits 2-7)"
NAME_RE="^[A-Za-z0-9][A-Za-z0-9 ._(),'-]{0,63}\$"
[ -z "$server_name" ] || [[ "$server_name" =~ $NAME_RE ]] || die "--server-name is up to 64 letters, digits, spaces and . _ ( ) , ' -"
[ -z "$region" ] || [[ "$region" =~ $NAME_RE ]] || die "--region is up to 64 letters, digits, spaces and . _ ( ) , ' -"
[ "$version" = latest ] || [[ "$version" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?$ ]] || die "--version is a release number, e.g. 0.4.0"
coord_domain="${coord_domain,,}"
[ -z "$coord_domain" ] || [[ "$coord_domain" =~ $DOMAIN_RE ]] || die "\"$coord_domain\" isn't a domain name"
IPV4_RE='^[0-9]{1,3}(\.[0-9]{1,3}){3}$'
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

# The PIDs of every running copy of the installed server, by command line
# (/proc/*/exe can name an emulator instead of the program).
server_pids() {
  local p cmd
  for p in /proc/[0-9]*; do
    [ "${p#/proc/}" = "$$" ] && continue
    cmd="$({ tr '\0' ' ' < "$p/cmdline"; } 2>/dev/null)" || continue
    case "$cmd" in
      "$PROGRAM_DIR/dedicated_server"*|*" $PROGRAM_DIR/dedicated_server"*)
        case "$cmd" in runuser*) ;; *) echo "${p#/proc/}" ;; esac ;;
    esac
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
    rm -f "$UNIT" "$COORD_UNIT"
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
  rm -rf "$PROGRAM_DIR" "$ETC_DIR"
  rm -f "$SYSCTL_FILE"
  if [ "$purge" -eq 1 ]; then
    rm -rf "$STATE_DIR"
    userdel "$USER_NAME" 2>/dev/null || true
    say "Removed the server and its data."
  else
    say "Removed the server. Its data (accounts, settings, keys) is still in $STATE_DIR; --purge deletes it."
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
  domain="$(sed -n '/^\[public\]$/,/^\[/ s/^host = "\(.*\)"$/\1/p' "$CONFIG" | head -1)"
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
if [ "$no_caddy" -eq 0 ]; then
  [[ "$domain" =~ $DOMAIN_RE ]] || die "\"$domain\" isn't a domain name"
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
if [ -n "$coord_domain" ] && [ "${coord_dns_checked:-0}" -eq 0 ]; then check_dns "$coord_domain"; fi

# --- The program --------------------------------------------------------

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Checks the release's SHA256SUMS against its signature (SHA256SUMS.sig,
# base32, by the release key): whoever can change a release on GitHub
# still can't change what this installs.
verify_release() {
  local base="$1"
  if ! "${CURL[@]}" -fsSL --retry 3 -o "$work/SHA256SUMS.sig" "$base/SHA256SUMS.sig" 2>/dev/null; then
    [ "$allow_unsigned" -eq 1 ] || die "this release isn't signed (no SHA256SUMS.sig). --allow-unsigned installs it anyway, checked by its checksum only"
    warn "the release isn't signed; installing it on its checksum alone (--allow-unsigned)"
    return 0
  fi
  printf '%s\n' "$RELEASE_KEY_PEM" > "$work/release.pem"
  { printf '5th-echelon/release/v1\n'; cat "$work/SHA256SUMS"; } > "$work/signed"
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
    die "the release's signature doesn't match: SHA256SUMS wasn't signed by the release key. Not installing it"
  fi
}

if [ "$coord_only" -eq 1 ]; then
  :
elif [ -n "$binary" ]; then
  [ -f "$binary" ] || die "$binary doesn't exist"
  cp "$binary" "$work/$ASSET"
  say "Using $binary"
else
  if [ "$version" = latest ]; then
    base="https://github.com/$REPO/releases/latest/download"
  else
    base="https://github.com/$REPO/releases/download/v${version#v}"
  fi
  say "Downloading the server ($version)"
  "${CURL[@]}" -fsSL --retry 3 -o "$work/$ASSET" "$base/$ASSET" \
    || die "couldn't download $base/$ASSET (is there a release yet? --binary installs a file you have)"
  "${CURL[@]}" -fsSL --retry 3 -o "$work/SHA256SUMS" "$base/SHA256SUMS" || die "couldn't download the release's SHA256SUMS"
  verify_release "$base"
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
      if [ "$version" = latest ]; then base="https://github.com/$REPO/releases/latest/download"; else base="https://github.com/$REPO/releases/download/v${version#v}"; fi
      "${CURL[@]}" -fsSL --retry 3 -o "$work/SHA256SUMS" "$base/SHA256SUMS" || die "couldn't download the release's SHA256SUMS"
      verify_release "$base"
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

if ! id "$USER_NAME" >/dev/null 2>&1; then
  say "Creating the system user $USER_NAME"
  useradd --system --home-dir "$STATE_DIR" --no-create-home --shell /usr/sbin/nologin "$USER_NAME" 2>/dev/null \
    || useradd --system --home-dir "$STATE_DIR" --no-create-home --shell /sbin/nologin "$USER_NAME"
fi

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
for f in "$CONFIG" "$STATE_DIR/federation.key"; do
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

# Sets `key = value` in [section] of service.toml.
toml_set() {
  sed -i -E "/^\\[$1\\]\$/,/^\\[/ s|^$2 = .*|$2 = $3|" "$CONFIG"
}
if [ -n "$relay" ]; then
  toml_set nat relay "\"$relay\""
  say "Relay: $relay"
fi
# [public] (the host name and ports players use) is rewritten below; its
# aliases stay unless new ones are given.
if [ "${#aliases[@]}" -gt 0 ]; then
  aliases_line="[$(printf '"%s", ' "${aliases[@]}" | sed 's/, $//')]"
else
  aliases_line="$(sed -n '/^\[public\]$/,/^\[/ s/^aliases = \(\[.*\]\)$/\1/p' "$CONFIG" | head -1)"
  # Only names and addresses, as --alias checks them.
  [[ "$aliases_line" =~ ^\[(\"[a-z0-9.-]+\"(,\ )?)*\]$ ]] || aliases_line=""
  mapfile -t aliases < <(printf '%s' "$aliases_line" | grep -o '"[^"]*"' | tr -d '"')
fi
sed -i '/^\[public\]$/,/^\[/{/^\[public\]$/d;/^\[/!d}' "$CONFIG"
if [ "$no_caddy" -eq 0 ]; then
  say "Settings for Caddy: the web parts and API on this machine only; players use $domain"
  sed -i -E 's|^api_server = .*|api_server = "127.0.0.1:50051"|' "$CONFIG"
  toml_set 'service\.onlineconfig' listen '"127.0.0.1:8080"'
  toml_set 'service\.content' listen '"127.0.0.1:8000"'
  printf '\n[public]\nhost = "%s"\napi = 80\ncontent = 80\n' "$domain" >> "$CONFIG"
  [ -z "$aliases_line" ] || printf 'aliases = %s\n' "$aliases_line" >> "$CONFIG"
else
  sed -i -E 's|^api_server = "127\.0\.0\.1:|api_server = "0.0.0.0:|' "$CONFIG"
  toml_set 'service\.onlineconfig' listen '"0.0.0.0:80"'
  toml_set 'service\.content' listen '"0.0.0.0:8000"'
  [ -z "$aliases_line" ] || printf '\n[public]\naliases = %s\n' "$aliases_line" >> "$CONFIG"
fi
[ -z "$aliases_line" ] || say "Also reached as: ${aliases[*]}"
# Friend lists: only friends on a new (public) server; an update keeps the setting.
if [ -z "$friends" ]; then
  if [ "$first_install" -eq 1 ]; then friends=mutual; else friends="$(value friends mode)"; friends="${friends:-mutual}"; fi
fi
if grep -q '^\[friends\]$' "$CONFIG"; then
  toml_set friends mode "\"$friends\""
else
  printf '\n[friends]\nmode = "%s"\n' "$friends" >> "$CONFIG"
fi
say "Friend lists: $friends"
if [ -n "$admin" ]; then toml_set admin enabled "$admin"; fi
if [ -n "$registration" ]; then toml_set limits open_registration "$registration"; fi
say "Admin API: $( [ "$(value admin enabled)" = true ] && echo "on (through an SSH tunnel only)" || echo off)"
say "New accounts: $( [ "$(value limits open_registration)" = false ] && echo closed || echo open)"
fi

# A coordinator on this machine: its own service, behind Caddy on HTTPS.
if [ -n "$coord_domain" ]; then
  say "Installing the coordinator for https://$coord_domain"
  install -m 755 "$work/$COORD_ASSET" "$PROGRAM_DIR/coordinator"
  install -d -m 700 -o "$USER_NAME" -g "$USER_NAME" "$COORD_DIR"
  echo "$coord_domain" > "$ETC_DIR/coordinator-domain"
  rm -f "$COORD_DIR/domain"
  if [ "$use_systemd" -eq 1 ]; then
    cat > "$COORD_UNIT" <<UNIT
[Unit]
Description=5th Echelon coordinator (friends across servers, server directory)
Documentation=https://github.com/$REPO
After=network-online.target
Wants=network-online.target

[Service]
User=$USER_NAME
Group=$USER_NAME
WorkingDirectory=$COORD_DIR
ExecStart=$PROGRAM_DIR/coordinator --listen 127.0.0.1:8700 --data $COORD_DIR
Restart=always
RestartSec=3
CapabilityBoundingSet=
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=$COORD_DIR
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
    systemctl enable "$COORD_SERVICE" >/dev/null 2>&1
    systemctl restart "$COORD_SERVICE"
    for _ in $(seq 40); do [ -s "$COORD_DIR/join-token.txt" ] && break; sleep 0.25; done
    [ -s "$COORD_DIR/join-token.txt" ] || { journalctl -u "$COORD_SERVICE" -n 20 --no-pager >&2 || true; die "the coordinator didn't start (its log is above)"; }
  else
    ( cd "$COORD_DIR" && runuser -u "$USER_NAME" -- "$PROGRAM_DIR/coordinator" --listen 127.0.0.1:8700 --data "$COORD_DIR" >/dev/null 2>&1 & sleep 1; kill $! 2>/dev/null || true )
  fi
  coordinator="https://$coord_domain"
  [ ! -L "$COORD_DIR/join-token.txt" ] || die "$COORD_DIR/join-token.txt isn't a plain file"
  join_token="$(head -c 256 "$COORD_DIR/join-token.txt" | tr -d '[:space:]')"
  [[ "$join_token" =~ ^[A-Z2-7]{16,128}$ ]] || die "the coordinator's join token isn't one"
fi

if [ "$coord_only" -eq 0 ]; then
# [federation]: rewritten when a coordinator is given, kept otherwise.
if [ -n "$coordinator" ]; then
  # A name or region set before (by hand, or an earlier run) stays unless given again.
  old_value() { sed -n "/^\\[federation\\]\$/,/^\\[/ s/^$1 = \"\\(.*\\)\"\$/\\1/p" "$CONFIG" | head -1; }
  server_name="${server_name:-$(old_value name)}"
  region="${region:-$(old_value region)}"
  if [ -z "$listed" ]; then listed="$(value federation listed)"; fi
  sed -i '/^\[federation\]$/,/^\[/{/^\[federation\]$/d;/^\[/!d}' "$CONFIG"
  {
    printf '\n[federation]\ncoordinator = "%s"\n' "$coordinator"
    [ -z "$join_token" ] || printf 'join_token = "%s"\n' "$join_token"
    printf 'name = "%s"\n' "${server_name:-${domain:-$public_address}}"
    [ -z "$region" ] || printf 'region = "%s"\n' "$region"
    [ "$listed" != false ] || printf 'listed = false\n'
  } >> "$CONFIG"
  say "Sharing friends through $coordinator$( [ "$listed" = false ] && echo ", not listed in the directory")"
elif [ "$listed" = false ] || [ "$listed" = true ]; then
  warn "--listed and --unlisted only matter with a coordinator; ignored"
fi
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
ExecStart=$PROGRAM_DIR/dedicated_server
Restart=always
RestartSec=3
# Port 80 without running as root.
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=$STATE_DIR $PROGRAM_DIR/data
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

# The site's routes, for http:// and https:// alike. The admin API
# (accounts and games) is never served to the internet: manage the server
# on the machine itself (or through an SSH tunnel to 127.0.0.1:50051).
site_routes() {
  cat <<'SITE'
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
  if [ -n "$coord_domain" ]; then
    cat <<SITE

# The 5th Echelon coordinator: friends across servers and the server
# directory, over HTTPS (Caddy gets the certificate).
$coord_domain {
	reverse_proxy 127.0.0.1:8700
}
SITE
  fi
}

notes=()
if [ "$no_caddy" -eq 0 ]; then
  install_caddy || true
  command -v caddy >/dev/null || install_caddy_static
  caddy version >/dev/null 2>&1 || die "Caddy was installed but doesn't run ($(command -v caddy)); install Caddy 2.6+ yourself and run this again"
  caddy_version="$(caddy version | cut -d' ' -f1)"
  case "$caddy_version" in
    v2.[0-5].*) die "Caddy $caddy_version is too old (2.6 or newer speaks the launcher's gRPC); update it and run this again" ;;
  esac
  install -d -m 755 /etc/caddy
  # The installer's block: the global option for the launcher's API and
  # the site, between markers so updates leave everything else alone.
  managed="$work/managed.caddy"
  {
    echo "$BEGIN_MARK"
    echo "{"
    echo "	# The launcher and overlay speak gRPC without TLS (h2c)."
    echo "	servers :80 {"
    echo "		protocols h1 h2 h2c"
    echo "	}"
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
    systemctl enable caddy >/dev/null 2>&1
    systemctl reload-or-restart caddy
  fi
  if [ "$use_systemd" -eq 1 ] && [ -n "$coord_domain" ]; then
    coord_ok=0
    for _ in $(seq 60); do
      if curl -fsS --max-time 3 --resolve "$coord_domain:443:127.0.0.1" "https://$coord_domain/v1/info" >/dev/null 2>&1; then coord_ok=1; break; fi
      sleep 1
    done
    if [ "$coord_ok" -eq 1 ]; then
      say "The coordinator answers at https://$coord_domain"
    else
      notes+=("Caddy has no certificate for $coord_domain yet, so other servers can't reach the coordinator. Check its A record (DNS only, not proxied) and that TCP 80 and 443 are open, then run this script again.")
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
      if [ "$tls_ok" -eq 1 ]; then
        sed -i '/^\[public\]$/,/^\[/ { /^api_tls = /d; s/^content = 80$/content = 80\napi_tls = 443/ }' "$CONFIG"
        systemctl restart "$SERVICE"
        # Back up before anything below talks to it.
        for _ in $(seq 40); do
          if (exec 3<>/dev/tcp/127.0.0.1/50051) 2>/dev/null; then break; fi
          sleep 0.5
        done
        say "The launcher's API is served over HTTPS too (https://$domain)"
      else
        journalctl -u caddy -n 20 --no-pager >&2 || true
        notes+=("Caddy has no certificate for $domain yet (its log is above), so launchers keep using the unencrypted API. Check the A record and that TCP 80 and 443 are open, then run this script again.")
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

# --- Firewall -----------------------------------------------------------

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
echo "  Keep SSH (TCP 22) open as well. Nothing else is needed."
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
