#!/usr/bin/env bash
# Global matchmaking (the 0.5.0 plan's G1, items 5 and 7), in containers: a coordinator that
# names the network's matchmaking server, that server (signing relay passes), and a relay-only
# server trusting its passes. Test players sign in to the matchmaking server, register at the
# relay with their passes, and the matchmaking server gives the relay's addresses for them; the
# relay carries traffic only between players the matchmaking server put together
# (tools/testbot's cross-relay). Signing in through a relay (0.5.5's SR): a third relay behind
# Caddy, as install-server.sh sets one up, passes the API on to the matchmaking server and its
# tunnel carries the game's sign-in there (relay-sign-in). A second relay: players at the two
# reach each other, each relay forwarding to the other (cross-relay-forward). Then the rest of the network around them:
# a game without a session key (0.4.7) relayed by the matchmaking server's own helper (nat-relay),
# a keyed one (nat-key), and an ordinary regional server in the same network, with friends across
# it and the matchmaking server (federation).
#
#   scripts/relay-test.sh <folder with dedicated_server, testbot and coordinator, in the build image>
#
# Used by `build/build.sh relay-test`.
set -euo pipefail
bin=${1:?folder with dedicated_server, testbot and coordinator}
image=${IMAGE:-fes-build:local}
mount=${BIN_MOUNT:--v fes-target:/target:ro}
net=fes-relay-test
here=$(cd "$(dirname "$0")/.." && pwd)
cleanup() { docker rm -f fes-relay-coord fes-relay-mm fes-relay-syd fes-relay-eu fes-relay-region fes-relay-sr fes-relay-sr-caddy >/dev/null 2>&1 || true; docker network rm "$net" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup
docker network create "$net" >/dev/null

docker run -d --name fes-relay-coord --network "$net" $mount "$image" \
  bash -c "mkdir -p /srv/c && exec $bin/coordinator --listen 0.0.0.0:8700 --data /srv/c --check-private-hosts --matchmaking-server mm-test >/srv/c/log 2>&1" >/dev/null
for _ in $(seq 100); do docker exec fes-relay-coord test -s /srv/c/join-token.txt 2>/dev/null && break; sleep 0.2; done
token=$(docker exec fes-relay-coord cat /srv/c/join-token.txt | tr -d '[:space:]')
coord=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$net\").IPAddress}}" fes-relay-coord)

server() { # server <container> <server id> <extra service.toml>
  docker run -d --name "$1" --network "$net" $mount "$image" bash -c "
    set -e; mkdir -p /srv/fe && cd /srv/fe && cp $bin/dedicated_server .
    echo $2 > server-id.txt
    ./dedicated_server >gen.log 2>&1 & gen=\$!
    for _ in \$(seq 100); do [ -s service.toml ] && break; sleep 0.1; done
    sleep 0.3; kill \$gen 2>/dev/null || true; wait \$gen 2>/dev/null || true
    printf '\n[federation]\ncoordinator = \"http://$coord:8700\"\njoin_token = \"$token\"\nname = \"$2\"\nregion = \"Test\"\nallow_http = true\n$3' >> service.toml
    exec ./dedicated_server --public-address \$(hostname -i | cut -d' ' -f1) >server.log 2>&1
  " >/dev/null
}
ip() { docker inspect -f "{{(index .NetworkSettings.Networks \"$net\").IPAddress}}" "$1"; }

server fes-relay-mm mm-test '\n[matchmaking]\nnetwork_relays = true\n'
key=
for _ in $(seq 100); do
  key=$(docker exec fes-relay-mm grep -ho 'Relay passes are signed with the key [A-Z2-7]*' /srv/fe/server.log 2>/dev/null | awk '{print $NF}' || true)
  [ -n "$key" ] && break; sleep 0.2
done
[ -n "$key" ] || { echo "FAIL the matchmaking server logged no relay-pass key"; docker exec fes-relay-mm tail -20 /srv/fe/server.log; exit 1; }
server fes-relay-syd relay-test "\n[role]\nkind = \"relay\"\nticket_key = \"$key\"\nmoved_to = \"mm.example.test\"\n"
server fes-relay-eu relay-eu "\n[role]\nkind = \"relay\"\nticket_key = \"$key\"\n"
server fes-relay-region region-test ''
mm=$(ip fes-relay-mm) syd=$(ip fes-relay-syd) eu=$(ip fes-relay-eu) region=$(ip fes-relay-region)

# A relay behind Caddy, as install-server.sh sets one up (0.5.5's SR): its web parts and API on
# loopback, the tunnel on 127.0.0.1:21130, and Caddy's routes from the installer's own
# site_routes, the API passed on to the matchmaking server (h2c here, its HTTPS name there).
docker run -d --name fes-relay-sr --network "$net" $mount "$image" bash -c "
  set -e; mkdir -p /srv/fe && cd /srv/fe && cp $bin/dedicated_server .
  echo relay-sr > server-id.txt
  ./dedicated_server >gen.log 2>&1 & gen=\$!
  for _ in \$(seq 100); do [ -s service.toml ] && break; sleep 0.1; done
  sleep 0.3; kill \$gen 2>/dev/null || true; wait \$gen 2>/dev/null || true
  sed -i -E \
    -e 's|^listen = \"0.0.0.0:80\"|listen = \"127.0.0.1:8080\"|' \
    -e 's|^listen = \"0.0.0.0:8000\"|listen = \"127.0.0.1:8000\"|' \
    -e 's|^api_server = .*|api_server = \"127.0.0.1:50051\"|' \
    service.toml
  if grep -q '^\[nat\]' service.toml; then sed -i '/^\[nat\]/a tunnel = \"127.0.0.1:21130\"' service.toml; else printf '\n[nat]\ntunnel = \"127.0.0.1:21130\"\n' >> service.toml; fi
  printf '\n[federation]\ncoordinator = \"http://$coord:8700\"\njoin_token = \"$token\"\nname = \"relay-sr\"\nregion = \"Test\"\nallow_http = true\n[public]\nhost = \"fes-relay-sr\"\napi = 80\ncontent = 80\n[role]\nkind = \"relay\"\nticket_key = \"$key\"\nsign_in = true\n' >> service.toml
  exec ./dedicated_server --public-address \$(hostname -i | cut -d' ' -f1) >server.log 2>&1
" >/dev/null
sr=$(ip fes-relay-sr)
caddyfile=$(mktemp)
# shellcheck disable=SC1090 # the installer's own routes
eval "$(sed -n '/^site_routes() {/,/^}/p' "$here/scripts/install-server.sh")"
{
  printf '{\n\tservers :80 {\n\t\tprotocols h1 h2c\n\t}\n}\n:80 {\n'
  site_routes mm.example.test | sed -e "/handle @grpc {/,/^\t}/ s|reverse_proxy https://mm.example.test|reverse_proxy h2c://$mm:50051|" \
    -e "s|reverse_proxy https://mm.example.test|reverse_proxy http://$mm:80|" -e '/header_up Host mm.example.test/d'
  printf '}\n'
} >"$caddyfile"
grep -q 'header_up X-5E-Relayed-For {remote_host}' "$caddyfile" || { echo "FAIL the installer's relay routes don't pass the player's address on"; exit 1; }
docker create --name fes-relay-sr-caddy --network container:fes-relay-sr caddy:2 >/dev/null
docker cp "$caddyfile" fes-relay-sr-caddy:/etc/caddy/Caddyfile >/dev/null
rm -f "$caddyfile"
# After the configured server is up: its first run, writing the defaults, holds port 80.
for _ in $(seq 100); do docker exec fes-relay-sr test -s /srv/fe/server.log 2>/dev/null && break; sleep 0.2; done
docker start fes-relay-sr-caddy >/dev/null
for s in fes-relay-mm fes-relay-region; do
  for _ in $(seq 100); do docker exec "$s" bash -c 'exec 3<>/dev/tcp/127.0.0.1/50051' 2>/dev/null && break; sleep 0.2; done
done

client() { docker run --rm --network "$net" $mount "$image" "$@"; }
rc=0
echo "--- the directory lists the three relays"
n=0
for _ in $(seq 60); do
  n=$(client curl -sf "http://$coord:8700/v1/servers" | grep -o '"role":"relay"' | wc -l || true)
  [ "$n" -ge 3 ] && break; sleep 2
done
[ "$n" -ge 3 ] && echo "PASS the three relays listed" || { echo "FAIL helpers: $n"; client curl -s "http://$coord:8700/v1/servers"; rc=1; }
# Links come with each member's next heartbeat (every 30 s) once both are listed.
sleep 35
echo "--- test players across the relay"
client "$bin/testbot" --server "$mm" --other "$syd" cross-relay || rc=1
echo "--- test players at two relays, forwarded between them"
client "$bin/testbot" --server "$mm" --other "$syd" --third "$eu" cross-relay-forward || rc=1
echo "--- a guest relayed at one relay joining a host who's direct at another (0.5.4)"
client "$bin/testbot" --server "$mm" --other "$syd" --third "$eu" cross-relay-direct-host || rc=1
echo "--- signing in through a relay (0.5.5): the API through its Caddy, the game's sign-in through its tunnel"
client "$bin/testbot" --server "$mm" --other "$sr" relay-sign-in || rc=1
echo "--- only a relay whose Caddy passes sign-in on says relay-sign-in ([role] sign_in, set by the installer)"
says() { client curl -sf "http://$1/api/info" | python3 -c 'import json,sys; print("relay-sign-in" in json.load(sys.stdin).get("features",[]))' 2>/dev/null || echo error; }
s_sr=$(says "$sr") s_syd=$(says "$syd")
[ "$s_sr" = True ] && [ "$s_syd" = False ] && echo "PASS relay-sign-in: the SR relay True, the Sydney relay (no sign_in) False" || { echo "FAIL relay-sign-in: the SR relay '$s_sr', the Sydney relay '$s_syd'"; rc=1; }
echo "--- the add-on's HTTP through the relay (SR): /mm/ on the relay is the matchmaking server's"
mmid=$(client curl -sf "http://$sr/mm/api/info" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))' 2>/dev/null || true)
own=$(client curl -sf "http://$mm/api/info" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))' 2>/dev/null || true)
[ -n "$mmid" ] && [ "$mmid" = "$own" ] && echo "PASS the relay's /mm/api/info is the matchmaking server's ($mmid)" || { echo "FAIL /mm/api/info through the relay: '$mmid', the matchmaking server's: '$own'"; rc=1; }
echo "--- the game's HTTP through the relay (SR): its online config and content are the matchmaking server's"
cfg=$(client curl -sf "http://$sr/OnlineConfigService.svc/GetOnlineConfig" || true)
case "$cfg" in *"address=$mm;"*) echo "PASS the relay's online config is the matchmaking server's (sign-in at $mm)" ;; *) echo "FAIL the online config through the relay: ${cfg:0:160}"; rc=1 ;; esac
a=$(client curl -sf "http://$sr/mp_balancing.ini" | md5sum) b=$(client curl -sf "http://$mm/mp_balancing.ini" | md5sum)
[ "$a" = "$b" ] && echo "PASS the relay's content is the matchmaking server's" || { echo "FAIL content through the relay differs"; rc=1; }
echo "--- a game without a session key (0.4.7), relayed by the matchmaking server's own helper"
client "$bin/testbot" --server "$mm" nat-relay || rc=1
echo "--- a keyed game (0.5.0) at the matchmaking server's own helper, with its relay pass"
client "$bin/testbot" --server "$mm" nat-key || rc=1
echo "--- an ordinary regional server beside them: its own helper relays, and friends span it and the matchmaking server"
client "$bin/testbot" --server "$region" nat-relay || rc=1
client "$bin/testbot" --server "$mm" --other "$region" federation || rc=1
echo "--- the relays say they hear from the matchmaking server (its standby asks them)"
seen=""
for _ in $(seq 20); do
  seen=$(client curl -sf "http://$syd/api/info" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("matchmaking_seen",""))' 2>/dev/null || true)
  [ -n "$seen" ] && [ "$seen" -le 30 ] && break; sleep 1
done
[ -n "$seen" ] && [ "$seen" -le 30 ] && echo "PASS a relay's /api/info has matchmaking_seen ($seen s)" || { echo "FAIL matchmaking_seen: '$seen'"; rc=1; }
moved=$(client curl -sf "http://$syd/api/info" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("moved_to",""))' 2>/dev/null || true)
[ "$moved" = mm.example.test ] && echo "PASS a relay that was a game server says where its players moved (moved_to)" || { echo "FAIL moved_to: '$moved'"; rc=1; }
echo "--- the relay's session events reach the matchmaking server"
events=0
for _ in $(seq 20); do
  events=$(docker exec fes-relay-mm python3 -c "import sqlite3; print(sqlite3.connect('/srv/fe/5th-echelon.db').execute(\"SELECT COUNT(*) FROM session_events WHERE kind = 'nat' AND detail LIKE '%relay-test%'\").fetchone()[0])" 2>/dev/null || echo 0)
  [ "$events" -ge 3 ] && break; sleep 1
done
[ "$events" -ge 3 ] && echo "PASS the relay's registrations are in the matchmaking server's session events" || { echo "FAIL relay events: $events"; rc=1; }
if [ $rc -ne 0 ]; then
  for s in fes-relay-mm fes-relay-syd fes-relay-eu fes-relay-region fes-relay-sr; do echo "--- $s (last 40 lines)"; docker exec "$s" tail -40 /srv/fe/server.log; done
  echo "--- fes-relay-sr-caddy"; docker logs --tail 20 fes-relay-sr-caddy 2>&1 || true
fi
exit $rc
