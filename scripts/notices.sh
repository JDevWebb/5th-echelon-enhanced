#!/bin/sh
# THIRD-PARTY-NOTICES.txt for a release: build.sh notices runs it in the build image, the release
# workflow on its runner. Needs cargo-about (pinned in build/Dockerfile and release.yml), Node, and
# coordinator/admin-ui/node_modules (npm ci).
#
#   scripts/notices.sh OUT [WORK_DIR]
#
# Each released program's crates, for the targets it's released for (about.toml), as cargo-about
# JSON in WORK_DIR, merged by build/notices.js with the admin UI's npm packages.
set -eu
out=${1:?the file to write}
work=${2:-$(mktemp -d)}
mkdir -p "$work"
about() { # NAME MANIFEST TARGET... (with --features before the targets when needed)
  name=$1 manifest=$2
  shift 2
  echo "notices: $name" >&2
  cargo about generate --locked -c about.toml -m "$manifest" --format json -o "$work/$name.json" "$@" 2>"$work/$name.log" ||
    { cat "$work/$name.log" >&2; exit 1; }
}
about hooks hooks/Cargo.toml --target i686-pc-windows-msvc
about dedicated_server dedicated_server/Cargo.toml --target x86_64-pc-windows-msvc --target x86_64-unknown-linux-gnu
about coordinator coordinator/Cargo.toml --target x86_64-unknown-linux-gnu
about testbot tools/testbot/Cargo.toml --target x86_64-pc-windows-msvc
about map_geometry tools/map_geometry/Cargo.toml --target x86_64-pc-windows-msvc
about launcher launcher-slint/Cargo.toml --features embed-dll --target x86_64-pc-windows-msvc --target x86_64-unknown-linux-gnu
node build/notices.js "$out" coordinator/admin-ui hooks/fonts "$work"/*.json
