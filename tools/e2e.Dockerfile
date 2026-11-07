# A clean Ubuntu with a virtual display, for tools/packaged-e2e.mjs: nothing from this machine's own libraries, and no window on
# the desktop. Only what the test needs to reach the app; the AppImage brings the rest itself.
FROM ubuntu:24.04
RUN apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq xvfb webkit2gtk-driver ca-certificates dbus file >/dev/null && rm -rf /var/lib/apt/lists/*
