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

## Done in the spike

- The whole app, calls included, type-checks for aarch64-apple-ios; the desktop build and its 252 tests are unchanged.

## Not yet (in rough order)

1. First run on a phone: does it start, sign in, show the cards, open a room, type with the iOS keyboard.
2. Phone layout: one column (cards, then a room full screen), safe areas (notch, home bar), bigger touch targets;
   copy what mega-touch.js does on the web.
3. Touch versions of hover, right-click (long press) and drag; turn off desktop-only parts on iOS (portal shortcuts,
   PipeWire, V4L2, pop-outs, the updater, the custom title bar).
4. Calls: AVAudioSession, background audio, CallKit.
5. Notifications: APNs needs the paid Apple developer account ($99/year); until then, none.
