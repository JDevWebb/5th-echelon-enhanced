#!/usr/bin/env bash
# Makes a release from main, start to finish, on the machine with the release
# key:
#
#   scripts/release.sh 0.4.1
#
#   1. checks main is clean, up to date with GitHub, and that CI passed on it
#      (pushing main first, and waiting for CI, when it's ahead); CI doesn't run
#      again for the version commits below, nor for release notes;
#   2. sets release.toml's version to 0.4.1 (dropping -dev), commits, and tags
#      v0.4.1;
#   3. pushes main and that one tag (never --tags: other local tags stay local);
#   4. waits for the release workflow to build the draft;
#   5. runs sign-release.sh, which checks the downloads and asks once before
#      signing with the key, then publishes;
#   6. moves main on to 0.4.2-dev and pushes it.
#
# A pre-release (0.5.0-rc.1) stops after step 5: it isn't followed by a -dev.
# Release notes go in docs/releases/v<version>.md, which the workflow puts
# first; it asks before releasing without them.
#
# Pushes go straight to GitHub ($PUSH_URL), so origin can keep pushing
# disabled for everything else. Needs gh signed in, Docker, and the release key
# (see sign-release.sh).
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=${1:?usage: scripts/release.sh <version, e.g. 0.4.1>}
REPO=${REPO:-JDevWebb/5th-echelon-enhanced}
PUSH_URL=${PUSH_URL:-https://github.com/$REPO.git}
TAG="v$VERSION"
die() { echo "error: $*" >&2; exit 1; }
say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
ask() { printf '%s [y/N] ' "$1"; read -r a; [ "$a" = y ] || [ "$a" = Y ]; }

printf '%s' "$VERSION" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?' || die "$VERSION isn't a version like 0.4.1"
case "$VERSION" in *-dev*) die "a release can't be a -dev version" ;; esac
prerelease=0; case "$VERSION" in *-*) prerelease=1 ;; esac

# The run of a workflow for a commit, once GitHub has started it, watched to
# the end. Fails if it didn't succeed.
wait_for() {
  local workflow=$1 sha=$2 id="" _
  for _ in $(seq 60); do
    id=$(gh run list -R "$REPO" --workflow "$workflow" --commit "$sha" --limit 1 --json databaseId --jq '.[0].databaseId // empty')
    [ -n "$id" ] && break
    sleep 10
  done
  [ -n "$id" ] || die "GitHub didn't start the $workflow workflow for $sha"
  echo "$workflow: https://github.com/$REPO/actions/runs/$id"
  gh run watch "$id" -R "$REPO" --exit-status --interval 30 >/dev/null || die "the $workflow run failed: https://github.com/$REPO/actions/runs/$id"
}

say "Checking main"
[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || die "not on main"
[ -z "$(git status --porcelain)" ] || die "the working tree has changes; commit or stash them"
git fetch -q "$PUSH_URL" main
remote=$(git rev-parse FETCH_HEAD)
git merge-base --is-ancestor "$remote" HEAD || die "GitHub's main has commits this one hasn't; pull first"
! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || die "$TAG already exists here"
[ -z "$(git ls-remote --tags "$PUSH_URL" "refs/tags/$TAG")" ] || die "$TAG already exists on GitHub"
current=$(sed -n 's/^version = "\(.*\)"$/\1/p' release.toml)
[ -n "$current" ] || die "no version in release.toml"
echo "release.toml: $current -> $VERSION"
if [ ! -f "docs/releases/$TAG.md" ]; then
  ask "No release notes in docs/releases/$TAG.md (only the verification notes then). Release anyway?" || exit 1
fi
scripts/check-clean.sh

if [ "$(git rev-parse HEAD)" != "$remote" ]; then
  say "Pushing main ($(git rev-list --count "$remote..HEAD") commits) and waiting for CI"
  git push "$PUSH_URL" HEAD:refs/heads/main
fi
# CI skips pushes that only change release.toml or release notes (ci.yml): what it checked
# is the newest commit that changed anything else.
tested=$(git log -1 --format=%H -- . ':(exclude)release.toml' ':(exclude)docs/releases')
wait_for CI "$tested"

say "Tagging $TAG"
sed -i.bak "s/^version = \".*\"$/version = \"$VERSION\"/" release.toml && rm -f release.toml.bak
grep -qx "version = \"$VERSION\"" release.toml || die "couldn't set the version in release.toml"
git commit -q -m "Release $TAG" release.toml
git tag -a "$TAG" -m "$TAG"
ask "Push main and $TAG to GitHub (this starts the release build)?" || { echo "Not pushed. To undo: git tag -d $TAG && git reset --hard HEAD~1"; exit 1; }
git push "$PUSH_URL" HEAD:refs/heads/main "refs/tags/$TAG"

say "Waiting for the release workflow to build the draft"
wait_for Release "$(git rev-parse "$TAG^{commit}")"

say "Signing"
PUBLISH=yes scripts/sign-release.sh "$TAG"

if [ "$prerelease" -eq 0 ]; then
  IFS=. read -r major minor patch <<<"$VERSION"
  next="$major.$minor.$((patch + 1))-dev"
  say "Moving main on to $next"
  sed -i.bak "s/^version = \".*\"$/version = \"$next\"/" release.toml && rm -f release.toml.bak
  git commit -q -m "Version $next" release.toml
  git push "$PUSH_URL" HEAD:refs/heads/main
fi
git fetch -q origin 2>/dev/null || true
say "Released $TAG: https://github.com/$REPO/releases/tag/$TAG"
echo "Coordinators roll it out to their servers on their own: follow it in the admin UI (Updates)."
