//! The portal capture (CALLS-PLAN.md section 6): the SDK's desktop capturer in `Generic` mode, which on Linux means
//! xdg-desktop-portal ScreenCast (the system's own picker, on KDE or GNOME) and a PipeWire stream. Wayland and X11 both
//! go this way; the screen or window is chosen in the portal's dialog, so there is no list of sources of our own.
//!
//! libwebrtc runs a GLib main loop for the portal's D-Bus calls (the `glib-main-loop` feature of `livekit`, on a thread
//! of its own). The capturer object is not `Send`, so it lives on this thread, which calls `capture_frame` at the
//! profile's rate. Frames come back as BGRA in memory.
//!
//! Ends: the picker cancelled, the portal session closed from the system's "stop sharing" control, or the window gone
//! all arrive as a permanent error; no frame at all within [`PICK_LIMIT`] is reported as a picker nobody answered.
//!
//! Not run by any automated test: the portal needs a person to pick a screen (see CALLS-PLAN.md step 5). The error
//! path without a session bus is tested.
use super::capture::{bgra_to_i420, Ctx};
use livekit::webrtc::desktop_capturer::{CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the portal's picker may stay open before the share is called off.
pub const PICK_LIMIT: Duration = Duration::from_secs(180);

pub fn run(ctx: Ctx) {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() && std::env::var_os("XDG_RUNTIME_DIR").is_none_or(|d| !std::path::Path::new(&d).join("bus").exists()) {
        ctx.ended("Screen sharing needs the desktop's portal (xdg-desktop-portal), and there is no session bus here.", false);
        return;
    }
    let mut o = DesktopCapturerOptions::new(DesktopCaptureSourceType::Generic);
    o.set_include_cursor(true);
    let Some(mut cap) = DesktopCapturer::new(o) else {
        ctx.ended("The screen capture could not be started. Check that xdg-desktop-portal and its KDE or GNOME backend are running.", false);
        return;
    };
    let got = Arc::new(AtomicU64::new(0));
    let permanent = Arc::new(AtomicBool::new(false));
    let (c2, got2, perm2) = (ctx.clone(), got.clone(), permanent.clone());
    cap.start_capture(None, move |r| match r {
        Ok(f) => {
            if let Some(b) = bgra_to_i420(f.data(), f.stride(), f.width().max(0) as u32, f.height().max(0) as u32, c2.limits()) { got2.fetch_add(1, Ordering::Relaxed); c2.frame(b); }
        }
        Err(CaptureError::Temporary) => {}
        Err(CaptureError::Permanent) => perm2.store(true, Ordering::SeqCst),
    });
    let t0 = Instant::now();
    loop {
        if ctx.stopped() { return; }   // dropping `cap` closes the portal session
        if ctx.die.load(Ordering::SeqCst) { ctx.ended("the capture ended (test)", true); return; }
        if permanent.load(Ordering::SeqCst) {
            let started = got.load(Ordering::Relaxed) > 0;
            ctx.ended(if started { "The screen share was ended from the system." } else { "No screen or window was chosen, so nothing is being shared." }, started);
            return;
        }
        if got.load(Ordering::Relaxed) == 0 && t0.elapsed() > PICK_LIMIT { ctx.ended("No screen or window was chosen in time, so nothing is being shared.", false); return; }
        cap.capture_frame();
        std::thread::sleep(Duration::from_secs_f64(1. / ctx.limits().fps.max(1.)));
    }
}
