# Contributing to 5th Echelon Enhanced

Thanks for helping keep *Splinter Cell: Blacklist* online. Contributions of every size count: a bug report with the right details, an evening testing with friends, a protocol finding, a docs fix, or code.

This page says how to report problems so they can be fixed, how to build and test, and the few rules this project can't bend.

## Reporting a problem

[Open an issue](https://github.com/JDevWebb/5th-echelon-enhanced/issues/new/choose) and pick the form that fits. The forms ask for what we need; filling them in carefully saves a lot of back and forth.

| Form | For |
|---|---|
| **Game or launcher problem** | The launcher, the in-game overlay (F5), saves, stats, crashes: anything on your PC |
| **Can't join, host or stay connected** | Joins that fail, invites that never arrive, being dropped from a match |
| **Running your own server** | The installer, Docker, a reverse proxy, the coordinator or its admin UI |
| **Idea or improvement** | Something new, or something that could work better |
| **Game research** | What you've worked out about the game's protocols or data |

**The quickest way to get a connection problem looked at** is the launcher's own report: **Settings › Feedback › Send feedback** right after it happens. It sends your logs, with your PC's name, your user folder and your internet address removed first, to the admins of the server you played on, who see it next to what the server saw. Then open an issue and say roughly when you sent it.

**Times matter.** Join and connection problems are found by matching what your game saw with what the server saw, so give the time it happened, with your time zone (or in UTC).

**Logs:** the game's log is `bl-tracing.log` in the game's folder (`bl-tracing.prev.log` is the game before). Read it before you attach it: it can contain your Windows user name, your PC's name and your internet address. The launcher's report removes those for you.

**Security problems never go in a public issue.** Report them privately, as [SECURITY.md](SECURITY.md) describes.

## Building and testing

Everything builds and tests in Docker, the same on Linux, macOS and Windows; you don't need Rust, Node or the Windows SDK on your machine.

```sh
build/build.sh test              # the workspace's tests (all but the Windows-only DLL)
build/build.sh bots              # test players against a fresh local server
build/build.sh federation-test   # two servers sharing friends through a coordinator
build/build.sh windows           # launcher.exe and the client DLL -> dist/
build/build.sh linux             # the Linux launcher and server -> dist/
build/build.sh fmt               # format before you commit
```

The top of `build/build.sh` lists the rest (the admin UI, the proxy test, the load test).

**Before a pull request:**
- Run `build/build.sh test` and `build/build.sh fmt`. For server changes, run `build/build.sh bots` too, and add a scenario to `tools/testbot` for new behaviour.
- **Changes to the game client or the launcher need a real Windows test**: the DLL only runs inside the game, and CI can't play it. Say in the pull request what you tried in the game (and with how many players), or that you couldn't.
- Admin UI changes: say which pages you looked at, and in which theme (it follows the system's light or dark mode).
- CI builds and tests every pull request on Linux and Windows.

## How the code is laid out

| Folder | What |
|---|---|
| `launcher/`, `setup/` | The launcher (egui) and its logic: finding the game, installing, accounts, saves, checks |
| `hooks/` | The client DLL that runs inside the game (`uplay_r1_loader.dll`): Uplay's API, the overlay, internet play |
| `dedicated_server/`, `quazal/` | The game server, and the Quazal protocols (PRUDP and RMC) it speaks |
| `coordinator/` | Friends across servers, the server directory, updates, metrics and the admin UI (`admin-ui/`, Vue) |
| `api/` | The gRPC API between the launcher, the DLL and the server |
| `identity/`, `nat_proto/`, `portmap/`, `geo/`, `stat_boards/` | Shared pieces: player identities, the NAT helper's protocol, router port mapping, geolocation, stats boards |
| `tools/testbot/` | Headless test players and the load test |
| `docs/` | Deploying, operating, the research notes and the release notes |

## Conventions

- **Pull requests:** focused on one thing, with what changed, why, and how you tested.
- **Commit messages:** a summary line that says what changed for whom, starting with the area (`Launcher: …`, `Server: …`, `Admin UI: …`), then a body that explains why: the problem it fixes and anything a reviewer should know.
- **Code:** match what's around it. Comments say why, not what, in plain sentences.
- **Words players read** (the launcher, the overlay, errors, release notes): plain English, short sentences, and say what to do next.
- **Version numbers:** leave `release.toml` alone. It names the next release with `-dev` and moves only when a release is made.
- **Release notes:** a change players or server admins will notice gets a line in the next release's notes (`docs/releases/`).
- **Privacy:** new data about players (logs, locations, anything sent off their PC) needs saying so plainly where players see it, and in the privacy section of [docs/operations.md](docs/operations.md#privacy).

## Rules this project can't bend

- **Never commit game files or anything extracted from them:** no executables, archives, textures, maps or save files from the game, not even small ones. Facts worked out from the game are welcome: IDs, names, protocol layouts, offsets. Research notes go in issues or `docs/research/` in your own words.
- **You need your own copy of the game.** Nothing here gives the game away or gets around buying it.
- **The community network takes no outside servers.** Run your own server, or your own network with its own coordinator ([docs/deploying.md](docs/deploying.md)); public servers can be listed in [docs/community-servers.md](docs/community-servers.md).

## Licence and credit

This project's own work is under the MIT licence; upstream 5th Echelon's code has no licence yet and stays its authors'. [LICENSE.md](LICENSE.md) says exactly which is which. By contributing, you agree your contribution is under the MIT licence, as part of this project's own work.

Your commits keep your name, and you'll be listed under [Authors and contributors](README.md#authors-and-contributors). Fixes that also apply to [upstream 5th Echelon](https://github.com/unixoide/5th-echelon) are offered there too.

**Want to help run the project** (reviewing pull requests, triaging issues, testing releases)? See [Become a collaborator](README.md#become-a-collaborator).

## Finding other players

The [community server](README.md#the-community-server) is at `play.scbl.jdevwebb.net`, and the [Discords in the README](README.md#community) are the place to find players and arrange games.
