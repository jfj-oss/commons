# Hosting the relay (not done, not required for trying it)

Nothing has been deployed and nothing listens on a public port. The relay binds `127.0.0.1` by default and warns if you point it elsewhere. This is what hosting it for a few friends would take.

## What you need

* A small VPS (1 vCPU, 512 MB-1 GB RAM, 10 GB disk is plenty: the relay holds ciphertext only; attachments dominate disk use).
* A domain name pointing at it (an `A`/`AAAA` record).
* Ports 80 and 443 open (80 only for the certificate challenge). The relay itself is never exposed directly.
* TLS: the relay speaks plain HTTP and expects a reverse proxy to terminate TLS. Request bodies are already end-to-end encrypted and requests are signed, but TLS hides the IP-level metadata from the path and protects the (public) cert/key-package traffic from tampering. Always use it.
* Docker and Docker Compose, or build the binary yourself (`cargo build --release -p commons-relay`, one static-ish binary, SQLite is bundled).

## The provided files (unused)

* `deploy/Dockerfile`: multi-stage build of `commons-relay`, runs as an unprivileged user, data in `/data`.
* `deploy/docker-compose.yml`: the relay (bound to the internal network only) and Caddy (automatic Let's Encrypt certificates) publishing 80/443.
* `deploy/Caddyfile`: reverse proxy with a body-size cap and long-poll friendly timeouts.

Steps, once you decide to:

```sh
# on the server
git clone <your private copy> commons && cd commons/deploy
echo 'COMMONS_DOMAIN=chat.example.org' > .env      # your domain
docker compose up -d --build
curl https://chat.example.org/health                 # -> ok
```

In the app, set the relay to `https://chat.example.org` when creating an account (the invite links carry it, so friends do not type it).

## Operating it

* **Backups**: copy `/data` (`relay.db` and `blobs/`). Losing the relay loses undelivered messages, key packages, and group logs; clients keep their own history and key packages are republished automatically, but groups would need re-inviting. SQLite is in WAL mode with `synchronous=FULL`; use `sqlite3 relay.db ".backup"` for a consistent copy while it runs.
* **Updates**: `git pull && docker compose up -d --build`. The wire protocol is versioned by path (`/v1`); stage 1 has no migration tooling.
* **Abuse and cost**: there is no registration gate in stage 1: anyone who can reach the server can register a device and use storage (per-request caps exist: 512 KB messages, 25 MB blobs, key package and inbox quotas, no per-account quota). For friends only, restrict by network (VPN/Tailscale, firewall allow-list) or put HTTP basic auth in front for registration. A shared-secret registration token is a small, obvious addition.
* **Logs**: the relay logs only startup and errors. Caddy will log IPs by default; turn that off if you do not want them.
* **Metadata you accept by hosting**: see `THREAT-MODEL.md`. The host (or anyone who compromises it) sees the account-level membership graph, timing, sizes, and which device posts what, never content.
* **Rough cost**: a 3-5 EUR/month VPS and a domain.

## Not covered

High availability, multiple relays/federation, rate limiting per account, push notifications (stage 2+).
