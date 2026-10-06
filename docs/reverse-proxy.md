# Running the server behind a reverse proxy

For a machine that already runs other services on the usual ports: everything TCP can share port 80 with your other sites through Caddy, by host name. The UDP ports can be moved to any free numbers. Players still only type the server's name: the launcher asks the server which ports it uses.

The full example, tested by `build/build.sh proxy-test`, is [`reverse-proxy/Caddyfile`](reverse-proxy/Caddyfile).

On a Linux server, [`scripts/install-server.sh`](../scripts/install-server.sh) does all of this for you: it installs Caddy, and writes both the site and the server's settings. The steps below are for doing it by hand, or for an existing Caddy.

## What goes where

| What | Protocol | Through Caddy? |
|---|---|---|
| Online config and community API | HTTP | **Yes**, on port 80 by host name. The game always uses port 80 and only ever plain HTTP. |
| Content (multiplayer balancing) | HTTP | **Yes**, the same host name, by path |
| Accounts, friends, invites (launcher and overlay) | gRPC without TLS (h2c) | **Yes**, the same host name, by content type |
| Game login and game service | UDP | **No**: forward the ports straight to the server, on any free numbers |
| Internet play helper (NAT) | UDP | **No**: forward them straight to the server; two ports next to each other |

The UDP services need to see players' real addresses. The NAT helper, the relay and the address fixes all depend on it, and a UDP proxy (nginx `stream`, Traefik UDP) hides them.

## 1. The server

In `service.toml`, keep the HTTP services on the machine only. Move the UDP ports if other services hold the defaults. Then say what players connect to in `[public]`:

```toml
api_server = "127.0.0.1:50051"

[service.onlineconfig]
listen = "127.0.0.1:8080"          # Caddy serves port 80

[service.content]
listen = "127.0.0.1:8000"

[service.sc_bl_auth]
listen = "0.0.0.0:31126"           # game login (UDP)

[service.sc_bl_secure]
listen = "0.0.0.0:31127"           # game service (UDP)

[nat]
listen = "0.0.0.0:31128"           # internet play helper (UDP), and 31129

[public]
host = "blacklist.example.com"     # what players type
api = 80                           # the API through Caddy
content = 80                       # content through Caddy
# login, secure and nat default to the ports above. Set them when the
# router forwards other numbers to these, e.g. login = 21126.
```

Start the server with `--public-address 203.0.113.10` (or `FE_PUBLIC_ADDRESS`), the address players' UDP reaches.

When the server starts, it rewrites every address it hands out to use these:
- the online config's login address;
- the game service in login tickets;
- content downloads, which are addressed to the host name so Caddy can route them;
- relay addresses.

`GET /api/info` reports the ports, and **Connect** in the launcher reads them.

Any port `[public]` leaves unset is the one its service listens on. So moving a listening port is enough, and with plain port forwarding to other numbers you only set the ones that differ. Without a `[public]` section, the server hands out what `service.toml` says, as before.

## 2. Caddy

```caddyfile
{
	servers :80 {
		protocols h1 h2c
	}
}

http://blacklist.example.com {
	@admin path /users.UsersAdmin/* /games.GamesAdmin/*
	handle @admin {
		respond 403
	}
	@grpc header Content-Type application/grpc*
	handle @grpc {
		reverse_proxy h2c://127.0.0.1:50051 {
			flush_interval -1
		}
	}
	handle /mp_balancing.ini {
		reverse_proxy 127.0.0.1:8000
	}
	handle /ugc/* {
		reverse_proxy 127.0.0.1:8000
	}
	handle {
		reverse_proxy 127.0.0.1:8080
	}
}
```

- **`http://`** matters. The game can't follow a redirect to HTTPS. Don't put this host name behind a global HTTPS redirect.
- **`protocols h1 h2c`** lets the launcher and overlay speak gRPC over plain HTTP on port 80. It applies to every site on port 80, and changes nothing for the others.
- **`@admin`** keeps the admin API (accounts and games) off the internet. Manage the server on its own machine, or through an SSH tunnel (`ssh -L 50051:127.0.0.1:50051 you@server`, then the launcher's "Manage a server" at `localhost`).
- **`flush_interval -1`** passes invites on the moment they arrive; they come over a stream that stays open.
- **`/mp_balancing.ini` and `/ugc/*`** go to the content server: the multiplayer balancing file the game downloads, and the uploads it sends (its ShadowNet snapshot, from 0.4.3). Without the `/ugc/*` route the game's uploads fail.
- **The API over HTTPS**, so passwords and sign-in tokens never travel readable: add the same routes as an `https://blacklist.example.com { … }` site (Caddy gets the certificate; TCP 443 must be open), then set `api_tls = 443` in `[public]`. Launchers use it from then on. The Linux installer does all of this.
- If Caddy runs in Docker, use the server's address instead of `127.0.0.1`. Also add Caddy's network to `[public] proxies` (see below).

## 3. Router and firewall

Forward straight to the server (UDP): the login and game-service ports, and the NAT helper's two ports (31126, 31127, 31128 and 31129 above). Port 80 is already Caddy's.

## Rate limits behind a proxy

Behind Caddy, every login and new account comes from Caddy's address. The server believes the `X-Forwarded-For` header Caddy adds, but only from proxies it trusts:
- Caddy on the same machine (loopback) is always trusted.
- A proxy elsewhere, e.g. in Docker, must be listed:

```toml
[public]
proxies = ["172.17.0.0/16"]
```

Without that, all players would share one budget of failed logins and new accounts.

## Checking it

- The launcher's **Settings › Network › Connection test** checks each part on the ports the server reported.
- From anywhere:
  ```sh
  curl http://blacklist.example.com/api/info
  ```
  shows the ports the server hands out.
