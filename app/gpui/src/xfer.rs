//! Sending and saving files (web reference: app.js `uploadFile`, `uploadLarge`, `downloadLarge`, `xferStep`, `xferRefresh`; PORT-PLAN.md section 5).
//!
//! The core does the cryptography and keeps all the state of a large transfer; this file does what the page does with it,
//! one chunk at a time on threads of their own (a core call can wait for the relay, so none runs on the window's thread):
//!   * a file up to `LARGE_OVER` goes in one `send_file` call; a larger one through `file_upload_begin / check / chunk / finish`,
//!     resumable (the relay remembers what arrived, so the same file offered again carries on), with pause and cancel;
//!   * a large file is saved to a folder in pieces (`file_download_*`), also pausable, cancellable and resumable;
//!   * every network step waits out a pause, and retries a transient failure with backoff (1 s up to 30 s, without end);
//!   * `file_transfers` lists what is unfinished (an upload waiting for its file, a download partly saved), found again after
//!     a restart; an upload whose parts are all safe on the relay is finished without asking for the file again.
//! Nothing here draws; the window reads `live()`, `parked()` and `toasts()` (files.rs).
use crate::core::Core;
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A file over this goes the large way (app.js `LARGE_OVER`).
pub const LARGE_OVER: u64 = 24 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Up, Down }

#[derive(Default, Clone)]
pub struct XState {
    pub bytes: u64, pub total: u64, pub safe: u64,
    /// "" | "checking" | "waiting"
    pub state: String, pub err: String, pub retry_at: f64, pub expires: f64,
    pub samples: Vec<(Instant, u64)>,
    /// The person pressed cancel on a transfer that already has something saved: asked once more before it goes.
    pub confirm: bool,
    /// Where a finished download went.
    pub path: String,
}

pub struct Xfer {
    pub key: String, pub kind: Kind, pub name: String, pub gid: String,
    pub id: Mutex<String>,
    pub paused: AtomicBool, pub cancelled: AtomicBool, pub idle: AtomicBool,
    pub st: Mutex<XState>,
}
impl Xfer {
    fn new(key: String, kind: Kind, name: &str, gid: &str, total: u64) -> Arc<Xfer> {
        Arc::new(Xfer { key, kind, name: name.to_string(), gid: gid.to_string(), id: Default::default(), paused: AtomicBool::new(false), cancelled: AtomicBool::new(false), idle: AtomicBool::new(false),
            st: Mutex::new(XState { total, state: "checking".into(), ..Default::default() }) })
    }
    pub fn is_paused(&self) -> bool { self.paused.load(Ordering::Acquire) }
    pub fn is_cancelled(&self) -> bool { self.cancelled.load(Ordering::Acquire) }
    pub fn snap(&self) -> XState { self.st.lock().unwrap().clone() }
    fn with<T>(&self, f: impl FnOnce(&mut XState) -> T) -> T { let r = f(&mut self.st.lock().unwrap()); bump(); r }
    fn mark(&self) {
        let mut s = self.st.lock().unwrap();
        let now = Instant::now(); let b = s.bytes;
        s.samples.push((now, b));
        while s.samples.len() > 2 && now.duration_since(s.samples[0].0) > Duration::from_secs(20) { s.samples.remove(0); }
    }
    /// app.js `xferRate`: speed over the last ~20 s of actual transfer, so a pause or an outage does not skew it. (bytes per second, seconds left)
    pub fn rate(&self) -> Option<(f64, f64)> {
        if self.is_paused() { return None; }
        let s = self.st.lock().unwrap();
        if !s.state.is_empty() || s.samples.len() < 2 { return None; }
        let (t0, b0) = s.samples[0]; let (t1, b1) = s.samples[s.samples.len() - 1];
        let dt = t1.duration_since(t0).as_secs_f64();
        if dt < 0.7 || b1 <= b0 { return None; }
        let speed = (b1 - b0) as f64 / dt;
        Some((speed, s.total.saturating_sub(s.bytes) as f64 / speed))
    }
}

static LIVE: Mutex<Vec<Arc<Xfer>>> = Mutex::new(Vec::new());
static GEN: AtomicU64 = AtomicU64::new(1);
/// Bumped on every change anybody could see; the window redraws when it sees a new number.
pub fn generation() -> u64 { GEN.load(Ordering::Acquire) }
fn bump() { GEN.fetch_add(1, Ordering::AcqRel); }
pub fn live() -> Vec<Arc<Xfer>> { LIVE.lock().unwrap().clone() }
pub fn find(key: &str) -> Option<Arc<Xfer>> { LIVE.lock().unwrap().iter().find(|x| x.key == key).cloned() }
/// The running save of message `id`, if any (the message's Download button turns into Cancel, with progress under it).
pub fn download_of(id: &str) -> Option<Arc<Xfer>> { find(&format!("d:{id}")) }
fn add(x: &Arc<Xfer>) -> bool {
    let mut l = LIVE.lock().unwrap();
    if l.iter().any(|v| v.key == x.key) { return false; }
    l.push(x.clone()); drop(l); bump(); true
}
fn remove(x: &Arc<Xfer>) { LIVE.lock().unwrap().retain(|v| !Arc::ptr_eq(v, x)); bump(); }

// ---------------------------------------------------------------------------------------------- notices

static TOASTS: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
/// A short notice (what the web shows as a toast).
pub fn toast(s: impl Into<String>) {
    let s = s.into();
    LOG.lock().unwrap().push(s.clone());
    let mut t = TOASTS.lock().unwrap();
    t.retain(|(x, _)| *x != s);
    t.push((s, Instant::now() + Duration::from_secs(6)));
    if t.len() > 4 { t.remove(0); }
    drop(t); bump();
}
/// Take a notice away early (the "Encrypting and uploading…" of a send that finished).
pub fn untoast(s: &str) { TOASTS.lock().unwrap().retain(|(x, _)| x != s); bump(); }
pub fn toasts() -> Vec<String> { let mut t = TOASTS.lock().unwrap(); t.retain(|(_, u)| *u > Instant::now()); t.iter().map(|(s, _)| s.clone()).collect() }
/// Every notice ever shown in this run (tests).
pub fn toast_log() -> Vec<String> { LOG.lock().unwrap().clone() }
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn say(s: impl Into<String>) { toast(s); }

// ---------------------------------------------------------------------------------------------- small helpers

pub fn unix() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.) }
/// app.js `fmtSize`.
pub fn fmt_size(n: u64) -> String {
    if n >= 1 << 30 { format!("{:.2} GB", n as f64 / (1u64 << 30) as f64) } else if n < 1024 { format!("{n} B") } else if n < 1 << 20 { format!("{:.1} KB", n as f64 / 1024.) } else { format!("{:.1} MB", n as f64 / (1u64 << 20) as f64) }
}
/// app.js `fmtDur`.
pub fn fmt_dur(s: f64) -> String {
    if s < 60. { format!("{}s", s.ceil()) } else if s < 3600. { format!("{} min", (s / 60.).ceil()) } else if s < 86400. { format!("{:.1} h", s / 3600.) } else { format!("{:.1} days", s / 86400.) }
}
/// app.js `fmtLeft`.
pub fn fmt_left(expires: f64) -> String {
    let s = expires - unix();
    if s <= 0. { "expired".into() } else if s < 3600. { format!("{} min left", (s / 60.).ceil()) } else if s < 86400. { format!("{} h left", (s / 3600.).round()) } else { format!("{} days left", (s / 86400.).round()) }
}
/// Anything the core marked "[retry]", and any failure to reach the core itself, is waited out (app.js `isRetry`).
pub fn is_retry(e: &str) -> bool { let l = e.to_lowercase(); e.starts_with("[retry]") || l.contains("failed to fetch") || l.contains("networkerror") || l.contains("load failed") || e == "locked" || l.contains("network") }
pub fn clean_err(e: &str) -> String { e.strip_prefix("[retry] ").unwrap_or(e).to_string() }

pub fn b64u(b: &[u8]) -> String { use base64::Engine as _; base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b) }
pub fn unb64(s: &str) -> Result<Vec<u8>, String> { use base64::Engine as _; base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|e| e.to_string()) }

/// What the browser calls `File.type`: from the name's ending, then from the first bytes.
pub fn mime_of(name: &str, head: &[u8]) -> String {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let m = match ext.as_str() {
        "png" => "image/png", "jpg" | "jpeg" | "jpe" => "image/jpeg", "gif" => "image/gif", "webp" => "image/webp", "bmp" => "image/bmp", "svg" => "image/svg+xml", "avif" => "image/avif", "heic" => "image/heic", "tif" | "tiff" => "image/tiff", "ico" => "image/x-icon",
        "mp4" | "m4v" => "video/mp4", "mov" => "video/quicktime", "webm" => "video/webm", "mkv" => "video/x-matroska", "avi" => "video/x-msvideo", "mpg" | "mpeg" => "video/mpeg", "ogv" => "video/ogg", "3gp" => "video/3gpp",
        "mp3" => "audio/mpeg", "m4a" => "audio/mp4", "aac" => "audio/aac", "wav" => "audio/wav", "ogg" | "oga" => "audio/ogg", "opus" => "audio/opus", "flac" => "audio/flac", "weba" => "audio/webm",
        "pdf" => "application/pdf", "zip" => "application/zip", "json" => "application/json", "txt" | "log" | "md" => "text/plain", "csv" => "text/csv", "html" | "htm" => "text/html", "xml" => "text/xml",
        "gz" | "tgz" => "application/gzip", "7z" => "application/x-7z-compressed", "tar" => "application/x-tar", "doc" => "application/msword", "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel", "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "ppt" => "application/vnd.ms-powerpoint", "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "",
    };
    if !m.is_empty() { return m.into(); }
    if head.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else if head.starts_with(&[0xff, 0xd8, 0xff]) { "image/jpeg" } else if head.starts_with(b"GIF8") { "image/gif" }
    else if head.len() > 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" { "image/webp" } else if head.len() > 8 && &head[4..8] == b"ftyp" { "video/mp4" } else if head.starts_with(b"OggS") { "audio/ogg" } else if head.starts_with(b"%PDF") { "application/pdf" }
    else { "application/octet-stream" }.into()
}

/// "Pasted image 2026-11-19 23.04.05.png": what a picture with no real name is called (app.js `uploadFile`).
pub fn pasted_name(ext: &str) -> String {
    use chrono::Timelike as _;
    let d = chrono::Local::now();
    format!("Pasted image {}-{:02}-{:02} {:02}.{:02}.{:02}.{ext}", chrono::Datelike::year(&d), chrono::Datelike::month(&d), chrono::Datelike::day(&d), d.hour(), d.minute(), d.second())
}

/// A name that is not taken in `dir`: "photo.png", "photo (1).png", ...
pub fn unique_in(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() { return p; }
    let (stem, ext) = match name.rfind('.') { Some(i) if i > 0 => (&name[..i], &name[i..]), _ => (name, "") };
    for n in 1..10_000 { let c = dir.join(format!("{stem} ({n}){ext}")); if !c.exists() { return c; } }
    p
}

// ---------------------------------------------------------------------------------------------- one network step

/// Every network step goes through here: wait while paused, retry transient failures with backoff, stop on a real refusal (app.js `xferStep`).
fn step<T>(x: &Xfer, mut f: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut delay = 1000u64;
    loop {
        while x.is_paused() && !x.is_cancelled() { x.idle.store(true, Ordering::Release); bump(); std::thread::sleep(Duration::from_millis(250)); }
        x.idle.store(false, Ordering::Release);
        if x.is_cancelled() { return Err("cancelled".into()); }
        match f() {
            Ok(r) => { if !x.st.lock().unwrap().state.is_empty() { x.with(|s| s.state.clear()); } return Ok(r); }
            Err(e) => {
                if x.is_cancelled() || !is_retry(&e) { return Err(e); }
                x.with(|s| { s.state = "waiting".into(); s.err = clean_err(&e).chars().take(90).collect(); });
                let mut left = delay as i64;
                while left > 0 && !x.is_cancelled() {
                    x.with(|s| s.retry_at = unix() + left as f64 / 1000.);
                    std::thread::sleep(Duration::from_millis(500)); left -= 500;
                    if x.is_paused() { break; }
                }
                delay = (delay * 2).min(30_000);
            }
        }
    }
}

/// Pause or resume a running transfer (the core remembers it for after a restart).
pub fn set_paused(core: &Arc<Core>, key: &str) {
    let Some(x) = find(key) else { return };
    if x.is_cancelled() { return; }
    let now = !x.paused.load(Ordering::Acquire);
    x.paused.store(now, Ordering::Release); x.idle.store(false, Ordering::Release);
    bump();
    let id = x.id.lock().unwrap().clone();
    if !id.is_empty() {
        let core = core.clone(); let (m, kind) = (if x.kind == Kind::Up { "file_upload_pause" } else { "file_download_pause" }, x.kind);
        let _ = kind;
        std::thread::spawn(move || { let _ = core.call(m, json!({"id": id, "paused": now})); });
    }
}
/// Cancel: with something already saved the first press only asks (returns true while it waits for the answer).
pub fn cancel(key: &str, sure: bool) -> bool {
    let Some(x) = find(key) else { return false };
    let had = { let s = x.st.lock().unwrap(); s.bytes > 0 || s.safe > 0 };
    if had && !sure { x.with(|s| s.confirm = true); return true; }
    x.cancelled.store(true, Ordering::Release); x.paused.store(false, Ordering::Release); x.with(|s| s.confirm = false);
    false
}
pub fn keep(key: &str) { if let Some(x) = find(key) { x.with(|s| s.confirm = false); } }

// ---------------------------------------------------------------------------------------------- uploads

pub struct Local { pub path: PathBuf, pub name: String, pub size: u64, pub last_modified: u64 }
pub fn local(path: &Path) -> Result<Local, String> {
    let md = std::fs::metadata(path).map_err(|_| format!("{} could not be read.", path.display()))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    if md.is_dir() { return Err(format!("{name} is a folder; only files can be sent."));
    }
    let lm = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    Ok(Local { path: path.to_path_buf(), name, size: md.len(), last_modified: lm })
}

/// Send these files, one after the other (app.js `uploadFiles`). Runs on its own thread.
pub fn send_paths(core: Arc<Core>, gid: String, paths: Vec<PathBuf>) {
    std::thread::Builder::new().name("mega-send".into()).spawn(move || {
        for p in paths {
            match local(&p) { Ok(l) => send_local(&core, &gid, l), Err(e) => say(e) }
        }
    }).ok();
}

/// Send one picture that came from the clipboard.
pub fn send_bytes(core: Arc<Core>, gid: String, name: String, mime: String, data: Vec<u8>) {
    std::thread::Builder::new().name("mega-send".into()).spawn(move || { send_small(&core, &gid, &name, &mime, &data); }).ok();
}

fn send_local(core: &Arc<Core>, gid: &str, l: Local) {
    if l.size > LARGE_OVER { upload_large(core, gid, &l, None); return; }
    let data = match std::fs::read(&l.path) { Ok(d) => d, Err(_) => return say(format!("{} could not be read.", l.name)) };
    let mime = mime_of(&l.name, &data[..data.len().min(16)]);
    send_small(core, gid, &l.name, &mime, &data);
}

fn send_small(core: &Arc<Core>, gid: &str, name: &str, mime: &str, data: &[u8]) {
    say("Encrypting and uploading…");
    let r = core.call("send_file", json!({"gid": gid, "name": name, "mime": mime, "data": b64u(data), "caption": ""}));
    untoast("Encrypting and uploading…");
    if let Err(e) = r { say(clean_err(&e)); }
    bump();
}

/// A parked upload offered its file again: the same name, size and modification time, or nothing is sent (app.js `xferResume`).
pub fn resume_check(u: &Value, picked: &Local) -> Result<(), String> {
    let (name, size, lm) = (u["name"].as_str().unwrap_or(""), u["size"].as_u64().unwrap_or(0), u["last_modified"].as_u64().unwrap_or(0));
    if picked.name.trim() != name { return Err(format!("That is {}, not {}.", picked.name, name)); }
    if picked.size != size { return Err(format!("That file is {}, but the one being sent was {}.", fmt_size(picked.size), fmt_size(size))); }
    if picked.last_modified != lm { return Err("That file was changed after the upload began, so it is not the same file. Nothing was sent.".into()); }
    Ok(())
}

/// Resume or start a large upload (app.js `uploadLarge`). `name` overrides the file's own (a continued upload keeps the sent name).
pub fn upload_large(core: &Arc<Core>, gid: &str, l: &Local, name: Option<&str>) {
    let name = name.unwrap_or(&l.name).to_string();
    let tmp = format!("u?{gid}|{name}|{}|{}", l.size, l.last_modified);
    let x = Xfer::new(tmp, Kind::Up, &name, gid, l.size);
    if !add(&x) { return; }
    let r = upload_large_run(core, gid, l, &name, &x);
    match r {
        Ok(true) => { say(format!("{name} sent.")); }
        Ok(false) => { say("Upload cancelled."); }
        Err(e) => {
            if x.is_cancelled() {
                let id = x.id.lock().unwrap().clone();
                if !id.is_empty() { let _ = core.call("file_upload_cancel", json!({"id": id})); }
                say("Upload cancelled.");
            } else { say(format!("Upload of {name} stopped: {}. It is kept safe; press continue in Transfers to carry on.", clean_err(&e))); }
        }
    }
    remove(&x);
    refresh_parked(core);
}

/// Ok(true): sent. Ok(false): cancelled cleanly.
fn upload_large_run(core: &Arc<Core>, gid: &str, l: &Local, name: &str, x: &Arc<Xfer>) -> Result<bool, String> {
    let mime = { let mut head = [0u8; 16]; let n = std::fs::File::open(&l.path).and_then(|mut f| f.read(&mut head)).unwrap_or(0); mime_of(name, &head[..n]) };
    let begin = |x: &Arc<Xfer>| -> Result<Value, String> {
        let info = step(x, || core.call("file_upload_begin", json!({"gid": gid, "name": name, "mime": mime, "size": l.size, "last_modified": l.last_modified, "caption": ""})))?;
        let id = info["id"].as_str().unwrap_or("").to_string();
        let chunks = info["chunks"].as_u64().unwrap_or(0); let cs = info["chunk_size"].as_u64().unwrap_or(1);
        let need = info["need_read"].as_array().map(|a| a.len() as u64).unwrap_or(0);
        *x.id.lock().unwrap() = id;
        x.with(|s| { s.expires = info["expires"].as_f64().unwrap_or(0.); s.safe = chunks.saturating_sub(need) * cs; s.bytes = s.safe.min(l.size); });
        x.mark();
        Ok(info)
    };
    let mut file = std::fs::File::open(&l.path).map_err(|_| format!("{} could not be read.", l.name))?;
    let mut slice = |cs: u64, i: u64| -> Result<Vec<u8>, String> {
        let at = i * cs; let len = cs.min(l.size.saturating_sub(at)) as usize;
        let mut buf = vec![0u8; len];
        file.seek(SeekFrom::Start(at)).and_then(|_| file.read_exact(&mut buf)).map_err(|e| format!("{} could not be read: {e}", l.name))?;
        Ok(buf)
    };
    let mut info = begin(x)?;
    let ids = |k: &str, info: &Value| -> Vec<u64> { info[k].as_array().map(|a| a.iter().filter_map(|v| v.as_u64()).collect()).unwrap_or_default() };
    let id = info["id"].as_str().unwrap_or("").to_string();
    let cs = info["chunk_size"].as_u64().unwrap_or(1);
    let (mut need, verify) = (ids("need_read", &info), ids("verify", &info));
    if !need.is_empty() && !verify.is_empty() {
        // honest resume: re-read a sample of what was already sent, and refuse if the file is not the same one
        x.with(|s| s.state = "checking".into());
        for i in verify { let b = slice(cs, i)?; step(x, || core.call("file_upload_check", json!({"id": id, "index": i, "data": b64u(&b)})))?; }
        x.with(|s| s.state.clear());
    }
    let mut rebegun = false;
    let mut n = 0usize;
    while n < need.len() {
        if x.is_cancelled() { break; }
        let i = need[n];
        let buf = slice(cs, i)?;
        match step(x, || core.call("file_upload_chunk", json!({"id": id, "index": i, "data": b64u(&buf)}))) {
            Ok(_) => {}
            Err(e) => {
                // the relay lost its copy (a restored backup, an expiry): start over from what it says it has
                if !rebegun && e.contains("no such file") { rebegun = true; info = begin(x)?; need = ids("need_read", &info); n = 0; continue; }
                return Err(e);
            }
        }
        n += 1;
        x.with(|s| s.bytes = (s.safe + n as u64 * cs).min(l.size)); x.mark();
    }
    if x.is_cancelled() { let _ = core.call("file_upload_cancel", json!({"id": id})); return Ok(false); }
    step(x, || core.call("file_upload_finish", json!({"id": id})))?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------- the core's list of unfinished transfers

#[derive(Clone, Default)]
pub struct Parked { pub uploads: Vec<Value>, pub downloads: Vec<Value> }
static PARKED: Mutex<Parked> = Mutex::new(Parked { uploads: Vec::new(), downloads: Vec::new() });
static TRIED: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The core's unfinished transfers that no thread of this window is working on.
pub fn parked() -> Parked {
    let p = PARKED.lock().unwrap().clone();
    let l = LIVE.lock().unwrap();
    let busy = |id: &str| l.iter().any(|x| *x.id.lock().unwrap() == id);
    Parked { uploads: p.uploads.into_iter().filter(|u| !busy(u["id"].as_str().unwrap_or(""))).collect(), downloads: p.downloads.into_iter().filter(|u| !l.iter().any(|x| x.key == format!("d:{}", u["id"].as_str().unwrap_or("")))).collect() }
}
pub fn parked_upload(id: &str) -> Option<Value> { PARKED.lock().unwrap().uploads.iter().find(|u| u["id"] == id).cloned() }
pub fn parked_download(id: &str) -> Option<Value> { PARKED.lock().unwrap().downloads.iter().find(|u| u["id"] == id).cloned() }

/// app.js `xferRefresh`: what the core lists, and the finish of an upload whose parts are all on the relay.
pub fn refresh_parked(core: &Arc<Core>) {
    let Ok(r) = core.call("file_transfers", json!({})) else { return };
    let arr = |k: &str| r[k].as_array().cloned().unwrap_or_default();
    let (ups, downs) = (arr("uploads"), arr("downloads"));
    {
        let mut p = PARKED.lock().unwrap();
        if p.uploads != ups || p.downloads != downs { p.uploads = ups.clone(); p.downloads = downs; bump(); }
    }
    for u in ups {
        let id = u["id"].as_str().unwrap_or("").to_string();
        let live = LIVE.lock().unwrap().iter().any(|x| *x.id.lock().unwrap() == id);
        if u["ready"].as_bool() == Some(true) && !live && !TRIED.lock().unwrap().contains(&id) {
            // ask the relay what it holds (the saved count can lag a crash); if it has everything, send the message
            let Ok(sy) = core.call("file_upload_sync", json!({"id": id})) else { continue };
            let have = sy["uploaded"].as_array().map(|a| a.len() as u64).unwrap_or(0);
            if sy["complete"].as_bool() != Some(true) && have < sy["chunks"].as_u64().unwrap_or(u64::MAX) { continue; }
            TRIED.lock().unwrap().push(id.clone());
            if core.call("file_upload_finish", json!({"id": id})).is_ok() { say(format!("{} sent.", u["name"].as_str().unwrap_or("File"))); }
        }
    }
}

/// Throw away a parked transfer (what Cancel does on a row of the list).
pub fn drop_parked(core: &Arc<Core>, up: bool, id: &str) {
    let m = if up { "file_upload_cancel" } else { "file_download_cancel" };
    if let Err(e) = core.call(m, json!({"id": id})) { say(clean_err(&e)); }
    refresh_parked(core);
}

// ---------------------------------------------------------------------------------------------- downloads

pub struct Meta { pub name: String, pub save_name: String, pub mime: String, pub size: u64 }

/// A small file's bytes in the core's pieces (`file_open`, then `file_read`); `sink` gets each piece in order.
pub fn fetch(core: &Core, gid: &str, id: &str, mut sink: impl FnMut(&[u8]) -> Result<(), String>) -> Result<Meta, String> {
    let f = core.call("file_open", json!({"gid": gid, "id": id}))?;
    let (size, chunk) = (f["size"].as_u64().unwrap_or(0), f["chunk"].as_u64().unwrap_or(1 << 20).max(1));
    let mut at = 0u64;
    while at < size {
        let part = unb64(core.call("file_read", json!({"gid": gid, "id": id, "offset": at}))?["data"].as_str().unwrap_or(""))?;
        if part.is_empty() { return Err("The file ended early.".into()); }
        sink(&part)?;
        at += chunk;
    }
    Ok(Meta { name: f["name"].as_str().unwrap_or("").into(), save_name: f["save_name"].as_str().unwrap_or("file").into(), mime: f["mime"].as_str().unwrap_or("").into(), size })
}

/// A small file saved into `folder` (the web hands it to the browser's downloads).
pub fn save_small(core: Arc<Core>, gid: String, id: String, folder: PathBuf) {
    std::thread::Builder::new().name("mega-save".into()).spawn(move || {
        let r = (|| -> Result<PathBuf, String> {
            let mut buf = vec![];
            let m = fetch(&core, &gid, &id, |p| { buf.extend_from_slice(p); Ok(()) })?;
            std::fs::create_dir_all(&folder).map_err(|e| format!("{} could not be made: {e}", folder.display()))?;
            let to = unique_in(&folder, &m.save_name);
            std::fs::write(&to, &buf).map_err(|e| format!("{} could not be written: {e}", to.display()))?;
            Ok(to)
        })();
        match r { Ok(p) => say(format!("Saved to {}", p.display())), Err(e) => say(clean_err(&e)) }
    }).ok();
}

/// Save a large file (app.js `downloadLarge`): `folder` is empty when a partly saved file carries on where it is.
pub fn download_large(core: Arc<Core>, gid: String, id: String, name: String, folder: String) {
    std::thread::Builder::new().name("mega-download".into()).spawn(move || {
        let x = Xfer::new(format!("d:{id}"), Kind::Down, &name, &gid, 0);
        if !add(&x) { return; }
        *x.id.lock().unwrap() = id.clone();
        let r = (|| -> Result<Option<String>, String> {
            let st = step(&x, || core.call("file_download_begin", json!({"gid": gid, "id": id, "folder": folder})))?;
            if st["finished"].as_bool() == Some(true) { return Ok(Some(format!("Already saved to {}", st["path"].as_str().unwrap_or("")))); }
            let (chunks, size, cs) = (st["chunks"].as_u64().unwrap_or(0), st["size"].as_u64().unwrap_or(0), st["chunk_size"].as_u64().unwrap_or(1));
            let mut done = st["done"].as_u64().unwrap_or(0);
            x.with(|s| { s.total = size; s.expires = st["expires"].as_f64().unwrap_or(0.); s.bytes = (done * cs).min(size); s.state.clear(); });
            x.mark();
            let mut path = String::new();
            while done < chunks && !x.is_cancelled() {
                let r = step(&x, || core.call("file_download_chunk", json!({"gid": gid, "id": id})))?;
                done = r["done"].as_u64().unwrap_or(done);
                x.with(|s| s.bytes = (done * cs).min(size)); x.mark();
                if r["finished"].as_bool() == Some(true) { path = r["path"].as_str().unwrap_or("").to_string(); break; }
            }
            if x.is_cancelled() { return Ok(None); }
            x.with(|s| s.path = path.clone());
            Ok(Some(format!("Saved to {path}")))
        })();
        match r {
            Ok(Some(t)) => say(t),
            Ok(None) => { let _ = core.call("file_download_cancel", json!({"id": id})); say("Download cancelled."); }
            Err(e) => {
                if x.is_cancelled() { let _ = core.call("file_download_cancel", json!({"id": id})); say("Download cancelled."); }
                else { say(format!("Download stopped: {}. What is saved is kept; press continue in Transfers to carry on.", clean_err(&e))); }
            }
        }
        remove(&x);
        refresh_parked(&core);
    }).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn sizes_and_times_read_as_the_page_writes_them() {
        assert_eq!(fmt_size(0), "0 B"); assert_eq!(fmt_size(1536), "1.5 KB"); assert_eq!(fmt_size(50 * 1048576), "50.0 MB"); assert_eq!(fmt_size(3 << 30), "3.00 GB");
        assert_eq!(fmt_dur(12.2), "13s"); assert_eq!(fmt_dur(90.), "2 min"); assert_eq!(fmt_dur(7200.), "2.0 h");
        assert_eq!(fmt_left(unix() - 5.), "expired");
        assert_eq!(fmt_left(unix() + 90.), "2 min left");
    }
    #[::core::prelude::v1::test]
    fn only_transient_failures_are_waited_out() {
        assert!(is_retry("[retry] relay busy") && is_retry("locked") && is_retry("Network is unreachable") && is_retry("Failed to fetch"));
        assert!(!is_retry("file too large") && !is_retry("no such file"));
        assert_eq!(clean_err("[retry] relay busy"), "relay busy");
    }
    #[::core::prelude::v1::test]
    fn the_type_comes_from_the_name_then_the_bytes() {
        assert_eq!(mime_of("a.PNG", b""), "image/png"); assert_eq!(mime_of("clip.mp4", b""), "video/mp4"); assert_eq!(mime_of("t.opus", b""), "audio/opus");
        assert_eq!(mime_of("noext", &[0x89, b'P', b'N', b'G', 0, 0]), "image/png"); assert_eq!(mime_of("noext", b"xx"), "application/octet-stream");
    }
    #[::core::prelude::v1::test]
    fn a_pasted_picture_gets_a_name_with_the_time() {
        let n = pasted_name("png");
        assert!(n.starts_with("Pasted image 20") && n.ends_with(".png") && n.len() == "Pasted image 2026-11-19 23.04.05.png".len(), "{n}");
    }
    #[::core::prelude::v1::test]
    fn a_taken_name_gets_a_number() {
        let d = std::env::temp_dir().join(format!("mega-xfer-test-{}", std::process::id())); std::fs::create_dir_all(&d).unwrap();
        assert_eq!(unique_in(&d, "a.txt"), d.join("a.txt"));
        std::fs::write(d.join("a.txt"), b"x").unwrap();
        assert_eq!(unique_in(&d, "a.txt"), d.join("a (1).txt"));
        std::fs::write(d.join("a (1).txt"), b"x").unwrap();
        assert_eq!(unique_in(&d, "a.txt"), d.join("a (2).txt"));
        std::fs::write(d.join("noext"), b"x").unwrap();
        assert_eq!(unique_in(&d, "noext"), d.join("noext (1)"));
        let _ = std::fs::remove_dir_all(&d);
    }
    #[::core::prelude::v1::test]
    fn a_continued_upload_must_be_the_same_file() {
        let u = json!({"name": "big.bin", "size": 100, "last_modified": 7});
        let ok = Local { path: "/x".into(), name: "big.bin".into(), size: 100, last_modified: 7 };
        assert!(resume_check(&u, &ok).is_ok());
        assert!(resume_check(&u, &Local { name: "other.bin".into(), ..Local { path: "/x".into(), name: String::new(), size: 100, last_modified: 7 } }).unwrap_err().contains("not big.bin"));
        assert!(resume_check(&u, &Local { size: 99, ..Local { path: "/x".into(), name: "big.bin".into(), size: 0, last_modified: 7 } }).unwrap_err().contains("99 B"));
        assert!(resume_check(&u, &Local { last_modified: 8, ..Local { path: "/x".into(), name: "big.bin".into(), size: 100, last_modified: 0 } }).unwrap_err().contains("changed"));
    }
    #[::core::prelude::v1::test]
    fn a_pause_is_waited_out_and_a_transient_error_retried() {
        let x = Xfer::new("t".into(), Kind::Up, "f", "g", 10);
        let mut n = 0;
        // a refusal stops at once
        assert_eq!(step(&x, || -> Result<(), String> { n += 1; Err("file too large".into()) }), Err("file too large".into())); assert_eq!(n, 1);
        // a cancel stops a step before it starts
        x.cancelled.store(true, Ordering::Release);
        assert_eq!(step(&x, || Ok(1)), Err("cancelled".into()));
        // a transient failure is tried again (the first wait is a second)
        let y = Xfer::new("t2".into(), Kind::Up, "f", "g", 10);
        let mut tries = 0;
        let r = step(&y, || { tries += 1; if tries < 2 { Err("[retry] busy".to_string()) } else { Ok(tries) } });
        assert_eq!(r, Ok(2)); assert!(y.snap().state.is_empty());
    }
}
