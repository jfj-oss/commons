#!/usr/bin/env bash
# Megatron with working calls on this one computer: relay + LiveKit + two people. Everything is on 127.0.0.1 and nothing
# can be reached from outside. Calls are end-to-end encrypted exactly as they are between friends; there is no plain mode.
#   scripts/run-local.sh            two app windows (on Linux a call opens in your browser)
#   scripts/run-local.sh browser    two browser tabs instead of app windows (:18801 and :18802); the call runs in the tab
#   scripts/run-local.sh servers    only the relay and LiveKit, for your own app: COMMONS_RELAY=http://127.0.0.1:18795
# Ctrl-C stops everything it started. Data stays in ~/.local/share/commons-local (COMMONS_LOCAL_DIR to change it).
set -euo pipefail
cd "$(dirname "$0")/.."
MODE=${1:-app}
DIR=${COMMONS_LOCAL_DIR:-$HOME/.local/share/commons-local}
RELAY_PORT=${COMMONS_LOCAL_PORT:-18795}
LK_PORT=$((RELAY_PORT + 90)) # 18885, then 18886 and 18887 for media
RELAY=http://127.0.0.1:$RELAY_PORT
J=${COMMONS_BUILD_JOBS:-4}

for p in "$RELAY_PORT" "$LK_PORT" $((LK_PORT + 1)); do
  if ss -ltnH "sport = :$p" | grep -q .; then echo "port $p is in use; set COMMONS_LOCAL_PORT to another base" >&2; exit 1; fi
done
scripts/get-livekit.sh >/dev/null
cargo build -j "$J" -p commons-relay
mkdir -p "$DIR"; chmod 700 "$DIR"
# A random key made here. It only guards this loopback LiveKit, but a fixed one would end up being copied somewhere.
[ -s "$DIR/lk-secret" ] || (umask 077; openssl rand -hex 32 > "$DIR/lk-secret")
(umask 077; cat > "$DIR/livekit.yaml" <<Y
port: $LK_PORT
bind_addresses: ["127.0.0.1"]
rtc:
  tcp_port: $((LK_PORT + 1))
  udp_port: $((LK_PORT + 2))
  use_external_ip: false
  node_ip: 127.0.0.1
  enable_loopback_candidate: true
  ips:
    includes: ["127.0.0.1/32"]
keys:
  commonslocal: $(cat "$DIR/lk-secret")
room:
  auto_create: true
logging:
  level: warn
Y
)
pids=()
stop() { [ ${#pids[@]} -eq 0 ] || kill "${pids[@]}" 2>/dev/null || true; wait 2>/dev/null || true; }
trap stop EXIT
tools/bin/livekit-server --config "$DIR/livekit.yaml" >"$DIR/livekit.log" 2>&1 & pids+=($!)
./target/debug/commons-relay --bind "127.0.0.1:$RELAY_PORT" --data "$DIR/relay" --livekit-url "ws://127.0.0.1:$LK_PORT" --livekit-key commonslocal --livekit-secret-file "$DIR/lk-secret" >"$DIR/relay.log" 2>&1 & pids+=($!)
for _ in $(seq 50); do curl -fs "$RELAY/health" >/dev/null 2>&1 && break; sleep 0.1; done
curl -fs "$RELAY/health" >/dev/null || { echo "the relay did not start; see $DIR/relay.log" >&2; exit 1; }

case "$MODE" in
  servers)
    echo "Relay $RELAY, LiveKit ws://127.0.0.1:$LK_PORT. Ctrl-C stops them." ;;
  browser)
    cargo build -j "$J" -p commons-core --features devbridge --bin dev_bridge
    COMMONS_DATA="$DIR/alice" COMMONS_RELAY=$RELAY ./target/debug/dev_bridge 18801 app/ui & pids+=($!)
    COMMONS_DATA="$DIR/bob"   COMMONS_RELAY=$RELAY ./target/debug/dev_bridge 18802 app/ui & pids+=($!)
    echo "Open http://127.0.0.1:18801 (one person) and http://127.0.0.1:18802 (the other). Ctrl-C stops everything." ;;
  app)
    cargo build -j "$J" -p commons-app
    COMMONS_DATA="$DIR/alice" COMMONS_RELAY=$RELAY ./target/debug/commons-app & pids+=($!)
    COMMONS_DATA="$DIR/bob"   COMMONS_RELAY=$RELAY ./target/debug/commons-app & pids+=($!)
    echo "Two windows are open. Make an account in each, a community in one, invite the other, then press Call in a room. Ctrl-C stops everything." ;;
  *) echo "usage: scripts/run-local.sh [app|browser|servers]" >&2; exit 2 ;;
esac
wait
