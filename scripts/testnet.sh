#!/usr/bin/env bash
# A temporary test network on Vultr, billed by the hour: made, installed with
# this checkout's installer and binaries, and deleted again, by itself.
#
#   scripts/testnet.sh up rehearsal --hours 6     # coordinator, matchmaking, 2 relays
#   scripts/testnet.sh list                        # what's running, for how long, the cost so far
#   scripts/testnet.sh ssh mm                      # a shell on one of them
#   scripts/testnet.sh check                       # the UDP check again
#   scripts/testnet.sh install                     # copy new binaries and install again
#   scripts/testnet.sh extend 2                    # 2 more hours
#   scripts/testnet.sh firewall                    # SSH from this computer's address as it is now
#   scripts/testnet.sh down                        # delete it all
#
# Layouts:
#   one        one server (s1) with its own coordinator
#   pair       a server with its coordinator (s1), and a second server (s2) that joins it
#   rehearsal  global matchmaking: a coordinator on its own, the matchmaking server
#              and two relays (as docs/cutover.md's rehearsal)
#   regional   three regional servers as a network is before its cutover: eu with the
#              coordinator and its admin UI, na and oceania joining it (for rehearsing the
#              night itself: docs/cutover.md's update window)
#
# Which files: this checkout's (dist/ and scripts/install-server.sh), or another release's with
# TESTNET_FILES=DIR, a folder with dedicated_server-linux-x86_64, coordinator-linux-x86_64 and
# install-server.sh (e.g. the release production runs, to start a rehearsal where it is).
#
# Only deleting stops Vultr's bill: a stopped or shut-down machine is still charged. So every
# network has an end (--hours, 4 by default): `up` leaves a watcher running on this computer that
# deletes the network then, and every run of this script deletes the networks that are past their
# end. If this computer sleeps through the end, the next run (or `down`) does it.
#
# Needs: curl, jq, ssh, scp, python3 (for the UDP check), and the Linux binaries from
# `build/build.sh server` in dist/. The settings are in
# ${XDG_CONFIG_HOME:-~/.config}/5th-echelon/testnet.env (never in the repo):
#
#   VULTR_API_KEY=...            Vultr › Account › API (allow your own IPv4 address there: the
#                                script calls the API over IPv4 only, so one /32 is enough)
#   # Optional: real names in a zone of yours on Cloudflare (DNS only, TTL 60), needed for the
#   # matchmaking server's moving name (--mm-record) and the move/failover steps. Without them
#   # the servers are named IP.sslip.io, and Let's Encrypt still gives them certificates.
#   CF_API_TOKEN=...             a token with Zone › DNS › Edit on that zone
#   CF_ZONE_ID=...
#   TESTNET_DOMAIN=test.example.net
#   # Optional:
#   TESTNET_PLAN=vc2-1c-1gb      the size of each machine (`testnet.sh plans`): 1 GB is plenty
#                                (the server needs under 100 MB at 1,000 players, docs/load-testing.md)
#   TESTNET_SSH_KEY=~/.ssh/id_ed25519.pub   default: the first key ssh-agent has
#   TESTNET_SSH_FROM=1.2.3.4     the address SSH is allowed from (default: this computer's IPv4,
#                                from api.ipify.org)
#
# Options for `up`:
#   --name NAME       the network's name, for running more than one (default: test)
#   --hours H         when it's deleted by itself (default: 4)
#   --plan PLAN       the machines' size (default: TESTNET_PLAN, or vc2-1c-1gb)
#   --region ROLE=R   where a machine goes (`testnet.sh regions`), e.g. --region mm=yto
#   --no-install      only make the machines (and run the UDP check)
#
# Every machine is tagged fe-testnet; `down --all` deletes every one of them.
set -euo pipefail

CONF_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/5th-echelon"
ENV_FILE="$CONF_DIR/testnet.env"
KNOWN_HOSTS="$CONF_DIR/testnet_known_hosts"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
API="https://api.vultr.com/v2"
TAG="fe-testnet"
# Vultr bills a month as 672 hours.
MONTH_HOURS=672
# The game takes server names of at most 27 characters.
NAME_MAX=27
UDP_CHECK_PORT=40999

die() { echo "testnet: $*" >&2; exit 1; }
say() { echo "==> $*" >&2; }

case "${1:-}" in "" | help | -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac
[ -f "$ENV_FILE" ] || die "no settings: put VULTR_API_KEY=... in $ENV_FILE (see this script's header)"
# shellcheck disable=SC1090
. "$ENV_FILE"
[ -n "${VULTR_API_KEY:-}" ] || die "VULTR_API_KEY isn't set in $ENV_FILE"
for tool in curl jq ssh scp; do command -v "$tool" >/dev/null || die "$tool is needed"; done
mkdir -p "$CONF_DIR"; chmod 700 "$CONF_DIR"
use_cf=0
if [ -n "${CF_API_TOKEN:-}" ] || [ -n "${CF_ZONE_ID:-}" ] || [ -n "${TESTNET_DOMAIN:-}" ]; then
  [ -n "${CF_API_TOKEN:-}" ] && [ -n "${CF_ZONE_ID:-}" ] && [ -n "${TESTNET_DOMAIN:-}" ] ||
    die "Cloudflare names need all three of CF_API_TOKEN, CF_ZONE_ID and TESTNET_DOMAIN"
  use_cf=1
fi

# --- Vultr's API ---------------------------------------------------------------------------

vapi() { # METHOD PATH [JSON]
  local out code
  out="$(mktemp)"
  code="$(curl -4 -sS -o "$out" -w '%{http_code}' -X "$1" "$API$2" \
    -H "Authorization: Bearer $VULTR_API_KEY" -H 'Content-Type: application/json' \
    ${3:+--data "$3"})" || { rm -f "$out"; die "Vultr didn't answer ($1 $2)"; }
  if [ "${code:0:1}" != 2 ]; then
    echo "testnet: Vultr said $code to $1 $2: $(jq -r '.error // .' "$out" 2>/dev/null || cat "$out")" >&2
    rm -f "$out"; return 1
  fi
  cat "$out"; rm -f "$out"
}

# Every test machine, as JSON lines: id label ip region plan created tags.
instances() {
  vapi GET "/instances?tag=$TAG&per_page=500" |
    jq -c '.instances[] | {id, label, ip: .main_ip, region, plan, created: .date_created, status, server_status, tags}'
}

net_instances() { instances | jq -c --arg t "fe-net-$1" 'select(.tags | index($t))'; }

expiry_of() { jq -r '[.tags[] | select(startswith("expires-")) | ltrimstr("expires-") | tonumber] | first // empty'; }

role_of() { jq -r '[.tags[] | select(startswith("fe-role-")) | ltrimstr("fe-role-")] | first // empty'; }

# --- Cloudflare (optional) -----------------------------------------------------------------

cfapi() { # METHOD PATH [JSON]
  curl -fsS -X "$1" "https://api.cloudflare.com/client/v4/zones/$CF_ZONE_ID$2" \
    -H "Authorization: Bearer $CF_API_TOKEN" -H 'Content-Type: application/json' ${3:+--data "$3"}
}

cf_record() { # NAME IP NET
  local body
  body="$(jq -nc --arg n "$1" --arg ip "$2" --arg c "fe-testnet:$3" \
    '{type: "A", name: $n, content: $ip, ttl: 60, proxied: false, comment: $c}')"
  cfapi POST /dns_records "$body" >/dev/null || die "Cloudflare didn't take the record $1"
}

cf_delete_net() { # NET
  [ "$use_cf" -eq 1 ] || return 0
  local ids id
  ids="$(cfapi GET "/dns_records?per_page=500&comment.exact=fe-testnet:$1" | jq -r '.result[].id')" || return 0
  for id in $ids; do cfapi DELETE "/dns_records/$id" >/dev/null || echo "testnet: couldn't delete DNS record $id" >&2; done
}

# --- Names ---------------------------------------------------------------------------------

# A role's game server name, and the coordinator's and the matchmaking record's.
game_name() { # ROLE NET IP
  if [ "$use_cf" -eq 1 ]; then echo "$1-$2.$TESTNET_DOMAIN"; else echo "${3//./-}.sslip.io"; fi
}
coord_name() { # NET IP
  if [ "$use_cf" -eq 1 ]; then echo "coord-$1.$TESTNET_DOMAIN"; else echo "c.${2//./-}.sslip.io"; fi
}
# The coordinator's admin UI (a name of its own).
admin_name() { # NET IP
  if [ "$use_cf" -eq 1 ]; then echo "admin-$1.$TESTNET_DOMAIN"; else echo "a.${2//./-}.sslip.io"; fi
}
match_name() { # NET
  [ "$use_cf" -eq 1 ] && echo "match-$1.$TESTNET_DOMAIN" || true
}

# --- SSH -----------------------------------------------------------------------------------

SSH_OPTS=(-o UserKnownHostsFile="$KNOWN_HOSTS" -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=10 -o ServerAliveInterval=30 -o LogLevel=ERROR
  -o ControlMaster=auto -o ControlPath="$CONF_DIR/cm-%C" -o ControlPersist=10m)
on() { local ip="$1"; shift; ssh "${SSH_OPTS[@]}" "root@$ip" "$@"; }
copy_to() { local ip="$1"; shift; scp -q "${SSH_OPTS[@]}" "$@" "root@$ip:fe/"; }

ssh_key_id() {
  local pub keys id
  if [ -n "${TESTNET_SSH_KEY:-}" ]; then
    pub="$(cat "${TESTNET_SSH_KEY/#\~/$HOME}")" || die "can't read TESTNET_SSH_KEY"
  else
    pub="$(ssh-add -L 2>/dev/null | head -1)"
    [ -n "$pub" ] && [[ "$pub" == ssh-* || "$pub" == ecdsa-* ]] || die "ssh-agent has no key: set TESTNET_SSH_KEY to a public key file"
  fi
  pub="$(echo "$pub" | awk '{print $1" "$2}')"
  keys="$(vapi GET /ssh-keys?per_page=500)"
  id="$(echo "$keys" | jq -r --arg k "$pub" '.ssh_keys[] | select((.ssh_key | split(" ")[0:2] | join(" ")) == $k) | .id' | head -1)"
  if [ -z "$id" ]; then
    id="$(vapi POST /ssh-keys "$(jq -nc --arg k "$pub" '{name: "fe-testnet", ssh_key: $k}')" | jq -r .ssh_key.id)"
  fi
  echo "$id"
}

# --- Vultr's firewall -----------------------------------------------------------------------

# The test machines' own firewall group (fe-testnet), made and kept by this script: SSH only from
# this computer's IPv4 address (as it is now; an old one is dropped), the web (certificates, the
# launchers, the coordinator), the game's UDP ports, the UDP check's port, and ping.
FIREWALL_NAME="fe-testnet"
firewall_group() {
  local id me rules want w proto port sub size rid
  me="${TESTNET_SSH_FROM:-$(curl -4 -fsS --max-time 10 https://api.ipify.org)}" || die "can't find this computer's IPv4 address (set TESTNET_SSH_FROM)"
  [[ "$me" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "\"$me\" isn't an IPv4 address"
  id="$(vapi GET "/firewalls?per_page=500" | jq -r --arg d "$FIREWALL_NAME" '.firewall_groups[] | select(.description == $d) | .id' | head -1)"
  [ -n "$id" ] || id="$(vapi POST /firewalls "{\"description\": \"$FIREWALL_NAME\"}" | jq -r .firewall_group.id)"
  rules="$(vapi GET "/firewalls/$id/rules?per_page=500" | jq -c '.firewall_rules[]')"
  want="tcp 22 $me 32
tcp 80 0.0.0.0 0
tcp 443 0.0.0.0 0
udp 21126:21129 0.0.0.0 0
udp $UDP_CHECK_PORT 0.0.0.0 0
icmp - 0.0.0.0 0"
  # An SSH rule from another address (an old one of this computer's) goes.
  for rid in $(jq -r --arg me "$me" 'select(.protocol == "tcp" and .port == "22" and .subnet != $me) | .id' <<<"$rules"); do
    vapi DELETE "/firewalls/$id/rules/$rid" >/dev/null
  done
  while read -r proto port sub size; do
    [ "$port" = - ] && port=""
    if [ -z "$(jq -r --arg p "$proto" --arg port "$port" --arg s "$sub" --argjson z "$size" \
        'select(.protocol == $p and (.port // "") == $port and .subnet == $s and .subnet_size == $z) | .id' <<<"$rules")" ]; then
      vapi POST "/firewalls/$id/rules" "$(jq -nc --arg p "$proto" --arg port "$port" --arg s "$sub" --argjson z "$size" \
        '{ip_type: "v4", protocol: $p, subnet: $s, subnet_size: $z, notes: "fe-testnet"} + (if $port == "" then {} else {port: $port} end)')" >/dev/null
    fi
  done <<<"$want"
  echo "$id"
}

# --- Layouts -------------------------------------------------------------------------------

# Each layout's roles, and their regions by default (Vultr's codes: `testnet.sh regions`).
layout_roles() {
  case "$1" in
    one) echo "s1" ;;
    pair) echo "s1 s2" ;;
    rehearsal) echo "coord mm relay-a relay-b" ;;
    regional) echo "eu na oceania" ;;
    *) die "no layout \"$1\" (one, pair, rehearsal, regional)" ;;
  esac
}
default_region() { # LAYOUT ROLE
  case "$1:$2" in
    one:s1 | pair:s1 | pair:s2) echo syd ;;
    rehearsal:coord | rehearsal:relay-a) echo fra ;;
    rehearsal:mm) echo yto ;;
    rehearsal:relay-b) echo syd ;;
    regional:eu) echo fra ;;
    regional:na) echo yto ;;
    regional:oceania) echo syd ;;
  esac
}

# --- Ends ----------------------------------------------------------------------------------

down_net() { # NET
  local ids id
  ids="$(net_instances "$1" | jq -r .id)"
  [ -n "$ids" ] || { cf_delete_net "$1"; return 0; }
  for id in $ids; do vapi DELETE "/instances/$id" >/dev/null && say "deleted $id"; done
  cf_delete_net "$1"
  say "network $1 is deleted"
}

# Deletes every network past its end.
sweep() {
  local now nets net exp
  now="$(date +%s)"
  nets="$(instances | jq -r '.tags[] | select(startswith("fe-net-")) | ltrimstr("fe-net-")' | sort -u)"
  for net in $nets; do
    exp="$(net_instances "$net" | expiry_of | sort -n | head -1)"
    if [ -n "$exp" ] && [ "$exp" -le "$now" ]; then
      say "network $net is past its end ($(date -r "$exp" 2>/dev/null || date -d "@$exp")): deleting it"
      down_net "$net"
    fi
  done
}

# Runs in the background after `up`: deletes the network at its end (read again each minute,
# so `extend` moves it), with a notice 15 minutes before.
reap() { # NET
  local exp now warned=0
  while :; do
    exp="$(net_instances "$1" 2>/dev/null | expiry_of | sort -n | head -1)" || exp=""
    [ -n "$exp" ] || { [ -n "$(net_instances "$1" 2>/dev/null | head -1)" ] || exit 0; sleep 60; continue; }
    now="$(date +%s)"
    if [ "$exp" -le "$now" ]; then down_net "$1"; exit 0; fi
    if [ $((exp - now)) -le 900 ] && [ "$warned" -eq 0 ]; then
      warned=1
      command -v osascript >/dev/null &&
        osascript -e "display notification \"Test network $1 is deleted in 15 minutes (testnet.sh extend to keep it)\" with title \"5th Echelon\"" || true
    fi
    sleep 60
  done
}

start_reaper() { # NET
  local log="$CONF_DIR/testnet-$1.log"
  if command -v caffeinate >/dev/null; then
    # Keeps the Mac from idle sleep until the network is gone (not a closed lid).
    nohup caffeinate -i "$0" _reap "$1" >>"$log" 2>&1 &
  else
    nohup "$0" _reap "$1" >>"$log" 2>&1 &
  fi
  say "the watcher (pid $!) deletes it at its end; its log: $log"
}

# --- The UDP check -------------------------------------------------------------------------

# The relay's largest packets travel as two IP fragments, and a provider that drops fragments
# breaks co-op loading for relayed players: each machine has to take 1473, 1492 and 2000 byte
# datagrams from another (or, with one machine, from this computer).
UDP_RECV='
import socket, sys, time
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(("0.0.0.0", int(sys.argv[1]))); s.settimeout(1)
got = set(); end = time.time() + 12
while time.time() < end:
    try: got.add(len(s.recv(4096)))
    except socket.timeout: pass
print(" ".join(str(n) for n in sorted(got)))
'
UDP_SEND='
import socket, sys, time
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
for _ in range(5):
    for n in (1473, 1492, 2000): s.sendto(b"x" * n, (sys.argv[1], int(sys.argv[2])))
    time.sleep(0.3)
'
udp_check() { # TO_IP FROM_IP|local
  local got
  on "$1" "command -v ufw >/dev/null && ufw status | grep -q active && ufw allow $UDP_CHECK_PORT/udp >/dev/null; true"
  on "$1" "python3 -c '$UDP_RECV' $UDP_CHECK_PORT" >"$CONF_DIR/udp.$$" &
  local recv=$!
  sleep 2
  if [ "$2" = local ]; then
    python3 -c "$UDP_SEND" "$1" "$UDP_CHECK_PORT"
  else
    on "$2" "python3 -c '$UDP_SEND' $1 $UDP_CHECK_PORT"
  fi
  wait "$recv" || true
  got="$(cat "$CONF_DIR/udp.$$")"; rm -f "$CONF_DIR/udp.$$"
  on "$1" "command -v ufw >/dev/null && ufw delete allow $UDP_CHECK_PORT/udp >/dev/null; true"
  if [ "$got" = "1473 1492 2000" ]; then
    say "UDP $2 -> $1: 1473, 1492 and 2000 bytes arrived"
  else
    echo "testnet: UDP $2 -> $1 got only [${got:-nothing}] of 1473 1492 2000: fragmented packets are lost on this path; the relay's co-op loading would fail" >&2
    return 1
  fi
}

# Each machine takes the check from the one before it, round (or from here, with one).
udp_ring() { # NET
  local all roles first prev role n
  all="$(net_instances "$1")"
  roles="$(jq -r '.tags[] | select(startswith("fe-role-")) | ltrimstr("fe-role-")' <<<"$all")"
  n="$(echo $roles | wc -w | tr -d ' ')"
  [ "$n" -gt 0 ] || die "there's no network $1"
  if [ "$n" -eq 1 ]; then
    command -v python3 >/dev/null && udp_check "$(jq -r .ip <<<"$all")" local || say "no python3 here: UDP check skipped"
    return
  fi
  first="" prev=""
  for role in $roles; do
    [ -n "$first" ] || first="$role"
    [ -n "$prev" ] && udp_check "$(ip_of "$all" "$role")" "$(ip_of "$all" "$prev")"
    prev="$role"
  done
  udp_check "$(ip_of "$all" "$first")" "$(ip_of "$all" "$prev")"
}

# --- Installing ----------------------------------------------------------------------------

FILES="${TESTNET_FILES:-}"
DS_BIN="${FILES:-$ROOT/dist}/dedicated_server-linux-x86_64"
CO_BIN="${FILES:-$ROOT/dist}/coordinator-linux-x86_64"
INSTALLER="${FILES:-$ROOT/scripts}/install-server.sh"
COMMON=(--no-auto-update --yes)

copy_files() { # IP
  on "$1" "mkdir -p fe"
  copy_to "$1" "$DS_BIN" "$CO_BIN" "$INSTALLER"
}

run_install() { # IP ARGS...
  local ip="$1"; shift
  local q="" a
  for a in "$@"; do q+=" $(printf '%q' "$a")"; done
  on "$ip" "cd fe && bash install-server.sh$q"
}

ds_args() { echo --binary; echo "/root/fe/$(basename "$DS_BIN")"; }
co_args() { echo --coordinator-binary; echo "/root/fe/$(basename "$CO_BIN")"; }

server_id() { # IP NAME
  local id i
  for i in $(seq 30); do
    id="$(on "$1" "curl -fsS --max-time 5 -H 'Host: $2' http://127.0.0.1/api/info" 2>/dev/null | jq -r '.id // empty' 2>/dev/null || true)"
    [ -n "$id" ] && { echo "$id"; return; }
    sleep 2
  done
  die "no server id from $2 (http://127.0.0.1/api/info on $1)"
}

ticket_key() { # IP
  local k i
  for i in $(seq 30); do
    k="$(on "$1" "journalctl -u 5th-echelon --no-pager | grep -o 'Relay passes are signed with the key [A-Z2-7]*' | tail -1 | awk '{print \$NF}'" || true)"
    [ "${#k}" -eq 52 ] && { echo "$k"; return; }
    sleep 2
  done
  die "the matchmaking server on $1 didn't log its relay-pass key"
}

join_token_to() { # COORD_IP IP
  local tok
  # The token is the last line, under a heading.
  tok="$(on "$1" "cd fe && bash install-server.sh --show-join-token" | tail -1 | tr -d '[:space:]')"
  [ -n "$tok" ] || die "no join token from the coordinator on $1"
  printf '%s' "$tok" | on "$2" "umask 077 && cat > fe/join-token"
}

# IP of ROLE in the network's JSON lines.
ip_of() { jq -r --arg t "fe-role-$2" 'select(.tags | index($t)) | .ip' <<<"$1" | head -1; }
# The city of ROLE's region, for the server directory.
region_of() {
  local code; code="$(jq -r --arg t "fe-role-$2" 'select(.tags | index($t)) | .region' <<<"$1" | head -1)"
  vapi GET "/regions?per_page=500" | jq -r --arg c "$code" '.regions[] | select(.id == $c) | .city' | head -1
}

install_net() { # NET
  local net="$1" all layout
  [ -x "$DS_BIN" ] || [ -f "$DS_BIN" ] || die "no $DS_BIN: build/build.sh server first"
  [ -f "$CO_BIN" ] || die "no $CO_BIN: build/build.sh server first"
  all="$(net_instances "$net")"
  [ -n "$all" ] || die "there's no network $net"
  layout="$(jq -r '[.tags[] | select(startswith("fe-layout-")) | ltrimstr("fe-layout-")] | first' <<<"$(head -1 <<<"$all")")"
  local roles role ip
  roles="$(layout_roles "$layout")"
  for role in $roles; do ip="$(ip_of "$all" "$role")"; say "copying the binaries and installer to $role ($ip)"; copy_files "$ip"; done
  local mrec; mrec="$(match_name "$net")"
  case "$layout" in
    one | pair)
      local a b an cn
      a="$(ip_of "$all" s1)"; an="$(game_name s1 "$net" "$a")"; cn="$(coord_name "$net" "$a")"
      say "installing s1 ($an) with its coordinator ($cn)"
      run_install "$a" "${COMMON[@]}" $(ds_args) $(co_args) --domain "$an" --server-name "test-$net" \
        --region "$(region_of "$all" s1)" --coordinator-domain "$cn"
      if [ "$layout" = pair ]; then
        b="$(ip_of "$all" s2)"; local bn; bn="$(game_name s2 "$net" "$b")"
        join_token_to "$a" "$b"
        say "installing s2 ($bn), joining $cn"
        run_install "$b" "${COMMON[@]}" $(ds_args) --domain "$bn" --server-name "test-$net-2" \
          --region "$(region_of "$all" s2)" --coordinator "https://$cn" --join-token-file /root/fe/join-token
      fi
      ;;
    rehearsal)
      local c m cn mn key id r rip rn
      c="$(ip_of "$all" coord)"; cn="$(coord_name "$net" "$c")"
      m="$(ip_of "$all" mm)"; mn="$(game_name mm "$net" "$m")"
      say "installing the coordinator alone ($cn)"
      run_install "$c" "${COMMON[@]}" $(co_args) --coordinator-domain "$cn" --coordinator-only
      join_token_to "$c" "$m"
      say "installing the matchmaking server ($mn)"
      run_install "$m" "${COMMON[@]}" $(ds_args) --domain "$mn" --server-name "test-$net-mm" \
        --region "$(region_of "$all" mm)" --matchmaking --coordinator "https://$cn" \
        --join-token-file /root/fe/join-token ${mrec:+--mm-record "$mrec"}
      key="$(ticket_key "$m")"; id="$(server_id "$m" "$mn")"
      for r in relay-a relay-b; do
        rip="$(ip_of "$all" "$r")"; rn="$(game_name "$r" "$net" "$rip")"
        join_token_to "$c" "$rip"
        say "installing $r ($rn)"
        run_install "$rip" "${COMMON[@]}" $(ds_args) --domain "$rn" --server-name "test-$net-$r" \
          --region "$(region_of "$all" "$r")" --role relay --ticket-key "$key" --coordinator "https://$cn" \
          --join-token-file /root/fe/join-token ${mrec:+--mm-record "$mrec"}
      done
      say "naming the matchmaking server ($id) on the coordinator"
      run_install "$c" "${COMMON[@]}" $(co_args) --coordinator-domain "$cn" --coordinator-only --matchmaking-server "$id"
      ;;
    regional)
      local e en ecn ean r rip rn
      e="$(ip_of "$all" eu)"; en="$(game_name eu "$net" "$e")"; ecn="$(coord_name "$net" "$e")"; ean="$(admin_name "$net" "$e")"
      say "installing eu ($en) with the coordinator ($ecn) and its admin UI ($ean)"
      run_install "$e" "${COMMON[@]}" $(ds_args) $(co_args) --domain "$en" --server-name "test-$net-eu" \
        --region "$(region_of "$all" eu)" --coordinator-domain "$ecn" --metrics-domain "$ean"
      for r in na oceania; do
        rip="$(ip_of "$all" "$r")"; rn="$(game_name "$r" "$net" "$rip")"
        join_token_to "$e" "$rip"
        say "installing $r ($rn), joining $ecn"
        run_install "$rip" "${COMMON[@]}" $(ds_args) --domain "$rn" --server-name "test-$net-$r" \
          --region "$(region_of "$all" "$r")" --coordinator "https://$ecn" --join-token-file /root/fe/join-token
      done
      ;;
  esac
  for role in $roles; do on "$(ip_of "$all" "$role")" "rm -f fe/join-token" || true; done
  say "installed: network $net"
  show_net "$net"
}

# --- Showing -------------------------------------------------------------------------------

plan_prices() { vapi GET "/plans?per_page=500" | jq -c '[.plans[] | {key: .id, value: .monthly_cost}] | from_entries'; }

show_net() { # [NET]
  local prices now
  prices="$(plan_prices)"; now="$(date +%s)"
  instances | jq -r --argjson p "$prices" --argjson now "$now" --argjson mh "$MONTH_HOURS" --arg net "${1:-}" '
    select($net == "" or (.tags | index("fe-net-" + $net)))
    | (.tags | map(select(startswith("fe-net-")) | ltrimstr("fe-net-")) | first) as $n
    | (.tags | map(select(startswith("fe-role-")) | ltrimstr("fe-role-")) | first) as $r
    | (.tags | map(select(startswith("expires-")) | ltrimstr("expires-") | tonumber) | first) as $e
    | (.created | sub("\\+00:00$"; "Z") | fromdateiso8601) as $c
    | ((($now - $c) / 3600 | ceil) as $h | {h: $h, cost: (($p[.plan] // 0) / $mh * $h)}) as $u
    | [$n, $r, .ip, .region, .plan, .status,
       "\($u.h)h $\($u.cost * 100 | round / 100)",
       (if $e then "ends in \((($e - $now) / 60) | floor)m" else "no end" end)]
    | @tsv' | column -t -s $'\t' || true
}

# --- Commands ------------------------------------------------------------------------------

cmd="${1:-}"; [ -n "$cmd" ] && shift || true
net="test"
case "$cmd" in
  _reap) reap "${1:?}"; exit 0 ;;
esac
sweep

case "$cmd" in
  up)
    layout="${1:?up needs a layout: one, pair, rehearsal or regional}"; shift
    hours=4 plan="${TESTNET_PLAN:-vc2-1c-1gb}" install=1 overrides=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --name) net="${2:?}"; shift ;;
        --hours) hours="${2:?}"; shift ;;
        --plan) plan="${2:?}"; shift ;;
        --region) overrides+=" ${2:?}"; shift ;;
        --no-install) install=0 ;;
        *) die "unknown option $1" ;;
      esac
      shift
    done
    [[ "$net" =~ ^[a-z0-9]{1,8}$ ]] || die "--name takes 1-8 letters and digits"
    [[ "$hours" =~ ^[0-9]+$ ]] && [ "$hours" -ge 1 ] && [ "$hours" -le 72 ] || die "--hours takes 1 to 72"
    [ -z "$(net_instances "$net" | head -1)" ] || die "network $net is already up (testnet.sh list; down --name $net)"
    roles="$(layout_roles "$layout")"
    for role in $roles; do
      n="$(game_name "$role" "$net" 255.255.255.255)"
      [ "$role" = coord ] || [ "${#n}" -le "$NAME_MAX" ] || die "$n is ${#n} characters; the game takes $NAME_MAX: a shorter TESTNET_DOMAIN or --name"
    done
    os_id="$(vapi GET "/os?per_page=500" | jq -r '[.os[] | select(.name == "Ubuntu 24.04 LTS x64")] | first.id // empty')"
    [ -n "$os_id" ] || die "Vultr has no Ubuntu 24.04 x64 image"
    key_id="$(ssh_key_id)"
    fw_id="$(firewall_group)"
    expires=$(( $(date +%s) + hours * 3600 ))
    say "making network $net ($layout, $plan), deleted by itself at $(date -r "$expires" 2>/dev/null || date -d "@$expires")"
    for role in $roles; do
      region="$(default_region "$layout" "$role")"
      for o in $overrides; do [ "${o%%=*}" = "$role" ] && region="${o#*=}"; done
      body="$(jq -nc --arg r "$region" --arg p "$plan" --argjson os "$os_id" --arg l "fe-$net-$role" \
        --arg k "$key_id" --arg fw "$fw_id" --arg net "$net" --arg role "$role" --arg lay "$layout" --arg e "$expires" --arg tag "$TAG" \
        '{region: $r, plan: $p, os_id: $os, label: $l, hostname: $l, sshkey_id: [$k], firewall_group_id: $fw, backups: "disabled",
          tags: [$tag, "fe-net-" + $net, "fe-role-" + $role, "fe-layout-" + $lay, "expires-" + $e]}')"
      id="$(vapi POST /instances "$body" | jq -r .instance.id)" || { down_net "$net"; die "couldn't make $role in $region; nothing is left running"; }
      say "$role: $id in $region"
    done
    start_reaper "$net"
    say "waiting for the machines to start"
    for _ in $(seq 90); do
      all="$(net_instances "$net")"
      ready="$(jq -s '[.[] | select(.status == "active" and .server_status == "ok" and .ip != "0.0.0.0")] | length' <<<"$all")"
      [ "$ready" -eq "$(echo "$roles" | wc -w)" ] && break
      sleep 5
    done
    [ "$ready" -eq "$(echo "$roles" | wc -w)" ] || die "the machines didn't start in 7 minutes (testnet.sh list; down)"
    say "the machines' addresses: $(jq -r .ip <<<"$all" | tr '\n' ' ')"
    # A machine still booting refuses SSH: a minute's wait (for them all), then one try each 20 s.
    say "waiting a minute for SSH"
    sleep 60
    for role in $roles; do
      ip="$(ip_of "$all" "$role")"
      ssh-keygen -R "$ip" -f "$KNOWN_HOSTS" >/dev/null 2>&1 || true
      ok=0
      for _ in $(seq 12); do on "$ip" true 2>/dev/null && { ok=1; break; }; sleep 20; done
      [ "$ok" -eq 1 ] || die "no SSH to $role ($ip) after 5 minutes: testnet.sh firewall if this computer's address changed; down to delete it"
      on "$ip" "cloud-init status --wait >/dev/null 2>&1; true"
      if [ "$use_cf" -eq 1 ]; then
        [ "$role" = coord ] || cf_record "$(game_name "$role" "$net" "$ip")" "$ip" "$net"
        case "$layout:$role" in one:s1 | pair:s1 | rehearsal:coord | regional:eu) cf_record "$(coord_name "$net" "$ip")" "$ip" "$net" ;; esac
        [ "$layout:$role" = regional:eu ] && cf_record "$(admin_name "$net" "$ip")" "$ip" "$net"
        [ "$layout:$role" = rehearsal:mm ] && cf_record "$(match_name "$net")" "$ip" "$net"
      fi
    done
    udp_ring "$net"
    if [ "$install" -eq 1 ]; then install_net "$net"; else show_net "$net"; fi
    ;;
  check)
    [ "${1:-}" = --name ] && net="${2:?}"
    udp_ring "$net"
    ;;
  install)
    [ "${1:-}" = --name ] && net="${2:?}"
    install_net "$net"
    ;;
  list)
    show_net
    ;;
  ssh)
    role="${1:?ssh needs a role (testnet.sh list)}"; shift
    [ "${1:-}" = --name ] && { net="${2:?}"; shift 2; }
    ip="$(ip_of "$(net_instances "$net")" "$role")"
    [ -n "$ip" ] || die "no $role in network $net"
    exec ssh "${SSH_OPTS[@]}" "root@$ip" "$@"
    ;;
  extend)
    more="${1:?extend needs hours}"; shift
    [ "${1:-}" = --name ] && net="${2:?}"
    [[ "$more" =~ ^[0-9]+$ ]] || die "extend takes whole hours"
    all="$(net_instances "$net")"; [ -n "$all" ] || die "there's no network $net"
    exp="$(head -1 <<<"$all" | expiry_of)"; new=$(( ${exp:-$(date +%s)} + more * 3600 ))
    while read -r line; do
      id="$(jq -r .id <<<"$line")"
      tags="$(jq -c --arg e "expires-$new" '[.tags[] | select(startswith("expires-") | not)] + [$e]' <<<"$line")"
      vapi PATCH "/instances/$id" "{\"tags\": $tags}" >/dev/null
    done <<<"$all"
    say "network $net now ends at $(date -r "$new" 2>/dev/null || date -d "@$new")"
    pgrep -f "_reap $net" >/dev/null || start_reaper "$net"
    ;;
  down)
    if [ "${1:-}" = --all ]; then
      for n in $(instances | jq -r '.tags[] | select(startswith("fe-net-")) | ltrimstr("fe-net-")' | sort -u); do down_net "$n"; done
    else
      [ "${1:-}" = --name ] && net="${2:?}"
      down_net "$net"
    fi
    ;;
  sweep) ;;
  firewall)
    # Brings the group up to date (this computer's address now) and puts the network in it.
    [ "${1:-}" = --name ] && net="${2:?}"
    fw_id="$(firewall_group)"
    for id in $(net_instances "$net" | jq -r .id); do
      vapi PATCH "/instances/$id" "{\"firewall_group_id\": \"$fw_id\"}" >/dev/null && say "$id is in firewall group $FIREWALL_NAME"
    done
    ;;
  regions)
    vapi GET "/regions?per_page=500" | jq -r '.regions[] | [.id, .city, .country] | @tsv' | column -t -s $'\t'
    ;;
  plans)
    vapi GET "/plans?type=vc2&per_page=500" |
      jq -r --argjson mh "$MONTH_HOURS" '.plans[] | [.id, "\(.vcpu_count) CPU", "\(.ram) MB", "$\(.monthly_cost)/mo", "$\(.monthly_cost / $mh * 1000 | round / 1000)/h"] | @tsv' | column -t -s $'\t'
    ;;
  *) die "no command \"$cmd\" (testnet.sh help)" ;;
esac
