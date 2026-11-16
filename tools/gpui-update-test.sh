#!/usr/bin/env bash
# End-to-end test of the native app's updater against a private local relay (COMMONS_TEST_PORT_OFFSET 1400+, throwaway key).
#  1. makes a throwaway key and a scratch dist/ whose feed announces a version (what publish-update.sh would have written)
#  2. runs scripts/publish-gpui.sh (dry run first, then for real) with the throwaway signer into that scratch dist/
#  3. runs the unit tests plus the end-to-end test, which starts commons-relay on 127.0.0.1 and checks, downloads, verifies and installs
# Needs the relay binary (MEGA_TEST_RELAY_BIN, default target/debug/commons-relay) and the Rust toolchain on PATH. Nothing outside
# the work folder is touched, no window opens, no sound.
#   tools/gpui-update-test.sh [workdir]
set -euo pipefail
cd "$(dirname "$0")/.."
W=$(realpath -m "${1:-$HOME/data/agent-tmp/ds-mega/update-test}")
rm -rf "$W"; mkdir -p "$W"/{keys,dist/update}
export COMMONS_TEST_PORT_OFFSET=${COMMONS_TEST_PORT_OFFSET:-1400}
export MEGA_TEST_RELAY_BIN=${MEGA_TEST_RELAY_BIN:-$PWD/target/debug/commons-relay}
[ -x "$MEGA_TEST_RELAY_BIN" ] || { echo "no relay at $MEGA_TEST_RELAY_BIN" >&2; exit 1; }
EMBED=$(cargo run -q -p commons-core --example minisign-test-sign -- gen "$W/keys")
printf '%s\n' "$EMBED" > "$W/keys/pub.b64"
head -c 200000 /dev/urandom > "$W/mega-gpui"; printf '\x7fELF' | dd of="$W/mega-gpui" conv=notrunc 2>/dev/null
echo '{"version":"0.9.0","notes":"Test notes.","pub_date":"2026-01-01T00:00:00Z","platforms":{"linux-x86_64":{"url":"update/t.AppImage","signature":"c2ln"}}}' > "$W/dist/update/latest.json"
export COMMONS_UPDATE_DIST="$W/dist" COMMONS_UPDATE_PUBKEY_FILE="$W/keys/pub.b64" COMMONS_UPDATE_TEST_SIGNER="$W/keys"
scripts/publish-gpui.sh --version 0.9.0 --binary "$W/mega-gpui" --platform linux-gpui-x86_64 --dry-run
[ -z "$(ls "$W/dist/update" | grep -v latest.json)" ] || { echo "FAIL: the dry run wrote files" >&2; exit 1; }
if scripts/publish-gpui.sh --version 0.9.1 --binary "$W/mega-gpui" --platform linux-gpui-x86_64 2>/dev/null; then echo "FAIL: a version the feed does not announce was published" >&2; exit 1; fi
scripts/publish-gpui.sh --version 0.9.0 --binary "$W/mega-gpui" --platform linux-gpui-x86_64
jq -e '.platforms["linux-x86_64"].url == "update/t.AppImage" and (.platforms["linux-gpui-x86_64"].signature | length > 100) and .notes == "Test notes."' "$W/dist/update/latest.json" >/dev/null || { echo "FAIL: the feed after publishing" >&2; exit 1; }
echo "PASS publish-gpui.sh: dry run writes nothing, a mismatched version is refused, the entry is added beside the others"
cargo test -j4 -p mega-gpui --offline --bin mega-gpui update:: -- --nocapture
