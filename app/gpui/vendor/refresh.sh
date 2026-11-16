#!/bin/bash
# Rebuilds app/gpui/vendor/<crate> from the crates.io release named below, checks its sha256 against the one crates.io
# publishes (the same value Cargo.lock held before the [patch]), then applies patches/<crate>-*.patch in order.
#   vendor/refresh.sh          rebuild every crate
#   vendor/refresh.sh --check  rebuild into a scratch folder and fail if the committed tree differs
# The prebuilt libwebrtc that webrtc-sys downloads at build time is not touched; only binding code is patched.
# To upgrade: change a version and sha256 here, run it, fix any patch that no longer applies, then re-run
# tools/gpui-call-interop.mjs (CALLS-PLAN.md step 0). If that fails, do not upgrade.
set -euo pipefail
cd "$(dirname "$0")"
CRATES=(
  "livekit 0.11.0 353924a7238e999d49472ee4dce036eb24c42626e7954be39287610f057f4b52"
  "libwebrtc 0.4.1 814e93897c8689013dac8e4babc08957fdc04823748903f4be2409d5d3b8d9a2"
  "webrtc-sys 0.3.54 7b35efe157521adc1ccc875e0687d5434c422758f44b1306be961447d7e5660c"
)
CHECK=0; [ "${1:-}" = "--check" ] && CHECK=1
OUT=.; [ $CHECK = 1 ] && OUT=$(mktemp -d "$PWD/.check.XXXXXX")
DL=${VENDOR_DOWNLOADS:-$HOME/.cache/mega-gpui-vendor}; mkdir -p "$DL"
for line in "${CRATES[@]}"; do
  read -r name ver sum <<<"$line"
  f="$DL/$name-$ver.crate"
  if [ ! -s "$f" ]; then
    # a copy cargo already downloaded is as good as a fresh one: the checksum below decides
    c=$(ls ${CARGO_HOME:-$HOME/.cargo}/registry/cache/*/"$name-$ver.crate" 2>/dev/null | head -1 || true)
    if [ -n "$c" ]; then cp "$c" "$f"; else curl -fsSL -o "$f" "https://static.crates.io/crates/$name/$name-$ver.crate"; fi
  fi
  echo "$sum  $f" | sha256sum -c --quiet - || { echo "checksum mismatch for $name $ver" >&2; exit 1; }
  rm -rf "$OUT/$name" "$OUT/$name-$ver"
  tar -xzf "$f" -C "$OUT"
  mv "$OUT/$name-$ver" "$OUT/$name"
  rm -f "$OUT/$name/.cargo_vcs_info.json" "$OUT/$name/Cargo.toml.orig"
  for p in patches/"$name"-*.patch; do
    [ -e "$p" ] || continue
    patch -s -p1 -d "$OUT/$name" < "$p"
  done
done
if [ $CHECK = 1 ]; then
  rc=0
  for line in "${CRATES[@]}"; do read -r name _ <<<"$line"; diff -r "$OUT/$name" "$name" > /dev/null || { echo "vendor/$name differs from crates.io + patches" >&2; rc=1; }; done
  rm -rf "$OUT"; [ $rc = 0 ] && echo "vendor matches crates.io + patches"; exit $rc
fi
echo "vendor rebuilt"
