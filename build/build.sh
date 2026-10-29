#!/bin/sh
# Builds and tests 5th Echelon Enhanced locally in Docker (no CI).
#
#   build/build.sh test      workspace tests (all but the Windows-only hooks DLL and launcher)
#   build/build.sh server    Linux x86_64 dedicated_server and coordinator -> dist/
#   build/build.sh linux     launcher-linux-x86_64 (DLL embedded) + dedicated_server-linux-x86_64 -> dist/
#   build/build.sh sums      dist/SHA256SUMS for a release
#   build/build.sh windows   launcher.exe (DLL embedded), uplay_r1_loader.dll, dedicated_server.exe, testbot.exe -> dist/
#   build/build.sh bots [scenario ...]
#                            test players against a fresh local server (tools/testbot)
#   build/build.sh tauri      the Tauri launcher for Windows and Linux x86_64 -> dist
#   build/build.sh tauri-trial
#                            the Tauri trial for Windows and Linux x86_64 -> dist/trials
#   build/build.sh shots     every launcher screen at several window sizes -> dist/shots/*.png
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
  test|server|linux|federation-test) ui ;;
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
      SODIUM_LIB_DIR=/tmp/sodium HOOKS_DLL=/target/win/i686-pc-windows-msvc/release/hooks.dll cargo xwin build -p launcher --release --features embed-dll --target x86_64-pc-windows-msvc
      cp /target/win/x86_64-pc-windows-msvc/release/launcher.exe dist/launcher.exe
      # The server for Windows, which the launcher can run.
      SODIUM_LIB_DIR=/tmp/sodium cargo xwin build -p dedicated_server --release --target x86_64-pc-windows-msvc
      cp /target/win/x86_64-pc-windows-msvc/release/dedicated_server.exe dist/dedicated_server.exe
      # The test bots, for running against the node from a Windows test PC.
      SODIUM_LIB_DIR=/tmp/sodium cargo xwin build -p testbot --release --target x86_64-pc-windows-msvc
      cp /target/win/x86_64-pc-windows-msvc/release/testbot.exe dist/testbot.exe'
    ;;
  bots)
    shift
    # Every scenario on a default server, then the friends-only one on a
    # server in the "mutual" mode, and one on a server requiring identities.
    run "$IMAGE" bash -c 'cargo build -q -p dedicated_server -p testbot && scripts/bots.sh /target/native/debug "$@" &&
      if [ $# -eq 0 ]; then FRIENDS_MODE=mutual scripts/bots.sh /target/native/debug friends-mutual &&
        REQUIRE_IDENTITY=1 scripts/bots.sh /target/native/debug identity-required; fi' bots "$@"
    ;;
  tauri)
    # The Tauri launcher (launcher-tauri, 0.4.8) for Windows and for Linux x86_64 -> dist: its
    # page (launcher-tauri/ui) built first, then embedded. Linux links against the WebKitGTK in
    # fes-tauri, as tauri-trial does.
    docker build -q -t fes-tauri:local -f build/Dockerfile.tauri build >/dev/null
    run -v fes-npm-cache:/root/.npm "$IMAGE" sh -c '
      set -e
      cd launcher-tauri/ui
      if [ ! -d node_modules ] || [ package-lock.json -nt node_modules ]; then npm ci --ignore-scripts --no-audit --no-fund; touch node_modules; fi
      npm run build'
    run -e CARGO_TARGET_DIR=/target/launcher-tauri -w /src/launcher-tauri/src-tauri "$IMAGE" \
      sh -c 'cargo xwin build --release --target x86_64-pc-windows-msvc && cp /target/launcher-tauri/x86_64-pc-windows-msvc/release/launcher-tauri.exe /src/dist/'
    sysroot="-C link-arg=-L/sysroot/usr/lib/x86_64-linux-gnu -C link-arg=-L/sysroot/lib/x86_64-linux-gnu"
    sysroot="$sysroot -C link-arg=-Wl,-rpath-link=/sysroot/usr/lib/x86_64-linux-gnu -C link-arg=-Wl,-rpath-link=/sysroot/lib/x86_64-linux-gnu"
    run -e CARGO_TARGET_DIR=/target/launcher-tauri -e CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="$sysroot" \
      -w /src/launcher-tauri/src-tauri fes-tauri:local \
      sh -c 'cargo build --release --target x86_64-unknown-linux-gnu && cp /target/launcher-tauri/x86_64-unknown-linux-gnu/release/launcher-tauri /src/dist/launcher-tauri-linux-x86_64'
    ;;
  tauri-trial)
    # The Tauri trial (trials/tauri-play) for Windows and for Linux x86_64 (the Steam Deck) ->
    # dist/trials. Linux links against x86_64 WebKitGTK unpacked in an image made from this one
    # (build/Dockerfile.tauri).
    docker build -q -t fes-tauri:local -f build/Dockerfile.tauri build >/dev/null
    mkdir -p dist/trials
    run -v "$PWD/trials/tauri-play":/trial -e CARGO_TARGET_DIR=/target/tauri-trial -w /trial/src-tauri "$IMAGE" \
      sh -c 'cargo xwin build --release --target x86_64-pc-windows-msvc && cp /target/tauri-trial/x86_64-pc-windows-msvc/release/tauri-play-trial.exe /src/dist/trials/'
    sysroot="-C link-arg=-L/sysroot/usr/lib/x86_64-linux-gnu -C link-arg=-L/sysroot/lib/x86_64-linux-gnu"
    sysroot="$sysroot -C link-arg=-Wl,-rpath-link=/sysroot/usr/lib/x86_64-linux-gnu -C link-arg=-Wl,-rpath-link=/sysroot/lib/x86_64-linux-gnu"
    run -v "$PWD/trials/tauri-play":/trial -e CARGO_TARGET_DIR=/target/tauri-trial -e CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="$sysroot" \
      -w /trial/src-tauri fes-tauri:local \
      sh -c 'cargo build --release --target x86_64-unknown-linux-gnu && cp /target/tauri-trial/x86_64-unknown-linux-gnu/release/tauri-play-trial /src/dist/trials/tauri-play-trial-linux-x86_64'
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
    # The launcher for Linux and Steam Deck (x86_64), with the client DLL
    # from `build.sh windows` inside, and the Linux server.
    [ -f dist/uplay_r1_loader.dll ] || { echo "build.sh windows first (the launcher carries its DLL)"; exit 1; }
    run -e CARGO_TARGET_DIR=/target/amd64 -e HOOKS_DLL=/src/dist/uplay_r1_loader.dll "$IMAGE" sh -c '
      set -e
      cargo build -p launcher --release --features embed-dll --target x86_64-unknown-linux-gnu
      cp /target/amd64/x86_64-unknown-linux-gnu/release/launcher dist/launcher-linux-x86_64
      cargo build -p dedicated_server -p coordinator --release --target x86_64-unknown-linux-gnu
      cp /target/amd64/x86_64-unknown-linux-gnu/release/dedicated_server dist/dedicated_server-linux-x86_64
      cp /target/amd64/x86_64-unknown-linux-gnu/release/coordinator dist/coordinator-linux-x86_64'
    ;;
  sums)
    # The checksums a release publishes; the launcher verifies every
    # download against them.
    run "$IMAGE" sh -c 'cd dist && rm -f SHA256SUMS && find . -maxdepth 1 -type f ! -name SHA256SUMS ! -name ".*" | sed "s|^\./||" | sort | xargs sha256sum > SHA256SUMS && cat SHA256SUMS'
    ;;
  fmt)
    run "$IMAGE" cargo fmt --all
    ;;
  shell)
    TTY=-it run "$IMAGE" bash
    ;;
  *)
    echo "usage: $0 test|server|windows|linux|bots|federation-test|proxy-test|ops-test|load|fmt|sums|shell" >&2; exit 2
    ;;
esac
