#!/usr/bin/env bash
# Megatron in your browser window. The app's own core (encryption, sync) runs here as a small local program and
# Brave, Chrome, Chromium or Edge draws the window: on Linux that is much smoother than the desktop app's WebKitGTK.
# It only listens on 127.0.0.1 and refuses other websites. Same account folder as the desktop app, so you can switch
# between them (one at a time).   megatron          open it
#                                  megatron --stop   stop the background part
set -u
D="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"
PORT="${MEGATRON_PORT:-7420}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/app.commons.chat"
RELAY="$(tr -d '\n' < "$D/relay.txt")"
URL="http://127.0.0.1:$PORT/"
say() { command -v notify-send >/dev/null && notify-send "mega" "$1"; echo "$1" >&2; }
up() { (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; }
start_bridge() {
  mkdir -p "$DATA"
  COMMONS_DATA="$DATA" COMMONS_RELAY="$RELAY" setsid "$D/bridge" "$PORT" "$D/ui" >"$D/bridge.log" 2>&1 < /dev/null &
  echo $! > "$D/.pid"
  for _ in $(seq 1 50); do up && break; sleep 0.1; done
}
# A failed start after an update: put the previous bridge and ui/ back, so a bad build cannot leave nothing working.
restore_previous() {
  [ -d "$D/.update-previous" ] || return 1
  [ -f "$D/.pid" ] && kill "$(cat "$D/.pid")" 2>/dev/null
  rm -f "$D/.pid"
  [ -e "$D/.update-previous/ui" ] && { rm -rf "$D/ui"; mv "$D/.update-previous/ui" "$D/ui"; }
  [ -e "$D/.update-previous/bridge" ] && { rm -f "$D/bridge"; mv "$D/.update-previous/bridge" "$D/bridge"; }
  rm -rf "$D/.update-previous" "$D/.update-note.json"
  return 0
}
if [ "${1:-}" = "--stop" ]; then [ -f "$D/.pid" ] && kill "$(cat "$D/.pid")" 2>/dev/null; rm -f "$D/.pid"; exit 0; fi
if pgrep -x commons-app >/dev/null; then say "Close the mega desktop app first: an account can be open in only one place."; exit 1; fi
if ! up; then
  # Signed update before the window opens (docs/UPDATES.md). At most about five seconds: offline, or a server that does
  # not answer, only delays the window a moment. The page shows what happened on this start.
  if [ -n "$RELAY" ] && [ -x "$D/bridge" ] && command -v timeout >/dev/null; then
    if command -v flock >/dev/null; then
      exec 9>"$D/.update.lock"
      flock -n 9 && { timeout 5 "$D/bridge" --update "$RELAY" "$D" >>"$D/bridge.log" 2>&1 || true; }
      exec 9>&-
    else
      timeout 5 "$D/bridge" --update "$RELAY" "$D" >>"$D/bridge.log" 2>&1 || true
    fi
  fi
  start_bridge
  if ! up && restore_previous; then
    say "The new version did not start; going back to the one before."
    start_bridge
  fi
  up || { say "mega could not start (see $D/bridge.log)."; exit 1; }
fi
for b in brave brave-browser google-chrome google-chrome-stable chromium chromium-browser microsoft-edge microsoft-edge-stable vivaldi; do
  command -v "$b" >/dev/null && exec "$b" --app="$URL" >/dev/null 2>&1
done
if command -v flatpak >/dev/null; then
  for f in com.brave.Browser com.google.Chrome org.chromium.Chromium com.microsoft.Edge; do
    flatpak info "$f" >/dev/null 2>&1 && exec flatpak run "$f" --app="$URL" >/dev/null 2>&1
  done
fi
exec xdg-open "$URL"
