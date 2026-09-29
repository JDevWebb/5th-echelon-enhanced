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
| `setup/`, except the files listed in section 3 | The launcher's logic library |
| `launcher/src/app.rs`, `flow.rs`, `play.rs`, `settings.rs`, `services.rs`, `task.rs`, `theme.rs`, `updater.rs`, `main.rs` | The launcher's interface (`updater.rs` and `main.rs` were rewritten from scratch) |
| `dedicated_server/src/community_api.rs`, `keys.rs`, `rate_limit.rs` | Community API, persistent keys, rate limits |
| `dedicated_server/src/storage/migrations/20260928000000_disable_sample_users.sql` | Disabling the sample accounts |
| `quazal/src/rmc/unhandled.rs` | Counting calls the server can't answer |
| `hooks/src/community.rs` | The overlay's player list and invites |
| `tools/testbot/` | Headless test players |
| `build/`, `docker/`, `release.toml`, `.dockerignore` | Builds, the server image and release numbering |
| `scripts/check-clean.sh`, `scripts/install-hooks.sh` | Repository checks |
| `docs/server-settings.md` | Server settings documentation |

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
| `hooks/fonts/*.ttf` | IBM Plex Sans, under the SIL Open Font License 1.1 (`hooks/fonts/OFL-IBMPlexSans.txt`) |
| `docs/logo.png`, `docs/demo.webm`, `docs/demo_thumb.png`, `docs/overlay_*.png`, `launcher/logo.ico` | Upstream 5th Echelon |
| `docs/screenshots/` | This fork's screenshots, but they show the upstream logo |
| `setup/data/base_savegame.xml` | Upstream 5th Echelon's generated save |
| The README | Partly written by this fork, and partly upstream's (community links, research tools) |

Third-party libraries used by the build (egui, hudhook, Dear ImGui, tokio, tonic and others) keep their own licences. They're fetched by Cargo and aren't part of this repository.

Splinter Cell and Splinter Cell: Blacklist are trademarks of Ubisoft Entertainment. This project isn't affiliated with Ubisoft and contains no game files.

## Contributions

Unless a pull request says otherwise, contributions to this repository are accepted under the MIT licence. They then become part of section 1 or 2.

## MIT licence

Copyright (c) 2026 JDevWebb and 5th Echelon Enhanced contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
