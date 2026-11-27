//! The desktop app; everything lives in the library (lib.rs) so the iPhone build (app/ios) can share it.
#[cfg(not(target_os = "ios"))]
fn main() { mega_gpui::desktop_main() }
#[cfg(target_os = "ios")]
fn main() {}
