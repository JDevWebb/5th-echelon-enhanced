#!/bin/sh
# Signs a release and publishes it. The release workflow makes each release
# as a draft; the launcher installs an update only when its SHA256SUMS
# carries the release key's signature (SHA256SUMS.sig), so a draft stays
# unpublished until someone with the key signs it here.
#
#   scripts/sign-release.sh v0.4.0
#
# The key lives outside the repo, in RELEASE_KEY
# (default ~/.config/5th-echelon-release/release.key); make one with
#   docker run ... cargo run -p identity --bin release-sign -- keygen <file>
# and put its public half in RELEASE_KEYS in launcher/src/updater.rs.
# Needs the gh CLI signed in to the release's repository, and Docker.
set -eu
cd "$(dirname "$0")/.."
TAG=${1:?usage: scripts/sign-release.sh <tag>}
KEY=${RELEASE_KEY:-$HOME/.config/5th-echelon-release/release.key}
REPO=${REPO:-JDevWebb/5th-echelon-enhanced}
[ -f "$KEY" ] || { echo "No release key at $KEY" >&2; exit 1; }
IMAGE=fes-build:local
docker build -q -t "$IMAGE" -f build/Dockerfile . >/dev/null

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh release download "$TAG" --repo "$REPO" --dir "$work"
rm -f "$work/SHA256SUMS.sig"
# Check every asset against SHA256SUMS before vouching for it.
(cd "$work" && sha256sum -c SHA256SUMS)
keydir=$(dirname "$KEY")
docker run --rm -v "$PWD":/src -w /src \
  -v fe-cargo-registry:/usr/local/cargo/registry -v fes-target:/target -e CARGO_TARGET_DIR=/target/native \
  -v "$keydir":/keys:ro -v "$work":/release \
  "$IMAGE" cargo run -q -p identity --bin release-sign -- sign "/keys/$(basename "$KEY")" /release/SHA256SUMS "${TAG#v}"
gh release upload "$TAG" "$work/SHA256SUMS.sig" --repo "$REPO" --clobber
printf 'Publish %s now? [y/N] ' "$TAG"
read -r answer
case "$answer" in
  y|Y) gh release edit "$TAG" --repo "$REPO" --draft=false && echo "Published $TAG." ;;
  *) echo "Signed; still a draft." ;;
esac
