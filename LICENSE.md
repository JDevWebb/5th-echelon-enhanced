# Licensing

This repository mixes code from several sources, and **only part of it is under a licence.** This file says which part, and how to tell.

- **5th Echelon Enhanced's own work is under the [MIT licence](#mit-licence)** (full text below), and so are contributions made to this repository.
- **Upstream [5th Echelon](https://github.com/unixoide/5th-echelon)'s code is not.** Its authors haven't chosen a licence ([unixoide/5th-echelon#129](https://github.com/unixoide/5th-echelon/issues/129), open with no answer as of November 2026), so it remains theirs, with all rights reserved.
- **Third-party material** keeps its own terms ([section 4](#4-third-party-material)).

This project can't license other people's work, and doesn't try to. If upstream adopts a licence, this file will be updated to match.

## 1. The rule: who wrote the line

Licensing here follows authorship, as recorded in the git history:

- **Lines written by this project** (commits by JDevWebb) are under MIT.
- **Lines contributed to this repository by pull request** are under MIT, as [CONTRIBUTING.md](CONTRIBUTING.md) says contributors agree. These include Matthias Walther's (MPW1412) pull requests #2, #3, #5, #7, #8, #10, #11, #12 and #13: host and party migration, quick start, the lobby-return crash fix, the Linux changes and the in-match chat relay, among others.
- **Lines from upstream 5th Echelon** (commits by upstream's authors, unixoide and "Sam Fisher", and its contributors: ThirteenAG, Thiago, Michał Kapała, koteyka and askorbinovaya-kislota) stay theirs, with no licence.
- **Lines from pull requests made to upstream and carried over** stay under their authors' terms: Matthias Walther's [#123](https://github.com/unixoide/5th-echelon/pull/123) and [#124](https://github.com/unixoide/5th-echelon/pull/124) (including the four invite migrations in `dedicated_server/src/storage/migrations/` dated 2026-09-29), and Thiago's [#128](https://github.com/unixoide/5th-echelon/pull/128) (`hooks/src/hooks/nla.rs`).

`git blame -w -M -C <file>` shows who wrote each line, following lines that moved or were copied between files. A file can mix both kinds: changes this project made to an upstream file are MIT; the upstream lines in it are not.

## 2. How much is which

A count of every tracked line (`git blame -w -M -C` at 0.5.0, leaving out `vendor/`, lock files, images and fonts):

| | Lines | Share |
|---|---|---|
| This project | about 158,000 | 60 % |
| Upstream 5th Echelon | about 93,000 | 35 % |
| Matthias Walther (contributions) | about 11,000 | 4 % |

About two thirds of upstream's lines are data and tools rather than the programs: the RMC protocol dumps in `scripts/ddls/`, the game's name hashes in `hooks/maphashes.txt` and `hooks/preload.dat`, the base save in `setup/data/base_savegame.xml`, and the research tools in `tools/`. In the programs themselves, upstream's code is about one line in six, most of it the game's protocol definitions.

## 3. Where upstream's code is

**Wholly or mostly upstream's** (no licence):

| Path | What |
|---|---|
| `quazal/` (except `rmc/unhandled.rs`, `rmc/failures.rs` and this project's changes) | The Quazal PRUDP and RMC protocol library the game speaks |
| `dedicated_server/sc_bl_protocols/` | The game's RMC protocol definitions, generated from upstream's dumps by upstream's generator |
| `dedicated_server/src/`: `overlord_core.rs`, `overlord_news.rs`, `overlord_challenge.rs`, `ticket.rs` and the other original service handlers, in part | The game's services as upstream wrote them; this project rewrote or added most of the server around them (see `git blame`) |
| `hooks/src/uplay_r1_loader.rs`, `hooks/src/uplay_r1_loader/`, `hooks/src/hooks.rs`, `hooks/src/dll_utils/`, `hooks/hooks-addresses/src/lib.rs` and `patterns.rs`, `hooks/hooks-proc/`, `hooks/shim-gen/` | The client DLL's base: the Uplay API stand-in, the game hooks and how they're found |
| `launcher/src/version.rs`, `launcher/src/dll_utils/`, `launcher/src/network/quazal.rs`, `launcher/build.rs` (in part) | Reading the game's version and the server check, from upstream's launcher |
| `setup/src/config.rs`, `setup/src/sys/win.rs` and the `GameVersion` type in `setup/src/game.rs` (in part) | Adapted from upstream's launcher |
| `tools/quazal-tools/`, `tools/umd_browser/`, `tools/storm_parser/`, `tools/wireshark-dissector/`, `tools/quazal.lua`, `tools/rmc.txt`, `tools/dormc.txt` | Upstream's research and development tools |
| `scripts/ddls/`, `hooks/maphashes.txt`, `hooks/preload.dat` | Upstream's protocol dumps and name tables |
| `setup/data/base_savegame.xml` | Upstream's generated save, used for a new player's rank 5 save |
| `data/mp_balancing.ini` | The multiplayer balancing the game downloads from its server, as upstream shipped it |
| `dedicated_server/src/news_upstream.json` | Upstream's news, kept to recognise it |
| `docs/research/splinter_cell_blacklist.md`, `docs/research/quazal.md` | Upstream's research notes |

**Wholly this project's** (MIT): `coordinator/` (except `admin-ui/src/lib/world.js` and the fonts), `launcher-slint/` (except the fonts and the developer's picture), `identity/`, `hwid/`, `replay/`, `moderation/`, `geo/`, `stat_boards/`, `nat_proto/`, `portmap/`, `api/`, `tools/testbot/` (with Matthias Walther's contributions), `tools/map_geometry/`, `build/`, `docker/`, `.github/`, `scripts/` (except `ddls/` and `launch.bat`), `docs/` (except the two research notes above), and most of `setup/`, `launcher/`, `dedicated_server/src/` and `hooks/src/`.

## 4. Third-party material

| What | Whose, and on what terms |
|---|---|
| `vendor/hudhook/` | hudhook 0.8.1 by Andrea Venuta, under the MIT licence (`vendor/hudhook/LICENSE`), vendored with this project's patches |
| The Slint UI toolkit (the launcher) | SixtyFPS GmbH; used under the [Slint Royalty-free Desktop, Mobile, and Web Applications License](https://slint.dev/terms-and-conditions), which asks for attribution: the launcher's About page and this README credit it |
| `hooks/fonts/*.ttf`, `launcher-slint/ui/fonts/*.ttf`, `coordinator/admin-ui/src/assets/fonts/*.ttf` | IBM Plex Sans, Sans Condensed and Mono, under the SIL Open Font License 1.1 (`hooks/fonts/OFL-*.txt`, `launcher-slint/ui/fonts/OFL-*.txt`, `coordinator/admin-ui/public/fonts-license.txt`) |
| `coordinator/admin-ui/src/lib/world.js` | The land outline of [Natural Earth](https://www.naturalearthdata.com) (public domain), via [world-atlas](https://github.com/topojson/world-atlas) (ISC licence) |
| GeoIP data downloaded at run time | [DB-IP](https://db-ip.com)'s IP to City Lite, under CC BY 4.0; the admin UI credits it |
| `launcher-slint/ui/assets/avatars/jdevwebb.png` | JDevWebb's profile picture, on the launcher's About page (the others it credits are shown by their initials) |
| Libraries fetched by Cargo and npm (tokio, tonic, egui, Dear ImGui via imgui-rs, Vue, three.js and others) | Their own licences; they aren't part of this repository, but the release binaries include them |

## No warranty, and what this isn't

**No warranty, for everything here.** Whatever its licence, the software in this repository, its builds, and the community network are provided "as is", without warranty of any kind, express or implied, including merchantability, fitness for a particular purpose and non-infringement. In no event are the authors or the network's operator liable for any claim, damages or other liability, in contract, tort or otherwise, arising from or in connection with them, their use, or other dealings in them.

**The community network** (`scbl.jdevwebb.net` and its relays, coordinator and admin UI) is a service JDevWebb runs with this software. The licence covers the code, not the service or access to it; the release signing key and the community coordinator's join token aren't part of it either.

Splinter Cell and Splinter Cell: Blacklist are trademarks of Ubisoft Entertainment. 5th Echelon Enhanced isn't affiliated with or endorsed by Ubisoft and contains no game files.

## Contributions

Unless a pull request says otherwise, contributions to this repository are accepted under the MIT licence ([CONTRIBUTING.md](CONTRIBUTING.md)).

## MIT licence

Copyright (c) 2026 JDevWebb and 5th Echelon Enhanced contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
