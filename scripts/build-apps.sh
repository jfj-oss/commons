#!/usr/bin/env bash
# Builds the Linux AppImage and the relay, and puts what friends download into dist/ (git-ignored).
# macOS and Windows builds are made on those systems: docs/DEPLOY.md, .github/workflows/apps.yml. Copy them into dist/ too.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_BUILD_JOBS=${COMMONS_BUILD_JOBS:-4}
[ -x tools/node_modules/.bin/tauri ] || (cd tools && npm ci)
# linuxdeploy's strip cannot read current Arch libraries; leaving them unstripped only costs size.
(cd app/src-tauri && NO_STRIP=1 ../../tools/node_modules/.bin/tauri build --bundles appimage)
cargo build --release -j "$CARGO_BUILD_JOBS" -p commons-relay
mkdir -p dist tools/bin
cp target/release/bundle/appimage/Commons_*_amd64.AppImage dist/Commons-linux-x86_64.AppImage
cp target/release/commons-relay tools/bin/commons-relay
ls -la dist
