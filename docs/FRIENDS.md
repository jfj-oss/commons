# Joining Commons

Commons is a private group chat. Messages, files and calls are encrypted on your computer, and the server cannot read them. Your friend who runs it sent you a link.

## Get in

1. **Open the link** in your browser. The page has the app for your computer. Download it.
2. **Start the app.**
   * **Windows:** run the installer (`Commons-windows-x64-setup.exe`); it needs no administrator rights. If a blue "Windows protected your PC" box appears, press **More info**, then **Run anyway**.
   * **Mac** (Apple silicon): open the `.dmg`, drag Commons to Applications. The first time, right-click Commons and choose **Open**, then **Open** again. If macOS still refuses, open System Settings, Privacy & Security, and press **Open Anyway**.
   * **Linux:** make the file executable (right-click, Properties, Permissions; or `chmod +x Commons-linux-x86_64.AppImage`) and run it. If it complains about FUSE, install `libfuse2`, or run it with `--appimage-extract-and-run`.
3. **Copy the invite** from the page, and paste it into the first box in the app.
4. **Pick a name and a passphrase.** The passphrase protects your keys on this computer. Nobody can reset it.
5. **Save the recovery key** the app shows you, somewhere that is not this computer. It is the only way back if you lose your devices. It does not bring old messages back.

If the link was for a community, you are now waiting at its door, and you are let in by itself within a few seconds, whenever someone who can let you in has the app open.

## Calls

Press **Call** in a room, then **Join voice**.

* **Linux:** the app window cannot do calls, so the app asks you to **Open this call in my browser**. Use Chrome, Brave, Edge or Firefox. Allow the microphone when asked. The call runs in that tab, with the same encryption; close the tab to leave.
* **Windows and Mac:** the call should run in the app. If the microphone or screen share does not work there, use **Open in my browser** in the same panel. On a Mac, screen sharing also needs Screen Recording allowed for your browser or Commons, in System Settings, Privacy & Security.

**Playing a game with its own voice chat?** Settings, Game keys: add the game and press its voice key, and while you hold it your friends here do not hear you. Details and what works on your system: [GAME-KEYS.md](GAME-KEYS.md). On a Mac it needs Input Monitoring, and it asks.

## When something is wrong

| You see | What it means | What to do |
| --- | --- | --- |
| "Could not reach the relay…" or "cannot reach the relay, retrying" | The server is off, or your network is blocking it. It runs on your friend's PC, so it is only there while their PC is on | Ask them whether it is online. Messages you write are kept and sent later |
| "That is not this relay's invite secret", or "only takes new accounts with its invite secret" | The link is old or cut short | Ask for a new link, and paste all of it |
| "That invite is no longer valid" | It was used up, revoked, or it expired | Ask for a new one |
| Waiting at the door for a long time | Nobody who can let you in has the app open | Ask them to open it |
| "Could not reach the call server" | The call could not get through | Try again; a VPN or a strict work network can be the cause |
| Choppy calls or screen share | Everything goes through your friend's home internet | Ask people to turn off their cameras, or pick a lower quality in the call settings |

To use a relay that moved: Settings, Relay address. On the first screen the same box is under **Relay server**. Normally the link does this for you.

## Good to know

* Keep your link to yourself. It lets people make an account on your friend's server.
* You see messages from the moment you joined. What was said before stays with the people who were already there; that is part of the encryption, not a fault.
* A new device of yours: on the new one choose "This is a new device for my account", copy the code it shows, and paste it under Devices on the old one. Old messages stay on the old device.
* The window title shows how many messages are unread, and a soft sound plays for new ones while Commons is in the background. Settings, Sound for new messages: all, only mentions and replies, or off.
* To compare that you are really talking to someone, open People and press the padlock.
* **Updates:** the app checks your friend's server for new versions and asks you first: **Install now** or **Later**. It never updates during a call; it asks again when the call has ended. The update is checked against a signature before it is installed, then the app restarts and asks for your passphrase. Keep the app where you first put it (on a Mac, in Applications), so the update can replace it. If a bar says the server asks everyone to use a newer version, please update.
* This is software made for a small group of friends. Nobody outside has audited it.
