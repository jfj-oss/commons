# mega on iPhone (native, GPUI) — spike

The desktop app's own code on the phone, instead of the web view (ios.yml / Tauri). Started 2026-11-25.

## How it fits together

- `vendor/gpui-mobile`: longbridge's gpui-mobile (iOS platform for GPUI: Metal through wgpu, UIKit, CoreText,
  touch as mouse with momentum), moved to gpui-pre 0.3.12. See its UPSTREAM.md.
- `src/lib.rs`: everything that used to be in main.rs; `main.rs` is now one line. `src/ios.rs` is the phone's start:
  fonts, theme, core, then the same `app::Mega` window, full screen.
- `app/ios`: the UIKit host (main.m, an app delegate that starts Rust and drives frames with a CADisplayLink), the
  XcodeGen spec, Info.plist and the icon. Bundle id `app.commons.chat.gpui`, so it installs next to the web-view app.
- `.github/workflows/ios-gpui.yml`: builds the unsigned .ipa on a macOS runner. Install it with SideStore.
- Data lives in the app's `Library/Application Support/app.commons.chat` (core.rs `data_dir`).

## Checking iOS builds on Linux

There is no iOS SDK on Linux, but Rust code can still be type-checked: C and C++ code is replaced by stand-ins that
produce empty objects (never link with this). Set `SDKROOT` to an empty folder holding a stub
`usr/include/TargetConditionals.h`, `CC_aarch64_apple_ios`/`CXX_aarch64_apple_ios` to a script that only touches its
`-o` file, `AR_aarch64_apple_ios` to one that only touches the `.a`, and put an `xcrun` that prints `$SDKROOT` first in
`PATH`. Then `cargo check -p mega-gpui --target aarch64-apple-ios`.

## Done

- Spike: the whole app, calls included, type-checks for aarch64-apple-ios (also after merging 0.3.17); the desktop build is unchanged.
- Phone layout (`src/phone.rs`, a runtime flag): on for the iPhone build, whenever `MEGA_PHONE_PREVIEW=1` is set, and for any window narrower than 600 points.
  - One column: the cards (home, a community's rooms) fill the screen; a room opens full screen with a round back button (to the
    community's cards); the rail is not drawn. The room flight animations that need the rail are skipped.
  - Safe areas: `phone::insets()` reads gpui-mobile's `safe_area_insets()` on iOS (falls back to 47 top / 34 bottom if it reads zero);
    the desktop preview uses those same numbers. `titlebar::frame` pads the page by them; there is no band, top row or traffic lights
    (`Mode::Phone`).
  - Touch targets 44 pt: chips, the composer's + and send, the + menu rows, the message and room menus' rows. Text 17 pt base, chat
    text never below 17. The top bar is two rows (name and settings, then the other chips).
  - Opening a room does not raise the keyboard; the person taps the box.
- Touch: a finger held still for 0.5 s sends a right-click at that point (`Mega::press_begin`; this opens a room's notification menu
  on the cards and a message's menu in a room). A tap on a message's words opens the same message menu (reply, react, edit, delete,
  copy, select all), so the hover-only tools have a tap path. A finger drag scrolls the list (no text selection on a phone; Copy
  message is in the menu). Drawing and the colour panel's drags are mouse drags and work as they are.
- Off on iOS: pop-out and mini windows (and their buttons), global shortcuts (`shortcuts::Mode::Off`), Game keys settings, the
  updater (`update::platform()` has no iOS key), the window frame and title bar. PipeWire screen capture and the V4L2 camera are
  Linux-only code already; there is no tray in the GPUI app.
- Desktop preview: `MEGA_PHONE_PREVIEW=1 cargo run -p mega-gpui` opens a 390 x 844 window in the phone layout (system frame, no custom
  decorations). `node tools/gpui-phone-shots.mjs [outdir]` takes screenshots of it in a private headless KWin (one local relay,
  throwaway account); see the header of that file for the build line.

## Not yet (in rough order)

1. First run on a phone (nothing below has run on a device): does it start, sign in, show the cards, open a room, type with the iOS
   keyboard. Suspects: the keyboard covering the composer (gpui-mobile does not resize the window for it; the page must be lifted by
   the keyboard's height), IME / marked text in the box, Metal surface scale, the fonts (they are embedded; check the glyph
   atlas), and whether touch moves reach `on_mouse_move` with `pressed_button` set (long press and tap use it).
2. Long press is untested on a device. The synthetic right-click is dispatched with `Window::dispatch_event`; if it misbehaves the
   fallback is a per-element handler.
3. The message menu's quick emoji are 33 pt wide on a phone (8 across 284 pt); make it two rows of four at 44 pt.
4. Calls: AVAudioSession (play and record, voice chat), background audio, CallKit. The engine starts as on the desktop and may be
   silent or lose its audio when the app is left (TODO in callui.rs `engine`).
5. Notifications: APNs needs the paid Apple developer account ($99/year); until then, none.
6. Landscape and iPad (the host is portrait only, iPhone only).

## First test on a device

1. Push the branch `ios-gpui` (the push triggers `.github/workflows/ios-gpui.yml`; the repository owner decides when to push) or run
   the workflow by hand once it is on the default branch. Wait for the macOS job (about 30 to 60 minutes the first time, less with
   the cache).
2. Download the `mega-gpui-ipa` artifact (a zip holding `mega-gpui.ipa`) from the run's page.
3. SideStore on the iPhone (already set up for the older web-view app): My Apps, plus, choose the .ipa (AirDrop it or save it to Files).
   It signs with the free Apple ID and installs as "mega" next to the web-view app (bundle id `app.commons.chat.gpui`).
   A free account's apps expire after 7 days; SideStore refreshes them.
4. Open it: it asks to create or restore an account on the first run (same screens as the desktop). Use a throwaway account first.
5. For logs: Console.app on a Mac with the phone attached, or `idevicesyslog | grep mega`; the app logs through NSLog with the
   prefix `mega `, and panics as `mega PANIC`.
