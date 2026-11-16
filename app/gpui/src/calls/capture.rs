//! Where the pictures of a screen share come from (CALLS-PLAN.md section 6). Everything here turns into one thing, an
//! [`Item`] stream of I420 frames already fitted to the profile's ceiling, so the publishing code in `share.rs` never
//! knows which source it has:
//!
//!  * [`ShareSource::Portal`]: the SDK's desktop capturer through xdg-desktop-portal ScreenCast and PipeWire
//!    (`capture_linux.rs`). The portal shows its own picker; the person chooses a screen or a window there.
//!  * [`ShareSource::Test`]: a synthetic, scrolling 1440p text pattern, only for the automated tests (the portal needs a
//!    person). It goes through the same frame conversion, fitting and publish path as the portal's frames.
//!
//! A capture runs on a thread of its own (libwebrtc's capturer is not `Send`), paced at the profile's frame rate, and
//! hands frames over a small channel that drops the oldest when the publisher is slow.
use super::share::Profile;
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// What to share. `Portal` lets the system's own picker choose; `Test` is the synthetic source for the test suites.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShareSource {
    Portal,
    Test(TestPattern),
}
impl ShareSource {
    pub fn name(&self) -> &'static str { match self { ShareSource::Portal => "portal", ShareSource::Test(_) => "test" } }
}

/// The test source: a screen of text-like glyphs, `width` x `height`, scrolling `speed` rows per frame. Speed 0 is the
/// calm variant: the page stands still and only a 96-pixel block moves (what an editor with a blinking cursor and a
/// video thumbnail looks like to an encoder), so the layers' frame rates are not held back by a scrolling page's bit cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TestPattern { pub width: u32, pub height: u32, pub speed: u32 }
impl Default for TestPattern { fn default() -> Self { TestPattern { width: 2560, height: 1440, speed: 3 } } }

/// What a capture reports.
pub enum Item {
    /// One picture, already fitted to the limits and converted.
    Frame(I420Buffer),
    /// The capture is over. `msg` says why; `system` is true when the system or the person ended it (the portal's
    /// session closed), false when it could not run at all.
    Ended { msg: String, system: bool },
}

/// What the profile allows, read by the capture thread for every frame, so a profile change needs no new capture
/// (which for the portal would mean a new picker).
#[derive(Clone, Copy, Debug)]
pub struct Limits { pub max_w: u32, pub max_h: u32, pub fps: f64 }
impl Limits { pub fn of(p: Profile) -> Limits { let s = p.spec(); Limits { max_w: s.width, max_h: s.height, fps: s.fps } } }

/// A running capture.
pub struct Capture {
    pub rx: Receiver<Item>,
    stop: Arc<AtomicBool>,
    die: Arc<AtomicBool>,
    limits: Arc<Mutex<Limits>>,
}
impl Capture {
    pub fn set_limits(&self, l: Limits) { *self.limits.lock() = l; }
    /// Tests: makes the capture end as if the portal's session had been closed from the system.
    pub fn die(&self) { self.die.store(true, Ordering::SeqCst); }
    /// Asks the thread to finish; it closes the portal session on its way out.
    pub fn stop(&self) { self.stop.store(true, Ordering::SeqCst); }
}
impl Drop for Capture { fn drop(&mut self) { self.stop(); } }

/// What a capture thread gets to work with.
#[derive(Clone)]
pub struct Ctx {
    pub tx: Sender<Item>,
    pub stop: Arc<AtomicBool>,
    pub die: Arc<AtomicBool>,
    pub limits: Arc<Mutex<Limits>>,
}
impl Ctx {
    /// Frames are dropped when the publisher is behind: a screen share wants the newest picture, never a queue.
    pub fn frame(&self, b: I420Buffer) { let _ = self.tx.try_send(Item::Frame(b)); }
    pub fn ended(&self, msg: impl Into<String>, system: bool) { let _ = self.tx.blocking_send(Item::Ended { msg: msg.into(), system }); }
    pub fn limits(&self) -> Limits { *self.limits.lock() }
    pub fn stopped(&self) -> bool { self.stop.load(Ordering::SeqCst) }
}

pub fn start(source: ShareSource, limits: Limits) -> Capture {
    let (tx, rx) = channel(3);
    let (stop, die) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let limits = Arc::new(Mutex::new(limits));
    let ctx = Ctx { tx, stop: stop.clone(), die: die.clone(), limits: limits.clone() };
    let name = format!("mega-share-{}", source.name());
    let r = std::thread::Builder::new().name(name).spawn(move || match source {
        ShareSource::Test(p) => run_test(ctx, p),
        ShareSource::Portal => portal(ctx),
    });
    if let Err(e) = r { log::warn!("share: capture thread: {e}"); }
    Capture { rx, stop, die, limits }
}

/// A capture that runs `f` on a thread of its own with this capture's channel and flags: the camera's source goes this way
/// (`camera.rs`), and so does anything else that produces fitted I420 pictures.
pub fn start_with(name: &str, limits: Limits, f: impl FnOnce(Ctx) + Send + 'static) -> Capture {
    let (tx, rx) = channel(3);
    let (stop, die) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let limits = Arc::new(Mutex::new(limits));
    let ctx = Ctx { tx, stop: stop.clone(), die: die.clone(), limits: limits.clone() };
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(move || f(ctx)) { log::warn!("capture thread {name}: {e}"); }
    Capture { rx, stop, die, limits }
}

#[cfg(target_os = "linux")]
fn portal(ctx: Ctx) { super::capture_linux::run(ctx) }
#[cfg(not(target_os = "linux"))]
fn portal(ctx: Ctx) { ctx.ended("Screen sharing is not built for this system yet.", false); }

// ------------------------------------------------------------------------------------------------ frames

/// The size a picture is sent at: shrunk to fit inside the ceiling keeping its shape, never enlarged, both sides even.
pub fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w == 0 || h == 0 { return (2, 2); }
    let s = (max_w as f64 / w as f64).min(max_h as f64 / h as f64).min(1.0);
    let (fw, fh) = if s >= 1.0 { (w, h) } else { ((w as f64 * s).round() as u32, (h as f64 * s).round() as u32) };
    ((fw & !1).max(2), (fh & !1).max(2))
}

/// BGRA bytes in memory (libwebrtc's desktop frames, libyuv calls that layout "ARGB") to I420 at the fitted size.
pub fn bgra_to_i420(data: &[u8], stride: u32, w: u32, h: u32, l: Limits) -> Option<I420Buffer> {
    let (cw, ch) = (w & !1, h & !1);
    if cw < 2 || ch < 2 || (data.len() as u64) < stride as u64 * (ch as u64 - 1) + cw as u64 * 4 { return None; }
    let (fw, fh) = fit(cw, ch, l.max_w, l.max_h);
    let mut full = I420Buffer::new(cw, ch);
    {
        let (sy, su, sv) = full.strides();
        let (y, u, v) = full.data_mut();
        // the assert inside wants a whole last row of stride bytes; a capture buffer may end at the last pixel
        let need = stride as usize * ch as usize;
        if data.len() >= need {
            yuv_helper::argb_to_i420(data, stride, y, sy, u, su, v, sv, cw as i32, ch as i32);
        } else {
            let mut padded = data.to_vec(); padded.resize(need, 0);
            yuv_helper::argb_to_i420(&padded, stride, y, sy, u, su, v, sv, cw as i32, ch as i32);
        }
    }
    if (fw, fh) == (cw, ch) { Some(full) } else { Some(full.scale(fw as i32, fh as i32)) }
}

/// An I420 picture fitted inside the limits (shrunk keeping its shape, never enlarged).
pub fn fit_i420(mut b: I420Buffer, l: Limits) -> I420Buffer {
    let (w, h) = (b.width(), b.height());
    let (fw, fh) = fit(w, h, l.max_w, l.max_h);
    if (fw, fh) == (w, h) { b } else { b.scale(fw as i32, fh as i32) }
}

// ------------------------------------------------------------------------------------------------ the test source

/// A page of text-like lines: dark glyph blocks of varying width and height on white, in rows 28 pixels tall. Deterministic.
/// `extra` more rows below the screen's height are what the scroll slides over.
pub fn test_page(w: u32, h: u32, extra: u32) -> Vec<u8> {
    let (w, rows) = (w as usize, (h + extra) as usize);
    let mut px = vec![255u8; w * rows * 4];
    let mut s = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; s };
    let mut line = 0;
    while line * 28 + 28 <= rows {
        let y0 = line * 28 + 6;
        let mut x = 40usize;
        let tint = (line % 7) as u8 * 6;
        while x + 40 < w {
            let gw = 5 + (next() % 9) as usize;
            let gh = 8 + (next() % 13) as usize;
            if next() % 6 != 0 {   // a space now and then
                for yy in 0..gh.min(22) { let row = y0 + 22 - gh.min(22) + yy; if row >= rows { break; }
                    for xx in 0..gw { let o = (row * w + x + xx) * 4; px[o] = 20 + tint; px[o + 1] = 20; px[o + 2] = 24; px[o + 3] = 255; } }
            }
            x += gw + 3 + (next() % 3) as usize;
        }
        line += 1;
    }
    px
}

/// The still page with a block of red sliding along the bottom.
fn calm_frame(page: &[u8], w: usize, h: usize, n: u64) -> Vec<u8> {
    let mut f = page.to_vec();
    let x0 = (n as usize * 11) % (w - 96);
    for y in h - 120..h - 24 { for x in x0..x0 + 96 { let o = (y * w + x) * 4; f[o] = 30; f[o + 1] = 30; f[o + 2] = 224; } }
    f
}

fn run_test(ctx: Ctx, p: TestPattern) {
    const EXTRA: u32 = 2000;
    let (w, h) = (p.width.max(64) & !1, p.height.max(64) & !1);
    let page = test_page(w, h, EXTRA);
    let stride = w as usize * 4;
    let t0 = std::time::Instant::now();
    let mut n: u64 = 0;
    loop {
        if ctx.stopped() { return; }
        if ctx.die.load(Ordering::SeqCst) { ctx.ended("the capture ended (test)", true); return; }
        let l = ctx.limits();
        let off = ((n * p.speed as u64) % EXTRA as u64) as usize;
        let slice = &page[off * stride..(off + h as usize) * stride];
        let framed;
        let slice = if p.speed == 0 { framed = calm_frame(slice, w as usize, h as usize, n); &framed[..] } else { slice };
        if let Some(b) = bgra_to_i420(slice, stride as u32, w, h, l) { ctx.frame(b); }
        n += 1;
        let due = t0 + std::time::Duration::from_secs_f64(n as f64 / l.fps.max(1.));
        let now = std::time::Instant::now();
        if due > now { std::thread::sleep(due - now); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use livekit::webrtc::video_frame::VideoBuffer;
    #[test]
    fn fitting_keeps_the_shape_and_never_enlarges() {
        assert_eq!(fit(3840, 2160, 2560, 1440), (2560, 1440));
        assert_eq!(fit(2560, 1440, 1920, 1080), (1920, 1080));
        assert_eq!(fit(1280, 720, 2560, 1440), (1280, 720));
        assert_eq!(fit(5120, 1440, 2560, 1440), (2560, 720));
        assert_eq!(fit(1921, 1081, 2560, 1440), (1920, 1080));
        assert_eq!(fit(1000, 1400, 2560, 700), (500, 700));
    }
    #[test]
    fn a_frame_is_converted_and_scaled() {
        let l = Limits { max_w: 960, max_h: 540, fps: 30. };
        let px = vec![255u8; 1920 * 1080 * 4];
        let b = bgra_to_i420(&px, 1920 * 4, 1920, 1080, l).unwrap();
        assert_eq!((b.width(), b.height()), (960, 540));
        assert!(bgra_to_i420(&px[..100], 1920 * 4, 1920, 1080, l).is_none());
    }
    #[test]
    fn the_test_page_has_text_on_it() {
        let p = test_page(640, 360, 100);
        let dark = p.chunks(4).filter(|c| c[0] < 100).count();
        assert!(dark > 5000 && dark < 640 * 460 / 2, "{dark}");
    }
}
