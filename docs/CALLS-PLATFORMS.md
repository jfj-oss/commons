# Where calls work (voice rooms and screen share)

Calls need four browser features: WebRTC (`RTCPeerConnection`), **frame encryption** (insertable streams, or `RTCRtpScriptTransform`), `getUserMedia`, and for screen share `getDisplayMedia`. Commons checks for them before every call (`CommonsCall.support()` in `app/ui/call.js`) and **refuses to start a call, and says why, if frame encryption is missing**. There is no unencrypted mode.

## Measured on this machine (Arch Linux, 2026-11-06)

| Engine | Result |
| --- | --- |
| **WebKitGTK 2.54.2 (`webkit2gtk-4.1`, what the Tauri desktop shell uses on Linux)** | `RTCPeerConnection` is **undefined**, even with `enable-webrtc` switched on in `WebKitSettings` (tried both ways: before and after creating the view). `RTCRtpScriptTransform` and `createEncodedStreams` are undefined too; `setSinkId` is missing. `getUserMedia`/`getDisplayMedia` exist but are useless without a peer connection. The distro build has no WebRTC. **Calls cannot work inside the Tauri window on this Linux system.** (`tools/webkit-probe.py`, run on GTK's Broadway virtual display, so no window appears on the desktop.) |
| Chrome for Testing 151 (Chromium) | Everything works: WebRTC, insertable streams, getUserMedia, getDisplayMedia, setSinkId. All the call tests run on it. |
| The distro's `chromium` 151 in *headless* mode | WebRTC exists but gathered **no ICE candidates at all** on this machine (so it cannot connect); Chrome for Testing 151 on the same machine does. Not investigated further and not tested with a visible window (the brief forbids opening desktop windows), so whether the normal desktop Chromium works is **unverified**. If it does not, use Chrome for Testing, Chrome, Brave, Edge or Firefox. |

## By platform (what to expect)

| Platform | Tauri window | Notes |
| --- | --- | --- |
| Linux (WebKitGTK) | **No** (measured above) | Use "Open this call in my browser" (below). |
| Windows (WebView2 = Chromium) | Should work (**unverified**: no Windows machine here) | The app needs the microphone permission prompt handled by WebView2; first run asks. |
| macOS (WKWebView) | Possibly (**unverified**) | Safari's engine has WebRTC and `RTCRtpScriptTransform`; the bundle now carries the microphone and camera strings. If it fails in the window, **Open in my browser** in the Call panel is there on every platform. |
| Browser tab (`dev_bridge` or the call page) in Chrome, Edge, Brave, Chromium, Firefox 117+, Safari 15.4+ | Yes where the engine has the four features | Tested: Chromium family only. |

## The route that works everywhere: a browser tab, hand-off from the app

When the web view cannot do calls, the Call panel says so and offers **Open this call in my browser**. The app (Rust core, which already holds the keys) then:

1. starts, once, a loopback-only HTTP server (`crates/core/src/callbridge.rs`) on a random port of `127.0.0.1`;
2. creates a fresh 256-bit one-time secret for this one room and opens `http://127.0.0.1:<port>/callpage.html#s=<secret>&g=<room>` in the default browser (`xdg-open` / `open` / `FileProtocolHandler`);
3. the page (the same `call.js` and `call-ui.js` as the app uses, so identical encryption and controls) removes the secret from the address bar and may call exactly two rpc methods, `call_state` and `call_token`, for that room only. It is served and checked by the core: wrong secret, other room, other method, or a `Host:` header other than `127.0.0.1:<port>` (DNS-rebinding) are refused (`browser_bridge_is_one_room_one_secret_loopback_only`).
4. Opening again revokes the old secret. The tab can read the media keys of that room's current epoch and the call token, nothing else: not messages, not other rooms, not the account key. Closing the tab ends the call on that side.

What this does not protect: other programs running as you on the same computer can reach the loopback port (they need the secret, which is only in the tab's URL fragment, never sent over HTTP), and a browser extension with access to `127.0.0.1` pages could read the keys. Same class of trust as the browser itself.

The panel always shows the link as well, for when no browser appeared. Inside an AppImage the browser is started without the AppImage's library paths, which would otherwise break it.

## The other option: ship a Chromium-based shell

Tauri can run on Chromium (CEF runtime) or the app can move to Electron; then calls work inside the window on Linux too. That is a packaging decision with a large size and update cost (about 150-250 MB per platform and tracking Chromium security releases), so it is not done. The browser hand-off needs none of it, and the call UI is the same code either way.

## Screen share specifics

* The picker is the browser's own (screen, window or tab). On Linux Wayland the picker comes from the desktop portal (PipeWire); on X11 Chromium shows its own.
* Audio with the share: Chromium on Windows and ChromeOS can share system/tab audio; on Linux and macOS only tab audio; Firefox shares no audio; Safari none. The app asks for it where possible and reports "no audio came with the share" when the system did not provide it.
## Reaching the server

Three routes, tried in the browser's usual order: direct UDP, direct TCP, TURN. A friend outside your tailnet on the Funnel setup (docs/DEPLOY.md) has only the last: TURN over TLS on the public port, measured on loopback with the path forced (`tools/hosting-e2e.mjs`, H16). LiveKit always announces that as port 443, so the relay tells clients the real port (`turn_port` in the call token) and `call.js` corrects the address before any connection exists.

* Tested here with synthetic frames from a canvas (never a real screen, per the brief); the browser's own picker and permission UI are therefore **unverified**.
