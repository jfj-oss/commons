#!/usr/bin/env bash
# Builds the macOS "browser version" on a Mac (Apple silicon) with only Rust and the Xcode command line tools, like
# scripts/build-mac.sh: two jobs, no LTO, background priority (efficiency cores). Writes
#   dist/Megatron-macos-browser.zip      Megatron.app for friends (drag to Applications, right-click Open the first time)
#   dist/Megatron-macos-browser.tar.gz   what the signed self-update installs (bridge, ui/)
# The version and updater public key are compiled into the bridge and launcher, exactly as in
# scripts/build-linux-browser.sh; COMMONS_TAURI_CONFIG may provide them instead (publish-update.sh does).
# COMMONS_MAC_FULL=1 uses every core and the normal release settings.
set -euo pipefail
cd "$(dirname "$0")/.."
ID=app.commons.chat.browser
CONF=app/src-tauri/tauri.conf.json
VER=${MEGATRON_BROWSER_VERSION:-$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$CONF" | head -1)}
PUB=${MEGATRON_BROWSER_PUBKEY:-$(sed -n 's/^ *"pubkey": *"\([^"]*\)".*/\1/p' "$CONF" | head -1)}
if [ -n "${COMMONS_TAURI_CONFIG:-}" ]; then
  command -v python3 >/dev/null || { echo "build-mac-browser: python3 is needed with COMMONS_TAURI_CONFIG" >&2; exit 1; }
  eval "$(python3 - "$CONF" "$COMMONS_TAURI_CONFIG" <<'PY'
import json, sys
base, extra = json.load(open(sys.argv[1])), json.loads(sys.argv[2])
v = extra.get("version") or base.get("version")
k = (extra.get("plugins") or {}).get("updater", {}).get("pubkey") or (base.get("plugins") or {}).get("updater", {}).get("pubkey")
print(f"VER={json.dumps(v)}")
print(f"PUB={json.dumps(k)}")
PY
)"
fi
[ -n "$VER" ] && [ "$VER" != null ] || { echo "build-mac-browser: no version" >&2; exit 1; }
[ -n "$PUB" ] && [ "$PUB" != null ] || { echo "build-mac-browser: no updater public key in $CONF" >&2; exit 1; }
# COMMONS_MAC_JOBS=N: the same light settings, but N jobs at normal priority (fast cores), for a Mac on power.
if [ -n "${COMMONS_MAC_JOBS:-}" ]; then
  export CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_BUILD_JOBS="$COMMONS_MAC_JOBS"
  RUN=()
elif [ -z "${COMMONS_MAC_FULL:-}" ]; then
  export CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_BUILD_JOBS=2
  RUN=(taskpolicy -b nice -n 19)
else RUN=(); fi
export MEGATRON_BROWSER_VERSION="$VER" MEGATRON_BROWSER_PUBKEY="$PUB"
"${RUN[@]}" cargo build --release -p commons-core --features devbridge --bin dev_bridge --bin megatron_launcher
# A staging folder, not dist/Megatron.app: scripts/build-mac.sh puts the desktop app there, and publish-update.sh
# packages that one for the desktop updater. The zip still contains Megatron.app at its top level.
STAGE=dist/.mac-browser-app
A=$STAGE/Megatron.app
rm -rf "$STAGE"; mkdir -p "$A/Contents/MacOS" "$A/Contents/Resources"
# Only the launcher lives in Contents/MacOS: codesign --deep treats every file there as nested code and signs it with
# an extended attribute, which would not survive a plain unzip. The bridge and ui/ go in Contents/Resources, and the
# launcher looks for them there (install_dir() in crates/core/src/bin/megatron_launcher.rs).
cp target/release/megatron_launcher "$A/Contents/MacOS/launcher"
cp target/release/dev_bridge "$A/Contents/Resources/bridge"
cp -r app/ui "$A/Contents/Resources/ui"
cp app/src-tauri/default-relay.txt "$A/Contents/Resources/relay.txt"
cp app/src-tauri/icons/icon.icns "$A/Contents/Resources/icon.icns"
cat > "$A/Contents/Info.plist" <<P
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>mega</string><key>CFBundleDisplayName</key><string>mega</string>
<key>CFBundleExecutable</key><string>launcher</string><key>CFBundleIdentifier</key><string>$ID</string>
<key>CFBundleVersion</key><string>$VER</string><key>CFBundleShortVersionString</key><string>$VER</string>
<key>CFBundlePackageType</key><string>APPL</string><key>CFBundleIconFile</key><string>icon</string>
<key>LSMinimumSystemVersion</key><string>12.0</string><key>NSHighResolutionCapable</key><true/>
<key>LSUIElement</key><true/>
</dict></plist>
P
plutil -lint "$A/Contents/Info.plist"
codesign --force --deep --sign - "$A"       # ad-hoc: Apple silicon refuses to run anything unsigned
codesign --verify --strict "$A"
rm -f dist/Megatron-macos-browser.zip dist/Megatron-macos-browser.tar.gz
# Megatron.app and the README, side by side at the top of the zip (ditto without --keepParent stores the folder's
# contents; --keepParent would put a stray .mac-browser-zip/ inside it).
Z=dist/.mac-browser-zip; rm -rf "$Z"; mkdir -p "$Z"
ditto "$A" "$Z/Megatron.app"
cp packaging/macos-browser/README.txt "$Z/"
xattr -cr "$Z" 2>/dev/null || true
ditto -c -k --norsrc "$Z" dist/Megatron-macos-browser.zip
rm -rf "$Z"
# The update carries the bridge and ui/ only: they live in Contents/Resources, while the launcher (Contents/MacOS)
# stays as it is, exactly like the Linux shell launcher, which is not in the tarball either.
U=dist/.mac-browser-update; rm -rf "$U"; mkdir -p "$U/megatron-browser"
cp "$A/Contents/Resources/bridge" "$U/megatron-browser/"
cp -r "$A/Contents/Resources/ui" "$U/megatron-browser/ui"
COPYFILE_DISABLE=1 tar -C "$U" -czf dist/Megatron-macos-browser.tar.gz megatron-browser
rm -rf "$U" "$STAGE"
ls -la dist/Megatron-macos-browser.zip dist/Megatron-macos-browser.tar.gz
