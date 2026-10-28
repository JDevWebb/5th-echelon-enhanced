#!/usr/bin/env bash
# The test players against a fresh server with delay (and loss) added between them, as for
# players far from the one matchmaking server (docs/MATCHMAKING-PLAN.md, experiment E2).
# Runs inside the build image with NET_ADMIN; `build/build.sh latency` does that.
#
#   scripts/latency-test.sh <folder with dedicated_server and testbot> [scenario ...]
#
# PROFILES lists what's added on loopback, one profile per run, as tc netem arguments with
# commas for spaces; every packet passes loopback once, so the round trip is twice the delay.
# The default: none, then 150, 220 and 300 ms round trips (EU, NA and Oceania to one server
# in North America, roughly), then 220 ms with jitter and 1% loss.
set -euo pipefail
bin=${1:?folder with dedicated_server and testbot}; shift
profiles=${PROFILES:-"none delay,75ms delay,110ms delay,150ms delay,110ms,10ms,loss,1%"}
here=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
trap 'tc qdisc del dev lo root 2>/dev/null || true; rm -rf "$out"' EXIT

summary=()
for p in $profiles; do
  tc qdisc del dev lo root 2>/dev/null || true
  args=${p//,/ }
  [ "$p" = none ] || tc qdisc add dev lo root netem $args
  echo "=== $p"
  start=$(date +%s)
  rc=0
  "$here/bots.sh" "$bin" "$@" >"$out/run.log" 2>&1 || rc=$?
  secs=$(( $(date +%s) - start ))
  grep -E '^(PASS|FAIL)' "$out/run.log" || true
  pass=$(grep -c '^PASS' "$out/run.log" || true)
  fail=$(grep -c '^FAIL' "$out/run.log" || true)
  [ "$rc" -eq 0 ] || grep -E '^FAIL' -A3 "$out/run.log" | head -40 || true
  summary+=("$(printf '%-30s %3s passed  %3s failed  %5s s' "$p" "$pass" "$fail" "$secs")")
done
echo
echo "=== summary"
printf '%s\n' "${summary[@]}"
