#!/usr/bin/env bash
# Runs the test players (tools/testbot) against a fresh server in a
# temporary folder. Used by `build/build.sh bots` and by CI.
#
#   scripts/bots.sh <folder with dedicated_server and testbot> [scenario ...]
#
# FRIENDS_MODE=mutual runs the server with friends-only lists (for the
# friends-mutual scenario); REQUIRE_IDENTITY=1 with `[limits]
# require_identity` (for identity-required).
set -euo pipefail
bin=$(cd "$1" && pwd); shift
dir=$(mktemp -d) && cd "$dir"
trap 'rm -rf "$dir"' EXIT
cp "$bin/dedicated_server" .
# The first start writes the default service.toml; stop it once written.
./dedicated_server >gen.log 2>&1 & gen=$!
for _ in $(seq 100); do [ -s service.toml ] && break; sleep 0.1; done
sleep 0.3; kill $gen 2>/dev/null || true; wait $gen 2>/dev/null || true
# The bots connect from 127.0.0.1: trust it, as a server trusts its VPN.
sed -i -E "s|^(storage_host = \".*\")$|\1\ntrusted_subnet = \"127.0.0.0/8\"|" service.toml
# Port 80 needs root, which CI's runner isn't: the config server moves up.
sed -i 's|^listen = "0.0.0.0:80"$|listen = "127.0.0.1:8080"|' service.toml
grep -q '^listen = "127.0.0.1:8080"$' service.toml || { echo "couldn't move the config server off port 80"; exit 1; }
if [ -n "${FRIENDS_MODE:-}" ]; then
  sed -i "/^\[friends\]/,/^\[/ s/^mode = .*/mode = \"$FRIENDS_MODE\"/" service.toml
  grep -q "^mode = \"$FRIENDS_MODE\"" service.toml || { echo "couldn't set [friends] mode"; exit 1; }
fi
if [ -n "${REQUIRE_IDENTITY:-}" ]; then
  sed -i "/^\[limits\]/,/^\[/ s/^require_identity = .*/require_identity = true/" service.toml
  grep -q "^require_identity = true" service.toml || { echo "couldn't set [limits] require_identity"; exit 1; }
fi
RUST_LOG=info ./dedicated_server >server.log 2>&1 & srv=$!
for _ in $(seq 100); do (echo >/dev/tcp/127.0.0.1/50051) 2>/dev/null && break; sleep 0.1; done
rc=0; "$bin/testbot" "$@" || rc=$?
kill $srv 2>/dev/null || true; wait $srv 2>/dev/null || true
if [ $rc -ne 0 ]; then echo "--- server log (last 60 lines)"; tail -60 server.log; fi
exit $rc
