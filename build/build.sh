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
#   build/build.sh load [options]
#                            a load test against a server limited to CPUS (1)
#                            and MEMORY (1g), e.g. --players 200 --relayed 20
#   build/build.sh proxy-test
#                            the test players against a server behind Caddy
#                            (docs/reverse-proxy.md), by host name only
#   build/build.sh federation-test
#                            two servers sharing friends through a coordinator
#                            (docs/friends.md)
#   build/build.sh fmt       cargo fmt (the crates this fork changes)
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

mkdir -p dist
case "${1:-test}" in
  test)
    run "$IMAGE" cargo test --workspace --exclude hooks --exclude launcher --exclude umd_browser
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
    # server in the "mutual" mode.
    run "$IMAGE" bash -c 'cargo build -q -p dedicated_server -p testbot && scripts/bots.sh /target/native/debug "$@" &&
      if [ $# -eq 0 ]; then FRIENDS_MODE=mutual scripts/bots.sh /target/native/debug friends-mutual; fi' bots "$@"
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
    run "$IMAGE" cargo fmt -p dedicated_server -p quazal -p launcher -p hooks -p hooks-config
    ;;
  shell)
    TTY=-it run "$IMAGE" bash
    ;;
  *)
    echo "usage: $0 test|server|windows|shell" >&2; exit 2
    ;;
esac
