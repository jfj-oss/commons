#!/usr/bin/env bash
# End-to-end test of the Linux browser version's signed update, with a throwaway key and a loopback relay:
# builds dev_bridge with a TEST public key and version 0.2.3 embedded, publishes test-signed tarballs, and runs
# packaging/linux-browser/megatron the way a friend's app menu does (a fake browser records the URL; no window opens).
# Checks: a good update is installed and the page is told; a wrongly signed update is refused and the page says why;
# a downgrade is refused; an unresponsive relay never holds the window. Nothing outside the work folder is touched.
#
#   tools/browser-update-test.sh [workdir]     (default: ~/data/agent-tmp/browser-update/test)
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
W=$(realpath -m "${1:-$HOME/data/agent-tmp/browser-update/test}")
rm -rf "$W"; mkdir -p "$W"/{keys,wrong-keys,site/update,install,xdg,home/bin,run}
PASS=0
pass() { PASS=$((PASS + 1)); printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*" >&2; exit 1; }
said() { printf '\n== %s\n' "$*"; }

SERVER_PID=''; HANG_PID=''
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
  [ -n "$HANG_PID" ] && kill "$HANG_PID" 2>/dev/null || true
  local i="$W/install/megatron-browser"
  if [ -f "$i/.pid" ]; then kill "$(cat "$i/.pid")" 2>/dev/null || true; rm -f "$i/.pid"; fi
}
trap cleanup EXIT

free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}
wait_port() {
  local port=$1
  for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$port/" >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  fail "the bridge did not answer on port $port"
}
note() { curl -sf "http://127.0.0.1:$1/update-note" 2>/dev/null || true; }

said "a throwaway key pair, and a bridge built with it (version 0.2.3)"
EMBED=$(cargo run -q -p commons-core --example minisign-test-sign -- gen "$W/keys")
WRONG=$(cargo run -q -p commons-core --example minisign-test-sign -- gen "$W/wrong-keys")
[ "$EMBED" != "$WRONG" ] || fail "the two test keys are the same"
MEGATRON_BROWSER_VERSION=0.2.3 MEGATRON_BROWSER_PUBKEY="$EMBED" \
  cargo build -q -p commons-core --features devbridge --bin dev_bridge
TARGET=${CARGO_TARGET_DIR:-$ROOT/target}
BRIDGE="$TARGET/debug/dev_bridge"
[ -x "$BRIDGE" ] || fail "no bridge at $BRIDGE"
pass "built a bridge that trusts only the throwaway key"

I="$W/install/megatron-browser"
mkdir -p "$I"
cp "$BRIDGE" "$I/bridge"; chmod +x "$I/bridge"
cp -r app/ui "$I/ui"
cp packaging/linux-browser/megatron "$I/megatron"; chmod +x "$I/megatron"
printf 'old' > "$I/ui/.version-marker"
SITE="$W/site"

# A stand-in for brave: the launcher execs the first browser it finds. It records the URL and exits.
cat > "$W/home/bin/brave" <<'SH'
#!/bin/sh
[ -n "${MEGATRON_TEST_BROWSER_LOG:-}" ] && printf '%s\n' "$@" > "$MEGATRON_TEST_BROWSER_LOG"
exit 0
SH
# There may be a real desktop app open on this machine; the test's launcher must not care (it never touches it).
cat > "$W/home/bin/pgrep" <<'SH'
#!/bin/sh
exit 1
SH
chmod +x "$W/home/bin/brave" "$W/home/bin/pgrep"

make_tar() { # <version> <marker> <key-dir> -> prints the signature
  local v=$1 marker=$2 key=$3 pkg="$W/pkg-$1" tar="$W/Megatron_${1}_linux-browser.tar.gz"
  rm -rf "$pkg"; mkdir -p "$pkg/megatron-browser"
  cp "$BRIDGE" "$pkg/megatron-browser/bridge"; chmod +x "$pkg/megatron-browser/bridge"
  cp -r app/ui "$pkg/megatron-browser/ui"
  printf '%s' "$marker" > "$pkg/megatron-browser/ui/.version-marker"
  tar -C "$pkg" -czf "$tar" megatron-browser
  cargo run -q -p commons-core --example minisign-test-sign -- sign "$key" "$tar" "$v"
}
write_manifest() { # <version> <signature>
  cp "$W/Megatron_${1}_linux-browser.tar.gz" "$SITE/update/Megatron_${1}_linux-browser.tar.gz"
  python3 - "$SITE/update/latest.json" "$1" "$2" <<'PY'
import json, sys
path, version, signature = sys.argv[1], sys.argv[2], sys.argv[3]
json.dump({"version": version, "notes": "test", "platforms": {
    "linux-x86_64": {"url": "update/desktop.AppImage", "signature": "ignored-by-the-browser"},
    "linux-browser-x86_64": {"url": f"update/Megatron_{version}_linux-browser.tar.gz", "signature": signature},
}}, open(path, "w"))
PY
}
launch() { # <port>
  rm -f "$W/browser.log"
  env -i PATH="$W/home/bin:/usr/local/bin:/usr/bin:/bin" HOME="$W/home" \
    XDG_DATA_HOME="$W/xdg" MEGATRON_PORT="$1" MEGATRON_TEST_BROWSER_LOG="$W/browser.log" \
    "$I/megatron" 2>>"$W/launcher.log"
}
stop_bridge() { "$I/megatron" --stop >/dev/null 2>&1 || true; }

RELAY_PORT=$(free_port)
RELAY="http://127.0.0.1:$RELAY_PORT"
python3 -m http.server "$RELAY_PORT" --bind 127.0.0.1 --directory "$SITE" >"$W/http.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 50); do curl -sf "$RELAY/" >/dev/null 2>&1 && break; sleep 0.1; done
printf '%s' "$RELAY" > "$I/relay.txt"

said "a good 0.2.4 is installed before the page opens"
SIG=$(make_tar 0.2.4 new "$W/keys")
write_manifest 0.2.4 "$SIG"
PORT=$(free_port)
launch "$PORT"
wait_port "$PORT"
[ "$(cat "$I/ui/.version-marker")" = new ] || fail "the new ui/ was not installed"
[ ! -e "$I/.update-previous" ] || fail "the old copy was not removed after the new bridge started"
N=$(note "$PORT")
case $N in *'"kind":"updated"'*'"version":"0.2.4"'*) ;; *) fail "expected an 'Updated to 0.2.4' note, got: $N" ;; esac
[ -n "$(cat "$W/browser.log" 2>/dev/null || true)" ] || fail "no browser was opened"
stop_bridge
pass "0.2.4 installed, the page said Updated to 0.2.4"

said "a wrongly signed 0.2.5 is refused, and the page says why"
SHA_BEFORE=$(sha256sum "$I/bridge" | cut -d' ' -f1)
SIG=$(make_tar 0.2.5 bad "$W/wrong-keys")
write_manifest 0.2.5 "$SIG"
PORT=$(free_port)
launch "$PORT"
wait_port "$PORT"
[ "$(cat "$I/ui/.version-marker")" = new ] || fail "a wrongly signed update changed the ui/"
[ "$(sha256sum "$I/bridge" | cut -d' ' -f1)" = "$SHA_BEFORE" ] || fail "a wrongly signed update changed the bridge"
N=$(note "$PORT")
case $N in *'"kind":"available"'*) ;; *) fail "expected an 'Update available' note, got: $N" ;; esac
case $N in *'"version":"0.2.5"'*) ;; *) fail "the note does not name 0.2.5: $N" ;; esac
case $N in *signature*) ;; *) fail "the note does not give the signature reason: $N" ;; esac
stop_bridge
pass "wrong signature refused; the note gives the reason"

said "a correctly signed 0.2.2 (a downgrade) is ignored"
SIG=$(make_tar 0.2.2 old "$W/keys")
write_manifest 0.2.2 "$SIG"
PORT=$(free_port)
launch "$PORT"
wait_port "$PORT"
[ "$(cat "$I/ui/.version-marker")" = new ] || fail "a downgrade changed the ui/"
N=$(note "$PORT")
[ "$N" = "{}" ] || fail "a downgrade left a note: $N"
stop_bridge
pass "downgrade refused without a word"

said "an unresponsive relay never holds the window"
kill "$SERVER_PID" 2>/dev/null || true; wait "$SERVER_PID" 2>/dev/null || true; SERVER_PID=''
HANG_PORT=$(free_port)
python3 - "$HANG_PORT" >/dev/null 2>&1 <<'PY' &
import socket, sys, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(8)
while True:
    c, _ = s.accept()
    time.sleep(3600)
PY
HANG_PID=$!
sleep 0.3
printf 'http://127.0.0.1:%s' "$HANG_PORT" > "$I/relay.txt"
PORT=$(free_port)
T0=$(date +%s%N)
launch "$PORT"
T1=$(date +%s%N)
MS=$(( (T1 - T0) / 1000000 ))
wait_port "$PORT"
[ -n "$(cat "$W/browser.log" 2>/dev/null || true)" ] || fail "the browser did not open while the relay hung"
[ "$MS" -lt 7000 ] || fail "the launcher waited ${MS}ms for an unresponsive relay (expected under 7000ms)"
stop_bridge
pass "the window opened in ${MS}ms with an unresponsive relay"

# Leave target/ with the ordinary bridge, not the one built with the throwaway key and test version.
cargo build -q -p commons-core --features devbridge --bin dev_bridge

printf '\n%d checks passed\n' "$PASS"
