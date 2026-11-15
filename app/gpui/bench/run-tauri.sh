#!/bin/bash
# run-tauri.sh <room> [env...] -> the patched Tauri build on alice inside the headless KWin; pid in run/app.pid
D=$HOME/data/agent-tmp/mega-gpui
. $D/hl-env.sh
ROOM=$1; shift
rm -f $D/run/marks.jsonl
env COMMONS_DATA=$D/run/people/alice COMMONS_RELAY=http://127.0.0.1:18941 MEGA_GPUI_PASS=throwaway-pass-1 MEGA_GPUI_OPEN="$ROOM" \
  MEGA_BENCH_OUT=$D/run/marks.jsonl XDG_CACHE_HOME=$D/run/tauri-cache XDG_DATA_HOME=$D/run/tauri-data "$@" setsid $D/target/release/commons-app > $D/logs/tauri-app.log 2>&1 &
echo $! > $D/run/app.pid
