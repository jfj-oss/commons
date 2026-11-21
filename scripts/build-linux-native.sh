#!/usr/bin/env bash
# Builds the native (GPUI) mega for Linux into a tarball: dist/mega-linux-x86_64.tar.gz (or -aarch64). Inside: the stripped
# binary, hicolor icons made from the Tauri app's icon, the desktop entry template, install.sh and uninstall.sh.
#   scripts/build-linux-native.sh [--no-build]     (--no-build packs an existing target/release/mega-gpui)
# MEGA_GPUI_VERSION (default: the Tauri app's version) is what the app compares against the update feed.
# Build on the oldest system you support (the binary links the system's glibc and needs Vulkan at run time).
set -euo pipefail
cd "$(dirname "$0")/.."
VER=${MEGA_GPUI_VERSION:-$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' app/src-tauri/tauri.conf.json | head -1)}
ARCH=$(uname -m)
TARGET=${CARGO_TARGET_DIR:-target}
if [ "${1:-}" != "--no-build" ]; then MEGA_GPUI_VERSION=$VER cargo build --release -p mega-gpui --bin mega-gpui; fi
BIN=$TARGET/release/mega-gpui
[ -x "$BIN" ] || { echo "no $BIN" >&2; exit 1; }
NAME=mega-linux-$ARCH
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
P=$W/$NAME
mkdir -p "$P/icons"
cp "$BIN" "$P/mega-gpui"; strip --strip-all "$P/mega-gpui"; chmod 755 "$P/mega-gpui"
ICON=app/src-tauri/icons/icon.png
for s in 16 24 32 48 64 128 256 512; do
  if [ $s -eq 512 ]; then cp "$ICON" "$P/icons/$s.png"; else convert "$ICON" -resize ${s}x${s} "$P/icons/$s.png"; fi
done
cp packaging/linux-native/install.sh packaging/linux-native/uninstall.sh packaging/linux-native/README.txt "$P/"
chmod 755 "$P/install.sh" "$P/uninstall.sh"
echo "$VER" > "$P/VERSION"
mkdir -p dist
tar -C "$W" -czf "dist/$NAME.tar.gz" "$NAME"
echo "dist/$NAME.tar.gz  ($(du -h "dist/$NAME.tar.gz" | cut -f1), version $VER)"
