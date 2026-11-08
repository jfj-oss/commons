#!/usr/bin/env bash
# Publishes a new version of the app, in one command: bumps the version, builds Linux (Docker, Ubuntu 22.04), Windows
# (Docker cross-build) and macOS (on the Mac, over ssh), signs the update files with the updater key, writes the update
# manifest, and puts everything in dist/, which the relay serves: friends' downloads at /download/, and the apps'
# updates at /update/ (docs/UPDATES.md). Nothing is restarted: the relay reads dist/ as it goes.
#
#   scripts/publish-update.sh                  next patch version (0.2.0 -> 0.2.1)
#   scripts/publish-update.sh minor            0.2.1 -> 0.3.0;  also: major, or an exact version such as 0.4.2
#   options:  --notes "text"                   shown to friends in the update prompt
#             --min-version X.Y.Z              apps older than this are told they must update (the relay's
#                                              --min-app-version, if set, wins)
#             --only linux,windows,mac         build only these (the others get no update this time)
#             --skip-mac                       go on without the Mac when it is off, away or low on battery
#             --same-version                   no bump: add systems to the version already published, e.g. the Mac
#                                              after it was skipped:  scripts/publish-update.sh --same-version --only mac
#
# The private key stays on this PC: ~/.config/commons/updater.key, its password in updater.key.password next to it
# (both mode 600). Builds are not touched by it; only the finished files are signed, here. Back the key up
# (docs/UPDATES.md): without it, nobody's app will ever accept another update.
#
# For tests only: COMMONS_UPDATE_SCRATCH=1 leaves the repository's version alone (the version goes in through
# COMMONS_TAURI_CONFIG), COMMONS_UPDATE_DIST picks another output folder, COMMONS_UPDATE_KEY another key, and
# COMMONS_UPDATE_PUBKEY_FILE builds the apps with another public key.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
die() { printf 'publish-update: %s\n' "$*" >&2; exit 1; }
say() { printf '%s\n' "$*"; }

WHAT=patch NOTES='' MIN='' ONLY=linux,windows,mac SKIP_MAC=0 SAME=0
while [ $# -gt 0 ]; do
  case "$1" in
    --notes) NOTES=$2; shift 2 ;;
    --min-version) MIN=$2; shift 2 ;;
    --only) ONLY=$2; shift 2 ;;
    --skip-mac) SKIP_MAC=1; shift ;;
    --same-version) SAME=1; shift ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    -*) die "unknown option $1" ;;
    *) WHAT=$1; shift ;;
  esac
done
want() { [[ ",$ONLY," == *",$1,"* ]]; }
is_ver() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; }

KEY=${COMMONS_UPDATE_KEY:-$HOME/.config/commons/updater.key}
DIST=$(realpath -m "${COMMONS_UPDATE_DIST:-dist}")
SCRATCH=${COMMONS_UPDATE_SCRATCH:-}
CLI=$ROOT/tools/node_modules/.bin/tauri
CONF=app/src-tauri/tauri.conf.json
MAC=${COMMONS_MAC:-user@100.114.187.94}
MAC_KEY=${COMMONS_MAC_KEY:-$HOME/.ssh/macbook-air}

# ---- checks first, so nothing is half done when one fails
[ -s "$KEY" ] || die "no updater key at $KEY. Restore it from your backup (docs/UPDATES.md); a new key would not be accepted by apps already out there."
[ -e "$KEY.password" ] || die "no key password file at $KEY.password"
for f in "$KEY" "$KEY.password"; do [ "$(stat -c %a "$f")" = 600 ] || die "$f must be mode 600 (chmod 600 $f)"; done
[ -x "$CLI" ] || (cd tools && npm ci)
command -v jq >/dev/null || die "jq is needed"
if want linux || want windows; then command -v docker >/dev/null || die "Docker is needed for the Linux and Windows builds"; fi
[ -z "$MIN" ] || is_ver "$MIN" || die "--min-version wants a version like 0.2.0"

CUR=$(jq -r .version "$CONF")
bump() { IFS=. read -r a b c <<<"${CUR%%-*}"; case $1 in major) echo "$((a+1)).0.0" ;; minor) echo "$a.$((b+1)).0" ;; patch) echo "$a.$b.$((c+1))" ;; esac; }
case "$WHAT" in major|minor|patch) NEW=$(bump "$WHAT") ;; *) NEW=$WHAT ;; esac
[ $SAME = 0 ] || NEW=$CUR
is_ver "$NEW" || die "\"$NEW\" is not a version"
if [ -z "$SCRATCH" ] && [ $SAME = 0 ]; then
  [ "$CUR" != "$NEW" ] && [ "$(printf '%s\n%s\n' "$CUR" "$NEW" | sort -V | tail -1)" = "$NEW" ] || die "$NEW is not newer than $CUR"
fi

# The signature's key id has to be the one the apps trust (minisign: 2 bytes algorithm, then the 8-byte key id).
PUBFILE=${COMMONS_UPDATE_PUBKEY_FILE:-}
if [ -n "$PUBFILE" ]; then TRUSTED=$(tr -d '\n' < "$PUBFILE"); else TRUSTED=$(jq -r .plugins.updater.pubkey "$CONF"); fi
keyid_pub() { base64 -d <<<"$1" | sed -n 2p | base64 -d | head -c 10 | tail -c 8 | od -An -tx1 | tr -d ' \n'; }
keyid_sig() { base64 -d < "$1" | sed -n 2p | base64 -d | head -c 10 | tail -c 8 | od -An -tx1 | tr -d ' \n'; }
WANT_ID=$(keyid_pub "$TRUSTED")
[ ${#WANT_ID} = 16 ] || die "could not read the public key the apps are built with"

mac_ok() {
  local b pct
  b=$(ssh -i "$MAC_KEY" -o BatchMode=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=8 -o ConnectTimeout=10 "$MAC" 'pmset -g batt' 2>/dev/null) || { echo "the Mac ($MAC) does not answer"; return 1; }
  grep -q "AC Power" <<<"$b" && return 0
  pct=$(grep -o '[0-9]*%' <<<"$b" | head -1 | tr -d %)
  [ "${pct:-0}" -gt 40 ] && return 0
  echo "the Mac is on battery at ${pct:-?}%"; return 1
}
if want mac; then
  if ! why=$(mac_ok); then
    [ $SKIP_MAC = 1 ] || die "$why. Plug it in, or pass --skip-mac (Mac friends then get this version later)."
    say "Skipping the Mac: $why."; ONLY=${ONLY//mac/}
  fi
fi

# ---- version
CFG=$(jq -cn --arg v "$NEW" '{version: $v}')
if [ -n "$PUBFILE" ]; then CFG=$(jq -c --arg k "$TRUSTED" '. + {plugins: {updater: {pubkey: $k}}}' <<<"$CFG"); fi
if [ -z "$SCRATCH" ] && [ $SAME = 1 ]; then
  CFG=''; say "Adding to version $NEW"
elif [ -z "$SCRATCH" ]; then
  tmp=$(mktemp "$CONF.XXXX"); jq --arg v "$NEW" '.version = $v' "$CONF" > "$tmp" && mv "$tmp" "$CONF"
  sed -i "0,/^version = \".*\"/s//version = \"$NEW\"/" app/src-tauri/Cargo.toml
  say "Version $CUR -> $NEW (tauri.conf.json, app/src-tauri/Cargo.toml)"
  CFG=$(jq -c 'del(.version)' <<<"$CFG"); [ "$CFG" != '{}' ] || CFG=''
fi
export COMMONS_TAURI_CONFIG=$CFG

STAGE=$DIST/.staging-$NEW
rm -rf "$STAGE"; mkdir -p "$STAGE"
LOGS=$STAGE/logs; mkdir -p "$LOGS"
JOBS=${COMMONS_BUILD_JOBS:-4}
heavy() { systemd-run --user --scope -q -p MemoryMax=6G "$@"; }

# ---- builds, one at a time (memory)
if want linux; then
  say "Building Linux (log: $LOGS/linux.log)"
  COMMONS_OUT=$STAGE COMMONS_BUILD_JOBS=$JOBS heavy scripts/build-appimage-portable.sh > "$LOGS/linux.log" 2>&1 || die "Linux build failed: $LOGS/linux.log"
  say "Building the Linux browser version (log: $LOGS/linux-browser.log)"
  COMMONS_OUT=$STAGE COMMONS_BUILD_JOBS=$JOBS heavy scripts/build-linux-browser.sh > "$LOGS/linux-browser.log" 2>&1 || die "Linux browser build failed: $LOGS/linux-browser.log"
fi
if want windows; then
  say "Building Windows (log: $LOGS/windows.log)"
  COMMONS_OUT=$STAGE COMMONS_BUILD_JOBS=$JOBS heavy scripts/build-windows.sh > "$LOGS/windows.log" 2>&1 || die "Windows build failed: $LOGS/windows.log"
fi
if want mac; then
  say "Building macOS on $MAC (log: $LOGS/mac.log)"
  mac_ok >/dev/null || die "the Mac is no longer fit to build (unplugged and at 40% or below?)"
  rsync -a --delete -e "ssh -i $MAC_KEY -o BatchMode=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=8" --exclude /target --exclude /target-docker --exclude /dist --exclude /data --exclude /.git \
    --exclude /tools/node_modules --exclude /tools/bin --exclude /app/src-tauri/gen --exclude '*.log' --exclude /mac-watch.sh ./ "$MAC:commons-build/" > "$LOGS/mac.log" 2>&1 \
    || die "could not copy the source to the Mac: $LOGS/mac.log"
  # Two jobs at background priority (scripts/build-mac.sh), and stopped if the Mac drops to 40% on battery.
  ssh -i "$MAC_KEY" -o BatchMode=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=8 "$MAC" 'zsh -s' >> "$LOGS/mac.log" 2>&1 <<MAC || die "macOS build failed: $LOGS/mac.log"
export PATH=\$HOME/.cargo/bin:\$PATH
cd ~/commons-build || exit 2
${CFG:+export TAURI_CONFIG='$CFG'}
# its own process group (so the whole build can be stopped), and the Mac kept awake while it runs
perl -e 'setpgrp(0, 0); exec @ARGV' scripts/build-mac.sh > publish-build.log 2>&1 &
B=\$!
caffeinate -i -w \$B &
while kill -0 \$B 2>/dev/null; do
  sleep 15
  src=\$(pmset -g batt | head -1); pct=\$(pmset -g batt | grep -o '[0-9]*%' | head -1 | tr -d %)
  if [[ "\$src" == *Battery* ]] && [ "\${pct:-0}" -le 40 ]; then echo "battery at \$pct%: stopping the build"; kill -TERM -- -\$B; wait \$B; exit 3; fi
done
wait \$B || { tail -40 publish-build.log; exit 4; }
cd dist && rm -f Megatron.app.tar.gz && COPYFILE_DISABLE=1 tar -czf Megatron.app.tar.gz Megatron.app && ls -la
MAC
  scp -q -i "$MAC_KEY" -o BatchMode=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=8 "$MAC:commons-build/dist/Megatron-macos-arm64.dmg" "$MAC:commons-build/dist/Megatron.app.tar.gz" "$STAGE/" || die "could not fetch the macOS build"
  mv "$STAGE/Megatron.app.tar.gz" "$STAGE/Megatron-macos-arm64.app.tar.gz"
fi

# ---- sign (the password goes through the environment, never the command line)
sign() {
  TAURI_SIGNING_PRIVATE_KEY_PATH=$KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$KEY.password")" "$CLI" signer sign --app-version "$NEW" "$1" > "$LOGS/sign.log" 2>&1 \
    || die "signing $1 failed: $LOGS/sign.log"
  [ "$(keyid_sig "$1.sig")" = "$WANT_ID" ] || die "$1 was signed with a key the apps do not trust (key id $(keyid_sig "$1.sig"), apps trust $WANT_ID)"
}
declare -A UP=()    # platform -> file in the staging folder
# The browser version is its own platform: no Tauri updater target is called "linux-browser-x86_64", so desktop apps
# ignore it (they look for linux-x86_64), and the browser bridge looks only for this one.
want linux   && UP[linux-x86_64]=Megatron-linux-x86_64.AppImage
want linux   && UP[linux-browser-x86_64]=Megatron-linux-browser.tar.gz
want windows && UP[windows-x86_64]=Megatron-windows-x64-setup.exe
want mac     && UP[darwin-aarch64]=Megatron-macos-arm64.app.tar.gz
[ ${#UP[@]} -gt 0 ] || die "nothing to publish"
for p in "${!UP[@]}"; do [ -s "$STAGE/${UP[$p]}" ] || die "missing build output ${UP[$p]}"; sign "$STAGE/${UP[$p]}"; done

# ---- into dist/: versioned update files first, then friends' downloads, the manifest last
mkdir -p "$DIST/update"
upname() { case $1 in linux-x86_64) echo "Megatron_${NEW}_x86_64.AppImage" ;; linux-browser-x86_64) echo "Megatron_${NEW}_linux-browser.tar.gz" ;; windows-x86_64) echo "Megatron_${NEW}_x64-setup.exe" ;; darwin-aarch64) echo "Megatron_${NEW}_aarch64.app.tar.gz" ;; esac; }
platforms='{}'
for p in "${!UP[@]}"; do
  n=$(upname "$p")
  cp --reflink=auto "$STAGE/${UP[$p]}" "$DIST/update/.$n.tmp" && mv "$DIST/update/.$n.tmp" "$DIST/update/$n"
  platforms=$(jq -c --arg p "$p" --arg u "update/$n" --rawfile s "$STAGE/${UP[$p]}.sig" '. + {($p): {url: $u, signature: $s}}' <<<"$platforms")
done
mkdir -p "$DIST/.previous"
for f in Megatron-linux-x86_64.AppImage Megatron-linux-browser.tar.gz Megatron-windows-x64-setup.exe Megatron-macos-arm64.dmg; do
  [ -s "$STAGE/$f" ] || continue
  [ -e "$DIST/$f" ] && cp --reflink=auto -p "$DIST/$f" "$DIST/.previous/$f"
  cp --reflink=auto "$STAGE/$f" "$DIST/.$f.tmp" && chmod 755 "$DIST/.$f.tmp" && mv "$DIST/.$f.tmp" "$DIST/$f"
done
# The portable Windows zip cannot update itself: the installer replaces it on the download page.
if want windows && [ -e "$DIST/Megatron-windows-x64.zip" ]; then mv "$DIST/Megatron-windows-x64.zip" "$DIST/.previous/"; fi
# Systems published earlier for this same version stay in (--same-version, or a rerun); their notes too, unless new ones are given.
OLDM='{}'; [ -s "$DIST/update/latest.json" ] && OLDM=$(jq -c --arg v "$NEW" 'if .version == $v then . else {} end' "$DIST/update/latest.json")
jq -n --arg v "$NEW" --arg n "$NOTES" --arg d "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --arg m "$MIN" --argjson p "$platforms" --argjson o "$OLDM" \
  '{version: $v, notes: (if $n == "" then ($o.notes // "") else $n end), pub_date: $d, platforms: (($o.platforms // {}) + $p)}
   + (if $m != "" then {minimum_version: $m} elif $o.minimum_version then {minimum_version: $o.minimum_version} else {} end)' > "$DIST/update/.latest.json.tmp"
mv "$DIST/update/.latest.json.tmp" "$DIST/update/latest.json"
# Keep this version's update files and the one before (an app may have read the old manifest a moment ago).
keep=$(ls "$DIST/update" | grep -oE '^(Commons|Megatron)_[0-9][^_]*_' | sort -uV | tail -2 | tr '\n' '|')
for f in "$DIST"/update/{Commons,Megatron}_*; do b=$(basename "$f"); [[ "|$keep" == *"|$(grep -oE '^(Commons|Megatron)_[0-9][^_]*_' <<<"$b")|"* ]] || rm -f "$f"; done
rm -rf "$STAGE"

say ""
say "Published $NEW: $(jq -r '.platforms | keys | join(", ")' "$DIST/update/latest.json")"
ls -la "$DIST" "$DIST/update"
[ -n "$SCRATCH" ] || say "Commit the version bump: git commit -am 'Release $NEW'"
