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
# The launcher gives the bridge a per-start secret and keeps it in $I/.bridge-secret-<port> (mode 600); the page sends it back.
sec() { cat "$I/.bridge-secret-$1" 2>/dev/null || true; }
note() { curl -sf -H "X-Megatron-Secret: $(sec "$1")" "http://127.0.0.1:$1/update-note" 2>/dev/null || true; }

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

said "the Update button: a running bridge finds, refuses and installs an update, then restarts itself"
api() { curl -sf -H "X-Megatron-Secret: $(sec "$PORT")" "http://127.0.0.1:$PORT$1" "${@:2}" 2>/dev/null || true; }
PORT=$(free_port)
launch "$PORT"
wait_port "$PORT"
PID_BEFORE=$(cat "$I/.pid")
N=$(api /update-check)
case $N in *'"available":false'*) ;; *) fail "the check offered a downgrade: $N" ;; esac
SIG=$(make_tar 0.2.6 bad "$W/wrong-keys")
write_manifest 0.2.6 "$SIG"
N=$(api /update-check)
case $N in *'"available":true'*'"version":"0.2.6"'*) ;; *) fail "the check did not offer 0.2.6: $N" ;; esac
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Origin: https://example.org' -H "X-Megatron-Secret: $(sec "$PORT")" "http://127.0.0.1:$PORT/update-install")
[ "$CODE" = 403 ] || fail "another website could press Update (HTTP $CODE)"
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/update-install")
[ "$CODE" = 403 ] || fail "a program without the secret could press Update (HTTP $CODE)"
N=$(api /update-install -X POST)
case $N in *'"error"'*signature*) ;; *) fail "a wrongly signed update was not refused with the reason: $N" ;; esac
[ "$(cat "$I/ui/.version-marker")" = new ] || fail "the refused update changed the ui/"
[ "$(cat "$I/.pid")" = "$PID_BEFORE" ] && kill -0 "$PID_BEFORE" || fail "the bridge did not survive a refused update"
SIG=$(make_tar 0.2.6 button "$W/keys")
write_manifest 0.2.6 "$SIG"
N=$(api /update-install -X POST)
case $N in *'"restarting":true'*'"version":"0.2.6"'*) ;; *) fail "the update was not installed: $N" ;; esac
sleep 1
wait_port "$PORT"
sleep 0.5
[ "$(cat "$I/ui/.version-marker")" = button ] || fail "the Update button did not install the new ui/"
[ ! -e "$I/.update-previous" ] || fail "the new bridge did not remove the old copy"
kill -0 "$PID_BEFORE" || fail "the restart changed the process the launcher tracks"
N=$(note "$PORT")
case $N in *'"kind":"updated"'*'"version":"0.2.6"'*) ;; *) fail "expected an 'Updated to 0.2.6' note, got: $N" ;; esac
stop_bridge
pass "Update refused a bad signature and another website, then installed a good one and restarted"
printf 'old' > "$I/ui/.version-marker"

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

said "the window: own profile, secret in the fragment only, every RPC and update route refuses without it"
rm -f "$I/.update-previous"
PORT=$(free_port)
launch "$PORT"
wait_port "$PORT"
SECRET=$(sec "$PORT")
[ "${#SECRET}" -ge 32 ] || fail "no secret was kept"
[ "$(stat -c %a "$I/.bridge-secret-$PORT")" = 600 ] || fail "the secret file is not mode 600"
BL=$(cat "$W/browser.log")
case $BL in *"--app=http://127.0.0.1:$PORT/#s=$SECRET"*) ;; *) fail "the window was not opened at the fragment URL: $BL" ;; esac
case $BL in *"--user-data-dir=$W/xdg/app.commons.chat.browser-profile"*) ;; *) fail "the window has no profile of its own: $BL" ;; esac
[ "$(stat -c %a "$W/xdg/app.commons.chat.browser-profile")" = 700 ] || fail "the profile folder is not private"
RPC='{"method":"state","args":{}}'
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
B="http://127.0.0.1:$PORT"
for pair in "POST:/rpc" "GET:/update-check" "GET:/update-note" "POST:/update-install"; do
  m=${pair%%:*}; r=${pair#*:}
  [ "$(code -X "$m" --data "$RPC" "$B$r")" = 403 ] || fail "$pair answered without the secret"
  [ "$(code -X "$m" --data "$RPC" -H "X-Megatron-Secret: ${SECRET%?}x" "$B$r")" = 403 ] || fail "$pair answered with a wrong secret"
done
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST --data "$RPC" -H "X-Megatron-Secret: $SECRET" "http://127.0.0.1:$PORT/rpc")
[ "$CODE" = 200 ] || fail "the right secret was refused (HTTP $CODE)"
[ "$(curl -s "http://127.0.0.1:$PORT/mode")" = "{\"launcher_updated\":false,\"packaged\":true,\"secret\":true}" ] || fail "mode: $(curl -s "http://127.0.0.1:$PORT/mode")"
# a second start reuses the same bridge and the same secret
PID1=$(cat "$I/.pid"); launch "$PORT"
[ "$(cat "$I/.pid")" = "$PID1" ] && [ "$(sec "$PORT")" = "$SECRET" ] || fail "a second start did not reuse the running bridge"
pass "fragment URL, own profile, secret file 600, every protected route refuses without the right secret"
stop_bridge
[ ! -e "$I/.bridge-secret-$PORT" ] || fail "--stop left the secret file"

said "a program already on the port is refused, not handed the window"
PORT=$(free_port)
python3 - "$PORT" >/dev/null 2>&1 <<'PY' &
import socket, sys, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(8)
while True:
    c, _ = s.accept()
    try: c.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
    except Exception: pass
    c.close()
PY
SQUAT_PID=$!
sleep 0.3
if launch "$PORT"; then fail "the launcher opened the window onto a squatter"; fi
[ -z "$(cat "$W/browser.log" 2>/dev/null || true)" ] || fail "a browser was opened onto the squatter"
kill -0 "$SQUAT_PID" || fail "the launcher killed a program that is not its own"
kill "$SQUAT_PID" 2>/dev/null || true
pass "a squatter on the port is refused and left alone"

said "UPGRADE: an old launcher with the new bridge keeps working (open mode), the bridge refreshes the launcher, the new one then turns the protection on"
cp crates/core/tests/fixtures/old-linux-launcher.sh "$I/megatron"; chmod +x "$I/megatron"
PORT=$(free_port)
launch "$PORT"
wait_port "$PORT"
BL=$(cat "$W/browser.log")
case $BL in *"--app=http://127.0.0.1:$PORT/"*) ;; *) fail "the old launcher did not open the plain URL: $BL" ;; esac
case $BL in *"#s="*) fail "the old launcher somehow had a secret" ;; esac
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST --data "$RPC" "http://127.0.0.1:$PORT/rpc")
[ "$CODE" = 200 ] || fail "the old launcher is locked out of the new bridge (HTTP $CODE)"
curl -sf "http://127.0.0.1:$PORT/mode" | grep -q '"secret":false' || fail "the bridge claims a secret it was never given"
sleep 0.5
grep -q 'megatron-launcher-protocol: 2' "$I/megatron" || fail "the bridge did not refresh the old launcher script"
[ -f "$I/megatron.old" ] && grep -q 'pgrep' "$I/megatron.old" || fail "the old launcher was not kept as megatron.old"
curl -sf "http://127.0.0.1:$PORT/mode" | grep -q '"launcher_updated":true' || fail "the page is not told the launcher was refreshed"
OLDPID=$(cat "$I/.pid")
pass "old launcher: plain URL works, open mode, launcher refreshed on disk"
launch "$PORT"
wait_port "$PORT"
SECRET=$(sec "$PORT")
[ "${#SECRET}" -ge 32 ] || fail "the refreshed launcher did not start a protected bridge"
[ "$(cat "$I/.pid")" != "$OLDPID" ] || fail "the old open-mode bridge was not replaced"
case $(cat "$W/browser.log") in *"#s=$SECRET"*) ;; *) fail "no fragment after the upgrade" ;; esac
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST --data "$RPC" "http://127.0.0.1:$PORT/rpc")
[ "$CODE" = 403 ] || fail "the bridge stayed open after the upgrade (HTTP $CODE)"
stop_bridge
pass "next start with the refreshed launcher: the open bridge was replaced by a protected one"

said "UPGRADE: the new launcher with an older bridge build (ignores the secret) still opens, with a plain warning"
cp "$I/bridge" "$I/bridge.real"
printf '#!/bin/sh\nexec env -u MEGATRON_SECRET "%s/bridge.real" "$@"\n' "$I" > "$I/bridge"; chmod +x "$I/bridge"
PORT=$(free_port)
launch "$PORT" || fail "the new launcher refused an older bridge"
wait_port "$PORT"
case $(cat "$W/browser.log") in *"--app=http://127.0.0.1:$PORT/"*) ;; *) fail "not opened at the plain URL" ;; esac
case $(cat "$W/browser.log") in *"#s="*) fail "a secret went to a bridge that cannot use it" ;; esac
grep -q 'too old to protect' "$W/launcher.log" || fail "no plain warning for the older bridge"
stop_bridge
mv -f "$I/bridge.real" "$I/bridge"
pass "older bridge: works, plain URL, warning said"

# Leave target/ with the ordinary bridge, not the one built with the throwaway key and test version.
cargo build -q -p commons-core --features devbridge --bin dev_bridge

printf '\n%d checks passed\n' "$PASS"
