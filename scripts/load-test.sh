#!/usr/bin/env bash
# Load-tests a server running in a container limited like a small VPS, and
# samples its CPU, memory and network while the test players run.
#
#   scripts/load-test.sh <folder with dedicated_server and testbot, in the build image> [load options]
#
#   CPUS=1 MEMORY=1g scripts/load-test.sh /target/native/release --players 200 --relayed 20
#
# Load options: see tools/testbot/src/load.rs (--players, --relayed,
# --match-size, --pps, --bytes, --duration, --connect, --activity).
# Used by `build/build.sh load`.
set -euo pipefail
bin=${1:?folder with dedicated_server and testbot}; shift
image=${IMAGE:-fes-build:local}
cpus=${CPUS:-1}
memory=${MEMORY:-1g}
net=fes-load-test
cleanup() { docker rm -f fes-load-srv >/dev/null 2>&1 || true; docker network rm "$net" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup
docker network create "$net" >/dev/null

# The server: default settings, but every test player comes from one
# address, so the per-address limits on accounts and logins are lifted.
docker run -d --name fes-load-srv --network "$net" --cpus "$cpus" --memory "$memory" -e FE_MAX_CONNECTIONS_PER_IP=100000 \
  --ulimit nofile=65536:65536 -v fes-target:/target:ro "$image" bash -c "
  set -e; mkdir -p /srv/fe && cd /srv/fe && cp $bin/dedicated_server .
  ./dedicated_server >gen.log 2>&1 & gen=\$!
  for _ in \$(seq 100); do [ -s service.toml ] && break; sleep 0.1; done
  sleep 0.3; kill \$gen 2>/dev/null || true; wait \$gen 2>/dev/null || true
  sed -i -E 's/^registrations_per_hour = .*/registrations_per_hour = 1000000/; s/^failed_logins_per_10_minutes = .*/failed_logins_per_10_minutes = 1000000/; s/^logins_per_10_minutes = .*/logins_per_10_minutes = 1000000/' service.toml
  exec ./dedicated_server --public-address \$(hostname -i | cut -d' ' -f1) >server.log 2>&1
" >/dev/null
ip=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$net\").IPAddress}}" fes-load-srv)
# Wait for the configured server (the first run only writes the defaults).
for _ in $(seq 100); do docker exec fes-load-srv test -s /srv/fe/server.log 2>/dev/null && break; sleep 0.2; done
for _ in $(seq 100); do docker exec fes-load-srv bash -c 'exec 3<>/dev/tcp/127.0.0.1/50051' 2>/dev/null && break; sleep 0.2; done

echo "server: $cpus CPU, $memory memory (docker limits)"
# Sample the server every 2 seconds: "time cpu% rss-kB".
samples=$(mktemp) marks=$(mktemp)
( while docker inspect fes-load-srv >/dev/null 2>&1; do
    cpu=$(docker stats --no-stream --format '{{.CPUPerc}}' fes-load-srv 2>/dev/null | tr -d '%')
    rss=$(docker exec fes-load-srv sed -n 's/^VmRSS:[[:space:]]*\([0-9]*\) kB/\1/p' /proc/1/status 2>/dev/null)
    echo "$(date +%s) ${cpu:-0} ${rss:-0}" >> "$samples"
    sleep 2
  done ) &
sampler=$!

# The test's own output, with the times play starts and ends.
rc=0
docker run --rm --network "$net" --ulimit nofile=65536:65536 -v fes-target:/target:ro "$image" \
  "$bin/testbot" --server "$ip" load "$@" 2>&1 | while IFS= read -r line; do
    case "$line" in matches:*) echo "start $(date +%s)" >> "$marks" ;; "--- summary") echo "end $(date +%s)" >> "$marks" ;; esac
    printf '%s\n' "$line"
  done || rc=$?
[ "${PIPESTATUS[0]}" -eq 0 ] || rc=1
kill "$sampler" 2>/dev/null || true
wait "$sampler" 2>/dev/null || true

echo "--- server"
play_start=$(sed -n 's/^start //p' "$marks" | head -1)
play_end=$(sed -n 's/^end //p' "$marks" | head -1)
stat() { # stat <from> <to> <column>: average and peak of the samples between
  awk -v a="${1:-0}" -v b="${2:-9999999999}" -v c="$3" '$1>=a && $1<=b {s+=$c; n++; if ($c>m) m=$c} END {if (n) printf "%.1f %.1f", s/n, m; else print "? ?"}' "$samples"
}
read -r play_cpu play_cpu_peak <<< "$(stat "$play_start" "$play_end" 2)"
read -r all_cpu all_cpu_peak <<< "$(stat 0 9999999999 2)"
read -r _ rss_peak <<< "$(stat 0 9999999999 3)"
read -r play_rss _ <<< "$(stat "$play_start" "$play_end" 3)"
echo "CPU during play: average ${play_cpu}%, peak ${play_cpu_peak}% (100% = one core)"
echo "CPU overall: average ${all_cpu}%, peak ${all_cpu_peak}% (the sign-ins are the peak)"
echo "memory (resident): during play $(awk -v k="$play_rss" 'BEGIN {printf "%.0f", k/1024}') MB, peak $(awk -v k="$rss_peak" 'BEGIN {printf "%.0f", k/1024}') MB"
docker exec fes-load-srv grep -o 'NAT relay: [^"]*' /srv/fe/server.log 2>/dev/null | tail -3 | sed 's/^/server: /' || true
if [ $rc -ne 0 ]; then echo "--- server log"; docker exec fes-load-srv tail -20 /srv/fe/server.log || true; fi
[ -n "${SHOW_SAMPLES:-}" ] && awk '{printf "%s cpu %s%% rss %.0f MB\n", $1, $2, $3/1024}' "$samples"
rm -f "$samples" "$marks"
exit $rc
