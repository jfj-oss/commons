#!/bin/bash
# Start a private headless KWin (own D-Bus session, own config dir, socket wayland-megatest). Prints its PID file.
D=$HOME/data/agent-tmp/mega-gpui/run
mkdir -p $D/kwin-config $D/kwin-cache
export XDG_CONFIG_HOME=$D/kwin-config XDG_CACHE_HOME=$D/kwin-cache
export KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1 KWIN_WAYLAND_NO_PERMISSION_CHECKS=1
unset WAYLAND_DISPLAY DISPLAY
# the inner shell writes the private bus address so test clients can join the same bus
setsid dbus-run-session -- bash -c 'echo "$DBUS_SESSION_BUS_ADDRESS" > '$D'/bus; exec kwin_wayland --virtual --socket wayland-megatest --no-lockscreen --no-global-shortcuts --width 1600 --height 1000' > $D/kwin.log 2>&1 &
echo $! > $D/kwin.pid
