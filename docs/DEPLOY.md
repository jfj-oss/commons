# Hosting Megatron

**The plan: this PC, Tailscale Funnel, one command.** Friends need no Tailscale. A small rented server is the fallback; it is further down.

```sh
deploy/go-online.sh     # starts relay + LiveKit, opens the public port, prints the link
deploy/go-offline.sh    # closes it again and stops them; accounts and messages stay
```

Nothing restarts after a reboot; run it again. The secrets are made once, so old links keep working. Everything lives in `~/.local/share/commons-host`. To fill the download page and publish an update to everyone's app: `scripts/publish-update.sh` (all three systems, signed; [UPDATES.md](UPDATES.md)). To ask apps older than a version to update: `COMMONS_MIN_APP_VERSION=0.2.0 deploy/go-online.sh` (the relay's `--min-app-version`).

## What was measured on this PC (2026-11-07, nothing changed)

| Question | Answer |
| --- | --- |
| Is Funnel allowed for this node? | **Not yet.** HTTPS certificates are on, the Funnel capability is not. The first `tailscale funnel` prints a link to approve it in the admin console, one time; go-online.sh waits for that. No public DNS record for the name exists until then |
| Which public ports? | Funnel offers only 443, 8443 and 10000, and it opens a whole port. **443 and 8443 already carry tailnet-only services here, Vaultwarden among them on 443, and funnelling them would publish those.** So everything shares 10000 |
| Does `tailscale` need root? | No, this user is the operator |
| Can relay + LiveKit signalling + media share one TLS port? | **Yes.** `--front` splits the stream by its first bytes. Tested end to end on loopback with a TLS terminator standing in for Funnel: account, chat, file, call and screen share, with media forced through TURN over TLS (`tools/hosting-e2e.mjs`, 22 checks, three runs in a row) |
| Media over LiveKit's plain TCP? | **No.** Funnel routes by TLS SNI, and ICE over TCP is not TLS. It cannot get through |
| Direct UDP from friends? | **Not without changes.** It needs a router port forward (the router does offer UPnP and NAT-PMP), and this PC sends its traffic through a Mullvad exit node, so replies would leave by the wrong way. Not attempted |
| IPv6 | None on this PC |

**Not measured, because it needs going live:** Funnel itself carrying this (it should: it is a TLS-terminated TCP forward, which is what the loopback test emulates), Funnel with the exit node on, and how much it carries. Tailscale documents an undisclosed bandwidth limit for Funnel. Media here is TCP inside TLS through Tailscale's servers, so expect voice to be fine and screen share to be the first thing to suffer, and expect loss to show up as delay. go-online.sh checks the relay from the public side and says so if that fails. If call quality is not good enough, use the rented server below: chat and files stay as they are.

## How it fits together

```
friend --TLS--> Funnel :10000 --> tailscaled (takes TLS off) --> 127.0.0.1:17885 relay --front
                                                                   |- /rtc...     -> LiveKit 127.0.0.1:17880 (signalling)
                                                                   |- other HTTP  -> relay 127.0.0.1:17884
                                                                   `- STUN/TURN   -> LiveKit TURN :17883 -> media on 127.0.0.1:17882
```

What took finding out, all set by go-online.sh: LiveKit's `external_tls` makes TURN speak plain behind a terminator; it announces port 443 whatever you set, so the relay passes the real port to clients (`--turn-public-port`); TURN refuses private peers unless `allow_restricted_peer_cidrs` names the SFU; with `node_ip: 127.0.0.1` the `rtc.ips` filter is required, or LiveKit opens a socket per interface and calls fail at random; and with TURN over UDP off, LiveKit hands clients Google's STUN servers, which the client now strips. Never run `tailscale funnel reset`: it clears the other served ports too.

Other ways in, if you ever want them: **Tailscale device sharing** (friends install Tailscale; section A below) gives direct UDP and the best quality, at the cost of an install for each friend.

---

# The rented server

The fallback. Same software, and better calls: real UDP, no third party in the path.

## What you need

* A small VPS (1 vCPU, 512 MB-1 GB RAM, 10 GB disk is plenty: the relay holds ciphertext only; attachments dominate disk use).
* A domain name pointing at it (an `A`/`AAAA` record).
* Ports 80 and 443 open (80 only for the certificate challenge). The relay itself is never exposed directly.
* TLS: the relay speaks plain HTTP and expects a reverse proxy to terminate TLS. Request bodies are already end-to-end encrypted and requests are signed, but TLS hides the IP-level metadata from the path and protects the (public) cert/key-package traffic from tampering. Always use it.
* Docker and Docker Compose, or build the binary yourself (`cargo build --release -p commons-relay`, one static-ish binary, SQLite is bundled).

## The provided files

* `deploy/Dockerfile`: multi-stage build of `commons-relay`, runs as an unprivileged user, data in `/data`.
* `deploy/docker-compose.yml`: the relay (bound to the internal network only) and Caddy (automatic Let's Encrypt certificates) publishing 80/443.
* `deploy/Caddyfile`: reverse proxy with a body-size cap and long-poll friendly timeouts.

Steps, once you decide to:

```sh
# on the server
git clone <your private copy> commons && cd commons/deploy
echo 'COMMONS_DOMAIN=chat.example.org' > .env      # your domain
echo "COMMONS_RELAY_INVITE=$(openssl rand -hex 16)" >> .env   # share this with friends
docker compose up -d --build
curl https://chat.example.org/health                 # -> ok
```

In the app, set the relay to `https://chat.example.org` when creating an account (the invite links carry it, so friends do not type it). The app checks the address before it creates anything. If the relay moves with its data, change the address in Settings.

## Operating it

* **Backups**: copy `/data` (`relay.db` and `blobs/`). Losing the relay loses undelivered messages, key packages, and group logs; clients keep their own history, notice, and register again, and key packages are republished automatically, but groups would need re-inviting. SQLite is in WAL mode with `synchronous=FULL`; use `sqlite3 relay.db ".backup"` for a consistent copy while it runs.
* **Updates**: `git pull && docker compose up -d --build`. The wire protocol is versioned by path (`/v1`); stage 1 has no migration tooling. Groups in a database from before owners were recorded get their owner from the first member to claim them.
* **Who can use it**: set `COMMONS_RELAY_INVITE=<secret>` in the environment and new accounts need that secret (the app asks for it under Relay server). Use a long random one, for example `openssl rand -hex 16`, and share it the way you share a Wi-Fi password. A friend's second device or a recovery needs no secret, since the account is already known. Do this whenever the relay can be reached from outside, including over Tailscale Funnel; on a private tailnet the network is already a gate. The relay refuses to start on a non-loopback address without a secret; `--open` overrides that for a relay that is meant to be public. Tailscale gives you TLS with `tailscale serve` or `tailscale funnel`, so no Caddy is needed.
* **Limits** (per-account, in `Limits` in `crates/relay/src/lib.rs`): 1000 accounts (`--max-accounts`), 32 devices per account, 4 GiB of messages and 2 GiB of files per account (`--quota-mb` sets both), 500 items or 64 MB in a device's inbox, 20 open pairings, 256 KB per message and 25 MB per file. Rates per 10 minutes: 60 new accounts or wrong secrets, 10 join requests per device and 120 per community, 200 key packages taken per device and 2000 in all, 2000 invitations sent. The message quota is a lifetime figure, because the log is append-only and nothing expires yet.
* **Logs**: the relay logs only startup and errors. Caddy will log IPs by default; turn that off if you do not want them.
* **Metadata you accept by hosting**: see `THREAT-MODEL.md`. The host (or anyone who compromises it) sees the account-level membership graph, timing, sizes, and which device posts what, never content.
* **Rough cost**: a 3-5 EUR/month VPS and a domain.

## Voice rooms and screen share (stage 2): LiveKit

Calls need one more server process, **LiveKit** (an SFU: it forwards each participant's media to the others). Media is encrypted in the clients with keys from the room's MLS group, so the LiveKit server only ever forwards ciphertext (see THREAT-MODEL.md for what it can still see). The relay signs the short-lived join tokens; clients get them with `POST /v1/groups/{gid}/call-token`.

### Two ways to host it (both work, same software)

**A. On your own PC, friends reach it through Tailscale device sharing.** Nothing is opened on your router and no port is public, but every friend has to install Tailscale, and ports 443 and 8443 on this PC are taken, so use others.

1. Install Tailscale on the PC. In the admin console share the PC with each friend (Machines, Share). A shared machine is reachable from the friend's own tailnet at its tailnet name (`pc.your-tailnet.ts.net`) and address (`tailscale ip -4`, a 100.x.y.z). Friends install Tailscale and accept the share.
2. Run the relay bound to loopback and LiveKit bound to the PC: `deploy/livekit.yaml` with `use_external_ip: false`, `node_ip: <the PC's tailnet IPv4>`, `turn.enabled: false`, `stun_servers: []`.
3. Give both HTTP services real HTTPS/WSS with `tailscale serve`, because browsers and webviews only allow the microphone, screen capture and frame encryption on a secure page and refuse `ws://` from it: `tailscale serve --bg --https=443 http://127.0.0.1:8787` (relay) and `tailscale serve --bg --https=8443 http://127.0.0.1:7880` (LiveKit signalling, WebSocket works through it). The relay then runs with `--livekit-url wss://pc.your-tailnet.ts.net:8443`, and the app's relay URL is `https://pc.your-tailnet.ts.net`. (`tailscale serve` is tailnet-only; do **not** use `tailscale funnel`, which is public.)
4. Media does not go through `serve`: it goes straight to the PC's tailnet address over WireGuard. Tailscale does the NAT traversal; if two peers cannot connect directly it falls back to its DERP relays, so strict friend networks still work and **you do not need LiveKit's TURN** in this setup.

Ports (all on the PC, interface `tailscale0` only; allow nothing on your LAN/WAN interfaces):

| Port | Proto | Who | What |
| --- | --- | --- | --- |
| 443 | TCP | friends via `tailscale serve` | relay (HTTPS) |
| 8443 | TCP | friends via `tailscale serve` | LiveKit signalling (WSS) |
| 7881 | TCP | friends, direct to tailnet IP | media over TCP, used when UDP is blocked |
| 7882 | UDP | friends, direct to tailnet IP | all media (one UDP port, LiveKit "mux") |
| 7880, 8787 | TCP | loopback only | what `serve` forwards to; never exposed |

Firewall example: `sudo ufw allow in on tailscale0 to any port 443,8443,7881 proto tcp` and `sudo ufw allow in on tailscale0 to any port 7882 proto udp`. Tailscale's sharing ACL also lets you restrict friends to exactly those ports (an `acls` rule with `dst: ["pc:443,8443,7881,7882"]`). The PC must be on while anyone calls or chats; the relay's store-and-forward does not need it, but nobody else can reach it when it is off.

**B. A small rented server.** Same files, `deploy/docker-compose.yml` (`docker compose --profile calls up -d`), `use_external_ip: true`, the built-in TURN enabled with a real certificate, Caddy in front of the relay and LiveKit signalling. A 2 vCPU / 2 GB VPS with a domain name; about 5-8 EUR a month plus the domain. Steps: copy the repo there, in `deploy/` put `COMMONS_DOMAIN`, `COMMONS_LIVEKIT_DOMAIN`, `COMMONS_RELAY_INVITE` (`openssl rand -hex 24`), `COMMONS_LIVEKIT_URL=wss://<livekit domain>`, `COMMONS_LIVEKIT_API_KEY` and `COMMONS_LIVEKIT_API_SECRET` in `.env`, put the same key pair and the TURN domain in `livekit.yaml`, copy the certificate to `deploy/certs/`, open the ports below, and `docker compose --profile calls up -d --build`. Friends' links then start with your domain; nothing else changes, but accounts do not move across, because a different relay starts empty. Ports on the server's firewall:

| Port | Proto | What |
| --- | --- | --- |
| 80, 443 | TCP | Caddy: certificate challenge, relay, and LiveKit signalling on its own hostname |
| 7881 | TCP | media over TCP |
| 7882 | UDP | media (single mux port) |
| 3478 | UDP | TURN/STUN |
| 5349 | TCP | TURN over TLS (gets through firewalls that only allow 443-like traffic). LiveKit always announces this as 443, so the relay passes the real port to clients: `COMMONS_TURN_PUBLIC_PORT`, 5349 in the compose file |
| 50000-50100 | UDP | TURN relay ports (a small range is enough for a few friends) |

Without TURN, people on networks that block UDP can still connect over 7881/TCP; with TURN-TLS on 5349 (or better, TURN on 443 with its own IP/hostname) almost every network works. LiveKit's default configuration points clients at Google's and Twilio's public STUN servers whenever TURN over UDP is off; the client strips those, and with `udp_port` set as in `deploy/livekit.yaml` it does not send them at all.

### Setup steps (either way)

```sh
openssl rand -hex 32            # the API secret; the key name is arbitrary (COMMONS_KEY in the template)
# put "COMMONS_KEY: <secret>" under `keys:` in livekit.yaml, then start the relay with:
export COMMONS_LIVEKIT_API_KEY=COMMONS_KEY COMMONS_LIVEKIT_API_SECRET=<secret> COMMONS_LIVEKIT_URL=wss://...
commons-relay --bind 127.0.0.1:8787 --data ./relay-data     # or --livekit-url/--livekit-key/--livekit-secret-file
```

The relay refuses to start with a secret shorter than 32 characters. Whoever has the secret can mint tokens for any room and can join as an invisible listener, but still only hears ciphertext. Keep it in the relay's environment (or a root-only file), not in git.

### Server size and bandwidth (estimates, not measurements of a deployment)

LiveKit forwards packets and does not decode or mix, and our frame encryption happens in the clients, so the server does no crypto work and little CPU: one modest vCPU is plenty for a friend group. Bandwidth is what matters, and it is the server's *upload* that scales: each stream is sent once per viewer.

| Situation | Server download | Server upload |
| --- | --- | --- |
| Voice, 6 people, Opus about 32 kbps each, one talking at a time on average | 6 x 32 = about 0.2 Mbit/s | 6 x 5 x 32 = about 1 Mbit/s worst case (all sending) |
| One screen share at 1080p/15 fps for 5 viewers | 1.5-3 Mbit/s | 5 x 1.5-3 = 7-15 Mbit/s |
| Same, each viewer picking the low layer (360p) | the same | about 5 x 0.3 = 1.5 Mbit/s |
| Two sharers plus voice, 6 people, mixed quality | about 5 Mbit/s | about 15-25 Mbit/s |

Data: voice about 0.5 GB/hour for the whole room; a screen share to five viewers about 3-7 GB/hour. On the home PC the limit is your upstream line (a 20 Mbit/s uplink supports roughly one 1080p share to five viewers, or two at 720p); viewers can pick a lower quality in the app. Sizing: 1 vCPU / 1 GB RAM runs it, 2 vCPU / 2 GB is comfortable; `max_participants` in the template is 12. **Cost, honestly:** on your own PC it is electricity and your internet line. On a rented server about 4-8 EUR/month for a 2 vCPU / 2-4 GB VPS with 1 Gbit/s and 20 TB included traffic (several European providers offer this); the screen-share numbers above are far below any such traffic cap, so there is no per-GB bill at friend-group scale. Add the relay VPS cost from above if they are the same machine: it is the same machine. A domain is about 10 EUR/year. These are list-price estimates, not quotes.

## Not covered

High availability, multiple relays/federation, log expiry, push notifications (stage 2+).
