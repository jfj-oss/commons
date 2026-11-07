# Commons

A private, end-to-end encrypted group chat for a small group of friends: communities with rooms, invite links, roles, replies, reactions, files, local search. The relay server stores and forwards ciphertext and cannot read anything. Built on MLS (RFC 9420) via OpenMLS; no cryptography is implemented here.

**Stage 1 status: works end to end on one machine (integration tests plus an env-driven long soak), never deployed, never audited.** Read [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md) before trusting it with anything that matters.

* [docs/DESIGN.md](docs/DESIGN.md): architecture, the MLS mapping, multi-device, recovery, failure handling, stage 2 seams
* [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md): what is and is not protected, exactly what the relay sees
* [docs/DEPLOY.md](docs/DEPLOY.md) and `deploy/`: how it would run on your own PC over Tailscale or on a small rented server, including LiveKit for calls (unused)
* [docs/CALLS-PLATFORMS.md](docs/CALLS-PLATFORMS.md): voice rooms and screen share: where they work (the Linux desktop web view cannot) and the browser hand-off

## Layout

```
crates/proto      wire types, request signing, device certificates
crates/relay      relay server (axum + SQLite), binds 127.0.0.1 by default
crates/core       client library + JSON API + integration tests (crates/core/tests)
app/src-tauri     Tauri 2 desktop shell (Linux, macOS arm64)
app/ui            HTML/CSS/JS front end (no build step)
app/ui/call*.js, vendor/  voice rooms and screen share (LiveKit client, frame encryption keys from MLS)
tools/call-e2e.mjs  end-to-end call tests (local LiveKit + relay + Chrome with fake devices), tools/webkit-probe.py
tools/ui-check.mjs  headless UI check (Chromium on the GPU via ANGLE/Vulkan, no window)
scripts/try-two-windows.sh
```

## Build and test

Rust 1.91+ (`rustc --version`), no sudo needed. On Linux the desktop shell also needs `webkit2gtk-4.1` and GTK 3 development packages.

```sh
cargo test                      # proto + relay + core + the whole integration harness (about a minute, mostly compiling)
cargo test -p commons-core --test reliability     # one suite
CHAOS_SEEDS=60 CHAOS_STEPS=250 cargo test -p commons-core --release --test simulation chaos_soak_env -- --nocapture   # long soak
FUZZ_ITERS=2000000 cargo test -p commons-core --release --test fuzz_parse   # long fuzz
```

Wrapping builds on a busy machine: `systemd-run --user --scope -q -p MemoryMax=6G cargo test -j 2`.

## Try it with two windows on one PC

```sh
scripts/try-two-windows.sh            # local relay on 127.0.0.1:8787 + two Tauri windows (two independent people)
scripts/try-two-windows.sh browser    # same with two browser tabs (127.0.0.1:7001 and :7002) instead of Tauri
```

1. In each window create an account (name + passphrase; the first window shows a recovery key: save it).
2. In window A: switcher > New community. Click **Invite**, **Create link**, copy the link.
3. In window B: switcher > **Join with an invite link**, paste. After a few seconds A lets B in and the rooms appear.
4. Chat, edit, react, reply, attach files, open **People** > padlock to compare the safety number, open **Devices** to see the second-device and recovery options, **!** for the activity log.

To add a second device of the same person: in a fresh window choose "This is a new device for my account", copy the code it shows, and in the first device open Devices > paste > Approve.

Manual relay: `cargo run -p commons-relay -- --bind 127.0.0.1:8787 --data ./relay-data`. Set `COMMONS_RELAY_INVITE=<secret>` to require a secret from new accounts (see docs/DEPLOY.md). Data directories: `COMMONS_DATA=/some/dir` per instance; the relay URL default comes from `COMMONS_RELAY`.

## Packaging

`cargo install tauri-cli --version '^2'` then `cargo tauri build` in `app/src-tauri` (run `cargo tauri icon app/src-tauri/icons/icon.png` first on macOS to produce `.icns`). macOS arm64 builds have been configured but not built or run in this environment (Linux only).

## Voice rooms and screen share (stage 2)

Any room has a **Call** button: voice, screen share, camera, end-to-end encrypted with keys from the room's MLS group (docs/DESIGN.md section 11). Needs a LiveKit server and a relay configured with its key (docs/DEPLOY.md). Local test setup, nothing leaves 127.0.0.1:

```sh
mkdir -p tools/bin && # download livekit-server 1.13.11 linux amd64 from github.com/livekit/livekit/releases into tools/bin/ (git-ignored)
cd tools && npm install && cd ..
cargo build -j2 -p commons-relay && cargo build -j2 -p commons-core --features devbridge --bin dev_bridge
systemd-run --user --scope -q -p MemoryMax=6G node tools/call-e2e.mjs      # about 8 minutes, one Chrome for Testing, fake media only
cargo test -j2 -p commons-core --test calls
```
