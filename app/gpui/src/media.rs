//! Pictures, video posters and the decrypted files that video and audio play from (web reference: app.js `fillImages`,
//! `fileBlob`, `playMedia`; PORT-PLAN.md section 5).
//!
//! A picture is fetched in the core's pieces and decoded on a worker thread, never the window's: once as a thumbnail that
//! fits the message column at twice its size (what the row shows), and, only while the lightbox has it open, once as a large
//! copy. The newest `MAX_PICS` thumbnails stay, the rest go back to the core's cache (the web keeps 48 object URLs). Nothing
//! large is fetched just because it scrolled by (a large picture needs Download first), as the page does.
//!
//! Video and audio play from a decrypted copy on disk (tmpfs under the runtime directory where there is one, removed when
//! the app closes), because ffmpeg wants a file it can seek in. A video's first frame is the poster; the window asks for it
//! when the row is drawn, the sound of an audio file only when Play is pressed (the page fetches nothing before Play).
use crate::core::Core;
use gpui_kit::RenderImage;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Thumbnails kept (app.js `MAX_IMGS`) and decrypted media files kept on disk (app.js `MAX_MEDIA`).
pub const MAX_PICS: usize = 48;
pub const MAX_MEDIA: usize = 6;
/// A thumbnail is at most this many pixels: twice what the column shows at most (460 x 320 logical).
pub const THUMB: (u32, u32) = (920, 640);
/// The lightbox's copy: no side longer than this (a GPU texture and a phone photo's 4000 px are both fine at this).
pub const FULL: u32 = 3072;

#[derive(Clone)]
pub struct Ready { pub img: Arc<RenderImage>, /** the picture's own size */ pub w: u32, pub h: u32 }
#[derive(Clone)]
pub enum Pic { Loading, Ready(Ready), Failed(String) }

#[derive(Clone, Default)]
pub struct Media {
    /// "fetching" | "ready" | "failed"
    pub state: &'static str, pub err: String, pub path: Option<PathBuf>,
    pub dur_ms: u64, pub poster: Option<Ready>, pub has_audio: bool, pub has_video: bool,
}

enum Job { Thumb(String, String), Full(String, String), Media(String, String, String) }

#[derive(Default)]
struct Store {
    pics: HashMap<String, Pic>, order: VecDeque<String>,
    full: VecDeque<(String, Ready)>, full_asked: HashSet<String>,
    media: HashMap<String, Media>, media_order: VecDeque<String>,
    /// Images that left the store: the window hands them back to the sprite atlas (`take_retired`).
    retired: Vec<Arc<RenderImage>>,
    queue: VecDeque<Job>, workers: usize,
}
static STORE: Mutex<Option<Store>> = Mutex::new(None);
static WAKE: Condvar = Condvar::new();
static GEN: AtomicU64 = AtomicU64::new(1);
static CORE: OnceLock<Arc<Core>> = OnceLock::new();
/// One big decode at a time: three of a 48 megapixel photo at once would be a gigabyte.
static DECODING: Mutex<()> = Mutex::new(());

fn with<T>(f: impl FnOnce(&mut Store) -> T) -> T { let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner()); f(g.get_or_insert_with(Store::default)) }
pub fn generation() -> u64 { GEN.load(Ordering::Acquire) }
fn bump() { GEN.fetch_add(1, Ordering::AcqRel); }
pub fn init(core: Arc<Core>) { let _ = CORE.set(core); clean_dir(); }

// ---------------------------------------------------------------------------------------------- the queue

fn push(job: Job) {
    let spawn = with(|s| { s.queue.push_back(job); if s.workers < 3 { s.workers += 1; true } else { false } });
    if spawn { std::thread::Builder::new().name("mega-media".into()).spawn(worker).ok(); }
    WAKE.notify_one();
}
fn worker() {
    loop {
        let job = {
            let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(j) = g.get_or_insert_with(Store::default).queue.pop_front() { break j; }
                g = WAKE.wait(g).unwrap_or_else(|e| e.into_inner());
            }
        };
        let Some(core) = CORE.get().cloned() else { continue };
        match job {
            Job::Thumb(gid, id) => {
                let r = fetch_bytes(&core, &gid, &id).and_then(|b| decode(&b, THUMB.0.max(THUMB.1)).map(|(i, w, h)| (to_gpu(i, THUMB.0, THUMB.1), w, h)));
                with(|s| {
                    let p = match r { Ok((img, w, h)) => Pic::Ready(Ready { img, w, h }), Err(e) => Pic::Failed(e) };
                    s.pics.insert(id.clone(), p);
                    s.order.retain(|k| *k != id); s.order.push_back(id);
                    while s.order.len() > MAX_PICS { if let Some(k) = s.order.pop_front() { if let Some(Pic::Ready(r)) = s.pics.remove(&k) { s.retired.push(r.img); } } }
                });
            }
            Job::Full(gid, id) => {
                let r = fetch_bytes(&core, &gid, &id).and_then(|b| decode(&b, FULL).map(|(i, w, h)| (to_gpu(i, FULL, FULL), w, h)));
                with(|s| {
                    s.full_asked.remove(&id);
                    if let Ok((img, w, h)) = r { s.full.retain(|(k, _)| *k != id); s.full.push_back((id, Ready { img, w, h })); }
                    while s.full.len() > 2 { if let Some((_, r)) = s.full.pop_front() { s.retired.push(r.img); } }
                });
            }
            Job::Media(gid, id, ext) => media_job(&core, &gid, &id, &ext),
        }
        bump();
    }
}

fn fetch_bytes(core: &Core, gid: &str, id: &str) -> Result<Vec<u8>, String> {
    let mut buf = vec![];
    crate::xfer::fetch(core, gid, id, |p| { buf.extend_from_slice(p); Ok(()) }).map_err(|e| crate::xfer::clean_err(&e))?;
    Ok(buf)
}

// ---------------------------------------------------------------------------------------------- pictures

/// Decode with the picture's own orientation, no side longer than `max`.
pub fn decode(bytes: &[u8], max: u32) -> Result<(image::RgbaImage, u32, u32), String> {
    let _one = DECODING.lock().unwrap_or_else(|e| e.into_inner());
    let bad = || "This picture can't be shown here. Use Download.".to_string();
    let mut rd = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_| bad())?;
    let mut lim = image::Limits::default(); lim.max_alloc = Some(1 << 30); rd.limits(lim);
    let mut dec = rd.into_decoder().map_err(|_| bad())?;
    let o = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec).map_err(|_| bad())?;
    img.apply_orientation(o);
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 { return Err(bad()); }
    if w.max(h) > max { img = img.resize(max, max, FilterType::Triangle); }
    Ok((img.into_rgba8(), w, h))
}
/// Fit inside `bw` x `bh` without enlarging.
fn to_gpu(img: image::RgbaImage, bw: u32, bh: u32) -> Arc<RenderImage> {
    let (w, h) = img.dimensions();
    let mut img = if w > bw || h > bh { let k = (bw as f32 / w as f32).min(bh as f32 / h as f32); image::imageops::resize(&img, ((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1), FilterType::Triangle) } else { img };
    for p in img.chunks_exact_mut(4) { p.swap(0, 2); }
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(img)]))
}

/// The thumbnail of picture message `id`; starts the fetch the first time it is asked for.
pub fn thumb(gid: &str, id: &str) -> Pic {
    let (p, ask) = with(|s| match s.pics.get(id) { Some(p) => (p.clone(), false), None => { s.pics.insert(id.to_string(), Pic::Loading); (Pic::Loading, true) } });
    if ask { push(Job::Thumb(gid.to_string(), id.to_string())); }
    p
}
pub fn thumb_peek(id: &str) -> Option<Pic> { with(|s| s.pics.get(id).cloned()) }
/// The large copy, or None while it is on its way (then the thumbnail stands in).
pub fn full(gid: &str, id: &str) -> Option<Ready> {
    let (r, ask) = with(|s| match s.full.iter().find(|(k, _)| k == id) { Some((_, r)) => (Some(r.clone()), false), None => { let a = s.full_asked.insert(id.to_string()); (None, a) } });
    if ask { push(Job::Full(gid.to_string(), id.to_string())); }
    r
}
/// A picture's pixels were wanted again (it failed): forget the failure so the next draw asks again.
pub fn retry(id: &str) { with(|s| { if matches!(s.pics.get(id), Some(Pic::Failed(_))) { s.pics.remove(id); } }); bump(); }
pub fn take_retired() -> Vec<Arc<RenderImage>> { with(|s| std::mem::take(&mut s.retired)) }
/// Ids whose picture is held (tests).
pub fn held() -> Vec<(String, &'static str, u32, u32)> {
    with(|s| { let mut v: Vec<_> = s.pics.iter().map(|(k, p)| match p { Pic::Loading => (k.clone(), "loading", 0, 0), Pic::Ready(r) => (k.clone(), "ready", r.w, r.h), Pic::Failed(_) => (k.clone(), "failed", 0, 0) }).collect(); v.sort(); v })
}

// ---------------------------------------------------------------------------------------------- ffmpeg

/// Where a tool lives: `MEGA_GPUI_FFMPEG` (a folder) first, then the PATH, then the places a desktop program is not told about.
pub fn tool(name: &str) -> Option<PathBuf> {
    static CACHE: Mutex<Option<HashMap<String, Option<PathBuf>>>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    c.get_or_insert_with(Default::default).entry(name.to_string()).or_insert_with(|| {
        let mut dirs: Vec<PathBuf> = vec![];
        if let Some(d) = std::env::var_os("MEGA_GPUI_FFMPEG") { dirs.push(d.into()); }
        if let Some(p) = std::env::var_os("PATH") { dirs.extend(std::env::split_paths(&p)); }
        dirs.extend(["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin", "/opt/local/bin", "/snap/bin"].iter().map(PathBuf::from));
        dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
    }).clone()
}
pub fn has_ffmpeg() -> bool { tool("ffmpeg").is_some() && tool("ffprobe").is_some() }

/// What `ffprobe -print_format json -show_entries format=duration:stream=codec_type` says: (milliseconds, has audio, has video).
pub fn parse_probe(json: &str) -> (u64, bool, bool) {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let dur = v["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).or_else(|| v["format"]["duration"].as_f64()).unwrap_or(0.);
    let has = |k: &str| v["streams"].as_array().is_some_and(|l| l.iter().any(|s| s["codec_type"] == k && s["disposition"]["attached_pic"].as_u64() != Some(1)));
    ((dur * 1000.).round().max(0.) as u64, has("audio"), has("video"))
}
fn probe(path: &Path) -> (u64, bool, bool) {
    let Some(ff) = tool("ffprobe") else { return (0, false, false) };
    let o = std::process::Command::new(ff).args(["-v", "error", "-print_format", "json", "-show_entries", "format=duration:stream=codec_type:stream_disposition=attached_pic", "-i"]).arg(path)
        .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
    o.ok().map(|o| parse_probe(&String::from_utf8_lossy(&o.stdout))).unwrap_or((0, false, false))
}
/// The first picture of a video (a little in, where a fade has passed), at most 960 wide, as the video is meant to be seen (rotation applied by ffmpeg).
fn poster(path: &Path, dur_ms: u64) -> Option<Ready> {
    let ff = tool("ffmpeg")?;
    let at = if dur_ms > 2000 { 0.4 } else { 0.0 };
    let grab = |ss: f64| -> Option<Vec<u8>> {
        let o = std::process::Command::new(&ff).args(["-v", "error", "-ss", &format!("{ss}"), "-i"]).arg(path)
            .args(["-frames:v", "1", "-vf", "scale='min(960,iw)':-2", "-f", "image2pipe", "-vcodec", "png", "pipe:1"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
        (!o.stdout.is_empty()).then_some(o.stdout)
    };
    let bytes = grab(at).or_else(|| grab(0.))?;
    let (img, w, h) = decode(&bytes, 960).ok()?;
    Some(Ready { img: to_gpu(img, 960, 960), w, h })
}

// ---------------------------------------------------------------------------------------------- decrypted copies for playing

/// tmpfs where there is one: a decrypted file does not stay on a disk.
pub fn dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(std::env::temp_dir);
    base.join(format!("mega-gpui-media-{}", unsafe_uid()))
}
fn unsafe_uid() -> String { std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "u".into()).chars().filter(|c| c.is_ascii_alphanumeric()).collect() }
fn clean_dir() { let _ = std::fs::remove_dir_all(dir()); }
/// At exit and at start: no decrypted copy survives the app.
pub fn clean_up() { clean_dir(); }
pub fn safe_name(id: &str) -> String { id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(80).collect() }

pub fn media_of(id: &str) -> Option<Media> { with(|s| s.media.get(id).cloned()) }
/// Ask for the decrypted copy of `id` (and its length, and a video's poster). Already asked: nothing happens.
pub fn want_media(gid: &str, id: &str, name: &str) {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("bin").chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect::<String>();
    let ask = with(|s| { if s.media.contains_key(id) { false } else { s.media.insert(id.to_string(), Media { state: "fetching", ..Default::default() }); true } });
    if ask { push(Job::Media(gid.to_string(), id.to_string(), ext)); }
}
/// Fetching failed or the copy is gone: the next `want_media` starts again.
pub fn forget_media(id: &str) { with(|s| { s.media.remove(id); s.media_order.retain(|k| k != id); }); bump(); }

fn media_job(core: &Arc<Core>, gid: &str, id: &str, ext: &str) {
    let d = dir();
    let r = (|| -> Result<(PathBuf, (u64, bool, bool), Option<Ready>), String> {
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)); }
        let path = d.join(format!("{}.{ext}", safe_name(id)));
        let part = path.with_extension(format!("{ext}.part"));
        let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        crate::xfer::fetch(core, gid, id, |p| f.write_all(p).map_err(|e| e.to_string())).map_err(|e| crate::xfer::clean_err(&e))?;
        f.flush().map_err(|e| e.to_string())?; drop(f);
        std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
        let pr = probe(&path);
        let post = if pr.2 { poster(&path, pr.0) } else { None };
        Ok((path, pr, post))
    })();
    with(|s| {
        match r {
            Ok((path, (dur_ms, has_audio, has_video), poster)) => {
                s.media.insert(id.to_string(), Media { state: "ready", path: Some(path), dur_ms, poster, has_audio, has_video, err: String::new() });
                s.media_order.retain(|k| k != id); s.media_order.push_back(id.to_string());
                while s.media_order.len() > MAX_MEDIA {
                    if let Some(k) = s.media_order.pop_front() { if let Some(m) = s.media.remove(&k) { if let Some(p) = m.path { let _ = std::fs::remove_file(p); } if let Some(p) = m.poster { s.retired.push(p.img); } } }
                }
            }
            Err(e) => { s.media.insert(id.to_string(), Media { state: "failed", err: e, ..Default::default() }); }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255]));
        let mut o = Vec::new(); image::DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut o), image::ImageFormat::Png).unwrap(); o
    }
    #[::core::prelude::v1::test]
    fn a_big_picture_is_shrunk_to_what_the_column_can_show_and_keeps_its_own_size() {
        let (img, w, h) = decode(&png(3000, 1500), 920).unwrap();
        assert_eq!((w, h), (3000, 1500), "the picture's own size is reported");
        assert_eq!(img.width(), 920);
        let t = to_gpu(img, THUMB.0, THUMB.1);
        let s = t.size(0);
        assert!(s.width.0 as u32 <= THUMB.0 && s.height.0 as u32 <= THUMB.1);
        let small = to_gpu(decode(&png(100, 50), 920).unwrap().0, THUMB.0, THUMB.1);
        assert_eq!(small.size(0).width.0, 100, "a small picture is never enlarged");
    }
    #[::core::prelude::v1::test]
    fn something_that_is_not_a_picture_says_so() { assert!(decode(b"not a picture", 920).unwrap_err().contains("Download")); }
    #[::core::prelude::v1::test]
    fn the_probe_answer_is_read() {
        let j = r#"{"streams":[{"codec_type":"video","disposition":{"attached_pic":0}},{"codec_type":"audio"}],"format":{"duration":"3.504000"}}"#;
        assert_eq!(parse_probe(j), (3504, true, true));
        let art = r#"{"streams":[{"codec_type":"video","disposition":{"attached_pic":1}},{"codec_type":"audio"}],"format":{"duration":"61.0"}}"#;
        assert_eq!(parse_probe(art), (61000, true, false), "cover art is not a video");
        assert_eq!(parse_probe("garbage"), (0, false, false));
    }
    #[::core::prelude::v1::test]
    fn a_message_id_is_a_safe_file_name() { assert_eq!(safe_name("a/b..c=d"), "a_b__c_d"); assert!(safe_name(&"x".repeat(300)).len() <= 80); }
}
