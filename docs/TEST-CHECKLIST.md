# Hands-on test checklist: mega 0.3.3 with real people, real devices, real internet

The automated tests run on one Linux machine, on loopback, with synthetic microphones and screens. They have never run on a
real phone, a real Windows PC or a real Mac, over a real internet path, or with a real microphone. This checklist covers
exactly that. It is written for the owner (who runs the relay) and two or three friends, and takes about an hour if the
optional checks at the end are skipped. The most valuable checks come first: if time runs out, what was skipped matters least.

Fill in the table at the end as you go, and send it to the owner. Where a check says **report**, write down the exact text
on screen, the version, the system and the browser.

## People and devices

| Name used below | Who | Device | Notes |
| --- | --- | --- | --- |
| **Owner** | runs the relay | Linux PC, the one the relay runs on | also has the admin role |
| **A** | friend | Windows 10 or 11, Brave installed (Edge is the fallback) | fresh install |
| **B** | friend | Mac with Apple silicon, Brave installed | fresh install |
| **P** | the Owner or a friend | iPhone with SideStore and Megatron.ipa already installed | see docs/IOS-SIDESTORE.md |
| **Old** | anyone | a second computer or user account still running the **previous** version (0.3.2 or older) | for the compatibility check; skip if nobody has one |

Two people are enough for most checks (Owner and A); three make the call checks meaningful. Friends should not be on the
owner's home network, or the internet path is not real. One friend on phone data (hotspot) is ideal for the wifi-drop check.

## Before you start (Owner, 5 minutes)

1. The relay is online the usual way (`deploy/go-online.sh`), LiveKit with it. `https://horse.taile1d3f4.ts.net:10000/join`
   opens from outside your network and lists **Megatron-windows-browser.zip**, **Megatron-macos-browser.zip** and
   **Megatron-linux-browser.tar.gz** at 0.3.3 (the version is in `update/latest.json` next to them).
2. Have a community with at least one text room and one voice room. Add a second voice room; several checks need it.
3. Create a **password invite** for each friend: in the community, **invite**, then **Create invite** (one use, 7 days is
   the default; one password per friend, and check each one says **Good.** under the box). Tell each friend their password
   by some other channel. Also keep **one old-style invite link**, if you still have one that is not expired; otherwise
   skip check B.
4. Have a file of 100 MB or more ready to send (anything over 24 MB goes through the large-file path). Bigger is
   better for the pause check: 300 MB gives time to pull the plug.
5. Have some music ready: a web page playing music in a Brave tab, and on the Linux PC a music app playing through PipeWire.
6. Agree how friends report: a text room in mega is fine, once chat works; before that, whatever you normally use.

Never paste the invite password or any secret into the results table.

## A. Fresh install and password join (A and B, 10 minutes) — the most important check

**Who:** A on Windows, B on Mac, each following docs/FRIENDS.md with no other help. The Owner watches the time and the
questions they ask, and writes down every place the guide was unclear.

**Steps and what should happen**

1. Download from the `/join` page. **Report:** which file the page offered first (it should sort your own system to the top), and
   how long the download took.
2. Install.
   * Windows: **Extract All**, **install.cmd**. Expect a blue **Windows protected your PC** box with **More info**, **Run anyway**;
     expect Brave or Edge to warn about the download too. **Report:** every warning seen, word for word, and whether the Start menu
     has **mega**. Then open **mega** from the Start menu. **Report:** whether the blue box appears again for `launcher.exe`.
   * Mac: drag **Megatron.app** to Applications, open it. Expect **"Megatron" Not Opened**, press **Done**, then **System Settings**,
     **Privacy & Security**, **Open Anyway**. **Report:** the exact dialog texts and buttons, and whether a second prompt asked for the
     Mac password. If right-click, **Open** worked instead, say so (it is not supposed to on current macOS).
3. mega opens in Brave as its own window (no address bar). **Report:** which browser opened it (Brave, Chrome, Edge, Safari), and how
   many seconds from click to the first screen. Over about five seconds means the update check is slow: the relay is far away or
   the connection to it is poor.
4. The first screen has **Invite password**, **Your name**, **A password for this device** (with a **show** button) and **Join**.
   Type the password, a name, a device password of 8 or more characters. Press **Join**.
5. You should be inside the community within seconds, looking at its cards. If instead a note says **Asked to join … An admin
   has to let you in**, the Owner opens mega (or already has it open) and it finishes by itself. **Report:** how long it took, and
   whether the Owner had to do anything.
6. A card in the corner says **Save your recovery key**, **Show my key**. Leave it for check C.
7. Open a text room, send one message. The Owner replies. **Report:** round trip in seconds.

**Report if it goes wrong:** the exact error under the Join button. Known ones: **No invite goes with that password…** (typo,
expired or used up: the Owner makes a new one), **Too many tries just now…** (wait ten minutes), **could not reach the server…**
(the relay is not reachable from that network). Anything else is a bug.

## B. An old invite link (A or B, 3 minutes)

**Who:** whoever finished A first, with an **old invite link** of the Owner's for a **different** community. The mega invite
dialog only makes passwords now, so this needs a link made with an earlier version that has not expired. If the Owner has none,
skip this check and say so.

1. In the community cards, open **more**, **join with an invite password**. In the box, paste the **link** instead of a password,
   press **Join**. Expect **Request sent**, then the new community appears. A link in the password box is accepted on purpose.
   The dialog also has **I have an invite link instead**, which opens the old link box; try that too.
2. Optional, on a spare computer: paste a link into **Invite password** on the very first screen (before any account). It should
   create the account and join, exactly as the old guide said.

**Report:** whether the link was accepted, the message shown, and whether the community appeared.

## C. The recovery key reminder (A and B, 2 minutes)

1. The corner card **Save your recovery key** is still there after a reload and after reopening mega. Reload the window
   (**Ctrl+R** / **⌘R** in the mega window). **Report:** does the card come back after unlocking?
2. Press **Show my key**, type the device password, see a long key and a **Copy** button. Press **Later**: the card stays.
3. Press **Show my key** again, then **I saved it**. The card goes and does not come back after a reload.
4. **more**, **devices and recovery**, **Show my recovery key** shows the same key again after the passphrase.

**Report:** anything that differs, and whether a wrong passphrase gives a clear message.

## D. Messages both ways, including with an older version (everyone, 5 minutes)

1. Everyone in the same text room: each person sends a message, a reply (hover a message, reply), a reaction, and a small picture.
   Everyone sees everyone's within a few seconds. **Report:** anything missing or out of order, and the delay.
2. **Old** (previous version) does the same. New versions and the old one must read each other's messages, replies, reactions
   and pictures. Known and acceptable: the old version does not see live things from new apps (tile drags, wall strokes, rings)
   and writes **someone has a newer version** once in its **activity log**. **Report:** any message that the old version could not
   show, or that the new ones could not show from it.
3. Switch rooms with the dots on the left and come back: the unread counts on the cards and dots are right. **Report** a wrong count.

## E. Voice call quality and latency over the real internet (3 people, 8 minutes)

1. Everyone presses **join** on the same voice room's card. Brave asks for the microphone; allow. The dock at the bottom says
   **Voice connected** with a green dot, and the room's card lists everyone with a face that lights up when they talk.
2. Talk for two minutes. **Report, per person:** can you hear everyone; is anyone robotic or cutting out; does the speaking
   indicator follow the talker. Try **Mute** and **Deafen**, then **Call settings**: pick another microphone or speakers if you have
   them, toggle **Noise suppression and echo cancellation**.
3. Latency: one person counts "one, two, three" and another repeats each number as soon as they hear it; a third times the echo
   with a phone stopwatch. Over the internet 0.3 to 0.6 s is normal; **report** the figure and anything over a second.
4. The connection path: **the app does not show which path the media takes.** To see it, press **Ctrl+N** (**⌘N**) in the mega
   window: that opens a normal window of the same browser profile; type `brave://webrtc-internals` (or `chrome://webrtc-internals`,
   `edge://webrtc-internals`). Find the active candidate pair; **relay** means the call goes through the relay's TURN server over
   TLS, which is what docs/DEPLOY.md expects for everyone outside the owner's Tailscale network. **Report:** the candidate type
   (host / srflx / relay) and the protocol. (Whether Ctrl+N works from an app window on every system is not confirmed; if it does
   not, skip this and say so.)
5. Press the padlock on the dock: a small note says **What the call server can see**, with **AES-GCM**, a backend name and a key
   epoch. **Report** if it says **NOT encrypted** anywhere; that is a bug, and the call should be left at once.
6. One person leaves and rejoins. The others hear them again within a few seconds. **Report** if the rejoined person is silent.

**Report if it goes wrong:** the dock's exact message. Known: **Could not reach the call server…** (ports, VPN, strict network),
**Microphone permission was denied…** (see check F). A call that says **Connected** but carries no sound for more than ten seconds
should rebuild itself; **report** if it stays silent.

## F. Microphone permission refused, then allowed (one person, 2 minutes)

1. On a device that has not yet joined a call, join and **refuse** the microphone prompt. Expect to be in the call muted, with the
   message **Microphone permission was denied. Allow microphone access for mega in your system or browser settings, then try again.
   You are in the call muted and can still listen.** You should still hear the others.
2. Allow the microphone again in the browser (the icon left of the title) and rejoin. You are heard.

**Report:** the exact text, and whether listening worked while muted.

## G. Reconnect when wifi drops (one friend on a laptop or phone hotspot, 4 minutes)

1. While in the call and talking, the friend switches wifi off for 15 seconds, then on again. Expect the dock to say
   **Reconnecting…** and then **Voice connected** again by itself, usually within 10 to 30 seconds of the network coming back.
   The others see the person disappear from the card and come back. **Report:** seconds to recover, and whether sound came back in
   both directions without anyone pressing anything.
2. Repeat with wifi off for about two minutes. The app keeps trying for ten minutes; it should still recover. If it gives up it says
   **Could not get back into the call after 10 minutes**. **Report** which happened.
3. Also drop the network while **not** in a call: the top of the window shows **Cannot reach the relay, retrying**. Type a message
   while offline; it is sent when the network returns. **Report** if it is lost.

## H. A call on the iPhone (P, 5 minutes)

1. Open mega on the iPhone (unlock with the passphrase). Join the voice room from its card. Allow the microphone.
2. **Report:** does the call connect at all? The iOS web view may refuse encrypted calls: the app then says so and does **not** fall
   back to an unencrypted call. Write down the exact text. This is the one platform where calls are genuinely unverified.
3. If it connects: talk both ways, check the speaking faces, check the dock (a slim bar at the bottom on a phone), **Mute**, **Leave**.
   Press the home button and come back after a minute: expect the call to have dropped or paused (iOS suspends the app) and to say
   **Reconnecting…** or to need a fresh **join**. **Report** what happened.
4. Camera from the phone: press the camera button. **Report** whether the others see it and at what quality.
5. Screen share is expected to be unavailable on the phone (the button is greyed out, **This browser or webview cannot share a screen**).

## I. Ringing (everyone, 6 minutes)

1. Owner is alone in a voice room. A and B are in a text room with mega visible. Owner presses the **bell** in the dock
   (**Ring everyone in this community who is not in the call**). Expect, on A and B within a second or two: a card **ringing the room**
   with the Owner's name, **decline** and **join**, and a two-note sound every few seconds. A presses **join** and lands in the call;
   B presses **decline**. The card disappears on both. The Owner sees a toast **ringing everyone in <community> who is not in the call.
   People with mega closed are not reached.** **Report:** delay, and whether the sound
   played (it is silent if **options**, **Sound for new messages** is **Off**, and on a page that was never clicked).
2. Ring one person: Owner opens **people** and presses the bell next to B's name. Only B gets a card (**calling you**). It ends when
   B joins or after 30 seconds.
3. **Backgrounded window:** B presses **notify me when away** on a ring card once (Brave asks to allow notifications; allow). Then B
   puts another window over mega and the Owner rings again. Expect a system notification **Owner is calling in <room>** plus the
   sound. **Report** whether it arrived, and how late. A long-hidden window may be slowed down by the browser and miss a 30-second
   ring; if that happens, say how long the window had been hidden.
4. **Closed app:** B quits mega entirely (close the window; on Windows and Mac the background part may keep running, that is fine).
   Owner rings. Expect **nothing** on B, now or later: a ring is live or nothing. B reopens mega: there must be no stale ring card.
   **Report** any card that appears late.
5. **Old** version: a ring from a new app is ignored there, with one **newer version** note in its activity log. **Report** if it
   shows anything else.

## J. Camera and screen share at each quality (two people, 6 minutes)

Sharer: one computer. Viewer: another, preferably over the slowest connection present.

1. **Camera:** press the camera button. In **Call settings**, **Camera quality**: try **720p, 30 fps**, then **1080p, 30 fps**. The
   viewer sees a tile named `<name> · camera`. **Report** sharpness, smoothness and whether the tile re-appeared after the quality change.
2. **Screen share:** **Share your screen**. The browser's own picker appears (screen, window, tab). Share a window with text, then one
   with a moving video. In **Call settings**, **Share as**: try all three, **Sharp text (up to 1440p, 30 fps)**, **Smooth motion
   (1080p, 30 fps)**, **Fast motion (1080p, 60 fps)**; stop and start the share after each change. **Report** readability of small text
   and smoothness of motion for each.
3. **Viewer's choice:** under the tile, **Quality: auto**, then **High**, **Medium**, **Low**, then **Fullscreen**. **Report** whether
   the picture visibly changes and how fast.
4. **Audio with the share:** tick **Include audio when sharing a screen or window, where the system allows it**, then share a tab
   that plays sound (tick the browser's **Share tab audio** box). Expected: tab audio works on Windows, Mac and Linux; whole-screen
   audio only on Windows. **Report** what carried sound and what did not.
5. Mac sharer: the first share needs **Screen Recording** allowed for Brave in **System Settings**, **Privacy & Security**; Brave then
   needs a restart. **Report** the exact prompts.
6. Windows and Mac: the launcher opens Brave with its **own profile**, so permissions (microphone, notifications, screen) are asked
   again even if you already allowed them in your normal Brave. **Report** anything confusing about that.

## K. Dragging the tile stack (two people, 2 minutes)

With two or more tiles showing (two cameras, or a camera and a share), one person drags a tile up or down in the stack on the left.
Expect: the order changes **for everyone in the call**, live, with a short slide animation; while held, the others see the tile marked
with the holder's name. Then the other person drags one back. On a phone, a long press (about a third of a second) starts the drag.
Press **hide** above the stack: it hides the stack for you only.

**Report:** whether both saw the same order within a second, any tile that jumped back, and whether a drag ever joined or left
anything by accident.

## L. Drawing on a wall and on a card (two people, 5 minutes)

1. **In a room:** both open the same text room. One presses **Draw** at the top (hint: **Draw on this room's backdrop, for everyone**).
   A toolbar appears with glyphs, **erase**, two colour swatches, **done** (and for the owner **lock** and **clear**), tagged
   **everyone sees this**. Draw a few strokes on the backdrop. The other person sees them within about a second, and can draw too.
   **Report** the delay, and whether strokes land on the words (they should flow around them).
2. **Owner only:** press **lock**. The other person's **Draw** button disappears (if they were drawing, a toast says **The owner locked
   the wall**). Press **lock** again to unlock. Press **clear**, then **clear for everyone?** to confirm: the wall is empty for everyone.
3. **On a card:** go back to the cards (**Esc**). With a room card in the middle, press the **draw** chip at the top; it changes to
   **drawing**. Draw on the card. The other person, resting on the same card, sees it; the room's wall and its card show the same
   drawing. Then do the same on the **community** card on the home screen (it has a wall of its own).
4. **Old** version: sees none of this, with one **newer version** note. **Report** if it draws anything.

## M. Sound from this computer into the call: a browser tab, and the Linux app route (Owner and one listener, 5 minutes)

1. In the call, press **Play a video into the call**, then under **Sound from this computer**, **A browser tab**. Brave's picker opens on
   the **tab** list; pick the tab playing music and tick **Share tab audio**. Expect a tile **Audio from a browser tab** for everyone
   and the music audible to the listener in stereo, while the sharer keeps hearing it normally. **Report** quality (it is sent at music
   quality, 192 kbit/s), delay, and whether your own voice still works alongside.
2. Pick a tab that is silent: after five seconds the title should say **No sound is coming from that tab…**. Pick a window instead of a
   tab: expect **That was a window or a screen, not a browser tab…**. Press **Stop** in the player bar to end it. Also end one with the
   browser's own **Stop sharing** bar: mega should notice and stop.
3. **Linux app route (Owner):** with a music app playing through PipeWire, press **An app on this computer**. Expect a list of apps
   that are playing; pick one. The listener hears it; the title follows the track. Pause the app: after ten seconds the share stops
   itself. **Report** whether the app appeared in the list (browsers are hidden there on purpose), and whether any sound was doubled
   or echoed on the Owner's side.

## N. Music making the backdrop pulse (two people, 2 minutes)

The chat backdrop (the moving pattern behind a text room) is meant to pulse to music that is playing into the call you are in.

1. While music plays into the call (check M), open a **text room** and watch its backdrop. **Report** whether it visibly pulses with the
   beat. It never runs with the system's reduced-motion setting on, nor when the owner set that room's **backdrop** slider to **off**
   (owner: right-click the room's dot to open **colours**; the slider is under **backdrop**). Check both are not the case first.
2. **What to report if it does nothing:** that is likely. Reading the code, the backdrop only listens to the call whose room is the
   **same room** as the open chat, and calls happen in voice rooms while backdrops only exist in text rooms, so the live path may never
   fire. Write down exactly what you saw so the owner can decide whether to change it.

## O. A large file: pause, a dropped connection, resume (Owner sends, A receives, 6 minutes)

1. Owner attaches the big file (paper clip). A **Transfers** panel appears with a progress bar, a speed, **Pause** and **Cancel**.
2. Press **Pause** midway: the note says **Pausing after this part…**, then **Paused. Nothing more is sent or fetched.** Wait ten seconds,
   press **Resume**: it carries on from where it was, not from zero. **Report** the sizes shown before and after.
3. Pull the plug: switch the Owner's wifi off for 20 seconds during the upload. The panel shows a retry countdown; when the network
   is back it continues by itself. **Report** whether it needed any clicks.
4. Reload the window (**Ctrl+R**) mid-upload. The Transfers panel shows the file as **Paused** or **Stopped** with how much is **safe on
   the relay**, the note **Press continue, then choose the same file again…**, and a play button (**Choose the file to continue**). Press
   it and pick the **same** file: only the missing parts are sent. Pick a **different** file on purpose once: expect **That is X, not Y.**
   or a size mismatch message, and nothing sent.
5. When it says **<name> sent.**, the file appears in the room with its size and how long it stays on the relay. A presses **Download**,
   chooses a folder, and the same panel shows the download. A presses **Pause**, reloads, presses the play button (**Continue**): it
   carries on from the last saved piece. **Report** the final size and whether the file opens.
6. **Report** any **Upload of … stopped: …** or **Download stopped: …** toast, word for word.

## P. Updating from one version to the next (Owner publishes, everyone else updates, 5 minutes)

Do this **last**, and only if the Owner has decided to publish the next version; it is a real release to every friend.

1. Owner publishes 0.3.4 (or whatever is next) with `scripts/publish-update.sh`, with a one-line note.
2. **Running apps:** within about four hours, or at once via **settings**, **Check for updates**: the button becomes **Update to 0.3.4**.
   Press it: **Downloading the update…**, then **Installed 0.3.4. Restarting mega…**, the page reloads, and asks for the device
   passphrase. After unlocking a bar says **Updated to 0.3.4**. **Report** the seconds it took and whether anything was lost (open
   room, draft text).
3. **During a call:** one person tries to update while in a call: expect **Leave the call first, then update.** and nothing installed.
4. **Closed apps:** a friend who had quit opens mega again: the update installs before the window opens (up to about five seconds), and the
   bar says **Updated to 0.3.4** right away. **Report** the delay from click to window.
5. **Mac:** after the update, Gatekeeper must not complain again (the app re-signs itself). **Report** any dialog.
6. **Windows:** `launcher.exe` and `bridge.exe` replace themselves while running. **Report** any error box, and check the Start menu entry
   still works.
7. **iPhone:** no in-app update is offered (expected). A new `.ipa` goes through SideStore, **My Apps**, install over the old one; the
   account and messages must still be there afterwards. **Report** if the unlock screen or the rooms are gone.
8. **Old** version: sees the bar **This server asks everyone to use mega X or newer** only if the Owner set `--min-version`; otherwise an
   ordinary offer. **Report** what it showed.

## Q. The admin panel (Owner and one admin, 4 minutes)

1. Owner opens the **server settings** card in the community (it exists only for the owner and admins). Sections: **server name**,
   **rooms**, **members and roles**, **banned**, **invites**, **hand over ownership**.
2. **server name:** change it, **save**. Everyone sees the new name on their cards within seconds (**Renamed to …**).
3. **rooms:** **add room** as **voice** and as **text**; **rename** one; **delete** one (confirm **Delete room**). Everyone's cards and dots
   follow. The last room cannot be deleted; the button is not there.
4. **members and roles:** **make admin** on A, then look from A's side: the **server settings** card appears for A, without the
   **hand over ownership** section and without the role buttons. **make member** again: it disappears for A.
5. **remove and ban** a throwaway account if you have one (**Remove and ban** to confirm). Expect **… is being removed. It finishes as soon
   as an admin has mega open.** The banned name appears under **banned**; **unban** it.
6. **invites:** the password invites you made are listed with uses and expiry; **revoke** one and have a friend try it: expect **No invite
   goes with that password…**.
7. Do **not** test **hand over ownership** unless you mean it: it cannot be taken back.

**Report** any change that one person saw and another did not after a minute.

## R. The iPhone's gestures and scrolling (P, 5 minutes)

These exist only on a phone-sized touch screen; a computer is unchanged.

1. In a chat with a few hundred messages, read back with the finger and let it fly. Expect it to stay smooth; the dimming and shrinking
   of the rows away from the middle now settles in when the scroll pauses, not while it moves. **Report** any stutter.
2. Swipe to the right, starting on the header, the composer or a gap between rows: the chat follows the finger, and past about a third of
   the screen (or a quick flick) it slides out and the room cards open. A short swipe springs back. Swipe left on the room in focus: back
   into its chat. On the rooms, swipe right again: all communities (with one community it rubber-bands and stays). The dots still work as
   before: a swipe in from the left edge or along them opens the cards.
3. Swipe a message to the right a little: the row follows, a reply mark appears, and letting go past it starts a reply to that message. Keep
   the same swipe going across the screen instead: the row springs back and the whole chat slides to the rooms, with no reply.
4. Hold a message: its menu (reactions, reply, copy text, edit and delete when they apply), with no text selection or copy bubble from iOS.
   Tap a message twice: a ❤️ (twice again takes it away). A single tap still shows the row's tools; a link opens once.
5. Tap a picture: full screen. Pinch to zoom and drag around; two taps zoom in and out; pull down to close; swipe sideways through the room's
   other pictures. **Report** anything that scrolled or selected when it should not have.

## Results

One line per check. Pass means it did what the check says; write the version, system and browser in Notes for anything else.

| # | Check | Who / device | Pass | Notes (exact text seen, seconds, version) |
| --- | --- | --- | --- | --- |
| A1 | Download page offers the right file first | A (Win), B (Mac) | | |
| A2 | Install, with the system's warnings | A (Win), B (Mac) | | |
| A3 | Opens in Brave as an app window; time to first screen | A, B | | |
| A4–A5 | Password join lands inside the community | A, B | | |
| A7 | First message round trip | A, B, Owner | | |
| B | Old invite link accepted | one friend | | |
| C | Recovery key reminder: survives reload, goes after **I saved it**, visible under devices | A, B | | |
| D1 | Messages, replies, reactions, pictures both ways | all | | |
| D2 | Same with the older version | Old | | |
| D3 | Unread counts right after switching rooms | all | | |
| E1–E2 | Voice quality, mute, deafen, device choice | all | | |
| E3 | Measured echo latency | all | | |
| E4 | Connection path (webrtc-internals) | one | | |
| E5 | Padlock says encrypted, AES-GCM | all | | |
| E6 | Leave and rejoin heard again | one | | |
| F | Mic refused: muted but listening; allowed: heard | one | | |
| G1–G2 | Wifi drop 15 s and 2 min: **Reconnecting…** then back | one friend | | |
| G3 | Offline message sent later | one friend | | |
| H | iPhone call connects, both ways, background behaviour | P | | |
| I1–I2 | Ring everyone, ring one person | all | | |
| I3 | Ring reaches a backgrounded window (notification) | B | | |
| I4 | Closed app: nothing, no stale card | B | | |
| J1 | Camera 720p and 1080p | two | | |
| J2 | Share as: text, motion, fast motion | two | | |
| J3 | Viewer quality auto/high/medium/low, fullscreen | two | | |
| J4 | Audio with the share | two | | |
| K | Tile stack drag seen by everyone | two | | |
| L1–L2 | Room wall: draw, lock, clear | two | | |
| L3 | Card wall and community card wall | two | | |
| M1–M2 | Tab audio into the call, silent-tab and window messages | Owner + one | | |
| M3 | Linux app audio route | Owner + one | | |
| N | Backdrop pulses to music | two | | |
| O1–O4 | Large upload: pause, resume, wifi drop, reload and continue | Owner | | |
| O5 | Large download: pause, continue, file opens | A | | |
| P | Update to the next version on every system | all | | |
| Q | Admin panel changes seen by everyone | Owner + A | | |
| R1–R5 | iPhone: smooth scrolling, swipes between levels, swipe to reply, hold and double tap, picture viewer | P | | |

Versions used: mega ______ (Owner), ______ (A), ______ (B), ______ (P), ______ (Old). Date: __________.
