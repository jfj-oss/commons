#!/usr/bin/env bash
# Builds an UNSIGNED Megatron .ipa on the Mac, over ssh, for SideStore to sign with a free Apple ID.
#
#   scripts/build-ios.sh              -> dist/Megatron-ios.ipa
#   scripts/build-ios.sh --fresh      regenerate the Xcode project (app/src-tauri/gen/apple) first
#
# The Mac needs the FULL Xcode (App Store, opened once so it finishes installing its iOS platform). The Xcode
# command line tools alone cannot build an iOS app. This script checks that before it copies anything, and says
# what is missing; it never installs Xcode and never touches an Apple account. SideStore does the signing.
#
# Kept light for the owner's 8 GB laptop: two cargo jobs, no LTO, background priority. COMMONS_MAC_FULL=1 uses
# every core and the normal release settings. COMMONS_MAC / COMMONS_MAC_KEY / COMMONS_MAC_DIR override the
# defaults below (the same ones scripts/build-mac.sh and scripts/publish-update.sh use).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
MAC=${COMMONS_MAC:-user@100.114.187.94}
MAC_KEY=${COMMONS_MAC_KEY:-$HOME/.ssh/macbook-air}
REMOTE=${COMMONS_MAC_DIR:-commons-build}
FRESH=
case "${1:-}" in
  --fresh) FRESH=1 ;;
  -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
  "") ;;
  *) echo "build-ios: unknown option $1 (try --help)" >&2; exit 2 ;;
esac
die() { printf 'build-ios: %s\n' "$*" >&2; exit 1; }
say() { printf '%s\n' "$*"; }
SSH=(ssh -i "$MAC_KEY" -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=8 -o StrictHostKeyChecking=accept-new)
SCP=(scp -q -i "$MAC_KEY" -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=8 -o StrictHostKeyChecking=accept-new)
[ -f "$MAC_KEY" ] || die "no ssh key at $MAC_KEY (set COMMONS_MAC_KEY)"

# ---- 1. Can the Mac build iOS at all? One quick ssh call, before any source is copied.
say "Checking the Mac ($MAC)"
CHECK=$("${SSH[@]}" "$MAC" 'echo "SSH ok"; echo "DEVDIR=$(xcode-select -p 2>/dev/null)"; echo "XCODEBUILD=$(xcodebuild -version 2>/dev/null | head -1)"; echo "SDK=$(xcrun --sdk iphoneos --show-sdk-path 2>/dev/null)"; echo "RUST=$(command -v cargo 2>/dev/null)"' 2>&1) \
  || die "the Mac does not answer over ssh. Is it on, awake and on the same Tailscale network? ($CHECK)"
case "$CHECK" in *"SSH ok"*) ;; *) die "ssh reached $MAC but the shell did not run: $CHECK" ;; esac
DEVDIR=$(printf '%s\n' "$CHECK" | sed -n 's/^DEVDIR=//p')
XCODEBUILD=$(printf '%s\n' "$CHECK" | sed -n 's/^XCODEBUILD=//p')
SDK=$(printf '%s\n' "$CHECK" | sed -n 's/^SDK=//p')
RUST=$(printf '%s\n' "$CHECK" | sed -n 's/^RUST=//p')
if [ -z "$DEVDIR" ] || [ "${DEVDIR%/Contents/Developer}" = "$DEVDIR" ] || [ -z "$XCODEBUILD" ] || [ -z "$SDK" ]; then
  cat >&2 <<EOF
build-ios: full Xcode is not installed (or not selected) on the Mac.

  selected developer directory: ${DEVDIR:-nothing}
  xcodebuild:                   ${XCODEBUILD:-not found}
  iPhone SDK:                   ${SDK:-not found}

What the Mac has now is the Xcode command line tools, which cannot build an iOS app.

To fix it, on the Mac:
  1. Install Xcode from the App Store (free; about 10 GB). It is slow on an 8 GB laptop, so let it finish.
  2. Open Xcode once, accept the licence, and let it install the iOS platform when asked
     (Settings > Platforms, or it prompts on first run).
  3. Point the tools at it:   sudo xcode-select -s /Applications/Xcode.app/Contents/Developer

This script does not install Xcode for you. Nothing was copied or built.
EOF
  exit 1
fi
[ -n "$RUST" ] || die "Rust (cargo) is not on the Mac's PATH. Install it from https://rustup.rs, or make ~/.cargo/bin visible to non-login shells."
say "Xcode: $XCODEBUILD ($DEVDIR)"

# ---- 2. Copy the source. Like publish-update.sh: no target/, dist/, data/, .git or module folders, and the
#         generated Xcode project is left in place on the Mac unless --fresh was asked for.
say "Copying the source to $MAC:$REMOTE"
RSYNC_EXCLUDES=(--exclude /target --exclude /target-docker --exclude /dist --exclude /data --exclude /.git
  --exclude /tools/node_modules --exclude /tools/bin --exclude /app/src-tauri/gen --exclude '*.log' --exclude /mac-watch.sh
  --exclude /node_modules)
rsync -a --delete -e "ssh -i $MAC_KEY -o BatchMode=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=8 -o StrictHostKeyChecking=accept-new" \
  "${RSYNC_EXCLUDES[@]}" ./ "$MAC:$REMOTE/" || die "could not copy the source to the Mac"

# ---- 3. Build there. The Rust iOS target and the Tauri CLI are installed only if missing.
say "Building on the Mac (this takes a while the first time)"
"${SSH[@]}" "$MAC" "FRESH='$FRESH' zsh -s" <<'MAC' || die "the iOS build failed on the Mac (see the messages above)"
set -eu
export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/commons-build" || { echo "no ~/commons-build on the Mac" >&2; exit 2; }

command -v cargo >/dev/null || { echo "cargo is not installed on the Mac (https://rustup.rs)" >&2; exit 3; }
if ! rustup target list --installed | grep -qx aarch64-apple-ios; then
  echo "Installing the Rust iOS target (aarch64-apple-ios)"
  rustup target add aarch64-apple-ios || exit 3
fi

# The Tauri CLI is needed to generate and drive the Xcode project. Prefer the copy in tools/ (npm), then a global one.
TAURI=()
if [ -x tools/node_modules/.bin/tauri ]; then TAURI=(tools/node_modules/.bin/tauri)
elif command -v tauri >/dev/null 2>&1; then TAURI=("$(command -v tauri)")
elif cargo tauri --version >/dev/null 2>&1; then TAURI=(cargo tauri)
elif command -v node >/dev/null 2>&1 && command -v npm >/dev/null 2>&1; then
  echo "Installing the Tauri CLI into tools/ (first run only)"
  ( cd tools && npm ci ) || exit 3
  TAURI=(tools/node_modules/.bin/tauri)
else
  echo "The Tauri CLI is missing: install Node.js from https://nodejs.org (then this script runs npm ci), or run 'cargo install tauri-cli --version ^2' on the Mac." >&2
  exit 3
fi

cd app/src-tauri
if [ ! -d gen/apple ] || [ -n "${FRESH:-}" ]; then
  echo "Generating the Xcode project (gen/apple)"
  rm -rf gen/apple
  "${TAURI[@]}" ios init || exit 4
fi

# Unsigned: SideStore signs the .ipa with the owner's Apple ID, so no certificate, team or provisioning profile is
# used here. xcodebuild honours these environment variables.
export CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="" EXPANDED_CODE_SIGN_IDENTITY=""
export CARGO_BUILD_JOBS=2 CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
if [ -n "${COMMONS_MAC_FULL:-}" ]; then unset CARGO_BUILD_JOBS CARGO_PROFILE_RELEASE_LTO CARGO_PROFILE_RELEASE_CODEGEN_UNITS; RUN=(); else RUN=(taskpolicy -b nice -n 19); fi
echo "Running: tauri ios build --no-sign --target aarch64"
"${RUN[@]}" "${TAURI[@]}" ios build --no-sign --target aarch64 > "$HOME/commons-build/ios-build.log" 2>&1 || {
  echo "--- last 40 lines of ios-build.log ---" >&2
  tail -40 "$HOME/commons-build/ios-build.log" >&2
  exit 4
}

# Tauri's Xcode project names and locations have changed between versions; find the device .app rather than guess.
APP=$(find gen/apple target/aarch64-apple-ios -type d -name '*.app' 2>/dev/null | grep -v -e 'iphonesimulator' -e '/sim/' | head -n 1 || true)
[ -n "$APP" ] || { echo "The build finished but no device .app was found under app/src-tauri/gen/apple or target/aarch64-apple-ios. See ios-build.log." >&2; exit 5; }
echo "Packing $(basename "$APP") from $APP"

PAYLOAD="$HOME/commons-build/.ios-payload"
rm -rf "$PAYLOAD" && mkdir -p "$PAYLOAD/Payload"
cp -R "$APP" "$PAYLOAD/Payload/Megatron.app"
rm -rf "$PAYLOAD/Payload/Megatron.app/_CodeSignature" "$PAYLOAD/Payload/Megatron.app/embedded.mobileprovision"
mkdir -p "$HOME/commons-build/dist"
rm -f "$HOME/commons-build/dist/Megatron-ios.ipa"
( cd "$PAYLOAD" && zip -qry "$HOME/commons-build/dist/Megatron-ios.ipa" Payload ) || exit 5
rm -rf "$PAYLOAD"
unzip -l "$HOME/commons-build/dist/Megatron-ios.ipa" | grep -q 'Payload/Megatron.app/Info.plist' \
  || { echo "The .ipa has no Payload/Megatron.app/Info.plist: packing went wrong." >&2; exit 5; }
ls -la "$HOME/commons-build/dist/Megatron-ios.ipa"
MAC

# ---- 4. Bring it back.
say "Copying the .ipa back to dist/"
mkdir -p dist
rm -f dist/Megatron-ios.ipa
"${SCP[@]}" "$MAC:$REMOTE/dist/Megatron-ios.ipa" dist/Megatron-ios.ipa || die "could not fetch the .ipa from the Mac"
[ -s dist/Megatron-ios.ipa ] || die "the .ipa came back empty"
say "Done: dist/Megatron-ios.ipa ($(du -h dist/Megatron-ios.ipa | cut -f1)). Install it with SideStore: docs/IOS-SIDESTORE.md."
