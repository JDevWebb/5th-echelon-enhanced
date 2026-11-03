#!/usr/bin/env bash
# Global matchmaking (the 0.4.8 plan's G1, items 5 and 7), in containers: a coordinator that
# names the network's matchmaking server, that server (signing relay passes), and a relay-only
# server trusting its passes. Test players sign in to the matchmaking server, register at the
# relay with their passes, and the matchmaking server gives the relay's addresses for them; the
# relay carries traffic only between players the matchmaking server put together
# (tools/testbot's cross-relay). A second relay: players at the two reach each other, each
# relay forwarding to the other (cross-relay-forward). Then the rest of the network around them:
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
cleanup() { docker rm -f fes-relay-coord fes-relay-mm fes-relay-syd fes-relay-eu fes-relay-region >/dev/null 2>&1 || true; docker network rm "$net" >/dev/null 2>&1 || true; }
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
server fes-relay-syd relay-test "\n[role]\nkind = \"relay\"\nticket_key = \"$key\"\n"
server fes-relay-eu relay-eu "\n[role]\nkind = \"relay\"\nticket_key = \"$key\"\n"
server fes-relay-region region-test ''
mm=$(ip fes-relay-mm) syd=$(ip fes-relay-syd) eu=$(ip fes-relay-eu) region=$(ip fes-relay-region)
for s in fes-relay-mm fes-relay-region; do
  for _ in $(seq 100); do docker exec "$s" bash -c 'exec 3<>/dev/tcp/127.0.0.1/50051' 2>/dev/null && break; sleep 0.2; done
done

client() { docker run --rm --network "$net" $mount "$image" "$@"; }
rc=0
echo "--- the directory lists both relays"
n=0
for _ in $(seq 60); do
  n=$(client curl -sf "http://$coord:8700/v1/servers" | grep -o '"role":"relay"' | wc -l || true)
  [ "$n" -ge 2 ] && break; sleep 2
done
[ "$n" -ge 2 ] && echo "PASS both relays listed" || { echo "FAIL helpers: $n"; client curl -s "http://$coord:8700/v1/servers"; rc=1; }
# Links come with each member's next heartbeat (every 30 s) once both are listed.
sleep 35
echo "--- test players across the relay"
client "$bin/testbot" --server "$mm" --other "$syd" cross-relay || rc=1
echo "--- test players at two relays, forwarded between them"
client "$bin/testbot" --server "$mm" --other "$syd" --third "$eu" cross-relay-forward || rc=1
echo "--- a game without a session key (0.4.7), relayed by the matchmaking server's own helper"
client "$bin/testbot" --server "$mm" nat-relay || rc=1
echo "--- a keyed game (0.4.8) at the matchmaking server's own helper, with its relay pass"
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
echo "--- the relay's session events reach the matchmaking server"
events=0
for _ in $(seq 20); do
  events=$(docker exec fes-relay-mm python3 -c "import sqlite3; print(sqlite3.connect('/srv/fe/5th-echelon.db').execute(\"SELECT COUNT(*) FROM session_events WHERE kind = 'nat' AND detail LIKE '%relay-test%'\").fetchone()[0])" 2>/dev/null || echo 0)
  [ "$events" -ge 3 ] && break; sleep 1
done
[ "$events" -ge 3 ] && echo "PASS the relay's registrations are in the matchmaking server's session events" || { echo "FAIL relay events: $events"; rc=1; }
if [ $rc -ne 0 ]; then
  for s in fes-relay-mm fes-relay-syd fes-relay-eu fes-relay-region; do echo "--- $s (last 40 lines)"; docker exec "$s" tail -40 /srv/fe/server.log; done
fi
exit $rc
