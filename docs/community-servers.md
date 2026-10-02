# Community servers

Servers that anyone can join. Players type the community network's address, **`play.scbl.jdevwebb.net`**, and the launcher sets them up on the server with the lowest ping; the Play screen lists the rest. This page also lists servers outside it.

## The community network

Servers that share friends through the community coordinator, `https://play.scbl.jdevwebb.net`. Friends and names follow players between them.

| Server | Address | Region | Friend lists | Run by |
|---|---|---|---|---|
| 5th Echelon Community EU | `eu1.scbl.jdevwebb.net` | Falkenstein, Germany | mutual | [JDevWebb](https://github.com/JDevWebb) |
| 5th Echelon Community Oceania | `oceania.scbl.jdevwebb.net` | Sydney, Australia | mutual | [JDevWebb](https://github.com/JDevWebb) |

**To add yours:** install it with [the installer](deploying.md), then [open an issue](https://github.com/JDevWebb/5th-echelon-enhanced/issues/new?template=add-server.yml). Once it's checked, you get the join token privately and your server joins the directory. You can also send a pull request adding it to this table.

**What membership means:**
- **Releases:** your server installs the network's signed releases as the coordinator rolls them out. That's the installer's updater, on by default.
- **Falling behind:** a server that turns updates off, or still runs an older release a day after the network moved on, leaves the directory until it catches up.
- **Metrics:** it reports anonymous metrics to the coordinator: player counts by city, what's being played, load and traffic. No names or addresses. See [operations.md](operations.md).

## Other servers and networks

Servers and groups with coordinators of their own. Add yours with a pull request: one row, with an address players can type.

| Server or network | Address | Region | Notes |
|---|---|---|---|
| | | | |

Running your own network is just as welcome: a coordinator of your own, with its own directory, for your community. See [Deploying](deploying.md#a-coordinator-on-its-own).
