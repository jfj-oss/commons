# Joining mega

mega is a private group chat with voice rooms. A friend of yours runs it on their own computer. Messages, files and calls are encrypted on your computer before they leave it, so the server cannot read them.

Your friend gave you an **invite password**. This page gets you from that password to your first call. It takes about ten minutes.

## 1. Get the app

Open **https://horse.taile1d3f4.ts.net:10000/join** in your browser. It lists the downloads. Pick the one for your computer; the files are also at these addresses:

| Your computer | Download |
| --- | --- |
| Windows 10 or 11 | https://horse.taile1d3f4.ts.net:10000/download/Megatron-windows-browser.zip |
| Mac (Apple silicon) | https://horse.taile1d3f4.ts.net:10000/download/Megatron-macos-browser.zip |
| Linux | https://horse.taile1d3f4.ts.net:10000/download/Megatron-linux-browser.tar.gz |
| iPhone | see **iPhone** below; there is no download link |

The downloads are still named **Megatron**, the app's old name. Same thing.

mega shows itself in a browser window. **Install Brave first** (https://brave.com) if you do not have Brave, Chrome or Edge: that is what the app looks for, in that order. On Windows, Edge is already there, so it works without installing anything, but Brave is the recommended one. Firefox and Safari are not used for the window.

The downloads are not signed with Microsoft or Apple, so each system warns you once. That is expected; the steps below say what to press.

### Windows

1. Right-click the downloaded zip and choose **Extract All**.
2. In the extracted folder, double-click **install.cmd**. It needs no administrator rights. If a blue **Windows protected your PC** box appears, press **More info**, then **Run anyway**. (Your browser may also ask whether to keep the download. Keep it.)
3. Open **mega** from the Start menu. The first time, the same blue box can appear once more for the app itself: again **More info**, **Run anyway**.

The app lives in your own user folder, so it never asks for an administrator.

### Mac

1. Open the downloaded zip. Drag **Megatron.app** to **Applications**.
2. Open it. macOS says **"Megatron" Not Opened** and will not run it. Press **Done** (not Move to Trash).
3. Open **System Settings**, then **Privacy & Security**. Scroll down to **Security**. Next to the message about Megatron, press **Open Anyway**, and confirm with your Mac password or Touch ID.
4. From now on it opens normally from Applications.

Right-clicking the app and choosing Open, the old trick, no longer works on current macOS; use the steps above. The app keeps the name **Megatron.app** on purpose, so updates keep finding it. Leave it in Applications.

If you have none of Brave, Chrome or Edge, it opens in Safari as a normal tab. That works for chat; calls there have not been tried, so install Brave for calls.

### Linux

1. Unpack the download: `tar -xzf Megatron-linux-browser.tar.gz`
2. In the unpacked folder: `./install.sh`
3. Open **mega** from your app menu (or type `megatron` in a terminal).

It looks for Brave, Chrome, Chromium or Edge, including Flatpak ones. If it finds none, it opens your default browser as a normal tab; install Brave for the best result. If you also have the older mega desktop app, close it first: an account can be open in one place at a time.

### iPhone

mega on an iPhone is a private build that the owner installs through **SideStore**, not the App Store. It works, with limits you should know before you start:

* **No notifications.** Nothing arrives while the app is closed or in the background. You see messages when you open it. Nobody can ring you unless mega is open on the screen.
* **It stops working after 7 days** unless SideStore refreshes it. Open SideStore every few days (with its VPN switched on), go to **My Apps** and tap **Refresh All**. If the 7 days pass, mega simply will not open until you refresh. Nothing is lost.
* **No automatic updates.** A new version is a new file that SideStore installs over the old one. Your account and messages stay.
* **Deleting the app deletes your account's keys on the phone.** The recovery key (below) is the only way back.
* Screen sharing is not available, and calls may not work in every version of iOS.

Setting it up:

1. Install SideStore on the iPhone by following the current guide at **https://sidestore.io**. It needs a computer once, a free Apple ID, and SideStore's own VPN profile (a local one; it does not send your traffic anywhere). Ask your friend to help; this part changes with every iOS version.
2. Get the **Megatron.ipa** file from the person who runs mega, and put it somewhere the Files app can see (AirDrop, iCloud Drive, a USB cable).
3. In SideStore, open **My Apps**, tap **+**, choose the `.ipa` and tap **Install**.
4. If the phone refuses to open it, go to **Settings**, **General**, **VPN & Device Management** and trust the developer there.

Then open mega. The first screen is the same as on a computer.

## 2. Join

mega opens with three boxes.

1. **Invite password**: type the password your friend told you.
2. **Your name**: what the others will see.
3. **A password for this device**: at least 8 characters. Press **show** to see what you typed. This password locks mega on this computer, and **nobody can reset it**, not even your friend, so pick one you will remember or keep it in a password manager.

Press **Join**. mega makes your account and asks to join the community. Normally you are in within seconds. If it says you are waiting for an admin, someone who runs the community has to have mega open; it finishes by itself when they do.

The invite password is for one person and expires after some days. Do not pass it on; ask your friend for another one for someone else.

### Save your recovery key

A small card appears in the corner: **Save your recovery key**. Press **Show my key**, type your device password, and copy the key somewhere that is **not** this computer: a password manager, or a piece of paper in a drawer. Then press **I saved it**.

Why it matters: your account lives only on your devices. If this computer is lost or broken and you have no recovery key, the account is gone and nobody can give it back to you. With the key, you get the same account on a new computer and your communities let you back in. Old messages do not come back either way; they only exist on the devices that received them.

Anyone who has the key can act as you, so treat it like a password. You can see it again later under **devices and recovery** (the **more** menu).

### Next time

When you open mega again it asks for the **Passphrase for this device**. That is the device password you chose. Press **Unlock**.

## 3. The basics

### Cards and dots

mega shows **cards**: one per community, and inside a community one per room. Scroll to bring a card to the middle, then click it (or press Enter) to open it. The small coloured dots beside the cards show where you are.

When you open a text room, the cards shrink into a column of **dots** on the left and the chat fills the screen. Click a dot to switch rooms, or hover over the column to see their names. Press **Esc** to go back out to the cards. **Ctrl+K** (⌘K on a Mac) opens **jump to**, which finds any room by name.

Along the top: **people** shows who is in the community, **more** has **settings**, **devices and recovery** and the **activity log**. Admins also see **invite** and **new room**.

Two special cards: **options** (sound, typing hints, read receipts and the downloads folder; only on this device) and, for admins, **server settings**.

### Text rooms

Type and press **Enter**; **Shift+Enter** makes a new line. The paper clip attaches a file. Hover over a message for reply and reaction buttons. The card for a room shows its last line and how many messages you have not read; **@** marks a mention of you.

You see messages from the moment you joined. What was said before stays with the people who were there; that is how the encryption works, not a fault.

**Notifications (Windows, Mac and Linux).** Your own computer tells you about new messages, even after you close the window, as long as mega keeps running in the background (it does, until you quit it or restart the computer). Nothing goes through a server for this. By default a notification says who wrote and in which room, not what; **options** (or Settings) can show the text, send only **@mentions and replies**, or turn them off. To mute one room, right-click its dot or card (hold it on a touch screen): an hour, eight hours, until you unmute, or mentions only. Do Not Disturb on your computer is respected.

### Voice rooms

Voice rooms have their own cards, below the text rooms, with the people in the call on them. Press **join** on the card (or click the voice dot while in a chat) to join its call. Nothing else joins a call: scrolling past or resting on a card never does.

The first time, Brave asks to use your **microphone**. Allow it. If you refuse, you are in the call muted and can still listen.

While you are in a call, a small panel (the dock) sits at the bottom with: **Mute**, **Deafen**, **Share your screen**, **Play a video into the call**, the camera, **Call settings** (microphone, speakers, camera, noise suppression, push to talk), a bell, and **Leave the call**. The padlock on the dock says what the call server can and cannot see. Right-click a person (long press on a phone) to change their volume for you only.

You can keep reading and writing in text rooms while you are in a call.

### Ringing

While in a voice call, the **bell** in the dock rings everyone in the community who is not in the call. To ring one person, open **people** and press the bell next to their name.

Being rung: a card appears with **join** and **decline**. It rings for 30 seconds. mega plays a sound unless you turned sounds off in **options**. If the mega window is behind other windows, press **notify me when away** on the card once, and from then on a system notification shows too. Nobody whose mega is closed can be rung, and nothing arrives later: a ring is live or nothing.

### Camera and sharing a screen

The camera button in the dock turns your camera on. **Share your screen** opens your browser's own picker: a whole screen, a window or a tab. Everyone's cameras and shares appear as tiles in a stack on the left (across the top on a phone). Drag a tile to reorder the stack for everyone; **hide** hides the stack for you only.

In **Call settings** you can choose the camera quality (**720p, 30 fps** or **1080p, 30 fps**) and what a screen share is tuned for: **Sharp text**, **Smooth motion** or **Fast motion**. People watching pick their own picture quality under each tile (**Quality: auto**, High, Medium, Low) and can go **Fullscreen**. To make the cameras and screens bigger, drag the right edge of their column (on a phone, the lower edge of the strip); double-click the edge to put it back. On a Mac, sharing a screen also needs **Screen Recording** allowed for Brave in **System Settings**, **Privacy & Security**.

**Play a video into the call** plays a video file from your computer to everyone, or music, or the sound of another browser tab (**Sound from this computer**, then **A browser tab**; tick **Share tab audio** in the picker). On Linux it can also take the sound of one app that is playing.

### Large files

Files over about 24 MB are sent in pieces. A **Transfers** panel appears with a progress bar and **Pause** and **Cancel** buttons. You can pause, close mega, come back the next day and press **continue**: an upload asks you to pick the same file again and sends only what is missing; a download carries on from the last saved piece. A dropped connection pauses by itself and retries.

Downloads of large files ask where to save them. The usual place is set in **options**, **Large-file downloads folder**. Large files stay on the server for a limited time; the file's line in the chat says how long.

### Updating

mega updates itself from your friend's server every time it starts, and only installs a version your friend has signed. Afterwards a bar says **Updated to** the new version. If it has been running for days, a bar may say a newer version is available, with **Update now**; or press **Check for updates** in **settings**. Updates never happen during a call. On an iPhone, see above: a new version comes through SideStore.

If the bar says the server asks everyone to use a newer version, please update soon: old versions may not understand new kinds of messages.

## 4. When something goes wrong

| What you see | What it means | What to do |
| --- | --- | --- |
| **No invite goes with that password. Check it, ask for it again, or it may have expired or been used up.** | The password is mistyped, was already used, or has expired. | Check for typos (it is case-sensitive). Otherwise ask your friend for a new one. |
| **Too many tries just now. Wait a few minutes and try again.** | The server allows only a few wrong guesses in a row, for everyone. | Wait ten minutes. |
| **could not reach the server** when joining, or **Cannot reach the relay, retrying** at the top later | The server is off or unreachable. It runs on your friend's computer, so it is only there while that computer is on and online. | Ask your friend. Messages you write are kept and sent when it is back. |
| **Microphone permission was denied** | Brave or your system is blocking the microphone for mega. | In Brave, click the icon left of the address (or the app's title bar) and allow the microphone. On a Mac also check **System Settings**, **Privacy & Security**, **Microphone**; on Windows, **Settings**, **Privacy**, **Microphone**. Then leave and join the call again. |
| **No microphone was found** | No input device. | Plug one in, or pick another under **Call settings**. |
| **Could not reach the call server** | The call could not get through. | Try again. A VPN or a strict work network is the usual cause. |
| **Reconnecting…** in the dock | Your connection dropped. | Wait. mega rejoins by itself and keeps trying for ten minutes. |
| **this account is already open in another Megatron window or process; close that one first**, or **Close the mega desktop app first** | The same account is open twice on this computer. | Close the other one. If no other window is open, the background part is still running: wait a minute and try again, or restart the computer. (The README in the download has a `--stop` command for it.) |
| **mega could not start. See … bridge.log** | The background part did not start. | Open it again. If it keeps happening, send your friend the `bridge.log` file it names. |
| Waiting at the door of a community for a long time | Nobody who can let you in has mega open. | Ask them to open it. |
| Wrong device password | mega cannot open your data without it, and nobody can reset it. | Try again. If it is really lost, ask your friend for help: with your recovery key, mega can be set up again on this computer as a new device for your account. Old messages do not come back. |
| Choppy calls or screen share | Everything goes through your friend's home internet. | Turn cameras off, or pick a lower quality under the tile. |

**One account, one device at a time.** Your account's keys are on this device. Do not type the invite password again on another computer: that would make a second, separate account (and the password is usually for one use anyway). To use the same account on a second computer or phone, open mega there and choose **This is a new device for my account**; it shows a code, which you approve on the device you already use under **devices and recovery**, **Add a device**. Old messages stay where they were received.

## 5. Privacy, honestly

* Your messages, files, names of rooms and communities, and the sound and video of calls are encrypted end to end. The server stores and forwards encrypted blocks it cannot open.
* The server **can** see some things: that your account exists, which devices belong to it, **which groups you are in and who else is in them**, when you send something and how big it is, when you join and leave a call and who is talking at a given moment, and your IP address. It does not know what any of it says. The person who runs the server could keep this information or lose it to someone who breaks in.
* The people in a room can read everything in it, keep copies, and forward them. Deleting a message asks their apps to hide it; it cannot take copies back. Encryption protects you from the server, not from a friend.
* Someone who gets into your unlocked computer, or installs spyware on it, can read what you can read.
* If you lose every device and the recovery key, the account is gone for good.
* This is software made by a friend for a small group of friends. It uses well-known building blocks, but **nobody outside has checked it**. Treat it as much more private than a normal chat app, not as proven.

The complete list of what the server sees is in [THREAT-MODEL.md](THREAT-MODEL.md).
