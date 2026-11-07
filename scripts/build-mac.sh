#!/usr/bin/env bash
# Builds Megatron.app and a .dmg on a Mac (Apple silicon) with only Rust and the Xcode command line tools: no Node, no Tauri CLI.
# Kept light on purpose, for a small laptop: two jobs, no LTO, background priority (efficiency cores). Output in dist/.
# COMMONS_MAC_FULL=1 uses every core and the normal release settings.
set -euo pipefail
cd "$(dirname "$0")/.."
ID=app.commons.chat
VER=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' app/src-tauri/tauri.conf.json | head -1)
if [ -z "${COMMONS_MAC_FULL:-}" ]; then
  export CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_BUILD_JOBS=2
  RUN=(taskpolicy -b nice -n 19)
else RUN=(); fi
# custom-protocol is what puts the UI inside the binary; the Tauri CLI normally adds it.
"${RUN[@]}" cargo build --release -p commons-app --features tauri/custom-protocol
A=dist/Megatron.app
rm -rf "$A"; mkdir -p "$A/Contents/MacOS" "$A/Contents/Resources"
cp target/release/commons-app "$A/Contents/MacOS/commons-app"
cp app/src-tauri/icons/icon.icns "$A/Contents/Resources/icon.icns"
cat > "$A/Contents/Info.plist" <<P
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Megatron</string><key>CFBundleDisplayName</key><string>Megatron</string>
<key>CFBundleExecutable</key><string>commons-app</string><key>CFBundleIdentifier</key><string>$ID</string>
<key>CFBundleVersion</key><string>$VER</string><key>CFBundleShortVersionString</key><string>$VER</string>
<key>CFBundlePackageType</key><string>APPL</string><key>CFBundleIconFile</key><string>icon</string>
<key>LSMinimumSystemVersion</key><string>12.0</string><key>NSHighResolutionCapable</key><true/>
$(sed -n '/<key>NS/,/<\/string>/p' app/src-tauri/Info.plist)
</dict></plist>
P
plutil -lint "$A/Contents/Info.plist"
codesign --force --deep --sign - "$A"       # ad-hoc: Apple silicon refuses to run anything unsigned
codesign --verify --strict "$A"
rm -f dist/Megatron-macos-arm64.dmg
hdiutil create -quiet -volname Megatron -srcfolder "$A" -ov -format UDZO dist/Megatron-macos-arm64.dmg
ls -la dist
