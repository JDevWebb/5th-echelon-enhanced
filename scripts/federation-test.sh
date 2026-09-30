#!/usr/bin/env bash
# Two servers sharing friends through a coordinator (docs/friends.md), in
# containers: a friendship made on one reaches the other, a block on the
# other comes back, and both servers are in the coordinator's directory.
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
    printf '\n[federation]\ncoordinator = \"http://$coord:8700\"\njoin_token = \"$token\"\nname = \"$2\"\nregion = \"Test\"\n' >> service.toml
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
if [ $rc -ne 0 ]; then
  for s in fes-fed-a fes-fed-b; do echo "--- $s"; docker exec "$s" grep -i federation /srv/fe/server.log | tail -20 || true; done
  echo "--- coordinator"; docker exec fes-fed-coord tail -30 /srv/c/log || true
fi
exit $rc
