//! Where a tile's picture comes from, and how it reaches the screen. This is the UI's half of video; the half that reads a
//! subscribed track belongs to the call engine and only has to call [`Frames::push`].
//!
//! The way a frame is drawn is the render bench's (videobench.rs): one BGRA `RenderImage` per frame through the sprite
//! atlas, the newest one only, and the previous one handed back with `Window::drop_image`, which keeps the atlas from
//! growing. A frame that arrives wakes the window; nothing else does, so a tile with no new picture costs nothing.
//!
//! `MEGA_GPUI_FAKE_TILES=<n>` makes n synthetic tiles (moving pictures, 30 per second) appear while a call is up, for
//! checks that must not need a camera: it is the only thing in this file that makes frames by itself.
use gpui_kit::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Camera, Screen }

/// One picture in the column: a camera or a screen share of someone in the call.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile { pub sid: String, pub identity: String, pub name: String, pub kind: Kind }

/// Newest frame per tile, written by whoever reads the track (any thread), taken by the window's render.
pub struct Frames {
    slots: Mutex<HashMap<String, Arc<RenderImage>>>,
    wake: futures::channel::mpsc::UnboundedSender<()>,
    pending: AtomicBool,
}

impl Frames {
    pub fn new() -> (Arc<Frames>, futures::channel::mpsc::UnboundedReceiver<()>) {
        let (wake, rx) = futures::channel::mpsc::unbounded();
        (Arc::new(Frames { slots: Default::default(), wake, pending: AtomicBool::new(false) }), rx)
    }
    /// A new picture for tile `sid`; replaces one that was not shown yet (frames are never queued).
    pub fn push(&self, sid: &str, img: Arc<RenderImage>) {
        self.slots.lock().insert(sid.to_string(), img);
        if !self.pending.swap(true, Ordering::AcqRel) { let _ = self.wake.unbounded_send(()); }
    }
    pub fn forget(&self, sid: &str) { self.slots.lock().remove(sid); }
    /// The window saw the wake-up; the next push wakes it again.
    pub fn ack(&self) { self.pending.store(false, Ordering::Release); }
    pub fn newest(&self, sid: &str) -> Option<Arc<RenderImage>> { self.slots.lock().get(sid).cloned() }
}

/// What a window draws: the picture of each tile, with the one it replaced handed back to the atlas.
#[derive(Default)]
pub struct Shown { cur: HashMap<String, Arc<RenderImage>> }
impl Shown {
    /// The picture to draw for `sid` now. Call from render; it frees the previous image on a change.
    pub fn take(&mut self, f: &Frames, sid: &str, window: &mut Window) -> Option<Arc<RenderImage>> {
        let new = f.newest(sid)?;
        match self.cur.get(sid) {
            Some(old) if Arc::ptr_eq(old, &new) => {}
            Some(_) => { if let Some(old) = self.cur.insert(sid.to_string(), new.clone()) { let _ = window.drop_image(old); } }
            None => { self.cur.insert(sid.to_string(), new.clone()); }
        }
        Some(new)
    }
    pub fn keep_only(&mut self, live: &[String], window: &mut Window) {
        let gone: Vec<String> = self.cur.keys().filter(|k| !live.contains(k)).cloned().collect();
        for k in gone { if let Some(i) = self.cur.remove(&k) { let _ = window.drop_image(i); } }
    }
}

/// Tiles from the engine's `subscribed` list ("<identity>/<track sid>:<Source>").
pub fn tiles_from(subscribed: &[String], name_of: impl Fn(&str) -> String) -> Vec<Tile> {
    let mut v: Vec<Tile> = subscribed.iter().filter_map(|s| {
        let (key, source) = s.rsplit_once(':')?;
        let kind = match source { "Camera" => Kind::Camera, "Screenshare" => Kind::Screen, _ => return None };
        let (identity, sid) = key.split_once('/')?;
        Some(Tile { sid: sid.to_string(), identity: identity.to_string(), name: name_of(identity), kind })
    }).collect();
    v.sort_by(|a, b| a.sid.cmp(&b.sid));
    v
}

pub fn fake_count() -> usize { std::env::var("MEGA_GPUI_FAKE_TILES").ok().and_then(|x| x.parse().ok()).unwrap_or(0) }

/// A moving test picture, `w` x `h`, frame `n`, in a tone that depends on the tile.
pub fn test_frame(w: u32, h: u32, n: u32, tone: u32) -> Arc<RenderImage> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    let (r0, g0, b0) = (60 + (tone * 37) % 120, 60 + (tone * 91) % 120, 90 + (tone * 53) % 120);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let bar = ((x + n * 4) / 40) % 2 == 0;
            let s = if bar { 1.0 } else { 0.78 };
            let d = (y as f32 / h as f32) * 0.4 + 0.6;
            px[i] = (b0 as f32 * s * d) as u8; px[i + 1] = (g0 as f32 * s * d) as u8; px[i + 2] = (r0 as f32 * s * d) as u8; px[i + 3] = 255;
        }
    }
    let img = image::RgbaImage::from_raw(w, h, px).expect("size");
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(img)]))
}

/// Starts the synthetic feeders for `n` fake tiles; they stop when `alive` goes false.
pub fn start_fake(frames: Arc<Frames>, n: usize, alive: Arc<AtomicBool>) {
    for i in 0..n {
        let (f, a) = (frames.clone(), alive.clone());
        let sid = format!("fake{i}");
        std::thread::Builder::new().name("fake-tile".into()).spawn(move || {
            let mut k = 0u32;
            while a.load(Ordering::Relaxed) {
                f.push(&sid, test_frame(480, 270, k, i as u32 + 1));
                k = k.wrapping_add(1);
                std::thread::sleep(std::time::Duration::from_millis(33));
            }
            f.forget(&sid);
        }).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn tiles_come_from_the_subscribed_list() {
        let s = vec!["dev1/TR_a:Camera".to_string(), "dev2/TR_b:Screenshare".to_string(), "dev2/TR_c:Microphone".to_string(), "junk".to_string()];
        let t = tiles_from(&s, |id| format!("n-{id}"));
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], Tile { sid: "TR_a".into(), identity: "dev1".into(), name: "n-dev1".into(), kind: Kind::Camera });
        assert_eq!(t[1].kind, Kind::Screen);
    }
}
