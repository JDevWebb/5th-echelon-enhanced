# Licensing

This repository mixes code from several sources, and **only part of it is under a licence.** This file says exactly which part.

- **This fork's own work is under the [MIT licence](#mit-licence)** (full text below).
- **Upstream 5th Echelon's code is not.** Its authors haven't chosen a licence yet ([unixoide/5th-echelon#129](https://github.com/unixoide/5th-echelon/issues/129)), so it remains theirs, with all rights reserved.
- **Code merged from others' pull requests** stays under its authors' terms.

The fork can't license other people's work, and doesn't try to. If upstream adopts a licence, this file will be updated to match.

## What is under the MIT licence

### 1. Files created by this fork

Everything in these files and folders is under MIT:

| Path | What |
|---|---|
| `identity/` | A player's identity across servers (Ed25519 keys and the messages they sign), and `release-sign`, which signs releases |
| `coordinator/`, except `coordinator/admin-ui/src/lib/world.js` and the fonts | The coordinator: friends shared between servers, name reservations, the server directory, update rollouts, metrics and the admin UI |
| `geo/` | Locating addresses with DB-IP's city database, for metrics |
| `nat_proto/` | The NAT helper's protocol between the server and the client (internet play) |
| `portmap/` | Asking the router to forward a port (UPnP and NAT-PMP), for the client and the launcher's connection test |
| `setup/`, except the files listed in section 3 | The launcher's logic library: finding the game, installing, accounts, saves, checks, the server directory |
| `launcher/src/app.rs`, `flow.rs`, `play.rs`, `server.rs` (except its log view; see section 3), `settings.rs`, `services.rs`, `task.rs`, `theme.rs`, `updater.rs`, `main.rs`, `network/nat.rs` | The launcher's interface, signed updates, and internet play checks (`updater.rs` and `main.rs` were rewritten from scratch) |
| `dedicated_server/src/clients.rs`, `news_0.3.150.json`, `community_api.rs`, `federation.rs`, `friends_policy.rs`, `keys.rs`, `metrics.rs`, `nat_helper.rs`, `rate_limit.rs`, `self_update.rs`, `storage/relationships.rs` | Refusing outdated clients, the community API, sharing friends with a coordinator, friend-list rules, persistent keys, the NAT helper and relay, rate limits, and friends, blocks and identities in the database |
| `dedicated_server/src/storage/migrations/` dated 2026-09-28, 2026-10-01 and 2026-10-04 | Disabling the sample accounts; friends, name conflicts and token epochs; client sign-ins |
| `quazal/src/rmc/unhandled.rs` | Counting calls the server can't answer |
| `hooks/src/community.rs`, `hooks/src/hooks/nat.rs`, `hooks/src/hooks/portmap.rs`, `hooks/hooks-config/src/text.rs` | The overlay's friends, player search and invites; internet play in the client (the NAT helper, the relay, keeping the router's port mapping) |
| `tools/testbot/` | Headless test players and the load test |
| `build/`, `docker/`, `release.toml`, `.dockerignore`, `.github/` | Builds, the server image, release numbering, CI and release workflows, issue forms |
| `scripts/install-server.sh`, `harden-host.sh`, `sign-release.sh`, `release.sh`, `bots.sh`, `load-test.sh`, `proxy-test.sh`, `federation-test.sh`, `check-clean.sh`, `install-hooks.sh` | The Linux installer, release signing, test runners and repository checks |
| `docs/deploying.md`, `operations.md`, `community-servers.md`, `friends.md`, `server-settings.md`, `reverse-proxy.md`, `reverse-proxy/`, `load-testing.md`, `roadmap.md`, `releases/`, `research/nat-traversal.md`, `research/cross-region.md` | This fork's documentation |
| `LICENSE.md` | This file |
| `SECURITY.md` | How to report security problems |

### 2. This fork's changes to other files

Many upstream files carry changes made by this fork. **Those changes (the lines this fork added or changed, as recorded in the git history) are under MIT. The files as a whole are not:** the upstream code in them stays as described at the top.

The same applies to this fork's follow-up changes to code merged from pull requests.

`git log --author=JDevWebb` lists the fork's commits, and `git blame` shows who wrote each line.

### 3. Not covered by this licence

| What | Whose, and on what terms |
|---|---|
| All code from upstream [5th Echelon](https://github.com/unixoide/5th-echelon), in any file | unixoide and upstream contributors; no licence yet |
| `setup/src/config.rs`, `setup/src/sys/win.rs`, the `GameVersion` type in `setup/src/game.rs`, and the log view in `launcher/src/server.rs` | Adapted from upstream's launcher: the upstream parts stay upstream's; this fork's changes are under MIT (section 2) |
| Code from [#123](https://github.com/unixoide/5th-echelon/pull/123) and [#124](https://github.com/unixoide/5th-echelon/pull/124), including the four invite migrations in `dedicated_server/src/storage/migrations/` dated 2026-09-29 | Matthias Walther |
| `hooks/src/hooks/nla.rs` ([#128](https://github.com/unixoide/5th-echelon/pull/128)) | Thiago |
| `coordinator/admin-ui/src/lib/world.js` | The land outline of [Natural Earth](https://www.naturalearthdata.com) (public domain), via [world-atlas](https://github.com/topojson/world-atlas) (ISC licence) |
| GeoIP data downloaded at run time | [DB-IP](https://db-ip.com)'s IP to City Lite, under CC BY 4.0; the admin UI credits it |
| `hooks/fonts/*.ttf` | IBM Plex Sans, Sans Condensed and Mono, under the SIL Open Font License 1.1 (`hooks/fonts/OFL-*.txt`) |
| `coordinator/admin-ui/src/assets/fonts/*.ttf` | the same IBM Plex fonts, for the admin UI, under the SIL Open Font License 1.1 (`coordinator/admin-ui/public/fonts-license.txt`) |
| `docs/logo.png`, `docs/demo.webm`, `docs/demo_thumb.png`, `docs/overlay_*.png`, `launcher/logo.ico` | Upstream 5th Echelon |
| `docs/screenshots/` | This fork's screenshots, but they show the upstream logo |
| `setup/data/base_savegame.xml` | Upstream 5th Echelon's generated save |
| `dedicated_server/src/news_upstream.json` | Upstream 5th Echelon's news (`data/news.json`), kept to recognise it |
| `docs/research/`, except `nat-traversal.md` and `cross-region.md` | Upstream's research notes |
| The README | Partly written by this fork, and partly upstream's (community links, research tools) |

**No warranty, for everything here.** Whatever its licence, the software in this repository, its builds, and the community network are provided "as is", without warranty of any kind, express or implied, including merchantability, fitness for a particular purpose and non-infringement. In no event are the authors or the network's operator liable for any claim, damages or other liability, in contract, tort or otherwise, arising from or in connection with them, their use, or other dealings in them. 5th Echelon Enhanced isn't affiliated with or endorsed by Ubisoft; *Tom Clancy's Splinter Cell: Blacklist* and its names and marks are Ubisoft's.

**The community network** (`play.scbl.jdevwebb.net` and its servers, and its admin UI at `scbl-metrics.jdevwebb.net`) is a service JDevWebb runs with this software. The licence covers the code, not the service or access to it; the release signing key and the community coordinator's join token aren't part of it either.

Third-party libraries used by the build (egui, hudhook, Dear ImGui, tokio, tonic and others) keep their own licences. They're fetched by Cargo and aren't part of this repository.

Splinter Cell and Splinter Cell: Blacklist are trademarks of Ubisoft Entertainment. This project isn't affiliated with Ubisoft and contains no game files.

## Contributions

Unless a pull request says otherwise, contributions to this repository are accepted under the MIT licence. They then become part of section 1 or 2.

## MIT licence

Copyright (c) 2026 JDevWebb and 5th Echelon Enhanced contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
