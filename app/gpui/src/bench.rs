//! Marks for measuring (SPIKE.md). With `MEGA_BENCH_OUT=<file>` every mark is appended to that file as a line of JSON
//! with the wall-clock time, the same format the patched Tauri build writes, so one script reads both.
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0) }
pub fn on() -> bool { std::env::var_os("MEGA_BENCH_OUT").is_some() }

pub fn mark(name: &str) { mark_with(name, serde_json::json!({})); }
pub fn mark_with(name: &str, mut extra: serde_json::Value) {
    let Some(path) = std::env::var_os("MEGA_BENCH_OUT") else { return };
    extra["mark"] = name.into();
    extra["t"] = now_ms().into();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) { let _ = writeln!(f, "{extra}"); }
}

/// Frame intervals in milliseconds, summarised.
pub fn summary(frames: &[f64]) -> serde_json::Value {
    if frames.is_empty() { return serde_json::json!({"frames": 0}); }
    let mut s = frames.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| s[((s.len() - 1) as f64 * p).round() as usize];
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    serde_json::json!({"frames": s.len(), "mean": mean, "p50": q(0.5), "p95": q(0.95), "p99": q(0.99), "max": s[s.len() - 1],
        "over_20ms": s.iter().filter(|x| **x > 20.0).count(), "over_33ms": s.iter().filter(|x| **x > 33.4).count()})
}

// ---- automation: for checks and measurements only, driven by the environment ----
use crate::app::Mega;
use gpui_kit::*;

/// `MEGA_GPUI_PASS` unlocks a THROWAWAY account without typing, `MEGA_GPUI_OPEN` opens a room by name, and
/// `MEGA_GPUI_SCROLL=<frames>` then scrolls it up and back down, one step per frame, and records the frame intervals.
/// Keys, paste and the wheel are checked from outside with real input events (examples/fakekeys.rs).
pub struct Auto {
    pub pass: Option<String>,
    pub open: Option<String>,
    pub room_loaded: bool,
    scroll: usize,
    state: u8,
    at: Option<std::time::Instant>,
    frames: std::rc::Rc<std::cell::RefCell<Vec<f64>>>,
}
impl Auto {
    pub fn from_env() -> Auto {
        let v = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Auto { pass: v("MEGA_GPUI_PASS"), open: v("MEGA_GPUI_OPEN"), room_loaded: false, scroll: v("MEGA_GPUI_SCROLL").and_then(|x| x.parse().ok()).unwrap_or(0), state: 0, at: None, frames: Default::default() }
    }
}

const STEP: f32 = 60.;

fn scroll_frame(this: WeakEntity<Mega>, n: usize, total: usize, last: std::time::Instant, window: &mut Window, cx: &mut App) {
    let now = std::time::Instant::now();
    let Some(e) = this.upgrade() else { return };
    let done = e.update(cx, |m, cx| {
        if n > 0 { m.auto.frames.borrow_mut().push(now.duration_since(last).as_secs_f64() * 1000.0); }
        if n >= total { return true; }
        // up for the first half, back down for the second
        m.list.scroll_by(px(if n < total / 2 { -STEP } else { STEP }));
        cx.notify();
        false
    });
    if done {
        e.update(cx, |m, cx| {
            let f = m.auto.frames.borrow().clone();
            mark_with("scroll_done", serde_json::json!({"rows": m.win.msgs.len(), "step_px": STEP, "summary": summary(&f)}));
            m.auto.state = 3; m.auto.at = Some(std::time::Instant::now());
            cx.notify();
        });
        return;
    }
    window.on_next_frame(move |window, cx| scroll_frame(this, n + 1, total, now, window, cx));
}

/// Called from every render; moves the automation along when its moment has come.
pub fn drive(m: &mut Mega, window: &mut Window, cx: &mut Context<Mega>) {
    let a = &mut m.auto;
    match a.state {
        0 if a.room_loaded => {
            a.state = 1; a.at = Some(std::time::Instant::now());
            window.on_next_frame(|_, _| mark("room_painted"));
            // let the room settle (and the idle measurement happen) before anything moves
            let wait = std::env::var("MEGA_GPUI_SETTLE_MS").ok().and_then(|x| x.parse().ok()).unwrap_or(1500u64);
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(wait)).await;
                let _ = this.update_in(cx, |m, window, cx| {
                    if m.auto.scroll > 0 {
                        m.auto.state = 2;
                        mark("scroll_start");
                        let (weak, total) = (cx.entity().downgrade(), m.auto.scroll);
                        window.on_next_frame(move |window, cx| scroll_frame(weak, 0, total, std::time::Instant::now(), window, cx));
                        cx.notify();
                    } else { m.auto.state = 3; cx.notify(); }
                });
            }).detach();
        }
        3 => {
            a.state = 4;
            mark("auto_done");
        }
        _ => {}
    }
}
