# source: environment for a client inside the private headless KWin
export WAYLAND_DISPLAY=wayland-megatest
export DBUS_SESSION_BUS_ADDRESS=$(cat $HOME/data/agent-tmp/mega-gpui/run/bus)
unset DISPLAY
