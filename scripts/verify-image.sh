#!/bin/sh
# Checks a release's server image: that the release key signed its digest
# (the IMAGE file published with the release, and IMAGE.sig), as
# scripts/sign-release.sh does when it signs the release. Prints the image by
# that digest, to run exactly what was signed whatever its tags say:
#
#   scripts/verify-image.sh v0.4.3
#   docker run ... "$(scripts/verify-image.sh v0.4.3)"
#
# Needs curl, OpenSSL 3 and base32 (Linux has them). The release key is the
# one install-server.sh, next to this script, carries.
set -eu
TAG=${1:?usage: scripts/verify-image.sh <tag>}
REPO=${REPO:-JDevWebb/5th-echelon-enhanced}
VERSION=${TAG#v}
die() { echo "error: $*" >&2; exit 1; }
[ "$TAG" = "v$VERSION" ] && printf '%s' "$VERSION" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?' \
  || die "$TAG isn't a release tag like v0.4.3"
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
base=${RELEASE_BASE:-https://github.com/$REPO/releases/download/$TAG}
curl -fsSL --max-filesize 4096 -o "$work/IMAGE" "$base/IMAGE" || die "$TAG has no signed server image (IMAGE)"
curl -fsSL --max-filesize 4096 -o "$work/IMAGE.sig" "$base/IMAGE.sig" || die "$TAG's server image has no signature (IMAGE.sig)"
LC_ALL=C grep -Eqx 'ghcr\.io/[a-z0-9._/-]+@sha256:[0-9a-f]{64}' "$work/IMAGE" && [ "$(wc -l < "$work/IMAGE")" -eq 1 ] \
  || die "IMAGE isn't one line, <image>@sha256:<digest>"
sed -n '/^RELEASE_KEY_PEM="/,/END PUBLIC KEY/p' "$here/install-server.sh" | sed 's/^RELEASE_KEY_PEM="//; s/"$//' > "$work/release.pem"
grep -q 'BEGIN PUBLIC KEY' "$work/release.pem" || die "no release key in $here/install-server.sh"
{ printf '5th-echelon/image/v1\n%s\n' "$VERSION"; cat "$work/IMAGE"; } > "$work/signed"
sig=$(tr -d '[:space:]' < "$work/IMAGE.sig")
while [ $(( ${#sig} % 8 )) -ne 0 ]; do sig="$sig="; done
printf '%s' "$sig" | base32 -d > "$work/signature" 2>/dev/null || die "IMAGE.sig isn't a signature"
openssl pkeyutl -verify -pubin -inkey "$work/release.pem" -rawin -in "$work/signed" -sigfile "$work/signature" >/dev/null 2>&1 \
  || die "IMAGE.sig doesn't verify against the release key: don't run that image"
echo "The release key signed $TAG's server image:" >&2
cat "$work/IMAGE"
