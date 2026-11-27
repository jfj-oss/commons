#!/usr/bin/env bash
# Shows the problem reports members' apps sent to this relay (POST /v1/diag; crates/core/src/diag.rs), newest last.
#   deploy/diag-reports.sh            the last 10, short
#   deploy/diag-reports.sh 3 --full   the last 3, with each one's call log
# Reports hold the error, the app version, the OS and the call's own log; never messages or keys.
set -euo pipefail
N=${1:-10}; FULL=${2:-}
DB=${COMMONS_RELAY_DB:-$HOME/.local/share/commons-host/relay-data/relay.db}
[ -f "$DB" ] || { echo "no relay database at $DB (set COMMONS_RELAY_DB)"; exit 1; }
command -v sqlite3 >/dev/null && command -v jq >/dev/null || { echo "needs sqlite3 and jq"; exit 1; }
n=$(sqlite3 "file:$DB?mode=ro" "SELECT COUNT(*) FROM diag_reports" 2>/dev/null || echo 0)
echo "$n report(s) stored"
sqlite3 -json "file:$DB?mode=ro" "SELECT * FROM (SELECT id, at, account, report FROM diag_reports ORDER BY id DESC LIMIT $((N))) ORDER BY id" 2>/dev/null | jq -r --arg full "$FULL" '.[]? | (.report | fromjson) as $r |
  "\n#\(.id)  \(.at | strftime("%Y-%m-%d %H:%M UTC"))  account \(.account[0:12])  mega \($r.version // "?") on \($r.os // "?") \($r.os_version // "")",
  "  \($r.error.kind // "?"): \($r.error.msg // $r.error // "?")",
  "  server \($r.server // "-")  attempts \($r.attempt // 0)  mic \($r.mic.name // "default")\(if $r.mic.system_muted then " (muted in system)" else "" end)",
  (if $full != "" then ($r.events // [] | map("    · " + .) | .[]) else empty end)'
