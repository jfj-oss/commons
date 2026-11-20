//! Playing a decrypted audio or video file (PORT-PLAN.md section 5; the page uses `<audio>` and `<video>`, GPUI has neither).
//!
//! ffmpeg decodes, and this file only keeps time:
//!   * sound: `ffmpeg -ss <at> -i <file> -f s16le -ac 2 -ar 48000 -` into a player the system already has (`pw-cat`, else `paplay`,
//!     else `aplay`; Linux). The bytes handed to the player are the clock: position = what was written, less what the player
//!     still holds.
//!   * picture: `ffmpeg ... -f rawvideo -pix_fmt bgra` at the size it is drawn, one frame at a time, each shown when the clock
//!     reaches its time (a late frame is skipped, an early one waits), as an ordinary GPUI image.
//!   * pause stops both and remembers the place; play and seek start both again from a place (ffmpeg's `-ss` is exact for audio and
//!     on a key frame, then decoded forward, for video). A file whose sound is shorter than its picture goes on by the wall clock.
//! One thing plays at a time in the window. Tests set `MEGA_GPUI_AUDIO_SINK=null`: the sound is decoded and paced in real time and
//! thrown away, so nothing is heard and no sound server is touched.
use crate::media::tool;
use gpui_kit::RenderImage;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Bytes of sound per second: 48 kHz, 2 channels, 16 bits.
const BPS: f64 = 192_000.;
/// Frames per second of picture.
pub const FPS: f64 = 25.;

pub struct Shared {
    stop: AtomicBool,
    audio_bytes: AtomicU64, audio_live: AtomicBool, audio_done: AtomicBool, video_done: AtomicBool,
    frame: Mutex<Option<Arc<RenderImage>>>, frames: AtomicU64, skipped: AtomicU64,
    err: Mutex<Option<String>>,
    t0: Instant, start_ms: u64, rate: f64, has_audio: bool, has_video: bool,
}
struct Session { id: String, sh: Arc<Shared> }
static CUR: Mutex<Option<Session>> = Mutex::new(None);

/// What a window reads: where the playing thing is, whether it reached its end, a failure, the newest picture.
pub struct Status { pub id: String, pub pos_ms: u64, pub ended: bool, pub err: Option<String>, pub frame: Option<Arc<RenderImage>>, pub frames: u64, pub skipped: u64 }

pub fn null_sink() -> bool { std::env::var("MEGA_GPUI_AUDIO_SINK").as_deref() == Ok("null") }

/// The program that plays raw sound, if this computer has one (or the null sink of a test).
fn sink_command() -> Option<Command> {
    #[cfg(unix)]
    {
        if let Some(p) = tool("pw-cat") { let mut c = Command::new(p); c.args(["-p", "-a", "--rate", "48000", "--channels", "2", "--format", "s16", "--media-role", "Music", "-"]); return Some(c); }
        if let Some(p) = tool("paplay") { let mut c = Command::new(p); c.args(["--raw", "--rate=48000", "--channels=2", "--format=s16le"]); return Some(c); }
        if let Some(p) = tool("aplay") { let mut c = Command::new(p); c.args(["-q", "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c", "2"]); return Some(c); }
    }
    None
}
/// Can this computer play a sound inside the window?
pub fn can_play() -> bool { tool("ffmpeg").is_some() && (null_sink() || sink_command().is_some()) }

/// The position as the listener has it: the sound handed over, less what the player still holds; the wall clock when there is no sound.
fn clock(sh: &Shared) -> u64 {
    if sh.has_audio && sh.audio_live.load(Ordering::Acquire) && !sh.audio_done.load(Ordering::Acquire) {
        let b = sh.audio_bytes.load(Ordering::Acquire) as f64;
        return sh.start_ms + ((b - 40_000.).max(0.) / BPS * 1000. * sh.rate) as u64;
    }
    if sh.has_audio && !sh.audio_live.load(Ordering::Acquire) && !sh.audio_done.load(Ordering::Acquire) { return sh.start_ms; }
    sh.start_ms + (sh.t0.elapsed().as_secs_f64() * 1000. * sh.rate) as u64
}

fn fail(sh: &Shared, e: String) { let mut g = sh.err.lock().unwrap(); if g.is_none() { *g = Some(e); } }

fn audio_thread(path: std::path::PathBuf, sh: Arc<Shared>) {
    let Some(ff) = tool("ffmpeg") else { return fail(&sh, "ffmpeg is not installed".into()) };
    let mut cmd = Command::new(ff);
    cmd.args(["-v", "error", "-nostdin", "-ss", &format!("{:.3}", sh.start_ms as f64 / 1000.), "-i"]).arg(&path).args(["-vn", "-f", "s16le", "-ac", "2", "-ar", "48000"]);
    if (sh.rate - 1.).abs() > 0.01 { cmd.args(["-af", &format!("atempo={}", sh.rate)]); }
    cmd.arg("pipe:1").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut dec = match cmd.spawn() { Ok(c) => c, Err(e) => { sh.audio_done.store(true, Ordering::Release); return fail(&sh, format!("ffmpeg: {e}")); } };
    let mut out = dec.stdout.take().unwrap();
    let mut player: Option<Child> = None;
    if !null_sink() {
        match sink_command().map(|mut c| c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()) {
            Some(Ok(c)) => player = Some(c),
            _ => { let _ = dec.kill(); sh.audio_done.store(true, Ordering::Release); return fail(&sh, "No sound player was found".into()); }
        }
    }
    let mut sink_in = player.as_mut().and_then(|p| p.stdin.take());
    let mut buf = vec![0u8; 4096];
    let mut first = true;
    loop {
        if sh.stop.load(Ordering::Acquire) { break; }
        let n = match out.read(&mut buf) { Ok(0) | Err(_) => break, Ok(n) => n };
        match sink_in.as_mut() {
            Some(w) => { if w.write_all(&buf[..n]).is_err() { fail(&sh, "The sound player stopped".into()); break; } }
            None => std::thread::sleep(Duration::from_secs_f64(n as f64 / BPS)),
        }
        sh.audio_bytes.fetch_add(n as u64, Ordering::AcqRel);
        if first { first = false; sh.audio_live.store(true, Ordering::Release); }
    }
    let stopped = sh.stop.load(Ordering::Acquire);
    let _ = dec.kill(); let _ = dec.wait();
    drop(sink_in);
    if let Some(mut p) = player {
        if stopped { let _ = p.kill(); let _ = p.wait(); }
        else { std::thread::spawn(move || { let _ = p.wait(); }); }   // let the last of the sound play out
    }
    sh.audio_done.store(true, Ordering::Release);
}

fn video_thread(path: std::path::PathBuf, w: u32, h: u32, sh: Arc<Shared>) {
    let Some(ff) = tool("ffmpeg") else { sh.video_done.store(true, Ordering::Release); return };
    let mut dec = match Command::new(ff).args(["-v", "error", "-nostdin", "-ss", &format!("{:.3}", sh.start_ms as f64 / 1000.), "-i"]).arg(&path)
        .args(["-an", "-vf", &format!("fps={FPS},scale={w}:{h}:flags=bilinear"), "-pix_fmt", "bgra", "-f", "rawvideo", "pipe:1"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(c) => c, Err(e) => { sh.video_done.store(true, Ordering::Release); return fail(&sh, format!("ffmpeg: {e}")); }
    };
    let mut out = dec.stdout.take().unwrap();
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let mut n = 0u64;
    loop {
        if sh.stop.load(Ordering::Acquire) || out.read_exact(&mut buf).is_err() { break; }
        let pts = sh.start_ms + (n as f64 * 1000. / FPS) as u64; n += 1;
        // early: wait for the clock; late: skip, so the picture catches up with the sound
        let mut late = false;
        loop {
            if sh.stop.load(Ordering::Acquire) { break; }
            let c = clock(&sh);
            if c + 4 >= pts { late = c > pts + 120; break; }
            std::thread::sleep(Duration::from_millis(4));
        }
        if sh.stop.load(Ordering::Acquire) { break; }
        if late { sh.skipped.fetch_add(1, Ordering::AcqRel); continue; }
        if let Some(img) = image::RgbaImage::from_raw(w, h, buf.clone()) {
            *sh.frame.lock().unwrap() = Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(img)])));
            sh.frames.fetch_add(1, Ordering::AcqRel);
        }
    }
    let _ = dec.kill(); let _ = dec.wait();
    sh.video_done.store(true, Ordering::Release);
}

/// Start `id` from `start_ms` (replacing whatever played). `video`: the size the picture is decoded at. `rate`: 1.0 normally (sound only otherwise).
pub fn start(id: &str, path: &std::path::Path, start_ms: u64, rate: f64, video: Option<(u32, u32)>, has_audio: bool) -> Result<(), String> {
    stop();
    if tool("ffmpeg").is_none() { return Err("ffmpeg is not installed".into()); }
    if has_audio && !can_play() { return Err("No sound player was found".into()); }
    let sh = Arc::new(Shared {
        stop: AtomicBool::new(false), audio_bytes: AtomicU64::new(0), audio_live: AtomicBool::new(false), audio_done: AtomicBool::new(!has_audio), video_done: AtomicBool::new(video.is_none()),
        frame: Mutex::new(None), frames: AtomicU64::new(0), skipped: AtomicU64::new(0), err: Mutex::new(None), t0: Instant::now(), start_ms, rate: rate.clamp(0.5, 2.0), has_audio, has_video: video.is_some(),
    });
    if has_audio { let (p, s) = (path.to_path_buf(), sh.clone()); std::thread::Builder::new().name("mega-audio".into()).spawn(move || audio_thread(p, s)).map_err(|e| e.to_string())?; }
    if let Some((w, h)) = video { let (p, s) = (path.to_path_buf(), sh.clone()); std::thread::Builder::new().name("mega-video".into()).spawn(move || video_thread(p, w, h, s)).map_err(|e| e.to_string())?; }
    *CUR.lock().unwrap() = Some(Session { id: id.to_string(), sh });
    Ok(())
}

/// Stop what plays (no-op when nothing does); its processes end with it.
pub fn stop() { if let Some(s) = CUR.lock().unwrap().take() { s.sh.stop.store(true, Ordering::Release); } }

pub fn status() -> Option<Status> {
    let (id, sh) = { let g = CUR.lock().unwrap(); let s = g.as_ref()?; (s.id.clone(), s.sh.clone()) };
    let ended = sh.audio_done.load(Ordering::Acquire) && sh.video_done.load(Ordering::Acquire);
    let (err, frame) = (sh.err.lock().unwrap().clone(), sh.frame.lock().unwrap().clone());
    Some(Status { id, pos_ms: clock(&sh), ended, err, frame, frames: sh.frames.load(Ordering::Acquire), skipped: sh.skipped.load(Ordering::Acquire) })
}

/// The size a video is decoded at: no wider than `max_w`, even sides, the picture's own shape.
pub fn video_size(w: u32, h: u32, max_w: u32) -> (u32, u32) {
    let ow = w.min(max_w).max(2) & !1;
    let oh = (((ow as f64 * h as f64 / w.max(1) as f64).round() as u32).max(2)) & !1;
    (ow, oh)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn a_video_is_decoded_at_an_even_size_in_its_own_shape() {
        assert_eq!(video_size(1920, 1080, 800), (800, 450));
        assert_eq!(video_size(640, 360, 800), (640, 360), "never enlarged");
        assert_eq!(video_size(1081, 607, 801), (800, 450));
        assert_eq!(video_size(1080, 1920, 400), (400, 712));
        let (w, h) = video_size(3, 3, 800); assert!(w % 2 == 0 && h % 2 == 0 && w >= 2);
    }
}
