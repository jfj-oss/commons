//! Pictures into the window (CALLS-PLAN.md section 5, "Drawing remote video"): one frame sink per subscribed remote video
//! track (a camera or a screen share), and the local preview of what this client publishes.
//!
//! The contract with the window is [`Frames::push`] (video.rs): one BGRA `RenderImage` per tile, only the newest kept.
//!
//!  * **Remote**: [`Sink::start`] reads the track with the SDK's `NativeVideoStream` (queue of one frame: a frame the
//!    reader has not taken yet is replaced, so a slow window never builds a queue), converts I420 to BGRA with libyuv and
//!    pushes it under the track's sid. A tile that is hidden ([`Frames::set_hidden`]) costs nothing: its frames are read and
//!    thrown away without conversion. Dropping the [`Sink`] (track gone, call over) ends the task and forgets the tile.
//!  * **Local**: [`Preview`] is what the share and camera tasks call with the picture they are about to publish. It is the
//!    same I420 buffer, not a second capture and no re-encode. The conversion is done once per frame, and only when the
//!    tile is not hidden; a picture larger than [`PREVIEW_MAX`] is scaled down first (a tile is never that big).
use crate::video::{bgra_image, Frames};
use futures::StreamExt;
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use livekit::prelude::RemoteVideoTrack;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The biggest picture a local preview is converted at.
pub const PREVIEW_MAX: (u32, u32) = (1280, 720);

/// An I420 picture as the BGRA bytes of a tile (libyuv "ARGB" is B, G, R, A in memory). `None` for an empty picture.
pub fn i420_to_bgra(b: &I420Buffer) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (b.width(), b.height());
    if w == 0 || h == 0 { return None; }
    let (y, u, v) = b.data();
    let (sy, su, sv) = b.strides();
    let mut out = vec![0u8; w as usize * h as usize * 4];
    yuv_helper::i420_to_argb(y, sy, u, su, v, sv, &mut out, w * 4, w as i32, h as i32);
    Some((w, h, out))
}

/// A running remote reader. Its drop ends it.
pub struct Sink { task: tokio::task::JoinHandle<()>, sid: String, frames: Arc<Frames>, pub received: Arc<AtomicU64> }
impl Sink {
    /// Must be called inside the call engine's tokio runtime.
    pub fn start(frames: Arc<Frames>, sid: String, track: RemoteVideoTrack) -> Sink {
        let received = Arc::new(AtomicU64::new(0));
        let (f2, s2, r2) = (frames.clone(), sid.clone(), received.clone());
        let task = tokio::spawn(async move {
            // the SDK's default queue is one frame: a frame nobody took is replaced
            let mut stream = NativeVideoStream::new(track.rtc_track());
            while let Some(frame) = stream.next().await {
                r2.fetch_add(1, Ordering::Relaxed);
                if f2.is_hidden(&s2) { continue; }
                let i420 = frame.buffer.to_i420();
                // the conversion of a big picture is the one expensive thing here: do it off the async worker
                let (f3, s3) = (f2.clone(), s2.clone());
                let _ = tokio::task::spawn_blocking(move || {
                    if let Some((w, h, px)) = i420_to_bgra(&i420) { if let Some(img) = bgra_image(w, h, px) { if !f3.is_hidden(&s3) { f3.push(&s3, img); } } }
                }).await;
            }
        });
        Sink { task, sid, frames, received }
    }
}
impl Drop for Sink {
    fn drop(&mut self) { self.task.abort(); self.frames.forget(&self.sid); }
}

/// The local preview of one published track (camera or screen). Cheap to clone.
#[derive(Clone)]
pub struct Preview { frames: Arc<Frames> }
impl Preview {
    pub fn new(frames: Arc<Frames>) -> Preview { Preview { frames } }
    /// A picture that is being sent; call with the same buffer that goes to the encoder.
    pub fn push(&self, sid: &str, b: &mut I420Buffer) {
        if self.frames.is_hidden(sid) { return; }
        let (w, h) = (b.width(), b.height());
        let (pw, ph) = super::capture::fit(w, h, PREVIEW_MAX.0, PREVIEW_MAX.1);
        let shown = if (pw, ph) != (w, h) { Some(b.scale(pw as i32, ph as i32)) } else { None };
        if let Some((w, h, px)) = i420_to_bgra(shown.as_ref().unwrap_or(b)) { if let Some(img) = bgra_image(w, h, px) { self.frames.push(sid, img); } }
    }
    pub fn forget(&self, sid: &str) { self.frames.forget(sid); }
    /// The picture goes but the tile's counters stay (a camera switched off, shown again later).
    pub fn clear(&self, sid: &str) { self.frames.clear(sid); }
}
