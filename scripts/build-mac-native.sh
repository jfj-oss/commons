#!/usr/bin/env bash
# Builds the native (GPUI) mega as "mega.app" on a Mac (Apple silicon or Intel): dist/mega.app and dist/mega-macos-<arch>.app.tar.gz.
# Needs only Rust and the Xcode command line tools. Ad-hoc signed (Apple silicon refuses unsigned code); the app seals itself
# again after a self-update. The bundle matches the manual install "/Applications/mega native.app": same layout, the
# executable is Contents/MacOS/mega-gpui, the version is MEGA_GPUI_VERSION.
#   scripts/build-mac-native.sh [--no-build]     (--no-build bundles an existing target/release/mega-gpui)
# COMMONS_MAC_JOBS=N limits the build jobs (default 4).
set -euo pipefail
cd "$(dirname "$0")/.."
ID=app.commons.chat.gpui
VER=${MEGA_GPUI_VERSION:-$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' app/src-tauri/tauri.conf.json | head -1)}
TARGET=${CARGO_TARGET_DIR:-target}
if [ "${1:-}" != "--no-build" ]; then MEGA_GPUI_VERSION=$VER cargo build -j "${COMMONS_MAC_JOBS:-4}" --release -p mega-gpui --bin mega-gpui; fi
BIN=$TARGET/release/mega-gpui
[ -f "$BIN" ] || { echo "no $BIN" >&2; exit 1; }
A=dist/mega.app
rm -rf "$A"; mkdir -p "$A/Contents/MacOS" "$A/Contents/Resources"
cp "$BIN" "$A/Contents/MacOS/mega-gpui"; chmod 755 "$A/Contents/MacOS/mega-gpui"
strip -x "$A/Contents/MacOS/mega-gpui" 2>/dev/null || true
# the icon: the bull in the same squircle as rune, on Apple's 824 px icon grid
cp packaging/native-icon/icon.icns "$A/Contents/Resources/icon.icns"
cat > "$A/Contents/Info.plist" <<P
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>mega</string><key>CFBundleDisplayName</key><string>mega</string>
<key>CFBundleExecutable</key><string>mega-gpui</string><key>CFBundleIdentifier</key><string>$ID</string>
<key>CFBundleVersion</key><string>$VER</string><key>CFBundleShortVersionString</key><string>$VER</string>
<key>CFBundlePackageType</key><string>APPL</string><key>CFBundleIconFile</key><string>icon</string>
<key>LSMinimumSystemVersion</key><string>12.0</string><key>NSHighResolutionCapable</key><true/>
$(sed -n '/<key>NS/,/<\/string>/p' app/src-tauri/Info.plist)
</dict></plist>
P
plutil -lint "$A/Contents/Info.plist"
codesign --force --deep --sign - "$A"
codesign --verify --strict "$A"
ARCH=$(uname -m); [ "$ARCH" = arm64 ] || ARCH=x86_64
rm -f "dist/mega-macos-$ARCH.app.tar.gz"
(cd dist && COPYFILE_DISABLE=1 tar -czf "mega-macos-$ARCH.app.tar.gz" mega.app)
echo "dist/mega.app and dist/mega-macos-$ARCH.app.tar.gz (version $VER)"
