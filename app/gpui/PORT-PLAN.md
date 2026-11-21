# mega on GPUI: port plan

Scope: the **desktop** app only (Linux first, then macOS; Windows later if wanted). iOS and the browser edition keep
the web UI in `app/ui`. Both UIs talk to the same core through the same JSON API (`commons_core::api::App::call`), so
nothing here may change what the core does. Where the GPUI app needs something the core does not offer, the plan is
to add it to the core for everyone, never to fork behaviour.

Status as of 2026-11-20 (checked against the code in `app/gpui/src`, not against what was planned). Status words: **done** (works in `mega-gpui` today, checked in a headless run), **next** (plain work, no unknowns),
**hard** (needs a component GPUI does not have, or real design), **web-only** (stays in the web UI; the GPUI app does
not need it or hands off to it).

How the web UI gets its data, and so how this app does: there are no pushed events. The page polls `snapshot` and
`messages` every 500 ms with the hash of what it holds and gets `{"same": true}` or a difference back; live events
(presence, wall, rings) are polled with `live_poll` / `ring_poll`. The GPUI app does exactly the same from one
background thread. A push channel from the core would cut idle CPU for both UIs and is listed under "Core changes worth
making" below, but it is not needed for the port.

## 1. Sign-in and account

| Feature (web file) | Status | Notes |
| --- | --- | --- |
| Unlock with passphrase (app.js `boot`) | done | |
| Create account on a relay (name, passphrase) | done | uses the default relay or a typed address |
| First screen with invite password (mega-invite.js `first`) | done (onboard.rs) | `create_account_password`, `password_strength`, `suggest_password` |
| Paste an invite link / relay link (`parse_link`) | done (onboard.rs; also Members > join) | |
| Recover with a recovery key (`recover_account`) | done (onboard.rs) | |
| Pair a new device: code, QR, approve (`start_pairing`, `pairing_*`, `approve_pairing`) | done (onboard.rs: QR from the core's SVG drawn as squares; devices.rs: approve with check digits) | the QR is an SVG string from the core; GPUI draws SVG |
| Recovery key shown and "saved" (`recovery_key`, `recovery_saved`) | done (devices.rs: corner card until saved, panel behind the device password) | |
| Lock | done | |
| Relay moved / relay secret (`set_relay`, `set_relay_invite`) | done (settings: relay address and sign-up secret) | |
| Single instance (second launch raises the window) | in progress (branch gpui-notify) | Tauri plugin today; here a lock file + a local socket |

## 2. Shell and navigation (mega.js)

| Feature | Status | Notes |
| --- | --- | --- |
| Home: community cards | done | cards.rs + nav.rs: the web's one column with a focused, enlarged card, the others shrink and fade, wheel notches and touchpad scroll snap |
| Community level: room cards | done | two to a row |
| Chat level: colour dots on the left rail, the open room as a pill, unread rings | done | |
| Dots open into cards on scroll ("open rail") | done | rail.rs |
| Card flight between levels (the card flies and becomes the page) | hard | GPUI has animation and springs but no shared-element transitions; done by hand with an overlay element |
| Squircle corners | done | squircle.rs: a true superellipse painted as a path (cards, the composer slip, reaction pills, panels) |
| "You" card, options room, "what's new" card (mega-new.js) | partly | the "what's new" card (cards.rs) and the options room card (paint.rs knows it) are there; the "You" card is settings |
| Tour (mega-tour.js) | next | |
| Command palette / search across rooms (`search`) | see section 4 | built with the message search |
| Window title with unread count | done | |
| Phone gestures (mega-touch.js) | web-only | |

## 3. The look

| Feature | Status | Notes |
| --- | --- | --- |
| Palette, room colours (owner's choice from the style document, else the default by id, same hash as mega.js) | done | look.rs |
| Faces: FK Raster Roman Sharp / Rounded, FK Raster Grotesk Rounded, Bellefair, Figtree, IBM Plex Mono | done | fonts.rs; see "Fonts" below |
| ASCII fields on cards: all 17 patterns, three tints, flowing around the words | done | ascii.rs: one glyph sprite per cell from GPUI's glyph atlas (instanced quads), 20 fps |
| Room backdrop: dim field behind the chat, heat from activity, ripples on new messages and now and then | done | flows around every message, the title and the composer; 10 fps, 25 while a ripple runs |
| Owner's look per room: pattern, dim level, backdrop colours (`set_look`, style document `looks`) | done | paint.rs (card and chat tiles, dim, backdrop colours, the Oracle's two switches) |
| Colour picker for owners (`set_style`) | done | paint.rs: square, hue strip, hex box, presets, per card and per room, sent when the pointer is let go |
| Shared ASCII wall (mega-wall.js; `wall_get`, `wall_draw`, `wall_snapshot`, `set_wall`) | done (wall.rs, tools/gpui-draw-e2e.mjs) | drawing with the pointer onto the cell grid, live strokes over `live_send`, obstacle masks both ways. The core already merges and validates; the pen, pacing and toolbar are ~450 lines of JS to port |
| ASCII message arrival (the text decodes from glyphs for 340 ms) | done | arrive.rs, with thrown reactions and size tiers |
| Message "landing" spring, centre focus while reading back | partly | the reading zone (reading.rs: rows shrink and fade away from the middle) is done; the landing spring is not |
| Typing mascot over the composer | done | typing.rs |
| Bloom under the pointer on a focused card | next | |
| Music-reactive backdrop (mega-music.js) | hard | needs the call's audio (section 9) |
| Battery saver holds everything still | done | |
| Field as a single shader | later | not needed: the measured cost of the fields is about 2 % of one core (SPIKE.md). If 4K at 20 fps ever matters, GPUI has no custom-shader hook; it would be a patch to the pinned GPUI or an offscreen texture painted as an image |

## 4. Messages

| Feature | Status | Notes |
| --- | --- | --- |
| Virtual list of one room, differences from the core, stays at the bottom, live updates | done | GPUI `list`; only visible rows are laid out |
| Send text; Enter sends, Shift+Enter new line; paste; multi-line growth | done | gpui-component `Textarea` |
| Typing indicator, sent and shown | done | |
| Mark read with the `seen` flag; nothing read while cards show | done | |
| Read receipts shown ("read by") | done | plain text; faces are next |
| Reactions shown, added and taken back | done | msgact.rs: chips as the web has them (yours filled with the room's bright colour, names on hover), the smile chip after them, the quick eight from a row's tools or the menu, and the whole set through "+" (emoji.rs). Checked both ways by tools/gpui-msg-e2e.mjs |
| Reply quote shown, replying, jumping to the original | done | the quote is a button (hover: its line takes the room's colour) that scrolls to and lights the message; a source older than the window is asked for with `around`; reply bar over the composer with a cancel button and Escape |
| Edited / deleted / pending / failed marks | done | |
| Load older (limit grows by 200) and jump to a message (`around`) | done | "Show earlier messages" above the first row, the reader's place kept (the message at the top of the view stays at the same offset); `around` for a quote or a search hit that is not held; the limit goes back to 200 on opening a room. Checked in e2e (230 messages, 200 then 400) |
| Day separators, "new" line, jump-to-bottom with count | done | day lines and the "N new" line were there; the jump button now counts messages that arrived while reading back ("3 new", filled with the room's colour) |
| Edit, delete, reply, react from a row (hover actions, context menu) | done | msgact.rs: four tools on a row while the pointer is over it (reply, react, edit own text, delete own or any as admin/owner; `file_delete` for a large file), right-click menu (quick reactions on top, Reply, Copy, Copy message, Edit, Delete, Select all; on pictures and files too), Up in an empty box edits your last message, Escape cancels. The core refuses what a role may not do; the buttons follow its rules |
| **Selecting and copying message text** | done (`select.rs`) | Own implementation over the rows' `StyledText` layouts (gpui-component's selectable text cannot span rows of the list): drag within and across rows, double-click word, triple-click message, Ctrl+C / Ctrl+A, right-click menu (Copy, Copy message, Select all), edge autoscroll; the reading zone keeps the hovered and selected rows whole. Checked with real pointer events by `tools/gpui-select-e2e.mjs` |
| Links (open with `open_link`, which checks them), mentions highlighted | done | rich.rs + views.rs: the page's own rules (app.js `fmt`, `linkify`): trailing punctuation off an address, @Name and @FirstName of the room's people (members asked every 4 s), yours filled; a click opens through the core and copies the address when no browser starts. Mention rows carry the web's gradient and bar; mention counts on the rail and cards are not built |
| Markdown-ish formatting | done | only what the page has: `**bold**`, `*italic*`, inline `code` (the page has no code blocks). The marks are removed and selection and copying work on the drawn words, as a browser's do |
| Big-emoji / message size steps (`data-mz`) | done | arrive.rs `size_tier` (by length, as mega.js has it) |
| Emoji picker | done | emoji.rs: search by Unicode name, recent (kept in gpui-emoji-recent.json), every group in a scrolling list of rows of eight, colour emoji from the system font; for a reaction ("+" after the quick eight) and for the composer (smile button). 1,386 single-codepoint emoji (emoji_table.rs, from the Unicode names and Noto Color Emoji's map); sequences (skin tones, flags, families) are not in it |
| Emoji, CJK, right-to-left | done / check | colour emoji and CJK render (checked); RTL and complex scripts go through the system shaper, unverified |
| Chat density per room (mega-new.js) | next | |
| Search in a room and across rooms | done | search.rs: a "search" chip in the room's header, the page's 220 ms pause, results as the page lists them (community / room, who, words with the match marked), "this room" narrows them, a click opens the room and goes to the message |

## 5. Files, pictures, voice clips

| Feature | Status | Notes |
| --- | --- | --- |
| File rows shown as name and size | done | |
| Send a file, drag and drop, paste a picture (`send_file`, `file_upload_*`) | done | files.rs + xfer.rs. Drop files on the room (GPUI `ExternalPaths`; a dimmed room with the highlight round its edge and "drop to send" while they are over it, the web's `#composer.dropping`), Ctrl/Cmd+V with a picture (or copied files) on the clipboard, the paperclip (`prompt_for_paths`, several files). Several files go one after the other. No resizing or recompression (the web does none); up to 24 MB in one `send_file`, more through the resumable `file_upload_begin/check/chunk/finish`. A pasted picture is named "Pasted image YYYY-MM-DD HH.MM.SS.png". Checked both ways against the web (tools/gpui-files-e2e.mjs: bytes identical by sha256). A real drop from a file manager and the paperclip's dialog need a desktop (portal, Wayland data device) and are untested headless: the drop is simulated at the same handler (`files_dropped`) |
| Pictures inline, lightbox (`file_open`, `file_read`) | done | media.rs: fetched in the core's pieces and decoded on worker threads (3, one big decode at a time), orientation applied, shown as a thumbnail at most 920 x 640 px (twice the column's 460 x 320), newest 48 kept as the page does; a large picture is not fetched until Download. Click: the lightbox (zoom with the buttons, + / - / 0 or the wheel, drag to move, arrows or the side buttons between the room's pictures, Esc or a click on the dark closes, a large copy up to 3072 px decoded only while it is open). Failed pictures say why and retry on click |
| Large files: resumable upload/download, progress, pause, cancel, save-as (`file_transfers`, `file_download_*`, `pick_folder`) | done | xfer.rs is app.js's `uploadLarge`/`downloadLarge`/`xferStep` on threads: backoff retry without end (1 s to 30 s), pause that waits out the part in flight, cancel that asks first when something is already on the relay, speed and time left over the last 20 s, the "Transfers" card at the bottom right (also lists what the core kept across a restart: continue asks for the same file and refuses another, a finished-on-the-relay upload is sent without it). Download opens "Save to" (folder remembered, Browse… is the core's `pick_folder`); a small file is saved into that folder too, with a free name; a saved large file offers Show in folder / Download again |
| Video inline | done (needs ffmpeg) | playback.rs: the first frame (ffmpeg, a little in) is the poster with a play button; Play decodes frames to BGRA at the size drawn, shows each when the clock reaches it (late ones skipped), with a play/pause, seek bar and time under it. The sound is the clock. Without ffmpeg on this computer the row is a plain "video" card and Play opens the file in the system's player (`open_with_system`) and says so. Not a bundled decoder: ffmpeg must be on the PATH (or `MEGA_GPUI_FFMPEG=<dir>`) |
| Voice clips: record, waveform, play (mega-voice.js) | play: done (needs ffmpeg); record: not built | A voice message is the page's row: play, the 64-bar waveform from the message's `peaks` that fills as it plays (click to seek), `0:03 / 0:07`, speed 1 / 1.5 / 2, download. Audio files get a player row (play, bar, time). Sound goes to `pw-cat`, else `paplay`, else `aplay` as raw 48 kHz stereo from ffmpeg (Opus/WebM, MP3, AAC, anything ffmpeg reads); the position is what was handed over. One thing plays at a time. No macOS/Windows sound path yet: there the file opens in the system's player. Recording (microphone, an Opus/WebM writer) is still to do |
| Profile pictures: show (`avatar_versions`, `avatar_data`) | done | avatars.rs: versions asked every 4 s, a picture is fetched and decoded off the UI thread only when its blob id is new, masked to its shape (circle or squircle), kept as small and large GPU images. Message rows, people list, person card, call bar faces, tiles with the camera off, direct and "you" cards; the letter seal when there is none |
| Profile pictures: crop and upload (avatar.js) | done | settings: choose (file dialog) or drop; drag, scroll or buttons to zoom 1..4, circle or square; sent as a 256 px JPEG under 40 KB (the page's own fallback format) through `set_avatar`; `remove_avatar`. Checked both ways against the web client (tools/gpui-avatars-e2e.mjs). The file dialog itself is untested headless (needs the desktop portal); a drop and the dialog share `avatar_open` |

## 6. People, direct calls, presence

| Feature | Status | Notes |
| --- | --- | --- |
| People list on home (one-to-one conversations from `snapshot.direct`), open and write | done | |
| Start a one-to-one (`direct_call`), refuse a person (`direct_refuse`) | done (call on a person's row and card; direct list with stop calls / allow in People) | |
| Members panel with roles, verification, safety numbers, devices (`members`, `safety`, `set_verified`, `devices`, `revoke_device`) | done | members.rs (members, roles, person card, safety number, verification; tools/gpui-members-e2e.mjs). Devices, revoke, history transfer and the activity log are in devices.rs (Settings: Devices and recovery, Activity log); tools/gpui-onboard-e2e.mjs |
| Live presence dots on cards, sent and received, in the web UI's wire format | done | checked both ways against a second account |
| Who is in a call, on voice cards (`call_presence`) | done | names (presence.rs) |
| "Listening to" status and listen-along (mega-listen.js; `listen_*`, `spotify_*`) | partly | the status shows on the people list and the rune bar (runebar.rs) has the call's listen-along; the standalone rows are not built |

## 7. Notifications

| Feature | Status | Notes |
| --- | --- | --- |
| System notifications for messages while the window is not looked at | done | the core's notifier and `NativeSink`, exactly as in the Tauri app; this app sends the heartbeat (`notify_beat`) with whether its window is active |
| Mode (all / mentions / off) and "show text" | done | settings |
| Per-room mute and mentions-only (`notify_room`) | next | a menu on the room |
| Click on a notification opens the room | in progress (branch gpui-notify) | the notification service with quick reply is in notifier.rs |
| Sound (the two-note chime) | next | a 0.6 s synthesised chime through `cpal`; honours the sound setting. Not built: the brief forbids audible sound in tests |
| Taskbar attention on a mention | next | |
| Rings for voice rooms and direct calls (mega-ring.js; `ring_poll`, `ring_send`, `ring_end`) | done | ringui.rs + calls/ring.rs, tools/gpui-ring-e2e.mjs |

## 8. Settings and admin

| Feature | Status | Notes |
| --- | --- | --- |
| Name, typing, read receipts, presence switch, listening sharing | done | |
| Battery saver auto / on / off (`power_status`) | done | |
| Chat font Bellefair / Raster | done | |
| Notifications | done | see 7 |
| Reading size, sound, download folder, rune address | next | |
| Devices and recovery, activity log (`notices`), history transfer (`history_*`) | next | |
| Game keys: per-game push-to-mute (`game_keys_*`, `running_programs`) | next | the service is in the core (commons-gamekeys); needs calls to matter |
| Server settings for owners and admins (mega-admin.js): rename, rooms, roles, kick, unban, invites, transfer | done | members.rs (the owner's panel), checked by tools/gpui-members-e2e.mjs |
| Create community, create room, invites by link and by password (mega-invite.js; QR) | done | members.rs (the QR is not drawn) |
| Join by link or password, cancel a join | done | members.rs; an invite link on the command line is joined at start |
| Updates (update-ui.js, `app/src-tauri/src/update.rs`) | done (update.rs, updateui.rs: what's new card, tools/gpui-update-test.sh) | the Tauri updater plugin does download, signature check and restart today. The signature check is already in the core (`update.rs`, minisign); a native updater for an AppImage/tarball and a macOS bundle has to be written and tested. Until then: the release script ships it and the app only says "a newer version exists" |

## 9. Calls and screen share (built; CALLS-PLAN.md has the state, this section is the original design)

### Where things stand today

On Linux the Tauri window cannot do calls at all: WebKitGTK has no WebRTC (docs/CALLS-PLATFORMS.md). The app hands the
call to a browser tab through the core's loopback bridge (`open_call_in_browser`). That hand-off is a core call, so
**step 0 for the GPUI app is the same button**: `open_call_in_browser` works from GPUI today with no new code beyond the
button, and gives Linux exactly what it has now. Everything below is about doing better than that: calls inside the
window.

### What the web UI does (call.js, call-ui.js, mega-deck.js) and what must match

- Media goes through a LiveKit SFU. The token is `call_token`; who may hear is `call_state` (members, epoch, key).
- Every frame is end-to-end encrypted with LiveKit's own frame encryption: `BaseKeyProvider({ sharedKey: false,
  ratchetWindowSize: 0, failureTolerance: -1 })`, one key per participant, set from the room's MLS epoch, polled
  every 1.5 s and rotated when the epoch changes. A track the server does not mark encrypted is unpublished at once
  and the call refuses to continue.
- Screen share in 0.3.11 (call.js line 619 on): three profiles.
  - Sharp text: up to 2560x1440 at 30 fps, 6 Mbit/s, `contentHint: 'detail'`, `degradationPreference:
    'maintain-resolution'`.
  - Smooth motion: 1920x1080 at 30 fps, 6 Mbit/s, `contentHint: 'motion'`, `maintain-framerate`.
  - Fast motion: 1920x1080 at 60 fps, 9 Mbit/s, `motion`, `maintain-framerate`.
  - Each publishes **the full picture plus one fallback layer** at 960x540 (350 kbit/s at 5 fps for text, 600 kbit/s
    at 15 fps for motion), VP8, simulcast on.
- Microphone with the browser's processing; music and "PC audio" published as stereo 192 kbit/s, DTX off, RED on;
  cameras with simulcast; a played video as a screen-share-source track named `movie`.
- The stack (mega-deck.js): every camera and screen share in one column on the left; dragging a tile reorders it
  for everyone (live channel); the column's edge drags to resize, per device.
- Game keys hold the microphone (mic-gate.js); rings; "only one thing plays at a time".

### Design for GPUI

The SDK type and option names below are from the LiveKit Rust SDK as known when this was written; none of it has
been compiled against a pinned version yet, so each name is to be checked at step 2.

**Library.** `livekit` (the LiveKit Rust SDK, which wraps libwebrtc through `libwebrtc`/`webrtc-sys`), pinned to an
exact version like GPUI. Zed ships calls and screen share on exactly this stack on macOS and Linux, so GPUI + LiveKit
Rust + native capture is a proven combination, including rendering remote video in a GPUI window.

**Process shape.** A `calls` module inside `mega-gpui` (not the core: iOS and the browser do calls in their web
view). It owns one tokio runtime for LiveKit. It asks the core for `call_token` and `call_state` on the same cadence as
call.js (1.5 s) so `call_active` keeps working (the updater and the core rely on that heartbeat), and it calls
`game_mute_wait` for game keys.

**Encryption.** `E2eeOptions { key_provider: KeyProvider::new(KeyProviderOptions { shared_key: false,
ratchet_window_size: 0, failure_tolerance: -1, .. }) , encryption_type: Gcm }`, with `set_key(identity, index, key)`
per participant from `call_state`, mirroring `_applyKeys` in call.js. The frame format is LiveKit's standard one on
both sides (the web UI uses the stock `livekit-client.e2ee.worker.js`), so a Rust client and a browser client should
decrypt each other. **This is the first thing to prove** (see order of work): a headless Rust participant and a
Chrome participant in one local LiveKit room, both directions, with the key salt and ratchet settings compared. The
same refusals apply: if `is_encrypted` is false on a published track, unpublish and fail the call.

**Audio.** LiveKit Rust gives raw PCM in and out (`NativeAudioSource`, `NativeAudioStream`); devices are ours. Capture
and playback through `cpal` (PipeWire/ALSA on Linux, CoreAudio on macOS). The browser gave echo cancellation, noise
suppression and gain for free; here they come from libwebrtc's audio processing module (`AudioProcessingModule` in
the SDK), which must be wired by hand: far-end reference from the playback mix, 10 ms frames. Device pickers, per
person volume, output device choice (`setSinkId` today) are ours too. Zed's `livekit_client` crate is a working
example of all of this and is the reference to read first.

**Screen capture.**
- Linux/Wayland: the xdg-desktop-portal ScreenCast portal (the system's own picker; `ashpd`) gives a PipeWire stream;
  frames arrive as DMA-BUF or shared memory. libwebrtc's desktop capturer (through the SDK's `DesktopCapturer`) does
  portal + PipeWire itself and is the first choice; a direct `pipewire` consumer is the fallback if it will not give
  60 fps or DMA-BUF.
- macOS: ScreenCaptureKit (the SDK's capturer uses it), with the system picker on macOS 14+.
- GPUI's own `screen-capture` feature (`scap`) exists but is X11-centred on Linux; not the plan.

**Publishing a share so it matches 0.3.11.**

| Web (0.3.11) | Rust SDK |
| --- | --- |
| `screenShareEncoding` 2560x1440 / 30 fps / 6 Mbit/s (and the two motion profiles) | `TrackPublishOptions { source: Screenshare, video_codec: VP8, video_encoding: Some(VideoEncoding { max_bitrate, max_framerate }), .. }`, with the capturer asked for that resolution and rate |
| full picture + one 960x540 layer | `simulcast: true` and exactly one extra layer. The Rust SDK computes default screen share layers itself; if its options cannot name the single 540p layer (to be checked against the pinned version), the fix is a small patch to how it builds `RtpEncodingParameters` (two encodings: `scale_resolution_down_by` = height/540 with 350 or 600 kbit/s and 5 or 15 fps, and the full one) |
| `contentHint: 'detail'` / `'motion'` | `NativeVideoSource::new(resolution, is_screencast = true)` marks the source as screen content (libwebrtc then favours sharpness and enables screen content tools); the motion profiles pass `false`. This is the Rust-side equivalent of the content hint |
| `degradationPreference: 'maintain-resolution'` / `'maintain-framerate'` | set on the sender's RTP parameters (`RtpParameters.degradation_preference`). Exposed in recent SDK versions; if not in the pinned one, it is a one-line addition in `webrtc-sys` |
| VP8 | VP8 (software). Hardware H.264/AV1 on the 4090 would be far cheaper, but every viewer must decode it, so the codec stays VP8 until the web side changes too |

Viewers ask for the layer they need by the size of the tile (`set_video_dimensions` / adaptive stream), as the browser
client does.

**Rendering video.** Remote frames arrive as I420. Zed converts to a GPU surface and paints it with GPUI's `surface`
element on macOS (CVPixelBuffer) and as images on Linux. Plan: NV12/I420 to a `RenderImage` per tile at the tile's size;
if that costs too much at 1440p30, the next step is a small patch to the pinned GPUI to accept a YUV texture.
This is the largest unknown for CPU use; it gets measured in the first week of the work.

**The stack and the dock.** A column of tiles with `gpui-component`'s `resizable` panel for the edge (the size saved in
the prefs file, per device, never sent); tiles reorder by drag (GPUI has drag and drop; the order goes out over
`live_send` in the format mega-deck.js uses, so a browser participant sees the same order). The dock (mute, deafen,
share, camera, leave, the lock that explains what the server can see) is ordinary buttons.

**What stays in the browser at first.** "Play into the call" (music, a video file), PC-audio capture of one app
(the core's pcaudio.rs already does the PipeWire routing on Linux), and the music-reactive backdrop. They need
decoders or a second capture path and are not what makes a call a call.

### Order of work for calls, with estimates

These are working-time estimates for one focused agent with a human testing real calls between two machines; they
are the weakest numbers in this document because each step depends on a library version behaving.

1. The hand-off button (`open_call_in_browser`) and call presence in the room: half a day. Linux parity with Tauri.
2. Proof of encryption interop: a Rust participant and a Chrome participant, audio both ways, in a local LiveKit:
   2-4 days. **If this fails, calls stay in the browser** and the rest of this section is void.
3. Voice: devices, echo cancellation and noise suppression, mute/deafen, speaking indicators, game keys, rings,
   reconnects, key rotation on membership change: 1.5-2.5 weeks. Echo cancellation is where the time goes.
4. Watching screen shares and cameras, the resizable stack, drag to reorder: 1-1.5 weeks.
5. Sharing a screen on Linux (portal, PipeWire) with the three profiles and the 540p layer: 1-2 weeks.
6. The same on macOS (ScreenCaptureKit, permissions, CoreAudio): 1-1.5 weeks, needs a Mac to test on.
7. Camera publishing, play-into-call, PC audio: 1-2 weeks, optional.

Total for calls that match 0.3.11 on Linux: **about 5-8 weeks**; macOS adds 1-2. Building libwebrtc is not needed
(the SDK downloads prebuilt binaries) but the binary grows by roughly 30-50 MB and the build needs the prebuilt
archive for each target.

## 10. Packaging and release (`mega-gpui` is in no release script; the native updater, update.rs, exists)

| Item | Status | Notes |
| --- | --- | --- |
| Linux: AppImage or tarball | next | one binary today, about 50 MB unstripped. Needs Vulkan at run time; a machine without a working Vulkan driver gets no window (GPUI has no GL fallback). Worth a clear error message |
| macOS: bundle, signing, Metal | next | GPUI's home platform; unbuilt and untested here |
| Windows | later | GPUI on Windows exists (DirectX); out of scope for now |
| Same data folder as the Tauri app | next | the spike uses its own folder unless `MEGA_GPUI_REAL_PROFILE=1`. The core's folder lock already keeps both apps off one account at once; switch the default when this replaces the Tauri app |
| Accessibility | check | gpui-component carries AccessKit roles; the custom cards and rows need labels |

## Fonts

- The FK Raster files are the **trial** files the web UI ships (the note in mega.css says the foundry allows them in
  builds until the licensed ones replace them). They are embedded unchanged. All Roman cuts share one family name, so
  in memory only each file's family records are pointed at its full name (fonts.rs); nothing modified is written or
  shipped. Worth a line to the foundry when the licensed files arrive: they may already have distinct family names.
- The trial faces lack `" # % & * + < = > @ \ ^ _ \` | ~`; those fall back to another face, as in the web UI.
- Bellefair, Figtree and IBM Plex Mono are the web UI's woff2 files decompressed to TrueType (GPUI does not read
  woff2); SIL OFL, licences in app/ui/fonts.

## Core changes worth making (for both UIs, none required)

1. `messages` builds, hashes and compares the whole window on every poll. With 5,000 rows open that is about 5 % of a
   core in either app, forever. A per-room change counter checked before building anything would make an idle poll
   free.
2. A blocking "wait for change" call (the relay side already long-polls) would let both UIs drop the 500 ms timer.
3. The Tauri shell runs each `rpc` on a pool thread (`spawn_blocking`); the allocator then keeps a high-water mark
   per thread. In this app that was 745 MB instead of 317 MB with a long room open, fixed by using one thread. The
   Tauri app's own process showed the same pattern and would likely shrink the same way.

## Order after phase 1

1. Sign-in completeness (invite password, links, recovery, pairing) and create/join/invite: without these the app
   cannot be someone's only client. About 1 week.
2. Message actions (reply, edit, delete, react), older messages, links, mentions, day lines: about 1 week.
3. Text selection and the emoji picker: about 1 week, the two "hard" items everyone meets on day one.
4. Files and pictures, large transfers, avatars: 1-1.5 weeks.
5. Members, devices, safety numbers, admin room, notifications per room, rings: 1-1.5 weeks.
6. Look: carousel and card flight, owner's looks, arrival animation, the wall: 1.5-2.5 weeks (the wall is half of it).
7. Calls: section 9, 5-8 weeks.
8. Packaging, updater, macOS: 1-2 weeks plus testing on real machines.

Everything but calls: roughly **7-10 weeks** of agent time. With calls: **12-18 weeks**. The Tauri app keeps
shipping the whole time; nothing forces a switch before the GPUI app is the better one.
