//! CALLS-PLAN.md step 1, the render bench: remote video drawn the way Linux has to (no `paint_surface` there), as a BGRA
//! `RenderImage` per frame through the sprite atlas, at 2560x1440 and 30 frames a second in one window-sized tile.
//!
//! `MEGA_GPUI_VIDEO_BENCH=<seconds>` opens this instead of the app (no core, no account). A thread stands in for a
//! subscribed track's reader: it makes an I420 frame with moving content, converts it with libwebrtc's libyuv as the
//! real reader will, and leaves only the newest frame in a slot. The window takes it on its next render and drops the
//! previous image from the atlas (`Window::drop_image`), which is what keeps the atlas from growing.
//! Every 10 s a line goes to `MEGA_BENCH_OUT`: CPU of the process (share of one core), RSS, and the intervals between
//! frames that reached the screen.
use crate::bench::{mark_with, summary};
use gpui_kit::*;
use livekit::webrtc::video_frame::native::VideoFrameBufferExt;
use livekit::webrtc::video_frame::{I420Buffer, VideoFormatType};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn wanted() -> Option<f64> { std::env::var("MEGA_GPUI_VIDEO_BENCH").ok().and_then(|s| s.parse().ok()) }

pub struct VideoBench {
    slot: Arc<parking_lot::Mutex<Option<Arc<RenderImage>>>>,
    current: Option<Arc<RenderImage>>,
    last_shown: Option<Instant>,
    intervals: Vec<f64>,
    all: Vec<f64>,
    made: Arc<std::sync::atomic::AtomicU64>,
    shown: u64,
    start: Instant,
    secs: f64,
    cpu0: (f64, Instant),
    _tasks: Vec<Task<()>>,
}

fn cpu_seconds() -> f64 {
    // utime + stime of this process, fields 14 and 15 of /proc/self/stat, in clock ticks (100 a second on Linux)
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let after = s.rsplit_once(')').map(|x| x.1).unwrap_or("");
    let f: Vec<&str> = after.split_whitespace().collect();
    let t = |i: usize| f.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.);
    (t(11) + t(12)) / 100.
}
fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status").unwrap_or_default().lines().find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok()).unwrap_or(0.) / 1024.
}

impl VideoBench {
    pub fn new(secs: f64, window: &mut Window, cx: &mut Context<Self>) -> VideoBench {
        let (w, h) = (2560u32, 1440u32);
        let slot: Arc<parking_lot::Mutex<Option<Arc<RenderImage>>>> = Default::default();
        let made: Arc<std::sync::atomic::AtomicU64> = Default::default();
        let (tx, rx) = futures::channel::mpsc::unbounded::<()>();
        let (s2, m2) = (slot.clone(), made.clone());
        std::thread::Builder::new().name("video-bench-src".into()).spawn(move || {
            let mut i420 = I420Buffer::new(w, h);
            let period = Duration::from_nanos(1_000_000_000 / 30);
            let mut next = Instant::now();
            for n in 0u64.. {
                {
                    // a gradient that scrolls, so every frame differs everywhere, like a moving picture
                    let (sy, su, _) = i420.strides();
                    let (y, u, v) = i420.data_mut();
                    for row in 0..h as usize { let base = (row as u64 + n * 4) as u8; let r = &mut y[row * sy as usize..row * sy as usize + w as usize]; for (x, p) in r.iter_mut().enumerate() { *p = base.wrapping_add((x / 8) as u8); } }
                    for row in 0..(h / 2) as usize { u[row * su as usize..row * su as usize + (w / 2) as usize].fill((n * 3) as u8); }
                    v.fill(128);
                }
                let mut bgra = vec![0u8; (w * h * 4) as usize];
                i420.to_argb(VideoFormatType::ARGB, &mut bgra, w * 4, w as i32, h as i32);
                let img = image::RgbaImage::from_raw(w, h, bgra).expect("frame size");
                let ri = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(img)]));
                m2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // only the newest frame is kept; one waiting is replaced, never queued
                let was_empty = s2.lock().replace(ri).is_none();
                if was_empty && tx.unbounded_send(()).is_err() { break; }
                next += period;
                let now = Instant::now();
                if next > now { std::thread::sleep(next - now); } else { next = now; }
            }
        }).expect("thread");
        let mut tasks = vec![];
        let mut rx = rx;
        tasks.push(cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() { if this.update(cx, |_, cx| cx.notify()).is_err() { break; } }
        }));
        tasks.push(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(10)).await;
            let done = this.update(cx, |b, cx| b.report(cx)).unwrap_or(true);
            if done { let _ = cx.update(|_, cx| cx.quit()); break; }
        }));
        mark_with("video_bench_start", serde_json::json!({"w": w, "h": h, "fps": 30, "secs": secs, "rss_mb": rss_mb()}));
        VideoBench { slot, current: None, last_shown: None, intervals: vec![], all: vec![], made, shown: 0, start: Instant::now(), secs, cpu0: (cpu_seconds(), Instant::now()), _tasks: tasks }
    }

    /// One line per 10 s; true when the run is over.
    fn report(&mut self, _cx: &mut Context<Self>) -> bool {
        let (c, t) = (cpu_seconds(), Instant::now());
        let cpu = (c - self.cpu0.0) / t.duration_since(self.cpu0.1).as_secs_f64() * 100.;
        self.cpu0 = (c, t);
        let iv = std::mem::take(&mut self.intervals);
        self.all.extend(&iv);
        let elapsed = self.start.elapsed().as_secs_f64();
        mark_with("video_bench_tick", serde_json::json!({"elapsed": elapsed, "cpu_pct_one_core": cpu, "rss_mb": rss_mb(), "made": self.made.load(std::sync::atomic::Ordering::Relaxed), "shown": self.shown, "intervals": summary(&iv)}));
        if elapsed + 0.5 >= self.secs {
            mark_with("video_bench_done", serde_json::json!({"elapsed": elapsed, "rss_mb": rss_mb(), "shown": self.shown, "intervals": summary(&self.all)}));
            return true;
        }
        false
    }
}

impl Render for VideoBench {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(new) = self.slot.lock().take() {
            if let Some(old) = self.current.replace(new) { let _ = window.drop_image(old); }
            let now = Instant::now();
            if let Some(l) = self.last_shown { self.intervals.push(now.duration_since(l).as_secs_f64() * 1000.); }
            self.last_shown = Some(now);
            self.shown += 1;
        }
        let tile = div().size_full().bg(rgb(0x000000));
        match &self.current {
            Some(i) => tile.child(img(ImageSource::Render(i.clone())).size_full().object_fit(ObjectFit::Contain)),
            None => tile,
        }
    }
}
