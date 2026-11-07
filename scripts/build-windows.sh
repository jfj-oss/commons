#!/usr/bin/env bash
# Cross-builds the Windows app from Linux in Docker: dist/Commons-windows-x64.zip (Commons.exe and WebView2Loader.dll side by side).
# No installer; unzip and run. Windows 10 and 11 normally have the WebView2 runtime already. Unsigned. `--test` also builds the
# game key tests as a Windows program, for Wine. A real installer comes from .github/workflows/apps.yml.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p dist target-docker
docker run --rm --memory=5g --cpus="${COMMONS_BUILD_JOBS:-3}" -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target-docker/w -e CARGO_HOME=/src/target-docker/cargo -e RUSTUP_HOME=/src/target-docker/rustup \
  -e CARGO_BUILD_JOBS="${COMMONS_BUILD_JOBS:-3}" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -e WANT_TEST="${1:-}" ubuntu:22.04 bash -euc '
  export DEBIAN_FRONTEND=noninteractive PATH=/src/target-docker/cargo/bin:$PATH
  apt-get update -qq
  apt-get install -y -qq curl ca-certificates build-essential pkg-config mingw-w64 cmake nasm zip >/dev/null
  command -v cargo >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
  rustup target add x86_64-pc-windows-gnu
  cargo build --release --target x86_64-pc-windows-gnu -p commons-app --features tauri/custom-protocol
  [ -z "$WANT_TEST" ] || cargo test --no-run --target x86_64-pc-windows-gnu -p commons-gamekeys --lib
  R=/src/target-docker/w/x86_64-pc-windows-gnu/release
  mkdir -p /tmp/z && cp $R/commons-app.exe /tmp/z/Commons.exe && cp $R/WebView2Loader.dll /tmp/z/
  rm -f /src/dist/Commons-windows-x64.zip
  (cd /tmp/z && zip -q /src/dist/Commons-windows-x64.zip Commons.exe WebView2Loader.dll)
  chown -R "$HOST_UID:$HOST_GID" /src/dist /src/target-docker
'
ls -la dist
