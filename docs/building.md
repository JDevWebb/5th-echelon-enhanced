<!-- Moved from the README (2026-11-10), unchanged. -->
# Build from source

The simplest way needs only Docker, on Windows, macOS or Linux:

```sh
build/build.sh test       # workspace tests
build/build.sh bots       # test players against a fresh local server
build/build.sh proxy-test # the test players against a server behind Caddy
build/build.sh federation-test   # two servers sharing friends through a coordinator
build/build.sh load --players 500 --relayed 20   # a load test (see docs/load-testing.md)
build/build.sh server     # dist/dedicated_server-linux-x86_64, coordinator-linux-x86_64
build/build.sh windows    # dist/launcher.exe (client DLL inside), uplay_r1_loader.dll, dedicated_server.exe
build/build.sh linux      # dist/launcher-linux-x86_64 (client DLL inside: run windows first), dedicated_server-linux-x86_64, coordinator-linux-x86_64
build/build.sh sums       # dist/SHA256SUMS, for a release
build/build.sh ui         # the coordinator's admin UI (coordinator/admin-ui/dist)
```

The launcher and the client DLL are one release: a launcher build fails if the DLL it would carry is from another one (an old `dist/uplay_r1_loader.dll`, say), so build `windows` before `linux`.

`build/build.sh ui` builds the coordinator's admin UI (Vue 3 and Vite, in `coordinator/admin-ui`), which the coordinator embeds; the targets that build the coordinator run it first.

To build natively instead, install [rustup](https://rustup.rs) (the toolchain in `rust-toolchain.toml` installs itself) and the [Protobuf compiler](https://github.com/protocolbuffers/protobuf/releases/latest) (`protoc` on your `PATH`, or `PROTOC` pointing at it). The coordinator also wants [Node.js](https://nodejs.org) 18 or later, to build its admin UI first (`npm ci && npm run build` in `coordinator/admin-ui`); without it, the coordinator builds with a page saying so in place of the UI. Then:

```sh
cargo build --release -p dedicated_server                      # the server, for your OS
cargo build --release -p launcher --features embed-dll         # on Windows: builds the client DLL and embeds it
```

## Releases and CI

GitHub Actions tests and builds every push and pull request (Linux tests, the test players, and the Windows and Linux builds). Releases are published by pushing a version tag:

`release.toml`'s `version` is the one release number for the launcher, the client DLL, the server and the coordinator. Between releases it names the next one, with a `-dev` suffix on builds that aren't a release (`0.4.1-dev` until 0.4.1 is out); it moves on per release, not per change.

On the machine with the release key, one command does it all:
```sh
scripts/release.sh 0.4.1
```
It checks `main` is clean and that CI passed on it, drops the suffix in `release.toml`, commits and tags `v0.4.1`, pushes `main` and that tag, waits for the release workflow, runs `sign-release.sh` (which asks once before signing), publishes, and moves `main` on to `0.4.2-dev`. Release notes go in `docs/releases/v0.4.1.md`; the workflow puts them first. CI doesn't run again for the two version commits, or for a push that only changes release notes: the release workflow builds the tag itself.

By hand, the same steps are:
1. Drop the suffix in `release.toml` (e.g. `0.4.1`) and commit it.
2. Tag it with exactly that version, and push that tag by name (never `--tags`):
   ```sh
   git tag v0.4.1 && git push origin main v0.4.1
   ```
3. Sign and publish it:
   ```sh
   scripts/sign-release.sh v0.4.1
   ```

The release workflow:
- builds every download on Windows and Linux runners, and adds the Linux installer (`scripts/install-server.sh` at the tag), so the release's signature covers it too;
- writes `SHA256SUMS`, which the launcher's updater and the Linux installer check against;
- attests each download's build provenance (`gh attestation verify <file> --repo JDevWebb/5th-echelon-enhanced`);
- makes the GitHub release as a draft;
- pushes the server image to `ghcr.io`, tagged `:<version>-unsigned` only.

`sign-release.sh`:
- checks that the tag's commit is on `main`, every download against `SHA256SUMS`, and each download's build provenance (`gh attestation verify`, when gh can), and prints the commit and hashes to compare with the workflow run;
- builds the signer without the key, then runs only that binary, with no network and the key mounted read-only, to sign the version and `SHA256SUMS` (`SHA256SUMS.sig`; the version is signed so a release can't be published again under another tag);
- checks the signature with OpenSSL, as the installer does, uploads it and publishes the draft;
- tags the server image `:<version>` (and `:latest` for a release) once it has checked the `-unsigned` image was built from the tag's commit. That needs gh's token to have the `write:packages` scope (`gh auth refresh -s write:packages`); without it, it prints the commands.

The key stays off GitHub, in `~/.config/5th-echelon-release/`. Launchers, the installer and the servers' updater only install releases it signed, and a coordinator only rolls out signed releases. Its public half is in `identity/src/lib.rs` (`RELEASE_KEYS`), `scripts/install-server.sh` and [deploying.md](deploying.md#get-the-installer) (with its fingerprint, for checking the installer by hand); `cargo run -p identity --bin release-sign -- keygen <file>` makes a new one.

Once a release is published, every network's coordinator finds it within 10 minutes and rolls it out to its servers.

A tag with a suffix (`v0.3.1-rc.1`) makes a pre-release, which the updater doesn't offer.

## Project layout

| Folder | What |
|---|---|
| `dedicated_server/` | The server: Quazal services, the game's protocols, the gRPC API and the community API |
| `quazal/` | The PRUDP/RMC network stack |
| `nat_proto/` | The NAT helper protocol between the server and the client (internet play) |
| `portmap/` | Router port forwarding (UPnP, NAT-PMP), for the client and the connection test |
| `identity/` | A player's identity across servers: the key, and the messages it signs |
| `geo/` | Where an address is (DB-IP's city database, kept current), for metrics |
| `coordinator/` | Shares friends between servers, keeps the server directory, rolls out updates, and serves the admin UI |
| `hooks/` | The game client (`uplay_r1_loader.dll`): Uplay emulation, network fixes and the overlay |
| `setup/` | The launcher's logic: finding the game, installing, accounts, saves and checks |
| `launcher-slint/` | The launcher (Windows, Linux and the Steam Deck), in Slint: its own workspace |
| `launcher/` | The launcher's library (its logic, shared), and the egui interface it had before 0.5.0 |
| `api/` | The gRPC API definitions |
| `tools/` | Test players, the DDL parser, Wireshark dissectors, and research tools |
| `docker/` | The server's Docker image and compose file |
| `.github/workflows/` | CI (`ci.yml`) and releases (`release.yml`) |

## Research tools

- **DDL parser:** reads the game's protocol definitions out of an executable:
  ```sh
  cargo run --release -p quazal-tools --bin ddl-parser -- -i mapping.json -o ddls.json path/to/exe
  ```
- **Wireshark dissectors:**
  - PRUDP/RMC, in Rust: `scripts/build_dissector.sh`.
  - A Lua dissector with the DO protocol: copy `tools/quazal.lua`, `tools/dormc.txt` and `tools/rmc.txt` to Wireshark's plugin folder.
  - A basic Lua dissector for Storm (P2P).

  For example:
  ```sh
  tshark -r capture.pcapng -d udp.port==3074,prudp -d udp.port==13000,prudp -V -O prudp,rmc
  ```
