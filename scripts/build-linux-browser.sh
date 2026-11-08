#!/usr/bin/env bash
# The Linux "browser version" for friends: the app's core as a small local server plus the UI, opened in Brave/Chrome/
# Chromium/Edge (packaging/linux-browser). Built inside Ubuntu 22.04 like the AppImage, so it runs on Ubuntu 22.04 and
# newer, Debian 12, Fedora, Arch. Needs Docker. Writes dist/Megatron-linux-browser.tar.gz.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$(realpath -m "${COMMONS_OUT:-dist}")
# The version and the updater public key go into the bridge at compile time: the version it reports, and the only key it
# will accept an update from (the same key as the desktop app, plugins.updater.pubkey in tauri.conf.json).
CONF=app/src-tauri/tauri.conf.json
VER=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$CONF" | head -1)
PUB=$(sed -n 's/^ *"pubkey": *"\([^"]*\)".*/\1/p' "$CONF" | head -1)
if [ -n "${COMMONS_TAURI_CONFIG:-}" ]; then
  command -v jq >/dev/null || { echo "build-linux-browser: jq is needed with COMMONS_TAURI_CONFIG" >&2; exit 1; }
  v=$(jq -r '.version // empty' <<<"$COMMONS_TAURI_CONFIG"); [ -z "$v" ] || VER=$v
  p=$(jq -r '.plugins.updater.pubkey // empty' <<<"$COMMONS_TAURI_CONFIG"); [ -z "$p" ] || PUB=$p
fi
[ -n "$VER" ] && [ "$VER" != null ] || { echo "build-linux-browser: no version" >&2; exit 1; }
[ -n "$PUB" ] && [ "$PUB" != null ] || { echo "build-linux-browser: no updater public key in $CONF" >&2; exit 1; }
mkdir -p "$OUT" target-docker
docker run --rm --memory=6g --cpus="${COMMONS_BUILD_JOBS:-4}" -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target-docker/t \
  -e CARGO_HOME=/src/target-docker/cargo -e RUSTUP_HOME=/src/target-docker/rustup -e CARGO_BUILD_JOBS="${COMMONS_BUILD_JOBS:-4}" \
  -e MEGATRON_BROWSER_VERSION="$VER" -e MEGATRON_BROWSER_PUBKEY="$PUB" \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" ubuntu:22.04 bash -euc '
  export DEBIAN_FRONTEND=noninteractive PATH=/src/target-docker/cargo/bin:$PATH
  apt-get update -qq && apt-get install -y -qq curl ca-certificates build-essential pkg-config libssl-dev >/dev/null
  command -v cargo >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
  cargo build -q --release -p commons-core --features devbridge --bin dev_bridge
  chown -R "$HOST_UID:$HOST_GID" /src/target-docker'
S=$(mktemp -d); P="$S/megatron-browser"; mkdir -p "$P"
cp target-docker/t/release/dev_bridge "$P/bridge"; strip "$P/bridge" 2>/dev/null || true
cp -r app/ui "$P/ui"
cp packaging/linux-browser/megatron packaging/linux-browser/install.sh packaging/linux-browser/README.txt "$P/"
cp app/src-tauri/default-relay.txt "$P/relay.txt"; cp app/src-tauri/icons/icon.png "$P/megatron.png"
chmod +x "$P/bridge" "$P/megatron" "$P/install.sh"
tar -C "$S" -czf "$OUT/Megatron-linux-browser.tar.gz" megatron-browser
rm -rf "$S"; ls -la "$OUT/Megatron-linux-browser.tar.gz"
