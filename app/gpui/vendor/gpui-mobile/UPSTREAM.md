# gpui-mobile (vendored for mega's iPhone build)

From https://github.com/longbridge/gpui-mobile at 7548ebc ("Upgrade gpui-pre to 0.3.10 and Android host fixes"),
used under its Apache-2.0 option (LICENSE-APACHE).

mega's changes:
- gpui-pre / gpui-pre-wgpu pinned to =0.3.12 (mega's gpui-kit 0.9.1 pins that version).
- `WgpuContext::new` takes the surface config since gpui-pre-wgpu 0.3.11 (src/ios/window.rs; the Android call is left as upstream wrote it, since mega only builds iOS).
- The `offscreen` feature no longer turns on the vendored gpui-pre-wgpu patch: mega never enables `offscreen`.
- libc no longer pinned to =0.2.189: 0.2.191 (the workspace's) fixed the iOS `_dyld_*` declarations.
- Built as an rlib only; the profiles and the `[patch]` section were dropped (the workspace root owns those).
