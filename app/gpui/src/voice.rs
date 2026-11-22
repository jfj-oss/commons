//! Recording a voice message in the composer (mega-voice.js, the recording half; playback is files.rs `voice_row`).
//!
//! A clip travels exactly as the page sends it: an ordinary encrypted file plus `voice: {ms, peaks}` (64 bytes, hex). The page's
//! fallback for browsers without MediaRecorder is a 16 kHz mono WAV, and that is what is sent here (no encoder to ship; every
//! client that plays the page's own fallback plays it). The mic is read through the sound server's own tools (`pw-cat`, `parecord`,
//! `arecord`), as playback.rs plays through them. Tests set `MEGA_GPUI_MIC=synthetic`: a made-up voice, never a real device.
//!
//! Behaviour as the page: press and hold (350 ms) to record and let go to send, slide 90 px away to cancel; a short tap keeps
//! recording with a cancel and a send button; five minutes at most; under 0.6 s is "too short"; changing room stops it.
use crate::app::Mega;
use crate::icons::{self, Icon};
use crate::look;
use crate::xfer;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MAX_MS: u64 = 5 * 60 * 1000;
pub const MIN_MS: u64 = 600;
pub const BARS: usize = 64;
const HOLD_MS: u128 = 350;
const CANCEL_PX: f32 = 90.;
const RATE: u32 = 16_000;
/// One slice of sound (and one waveform column): 50 ms.
const SLICE: usize = (RATE as usize) / 20;

/// What the capture thread fills: every sample, and the loudest of each slice.
#[derive(Default)]
pub struct Take { pub pcm: Vec<i16>, pub peaks: Vec<f32>, pub err: Option<String> }

pub struct Rec {
    pub gid: String, pub t0: Instant, pub take: Arc<Mutex<Take>>, stop: Arc<AtomicBool>, child: Arc<Mutex<Option<Child>>>,
    /// "pending" (pressed, deciding), "hold" (let go sends), "lock" (buttons).
    pub mode: &'static str, pub cancelling: bool, pub from: Option<Point<Pixels>>, pub down: bool,
}

#[derive(Default)]
pub struct VoiceUi { pub rec: Option<Rec> }

fn capture_command() -> Option<Command> {
    use crate::media::tool;
    if let Some(p) = tool("pw-cat") { let mut c = Command::new(p); c.args(["-r", "--rate", "16000", "--channels", "1", "--format", "s16", "--media-role", "Communication", "-"]); return Some(c); }
    if let Some(p) = tool("parecord") { let mut c = Command::new(p); c.args(["--raw", "--rate=16000", "--channels=1", "--format=s16le"]); return Some(c); }
    if let Some(p) = tool("arecord") { let mut c = Command::new(p); c.args(["-q", "-t", "raw", "-f", "S16_LE", "-r", "16000", "-c", "1"]); return Some(c); }
    None
}
pub fn synthetic() -> bool { std::env::var("MEGA_GPUI_MIC").as_deref() == Ok("synthetic") }
pub fn can_record() -> bool { synthetic() || capture_command().is_some() }

fn push(take: &Mutex<Take>, pcm: &[i16]) {
    let mut t = take.lock().unwrap();
    for ch in pcm.chunks(SLICE) {
        let m = ch.iter().map(|s| (*s as i32).unsigned_abs() as f32 / 32768.).fold(0f32, f32::max);
        t.peaks.push(m);
    }
    t.pcm.extend_from_slice(pcm);
}

/// Start capturing on a thread of its own.
fn start_capture(take: Arc<Mutex<Take>>, stop: Arc<AtomicBool>, child: Arc<Mutex<Option<Child>>>) {
    std::thread::Builder::new().name("mega-mic".into()).spawn(move || {
        if synthetic() {
            // a made-up voice: syllables of a 220 Hz tone with a slow loudness, one slice per 50 ms
            let mut n = 0usize; let t0 = Instant::now();
            while !stop.load(Ordering::Acquire) {
                let mut s = Vec::with_capacity(SLICE);
                for _ in 0..SLICE {
                    let t = n as f32 / RATE as f32;
                    let env = (0.5 + 0.5 * (t * 2. * std::f32::consts::PI * 1.3).sin()).powi(2) * (0.35 + 0.65 * (t * 0.7).sin().abs());
                    s.push(((t * 220. * 2. * std::f32::consts::PI).sin() * env * 0.8 * 32767.) as i16); n += 1;
                }
                push(&take, &s);
                let due = Duration::from_millis(50 * (n / SLICE) as u64);
                if let Some(w) = due.checked_sub(t0.elapsed()) { std::thread::sleep(w); }
            }
            return;
        }
        let Some(mut cmd) = capture_command() else { take.lock().unwrap().err = Some("No microphone was found.".into()); return };
        let mut ch = match cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).spawn() { Ok(c) => c, Err(_) => { take.lock().unwrap().err = Some("No microphone was found.".into()); return } };
        let mut out = ch.stdout.take().unwrap();
        *child.lock().unwrap() = Some(ch);
        let mut buf = vec![0u8; SLICE * 2]; let mut keep: Vec<u8> = vec![];
        loop {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    keep.extend_from_slice(&buf[..n]);
                    let whole = keep.len() / 2 * 2;
                    if whole >= SLICE * 2 {
                        let pcm: Vec<i16> = keep[..whole].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                        keep.drain(..whole); push(&take, &pcm);
                    }
                }
            }
            if stop.load(Ordering::Acquire) { break; }
        }
        if take.lock().unwrap().pcm.is_empty() && !stop.load(Ordering::Acquire) { take.lock().unwrap().err = Some("No microphone was found.".into()); }
    }).ok();
}

fn stop_capture(r: &Rec) {
    r.stop.store(true, Ordering::Release);
    if let Some(mut c) = r.child.lock().unwrap().take() { let _ = c.kill(); let _ = c.wait(); }
}

/// 64 bytes: the loudest moment in each slice of the clip, scaled so the loudest slice is 255 (toPeaks).
pub fn to_peaks(s: &[f32]) -> Vec<u8> {
    let n = s.len();
    if n == 0 { return vec![0; BARS]; }
    let out: Vec<f32> = (0..BARS).map(|i| { let a = i * n / BARS; let b = ((i + 1) * n / BARS).max(a + 1); s[a..b.min(n)].iter().cloned().fold(0., f32::max) }).collect();
    let top = out.iter().cloned().fold(1e-4, f32::max);
    out.iter().map(|v| ((v / top).sqrt() * 255.).round() as u8).collect()
}

/// A 16 kHz mono 16-bit WAV (wavOf).
pub fn wav(pcm: &[i16]) -> Vec<u8> {
    let len = pcm.len() * 2;
    let mut o = Vec::with_capacity(44 + len);
    o.extend_from_slice(b"RIFF"); o.extend_from_slice(&((36 + len) as u32).to_le_bytes()); o.extend_from_slice(b"WAVEfmt ");
    o.extend_from_slice(&16u32.to_le_bytes()); o.extend_from_slice(&1u16.to_le_bytes()); o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&RATE.to_le_bytes()); o.extend_from_slice(&(RATE * 2).to_le_bytes()); o.extend_from_slice(&2u16.to_le_bytes()); o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(b"data"); o.extend_from_slice(&(len as u32).to_le_bytes());
    for s in pcm { o.extend_from_slice(&s.to_le_bytes()); }
    o
}
fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

pub fn clock(ms: u64) -> String { let s = (ms as f64 / 1000.).round() as u64; format!("{}:{:02}", s / 60, s % 60) }

impl Mega {
    pub fn voice_init(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(50)).await;
            if this.update_in(cx, |this, window, cx| this.voice_tick(window, cx)).is_err() { break; }
        });
        self._tasks.push(t);
        cx.on_app_quit(|this, _| { if let Some(r) = this.voice.rec.take() { stop_capture(&r); } async {} }).detach();
    }

    pub fn recording(&self) -> bool { self.voice.rec.is_some() }

    pub fn voice_ms(&self) -> u64 { self.voice.rec.as_ref().map(|r| (r.t0.elapsed().as_millis() as u64).min(MAX_MS)).unwrap_or(0) }

    /// The mic button went down (or a keyboard click on it).
    pub fn voice_press(&mut self, at: Option<Point<Pixels>>, _w: &mut Window, cx: &mut Context<Self>) {
        if self.voice.rec.is_some() { return; }
        let Some(gid) = self.gid.clone() else { return xfer::toast("Open a room first.") };
        if self.msg.edit.is_some() { return; }
        if !can_record() { return xfer::toast("No microphone was found."); }
        let (take, stop, child) = (Arc::new(Mutex::new(Take::default())), Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None)));
        start_capture(take.clone(), stop.clone(), child.clone());
        self.voice.rec = Some(Rec { gid, t0: Instant::now(), take, stop, child, mode: "pending", cancelling: false, from: at, down: at.is_some() });
        cx.notify();
    }

    /// The pointer moved while the mic button is held: far enough away cancels.
    pub fn voice_move(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(r) = self.voice.rec.as_mut().filter(|r| r.down) else { return };
        if let Some(f) = r.from {
            let far = ((f32::from(p.x - f.x)).powi(2) + (f32::from(p.y - f.y)).powi(2)).sqrt() > CANCEL_PX;
            if far != r.cancelling { r.cancelling = far; cx.notify(); }
        }
    }

    /// The button came up: after a hold it sends (or cancels when slid away); a tap keeps recording with buttons.
    pub fn voice_release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.voice.rec.as_mut().filter(|r| r.down) else { return };
        r.down = false;
        if r.t0.elapsed().as_millis() >= HOLD_MS { let send = !r.cancelling; self.voice_finish(send, window, cx); }
        else { r.mode = "lock"; cx.notify(); }
    }

    pub fn voice_finish(&mut self, send: bool, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.voice.rec.take() else { return };
        stop_capture(&r); cx.notify();
        if !send { return; }
        let ms = (r.t0.elapsed().as_millis() as u64).min(MAX_MS);
        let (pcm, peaks) = { let t = r.take.lock().unwrap(); (t.pcm.clone(), t.peaks.clone()) };
        let ms = if pcm.is_empty() { ms } else { ms.min(pcm.len() as u64 * 1000 / RATE as u64 + 50) };
        if ms < MIN_MS { return xfer::toast("That was too short. Hold the button and talk, then let go."); }
        if pcm.is_empty() { return xfer::toast("Nothing was recorded."); }
        let max = MAX_MS as usize * RATE as usize / 1000;
        let pcm = if pcm.len() > max { pcm[..max].to_vec() } else { pcm };
        let core = self.core.clone(); let gid = r.gid.clone();
        let name = format!("Voice message {}.wav", chrono::Local::now().format("%Y-%m-%d %H.%M.%S"));
        let data = wav(&pcm); let peaks = hex(&to_peaks(&peaks));
        xfer::toast("Sending voice message…");
        std::thread::Builder::new().name("mega-send".into()).spawn(move || {
            let r = core.call("send_file", json!({"gid": gid, "name": name, "mime": "audio/wav", "data": xfer::b64u(&data), "caption": "", "voice": {"ms": ms, "peaks": peaks}}));
            xfer::untoast("Sending voice message…");
            if let Err(e) = r { xfer::toast(e.to_string()); }
            xfer::bump_pub();
        }).ok();
    }

    fn voice_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.voice.rec.as_ref() else { return };
        if self.gid.as_deref() != Some(r.gid.as_str()) { xfer::toast("Recording stopped: you changed room."); return self.voice_finish(false, window, cx); }
        let err = r.take.lock().unwrap().err.clone();
        if let Some(e) = err { xfer::toast(e); return self.voice_finish(false, window, cx); }
        let ms = r.t0.elapsed().as_millis() as u64;
        if r.mode == "pending" && ms >= HOLD_MS as u128 as u64 && r.down { if let Some(r) = self.voice.rec.as_mut() { r.mode = "hold"; } }
        if ms >= MAX_MS { if let Some(r) = self.voice.rec.as_mut() { r.mode = "lock"; r.down = false; } }
        cx.notify();
    }

    /// The recording bar that replaces the composer's row (`.vrec`).
    pub fn voice_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self.voice.rec.as_ref()?;
        let ms = self.voice_ms();
        let cancelling = r.cancelling;
        let col = if cancelling { rgb(look::DANGER).into() } else { look::paper() };
        let n = 44usize;
        let peaks: Vec<f32> = { let t = r.take.lock().unwrap(); let l = t.peaks.len(); (0..n).map(|i| if l + i >= n { t.peaks[l + i - n] } else { -1. }).collect() };
        let wave = canvas(|_, _, _| {}, move |b, _, window, _| {
            let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
            let step = 4.; let k = ((w / step) as usize).min(peaks.len());
            for i in 0..k {
                let v = peaks[peaks.len() - k + i];
                let a = if v < 0. { 0.06 } else { (v.sqrt() * 1.15).clamp(0.06, 1.) };
                let bh = (a * h).max(2.);
                window.paint_quad(fill(Bounds { origin: point(b.origin.x + px(i as f32 * step), b.origin.y + px((h - bh) / 2.)), size: size(px(2.), px(bh)) }, col).corner_radii(px(1.)));
            }
        }).h(px(32.)).flex_1().min_w(px(60.));
        let hint = if ms >= MAX_MS { "five minutes is the most — send or cancel" } else if r.mode == "lock" { "" } else if cancelling { "release to cancel" } else if r.mode == "hold" { "release to send · slide away to cancel" } else { "recording…" };
        let lock = r.mode == "lock";
        let pulse = ((ms as f32 / 1000.) * std::f32::consts::PI).sin() * 0.5 + 0.5;
        Some(h_flex().id("vrec").relative().items_center().gap(px(10.)).pl(px(14.)).pr(px(8.)).min_h(px(48.)).font_family(look::F_UI)
            .child(div().flex_none().size(px(10.)).rounded_full().bg(col).opacity(0.45 + 0.55 * pulse))
            .child(div().id("vrec-timer").flex_none().w(px(40.)).text_size(px(13.)).text_color(col).child(clock(ms)))
            .child(wave)
            .child(div().id("vrec-hint").flex_none().text_size(px(12.)).text_color(if cancelling { col } else { look::mute() }).child(hint))
            .when(lock, |d| d.child(div().id("vrec-cancel").flex_none().flex().items_center().justify_center().size(px(36.)).rounded_full().cursor_pointer().child(icons::icon(Icon::Trash, 18., look::mute()))
                    .on_click(cx.listener(|this, _, w, cx| this.voice_finish(false, w, cx))))
                .child(div().id("vrec-send").flex_none().flex().items_center().h(px(36.)).px(px(16.)).rounded_full().cursor_pointer().bg(look::paper()).text_color(rgb(look::VOID)).text_size(px(13.)).child("send")
                    .on_click(cx.listener(|this, _, w, cx| this.voice_finish(true, w, cx)))))
            .into_any_element())
    }

    pub fn voice_cmd(&mut self, cmd: &Value, window: &mut Window, cx: &mut Context<Self>) -> Value {
        match cmd["cmd"].as_str().unwrap_or("") {
            "voice_press" => { self.voice_press(Some(point(px(cmd["x"].as_f64().unwrap_or(0.) as f32), px(cmd["y"].as_f64().unwrap_or(0.) as f32))), window, cx); json!({"ok": self.recording()}) }
            "voice_move" => { self.voice_move(point(px(cmd["x"].as_f64().unwrap_or(0.) as f32), px(cmd["y"].as_f64().unwrap_or(0.) as f32)), cx); json!({"ok": true}) }
            "voice_release" => { self.voice_release(window, cx); json!({"ok": true}) }
            "voice_send" => { self.voice_finish(true, window, cx); json!({"ok": true}) }
            "voice_cancel" => { self.voice_finish(false, window, cx); json!({"ok": true}) }
            _ => json!({"recording": self.recording(), "ms": self.voice_ms(), "mode": self.voice.rec.as_ref().map(|r| r.mode), "cancelling": self.voice.rec.as_ref().map(|r| r.cancelling),
                "slices": self.voice.rec.as_ref().map(|r| r.take.lock().unwrap().peaks.len()), "synthetic": synthetic(), "toasts": xfer::toasts()}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clock, to_peaks, wav};
    #[test] fn peaks_scale_the_loudest_to_255() { let s: Vec<f32> = (0..640).map(|i| if i == 100 { 0.5 } else { 0.1 }).collect(); let p = to_peaks(&s); assert_eq!(p.len(), 64); assert_eq!(*p.iter().max().unwrap(), 255); }
    #[test] fn silence_is_all_zero_bars() { assert_eq!(to_peaks(&[]), vec![0; 64]); }
    #[test] fn the_wav_header_says_16k_mono() { let w = wav(&[1, 2, 3]); assert_eq!(&w[..4], b"RIFF"); assert_eq!(w.len(), 44 + 6); assert_eq!(u32::from_le_bytes([w[24], w[25], w[26], w[27]]), 16000); }
    #[test] fn clock_reads_as_the_page() { assert_eq!(clock(7400), "0:07"); assert_eq!(clock(75000), "1:15"); }
}
