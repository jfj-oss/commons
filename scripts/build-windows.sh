#!/usr/bin/env bash
# Cross-builds the Windows app from Linux in Docker:
#   dist/Commons-windows-x64-setup.exe  the installer friends download (per user, no administrator rights). The updater
#                                       installs new versions with it, so this is the one that gets updates.
#   dist/Commons-windows-x64.zip        Commons.exe and WebView2Loader.dll side by side, to run without installing (no
#                                       updates). Only with COMMONS_WIN_ZIP=1 or --test.
# Ubuntu 24.04 for its NSIS 3.09 (22.04's lacks RestartManager.nsh, which the installer needs). Windows 10 and 11 normally have the WebView2 runtime already. Not Authenticode-signed. `--test` also builds the game key
# tests as a Windows program, for Wine. COMMONS_OUT changes the output folder; COMMONS_TAURI_CONFIG is merged into
# tauri.conf.json (scripts/publish-update.sh uses both).
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$(realpath -m "${COMMONS_OUT:-dist}")
mkdir -p "$OUT" target-docker
docker run --rm --memory=5g --cpus="${COMMONS_BUILD_JOBS:-3}" -v "$PWD":/src -v "$OUT":/out -w /src -e CARGO_TARGET_DIR=/src/target-docker/w -e CARGO_HOME=/src/target-docker/cargo -e RUSTUP_HOME=/src/target-docker/rustup \
  -e CARGO_BUILD_JOBS="${COMMONS_BUILD_JOBS:-3}" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -e WANT_TEST="${1:-}" -e WANT_ZIP="${COMMONS_WIN_ZIP:-}" -e COMMONS_TAURI_CONFIG="${COMMONS_TAURI_CONFIG:-}" ubuntu:24.04 bash -euc '
  export DEBIAN_FRONTEND=noninteractive PATH=/src/target-docker/cargo/bin:$PATH
  apt-get update -qq
  apt-get install -y -qq curl ca-certificates build-essential pkg-config mingw-w64 cmake nasm zip nsis xz-utils >/dev/null
  command -v cargo >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
  rustup target add x86_64-pc-windows-gnu
  curl -fsSL https://nodejs.org/dist/v22.11.0/node-v22.11.0-linux-x64.tar.xz | tar -xJ --strip-components=1 -C /usr/local
  mkdir -p /tmp/tools && cp /src/tools/package.json /src/tools/package-lock.json /tmp/tools/
  (cd /tmp/tools && npm ci >/dev/null)
  R=/src/target-docker/w/x86_64-pc-windows-gnu/release
  rm -f $R/bundle/nsis/*.exe
  cfg=(); [ -z "$COMMONS_TAURI_CONFIG" ] || cfg=(--config "$COMMONS_TAURI_CONFIG")
  (cd /src/app/src-tauri && /tmp/tools/node_modules/.bin/tauri build --target x86_64-pc-windows-gnu --bundles nsis "${cfg[@]}")
  set -- $R/bundle/nsis/*-setup.exe; [ $# = 1 ] || { echo "expected one installer, found: $*" >&2; exit 1; }
  cp "$1" /out/Commons-windows-x64-setup.exe
  [ -z "$WANT_TEST" ] || cargo test --no-run --target x86_64-pc-windows-gnu -p commons-gamekeys --lib
  if [ -n "$WANT_TEST$WANT_ZIP" ]; then
    rm -rf /tmp/z; mkdir -p /tmp/z && cp $R/commons-app.exe /tmp/z/Commons.exe && cp $R/WebView2Loader.dll /tmp/z/
    rm -f /out/Commons-windows-x64.zip
    (cd /tmp/z && zip -q /out/Commons-windows-x64.zip Commons.exe WebView2Loader.dll)
  fi
  chown -R "$HOST_UID:$HOST_GID" /out /src/target-docker /src/app/src-tauri/gen
'
ls -la "$OUT"
