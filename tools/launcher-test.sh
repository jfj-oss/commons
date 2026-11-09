#!/usr/bin/env bash
# The Windows/macOS launcher (crates/core/src/bin/megatron_launcher.rs), run on Linux with a fake browser: the secret protocol.
# Fresh start, reuse, a squatter on the port, an open-mode bridge of ours from before (taken over), an older bridge build that
# ignores the secret. Needs: cargo build -p commons-core --features devbridge --bin dev_bridge --bin megatron_launcher
#   tools/launcher-test.sh [workdir]
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
T=${CARGO_TARGET_DIR:-$ROOT/target}/debug
W=$(realpath -m "${1:-$HOME/data/agent-tmp/auditfix/launcher-test}")
rm -rf "$W"; mkdir -p "$W/i" "$W/data"
PASS=0; pass() { PASS=$((PASS + 1)); printf 'PASS  %s\n' "$*"; }; fail() { printf 'FAIL  %s\n' "$*" >&2; exit 1; }
cp "$T/dev_bridge" "$W/i/bridge"; cp "$T/megatron_launcher" "$W/i/launcher"; cp -r app/ui "$W/i/ui"; : > "$W/i/relay.txt"
cat > "$W/browser" <<'SH'
#!/bin/sh
printf '%s\n' "$@" > "$BROWSER_LOG"
SH
chmod +x "$W/browser"
free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }
launch() { rm -f "$W/b.log"; env MEGATRON_PORT="$1" MEGATRON_DATA="$W/data" MEGATRON_NO_UPDATE=1 MEGATRON_BROWSER="$W/browser" BROWSER_LOG="$W/b.log" "$W/i/launcher"; sleep 0.4; }
stop() { env MEGATRON_PORT="$PORT" MEGATRON_DATA="$W/data" "$W/i/launcher" --stop || true; }
trap 'stop 2>/dev/null; [ -n "${SQ:-}" ] && kill "$SQ" 2>/dev/null; true' EXIT
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

PORT=$(free_port)
launch "$PORT"
S=$(cat "$W/data/.bridge-secret-$PORT")
[ "${#S}" -ge 32 ] || fail "no secret kept"
[ "$(stat -c %a "$W/data/.bridge-secret-$PORT")" = 600 ] || fail "secret file mode"
grep -qx -- "--app=http://127.0.0.1:$PORT/#s=$S" "$W/b.log" || fail "url: $(cat "$W/b.log")"
grep -qx -- "$W/data/browser-profile" "$W/b.log" || fail "no own profile: $(cat "$W/b.log")"
[ "$(code -X POST --data '{"method":"state","args":{}}' "http://127.0.0.1:$PORT/rpc")" = 403 ] || fail "rpc without secret"
[ "$(code -X POST --data '{"method":"state","args":{}}' -H "X-Megatron-Secret: $S" "http://127.0.0.1:$PORT/rpc")" = 200 ] || fail "rpc with secret"
pass "fresh start: fragment URL, own profile, secret file 600, rpc refused without it"
PID=$(cat "$W/i/.pid"); launch "$PORT"
[ "$(cat "$W/i/.pid")" = "$PID" ] && [ "$(cat "$W/data/.bridge-secret-$PORT")" = "$S" ] || fail "second start did not reuse"
grep -qx -- "--app=http://127.0.0.1:$PORT/#s=$S" "$W/b.log" || fail "second url"
pass "second start reuses the bridge and its secret"
stop; sleep 0.3

# an open-mode bridge of ours (what the previous launcher started): taken over, not trusted
(env COMMONS_DATA="$W/data" COMMONS_RELAY= setsid "$W/i/bridge" "$PORT" "$W/i/ui" >/dev/null 2>&1 & echo $! > "$W/i/.pid"); sleep 1
[ "$(code -X POST --data '{"method":"state","args":{}}' "http://127.0.0.1:$PORT/rpc")" = 200 ] || fail "setup: open bridge"
OLD=$(cat "$W/i/.pid"); launch "$PORT"
[ "$(cat "$W/i/.pid")" != "$OLD" ] || fail "open bridge kept"
[ "$(code -X POST --data '{"method":"state","args":{}}' "http://127.0.0.1:$PORT/rpc")" = 403 ] || fail "still open after takeover"
grep -q "#s=" "$W/b.log" || fail "no secret url after takeover"
pass "an open-mode bridge of ours was replaced by a protected one"
stop; sleep 0.3

# a squatter
python3 - "$PORT" >/dev/null 2>&1 <<'PY' &
import socket, sys
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(8)
while True:
    c, _ = s.accept(); c.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"); c.close()
PY
SQ=$!; sleep 0.4
launch "$PORT"
[ ! -s "$W/b.log" ] || fail "browser opened onto a squatter"
kill -0 "$SQ" || fail "the launcher killed a foreign program"
kill "$SQ"; SQ=""; sleep 0.3
pass "a program on the port is refused and left alone"

# an older bridge build (ignores the secret)
mv "$W/i/bridge" "$W/i/bridge.real"
printf '#!/bin/sh\nexec env -u MEGATRON_SECRET "%s/bridge.real" "$@"\n' "$W/i" > "$W/i/bridge"; chmod +x "$W/i/bridge"
launch "$PORT"
grep -qx -- "--app=http://127.0.0.1:$PORT/" "$W/b.log" || fail "older bridge: $(cat "$W/b.log")"
[ "$(code -X POST --data '{"method":"state","args":{}}' "http://127.0.0.1:$PORT/rpc")" = 200 ] || fail "older bridge locked out"
pass "an older bridge build still opens (plain URL)"
printf '\n%d checks passed\n' "$PASS"
