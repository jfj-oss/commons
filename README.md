# Megatron

A private, end-to-end encrypted group chat for a small group of friends: communities with rooms, invite links, roles, replies, reactions, files, local search. The relay server stores and forwards ciphertext and cannot read anything. Built on MLS (RFC 9420) via OpenMLS; no cryptography is implemented here.

**Status: chat, files, calls and screen share work end to end, tested on one machine (loopback), including the packaged Linux app and the hosted path. Not yet used over the real internet, never audited.** Read [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md) before trusting it with anything that matters.

**Friends: [docs/FRIENDS.md](docs/FRIENDS.md).** Host: `deploy/go-online.sh` ([docs/DEPLOY.md](docs/DEPLOY.md)). Just trying it, calls included, on this one computer: `scripts/run-local.sh`.

* [docs/DESIGN.md](docs/DESIGN.md): architecture, the MLS mapping, multi-device, recovery, failure handling, stage 2 seams
* [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md): what is and is not protected, exactly what the relay sees
* [docs/DEPLOY.md](docs/DEPLOY.md) and `deploy/`: hosting on your own PC through Tailscale Funnel, or on a small rented server, including LiveKit for calls
* [docs/GAME-KEYS.md](docs/GAME-KEYS.md): per-game push-to-mute, and which modes really work on which system
* [docs/UPDATES.md](docs/UPDATES.md): signed auto-updates from the relay, publishing a version, backing up the signing key, compatibility between versions
* [docs/CALLS-PLATFORMS.md](docs/CALLS-PLATFORMS.md): voice rooms and screen share: where they work (the Linux desktop web view cannot) and the browser hand-off

## Layout

```
crates/proto      wire types, request signing, device certificates
crates/relay      relay server (axum + SQLite), binds 127.0.0.1 by default
crates/core       client library + JSON API + integration tests (crates/core/tests)
crates/gamekeys   per-game push-to-mute: the decision logic, and passive key and process watching for each system
app/src-tauri     Tauri 2 desktop shell (Linux, macOS arm64)
app/ui            HTML/CSS/JS front end (no build step)
app/ui/call*.js, vendor/  voice rooms and screen share (LiveKit client, frame encryption keys from MLS)
tools/call-e2e.mjs  end-to-end call tests (local LiveKit + relay + Chrome with fake devices), tools/webkit-probe.py
tools/call-play-e2e.mjs  playing into a call: a local file, the canvas fallback, a stand-in Jellyfin (Quick Connect), local music from a stand-in rune, two clients
tools/ui-check.mjs  headless UI check (Chromium on the GPU via ANGLE/Vulkan, no window)
tools/hosting-e2e.mjs  the hosted path on loopback: go-online.sh, a TLS terminator standing in for Funnel, media forced through TURN over TLS
tools/packaged-e2e.mjs the AppImage on a clean Ubuntu in Docker, through to a call
tools/update-e2e.mjs   signed updates end to end: the AppImage updating itself from a throwaway relay, and refusing tampered ones
tools/compat-e2e.mjs   an older and this version in one community, both ways, on the new and the old relay
scripts/               run-local.sh (everything on one PC), try-two-windows.sh, build-apps.sh, build-appimage-portable.sh, build-mac.sh, get-livekit.sh,
                       publish-update.sh (one command: version bump, all three builds, signing, into dist/ for the relay)
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
scripts/try-two-windows.sh            # local relay on 127.0.0.1:18787 + two Tauri windows (two independent people)
scripts/try-two-windows.sh browser    # same with two browser tabs (127.0.0.1:7001 and :7002) instead of Tauri
```

1. In each window create an account (name + passphrase; the first window shows a recovery key: save it).
2. In window A: switcher > New community. Click **Invite**, **Create link**, copy the link.
3. In window B: switcher > **Join with an invite link**, paste. After a few seconds A lets B in and the rooms appear.
4. Chat, edit, react, reply, attach files, open **People** > padlock to compare the safety number, open **Devices** to see the second-device and recovery options, **!** for the activity log.

To add a second device of the same person: in a fresh window choose "This is a new device for my account", copy the code it shows, and in the first device open Devices > paste > Approve.

Port 8787 is taken on the development PC; `COMMONS_TRY_PORT` picks another. Manual relay: `cargo run -p commons-relay -- --bind 127.0.0.1:18787 --data ./relay-data`. Set `COMMONS_RELAY_INVITE=<secret>` to require a secret from new accounts (see docs/DEPLOY.md). Data directories: `COMMONS_DATA=/some/dir` per instance; the relay URL default comes from `COMMONS_RELAY`.

## Packaging

**Releases:** `scripts/publish-update.sh` does all of the below, signs the update files and publishes them for every installed app to pick up ([docs/UPDATES.md](docs/UPDATES.md)).

The Tauri CLI comes from `tools/` (`cd tools && npm ci`). Unsigned builds, all of them; docs/FRIENDS.md says what friends see.

* **Linux**: `scripts/build-appimage-portable.sh` builds in Ubuntu 22.04 under Docker and writes `dist/Commons-linux-x86_64.AppImage`, which runs on Ubuntu 22.04 and newer. `scripts/build-apps.sh` builds on this machine, but that AppImage needs this machine's glibc (2.44 on Arch), so it is for you only.
* **macOS** (Apple silicon): on a Mac with Rust and the command line tools, `scripts/build-mac.sh` writes `dist/Commons.app` and `dist/Commons-macos-arm64.dmg`. No Node needed. It runs at background priority with two jobs, for small laptops; `COMMONS_MAC_FULL=1` uses every core.
* **Windows**: `scripts/build-windows.sh` cross-builds in Docker and writes `dist/Commons-windows-x64-setup.exe`, a per-user NSIS installer, which is also what the updater installs (`COMMONS_WIN_ZIP=1` adds the old unzip-and-run `.zip`, which cannot update itself). **Built, never run on Windows.** For a real installer, `.github/workflows/apps.yml` builds all three on GitHub: push to a **private** GitHub repository, `gh workflow run apps.yml`, then `gh run download`. Or on any Windows PC with Rust, Node and Git Bash: `cd tools && npm ci && cd ../app/src-tauri && ../../tools/node_modules/.bin/tauri build --bundles nsis`.

## Voice rooms and screen share (stage 2)

Any room has a **Call** button: voice, screen share, camera, end-to-end encrypted with keys from the room's MLS group (docs/DESIGN.md section 11). Needs a LiveKit server and a relay configured with its key.

```sh
scripts/run-local.sh            # relay + LiveKit + two app windows; nothing leaves 127.0.0.1
scripts/run-local.sh browser    # the same with two browser tabs, where the call runs in the tab
```

The Linux app's web view has no WebRTC, so there the Call button opens the call in your browser (docs/CALLS-PLATFORMS.md). The tests:

```sh
scripts/get-livekit.sh          # livekit-server 1.13.11 into tools/bin/ (git-ignored), checksum verified
cd tools && npm ci && cd ..
cargo build -j2 -p commons-relay && cargo build -j2 -p commons-core --features devbridge --bin dev_bridge && cargo build --release -j2 -p commons-relay
systemd-run --user --scope -q -p MemoryMax=6G node tools/call-e2e.mjs      # about 8 minutes, one Chrome for Testing, fake media only
systemd-run --user --scope -q -p MemoryMax=8G node tools/call-play-e2e.mjs # play a video into a call, about 5 minutes
cargo test -j2 -p commons-core --test calls
systemd-run --user --scope -q -p MemoryMax=6G node tools/hosting-e2e.mjs    # the hosted path, 2 minutes
docker build -t commons-e2e-ubuntu -f tools/e2e.Dockerfile tools
systemd-run --user --scope -q -p MemoryMax=6G node tools/packaged-e2e.mjs   # the AppImage, game keys included, 3 minutes
node --test tools/mic-gate.test.mjs                                         # who decides about the mic
```
