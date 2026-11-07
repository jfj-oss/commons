#!/usr/bin/env bash
# Puts Commons on the internet from this PC through Tailscale Funnel, and prints the link to send to friends.
# Run it yourself; deploy/go-offline.sh undoes it. Nothing here starts by itself after a reboot.
#
# Why port 10000, and one port for everything: Funnel offers 443, 8443 and 10000, and it opens a whole port. On this PC
# 443 and 8443 carry tailnet-only services (Vaultwarden among them). Funnelling those ports would publish them. So this
# script never touches them: one TLS-terminated stream on 10000 goes to the relay's --front, which splits it into the
# relay, LiveKit signalling and TURN by its first bytes (crates/relay/src/front.rs). Media rides on TURN over TLS, because
# a Funnel can carry TCP only. See docs/DEPLOY.md.
#
# Options, for tools/hosting-e2e.mjs only:  --local-test NAME PORT  skip Tailscale; NAME:PORT is a local TLS terminator
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
DIR=${COMMONS_HOST_DIR:-$HOME/.local/share/commons-host}
BASE=${COMMONS_HOST_BASE:-17880}             # local ports, loopback only
LK=$BASE RTC_UDP=$((BASE+2)) TURN=$((BASE+3)) RELAY=$((BASE+4)) FRONT=$((BASE+5)) RANGE=$((BASE+20))
PUBLIC_PORT=10000
HOST='' TEST=0
if [ "${1:-}" = --local-test ]; then TEST=1; HOST=$2; PUBLIC_PORT=$3; fi
say() { printf '%s\n' "$*"; }
die() { printf 'go-online: %s\n' "$*" >&2; exit 1; }

# ---- Tailscale: read-only checks first, so nothing has started if one fails
if [ $TEST = 0 ]; then
  command -v tailscale >/dev/null || die "Tailscale is not installed."
  ts=$(tailscale status --json) || die "Tailscale is not running."
  HOST=$(jq -r '.Self.DNSName | rtrimstr(".")' <<<"$ts")
  [ -n "$HOST" ] && [ "$HOST" != null ] || die "could not read this machine's Tailscale name."
  jq -e '.CertDomains | length > 0' <<<"$ts" >/dev/null || die "HTTPS certificates are off for this tailnet. Turn on HTTPS in the Tailscale admin console (DNS page)."
  serve=$(tailscale serve status --json 2>/dev/null || echo '{}')
  if jq -e --arg p "$PUBLIC_PORT" '(.TCP // {}) | has($p)' <<<"$serve" >/dev/null; then
    want="127.0.0.1:$FRONT"
    jq -e --arg p "$PUBLIC_PORT" --arg w "$want" '.TCP[$p].TCPForward == $w' <<<"$serve" >/dev/null || die "port $PUBLIC_PORT is already served by something else here. Not touching it."
  fi
fi

# ---- our files
mkdir -p "$DIR"; chmod 700 "$DIR"
# Made once and kept: changing them would lock out everyone who has an old link.
[ -s "$DIR/invite-secret" ] || (umask 077; openssl rand -hex 24 > "$DIR/invite-secret")
[ -s "$DIR/lk-secret" ] || (umask 077; openssl rand -hex 32 > "$DIR/lk-secret")
INVITE=$(cat "$DIR/invite-secret")
[ ${#INVITE} -ge 32 ] || die "$DIR/invite-secret is too short. An invite secret is required: delete the file to get a new one."
[ -x tools/bin/livekit-server ] || scripts/get-livekit.sh
BIN=target/release/commons-relay
[ -x "$BIN" ] || BIN=tools/bin/commons-relay
[ -x "$BIN" ] || { cargo build --release -j 4 -p commons-relay; BIN=target/release/commons-relay; }
mkdir -p dist

(umask 077; cat > "$DIR/livekit.yaml" <<Y
# Written by deploy/go-online.sh. The way in from outside is the front. LiveKit's TURN port ignores bind_addresses, so it
# also listens on the LAN; it needs a token's credentials to do anything.
port: $LK
bind_addresses: ["127.0.0.1"]
rtc:
  tcp_port: 0                # no ICE over TCP: a Funnel cannot carry it, and it would listen on every interface
  udp_port: $RTC_UDP
  use_external_ip: false
  node_ip: 127.0.0.1
  enable_loopback_candidate: true
  ips:                       # without this it opens a socket per interface, and whichever wins a race can leave loopback deaf
    includes: ["127.0.0.1/32"]
turn:
  enabled: true
  domain: $HOST
  tls_port: $TURN            # plain TCP here: Funnel has already taken the TLS off (external_tls)
  external_tls: true
  relay_range_start: $RANGE
  relay_range_end: $((RANGE + 60))
  # TURN refuses private peers by default, and this SFU is one. A call member can therefore reach UDP on loopback through TURN.
  allow_restricted_peer_cidrs: ["127.0.0.1/32"]
keys:
  COMMONS_KEY: $(cat "$DIR/lk-secret")
room:
  auto_create: true
  empty_timeout: 60
  max_participants: 12
logging:
  level: ${COMMONS_HOST_LOG:-warn}
Y
)

# ---- start, unless already running
alive() { [ -s "$DIR/$1.pid" ] && [ "$(basename "$(readlink "/proc/$(cat "$DIR/$1.pid")/exe" 2>/dev/null)")" = "$2" ]; }
free() { ! ss -ltnH "sport = :$1" | grep -q .; }
if ! alive livekit livekit-server; then
  for p in "$LK" "$TURN"; do free "$p" || die "local port $p is in use; set COMMONS_HOST_BASE to another base."; done
  setsid nohup tools/bin/livekit-server --config "$DIR/livekit.yaml" >"$DIR/livekit.log" 2>&1 </dev/null &
  echo $! > "$DIR/livekit.pid"
fi
if ! alive relay commons-relay; then
  for p in "$RELAY" "$FRONT"; do free "$p" || die "local port $p is in use; set COMMONS_HOST_BASE to another base."; done
  COMMONS_RELAY_INVITE=$INVITE setsid nohup "$BIN" --bind "127.0.0.1:$RELAY" --data "$DIR/relay-data" --site "$ROOT/dist" \
    --front "127.0.0.1:$FRONT" --front-livekit "127.0.0.1:$LK" --front-turn "127.0.0.1:$TURN" \
    --livekit-url "wss://$HOST:$PUBLIC_PORT" --livekit-key COMMONS_KEY --livekit-secret-file "$DIR/lk-secret" --turn-public-port "$PUBLIC_PORT" \
    >"$DIR/relay.log" 2>&1 </dev/null &
  echo $! > "$DIR/relay.pid"
fi
for _ in $(seq 100); do curl -fs "http://127.0.0.1:$FRONT/health" >/dev/null 2>&1 && break; sleep 0.1; done
[ "$(curl -fs "http://127.0.0.1:$FRONT/health" || true)" = ok ] || die "the relay did not start; see $DIR/relay.log"
for _ in $(seq 100); do curl -fs "http://127.0.0.1:$LK/" >/dev/null 2>&1 && break; sleep 0.1; done
curl -fs "http://127.0.0.1:$LK/" >/dev/null || die "LiveKit did not start; see $DIR/livekit.log"

# ---- open the door
if [ $TEST = 0 ]; then
  if ! jq -e '[.Self.CapMap // {} | keys[] | select(test("funnel"))] | length > 0' <<<"$ts" >/dev/null; then
    say "Funnel is not switched on for this tailnet yet. Tailscale will now print a link: open it, approve, and this carries on."
  fi
  tailscale funnel --bg --yes --tls-terminated-tcp="$PUBLIC_PORT" "tcp://127.0.0.1:$FRONT"
  # A look from outside: the public DNS answer, not the tailnet's, so this really goes through Funnel.
  ok=0
  for _ in $(seq 18); do
    [ "$(curl -fs --max-time 8 --doh-url https://1.1.1.1/dns-query "https://$HOST:$PUBLIC_PORT/health" || true)" = ok ] && { ok=1; break; }
    sleep 5
  done
  [ $ok = 1 ] && say "Checked from the public side: the relay answers." || say "WARNING: no answer from the public side yet. DNS can take a few minutes; try the link from a phone off Wi-Fi."
fi

payload=$(jq -cn --arg r "https://$HOST:$PUBLIC_PORT" --arg s "$INVITE" '{relay:$r,secret:$s}' | base64 -w0 | tr '+/' '-_' | tr -d '=')
say ""
say "Commons is online at https://$HOST:$PUBLIC_PORT"
say ""
say "Your link (open the app, paste it, make your account, create a community):"
say "  https://$HOST:$PUBLIC_PORT/join#relay/$payload"
say ""
say "For friends: in the app press Invite and send them that link. It has everything in it, app download included."
say "The link above also works for them, without the community. Keep both private: they let people make accounts."
[ -n "$(ls -A dist 2>/dev/null)" ] || say "Note: dist/ is empty, so the page has no downloads. scripts/build-apps.sh fills it."
say "Stop with: deploy/go-offline.sh"
