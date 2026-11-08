#!/usr/bin/env bash
# Cross-builds the Windows "browser version" from Linux in Docker, with the same toolchain as scripts/build-windows.sh:
# the core as a small local server (`bridge.exe`), the UI, and `launcher.exe` (no console window). Writes
#   dist/Megatron-windows-browser.zip      what a friend downloads: unpack, run install.cmd
#   dist/Megatron-windows-browser.tar.gz   what the signed self-update installs (bridge.exe, launcher.exe, ui/)
# The version and updater public key are compiled into the bridge and launcher, exactly as in scripts/build-linux-browser.sh.
# COMMONS_OUT changes the output folder; COMMONS_TAURI_CONFIG is merged into the version/key lookup (publish-update.sh).
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$(realpath -m "${COMMONS_OUT:-dist}")
CONF=app/src-tauri/tauri.conf.json
VER=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$CONF" | head -1)
PUB=$(sed -n 's/^ *"pubkey": *"\([^"]*\)".*/\1/p' "$CONF" | head -1)
if [ -n "${COMMONS_TAURI_CONFIG:-}" ]; then
  command -v jq >/dev/null || { echo "build-windows-browser: jq is needed with COMMONS_TAURI_CONFIG" >&2; exit 1; }
  v=$(jq -r '.version // empty' <<<"$COMMONS_TAURI_CONFIG"); [ -z "$v" ] || VER=$v
  p=$(jq -r '.plugins.updater.pubkey // empty' <<<"$COMMONS_TAURI_CONFIG"); [ -z "$p" ] || PUB=$p
fi
[ -n "$VER" ] && [ "$VER" != null ] || { echo "build-windows-browser: no version" >&2; exit 1; }
[ -n "$PUB" ] && [ "$PUB" != null ] || { echo "build-windows-browser: no updater public key in $CONF" >&2; exit 1; }
mkdir -p "$OUT" target-docker
docker run --rm --memory=5g --cpus="${COMMONS_BUILD_JOBS:-4}" -v "$PWD":/src -v "$OUT":/out -w /src \
  -e CARGO_TARGET_DIR=/src/target-docker/wb -e CARGO_HOME=/src/target-docker/cargo -e RUSTUP_HOME=/src/target-docker/rustup \
  -e CARGO_BUILD_JOBS="${COMMONS_BUILD_JOBS:-4}" -e MEGATRON_BROWSER_VERSION="$VER" -e MEGATRON_BROWSER_PUBKEY="$PUB" \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" ubuntu:24.04 bash -euc '
  export DEBIAN_FRONTEND=noninteractive PATH=/src/target-docker/cargo/bin:$PATH
  apt-get update -qq && apt-get install -y -qq curl ca-certificates build-essential pkg-config mingw-w64 zip >/dev/null
  command -v cargo >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
  rustup target add x86_64-pc-windows-gnu
  cargo build -q --release --target x86_64-pc-windows-gnu -p commons-core --features devbridge --bin dev_bridge --bin megatron_launcher
  R=/src/target-docker/wb/x86_64-pc-windows-gnu/release
  # Keep the package to system DLLs only: refusal here means the build picked up a DLL friends would not have.
  if x86_64-w64-mingw32-objdump -p "$R/dev_bridge.exe" "$R/megatron_launcher.exe" | grep -i "DLL Name" | grep -viE "api-ms-win-|KERNEL32|kernelbase|ntdll|msvcrt|bcrypt|bcryptprimitives|ws2_32|advapi32|user32|secur32|iphlpapi|shell32|ole32|oleaut32|psapi|dbghelp|shlwapi|credui|wintrust|userenv|version|crypt32|ncrypt|combase|rpcrt4" ; then
    echo "build-windows-browser: unexpected DLL dependencies (see above)" >&2; exit 1
  fi
  for f in dev_bridge megatron_launcher; do x86_64-w64-mingw32-strip "$R/$f.exe"; done
  S=$(mktemp -d); P=$S/megatron-browser; U=$S/upd/megatron-browser; mkdir -p "$P" "$U"
  cp "$R/dev_bridge.exe" "$P/bridge.exe"; cp "$R/megatron_launcher.exe" "$P/launcher.exe"
  cp -r /src/app/ui "$P/ui"
  cp /src/packaging/windows-browser/README.txt /src/packaging/windows-browser/install.cmd "$P/"
  cp /src/app/src-tauri/default-relay.txt "$P/relay.txt"; cp /src/app/src-tauri/icons/icon.ico "$P/megatron.ico"
  rm -f /out/Megatron-windows-browser.zip /out/Megatron-windows-browser.tar.gz
  (cd "$S" && zip -qr /out/Megatron-windows-browser.zip megatron-browser)
  cp "$P/launcher.exe" "$P/bridge.exe" "$U/"; cp -r "$P/ui" "$U/ui"
  tar -C "$S/upd" -czf /out/Megatron-windows-browser.tar.gz megatron-browser
  chown -R "$HOST_UID:$HOST_GID" /out /src/target-docker
'
ls -la "$OUT/Megatron-windows-browser.zip" "$OUT/Megatron-windows-browser.tar.gz"
