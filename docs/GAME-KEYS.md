# Game keys: push-to-mute

Hold your game's voice key, and your mic goes quiet in the Commons call, so friends do not hear what you tell your game team. Settings, **Game keys**: a list of games by running program, one key each, hold to mute (default) or press to toggle, plus an "any app" key. Set the same key as the game's voice key. Everything is stored in this device's encrypted state: the relay never sees it, and pairing does not pass it on.

## How it behaves

* A game's key counts only while that program runs, by process name, checked every 2 seconds. "Any app" counts during any call.
* Keys are looked at only during a call, and only the keys you set. With no call, or no rules, the helper is not running.
* The mic goes off as soon as the key is seen, and comes back 200 ms after it is let go, so the end of a word does not leak. **The start of a word can leak:** the key is seen within about 10 ms, and the mute reaches the browser a few milliseconds later, but sound already encoded is gone. Press the key a moment before you speak.
* The panel says "Muted by game key". Friends see the normal muted state and nothing about why.
* Your own mute wins: a key letting go never unmutes you. Pressing Unmute beats a toggled key. A held key stays in charge until you let go.
* If the game closes, the helper fails or the app dies, the mic goes back to what you had set. The call page and the app check on each other every few seconds, so losing the app takes up to 9 seconds to notice, and until then the mic stays as it was.
* It mutes with the same track mute as the Mute button, so end-to-end encryption is not involved at all.

## It only looks

No key is grabbed or swallowed, nothing is injected, no game process is opened, and no game memory is read. Programs are known from the process list by name: a Toolhelp snapshot on Windows, `proc_name` on macOS, `/proc` on Linux. For anti-cheat that leaves a background program reading key state and a process list, which many overlay and voice programs do too. That is a judgement from how these systems are known to work, not a promise: no anti-cheat vendor has been asked, and none of this has been run next to one.

## What works where

| Platform | How it sees keys | Hold | Toggle | Mouse side buttons | Tested |
| --- | --- | --- | --- | --- | --- |
| Linux, X11 session | X server key state | yes | yes | no | **Real**: AppImage on Ubuntu, X server on a virtual display, key pressed through the X server, real encrypted call, through the bridge to the tab |
| Linux Wayland, Wine/Proton or other X11 game | The same, through Xwayland, which knows keys only while an X11 window has focus | yes, while it has focus | yes | no | The X path is tested. **Xwayland focus behaviour untested:** it needs a person at a desktop |
| Linux Wayland, native game, user in the `input` group | Reads `/dev/input` without grabbing it | yes | yes | yes | Event parsing tested. **Real devices untested:** this user is not in that group. Joining it lets every program you run read every key, so it is your call and nothing sets it up |
| Linux Wayland, system shortcut option (portal) | `org.freedesktop.portal.GlobalShortcuts`, one approval | if the desktop reports release, which KDE's portal version 2 should | yes | no | Against a stand-in portal on a private bus only: press, release, refusal and portal loss. **The real KDE dialog is untested:** it would have opened a window on the desktop |
| Windows | `GetAsyncKeyState` polling, no hook at all | yes | yes | yes | Cross-built. The logic and the process list ran as a Windows program under Wine. **Key polling itself, and the app, never ran on Windows.** If a game runs as administrator, Commons may need to as well |
| macOS | `CGEventSourceKeyState` polling, Input Monitoring permission | yes, once allowed | yes | yes | **Compiled, and its process list run, on a Mac.** Key state and the permission prompt are untested |

The portal option warns you for a reason. As far as can be told from how KWin handles global shortcuts, the compositor consumes a key that triggers one, so the focused game may never get it. That breaks the rule that a key must not swallow game input, which is why this option is off by default. It is there for a key the game does not need, such as a separate key you also bind in the game's own settings to something harmless, and for native Wayland games where nothing else can see keys. If a hold is not reported on your desktop, use press to toggle.
