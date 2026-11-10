#!/usr/bin/env bash
# Real-path smoke test of the Linux notifier: a private D-Bus session with a stub notification server, so nothing ever
# reaches a real desktop. Shows one note through the same code the bridge uses, has the stub "click" it, and checks that the
# note arrived as freedesktop says, with markup escaped, text off by default, and that the click started the launcher.
# usage: tools/notify-smoke.sh   (builds nothing: needs `cargo build --example notify-smoke -p commons-core` first, or CARGO_TARGET_DIR set)
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="${CARGO_TARGET_DIR:-target}/debug/examples/notify-smoke"
OUT="${1:-${TMPDIR:-/tmp}/notify-smoke}"
mkdir -p "$OUT"; rm -f "$OUT"/*.log "$OUT/launched"
printf '#!/bin/sh\ntouch "%s/launched"\n' "$OUT" > "$OUT/fake-launcher"; chmod +x "$OUT/fake-launcher"
inner() {
  python3 tools/notify-smoke.py "$OUT/server.log" > "$OUT/stub.out" &
  stub=$!
  for _ in $(seq 1 50); do grep -q ready "$OUT/stub.out" 2>/dev/null && break; sleep 0.1; done
  "$BIN" "$OUT/fake-launcher" > "$OUT/run1.log"
  SMOKE_TEXT=1 "$BIN" "$OUT/fake-launcher" > "$OUT/run2.log"
  kill $stub 2>/dev/null || true
}
export OUT BIN
dbus-run-session -- bash -c "$(declare -f inner); inner"
python3 - "$OUT" <<'P'
import json, sys, os
out = sys.argv[1]
rows = [json.loads(l) for l in open(out + '/server.log')]
assert len(rows) == 2, rows
a, b = rows
assert a['summary'] == 'mega' and a['body'] == 'New message from Ben in #music', a
assert b['summary'] == 'Ben in #music' and b['body'] == 'a &lt;b&gt;tag&lt;/b&gt; &amp; more', b
assert a['actions'][:2] == ['default', 'Open'] and a['category'] == 'im.received', a
assert os.path.exists(out + '/launched'), 'the click did not start the launcher'
for f in ('run1.log', 'run2.log'):
    assert 'open after click: "room-1"' in open(out + '/' + f).read(), f
print('notify smoke ok:', len(rows), 'notifications through a private D-Bus session')
P
