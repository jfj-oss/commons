#!/usr/bin/env bash
# The Linux "browser version" for friends: the app's core as a small local server plus the UI, opened in Brave/Chrome/
# Chromium/Edge (packaging/linux-browser). Built inside Ubuntu 22.04 like the AppImage, so it runs on Ubuntu 22.04 and
# newer, Debian 12, Fedora, Arch. Needs Docker. Writes dist/Megatron-linux-browser.tar.gz.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$(realpath -m "${COMMONS_OUT:-dist}")
mkdir -p "$OUT" target-docker
docker run --rm --memory=6g --cpus="${COMMONS_BUILD_JOBS:-4}" -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target-docker/t \
  -e CARGO_HOME=/src/target-docker/cargo -e RUSTUP_HOME=/src/target-docker/rustup -e CARGO_BUILD_JOBS="${COMMONS_BUILD_JOBS:-4}" \
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
