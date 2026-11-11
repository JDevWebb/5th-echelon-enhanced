#!/bin/sh
# Builds and tests 5th Echelon Enhanced locally in Docker (GitHub Actions also runs ci.yml on
# pushes to main and pull requests, and release.yml on v* tags).
#
#   build/build.sh test      workspace tests (all but the Windows-only hooks DLL and launcher)
#   build/build.sh server    Linux x86_64 dedicated_server and coordinator -> dist/
#   build/build.sh linux     launcher-linux-x86_64 (the Slint launcher, DLL embedded) + dedicated_server-linux-x86_64
#                            -> dist/
#   build/build.sh notices   dist/THIRD-PARTY-NOTICES.txt: every library the release files carry and
#                            its licence (scripts/notices.sh, about.toml)
#   build/build.sh sums      dist/SHA256SUMS for a release
#   build/build.sh test-dll  dist/uplay_r1_loader-test.dll: the client DLL with test commands (test clients only)
#   build/build.sh windows   launcher.exe (the Slint launcher, DLL embedded), uplay_r1_loader.dll,
#                            dedicated_server.exe, testbot.exe, map-geometry.exe -> dist/
#   build/build.sh bots [scenario ...]
#                            test players against a fresh local server (tools/testbot)
#   build/build.sh slint-shots
#                            every screen of the launcher in both looks at several window sizes, on
#                            made-up data, on a virtual screen -> dist/slint-shots/*.png
#   build/build.sh slint-docs
#                            the README's and the wiki's screenshots, the launcher's own pages on a
#                            made-up install -> dist/slint-docs/*.png
#   (not released from 0.5.0, kept a release as the fallback: the egui launcher)
#   build/build.sh egui      the egui launcher for Windows and Linux x86_64, the DLL embedded
#                            -> dist/launcher-egui.exe, dist/launcher-egui-linux-x86_64
#   build/build.sh shots     every egui launcher screen at several window sizes -> dist/shots/*.png
#   build/build.sh latency [scenario ...]
#                            the test players with delay and loss added (PROFILES)
#   build/build.sh load [options]
#                            a load test against a server limited to CPUS (1)
#                            and MEMORY (1g), e.g. --players 200 --relayed 20
#   build/build.sh proxy-test
#                            the test players against a server behind Caddy
#                            (docs/reverse-proxy.md), by host name only
#   build/build.sh federation-test
#                            two servers sharing friends through a coordinator
#                            (docs/friends.md)
#   build/build.sh relay-test
#                            global matchmaking: a matchmaking server and a relay
#                            elsewhere, players across it
#   build/build.sh ui        the coordinator's admin UI (Vue, coordinator/admin-ui -> dist/,
#                            embedded in the coordinator; the targets building it run this first)
#   build/build.sh crash-fixes <game folder>
#                            whether every crash fix finds its place in the game's
#                            executables (read-only; the game never goes in the repo)
#   build/build.sh fmt       cargo fmt (the whole workspace, as CI checks it)
#   build/build.sh shell     interactive shell in the build container
#
# Caches (cargo registry, xwin SDK, target dirs) live in Docker volumes.
# The image and target volume are "fes-" so builds never touch another
# checkout's; the download caches are shared (read-mostly, content-addressed).
set -eu
cd "$(dirname "$0")/.."
IMAGE=fes-build:local
docker build -q -t "$IMAGE" -f build/Dockerfile . >/dev/null

run() {
  docker run --rm ${TTY:-} \
    -v "$PWD":/src \
    -v fe-cargo-registry:/usr/local/cargo/registry \
    -v fe-xwin-cache:/root/.cache/cargo-xwin \
    -v fes-target:/target \
    -e CARGO_TARGET_DIR=/target/native \
    -e XWIN_ACCEPT_LICENSE=1 -e XWIN_ARCH=x86,x86_64 \
    "$@"
}

# The admin UI the coordinator embeds (coordinator/admin-ui/dist), built when its sources
# or lock file are newer than the last build. npm's cache lives in a volume too.
ui() {
  run -v fes-npm-cache:/root/.npm "$IMAGE" sh -c '
    set -e
    cd coordinator/admin-ui
    if [ ! -d node_modules ] || [ package-lock.json -nt node_modules ]; then npm ci --ignore-scripts --no-audit --no-fund; touch node_modules; fi
    if [ ! -f dist/index.html ] || [ -n "$(find src index.html public vite.config.js package-lock.json -newer dist/index.html | head -1)" ]; then npm run build; fi'
}

mkdir -p dist
case "${1:-test}" in
  test|server|linux|federation-test|relay-test|notices) ui ;;
esac
case "${1:-test}" in
  ui)
    ui
    ;;
  test)
    # The launcher's screens headless too (its ui-tests feature: never in the launcher as built).
    run "$IMAGE" sh -c 'cargo test --workspace --exclude hooks --exclude umd_browser && cargo test -p launcher --features ui-tests'
    ;;
  server)
    # The node runs on x86_64; cross-compile for it from any host.
    run -e CARGO_TARGET_DIR=/target/amd64 "$IMAGE" \
      sh -c 'cargo build -p dedicated_server -p coordinator --release --target x86_64-unknown-linux-gnu &&
        cp /target/amd64/x86_64-unknown-linux-gnu/release/dedicated_server dist/dedicated_server-linux-x86_64 &&
        cp /target/amd64/x86_64-unknown-linux-gnu/release/coordinator dist/coordinator-linux-x86_64'
    ;;
  test-dll)
    # The client DLL with the test commands (hooks' test-commands feature) for test clients
    # (the gamebox): dist/uplay_r1_loader-test.dll. Never shipped: releases build without it.
    run -e CARGO_TARGET_DIR=/target/win-test "$IMAGE" sh -c '
      cargo xwin build -q -p hooks --release --features test-commands --target i686-pc-windows-msvc &&
      cp /target/win-test/i686-pc-windows-msvc/release/hooks.dll dist/uplay_r1_loader-test.dll'
    ;;
  windows)
    run -e CARGO_TARGET_DIR=/target/win "$IMAGE" sh -c '
      set -e
      cargo xwin build -p hooks --release --target i686-pc-windows-msvc
      cp /target/win/i686-pc-windows-msvc/release/hooks.dll dist/uplay_r1_loader.dll
      # libsodium-sys picks its bundled MSVC library by the *host*, so point it
      # there by hand when cross-compiling (the linker wants sodium.lib).
      cargo fetch -q
      mkdir -p /tmp/sodium
      cp "$(ls -d /usr/local/cargo/registry/src/*/libsodium-sys-0.2.7 | head -1)/msvc/x64/Release/v142/libsodium.lib" /tmp/sodium/sodium.lib
      # The launcher (launcher-slint, its own workspace), carrying the client DLL, as launcher.exe:
      # the name the updater fetches.
      (cd launcher-slint && SODIUM_LIB_DIR=/tmp/sodium HOOKS_DLL=/target/win/i686-pc-windows-msvc/release/hooks.dll CARGO_TARGET_DIR=/target/launcher-slint \
        cargo xwin build --release --features embed-dll --target x86_64-pc-windows-msvc)
      cp /target/launcher-slint/x86_64-pc-windows-msvc/release/launcher-slint.exe dist/launcher.exe
      # The server for Windows, which the launcher can run.
      SODIUM_LIB_DIR=/tmp/sodium cargo xwin build -p dedicated_server --release --target x86_64-pc-windows-msvc
      cp /target/win/x86_64-pc-windows-msvc/release/dedicated_server.exe dist/dedicated_server.exe
      # The test bots, for running against the node from a Windows test PC.
      SODIUM_LIB_DIR=/tmp/sodium cargo xwin build -p testbot --release --target x86_64-pc-windows-msvc
      cp /target/win/x86_64-pc-windows-msvc/release/testbot.exe dist/testbot.exe
      # The replay viewer'"'"'s map files, made by an admin from their own game.
      cargo xwin build -p map_geometry --release --target x86_64-pc-windows-msvc
      cp /target/win/x86_64-pc-windows-msvc/release/map-geometry.exe dist/map-geometry.exe'
    ;;
  bots)
    shift
    # Every scenario on a default server, then the friends-only one on a
    # server in the "mutual" mode, and one on a server requiring identities.
    run "$IMAGE" bash -c 'cargo build -q -p dedicated_server -p testbot && scripts/bots.sh /target/native/debug "$@" &&
      if [ $# -eq 0 ]; then FRIENDS_MODE=mutual scripts/bots.sh /target/native/debug friends-mutual &&
        REQUIRE_IDENTITY=1 scripts/bots.sh /target/native/debug identity-required &&
        MATCHMAKING=1 scripts/bots.sh /target/native/debug relay-pass; fi' bots "$@"
    ;;
  egui)
    # The egui launcher (launcher/), not released from 0.5.0: the fallback, kept a release, for
    # Windows and Linux x86_64, the DLL embedded -> dist/launcher-egui.exe, launcher-egui-linux-x86_64.
    [ -f dist/uplay_r1_loader.dll ] || { echo "build.sh windows first (the launcher carries its DLL)"; exit 1; }
    run -e CARGO_TARGET_DIR=/target/win "$IMAGE" sh -c '
      set -e
      cargo fetch -q
      mkdir -p /tmp/sodium
      cp "$(ls -d /usr/local/cargo/registry/src/*/libsodium-sys-0.2.7 | head -1)/msvc/x64/Release/v142/libsodium.lib" /tmp/sodium/sodium.lib
      SODIUM_LIB_DIR=/tmp/sodium HOOKS_DLL=/src/dist/uplay_r1_loader.dll cargo xwin build -p launcher --release --features embed-dll --target x86_64-pc-windows-msvc
      cp /target/win/x86_64-pc-windows-msvc/release/launcher.exe dist/launcher-egui.exe'
    run -e CARGO_TARGET_DIR=/target/amd64 -e HOOKS_DLL=/src/dist/uplay_r1_loader.dll "$IMAGE" sh -c '
      cargo build -p launcher --release --features embed-dll --target x86_64-unknown-linux-gnu &&
      cp /target/amd64/x86_64-unknown-linux-gnu/release/launcher dist/launcher-egui-linux-x86_64'
    ;;
  slint-shots)
    # Every screen of the Slint launcher in both looks at several window sizes -> dist/slint-shots
    # (launcher-slint/src/shots.rs): made-up data, Slint's software renderer, on an X server in
    # memory (build/Dockerfile.slint). Built with the network, run with none.
    docker build -q -t fes-slint:local -f build/Dockerfile.slint build >/dev/null
    rm -rf dist/slint-shots
    run -e CARGO_TARGET_DIR=/target/launcher-slint -w /src/launcher-slint fes-slint:local cargo build -q
    run --network none -e CARGO_TARGET_DIR=/target/launcher-slint -e SLINT_BACKEND=winit-software fes-slint:local \
      xvfb-run -a -s "-screen 0 2000x1200x24" /target/launcher-slint/debug/launcher-slint --shots /src/dist/slint-shots
    ;;
  slint-docs)
    # The README's and the wiki's screenshots -> dist/slint-docs (launcher-slint/src/docs_shots.rs):
    # the launcher's own pages on a made-up install, in the dark look at 1280x800, named as the
    # docs use them. Built with the network, run with none (nothing reaches a real server).
    docker build -q -t fes-slint:local -f build/Dockerfile.slint build >/dev/null
    rm -rf dist/slint-docs
    # With the client DLL (build.sh windows makes it), so Settings › 5th Echelon client has one.
    [ -f dist/uplay_r1_loader.dll ] || { echo "dist/uplay_r1_loader.dll is missing: build.sh windows first"; exit 1; }
    run -e CARGO_TARGET_DIR=/target/launcher-slint-docs -e HOOKS_DLL=/src/dist/uplay_r1_loader.dll -w /src/launcher-slint fes-slint:local \
      cargo build -q --features embed-dll
    run --network none -e SLINT_BACKEND=winit-software \
      -e XDG_CONFIG_HOME=/tmp/fe-docs -e HOME=/tmp/fe-docs-home -e DISPLAY=:99 fes-slint:local sh -c '
        # Xvfb started here: xvfb-run can wait forever for its ready signal.
        Xvfb :99 -screen 0 2000x1200x24 -nolisten tcp & sleep 2
        timeout 600 /target/launcher-slint-docs/debug/launcher-slint --shots-docs /src/dist/slint-docs'
    ;;
  shots)
    # Every screen at several window sizes, as PNGs in dist/shots (launcher/src/shots.rs): the
    # app on a made-up game folder, drawn by wgpu on a software Vulkan (lavapipe), in an image
    # made from this one with Mesa added.
    printf 'FROM %s\nRUN apt-get update && apt-get install -y --no-install-recommends mesa-vulkan-drivers libvulkan1 && rm -rf /var/lib/apt/lists/*\n' "$IMAGE" \
      | docker build -q -t fes-shots:local - >/dev/null
    rm -rf dist/shots
    # Built with the network (cargo fetches its git dependencies), run with none, so nothing the
    # app asks for reaches the community network it's set up for.
    shots=$(run fes-shots:local cargo test -q -p launcher --lib --features shots --no-run --message-format=json \
      | grep -o '"executable":"[^"]*"' | grep '/launcher-' | tail -1 | cut -d'"' -f4)
    [ -n "$shots" ] || { echo "the shots test didn't build"; exit 1; }
    run --network none -e XDG_CONFIG_HOME=/tmp/fe-shots -e SHOTS_OUT=/src/dist/shots -e WGPU_BACKEND=vulkan fes-shots:local \
      "$shots" shots:: --nocapture --test-threads 1
    ;;
  latency)
    # The test players with delay and loss added (scripts/latency-test.sh; PROFILES to choose).
    shift
    run --cap-add NET_ADMIN -e PROFILES "$IMAGE" bash -c 'cargo build -q -p dedicated_server -p testbot && scripts/latency-test.sh /target/native/debug "$@"' latency "$@"
    ;;
  federation-test)
    run "$IMAGE" cargo build -q -p dedicated_server -p testbot -p coordinator
    IMAGE="$IMAGE" scripts/federation-test.sh /target/native/debug
    ;;
  relay-test)
    run "$IMAGE" cargo build -q -p dedicated_server -p testbot -p coordinator
    IMAGE="$IMAGE" scripts/relay-test.sh /target/native/debug
    ;;
  load)
    shift
    run "$IMAGE" cargo build -q --release -p dedicated_server -p testbot
    IMAGE="$IMAGE" scripts/load-test.sh /target/native/release "$@"
    ;;
  crash-fixes)
    game=$(cd "${2:?the game folder (with src/SYSTEM)}" && pwd)
    run -v "$game":/game:ro "$IMAGE" sh -c 'cargo build -q -p hooks-addresses --bin check_crash_fixes && /target/native/debug/check_crash_fixes /game'
    ;;
  ops-test)
    # What install-server.sh writes and runs as root (the updater, backups, failover), in a
    # throwaway container.
    run "$IMAGE" cargo build -q -p coordinator
    IMAGE="$IMAGE" scripts/ops-test.sh /target/native/debug
    ;;
  proxy-test)
    run "$IMAGE" cargo build -q -p dedicated_server -p testbot
    IMAGE="$IMAGE" scripts/proxy-test.sh /target/native/debug
    ;;
  linux)
    # The launcher for Linux and Steam Deck (x86_64; launcher-slint: fontconfig, X11 or Wayland
    # and OpenGL opened when it runs, so it needs only libc), with the client DLL from
    # `build.sh windows` inside, and the Linux server.
    [ -f dist/uplay_r1_loader.dll ] || { echo "build.sh windows first (the launcher carries its DLL)"; exit 1; }
    run -e CARGO_TARGET_DIR=/target/amd64 -e HOOKS_DLL=/src/dist/uplay_r1_loader.dll "$IMAGE" sh -c '
      set -e
      (cd launcher-slint && CARGO_TARGET_DIR=/target/launcher-slint cargo build --release --features embed-dll --target x86_64-unknown-linux-gnu)
      cp /target/launcher-slint/x86_64-unknown-linux-gnu/release/launcher-slint dist/launcher-linux-x86_64
      cargo build -p dedicated_server -p coordinator --release --target x86_64-unknown-linux-gnu
      cp /target/amd64/x86_64-unknown-linux-gnu/release/dedicated_server dist/dedicated_server-linux-x86_64
      cp /target/amd64/x86_64-unknown-linux-gnu/release/coordinator dist/coordinator-linux-x86_64'
    ;;
  sums)
    # The checksums a release publishes; the launcher verifies every
    # download against them.
    run "$IMAGE" sh -c 'cd dist && rm -f SHA256SUMS && find . -maxdepth 1 -type f ! -name SHA256SUMS ! -name ".*" | sed "s|^\./||" | sort | xargs sha256sum > SHA256SUMS && cat SHA256SUMS'
    ;;
  notices)
    # The third-party licences of everything a release carries (scripts/notices.sh, about.toml).
    # cargo-about's JSON and logs in /target/notices, kept only when it fails.
    run "$IMAGE" sh -c 'rm -rf /target/notices && scripts/notices.sh dist/THIRD-PARTY-NOTICES.txt /target/notices && rm -rf /target/notices'
    ;;
  fmt)
    run "$IMAGE" cargo fmt --all
    ;;
  shell)
    TTY=-it run "$IMAGE" bash
    ;;
  *)
    echo "usage: $0 test|server|windows|linux|bots|federation-test|proxy-test|ops-test|load|fmt|notices|sums|shell" >&2; exit 2
    ;;
esac
