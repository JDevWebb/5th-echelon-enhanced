#!/usr/bin/env bash
# Fails if anything committed mentions names, addresses or files that belong
# to private projects rather than to this one. Run before every push (see
# scripts/install-hooks.sh). Checks every file of HEAD and every commit
# message that isn't upstream's.
set -euo pipefail
cd "$(dirname "$0")/.."

pattern='lanfire|warpline|10\.77\.|\bember\b|heliosit'
rc=0

if git grep -n -I -i -E "$pattern" HEAD -- . ':!scripts/check-clean.sh'; then
  echo "check-clean: the files above mention private names" >&2
  rc=1
fi

base=$(git merge-base HEAD upstream/main 2>/dev/null || true)
range=${base:+$base..}HEAD
if git log --format='%h %s%n%b' "$range" | grep -i -E "$pattern"; then
  echo "check-clean: the commit messages above mention private names" >&2
  rc=1
fi

[ $rc -eq 0 ] && echo "check-clean: ok"
exit $rc
