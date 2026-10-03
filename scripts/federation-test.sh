#!/usr/bin/env bash
# Two servers sharing friends through a coordinator (docs/friends.md), in
# containers: a friendship made on one reaches the other, a block on the
# other comes back, and both servers are in the coordinator's directory. Then
# the admin UI's side: a server's players reach the coordinator, and an admin's
# ban goes out to the server and comes back done.
#
#   scripts/federation-test.sh <folder with dedicated_server, testbot and coordinator, in the build image>
#
# Used by `build/build.sh federation-test`.
set -euo pipefail
bin=${1:?folder with dedicated_server, testbot and coordinator}
image=${IMAGE:-fes-build:local}
net=fes-fed-test
cleanup() { docker rm -f fes-fed-coord fes-fed-a fes-fed-b >/dev/null 2>&1 || true; docker network rm "$net" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup
docker network create "$net" >/dev/null

docker run -d --name fes-fed-coord --network "$net" -v fes-target:/target:ro "$image" \
  bash -c "mkdir -p /srv/c && exec $bin/coordinator --listen 0.0.0.0:8700 --data /srv/c >/srv/c/log 2>&1" >/dev/null
for _ in $(seq 100); do docker exec fes-fed-coord test -s /srv/c/join-token.txt 2>/dev/null && break; sleep 0.2; done
token=$(docker exec fes-fed-coord cat /srv/c/join-token.txt | tr -d '[:space:]')
coord=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$net\").IPAddress}}" fes-fed-coord)

server() { # server <name> <label>
  docker run -d --name "$1" --network "$net" -v fes-target:/target:ro "$image" bash -c "
    set -e; mkdir -p /srv/fe && cd /srv/fe && cp $bin/dedicated_server .
    ./dedicated_server >gen.log 2>&1 & gen=\$!
    for _ in \$(seq 100); do [ -s service.toml ] && break; sleep 0.1; done
    sleep 0.3; kill \$gen 2>/dev/null || true; wait \$gen 2>/dev/null || true
    grep -q '^\[federation\]' service.toml || printf '\n[federation]\ncoordinator = \"http://$coord:8700\"\njoin_token = \"$token\"\nname = \"$2\"\nregion = \"Test\"\nallow_http = true\n' >> service.toml
    exec ./dedicated_server --public-address \$(hostname -i | cut -d' ' -f1) >server.log 2>&1
  " >/dev/null
}
server fes-fed-a "Server A"
server fes-fed-b "Server B"
ip() { docker inspect -f "{{(index .NetworkSettings.Networks \"$net\").IPAddress}}" "$1"; }
a=$(ip fes-fed-a) b=$(ip fes-fed-b)
for s in fes-fed-a fes-fed-b; do
  for _ in $(seq 100); do docker exec "$s" bash -c 'exec 3<>/dev/tcp/127.0.0.1/50051' 2>/dev/null && break; sleep 0.2; done
done

client() { docker run --rm --network "$net" -v fes-target:/target:ro "$image" "$@"; }
rc=0
echo "--- directory"
for _ in $(seq 30); do
  n=$(client curl -sf "http://$coord:8700/v1/servers" | grep -o '"name":"Server [AB]"' | wc -l || true)
  [ "$n" -eq 2 ] && break; sleep 1
done
client curl -sf "http://$coord:8700/v1/servers"; echo
[ "${n:-0}" -eq 2 ] && echo "PASS directory lists both servers" || { echo "FAIL directory"; rc=1; }
echo "--- test players"
client "$bin/testbot" --server "$a" --other "$b" federation || rc=1
echo "--- metrics"
# Each server reports its metrics as it starts, then every minute.
servers_reporting=$(docker exec fes-fed-coord python3 -c "import sqlite3; print(sqlite3.connect('/srv/c/coordinator.db').execute('SELECT COUNT(DISTINCT server_id) FROM samples').fetchone()[0])" 2>/dev/null || echo 0)
[ "$servers_reporting" -eq 2 ] && echo "PASS both servers report metrics" || { echo "FAIL metrics: $servers_reporting server(s) reported"; rc=1; }
echo "--- the coordinator moves to another address"
# Server A is pointed at the same coordinator by another name, and restarted: it must join
# again on its own (with its secret), not be locked out.
docker exec fes-fed-a sed -i "s#coordinator = \"http://$coord:8700\"#coordinator = \"http://fes-fed-coord:8700\"#" /srv/fe/service.toml
docker restart fes-fed-a >/dev/null
moved=0
for _ in $(seq 60); do
  if docker exec fes-fed-a grep -qh "Federation: joined http://fes-fed-coord:8700" /srv/fe/server.log /srv/fe/gen.log 2>/dev/null; then moved=1; break; fi
  sleep 1
done
[ "$moved" -eq 1 ] && echo "PASS server A joined again at the new address" || { echo "FAIL server A didn't join at the new address"; rc=1; }
echo "--- players and admin actions"
db() { # db <container> <database> <sql>: the first column of the first row
  docker exec "$1" python3 -c "import sqlite3,sys; r=sqlite3.connect(sys.argv[1]).execute(sys.argv[2]).fetchone(); print('' if r is None else r[0])" "$2" "$3"
}
coord_db() { db fes-fed-coord /srv/c/coordinator.db "$1"; }
coord_write() { # coord_write <sql>: runs it and commits; prints the first column of the first row
  docker exec fes-fed-coord python3 -c "import sqlite3,sys; c=sqlite3.connect('/srv/c/coordinator.db'); r=c.execute(sys.argv[1]).fetchone(); c.commit(); print('' if r is None else r[0])" "$1"
}
server_a=$(coord_db "SELECT id FROM servers WHERE json_extract(listing, '$.name') = 'Server A'")
# Server A sends everyone as it starts (it just restarted, after the test players made accounts).
players=0
for _ in $(seq 60); do
  players=$(coord_db "SELECT COUNT(*) FROM players WHERE server_id = '$server_a'")
  [ "${players:-0}" -gt 0 ] && break; sleep 1
done
[ "${players:-0}" -gt 0 ] && echo "PASS server A's $players players reached the coordinator" || { echo "FAIL no players from server A"; rc=1; }
player=$(coord_db "SELECT id FROM players WHERE server_id = '$server_a' ORDER BY id LIMIT 1")
if [ -n "$player" ]; then
  action=$(coord_write "INSERT INTO player_actions (server_id, player, kind, args, created_by, created_at) VALUES ('$server_a', $player, 'ban', '{\"reason\":\"federation test\"}', 'test', CAST(strftime('%s','now') AS INTEGER)) RETURNING id")
  status=pending
  for _ in $(seq 60); do
    status=$(coord_db "SELECT status FROM player_actions WHERE id = ${action:-0}")
    [ "$status" != pending ] && break; sleep 1
  done
  banned=$(db fes-fed-a /srv/fe/5th-echelon.db "SELECT COUNT(*) FROM bans WHERE user_id = $player AND reason = 'federation test'")
  [ "$status" = done ] && [ "${banned:-0}" -eq 1 ] && echo "PASS an admin's ban reached server A and came back done" \
    || { echo "FAIL the ban: status ${status:-none}, banned ${banned:-0}"; rc=1; }
fi
if [ $rc -ne 0 ]; then
  for s in fes-fed-a fes-fed-b; do echo "--- $s"; docker exec "$s" grep -iE -A3 "federation|ERRO" /srv/fe/server.log | tail -40 || true; done
  echo "--- coordinator"; docker exec fes-fed-coord tail -30 /srv/c/log || true
fi
exit $rc
