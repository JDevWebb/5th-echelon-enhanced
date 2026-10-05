#!/usr/bin/env bash
# Two servers sharing friends through a coordinator (docs/friends.md), in
# containers: a friendship made on one reaches the other, a block on the
# other comes back, and both servers are in the coordinator's directory. Then
# the admin UI's side: a server's players reach the coordinator, and an admin's
# ban goes out to the server and comes back done. Then global stats: a stat
# written on server A reaches the coordinator and server B's leaderboards. Last,
# a player's report sent to server A reaches the coordinator with the server's side.
#
#   scripts/federation-test.sh <folder with dedicated_server, testbot and coordinator, in the build image>
#
# Used by `build/build.sh federation-test`.
set -euo pipefail
bin=${1:?folder with dedicated_server, testbot and coordinator}
image=${IMAGE:-fes-build:local}
# Where the containers find the programs: the build volume (build.sh), or BIN_MOUNT, e.g.
# "-v $PWD/target/debug:/bin-ci:ro" with the folder /bin-ci (CI).
mount=${BIN_MOUNT:--v fes-target:/target:ro}
net=fes-fed-test
cleanup() { docker rm -f fes-fed-coord fes-fed-a fes-fed-b >/dev/null 2>&1 || true; docker network rm "$net" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup
docker network create "$net" >/dev/null

docker run -d --name fes-fed-coord --network "$net" $mount "$image" \
  bash -c "mkdir -p /srv/c && exec $bin/coordinator --listen 0.0.0.0:8700 --data /srv/c >/srv/c/log 2>&1" >/dev/null
for _ in $(seq 100); do docker exec fes-fed-coord test -s /srv/c/join-token.txt 2>/dev/null && break; sleep 0.2; done
token=$(docker exec fes-fed-coord cat /srv/c/join-token.txt | tr -d '[:space:]')
coord=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$net\").IPAddress}}" fes-fed-coord)

server() { # server <name> <label>
  docker run -d --name "$1" --network "$net" $mount "$image" bash -c "
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

client() { docker run --rm --network "$net" $mount "$image" "$@"; }
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
  [ "$status" = "done" ] && [ "${banned:-0}" -eq 1 ] && echo "PASS an admin's ban reached server A and came back done" \
    || { echo "FAIL the ban: status ${status:-none}, banned ${banned:-0}"; rc=1; }
fi
echo "--- global stats"
# A ladder kill for a player with an identity on server A, as the game's write leaves it.
global_id=$(db fes-fed-a /srv/fe/5th-echelon.db "SELECT global_id FROM users WHERE global_id IS NOT NULL ORDER BY id LIMIT 1")
if [ -n "$global_id" ]; then
  docker exec fes-fed-a python3 -c "import sqlite3,sys; c=sqlite3.connect('/srv/fe/5th-echelon.db'); c.execute('INSERT INTO stats_outbox (global_id, name, board, context, stat, value) VALUES (?, \'Tester\', 17, 1, 100, 42)', (sys.argv[1],)); c.commit()" "$global_id"
  kills=""
  for _ in $(seq 60); do
    kills=$(coord_db "SELECT value FROM global_stats WHERE global_id = '$global_id' AND board = 17 AND context = 1 AND stat = 100")
    [ -n "$kills" ] && break; sleep 1
  done
  [ "${kills%.0}" = 42 ] && echo "PASS server A's stat reached the coordinator" || { echo "FAIL the stat didn't reach the coordinator (${kills:-none})"; rc=1; }
  # Server B fetches the global leaderboards as it starts, then every 5 minutes.
  docker restart fes-fed-b >/dev/null
  top=""
  for _ in $(seq 60); do
    top=$(db fes-fed-b /srv/fe/5th-echelon.db "SELECT top FROM global_leaderboards WHERE leaderboard = 10 AND context = 1")
    [ -n "$top" ] && break; sleep 1
  done
  case "$top" in *"$global_id"*) echo "PASS server B has the global leaderboard with server A's player";; *) echo "FAIL server B's leaderboards: ${top:-none}"; rc=1;; esac
else
  echo "FAIL no player with an identity on server A"; rc=1
fi
echo "--- reports"
client "$bin/testbot" --server "$a" report || rc=1
reports=0
for _ in $(seq 60); do
  reports=$(coord_db "SELECT COUNT(*) FROM player_reports WHERE server_id = '$server_a'")
  [ "${reports:-0}" -ge 5 ] && break; sleep 1
done
with_side=$(coord_db "SELECT COUNT(*) FROM player_reports r JOIN player_report_files f ON f.report_id = r.id WHERE r.server_id = '$server_a' AND r.server_log LIKE '%Reporter%' AND r.summary LIKE '%started%'")
[ "${reports:-0}" -ge 5 ] && [ "${with_side:-0}" -ge 1 ] && echo "PASS server A's $reports reports reached the coordinator, with the server's side and the log" \
  || { echo "FAIL reports: ${reports:-0} arrived, ${with_side:-0} with the server's side and a file"; rc=1; }
echo "--- refused reports"
# The report scenario's refusals (a file named ../uplay.toml, a sixth report in a day) are
# noted, and reach the coordinator.
refused=""
for _ in $(seq 30); do
  refused=$(coord_db "SELECT group_concat(DISTINCT json_extract(detail, '$.reason')) FROM session_events WHERE server_id = '$server_a' AND kind = 'report_refused'")
  case ",$refused," in *,invalid,*) case ",$refused," in *,too_many,*) break;; esac;; esac
  sleep 1
done
case ",$refused," in *,invalid,*) case ",$refused," in *,too_many,*) ok_refused=1;; esac;; esac
[ "${ok_refused:-0}" = 1 ] && echo "PASS server A's refused reports reached the coordinator ($refused)" || { echo "FAIL refused reports: ${refused:-none}"; rc=1; }
echo "--- session events"
# The test players signed in and out on server A: those events reach the coordinator (they're
# sent every few seconds).
kinds=""
for _ in $(seq 30); do
  kinds=$(coord_db "SELECT group_concat(DISTINCT kind) FROM session_events WHERE server_id = '$server_a'")
  case ",$kinds," in *,signin,*) case ",$kinds," in *,signout,*) break;; esac;; esac
  sleep 1
done
case ",$kinds," in *,signin,*) case ",$kinds," in *,signout,*) ok=1;; esac;; esac
[ "${ok:-0}" = 1 ] && echo "PASS server A's session events reached the coordinator ($kinds)" || { echo "FAIL session events: ${kinds:-none}"; rc=1; }
echo "--- relay ping"
# A relayed player joins a direct one's match on server A: the join says how they reach each
# other (both games' round trips to the server, and the relayed round trip).
client "$bin/testbot" --server "$a" relay-ping || rc=1
net=""
for _ in $(seq 30); do
  net=$(coord_db "SELECT json_extract(detail, '$.relayed') || ' ' || json_extract(detail, '$.ping_ms') || ' ' || json_extract(detail, '$.host_ping_ms') || ' ' || json_extract(detail, '$.relay_ms') FROM session_events WHERE server_id = '$server_a' AND kind = 'join' AND json_extract(detail, '$.host_name') LIKE 'PingHost%'")
  [ -n "$net" ] && break; sleep 1
done
case "$net" in "1 "[0-9]*" "[0-9]*" "[0-9]*) echo "PASS the relayed join reached the coordinator with its round trips (relayed, ping, host's, through the relay: $net)";;
  *) echo "FAIL the relayed join's network detail: ${net:-no join}"; rc=1;; esac
if [ $rc -ne 0 ]; then
  for s in fes-fed-a fes-fed-b; do echo "--- $s"; docker exec "$s" grep -iE -A3 "federation|ERRO" /srv/fe/server.log | tail -40 || true; done
  echo "--- coordinator"; docker exec fes-fed-coord tail -30 /srv/c/log || true
fi
exit $rc
