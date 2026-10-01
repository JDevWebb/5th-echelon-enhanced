#!/usr/bin/env bash
# Runs the test players against a server behind Caddy, set up as in
# docs/reverse-proxy.md: HTTP, content and the gRPC API through Caddy on
# port 80 by host name, UDP on moved ports straight to the server. The
# players only know the host name and take the ports from /api/info.
#
#   scripts/proxy-test.sh <folder with dedicated_server and testbot, inside the build image>
#
# Used by `build/build.sh proxy-test`.
set -euo pipefail
bin=${1:?folder with dedicated_server and testbot}
image=${IMAGE:-fes-build:local}
net=fes-proxy-test
host=blacklist.example.com
here=$(cd "$(dirname "$0")/.." && pwd)
cleanup() { docker rm -f fes-proxy-srv fes-proxy-caddy >/dev/null 2>&1 || true; docker network rm "$net" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup
docker network create "$net" >/dev/null

# The server: ports moved off the defaults (as when other services hold
# them), HTTP on loopback only, reachable through Caddy. Every test player
# comes from one address (Caddy passes it on), so new accounts get more room.
docker run -d --name fes-proxy-srv --network "$net" -v fes-target:/target:ro "$image" bash -c "
  set -e; mkdir -p /srv/fe && cd /srv/fe
  # Run from its own folder, apart from the program, as the Linux installer's service does.
  $bin/dedicated_server >gen.log 2>&1 & gen=\$!
  for _ in \$(seq 100); do [ -s service.toml ] && break; sleep 0.1; done
  sleep 0.3; kill \$gen 2>/dev/null || true; wait \$gen 2>/dev/null || true
  sed -i -E \
    -e 's|^listen = \"0.0.0.0:80\"|listen = \"127.0.0.1:8080\"|' \
    -e 's|^listen = \"0.0.0.0:8000\"|listen = \"127.0.0.1:8000\"|' \
    -e 's|^api_server = .*|api_server = \"127.0.0.1:50051\"|' \
    -e 's|0.0.0.0:21126|0.0.0.0:31126|; s|0.0.0.0:21127|0.0.0.0:31127|; s|0.0.0.0:21128|0.0.0.0:31128|' \
    -e 's|^registrations_per_hour = .*|registrations_per_hour = 1000|' \
    service.toml
  printf '\n[public]\nhost = \"$host\"\napi = 80\ncontent = 80\n' >> service.toml
  exec $bin/dedicated_server --public-address \$(hostname -i | cut -d' ' -f1) >server.log 2>&1
"
ip=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$net\").IPAddress}}" fes-proxy-srv)

# Wait for the configured server (the first run only writes the defaults).
for _ in $(seq 100); do docker exec fes-proxy-srv test -s /srv/fe/server.log 2>/dev/null && break; sleep 0.2; done

# Caddy in the server's network namespace, with the documented Caddyfile.
docker run -d --name fes-proxy-caddy --network container:fes-proxy-srv \
  -v "$here/docs/reverse-proxy/Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2 >/dev/null

client() { docker run --rm --network "$net" --add-host "$host:$ip" -v fes-target:/target:ro "$image" "$@"; }
for _ in $(seq 60); do client curl -sf -o /dev/null "http://$host/api/info" && break; sleep 0.5; done

rc=0
echo "--- through Caddy"
client sh -c "
  set -e
  curl -sf http://$host/api/info; echo
  curl -sf http://$host/OnlineConfigService.svc/GetOnlineConfig | grep -q 'port=31126' && echo 'online config: login on 31126'
  curl -sf http://$host/mp_balancing.ini | head -c 40 | grep -q . && echo 'content: served'
  code=\$(curl -s -o /dev/null -w '%{http_code}' --http2-prior-knowledge -X POST -H 'Content-Type: application/grpc' http://$host/users.UsersAdmin/List)
  [ \"\$code\" = 403 ] && echo 'admin API: refused (403)' || { echo \"admin API answered \$code, not 403\"; exit 1; }
" || rc=1
echo "--- test players (host name only)"
client "$bin/testbot" --server "$host" --info login lobby-invite private-match-invite cleanup leave-session abandon-empty duplicate-request lost-push nat-probe nat-relay nat-public-address direct-test || rc=1
if [ $rc -ne 0 ]; then
  echo "--- server log"; docker exec fes-proxy-srv tail -40 /srv/fe/server.log || true
  echo "--- caddy log"; docker logs --tail 40 fes-proxy-caddy || true
fi
exit $rc
