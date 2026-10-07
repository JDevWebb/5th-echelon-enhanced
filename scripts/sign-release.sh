#!/bin/sh
# Signs a release and publishes it. The release workflow makes each release
# as a draft; the launcher, the installer and the servers' updater install a
# release only when its SHA256SUMS carries the release key's signature for
# that version (SHA256SUMS.sig), so a draft stays unpublished until someone
# with the key signs it here.
#
#   scripts/sign-release.sh v0.4.0
#
# Before signing, it checks, and prints for you to compare with the CI run:
#   - the tag's commit, which must be on main on GitHub;
#   - every download against SHA256SUMS (their hashes);
#   - each download's build provenance (gh attestation verify: built by this
#     repository's release workflow, from this tag), when gh can.
#
# The key never meets the build: the signer (identity's release-sign) is
# built first, without the key; then only that binary runs, in a container
# with no network, the key mounted read-only and nothing else but a copy of
# SHA256SUMS. Its signature is then checked with OpenSSL against the public
# key install-server.sh carries, as the installer checks it.
#
# The server's Docker image, which the release workflow pushed only as
# :<version>-unsigned, is signed with the release too: its digest (the image
# exactly, whatever the tag points at later) is checked against its build
# provenance (this repository's workflow, at the tag), written to IMAGE and
# signed with the same key, for its own purpose (identity::image_message), as
# IMAGE.sig; both go up with the release (scripts/verify-image.sh checks
# them). Once published, that digest gets the image's real tags (:<version>,
# and :latest for a release). That needs gh signed in with the write:packages
# scope (gh auth refresh -s write:packages); otherwise it prints how.
#
# The key lives outside the repo, in RELEASE_KEY
# (default ~/.config/5th-echelon-release/release.key); make one with
#   docker run ... cargo run -p identity --bin release-sign -- keygen <file>
# and put its public half in RELEASE_KEYS in identity/src/lib.rs (and its
# PEM in install-server.sh).
# Needs the gh CLI signed in to the release's repository, and Docker.
set -eu
cd "$(dirname "$0")/.."
TAG=${1:?usage: scripts/sign-release.sh <tag>}
KEY=${RELEASE_KEY:-$HOME/.config/5th-echelon-release/release.key}
REPO=${REPO:-JDevWebb/5th-echelon-enhanced}
VERSION=${TAG#v}
die() { echo "error: $*" >&2; exit 1; }
[ "$TAG" = "v$VERSION" ] && printf '%s' "$VERSION" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?' \
  || die "$TAG isn't a release tag like v0.4.0"
[ -f "$KEY" ] || die "no release key at $KEY"
IMAGE=fes-build:local
docker build -q -t "$IMAGE" -f build/Dockerfile . >/dev/null

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/release" "$work/bin" "$work/sign"

# The tag's commit, on main.
commit=$(gh api "repos/$REPO/commits/$TAG" --jq .sha) || die "GitHub doesn't know $TAG"
on_main=$(gh api "repos/$REPO/compare/main...$commit" --jq .status) || die "couldn't compare $commit with main"
case "$on_main" in
  identical|behind) ;;
  *) die "$TAG ($commit) isn't on main ($on_main); not signing it" ;;
esac
echo "$TAG is commit $commit, on main."

gh release download "$TAG" --repo "$REPO" --dir "$work/release"
# What signing adds (from a run before that didn't publish): made again below.
rm -f "$work/release/SHA256SUMS.sig" "$work/release/IMAGE" "$work/release/IMAGE.sig"
# SHA256SUMS exactly as sha256sum writes it, as launchers read it: every line
# "<sha256>  <file>", each download once, and nothing else. A loose line (a
# leading space, a tab) is skipped by some sha256sum -c with only a warning,
# so the check below would pass a line a looser reader could take.
sums="$work/release/SHA256SUMS"
[ -f "$sums" ] || die "the release has no SHA256SUMS"
if LC_ALL=C grep -Evx '[0-9a-f]{64}  [A-Za-z0-9._-]+' "$sums" | grep -q .; then
  die "SHA256SUMS has a line that isn't \"<sha256>  <file>\"; not signing it"
fi
if LC_ALL=C grep -q "$(printf '\r')" "$sums"; then die "SHA256SUMS has Windows line endings; not signing it"; fi
listed=$(cut -c67- "$sums" | LC_ALL=C sort)
[ -z "$(printf '%s\n' "$listed" | uniq -d)" ] || die "SHA256SUMS lists a file twice; not signing it"
present=$(cd "$work/release" && find . -mindepth 1 -maxdepth 1 ! -name SHA256SUMS -exec basename {} \; | LC_ALL=C sort)
[ "$listed" = "$present" ] || die "SHA256SUMS doesn't list exactly the release's downloads; not signing it"
# Check every download against SHA256SUMS before vouching for it, with GNU
# sha256sum (the build image), which refuses what it can't read.
docker run --rm --network none -v "$work/release":/release:ro -w /release "$IMAGE" sha256sum --strict -c SHA256SUMS
echo
echo "SHA256SUMS (compare with the release workflow's \"Collect and checksum\" step):"
cat "$work/release/SHA256SUMS"
echo
if gh attestation verify --help 2>/dev/null | grep -q -- '--source-ref'; then
  for f in "$work/release"/*; do
    [ "$(basename "$f")" = SHA256SUMS ] && continue
    gh attestation verify "$f" --repo "$REPO" --source-ref "refs/tags/$TAG" >/dev/null \
      || die "$(basename "$f") has no build provenance from $REPO's workflow at $TAG"
  done
  echo "Every download's build provenance checks out (this repository's workflow, at $TAG)."
else
  echo "This gh can't check build provenance (gh attestation verify --source-ref); skipped."
fi
# The server's image, by digest, built by this repository's workflow from this tag.
owner=$(printf '%s' "${REPO%%/*}" | tr '[:upper:]' '[:lower:]')
image="ghcr.io/$owner/5th-echelon-server"
digest=$(docker buildx imagetools inspect "$image:$VERSION-unsigned" --format '{{json .Manifest}}' 2>/dev/null \
  | grep -o '"digest":"sha256:[0-9a-f]\{64\}"' | head -1 | cut -d'"' -f4 || true)
if [ -z "$digest" ]; then
  printf '%s:%s-unsigned isn'"'"'t there. Sign the release without its server image? [y/N] ' "$image" "$VERSION"
  read -r answer
  case "$answer" in y|Y) ;; *) echo "Not signed."; exit 1 ;; esac
else
  if gh attestation verify --help 2>/dev/null | grep -q -- '--source-ref'; then
    gh attestation verify "oci://$image@$digest" --repo "$REPO" --source-ref "refs/tags/$TAG" >/dev/null \
      || die "$image@$digest has no build provenance from $REPO's workflow at $TAG; not signing it"
    echo "The server image $image@$digest: built by this repository's workflow, at $TAG."
  else
    # Without gh's check, its revision label, exactly (the workflow sets it to the commit).
    revision=$(docker buildx imagetools inspect "$image@$digest" --format '{{json .Image}}' 2>/dev/null \
      | grep -o '"org.opencontainers.image.revision":"[0-9a-f]\{40\}"' | head -1 | cut -d'"' -f4 || true)
    [ "$revision" = "$commit" ] || die "$image@$digest says it was built from ${revision:-no commit}, not $commit; not signing it"
    echo "The server image $image@$digest: labelled with $commit (this gh can't check its provenance)."
  fi
  printf '%s@%s\n' "$image" "$digest" > "$work/sign/IMAGE"
fi
printf 'Sign %s (commit %s)? [y/N] ' "$TAG" "$commit"
read -r answer
case "$answer" in y|Y) ;; *) echo "Not signed."; exit 1 ;; esac

# The signer, built without the key, from the lock file only, and from
# scratch (no shared caches, which another build could have left anything in).
docker run --rm -v "$PWD":/src:ro -w /src -e CARGO_TARGET_DIR=/tmp/target -v "$work/bin":/out \
  "$IMAGE" sh -c 'cargo build -q --locked --release -p identity --bin release-sign && cp /tmp/target/release/release-sign /out/'
# Only that binary runs with the key: no network, no caches or sources, and
# nothing to write but the signature.
cp "$work/release/SHA256SUMS" "$work/sign/SHA256SUMS"
# shellcheck disable=SC2016
docker run --rm --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
  -v "$work/bin/release-sign":/release-sign:ro -v "$KEY":/key:ro -v "$work/sign":/release \
  "$IMAGE" sh -c '/release-sign sign /key /release/SHA256SUMS "$1" && if [ -f /release/IMAGE ]; then /release-sign sign-image /key /release/IMAGE "$1"; fi' sign "$VERSION"
# A signature, and nothing else, for this version and SHA256SUMS, by the key
# install-server.sh carries: checked with OpenSSL, as the installer does.
grep -Eqx '[A-Z2-7]{103}' "$work/sign/SHA256SUMS.sig" || die "the signer wrote something that isn't a signature; not uploading it"
[ ! -f "$work/sign/IMAGE" ] || grep -Eqx '[A-Z2-7]{103}' "$work/sign/IMAGE.sig" || die "the signer wrote something that isn't the image's signature; not uploading it"
sed -n '/^RELEASE_KEY_PEM="/,/END PUBLIC KEY/p' scripts/install-server.sh | sed 's/^RELEASE_KEY_PEM="//; s/"$//' > "$work/sign/release.pem"
# shellcheck disable=SC2016
docker run --rm --network none -v "$work/sign":/release:ro "$IMAGE" sh -c '
  set -e
  cd /tmp
  { printf "5th-echelon/release/v2\n%s\n" "$1"; cat /release/SHA256SUMS; } > signed
  sig=$(tr -d "[:space:]" < /release/SHA256SUMS.sig)
  while [ $(( ${#sig} % 8 )) -ne 0 ]; do sig="$sig="; done
  printf "%s" "$sig" | base32 -d > signature
  openssl pkeyutl -verify -pubin -inkey /release/release.pem -rawin -in signed -sigfile signature >/dev/null
  if [ -f /release/IMAGE ]; then
    { printf "5th-echelon/image/v1\n%s\n" "$1"; cat /release/IMAGE; } > signed
    sig=$(tr -d "[:space:]" < /release/IMAGE.sig)
    while [ $(( ${#sig} % 8 )) -ne 0 ]; do sig="$sig="; done
    printf "%s" "$sig" | base32 -d > signature
    openssl pkeyutl -verify -pubin -inkey /release/release.pem -rawin -in signed -sigfile signature >/dev/null
  fi' verify "$VERSION" \
  || die "the signature doesn't verify against install-server.sh's release key; not uploading it"
echo "Signed $TAG ($VERSION), and checked the signature."

if [ -f "$work/sign/IMAGE" ]; then
  gh release upload "$TAG" "$work/sign/SHA256SUMS.sig" "$work/sign/IMAGE" "$work/sign/IMAGE.sig" --repo "$REPO" --clobber
else
  gh release upload "$TAG" "$work/sign/SHA256SUMS.sig" --repo "$REPO" --clobber
fi
# PUBLISH=yes (scripts/release.sh) publishes without asking again.
if [ "${PUBLISH:-}" = yes ]; then
  answer=y
else
  printf 'Publish %s now? [y/N] ' "$TAG"
  read -r answer
fi
case "$answer" in
  y|Y) gh release edit "$TAG" --repo "$REPO" --draft=false && echo "Published $TAG." ;;
  *) echo "Signed; still a draft. Run this again to publish it (and tag its image)."; exit 0 ;;
esac

# The server's image: the digest signed above gets its real tags.
[ -n "$digest" ] || exit 0
case "$VERSION" in *-*) latest="" ;; *) latest="$image:latest" ;; esac
image_help="Tag it by hand once gh has the scope (gh auth refresh -s write:packages):
  gh auth token | docker login ghcr.io -u \$(gh api user --jq .login) --password-stdin
  docker buildx imagetools create --tag $image:$VERSION${latest:+ --tag $latest} $image@$digest"
if ! gh auth token | docker login ghcr.io -u "$(gh api user --jq .login)" --password-stdin >/dev/null 2>&1; then
  echo "Couldn't sign in to ghcr.io with gh's token, so the server image isn't tagged. $image_help"
  exit 0
fi
if docker buildx imagetools create --tag "$image:$VERSION" ${latest:+--tag "$latest"} "$image@$digest"; then
  echo "Tagged the server image $image@$digest as :$VERSION${latest:+ and :latest}."
else
  echo "Couldn't tag the server image (gh's token needs write:packages). $image_help"
fi
