#!/usr/bin/env bash
# Try Megatron on this machine with two independent instances ("two people") and a local relay.
# The relay binds 127.0.0.1 only; nothing leaves this computer.
#   scripts/try-two-windows.sh          # builds, starts the relay and two app windows
#   scripts/try-two-windows.sh browser  # same, but two browser tabs instead of Tauri windows (dev bridge)
set -euo pipefail
cd "$(dirname "$0")/.."
MODE=${1:-tauri}
DATA=${COMMONS_TRY_DIR:-$HOME/.local/share/commons-try}
PORT=${COMMONS_TRY_PORT:-18787}
RELAY=http://127.0.0.1:$PORT
mkdir -p "$DATA"
cargo build -j 6 -p commons-relay
pids=()
trap 'kill ${pids[@]} 2>/dev/null || true' EXIT
./target/debug/commons-relay --bind 127.0.0.1:$PORT --data "$DATA/relay" & pids+=($!)
sleep 1
if [ "$MODE" = browser ]; then
  cargo build -j 6 -p commons-core --features devbridge --bin dev_bridge
  COMMONS_DATA="$DATA/alice" COMMONS_RELAY=$RELAY ./target/debug/dev_bridge 7001 app/ui & pids+=($!)
  COMMONS_DATA="$DATA/bob"   COMMONS_RELAY=$RELAY ./target/debug/dev_bridge 7002 app/ui & pids+=($!)
  echo "Open http://127.0.0.1:7001 (Alice) and http://127.0.0.1:7002 (Bob). Ctrl-C stops everything."
else
  cargo build -j 6 -p commons-app
  COMMONS_DATA="$DATA/alice" COMMONS_RELAY=$RELAY ./target/debug/commons-app & pids+=($!)
  COMMONS_DATA="$DATA/bob"   COMMONS_RELAY=$RELAY ./target/debug/commons-app & pids+=($!)
  echo "Two Megatron windows are open. Create an account in each, make a community in one, invite from the Invite button, paste the link in the other (switcher > Join with an invite link)."
fi
wait
