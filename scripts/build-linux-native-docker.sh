#!/usr/bin/env bash
# The native (GPUI) mega for Linux, built inside Ubuntu 22.04 (glibc 2.35) like the AppImage, so what friends and the
# self-updater get runs on Ubuntu 22.04 and newer, Debian 12, Fedora, Arch. Needs Docker. Writes, into COMMONS_OUT (default
# dist/): mega-linux-x86_64.tar.gz and mega-gpui-linux-x86_64 (scripts/build-linux-native.sh makes both; this only gives it
# the right system). MEGA_GPUI_VERSION is what the app compares against the feed (scripts/publish-update.sh sets it).
# Cache: target-docker/native-t (git-ignored; shares the cargo and rustup folders of build-appimage-portable.sh).
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$(realpath -m "${COMMONS_OUT:-dist}")
mkdir -p "$OUT" target-docker
docker run --rm --memory=8g --cpus="${COMMONS_BUILD_JOBS:-4}" -v "$PWD":/src -v "$OUT":/out -w /src -e CARGO_TARGET_DIR=/src/target-docker/native-t -e CARGO_HOME=/src/target-docker/cargo -e RUSTUP_HOME=/src/target-docker/rustup \
  -e CARGO_BUILD_JOBS="${COMMONS_BUILD_JOBS:-4}" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -e MEGA_GPUI_VERSION="${MEGA_GPUI_VERSION:-}" -e MEGA_GPUI_UPDATE_PUBKEY="${MEGA_GPUI_UPDATE_PUBKEY:-}" ubuntu:22.04 bash -euc '
  export DEBIAN_FRONTEND=noninteractive PATH=/src/target-docker/cargo/bin:$PATH
  apt-get update -qq
  # the same list as the native-linux job in .github/workflows/apps.yml
  apt-get install -y -qq curl ca-certificates build-essential pkg-config cmake clang libssl-dev libasound2-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig1-dev libfreetype6-dev libvulkan-dev libdbus-1-dev libpulse-dev libglib2.0-dev libgbm-dev imagemagick >/dev/null
  command -v cargo >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
  [ -n "$MEGA_GPUI_VERSION" ] || unset MEGA_GPUI_VERSION
  [ -n "$MEGA_GPUI_UPDATE_PUBKEY" ] || unset MEGA_GPUI_UPDATE_PUBKEY
  COMMONS_OUT=/out scripts/build-linux-native.sh
  chown -R "$HOST_UID:$HOST_GID" /out /src/target-docker
'
ls -la "$OUT"
