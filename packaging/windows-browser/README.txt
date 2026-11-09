mega for Windows, browser version

Same app, shown in a Brave, Chrome or Edge window instead of its own. Edge is already on every Windows 10 and 11.

1. Unpack this file (right-click, Extract All).
2. Run install.cmd. It needs no administrator rights.
3. Open mega from the Start menu.

It updates itself from the same server when it starts: the update is signed, and nothing is installed unless the
signature and the version check out. Without a connection it simply opens, as always.

To stop its background part, in the folder it was installed to (%LOCALAPPDATA%\Megatron-browser) run:
  launcher.exe --stop

Its account and messages stay in %LOCALAPPDATA%\Megatron-browser\data.
