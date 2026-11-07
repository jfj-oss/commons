#!/usr/bin/env bash
# Takes Megatron off the internet and stops what go-online.sh started. Accounts and messages stay on disk.
# Only port 10000 is closed. Never `tailscale funnel reset`: that would also clear this PC's other served ports.
set -uo pipefail
DIR=${COMMONS_HOST_DIR:-$HOME/.local/share/commons-host}
if [ "${1:-}" != --local-test ] && command -v tailscale >/dev/null; then
  tailscale funnel --yes --tls-terminated-tcp=10000 off || echo "go-offline: Tailscale did not close port 10000 (it may already be closed). Check: tailscale funnel status" >&2
fi
stop() {
  local f="$DIR/$1.pid" pid
  [ -s "$f" ] || return 0
  pid=$(cat "$f")
  if [ "$(basename "$(readlink "/proc/$pid/exe" 2>/dev/null)")" = "$2" ]; then
    kill "$pid" 2>/dev/null
    for _ in $(seq 50); do [ -d "/proc/$pid" ] || break; sleep 0.1; done
    [ -d "/proc/$pid" ] && kill -9 "$pid" 2>/dev/null
  fi
  rm -f "$f"
}
stop relay commons-relay
stop livekit livekit-server
echo "Megatron is offline. Your data is still in $DIR."
