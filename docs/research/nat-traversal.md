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

## Plan

In order; each step is useful on its own.

1. **Answer NAT echo (native fix).**
   - Reply to stream-7 packets with a station URL built from the address the packet came from. That is the Storm socket's real public ip:port.
   - The game then advertises public A plus local B, and its own probing does the hole punching through the forwarding the server already does.
   - Needs: the echo packet framing (from a capture, or `FUN_02199c60` / `FUN_0219f810`), and which server port it targets (`*(*(NAT+0x74))+0xc`, probably the secure service).
2. **Fallback without the echo: rewrite at registration.**
   - For clients outside `trusted_subnet`, rewrite the `type=2` URL's address to the observed public IP, and keep the original as a local URL.
   - The port is only right with forwarding, UPnP or port-preserving NAT.
3. **Client hook alternative.**
   - Hook `GetPublicAddress` (0x01fcae30) to return an address our server tells us.
   - Only if step 1 can't be done natively.
4. **Router port mapping.** The launcher maps UDP 13000 (and 3074) with UPnP or NAT-PMP, for NATs that punching can't beat.
5. **Relay (later).** For symmetric NAT and CGNAT. Hard, because addresses are embedded in Storm packets.
6. **Optional: NAT-type detection server** (`punch_DetectUrls`), for correct NAT labels in session attributes.
   - Request, 13 bytes: `u16be len=13`, `u16be cmd` (1, 2 or 3), seq, then garbage.
   - Reply, 19 bytes: `u16be len`, `u16be kind` (1 = reply, 2 = next port, 3 = next server), bytes 13–16 the mapped IPv4 with its octets reversed, bytes 17–18 the port big-endian.
   - It uses its own ephemeral socket. Details are in the RE notes.

## First runtime check

Log PeerManager `+0x208` / `+0x20c` in `FUN_0204a120`, to confirm the retail game runs the Quazal-NAT mode (the NATEcho path). Then capture one NAT echo packet.
