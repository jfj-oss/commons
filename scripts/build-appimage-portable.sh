#!/usr/bin/env bash
# The AppImage for friends. One built on a rolling distro needs that distro's glibc and fails on everything older, so this
# builds inside Ubuntu 22.04 (glibc 2.35): the result runs on Ubuntu 22.04 and newer, Debian 12, Fedora, Arch.
# Needs Docker. Writes dist/Commons-linux-x86_64.AppImage. Uses its own cache in target-docker/ (git-ignored).
# COMMONS_OUT changes the output folder; COMMONS_TAURI_CONFIG is merged into tauri.conf.json (scripts/publish-update.sh).
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$(realpath -m "${COMMONS_OUT:-dist}")
mkdir -p "$OUT" target-docker
docker run --rm --memory=6g --cpus="${COMMONS_BUILD_JOBS:-4}" -v "$PWD":/src -v "$OUT":/out -w /src -e CARGO_TARGET_DIR=/src/target-docker/t -e CARGO_HOME=/src/target-docker/cargo -e RUSTUP_HOME=/src/target-docker/rustup \
  -e APPIMAGE_EXTRACT_AND_RUN=1 -e CARGO_BUILD_JOBS="${COMMONS_BUILD_JOBS:-4}" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -e COMMONS_TAURI_CONFIG="${COMMONS_TAURI_CONFIG:-}" ubuntu:22.04 bash -euc '
  export DEBIAN_FRONTEND=noninteractive PATH=/src/target-docker/cargo/bin:$PATH
  apt-get update -qq
  apt-get install -y -qq curl ca-certificates build-essential pkg-config file patchelf libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev xdg-utils >/dev/null
  command -v cargo >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
  curl -fsSL https://nodejs.org/dist/v22.11.0/node-v22.11.0-linux-x64.tar.xz | tar -xJ --strip-components=1 -C /usr/local
  mkdir -p /tmp/tools && cp /src/tools/package.json /src/tools/package-lock.json /tmp/tools/
  cd /tmp/tools && npm ci >/dev/null
  B=/src/target-docker/t/release/bundle/appimage
  rm -f $B/*.AppImage
  cfg=(); [ -z "$COMMONS_TAURI_CONFIG" ] || cfg=(--config "$COMMONS_TAURI_CONFIG")
  cd /src/app/src-tauri && /tmp/tools/node_modules/.bin/tauri build --bundles appimage "${cfg[@]}"
  set -- $B/Commons_*_amd64.AppImage; [ $# = 1 ] || { echo "expected one AppImage, found: $*" >&2; exit 1; }
  cp "$1" /out/Commons-linux-x86_64.AppImage
  chown -R "$HOST_UID:$HOST_GID" /out /src/target-docker /src/app/src-tauri/gen
'
ls -la "$OUT"
