#!/bin/bash
# run-gpui.sh <room> [extra env assignments...]  -> starts mega-gpui on alice inside the headless KWin; pid in run/app.pid
D=$HOME/data/agent-tmp/mega-gpui
. $D/hl-env.sh
ROOM=$1; shift
rm -f $D/run/marks.jsonl
env COMMONS_DATA=$D/run/people/alice COMMONS_RELAY=http://127.0.0.1:18941 MEGA_GPUI_PASS=throwaway-pass-1 MEGA_GPUI_OPEN="$ROOM" \
  MEGA_BENCH_OUT=$D/run/marks.jsonl MEGA_GPUI_NOTIFY_FILE=$D/run/notes.jsonl "$@" setsid $D/target/release/mega-gpui > $D/logs/app.log 2>&1 &
echo $! > $D/run/app.pid
