# Calls in mega on GPUI: the plan

Written 2026-11-15, planning only. Nothing in `app/gpui/src` was changed. It replaces section 9 of PORT-PLAN.md where
the two differ: that section was written before any SDK version was pinned or compiled, and several of its guesses
turned out wrong (noted below).

Decisions this plan takes as given: the browser edition is ending and mega desktop becomes this native app; calls
are the top priority of the port; the iPhone app keeps its web UI, so **nothing on the wire may change**. Web and
iOS clients must keep working in the same call as a native one.

## Progress (2026-11-15, branch `gpui-calls`)

Steps 0, 1 and 2 are done; step 3 waits for a person (echo check).

- Step 0: `examples/call_probe.rs` + `tools/gpui-call-interop.mjs`, 15 checks, two runs, all pass (all eight items
  below plus L1, L2 and W1).
- Step 1: `livekit =0.11.0` with L1, L2 and W1 in `app/gpui/vendor` (`refresh.sh --check` verifies them against
  crates.io). No runtime crate of any other workspace member changed (`cargo tree` with features is identical), so
  `app/gpui` stays in the workspace, and the Tauri app builds `--locked`. Render bench (`MEGA_GPUI_VIDEO_BENCH`):
  2560x1440 at 30 fps for 5 minutes, 10 to 15 % of one core, p99 33.7 ms, +15 MB from minute 1 to 5. Release
  binary: +27.5 MB stripped (53.0 to 80.5 MB).
- Step 2: `calls/{engine,keys,gate,review,reconnect,errors,ice,testhook,headless}.rs`, `mega-gpui --call-headless`,
  `tools/gpui-call-e2e.mjs`, 18 checks, two runs, all pass; 25 unit tests.

Corrections to this plan found on the way:

- `call_active` goes false 20 s after the last `call_state` (the core's `CALL_QUIET`), not 5 s. The core was not
  changed; the test checks 25 s.
- Core calls use two dedicated threads, not one: `game_mute_wait` is a 4 s long poll and would hold up the 1.5 s
  heartbeat.
- W1's gain is applied in the playout mix, after a `NativeAudioStream` tap, so a tap does not show it. The receiver's
  `inbound-rtp` `totalAudioEnergy` does (volume 0: 1e-7, volume 1: 0.23), with no PipeWire needed.
- The local LiveKit 1.13.11 sends `stun:stun.l.google.com`, `stun:stun1.l.google.com` and
  `stun:global.stun.twilio.com` in its join response, so L1 is needed exactly as described.
- The vendored sources are committed (about 9 MB, mostly webrtc-sys), not only the patch files: a `[patch]` path that
  does not exist breaks every cargo command in the workspace.
- The engine sets `join_retries: 0` so that call.js's retry timings are the only ones in play.

## Summary

- **Go.** A Rust LiveKit participant and the real `call.js` in headless Chromium were put in one local LiveKit room
  today with mega's real keys (from the core's `call_state`). Each side decoded the other's audio and video. Three
  controls that should fail did fail. Details in section 2.
- **SDK:** `livekit = "=0.11.0"` (with `libwebrtc 0.4.1`, `webrtc-sys 0.3.54`, prebuilt libwebrtc m150). It builds
  with our Rust 1.99.1 in 36 s, downloads libwebrtc instead of compiling it, and is Apache-2.0.
- **Two things must be set that the SDK does not default to**, or nothing decrypts: the key derivation must be HKDF
  (the SDK's default is PBKDF2), and the sender's key index must be set by hand on every published track (the SDK
  leaves it at 0; mega uses `epoch % 16`).
- **The SDK now does more than PORT-PLAN assumed.** It has its own microphone and speaker handling with echo
  cancellation, noise suppression and gain (`PlatformAudio`), its own screen capturer for the Wayland portal and
  ScreenCaptureKit, and publish options for exactly the 0.3.11 screen share layout. No `cpal` and no hand-wired
  audio processing are needed.
- **Three small patches to the SDK are needed** (about 60 lines in all), kept as patch files over the crates.io
  sources: filter the ICE server list, set the key index when a sender's cryptor is created, and a volume control
  for one remote audio track.
- **Not in the SDK:** camera capture, and drawing video in a GPUI window on Linux. Both are planned below.
- **Estimate:** about 5 to 7 weeks of agent time for Linux calls that match 0.3.11 (steps 0 to 8), plus 1 to 1.5
  weeks for macOS with a Mac to test on. "Play into the call" (files, rune, Jellyfin) is not in that figure.

## 1. SDK choice and version

All of this was checked with `cargo` in a scratch crate (`~/data/agent-tmp/mega-calls/probe`), not from memory.

| Item | Finding |
| --- | --- |
| Crates | `livekit 0.11.0` (2026-11-13), `libwebrtc 0.4.1`, `webrtc-sys 0.3.54`, `webrtc-sys-build 0.3.21`, `livekit-protocol 0.8.3`. `livekit-api` is **not needed**: the relay mints the tokens |
| Toolchain | Rust 1.99.1 at `~/data/agent-tmp/commons-ux/wt/target-docker` builds it. No MSRV is declared by the crates |
| Build cost | First debug build of a crate depending on `livekit`: **36 s** wall at `-j 8`, including the libwebrtc download. A release build of the probe was about the same. libwebrtc is **not compiled**: `webrtc-sys-build` downloads `webrtc-linux-x64-release.zip` (156 MB download, 721 MB unpacked, `libwebrtc.a` 85 MB) from the `livekit/rust-sdks` GitHub release `webrtc-a545de4`, then compiles about 40 small C++ binding files |
| Offline builds | The download goes into the target folder (`target/<profile>/build/scratch-*/out/livekit_webrtc`). For reproducible or offline builds set `LK_CUSTOM_WEBRTC=<folder>` to a copy kept beside the toolchain |
| libwebrtc | Milestone 150, from `webrtc-sdk/webrtc@m150_release`. Built with H.264, H.265, VP9, AV1, PipeWire and X11 capture (`args.gn` in the archive) |
| Binary size | Release probe (LiveKit, tokio, TLS, nothing else): 39.8 MB, **30.2 MB stripped**. Expect `mega-gpui` to grow by roughly 30 MB |
| System libraries | `ldd` on the probe shows only libc, libm and libgcc. PulseAudio, PipeWire, GLib and X11 are opened at run time when used, so the app still starts on a machine without them |
| Licence | Apache-2.0 for all LiveKit crates. libwebrtc is BSD-3-Clause with third-party notices in `LICENSE.md` inside the archive; that file must ship with the app (step 1) |
| Linux | Works here (CachyOS, PipeWire with pipewire-pulse): the probe joined, published and received |
| macOS | Prebuilt archives exist for `mac-arm64-release` and `mac-x64-release` (both answer HTTP 200). **Not built or run**: there is no Mac in this work |
| TLS | Feature `rustls-tls-native-roots` resolves and builds. Use it, so the app needs no OpenSSL |

Pin all four crates with `=` like GPUI. LiveKit released four versions in the last three weeks; upgrades should be a
deliberate step that re-runs step 0.

**Not checked:** adding `livekit` to the *workspace* `Cargo.lock`. The probe was a crate of its own. LiveKit brings
`reqwest 0.13`, `tokio-tungstenite 0.29` and `prost`, and SPIKE.md records that adding GPUI already moved three
build-time crates. Step 1 checks that no runtime crate of the released apps changes version. If one does,
`app/gpui` becomes a workspace of its own with its own lock file.

## 2. End-to-end encryption interop

### What mega does today

- Core, `crates/core/src/call.rs`: for every device in the room's MLS group, the frame key is
  `MLS-Exporter("commons-call-e2ee-v1", "<group id>|<device id>", 32)` at the current epoch. `call_state` returns
  `{epoch, me, keys: {device: base64 key}, members}`.
- Relay, `crates/relay/src/lib.rs` (`mint_livekit_token`): identity is the device id, the room is
  `sha256("commons/livekit-room/v1" + gid)` in hex, the grant is join, publish, subscribe and
  `canUpdateOwnMetadata`. **`canPublishData` is false**, so nothing in a call uses LiveKit data messages.
- `call.js`: `BaseKeyProvider({ sharedKey: false, ratchetWindowSize: 0, failureTolerance: -1 })`. Each key is
  imported with `createKeyMaterialFromBuffer` (so the derivation is HKDF) and set with
  `onSetEncryptionKey(material, device, epoch % 16)`. Keys are polled every 1.5 s. On a new epoch the old slot stays
  3 s, then is overwritten with random bytes for every device except our own.

### The two implementations, side by side

Web: `livekit-client 2.23.0`, sources in `tools/node_modules/livekit-client/src/e2ee`. Native: the C++ transformer
in libwebrtc (`api/crypto/frame_crypto_transformer.{h,cc}`), reached through `webrtc-sys/src/frame_cryptor.rs`. The
header was read from the prebuilt archive; the `.cc` from the `m150_release` branch, which may be a few commits
away from the prebuilt build.

| | Web worker | Rust SDK (libwebrtc) | Same? |
| --- | --- | --- | --- |
| Per-participant keys | `sharedKey: false` | `KeyProvider::new` is always per-participant | yes |
| Key derivation | HKDF-SHA256, because the key is imported as `'HKDF'` | `KeyDerivationAlgorithm::HKDF` exists; **the default is PBKDF2** | only when set |
| Salt | `"LKFrameEncryptionKey"` | `DEFAULT_RATCHET_SALT`, the same string | yes |
| HKDF info | 128 zero bytes | 128 zero bytes (`DeriveHkdfSha256FromSecret`) | yes |
| Frame key size | `keySize: 128`, AES-128-GCM | `key_size` 128 in C++; the Rust options cannot change it | yes |
| IV | 12 bytes: SSRC, RTP timestamp, timestamp minus a send counter | the same three fields (`makeIv`) | yes |
| Frame layout | clear header, ciphertext and tag, IV, `[IV length, key index]` | the same | yes |
| Clear header, also the AAD | Opus 1 byte; VP8 10 bytes on a key frame, 3 otherwise | the same (`get_unencrypted_bytes`) | yes |
| GCM tag | 128 bits | 128 bits | yes |
| Key ring | 16 slots | `key_ring_size: 16` | yes |
| Ratchet window | 0 | set `ratchet_window_size: 0` (the Rust default is 16) | when set |
| Failure tolerance | -1 | -1 is the Rust default | yes |
| Server-injected frames | `sifTrailer` from the join response, payload checked | `set_sif_trailer` is called by `Room::connect`, payload checked | yes |
| RED for audio | off when encryption is on | off when encryption is on (`local_participant.rs:412`) | yes |
| **Key index used to send** | setting a key at an index selects it | **stays 0** unless `FrameCryptor::set_key_index` is called. The `livekit` crate never calls it | **no** |

### What was run today

`~/data/agent-tmp/mega-calls/harness/interop.mjs` uses the existing test world (`tools/lib/env.mjs`, `world.mjs`):
a local `livekit-server` 1.13.11, a relay, two accounts in one room. Alice is the real 0.3.11 `call.js` in Chrome
for Testing (headless, ANGLE on Vulkan, `--password-store=basic --use-mock-keychain`, fake devices, muted). Bob is
`probe/src/main.rs`, a Rust participant that takes Bob's `call_state` and `call_token` from his bridge. Results are
in `~/data/agent-tmp/mega-calls/run1/result.json`. The room was at epoch 1, so the key index was 1.

| Run | Rust hears and sees the browser | Browser hears and sees Rust |
| --- | --- | --- |
| Correct settings | 500 Hz tone, share of energy 0.997; 277 video frames at 1280x720; state `Ok` | 900 Hz tone, share 0.56 (the test suite's own pass mark is 0.15); 640x360 video playing, centre pixel (32, 191, 65) for a sent (32, 192, 64); no decrypt errors; both tracks marked encrypted |
| Rust's own key wrong by one byte | still fine | silence, no picture, 12 decrypt errors |
| Key index left at 0 | still fine | nothing arrives; Rust reports `MissingKey` and sends nothing |
| PBKDF2 (the SDK default) | 1,400 audio frames of zero energy, no video | silence, decrypt errors |

So the wire format matches for Opus and VP8 in both directions, and the failures prove the pass was not plain
media slipping through.

### Verdict and what is still unproven

**Go, with high confidence** for audio and VP8 video. Not yet covered, all in step 0:

- A key rotation during a call, and a removed member being locked out.
- Simulcast sent **from** Rust (the probe sent one layer; it received Chromium's simulcast camera).
- A reconnect. After a full reconnect the SDK republishes tracks itself, which makes new cryptors at index 0.
- Safari and the iPhone web view (they use `RTCRtpScriptTransform`; the frame format is the same code) and Firefox.
- The production LiveKit server's version. The SDK defaults to one peer connection and falls back to two.

### Rules the native client must follow (from the findings)

1. `KeyProviderOptions { ratchet_window_size: 0, failure_tolerance: -1, key_ring_size: 16,
   key_derivation_algorithm: KeyDerivationAlgorithm::HKDF, ..Default::default() }`.
2. `RoomOptions.encryption = Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider })`. Keys are
   set before `Room::connect`.
3. After every publish, every `LocalTrackRepublished` and every rotation: for each entry of
   `room.e2ee_manager().frame_cryptors()` whose identity is ours, call `set_key_index(epoch % 16)`.
4. **No media is fed to a track until rule 3 has run for it.** We own the video and music sources, so they simply
   start late. The microphone comes from the device module and cannot be held back, which is why patch L2 below
   exists: with it the cryptor is born with the right index.
5. Our own old key slot is overwritten with random bytes after the 3 s grace, like everyone else's. This is safe
   natively (setting a key does not change the sending index, unlike the web) and means a cryptor that wrongly sits
   on an old index sends nothing instead of using a stale key.
6. A published track whose `encryption_type()` is `None` is unpublished at once and the call reports `no-e2ee`. A
   remote track is subscribed only if its sender is in `call_state.members` and the publication is encrypted
   (`auto_subscribe: false`, then `set_subscribed(true)`), exactly as `_reviewPub` does.

### If interop had failed, or breaks after an upgrade

The fallback is our own frame transformer: the format is about 150 lines (HKDF, AES-128-GCM, the trailer), and
`webrtc-sys` already has the hook points (`new_frame_cryptor_for_rtp_sender` wraps a `FrameTransformerInterface`).
It would be a vendored C++ class in `webrtc-sys`, roughly a week. Nothing suggests it is needed.

## 3. Patches to the SDK

Kept as patch files in `app/gpui/vendor/patches/`, applied to unpacked crates.io sources in `app/gpui/vendor/` by a
script, and wired with `[patch.crates-io]`. The prebuilt libwebrtc is unchanged; only binding code is touched.

| Patch | Crate | What | Why |
| --- | --- | --- | --- |
| L1 | `livekit` (`rtc_engine/rtc_session.rs`, `make_rtc_config`) | An optional hook on `RoomOptions` that receives the server's ICE list and returns the one to use | `call.js` removes Google's and Twilio's STUN servers and moves TURN over TLS from port 443 to `turn_port` (`fixIce`). The SDK takes the server's list as it is unless we supply a whole list, and we cannot: the TURN credentials only arrive in the join response. Without L1 the native app would tell Google who is calling |
| L2 | `livekit` (`room/e2ee/manager.rs`, `setup_rtp_sender`) | `frame_cryptor.set_key_index(key_provider.get_latest_key_index())` | Closes the window in rule 4 for the microphone and after the SDK's own republish. Worth sending upstream |
| W1 | `webrtc-sys` (`audio_track.{h,cpp,rs}`) and `libwebrtc` | `set_volume(f64)` on an audio track, calling `GetSource()->SetVolume()` | Volume per person. The SDK plays remote audio inside libwebrtc and exposes no gain. libwebrtc supports 0 to 10 on a remote source. Expected about 30 lines; **not yet written or compiled** |

Step 1 first logs what ICE servers the join response really contains, to confirm L1 is needed as described.

## 4. Audio

PORT-PLAN said: `cpal` for devices, and libwebrtc's audio processing wired by hand. **That is no longer the plan.**
`livekit 0.11` has `PlatformAudio`, which switches libwebrtc's own audio device module on.

| Need | How |
| --- | --- |
| Microphone | `PlatformAudio::new()`, then `LocalAudioTrack::create_audio_track("mic", audio.rtc_source())`, published with `source: Microphone`, `dtx: true` |
| Speakers | The same module plays every subscribed remote audio track. Nothing to write |
| Linux backend | PulseAudio API, so PipeWire through pipewire-pulse; ALSA otherwise. Checked here: it listed 2 inputs and 4 outputs without opening a stream |
| macOS backend | CoreAudio, by the SDK's documentation. Unverified |
| Echo cancellation, noise suppression, gain | libwebrtc's software processing, on by default: the probe reports `Software` for all three. `set_echo_cancellation`, `set_noise_suppression`, `set_auto_gain_control` map to the three existing prefs. `setNoiseSuppression(on)` in call.js also forces echo cancellation on; keep that |
| Device lists | `recording_devices()`, `playout_devices()`: id, name, index |
| Switching during a call | `switch_recording_device(&id)`, `switch_playout_device(&id)`. Prefs keep `micId` and `outId`; an id that is gone falls back to the default |
| Mute | `publication.mute()` / `unmute()` so the server's track state (which the relay reads for the rail) changes, plus `stop_recording()` / `start_recording()` so the system's microphone indicator goes off |
| Deafen | `disable()` on every remote audio track, attribute `deaf=1`, and it implies mute. Undo with `enable()` |
| Volume per person | Patch W1, applied to each of that account's audio tracks on subscribe. Stored per **account** in prefs, as `commons.call.volumes` is today |
| Speaking | `RoomEvent::ActiveSpeakersChanged`. Our own state goes out as attribute `speaking`, at most one change per 600 ms, at once on mute changes (`_announceSpeaking`) |

**The "noise gate".** `mic-gate.js` is not a level gate. It arbitrates two voices over the microphone: the person
(`manual`) and a game key (`game`). Either can keep it off; neither overrules the other's "off"; changes are sent
one at a time and only the latest wish counts. Port it as `calls/gate.rs` with the same five methods, and port
`tools/mic-gate.test.mjs` as its unit test. Game keys come from the core as today: a `game_mute_wait` long-poll
with `since`, silence for 9 s means the keys hold nothing, `game_mute_clear` on unmute. Push-to-talk uses GPUI key
events while the window has focus and never while a text field has it.

**Risks here.** Echo cancellation quality with speakers can only be judged by a person. Whether stereo music plays
in stereo through the device module is unmeasured (step 3 measures it with a left-only tone from Chromium).

**Silence in tests.** `MEGA_GPUI_CALL_AUDIO=none` makes the engine skip `PlatformAudio`: libwebrtc then keeps its
synthetic device (plays nothing, records nothing), the "microphone" is a `NativeAudioSource` fed a tone, and
hearing is measured on a `NativeAudioStream`. Every automated test uses this. Tests of real devices (step 3) run
against a private PipeWire with a null sink in its own `XDG_RUNTIME_DIR`, never the desktop's.

## 5. Video: cameras and drawing

### Camera capture

The SDK has no camera capturer. Plan: `nokhwa` 0.10.11 (Apache-2.0; V4L2 on Linux, AVFoundation on macOS),
capturing 1280x720 at 30 fps as MJPEG or YUYV, converted to I420 with the SDK's libyuv helpers and pushed into a
`NativeVideoSource::new(resolution, false)`. **`nokhwa` was not probed**; step 6 starts by building it. If it will
not build or behave, the fallback on Linux is the portal's camera through PipeWire, and on macOS a small
AVFoundation capture in Objective-C.

Publishing, matching `setCamera` in call.js:

    TrackPublishOptions {
        source: TrackSource::Camera, video_codec: VideoCodec::VP8, simulcast: true,
        video_encoding: Some(VideoEncoding { max_bitrate: 2_500_000, max_framerate: 30.0 }),
        simulcast_layers: Some(vec![VideoPreset::new(320, 180, 200_000, 15.0), VideoPreset::new(640, 360, 800_000, 30.0)]),
        degradation_preference: Some(DegradationPreference::Balanced), ..Default::default()
    }

The 1080p profile is the same shape with call.js's numbers. A camera switched off is **muted, not unpublished**,
as in the web client, and its tile goes away on mute.

### Drawing remote video in GPUI

Findings from the pinned GPUI (`gpui-pre 0.3.12`, `gpui-pre-wgpu`):

- `paint_surface` takes a CoreVideo pixel buffer and exists **only on macOS**. The wgpu renderer has a two-plane
  YCbCr shader, but on Linux it skips surfaces (`wgpu_renderer.rs:1637`, "macOS-only").
- So on Linux a frame is drawn as an image: `RenderImage` (BGRA) through `img(ImageSource::Render(..))`, uploaded
  to the sprite atlas, with `window.drop_image(previous)` each time or the atlas grows without bound.

Measured here in release (`probe/src/bin/facts.rs`, Ryzen 7800X3D, one thread):

| Size | I420 to BGRA | One copy of the BGRA frame |
| --- | --- | --- |
| 2560x1440 | 0.52 ms | 0.28 ms |
| 1920x1080 | 0.29 ms | 0.15 ms |
| 960x540 | 0.07 ms | 0.04 ms |

At 1440p30 that is about 25 ms of CPU a second for conversion and copy. **The GPU upload was not measured**: it is
14.7 MB a frame, 442 MB a second at 1440p30. It is expected to be a few milliseconds a frame on this machine, and
it is the number step 1 must produce on a real window before the tile design is fixed.

Design:

- One task per subscribed video track reads a `NativeVideoStream` and keeps only the newest frame.
- A tile is a GPUI entity of its own. A new frame notifies only that entity, so the rest of the window is not laid
  out again.
- Before converting, the frame is scaled to the tile's size in device pixels when the tile is smaller
  (`I420Buffer::scale`). A tile in the column is usually a few hundred pixels wide, so the common cost is a small
  fraction of the table above. Only an enlarged tile pays full price.
- Frames arriving faster than the window paints are dropped, never queued.
- macOS (step 9): try `paint_surface` with the frame's pixel buffer (`get_cv_pixel_buffer`), which avoids the
  conversion; fall back to the image path.
- If the image path proves too slow on a weak GPU, the next step is a patch to `gpui-pre-wgpu` that draws surfaces
  on Linux from two textures. The shader and pipeline are already there. That means carrying a patched GPUI, so it
  is a fallback only.

### The column of tiles

- Every camera and screen share in one column on the left, as `mega-deck.js` describes. The column's edge drags to
  resize: `gpui-component`'s resizable panel, the width kept in the prefs file per device and never sent. Limits
  as in mega-deck.js.
- Order: the same protocol over the core's live channel (`live_send`, `live_poll`, kind `deck.m` with
  `{s, o, n, b, e, a}` and `deck.q`), with the receiver's rules unchanged: only accounts in the call count, unknown
  tiles are ignored, the higher stamp wins, a held tile is its holder's for 3 s. Tile ids are LiveKit track sids,
  the same on every client. Port the logic to `calls/deck.rs` as pure functions with unit tests.
- Quality per tile: `publication.set_video_quality(High | Medium | Low)`; "auto" is High, as in `setViewQuality`.
  `RoomOptions { adaptive_stream: false, dynacast: true }`, as call.js sets.
- A hide and show control, and the tile's label (name, camera or screen), as today.

## 6. Screen share

### Capture

The SDK wraps libwebrtc's desktop capturer (`libwebrtc::desktop_capturer`), so PORT-PLAN's `ashpd` and a direct
PipeWire consumer are not needed at first.

- **Linux, Wayland (KDE here):** `DesktopCapturerOptions::new(DesktopCaptureSourceType::Generic)` uses the
  xdg-desktop-portal ScreenCast picker and a PipeWire stream. It needs a GLib main loop; GPUI has none, so enable
  the `glib-main-loop` feature of `livekit`, which runs one on a thread of its own. Call `start_capture(None, cb)`
  and then `capture_frame()` on a timer at the profile's rate.
- **Linux, X11:** `Screen` and `Window` source types with `get_source_list()`; a simple picker of our own.
- **macOS:** the same API over ScreenCaptureKit, with the system picker (`set_sck_system_picker`, on by default).
- Frames arrive as BGRA in memory. Convert to I420 with libyuv and push into a `NativeVideoSource`. A picture
  larger than the profile's ceiling is scaled down keeping its shape.
- `set_include_cursor(true)`.
- When the capturer reports a permanent error or the portal session ends, the share stops and the dock says so,
  like the browser's "stop sharing" bar.

Unmeasured: whether the portal path delivers 60 fps and what it costs at 1440p. libwebrtc's capturer copies frames
through memory. If it cannot hold the "fast motion" profile, the fallback is `ashpd` plus the `pipewire` crate with
DMA-BUF, which the SDK can wrap (`NativeBuffer::from_dmabuf`).

### Matching the 0.3.11 encodings

Everything needed is in `TrackPublishOptions` of 0.11.0. **No patch.**

| 0.3.11 (call.js `shareProfiles`) | Rust |
| --- | --- |
| Sharp text: up to 2560x1440, 30 fps, 6 Mbit/s | `video_encoding: Some(VideoEncoding { max_bitrate: 6_000_000, max_framerate: 30.0 })` |
| Smooth motion: 1920x1080, 30 fps, 6 Mbit/s. Fast motion: 1080p, 60 fps, 9 Mbit/s | the same field with those numbers |
| The full picture plus **one** 960x540 layer (350 kbit/s at 5 fps for text, 600 kbit/s at 15 fps for motion) | `simulcast: true, simulcast_layers: Some(vec![VideoPreset::new(960, 540, 350_000, 5.0)])` |
| `degradationPreference` maintain-resolution or maintain-framerate | `degradation_preference: Some(DegradationPreference::MaintainResolution)` or `MaintainFramerate` |
| `contentHint: 'detail'` or `'motion'` | `NativeVideoSource::new(res, is_screencast)`: `true` for sharp text, `false` for the two motion profiles |
| VP8 | `video_codec: VideoCodec::VP8` |
| Source | `source: TrackSource::Screenshare` |

Why one layer comes out: `compute_video_encodings` (`options.rs`) pops the custom list into "mid" and "low". With
one entry there is no "low", so it returns two encodings: the 540p one (rid `q`, scaled down by height / 540) and
the full one. That is read from the source, not yet run; step 5 has a unit test for it and a measurement from a
Chromium viewer.

One uncertainty: libwebrtc treats a screencast source specially with VP8 simulcast (its own layering and frame-rate
limits). If the sharp text profile arrives below 30 fps or with the layers collapsed, use `is_screencast: false`
with maintain-resolution and compare sharpness on a text screen.

Sound with a share: the browser on Linux could only share a tab's sound, so there is nothing to match. One app's
sound goes through PC audio (section 7). System sound with a share on macOS is left out for now.

## 7. PC audio, music and listen-along

| Feature | Today | Native |
| --- | --- | --- |
| **Listen-along** (`mega-listen.js`) | Not a call feature. The core makes this person's own player (rune or Spotify) play the friend's song in step. The UI is a note under a person's picture | The same `listen_*` core calls from the People view. No media code. Belongs with the People work, not here |
| **PC audio, one app** (`call-pcaudio.js` route 2, `crates/core/src/pcaudio.rs`) | The core makes a virtual PipeWire source ("Megatron PC audio") and moves one app's stream onto it; the page opens that source as an input | The core part is unchanged (`pcaudio_caps`, `pcaudio_streams`, start, stop, heartbeat). The app reads the virtual source with a fixed-argument `parec --device=megatron_pcaudio_src --format=s16le --rate=48000 --channels=2 --raw` child process, the way the core already drives `pactl` and `pw-loopback`, and feeds 10 ms frames to a 2-channel `NativeAudioSource` with all processing off. Published as `name: "music-audio"`, `source: ScreenshareAudio`, `audio_encoding: 192_000`, `dtx: false`. It is the call's `play` state, so the "one thing at a time" rule and the Stop control apply |
| **PC audio, a browser tab** (route 1) | `getDisplayMedia` audio | Gone with the browser. Route 2 covers it on Linux. macOS has no equivalent until ScreenCaptureKit audio is done |
| **The `music` picture** | A 1280x720 canvas with title and cover at 2 fps, published as `name: "music"`, `source: Screenshare` | A plain card rendered once into an I420 buffer and pushed at 2 fps, so web viewers see the tile they expect |
| **Hearing others' music** | An ordinary subscribed audio track | The same; it plays through the device module with its own volume |
| **Music-reactive backdrop** (`mega-music.js`) | An analyser on the music track, connected to nothing | A `NativeAudioStream` on the same remote track beside normal playback, bass energy and onsets computed in Rust. That a tap and playback work together is to be confirmed in step 8 |
| **Play into the call** (a file, rune, Jellyfin; `PLAY_PROFILES`) | The browser decodes and captures a `<video>` | Needs a decoder (ffmpeg or GStreamer) and A/V sync. **Not planned in steps 0 to 9**; see the questions |

Unverified: whether the Rust SDK sends two-channel Opus as stereo. No stereo flag appears in its publish path.
Step 8 measures it from a Chromium listener; if it arrives mono, the fix is a small addition to patch L1's crate.

## 8. UI and state

### Shape

- `app/gpui/src/calls/`, behind a cargo feature `calls` (on by default on Linux and macOS) so the rest of the port
  still builds without libwebrtc.
- `CallEngine` owns one tokio runtime (2 worker threads) for LiveKit. The UI sends `CallCommand`s over a channel
  and reads a `CallSnapshot` (phase, error, peers, tiles, muted, deafened, sharing, camera, key state, attempt),
  the same fields as `snapshot()` in call.js. Changes are coalesced to one notification per 60 ms.
- Core calls (`call_state`, `call_token`, `game_mute_wait`, `live_*`, `ring_*`) are blocking. They go through one
  dedicated thread, never a pool: SPIKE.md found a pool keeps a heap per thread.
- Video frames do not go through the snapshot. Each tile has its own frame slot (section 5).

### Behaviour, ported from call.js with the same constants

| Thing | Rule |
| --- | --- |
| Join | `call_state` and `call_token` together, 15 s limit. Keys set, then connect with `auto_subscribe: false`. Then review and subscribe the people already there **before** opening the microphone, so hearing does not wait on the mic. A server that cannot be reached gets retries after 0.7, 1.5, 3 and 5 s before the person is told |
| Heartbeat | `call_state` every 1.5 s for as long as the call lasts. Besides key changes this keeps `call_active` true, which the updater waits on |
| Keys | Re-applied when the epoch or the member count changes, and again for a participant when it appears or one of its tracks is subscribed (`_rekey`). Old slots wiped after 3 s. Section 2's rules |
| Removed from the room | `call_state` failing with "no longer a member", "not joined" or "out of sync" ends the call at once with the `removed` notice |
| Leave | Stop the poll and the game-key watch, unpublish, close the room, drop the key provider |
| Mute, deafen, push-to-talk, game keys | Section 4 |
| Errors | The kinds in call.js. Passing ones (`screen`, `no-screen`, `busy`, `device`, `play`) fade after 8 s; lasting ones (`network`, `removed`, `unsupported`) stay |
| Who is in a call | Already in the port (`presence.rs`, `call_presence`). The engine adds nothing: the relay reads LiveKit. The engine only publishes the `speaking` and `deaf` attributes |
| Rings (`mega-ring.js`) | `ring_poll` with `since`, a card with join and decline, gone after 30 s or when answered; a tone every 2.6 s unless sound is off; `ring_send` and `ring_end` from the dock and from People. Join is the ordinary join. The tone is the app's notification sound path, not the call's |
| Direct calls | `direct_call` gives a room; from there it is the same join |
| Tile order | Section 5, over the core's live channel |
| `open_call_in_browser` | Stays as a button until step 3 is accepted, then goes |

### Reconnects

call.js lets the SDK try one quick resume and forbids it a full reconnect, because a republish into the old
encryption pipeline broke decryption for some pairs; it rebuilds the room itself. The Rust SDK does full
reconnects on its own and republishes tracks (`handle_restarted`), and it has no reconnect policy option that was
found. Plan:

- On `RoomEvent::Reconnecting`: phase `reconnecting`. If `Reconnected` follows within 3 s, apply rule 3 to every
  local track, poll keys at once and carry on.
- Otherwise, or on `Disconnected`, or when the media path has been down for 7 polls (about 10 s): close the room
  and build a fresh one with a new key provider, reusing the token if it has more than 60 s left. Waits between
  attempts 0.4, 0.4, 0.8, 1, 1.5, 2 s, then 5 s after a minute, giving up after 10 minutes. A refused token (401,
  403) is not reused. "Not a member" ends the call.
- Patch L2 makes the SDK's own republish safe in the meantime.

## 9. Interop with web and iOS clients

Nothing on the wire changes:

- The same tokens, room names and identities; the same key export, index rule and rotation timing.
- The same track sources and names (`music`, `music-audio`), codec (VP8, Opus), simulcast layout and attributes
  (`speaking`, `deaf`).
- Tile order and rings use the core's channels, which the native app calls through the same core.
- No LiveKit data messages, as today (the token does not allow them).

The core and the relay need **no change** for any step below. Each step's acceptance test has a Chromium
participant running the unmodified `app/ui` in the same call.

## 10. Steps for the implementing agent

Estimates are working time for one agent. Steps 3, 5, 6 and 9 also need a person for a few minutes each, named in
the step. All automated tests are headless and silent: the existing harness (`tools/lib/env.mjs`, `world.mjs`:
local LiveKit, relay, bridges, Chrome for Testing with ANGLE on Vulkan and fake devices), the private KWin from
`bench/hl-start.sh` for anything with a window, and `MEGA_GPUI_CALL_AUDIO=none`. Heavy cargo runs go under
`systemd-run --user --scope -p MemoryMax=8G` with `-j 8`. Use `COMMONS_TEST_PORT_OFFSET` so a run never meets the
user's own relay or LiveKit.

### Step 0. Encryption proof as a permanent test (1.5 to 2.5 days)

The core of it passed today. This step makes it repeatable and closes the gaps in section 2.

- Files: `app/gpui/examples/call_probe.rs` (from `~/data/agent-tmp/mega-calls/probe/src/main.rs`, driven by JSON
  lines on stdin so a test can ask it to rotate, share and leave); `tools/gpui-call-interop.mjs` (from
  `harness/interop.mjs`).
- Pass, measured exactly as today:
  1. Each direction, audio: share of energy at the sender's tone above 0.15 (the suite's `TONE_MIN`).
  2. Each direction, video: frames keep arriving for 5 s; in the browser `currentTime` advances and the centre
     pixel is within 12 of the sent colour per channel; in Rust the frame count rises and the size is right.
  3. No decrypt errors on either side in the good run; both sides see the other's tracks marked encrypted.
  4. The three controls fail as in section 2 (wrong key, index 0, PBKDF2).
  5. Rotation: Carol joins the room during the call (new epoch). Within 5 s both directions pass again with the
     new index, and after the 3 s grace Rust's `get_key` for the old slot no longer returns the old key.
  6. Removal: Carol is removed. With trust forced on in her page (`_testTrustAll`, as `call-e2e.mjs` does) she
     hears nothing from the Rust participant: tone share below 0.02.
  7. Rust publishes a screen-share-shaped track (1440p plus the 540p layer). The browser plays it at both
     `setVideoQuality(HIGH)` and `LOW` with the right sizes.
  8. LiveKit is killed and restarted (`ctl.stopLivekit`, `startLivekit`). Both directions pass again within 10 s
     of the server being back.
- If 1 to 4 ever fail after an SDK upgrade, stop and do not upgrade.

### Step 1. Landing the dependency (1.5 to 2.5 days)

- Files: `app/gpui/Cargo.toml` (feature `calls`; `livekit = { version = "=0.11.0", features =
  ["rustls-tls-native-roots", "glib-main-loop"] }`), `app/gpui/vendor/` with patches L1, L2 and W1 and the script
  that rebuilds the vendored crates from crates.io, the third-party licence file, `app/gpui/src/calls/mod.rs`
  (empty engine), a bench entry in `bench.rs`.
- Accept:
  - `cargo tree` shows no runtime crate of the Tauri app or the core changed version; otherwise `app/gpui` gets its
    own workspace. The Tauri app still builds.
  - Step 0 passes on the patched crates. A test shows the ICE hook removes a Google STUN entry and rewrites a
    `turns:` port; a test shows a newly published track's cryptor already has the right index (L2); a test sets a
    remote track's volume to 0 and measures silence on a tap, then 1 and measures the tone (W1). If W1's tap does
    not reflect the gain, measure through a private PipeWire null sink instead.
  - **Render bench:** a window in the private KWin draws synthetic 2560x1440 frames at 30 fps in one tile through
    the image path for 5 minutes. Report CPU of the process, frame intervals and memory. Pass: no frame over 40 ms
    at p99, memory flat (within 20 MB from minute 1 to minute 5), and CPU under 35 % of one core on this machine.
    If it fails, decide between scaling, a lower rate for large tiles, and the GPUI surface patch **before** step 4.
  - Release binary growth reported.

### Step 2. The call engine, no window (4 to 6 days)

- Files: `calls/engine.rs` (state machine, commands, snapshot), `calls/keys.rs`, `calls/review.rs` (who may be
  heard), `calls/reconnect.rs`, `calls/gate.rs`, `calls/errors.rs`, `calls/testhook.rs` (a line-JSON control port
  enabled only by `MEGA_GPUI_CALL_TEST`), unit tests beside each.
- Accept, with two Chromium participants and one `mega-gpui --call-headless` in one room (`tools/gpui-call-e2e.mjs`,
  modelled on `call-e2e.mjs`):
  - Join, hear and be heard; leave; join again. The relay's `call_presence` shows the native device, its mute
    state, `speaking` and `deaf`.
  - A device that is not a member (a token minted with `operatorToken`) is never subscribed and is listed as
    blocked. An unencrypted publication is never subscribed.
  - Rotation and removal as in step 0, driven through the engine.
  - `call_active` in the core stays true during the call and goes false within 5 s of leaving.
  - Server restart: back within 10 s. A 20 s outage: the engine keeps trying and recovers. Removal from the room
    during an outage ends the call with `removed`.
  - Gate unit tests ported from `tools/mic-gate.test.mjs` pass. A simulated game key mutes and releases without
    overriding a manual mute.

### Step 3. Voice in the app (4 to 6 days, plus a person for 10 minutes)

- Files: `calls/audio.rs` (`PlatformAudio`, devices, processing, volume), `calls/dock.rs` (mute, deafen, leave,
  devices, the lock that explains what the server sees), `calls/settings.rs`, the voice room view (what
  `mega-voice.js` shows), prefs.
- Accept, automated: in the private KWin the dock's buttons drive the engine (real pointer events) and a
  screenshot shows each state; with a private PipeWire holding a null sink and a tone source, the app's microphone
  is heard by Chromium and Chromium's tone reaches the null sink's monitor; switching to a second null device
  during the call moves the sound within 2 s; volume 0 for one account silences only that account; a left-only
  tone from Chromium's music track arrives left-only (stereo) or the result is recorded as mono.
- A person: one real call between this machine on speakers and a phone. Is there echo? Is the voice natural with
  noise suppression on and off? This is the acceptance for echo cancellation; no test can stand in for it.
- After this step the hand-off to the browser can be removed on Linux.

### Step 4. Watching cameras and screens (4 to 6 days)

- Files: `calls/video.rs` (frame slots, scaling, conversion), `calls/tiles.rs` (the tile entity), `calls/deck.rs`
  (order protocol), the column in the room view, prefs for the width.
- Accept: Chromium publishes a camera and the fake screen; the app shows two tiles (screenshot) and a pixel read
  from the screenshot matches the fake screen's colour blocks; mute, deafen and volume changes do not recreate
  tiles (entity ids unchanged, no blank frame in a 50 ms sampler over 10 s, the counterpart of test J2); a camera
  switched off removes its tile and leaves the screen tile alone; dragging the edge changes the width and it
  survives a restart; a drag in the app reorders Chromium's tiles and the reverse, and two simultaneous drags
  converge; `deck.rs` unit tests cover stamps, holds and unknown tiles; CPU with one 1440p30 share in a column
  tile and enlarged, both reported.

### Step 5. Sharing a screen on Linux (4 to 7 days, plus a person for 5 minutes)

- Files: `calls/share.rs` (profiles, publishing), `calls/capture_linux.rs`.
- Accept, automated: a unit test on `compute_video_encodings` for each profile (two encodings, the 540p scale and
  bitrates); with a synthetic frame source in place of the capturer, a Chromium viewer's `getStats` shows 2560x1440
  near 30 fps on HIGH and 960x540 on LOW for sharp text, and the motion profiles likewise; with a 3 Mbit/s cap on
  the sender (as the 0.3.11 measurement did) the viewer still gets the full size; stopping the share removes the
  tile everywhere; a share and "play" refuse each other.
- Capture itself: inside the private KWin, if its portal can be driven without a picker; otherwise a person picks
  a screen once on the desktop. A person also judges text sharpness at 1440p against the 0.3.11 browser share and
  reports the frame rate of the fast motion profile.

### Step 6. Camera (2 to 3 days, plus a person for 5 minutes)

- Files: `calls/camera.rs`.
- Accept: `nokhwa` builds on Linux; with a synthetic source the three layers reach Chromium at the sizes in
  section 5; camera off mutes and the tile goes; a person checks a real webcam and the device picker.

### Step 7. Rings, direct calls and the one-thing rule (2 to 3 days)

- Files: `calls/ring.rs`, People and dock buttons.
- Accept: a ring from Chromium shows a card in the app within 2 s (screenshot) and joining from it works; a ring
  from the app shows in Chromium; a card goes after 30 s and when the caller ends it; no tone when sound is off
  (the notification file hook records the attempt); a direct call from People connects both ways.

### Step 8. PC audio and music (3 to 4 days)

- Files: `calls/pcaudio.rs`, `calls/music.rs` (the card picture, the backdrop tap).
- Accept: in a private PipeWire, a tone played by a test process is shared through the core's virtual source and
  Chromium hears it at music quality (tone share above 0.15, and stereo if step 3 found stereo works); the user's
  default devices are unchanged before and after (the core's own check); stopping, leaving and killing the app each
  end the routing (the core's heartbeat); the `music` tile appears in Chromium; music from Chromium moves the
  backdrop's energy value while voices do not.

### Step 9. macOS (5 to 8 days, needs a Mac and a person)

- Files: `calls/capture_macos.rs`, the bundle's Info.plist strings for microphone, camera and screen recording.
- Accept: the app builds on macOS with the prebuilt libwebrtc; step 2's suite passes against a Chromium
  participant; a person checks the three permission prompts, a call with echo cancellation, a screen share through
  the system picker and a camera.

### Not scheduled

- Play into the call (files, rune, Jellyfin): 1 to 2 weeks, mostly a decoder and A/V sync.
- System sound with a screen share on macOS.

### Totals

| | Agent time |
| --- | --- |
| Steps 0 to 2 (the risk: encryption, patches, engine, reconnects) | 1.5 to 2.5 weeks |
| Steps 3 and 4 (a usable call: voice and watching) | 1.5 to 2.5 weeks |
| Steps 5 to 8 (sharing, camera, rings, PC audio) | 2 to 3.5 weeks |
| **Linux, matching 0.3.11 without play-into-call** | **about 5 to 7 weeks** (the ranges do not all land on their upper ends) |
| Step 9, macOS | 1 to 1.5 weeks |

## 11. Risks

| Risk | How likely | What then |
| --- | --- | --- |
| An SDK upgrade changes the frame format or a default | low, but LiveKit releases weekly | exact pins; step 0 is the gate for any upgrade |
| Reconnect and rotation at the same moment leave one pair unable to decode | medium; call.js fought exactly this | rebuild the room instead of trusting the SDK's reconnect; step 0 items 5 and 8 and step 2's outage tests |
| Echo cancellation is worse than the browser's | medium; same library family as Chromium, different device layer | only a person can judge; the three switches are there; headphones always work |
| The image path is too slow for large video on weaker GPUs | medium on integrated graphics, low here | measured in step 1; scaling to tile size first; the GPUI surface patch as a last resort |
| The portal capturer cannot hold 60 fps or 1440p cheaply | medium | `ashpd` plus PipeWire with DMA-BUF |
| Screencast mode in libwebrtc changes the two-layer layout | medium | `is_screencast: false` with maintain-resolution |
| `nokhwa` does not build or misbehaves | medium, unprobed | the portal camera on Linux, AVFoundation code on macOS |
| Stereo music is sent or played as mono | unknown | measured in steps 3 and 8; a small patch on the sending side |
| Carrying three patches over a fast-moving SDK | certain cost, small | patch files and a script; L2 and W1 offered upstream |
| `livekit` disturbs the workspace lock file | medium | `app/gpui` as its own workspace |
| macOS is entirely unbuilt | certain | step 9 is last and has its own estimate |
| The production LiveKit server is older than the local 1.13.11 and dislikes the single peer connection | low; the SDK falls back | set `single_peer_connection: false` if a join fails against it |

## 12. Questions for the user

1. **Play into the call** (a video file, rune, Jellyfin) is the one 0.3.11 call feature this plan does not
   schedule. Keep it out until the rest works, or is it needed before the browser edition goes?
2. **Patched SDK crates.** Three small patches are carried in the repository. Acceptable, or should the first
   release go without per-person volume (W1) to carry only two?
3. **Google and Twilio STUN.** Patch L1 keeps call.js's rule of never contacting them. Confirm that rule still
   stands; without it one patch goes.
4. **PC audio of a browser tab** disappears with the browser edition; on Linux "one app" covers it, on macOS
   nothing does for now. Acceptable?
5. **Which LiveKit server version runs in production?** Step 0 used 1.13.11 from `tools/bin`.
6. **Push-to-talk** will only work while the mega window has focus (game keys cover the rest through the core).
   Is that enough?
7. **A Mac for step 9**, and a person for the short checks in steps 3, 5 and 6: when are they available?
8. **Hardware encoding.** The SDK can use NVIDIA or VAAPI encoders, but only for H.264, which every viewer would
   have to decode through the same encryption path. This plan stays on VP8 like 0.3.11. Revisit later?

## Where the evidence is

Everything is under `~/data/agent-tmp/mega-calls/`, outside the repository:

- `probe/`: the scratch crate. `src/main.rs` is the Rust participant, `src/bin/facts.rs` the device listing and
  conversion timing.
- `harness/interop.mjs`: the run of section 2. `run1/result.json`: its results.
- `ref/frame_crypto_transformer.cc`: the C++ source that was read.
- `logs/`: build logs.
