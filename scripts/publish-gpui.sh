#!/usr/bin/env bash
# Adds the NATIVE (GPUI) app to the update feed the relay serves: signs the build with the updater key, copies it into
# dist/update/, and adds this system's entry (linux-gpui-x86_64, darwin-gpui-aarch64) to update/latest.json. The Tauri
# apps and the browser versions ignore these entries; existing entries are never touched.
#
#   scripts/publish-gpui.sh --version X.Y.Z [--binary PATH | --app PATH/Megatron.app | --archive FILE.tar.gz]
#                           [--platform KEY] [--notes "text"] [--dry-run]
#
#   --version       the version this build is (must be the version latest.json already announces: run
#                   scripts/publish-update.sh first, then this adds the native build to that same version). The build is
#                   made with MEGA_GPUI_VERSION=<it>, which is what the app compares against the feed.
#   --binary        Linux: the built mega-gpui executable (a release build, ideally made on the oldest system you
#                   support, as the AppImage is: docs/UPDATES.md)
#   --app           macOS: the built Megatron.app (archived here, ad-hoc signed; the app seals itself again on install)
#   --archive       macOS: an already made .app.tar.gz (one top-level Megatron.app)
#   --platform      the feed key; default from this machine (linux-gpui-x86_64, darwin-gpui-aarch64)
#   --notes         replaces the release notes in the feed (default: keep what publish-update.sh wrote)
#   --dry-run       checks everything and prints what it would do; signs nothing, copies nothing, reads no key
#
# The private key stays on this PC (~/.config/commons/updater.key and .password, mode 600), exactly as for
# publish-update.sh. For tests only: COMMONS_UPDATE_DIST picks another output folder, COMMONS_UPDATE_KEY another key,
# COMMONS_UPDATE_PUBKEY_FILE another trusted public key, COMMONS_UPDATE_TEST_SIGNER=<dir> signs with the throwaway
# signer (crates/core/examples/minisign-test-sign.rs) and the key pair in <dir> instead of the Tauri CLI.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
die() { printf 'publish-gpui: %s\n' "$*" >&2; exit 1; }
say() { printf '%s\n' "$*"; }

VERSION='' BIN='' APP='' ARCHIVE='' PLAT='' NOTES='' NOTES_SET=0 DRY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=$2; shift 2 ;;
    --binary) BIN=$2; shift 2 ;;
    --app) APP=$2; shift 2 ;;
    --archive) ARCHIVE=$2; shift 2 ;;
    --platform) PLAT=$2; shift 2 ;;
    --notes) NOTES=$2; NOTES_SET=1; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) die "unknown option $1" ;;
  esac
done
is_ver() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; }
[ -n "$VERSION" ] && is_ver "$VERSION" || die "--version X.Y.Z is needed"
command -v jq >/dev/null || die "jq is needed"

if [ -z "$PLAT" ]; then
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) PLAT=linux-gpui-x86_64 ;;
    Darwin-arm64) PLAT=darwin-gpui-aarch64 ;;
    Darwin-x86_64) PLAT=darwin-gpui-x86_64 ;;
    *) die "no native build for $(uname -s)-$(uname -m); pass --platform" ;;
  esac
fi
case "$PLAT" in linux-gpui-x86_64|linux-gpui-aarch64|darwin-gpui-aarch64|darwin-gpui-x86_64) ;; *) die "\"$PLAT\" is not a native platform key" ;; esac

DIST=$(realpath -m "${COMMONS_UPDATE_DIST:-dist}")
FEED=$DIST/update/latest.json
KEY=${COMMONS_UPDATE_KEY:-$HOME/.config/commons/updater.key}
TEST_SIGNER=${COMMONS_UPDATE_TEST_SIGNER:-}
CONF=app/src-tauri/tauri.conf.json
CLI=$ROOT/tools/node_modules/.bin/tauri

# ---- the input, by system
case "$PLAT" in
  linux-*)
    [ -n "$BIN" ] || die "Linux: --binary PATH (the built mega-gpui) is needed"
    [ -f "$BIN" ] && [ "$(head -c4 "$BIN" | od -An -tx1 | tr -d ' \n')" = 7f454c46 ] || die "$BIN is not a Linux executable"
    SRC=$BIN; NAME="Megatron_${VERSION}_gpui-${PLAT#linux-gpui-}" ;;
  darwin-*)
    if [ -n "$ARCHIVE" ]; then [ -f "$ARCHIVE" ] || die "no archive at $ARCHIVE"; SRC=$ARCHIVE
    else [ -d "$APP" ] || die "macOS: --app PATH/Megatron.app or --archive FILE is needed"; SRC=$APP; fi
    NAME="Megatron_${VERSION}_gpui-${PLAT#darwin-gpui-}.app.tar.gz" ;;
esac

# ---- the feed: this version must already be what it announces (adding to it, never replacing the Tauri entries)
[ -s "$FEED" ] || die "no $FEED: publish the version with scripts/publish-update.sh first"
FEEDV=$(jq -r .version "$FEED")
[ "$FEEDV" = "$VERSION" ] || die "the feed announces $FEEDV, not $VERSION. Run scripts/publish-update.sh for $VERSION first; this only adds the native build to the version that is published (a feed with only a native entry would take the other apps' updates away)."
TRUSTED=''
if [ -n "${COMMONS_UPDATE_PUBKEY_FILE:-}" ]; then TRUSTED=$(tr -d '\n' < "$COMMONS_UPDATE_PUBKEY_FILE"); else TRUSTED=$(jq -r .plugins.updater.pubkey "$CONF"); fi
keyid_pub() { base64 -d <<<"$1" | sed -n 2p | base64 -d | head -c 10 | tail -c 8 | od -An -tx1 | tr -d ' \n'; }
keyid_sig() { base64 -d < "$1" | sed -n 2p | base64 -d | head -c 10 | tail -c 8 | od -An -tx1 | tr -d ' \n'; }
WANT_ID=$(keyid_pub "$TRUSTED"); [ ${#WANT_ID} = 16 ] || die "could not read the public key the apps are built with"

say "Native build for $PLAT, version $VERSION"
say "  input:   $SRC"
say "  file:    $DIST/update/$NAME"
say "  feed:    $FEED (version $FEEDV; entry $PLAT $( [ "$(jq -r --arg p "$PLAT" '.platforms[$p] // empty' "$FEED")" ] && echo replaced || echo added ))"
say "  notes:   $( [ $NOTES_SET = 1 ] && echo 'replaced by --notes' || echo 'kept from the feed' )"
say "  key:     $( [ -n "$TEST_SIGNER" ] && echo "throwaway test signer in $TEST_SIGNER" || echo "$KEY (not read in a dry run)" )"
if [ $DRY = 1 ]; then say "Dry run: nothing was signed, copied or changed."; exit 0; fi

# ---- the key (as publish-update.sh checks it)
if [ -z "$TEST_SIGNER" ]; then
  [ -s "$KEY" ] || die "no updater key at $KEY. Restore it from your backup (docs/UPDATES.md)."
  [ -e "$KEY.password" ] || die "no key password file at $KEY.password"
  for f in "$KEY" "$KEY.password"; do [ "$(stat -c %a "$f")" = 600 ] || die "$f must be mode 600"; done
  [ -x "$CLI" ] || (cd tools && npm ci)
fi

WORK=$(mktemp -d "$DIST/.gpui-publish.XXXXXX"); trap 'rm -rf "$WORK"' EXIT
case "$PLAT" in
  linux-*) cp --reflink=auto "$SRC" "$WORK/$NAME"; chmod 755 "$WORK/$NAME" ;;
  darwin-*)
    if [ -n "$ARCHIVE" ]; then cp "$ARCHIVE" "$WORK/$NAME"
    else (cd "$(dirname "${SRC%/}")" && COPYFILE_DISABLE=1 tar -czf "$WORK/$NAME" "$(basename "${SRC%/}")"); fi ;;
esac

# ---- sign: the password goes through the environment, never the command line
if [ -n "$TEST_SIGNER" ]; then
  SIG=$(cargo run -q -p commons-core --example minisign-test-sign -- sign "$TEST_SIGNER" "$WORK/$NAME" "$VERSION")
  [ -n "$SIG" ] || die "the test signer gave nothing"
else
  TAURI_SIGNING_PRIVATE_KEY_PATH=$KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$KEY.password")" "$CLI" signer sign --app-version "$VERSION" "$WORK/$NAME" > "$WORK/sign.log" 2>&1 \
    || die "signing failed: $WORK/sign.log"
  SIG=$(tr -d '\n' < "$WORK/$NAME.sig")
fi
printf '%s' "$SIG" > "$WORK/$NAME.sig.b64"
[ "$(keyid_sig "$WORK/$NAME.sig.b64")" = "$WANT_ID" ] || die "the build was signed with a key the apps do not trust (they trust key id $WANT_ID)"

# ---- publish: the file first, the feed last (the relay reads dist/ as it goes)
mkdir -p "$DIST/update"
cp --reflink=auto "$WORK/$NAME" "$DIST/update/.$NAME.tmp" && mv "$DIST/update/.$NAME.tmp" "$DIST/update/$NAME"
jq --arg p "$PLAT" --arg u "update/$NAME" --rawfile s "$WORK/$NAME.sig.b64" --arg n "$NOTES" --argjson set "$NOTES_SET" \
  '.platforms[$p] = {url: $u, signature: $s} | if $set == 1 then .notes = $n else . end' "$FEED" > "$DIST/update/.latest.json.tmp"
mv "$DIST/update/.latest.json.tmp" "$FEED"
say "Published $PLAT $VERSION: $(jq -r '.platforms | keys | join(", ")' "$FEED")"
