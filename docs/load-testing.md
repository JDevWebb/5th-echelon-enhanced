# Load testing

`build/build.sh load` runs the server in a container limited like a small VPS, then connects hundreds of test players to it.

The test players do what the game does:
- they sign in the same way, through an API account, the game login and the game service;
- they keep using the server: the friend list, friend searches, lobby searches and hosting lobbies;
- they play matches that send game-like traffic, with a share of the players going through the relay.

```sh
build/build.sh load --players 500 --relayed 20
CPUS=2 MEMORY=2g build/build.sh load --players 1000 --relayed 20 --duration 120
```

| Option | Default | What |
|---|---|---|
| `--players` | 100 | players signed in at once |
| `--relayed` | 20 | percent of players who go through the relay (symmetric or carrier-grade NAT) |
| `--match-size` | 4 | players per match (Spies vs Mercs 4v4 is 8) |
| `--pps`, `--bytes` | 30, 200 | packets per second, and their size, from each player to each other player in its match |
| `--duration` | 60 | seconds of play after everyone has signed in |
| `--connect` | 32 | sign-ins at a time |
| `--activity` | 10 | average seconds between a player's actions |
| `--names` | | names for the players, in order (`Kiwi,Fisher,…`); the rest are numbered |
| `CPUS`, `MEMORY` | 1, 1g | the server container's limits |

## What it reports

Every 5 seconds, then as a summary:
- the relay's packets per second in and out, its loss and latency;
- how long the friend list and the friend and lobby searches take (p50 and p99), and how many fail.

It also shows the server's CPU during play and during the sign-ins, its resident memory, and the relay's own count of forwarded and dropped packets from the server log.

Only traffic that touches the relay is sent. Players who connect directly never reach the server, so they cost it nothing during a match.

## Results

Measured on a MacBook (Docker Desktop, 10 cores shared by the server and the test players), with the defaults above: 20% of players relayed, matches of 4, 30 packets/s of 200 bytes per link, 60 s of play. The 1,000-player runs had the UDP buffer limit the Linux installer sets (4 MB); the smaller ones didn't need it and ran with Linux's default.

| Players | Server CPUs | Sign-ins | Relay in | Relay loss | Relay p99 | Searches failed | CPU in play | Memory in play (peak) |
|---|---|---|---|---|---|---|---|---|
| 100 | 1 | 18/s | 3.4k pkt/s | 0% | 1.3 ms | 0 | 5% | 15 MB (32) |
| 500 | 1 | 19/s | 15k pkt/s | 0.15% | 3.5 ms | 9 of 1,900 | 32% | 32 MB (63) |
| 1,000 | 1 | 19/s | 31k pkt/s | 0% | 15.5 ms | 302 of 3,700 | 63% | 53 MB (75) |
| 1,000 | 2 | 28/s | 33k pkt/s | 0% | 1.9 ms | 59 of 3,800 | 70% | 57 MB (81) |

- **Sign-ins are the peak.** Each costs one Argon2 hash, about 50 ms of one core, so a server signs in about 18 players a second per core. A burst after a restart takes a minute for 1,000 players on one core.
- **Memory is small.** Under 100 MB at 1,000 players.
- **Up to 500 players, one core is plenty.** The relay forwards 15,000 packets a second with no loss worth noting, at a third of a core.
- **At 1,000 players, use two cores.** One core relays everything, but it runs near its limit: the relay's p99 latency rises to 15 ms, and about 8% of friend and lobby searches time out. With two, searches failing drop to under 2% and the relay's p99 is back under 2 ms.
- **The UDP buffer limit matters.** With Linux's default (208 KB) instead of 4 MB, 1,000 players lost 15–29% of relayed packets in bursts, although the server had CPU to spare. The Linux installer raises it (`net.core.rmem_max`); elsewhere the server logs a warning with the command.
- **Bandwidth is the real cost.** The relay sends out what it takes in: about 7 MB/s (roughly 60 Mbit/s) each way at 1,000 players with 20% relayed, 3 MB/s at 500.

## What the test found and fixed

The first runs found three problems, all fixed:

1. **Memory.** After a handful of sign-ins, the server held hundreds of MB, although it needs about 10.
   - Each password hash (Argon2) uses about 19 MiB. glibc kept those blocks after they were freed, in up to 8 arenas per core.
   - The server now sets a fixed mmap threshold and 2 arenas at startup, so the memory goes back to the system. It also hashes at most 2–4 passwords at a time, on the blocking pool, so a burst of logins doesn't stall the API.
2. **Relay addresses changed mid-match.** A keepalive probe without the "relay me" flag, or a router port mapping appearing later, changed the address the server relayed a player at. The game had already told the other players the old one.
   - A registration now keeps its address: a relayed player stays relayed, and a direct one keeps its address unless its NAT gave it a new one.
3. **Relay throughput.** The relay cloned two strings for every packet, and bursts overflowed the socket's default buffer.
   - It now relays without allocating, with 4 MB socket buffers.
   - The Linux installer allows those buffers (`net.core.rmem_max`); elsewhere the server logs a warning with the command to run.
