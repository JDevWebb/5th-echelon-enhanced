# NAT traversal: playing without a LAN or VPN

Research notes (2026-09-30), from reverse engineering the PC game (`Blacklist_DX11_game.exe`).

Function addresses refer to that executable. Labels:
- **C:** confirmed, seen in the code.
- **I:** inferred.

Nothing here has been tested in the game yet.

## How players connect today

- **Quazal (UDP 3074):** login and matchmaking with the server.
- **Storm (UDP 13000):** the match itself, peer to peer.
  - Every player connects to every other.
  - Storm packets embed peer addresses (little-endian IPv4 + port).
- **Registration:** at `StateRegisterURLs` (0x008a03e0) each game registers its **Storm** address with the server (C).
  - It uses RMC `RegisterURLs`: `prudp:/address=A;port=13000;RVCID=…;hdrType=0;type=2`.
  - If it has a second, local address B, it also sends a second URL without `type`.
- **How joiners read those URLs** (0x0084c500, 0x00836090):
  - `type&2` becomes the primary address A; anything else becomes the local address B (C).
  - `type&1` is skipped (C).
  - A URL without `hdrType` stops parsing (C), so rewrites must keep it.

## Why it only works on a LAN

- A player's address A is its **local** IP (C).
  - Storm resolves IP 0 with gethostname and gethostbyname (0x020de070).
  - That's also why the hook's gethostbyname pin works.
- The game has a way to learn its public address, `Storm::NATTraversal::GetPublicAddress` (0x01fcae30) (C):
  1. It sends a Quazal **NAT echo** (PRUDP stream `NATEcho` = 7, type byte 2) **from the Storm socket**, up to 10 tries, 250 ms apart.
  2. The reply carries a station URL with the observed address.
  3. On success, A becomes the public address and B the old local one (0x0204a610).
- Our server never answers NAT echo, so the lookup times out and A stays local (I).
  - `quazal` defines `StreamType::NATEcho` but nothing handles it.
- Peers then probe each other through the Quazal NATTraversal protocol (C/I):
  - `Storm::PeerProbe` (0x01fcc630) sends `RequestProbeInitiationExt`.
  - The server forwards it as `InitiateProbe` (`dedicated_server/src/nat_traversal.rs`).
  - The probes target A and B.
- Storm's separate Ubisoft punch client is almost certainly unused on PC (I, strong).
  - Its server address is never set.
  - Peers never get the 33-character PunchGUID it requires.
- NAT-type detection (`punch_DetectUrls`) only produces a NAT class label.
  - When detection fails, the game stores "moderate".
  - Its protocol is simple and unauthenticated; see the detection section below.

## What's built

1. **The NAT echo, answered in the client** (`hooks/src/hooks/nat.rs`).
   - The client hooks `bind` to find Storm's socket, and probes the server's NAT helper from it (`nat_proto`, UDP 21128). The helper sees that socket's public mapping.
   - When the game sends its NAT echo (`FUN_02199c60`, `thiscall`, ECX = the NAT engine), the client calls the game's own reply parser (`FUN_0219f810`, `thiscall(engine, sender, data, len)`) with `02 "prudp:/address=IP;port=PORT" 00`.
   - The game then advertises that address, as it would have with Ubisoft's servers.
   - Why the client does it rather than the server: the echo travels inside Storm's bit-packed framing, which is only partly known.
   - DX9 addresses (`0x02172770` / `0x02178320`) were found by byte pattern; unknown builds are searched with the same patterns.
2. **Fallback at registration** (`dedicated_server/src/nat_helper.rs`, `urls_with_public_address`).
   - If the game still registers a private address, the server replaces it with the helper's address and keeps the original as the local URL.
3. **Router port mapping** (`hooks/src/hooks/portmap.rs`): UPnP, then NAT-PMP, for UDP 13000, renewed while the game runs.
4. **Relay** (`nat_helper.rs`, `nat.rs`).
   - Relayed players advertise `server:4xxxx`, a virtual address.
   - The client wraps packets for relay addresses, and every packet of a relayed player, in `DataTo`. The helper forwards them as `DataFrom`, carrying the sender's advertised address. The client unwraps them and shows the game that address as the sender.
   - Mixed direct and relayed players work, because every packet is presented from the address its sender advertises.
   - Who is relayed (`auto`): symmetric NAT (the mapping differs between 21128 and 21129), a player's choice, or any player whose router has no public port mapping for the game, unless the server is on their own network. Since the first community night (2026-10-02), when every host without a mapping could invite no one. Or everyone, with `relay = "all"`.
5. **NAT-type detection server:** not built; it would only label sessions.

## To check in the game

- `bl-tracing.log` should show:
  1. `NAT: Storm socket bound`;
  2. `NAT: the game asks for its public address`;
  3. `NAT: told the game to advertise …`.
- If the second line never appears, the retail game doesn't use the Quazal-NAT mode. The registration fallback still applies, and the next step is hooking `0x0204a610`.
- A co-op join between two different networks (one on a phone hotspot), with **Automatic**, and again with **Always through the server**.
