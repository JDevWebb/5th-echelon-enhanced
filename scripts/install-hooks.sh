#!/usr/bin/env bash
# Installs a pre-push hook that runs scripts/check-clean.sh, so nothing that
# mentions private names can be pushed. There's no CI; this is the guard.
set -euo pipefail
cd "$(dirname "$0")/.."
hook=$(git rev-parse --git-path hooks/pre-push)
cat > "$hook" <<'HOOK'
#!/usr/bin/env bash
exec "$(git rev-parse --show-toplevel)/scripts/check-clean.sh"
HOOK
chmod +x "$hook"
echo "installed $hook"
