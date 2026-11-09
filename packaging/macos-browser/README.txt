mega for macOS, browser version

Same app, shown in a Brave, Chrome or Edge window instead of its own. Without one of those, it opens in Safari.

1. Unpack this file.
2. Drag Megatron.app to Applications.
3. The app is not notarized, so the first time macOS refuses to open it. Press Done (not Move to Trash), then open
   System Settings, Privacy & Security, scroll down to Security and press Open Anyway next to Megatron.
4. From then on, open it from Applications. It is still called Megatron.app there, so updates keep working.

It updates itself from the same server when it starts: the update is signed, and nothing is installed unless the
signature and the version check out. Without a connection it simply opens, as always.

To stop its background part, run:
  /Applications/Megatron.app/Contents/MacOS/launcher --stop

Its account and messages stay in ~/Library/Application Support/Megatron-browser.
