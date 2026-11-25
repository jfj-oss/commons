#!/usr/bin/env bash
# Puts the native app's published version on GitHub as well, so friends' apps can update while the relay is off:
# a release vX.Y.Z of jfj-oss/commons with the native update files, friends' downloads, and mega-native.json (the
# relay's manifest for the native platforms, file addresses on GitHub). The app reads
# https://github.com/jfj-oss/commons/releases/latest/download/mega-native.json (app/gpui/src/update.rs, GITHUB_FEED),
# so the newest release must always be a native one: other apps must not make releases in this repository.
# publish-update.sh runs this at its end; run it by hand to retry, or after a --same-version publish (it replaces
# the release's files).
#
#   scripts/publish-github.sh 0.3.14           (reads dist/update/latest.json; COMMONS_UPDATE_DIST picks another dist)
set -euo pipefail
cd "$(dirname "$0")/.."
die() { printf 'publish-github: %s\n' "$*" >&2; exit 1; }
REPO=${COMMONS_GITHUB_REPO:-jfj-oss/commons}
VER=${1:?usage: scripts/publish-github.sh VERSION}
DIST=$(realpath -m "${COMMONS_UPDATE_DIST:-dist}")
M=$DIST/update/latest.json
command -v gh >/dev/null || die "gh (GitHub CLI) is needed"
[ -s "$M" ] || die "no manifest at $M"
[ "$(jq -r .version "$M")" = "$VER" ] || die "$M is version $(jq -r .version "$M"), not $VER"
TAG=v$VER BASE="https://github.com/$REPO/releases/download/v$VER"
OUT=$(mktemp -d); trap 'rm -rf "$OUT"' EXIT
declare -A NAME=([linux-gpui-x86_64]=mega-gpui-linux-x86_64 [darwin-gpui-aarch64]=mega-gpui-macos-arm64.app.tar.gz)
files=() platforms='{}'
for p in linux-gpui-x86_64 darwin-gpui-aarch64; do
  u=$(jq -r --arg p "$p" '.platforms[$p].url // empty' "$M"); [ -n "$u" ] || continue
  src=$DIST/$u; [ -s "$src" ] || die "missing $src"
  cp --reflink=auto "$src" "$OUT/${NAME[$p]}"; files+=("$OUT/${NAME[$p]}")
  platforms=$(jq -c --arg p "$p" --arg u "$BASE/${NAME[$p]}" --arg s "$(jq -r --arg p "$p" '.platforms[$p].signature' "$M")" '. + {($p): {url: $u, signature: $s}}' <<<"$platforms")
done
[ "$platforms" != '{}' ] || die "version $VER has no native build in $M"
for f in mega-linux-x86_64.tar.gz mega-macos-arm64.app.tar.gz; do [ -s "$DIST/$f" ] && files+=("$DIST/$f"); done
jq --argjson p "$platforms" '{version, notes, pub_date, platforms: $p} + (if .minimum_version then {minimum_version} else {} end)' "$M" > "$OUT/mega-native.json"
files+=("$OUT/mega-native.json")
NOTES=$(jq -r '.notes // ""' "$M")
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  gh release upload "$TAG" --repo "$REPO" --clobber "${files[@]}"
else
  # the tag goes on the commit this was built from, which must be pushed first
  gh release create "$TAG" --repo "$REPO" --target "$(git rev-parse HEAD)" --title "mega $VER" --notes "${NOTES:-mega $VER}" --latest "${files[@]}"
fi
echo "GitHub release $TAG: $(printf '%s ' "${files[@]##*/}")"
