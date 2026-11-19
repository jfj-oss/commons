//! Profile pictures (web reference: app/ui/avatar.js, and `fillAvatars` / `refreshAvatars` in app.js).
//!
//! The core holds one reference per account (`avatar_versions`: account -> blob id) and the encrypted bytes
//! (`avatar_data`). This file does what the page does with them:
//!   * show: the versions are asked for every few seconds; a picture is downloaded and decoded only when its blob id is new,
//!     decoded off the UI thread, masked to the shape it was sent with (circle or squircle) and kept as GPU images. A person
//!     without a picture keeps the letter seal.
//!   * set: pick (dialog or drop), decode with the photo's own orientation, crop with drag and zoom to a square exactly as
//!     avatar.js does (256 x 256 stage, zoom 1..4, the picture always covers it), shrink to a JPEG under 40 KB (the page's
//!     fallback when its web view cannot write WebP: same `image/jpeg` + `shape` fields, so web and iOS show it), and send it
//!     through `set_avatar`. `remove_avatar` goes back to the seal.
use crate::app::Mega;
use crate::look;
use crate::squircle as sq;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, RgbaImage};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The picture the page makes: 256 x 256, at most 40 KB.
pub const SIZE: u32 = 256;
pub const MAX_BYTES: usize = 40 * 1024;
/// Zoom limits of the cropper (avatar.js `setZoom`).
pub const ZOOM_MIN: f32 = 1.;
pub const ZOOM_MAX: f32 = 4.;
/// The larger side of a picture kept for cropping (a phone photo is 4000 px; the crop never needs more than this).
const WORK: u32 = 1536;
/// Below this many device pixels the small copy is used (a 256 px picture drawn at 36 px aliases without a prefilter).
const SMALL: u32 = 96;
/// Corner of a square-shaped picture, as a share of its side (the seal's squircle: 11 px at 36 px).
const SQ_R: f32 = 0.3;

// ------------------------------------------------------------------ the crop, as numbers

/// The cropper's state in avatar.js terms: `zoom` over the cover scale, and the offset of the picture's centre from the
/// stage's centre in stage pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop { pub zoom: f32, pub x: f32, pub y: f32, pub circle: bool }

impl Default for Crop { fn default() -> Crop { Crop { zoom: 1., x: 0., y: 0., circle: true } } }

/// Pixels of the stage per pixel of the picture at zoom 1: the picture's short side fills the stage.
pub fn base(w: u32, h: u32) -> f32 { SIZE as f32 / w.min(h).max(1) as f32 }

impl Crop {
    /// Keep the picture covering the stage: zoom within its limits, the offset within what the picture's overhang allows.
    pub fn clamped(mut self, w: u32, h: u32) -> Crop {
        self.zoom = if self.zoom.is_finite() { self.zoom.clamp(ZOOM_MIN, ZOOM_MAX) } else { 1. };
        let s = base(w, h) * self.zoom;
        let mx = ((w as f32 * s - SIZE as f32) / 2.).max(0.);
        let my = ((h as f32 * s - SIZE as f32) / 2.).max(0.);
        self.x = if self.x.is_finite() { self.x.clamp(-mx, mx) } else { 0. };
        self.y = if self.y.is_finite() { self.y.clamp(-my, my) } else { 0. };
        self
    }
    /// Where the picture is drawn on the stage: (left, top, width, height) in stage pixels.
    pub fn placed(&self, w: u32, h: u32) -> (f32, f32, f32, f32) {
        let s = base(w, h) * self.zoom;
        let (dw, dh) = (w as f32 * s, h as f32 * s);
        ((SIZE as f32 - dw) / 2. + self.x, (SIZE as f32 - dh) / 2. + self.y, dw, dh)
    }
    /// The square of the picture that lands on the stage, in picture pixels: (left, top, side).
    pub fn source(&self, w: u32, h: u32) -> (f32, f32, f32) {
        let (l, t, _, _) = self.placed(w, h);
        let s = base(w, h) * self.zoom;
        (-l / s, -t / s, SIZE as f32 / s)
    }
}

/// The 256 x 256 square the crop selects (before any frame), resampled once from the picture itself.
pub fn render_square(src: &RgbaImage, c: &Crop) -> RgbaImage {
    let (w, h) = src.dimensions();
    let c = c.clamped(w, h);
    let (sx, sy, side) = c.source(w, h);
    let x0 = (sx.round().max(0.) as u32).min(w.saturating_sub(1));
    let y0 = (sy.round().max(0.) as u32).min(h.saturating_sub(1));
    let side = (side.round().max(1.) as u32).min(w - x0).min(h - y0).max(1);
    let part = image::imageops::crop_imm(src, x0, y0, side, side).to_image();
    if side == SIZE { part } else { image::imageops::resize(&part, SIZE, SIZE, FilterType::Lanczos3) }
}

/// JPEG bytes under `MAX_BYTES`: the page's ladder (quality steps at 256, then smaller sizes). Transparent parts go black,
/// as they do when a canvas is written as JPEG.
pub fn encode(square: &RgbaImage) -> Result<Vec<u8>, String> {
    let rgb = |img: &RgbaImage| -> image::RgbImage {
        let mut o = image::RgbImage::new(img.width(), img.height());
        for (x, y, p) in img.enumerate_pixels() { let a = p[3] as u32; o.put_pixel(x, y, image::Rgb([(p[0] as u32 * a / 255) as u8, (p[1] as u32 * a / 255) as u8, (p[2] as u32 * a / 255) as u8])); }
        o
    };
    let try_q = |img: &image::RgbImage, q: u8| -> Option<Vec<u8>> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q).encode_image(img).ok()?;
        (out.len() <= MAX_BYTES).then_some(out)
    };
    let full = rgb(square);
    for q in [90, 85, 75, 65, 55, 45] { if let Some(b) = try_q(&full, q) { return Ok(b); } }
    for s in [192u32, 160, 128, 96, 64] {
        let small = image::imageops::resize(&full, s, s, FilterType::Lanczos3);
        for q in [70, 50, 35] { if let Some(b) = try_q(&small, q) { return Ok(b); } }
    }
    Err("This picture could not be shrunk below 40 KB. Try another one.".into())
}

/// What is sent: the bytes, their type and the frame.
pub struct Prepared { pub bytes: Vec<u8>, pub mime: &'static str, pub shape: &'static str }

pub fn prepare(src: &RgbaImage, c: &Crop) -> Result<Prepared, String> {
    let sq = render_square(src, c);
    Ok(Prepared { bytes: encode(&sq)?, mime: "image/jpeg", shape: if c.circle { "circle" } else { "square" } })
}

/// A picture file as it is meant to be seen: its EXIF orientation applied, and no larger than `WORK` on a side.
pub fn decode_file(bytes: &[u8]) -> Result<RgbaImage, String> {
    let not = || "That file is not a picture.".to_string();
    let mut dec = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_| not())?.into_decoder().map_err(|_| not())?;
    let o = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec).map_err(|_| not())?;
    img.apply_orientation(o);
    if img.width() == 0 || img.height() == 0 { return Err(not()); }
    if img.width().max(img.height()) > WORK { img = img.resize(WORK, WORK, FilterType::Triangle); }
    Ok(img.into_rgba8())
}

// ------------------------------------------------------------------ masks and GPU images

/// How much of the pixel (x, y) of a `n` x `n` picture lies inside a superellipse corner of radius `r` and exponent `p`
/// (2 = circle, 4 = the squircle of `squircle.rs`), as 0..1, anti-aliased over one pixel.
fn coverage(x: f32, y: f32, n: f32, r: f32, p: f32) -> f32 {
    let (cx, cy) = (x + 0.5, y + 0.5);
    let dx = (r - cx).max(cx - (n - r)).max(0.);
    let dy = (r - cy).max(cy - (n - r)).max(0.);
    if dx <= 0. || dy <= 0. { return 1.; }
    // distance to the curve ~ (f - 1) * r for f = (a^p + b^p)^(1/p), a, b the corner-box fractions
    let f = ((dx / r).powf(p) + (dy / r).powf(p)).powf(1. / p);
    (0.5 - (f - 1.) * r).clamp(0., 1.)
}

/// Cut the picture to its frame: a circle, or the squircle of the seals.
pub fn mask(img: &mut RgbaImage, circle: bool) {
    let n = img.width() as f32;
    let (r, p) = if circle { (n / 2., 2.) } else { (n * SQ_R, 4.) };
    for (x, y, px_) in img.enumerate_pixels_mut() {
        let c = coverage(x as f32, y as f32, n, r, p);
        if c < 1. { px_[3] = (px_[3] as f32 * c).round() as u8; }
    }
}

fn gpu(mut img: RgbaImage) -> Arc<RenderImage> {
    for p in img.chunks_exact_mut(4) { p.swap(0, 2); }
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(img)]))
}

/// One person's picture as the window draws it.
#[derive(Clone)]
pub struct Pic { pub blob: String, pub circle: bool, small: Arc<RenderImage>, big: Arc<RenderImage> }

impl Pic {
    fn build(blob: &str, shape: &str, bytes: &[u8]) -> Result<Pic, String> {
        let mut img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8();
        let circle = shape != "square";
        if img.width() != img.height() {
            // the page crops with object-fit: cover; a picture that is not square (an old or foreign client) is cropped in the middle
            let s = img.width().min(img.height());
            img = image::imageops::crop_imm(&img, (img.width() - s) / 2, (img.height() - s) / 2, s, s).to_image();
        }
        let big_side = img.width().min(SIZE);
        let mut big = if img.width() != big_side { image::imageops::resize(&img, big_side, big_side, FilterType::Lanczos3) } else { img };
        let mut small = image::imageops::resize(&big, SMALL, SMALL, FilterType::Lanczos3);
        mask(&mut big, circle);
        mask(&mut small, circle);
        Ok(Pic { blob: blob.to_string(), circle, small: gpu(small), big: gpu(big) })
    }
    /// The copy for something drawn `px` logical pixels wide.
    pub fn at(&self, px: f32) -> Arc<RenderImage> { if px <= 48. { self.small.clone() } else { self.big.clone() } }
}

// ------------------------------------------------------------------ the store

#[derive(Default)]
struct Store {
    pics: HashMap<String, Pic>,
    /// Bumped whenever a picture appears, changes or goes: the window redraws when it sees a new number.
    gen: u64,
    /// (account, blob) that could not be fetched or decoded, and when: asked again after a minute.
    failed: HashMap<(String, String), Instant>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);

fn with<T>(f: impl FnOnce(&mut Store) -> T) -> T {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Store::default))
}

pub fn get(account: &str) -> Option<Pic> { if account.is_empty() { None } else { with(|s| s.pics.get(account).cloned()) } }
pub fn generation() -> u64 { with(|s| s.gen) }
/// (account, blob, shape) of everything held (tests).
pub fn held() -> Vec<(String, String, &'static str)> { with(|s| { let mut v: Vec<_> = s.pics.iter().map(|(a, p)| (a.clone(), p.blob.clone(), if p.circle { "circle" } else { "square" })).collect(); v.sort(); v }) }
fn put(account: &str, pic: Pic) { with(|s| { s.pics.insert(account.to_string(), pic); s.gen += 1; }); }
fn drop_own(account: &str) { with(|s| { if s.pics.remove(account).is_some() { s.gen += 1; } }); }

/// Ask the core which pictures exist, drop the ones that went or changed, fetch the new ones. Runs on a thread of its own
/// (`avatar_data` can wait for the relay); one run at a time.
pub fn refresh(core: &crate::core::Core) {
    if BUSY.swap(true, Ordering::AcqRel) { return; }
    struct Done;
    impl Drop for Done { fn drop(&mut self) { BUSY.store(false, Ordering::Release); } }
    let _done = Done;
    let Ok(v) = core.call("avatar_versions", json!({})) else { return };
    let vers: HashMap<String, String> = v.as_object().map(|m| m.iter().filter_map(|(a, b)| b.as_str().map(|b| (a.clone(), b.to_string()))).collect()).unwrap_or_default();
    with(|s| { let before = s.pics.len(); s.pics.retain(|a, p| vers.get(a) == Some(&p.blob)); if s.pics.len() != before { s.gen += 1; } });
    for (acct, blob) in vers {
        let have = with(|s| s.pics.get(&acct).is_some_and(|p| p.blob == blob) || s.failed.get(&(acct.clone(), blob.clone())).is_some_and(|t| t.elapsed() < Duration::from_secs(60)));
        if have { continue; }
        let got = core.call("avatar_data", json!({"account": acct})).and_then(|d| {
            if d["blob"].as_str() != Some(blob.as_str()) { return Err("changed again".into()); } // on its way: the next look takes the new one
            let bytes = commons_b64(d["data"].as_str().unwrap_or(""))?;
            Pic::build(&blob, d["shape"].as_str().unwrap_or("circle"), &bytes)
        });
        match got { Ok(p) => put(&acct, p), Err(_) => with(|s| { s.failed.insert((acct, blob), Instant::now()); }) }
    }
}

fn commons_b64(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|e| e.to_string())
}
fn to_b64(b: &[u8]) -> String { use base64::Engine as _; base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b) }

// ------------------------------------------------------------------ drawing

/// The picture in a `size` px box, cut to its frame, or None when this person has none.
pub fn pic_view(account: &str, size: f32) -> Option<Div> {
    let p = get(account)?;
    Some(div().relative().size(px(size)).flex_none().child(img(ImageSource::Render(p.at(size))).size_full().object_fit(ObjectFit::Fill)))
}

// ------------------------------------------------------------------ the window's side

pub struct Cropping {
    /// The picture, as decoded (orientation applied), and the same as a GPU image for the stage.
    pub src: Arc<RgbaImage>,
    pub view: Arc<RenderImage>,
    pub crop: Crop,
    /// A drag in progress: pointer at the start and the offset then.
    pub drag: Option<(Point<Pixels>, f32, f32)>,
    pub sending: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct AvatarUi {
    pub cropping: Option<Cropping>,
    pub seen: u64,
    pub polled: Option<Instant>,
    pub note: Option<String>,
}

fn choice(id: &'static str, on: bool, text: &'static str) -> Stateful<Div> {
    div().id(id).relative().px_3().py_1().border_1().border_color(hsla(0., 0., 0., 0.)).cursor_pointer()
        .when(on, |d| d.text_color(rgb(0x0c0d12)).child(sq::back(999., look::paper())))
        .child(div().relative().child(text))
        .child(sq::front(999., None, Some((1., if on { look::paper() } else { look::line() })), None))
}

impl Mega {
    pub fn me_account(&self) -> String { self.snap.as_ref().map(|s| s.me_account.clone()).unwrap_or_default() }

    /// Every 700 ms: redraw when a picture arrived, and look for new ones every four seconds (app.js `refreshAvatars`).
    pub fn avatar_tick(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let g = generation();
        if g != self.av.seen { self.av.seen = g; cx.notify(); }
        if self.snap.is_none() || self.screen != crate::app::Screen::Main { return; }
        if self.av.polled.is_some_and(|t| t.elapsed() < Duration::from_secs(4)) { return; }
        self.av.polled = Some(Instant::now());
        self.avatar_poll();
    }
    pub fn avatar_poll(&mut self) {
        let core = self.core.clone();
        std::thread::Builder::new().name("mega-avatars".into()).spawn(move || refresh(&core)).ok();
    }

    /// The file dialog (a platform dialog; on Linux the desktop portal's).
    pub fn avatar_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Choose a picture".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() { let _ = this.update_in(cx, |this, window, cx| this.avatar_open(p, window, cx)); }
            }
        }).detach();
    }

    /// A picture was chosen or dropped: decode it (off the UI thread) and show the cropper.
    pub fn avatar_open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.av.note = None;
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move {
                let bytes = std::fs::read(&path).map_err(|_| "That file could not be read.".to_string())?;
                let img = decode_file(&bytes)?;
                let view = gpu(img.clone());
                Ok::<_, String>((Arc::new(img), view))
            }).await;
            let _ = this.update_in(cx, |this, _, cx| {
                match r {
                    Ok((src, view)) => this.av.cropping = Some(Cropping { src, view, crop: Crop::default(), drag: None, sending: false, error: None }),
                    Err(e) => this.av.note = Some(e),
                }
                cx.notify();
            });
        }).detach();
    }

    pub fn avatar_crop_set(&mut self, f: impl FnOnce(&mut Crop), cx: &mut Context<Self>) {
        if let Some(c) = self.av.cropping.as_mut() { let (w, h) = c.src.dimensions(); f(&mut c.crop); c.crop = c.crop.clamped(w, h); cx.notify(); }
    }

    /// "Use picture": crop, shrink and send (off the UI thread); the window shows it at once, the core announces it.
    pub fn avatar_use(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.av.cropping.as_mut() else { return };
        if c.sending { return; }
        c.sending = true; c.error = None;
        let (src, crop) = (c.src.clone(), c.crop);
        let core = self.core.clone();
        let me = self.me_account();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move {
                let p = prepare(&src, &crop)?;
                let shown = Pic::build("", p.shape, &p.bytes)?;
                let sent = core.call("set_avatar", json!({"mime": p.mime, "shape": p.shape, "data": to_b64(&p.bytes)}))?;
                Ok::<_, String>((sent["blob"].as_str().unwrap_or("").to_string(), shown))
            }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                match r {
                    Ok((blob, mut shown)) => { shown.blob = blob; put(&me, shown); this.av.cropping = None; this.av.note = Some("Profile picture set.".into()); this.snap_hash.clear(); this.avatar_poll(); }
                    Err(e) => if let Some(c) = this.av.cropping.as_mut() { c.sending = false; c.error = Some(e); },
                }
                let _ = window;
                cx.notify();
            });
        }).detach();
    }

    pub fn avatar_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = self.me_account();
        self.rpc("remove_avatar", json!({}), window, cx, move |this, r, _, cx| {
            match r { Ok(_) => { drop_own(&me); this.av.note = Some("Profile picture removed; your seal is back.".into()); this.avatar_poll(); } Err(e) => this.av.note = Some(e) }
            cx.notify();
        });
    }

    /// The settings row: the picture (or seal), choose, remove; a picture can be dropped on it.
    pub fn avatar_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let me = self.me_account();
        let name = self.snap.as_ref().map(|s| s.me_name.clone()).unwrap_or_default();
        let has = get(&me).is_some();
        h_flex().id("av-set").gap_4().items_center().p_2().rounded(px(14.)).border_1().border_color(look::line())
            .drag_over::<ExternalPaths>(|d, _, _, _| d.border_color(look::paper()))
            .on_drop(cx.listener(|this, p: &ExternalPaths, window, cx| { if let Some(f) = p.paths().first() { this.avatar_open(f.clone(), window, cx); } }))
            .child(crate::views::avatar(&me, &name, 84.))
            .child(v_flex().gap_2()
                .child(h_flex().gap_2()
                    .child(Button::new("av-pick").label("Choose a picture…").on_click(cx.listener(|this, _, window, cx| this.avatar_dialog(window, cx))))
                    .when(has, |d| d.child(Button::new("av-none").label("Remove").on_click(cx.listener(|this, _, window, cx| this.avatar_remove(window, cx))))))
                .child(div().text_size(px(13.)).text_color(look::mute()).child(self.av.note.clone().unwrap_or_else(|| "Shown instead of your letter. Drop a picture here, or choose one. It is shrunk on this computer and sent encrypted.".into()))))
    }

    /// The cropper over everything (avatar.js `cropPicture`): drag to move, wheel or the buttons to zoom, a shape, Use picture.
    pub fn avatar_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.av.cropping.as_ref()?;
        let (w, h) = c.src.dimensions();
        let (l, t, dw, dh) = c.crop.placed(w, h);
        let (circle, sending, zoom, err) = (c.crop.circle, c.sending, c.crop.zoom, c.error.clone());
        let view = c.view.clone();
        let stage = div().id("crop-stage").relative().size(px(SIZE as f32)).flex_none().overflow_hidden().bg(rgb(0x000000)).cursor_grab()
            .child(img(ImageSource::Render(view)).absolute().left(px(l)).top(px(t)).w(px(dw)).h(px(dh)).object_fit(ObjectFit::Fill))
            .child(canvas(|_, _, _| (), move |b, _, window, _| frame(window, b, circle)).absolute().top_0().left_0().size_full())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(c) = this.av.cropping.as_mut() { c.drag = Some((ev.position, c.crop.x, c.crop.y)); cx.notify(); }
            }))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let dy = match ev.delta { ScrollDelta::Pixels(p) => f32::from(p.y), ScrollDelta::Lines(l) => l.y * 20. };
                if dy != 0. { this.avatar_crop_set(|c| c.zoom *= if dy > 0. { 1.1 } else { 1. / 1.1 }, cx); }
            }));
        let card = v_flex().id("crop-card").gap_3().p_5().rounded(px(18.)).bg(rgb(0x14131a)).border_1().border_color(look::line()).shadow_lg().items_center()
            .child(div().font_family(look::F_SHARP).text_size(px(24.)).child("Your picture"))
            .child(stage)
            .child(h_flex().gap_2().items_center()
                .child(choice("crop-minus", false, "−").on_click(cx.listener(|this, _, _, cx| this.avatar_crop_set(|c| c.zoom /= 1.25, cx))))
                .child(div().w(px(120.)).text_center().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(format!("zoom {zoom:.1}×")))
                .child(choice("crop-plus", false, "+").on_click(cx.listener(|this, _, _, cx| this.avatar_crop_set(|c| c.zoom *= 1.25, cx)))))
            .child(h_flex().gap_2()
                .child(choice("crop-circle", circle, "Circle").on_click(cx.listener(|this, _, _, cx| this.avatar_crop_set(|c| c.circle = true, cx))))
                .child(choice("crop-square", !circle, "Square").on_click(cx.listener(|this, _, _, cx| this.avatar_crop_set(|c| c.circle = false, cx)))))
            .child(div().max_w(px(280.)).text_size(px(12.)).text_color(look::mute()).child("Drag the picture to move it, scroll to zoom. It is shrunk on this computer before it is sent."))
            .when_some(err, |d, e| d.child(div().max_w(px(280.)).text_size(px(12.)).text_color(rgb(look::DANGER)).child(e)))
            .child(h_flex().gap_2()
                .child(Button::new("crop-cancel").label("Cancel").on_click(cx.listener(|this, _, _, cx| { this.av.cropping = None; cx.notify(); })))
                .child(Button::new("crop-use").primary().label(if sending { "Sending…" } else { "Use picture" }).on_click(cx.listener(|this, _, window, cx| this.avatar_use(window, cx)))));
        Some(div().id("crop-back").absolute().inset_0().flex().items_center().justify_center().bg(hsla(0., 0., 0., 0.62)).occlude()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(c) = this.av.cropping.as_mut() {
                    if let Some((p0, x0, y0)) = c.drag {
                        if ev.pressed_button.is_none() { c.drag = None; } else {
                            let (w, h) = c.src.dimensions();
                            c.crop.x = x0 + f32::from(ev.position.x - p0.x); c.crop.y = y0 + f32::from(ev.position.y - p0.y);
                            c.crop = c.crop.clamped(w, h);
                        }
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| { if let Some(c) = this.av.cropping.as_mut() { c.drag = None; cx.notify(); } }))
            .child(card).into_any_element())
    }

    /// Tests (uitest.rs): what the store holds and what the cropper shows.
    pub fn avatar_dump(&self) -> Value {
        json!({"held": held().into_iter().map(|(a, b, s)| json!({"account": a, "blob": b, "shape": s})).collect::<Vec<_>>(),
               "cropping": self.av.cropping.as_ref().map(|c| json!({"w": c.src.width(), "h": c.src.height(), "zoom": c.crop.zoom, "x": c.crop.x, "y": c.crop.y, "circle": c.crop.circle, "sending": c.sending, "error": c.error})),
               "note": self.av.note})
    }
}

/// The frame over the stage: what lies outside the chosen shape is dimmed, and its edge drawn.
fn frame(window: &mut Window, b: Bounds<Pixels>, circle: bool) {
    let (x, y, n) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width));
    let dim = hsla(0., 0., 0., 0.6);
    let edge = hsla(0., 0., 1., 0.8);
    if !circle { sq::cutters(window, b, n * SQ_R, dim); sq::stroke(window, b, n * SQ_R, 1.5, edge); return; }
    let r = n / 2.;
    let arc = |a0: f32, a1: f32| -> Vec<(f32, f32)> { (0..=24).map(|i| { let a = a0 + (a1 - a0) * i as f32 / 24.; (x + r + r * a.cos(), y + r + r * a.sin()) }).collect() };
    let pi = std::f32::consts::PI;
    // each corner: the box's corner, then the quarter circle between the two edge midpoints
    let corners = [((x, y), arc(pi, 1.5 * pi)), ((x + n, y), arc(1.5 * pi, 2. * pi)), ((x + n, y + n), arc(0., 0.5 * pi)), ((x, y + n), arc(0.5 * pi, pi))];
    for (c, a) in corners {
        let mut pb = PathBuilder::fill();
        let mut pts: Vec<Point<Pixels>> = vec![point(px(c.0), px(c.1))];
        pts.extend(a.iter().map(|&(px_, py_)| point(px(px_), px(py_))));
        pb.add_polygon(&pts, true);
        if let Ok(p) = pb.build() { window.paint_path(p, dim); }
    }
    let ring: Vec<Point<Pixels>> = arc(0., 2. * pi).iter().map(|&(px_, py_)| point(px(px_), px(py_))).collect();
    let mut pb = PathBuilder::stroke(px(1.5));
    pb.add_polygon(&ring, true);
    if let Ok(p) = pb.build() { window.paint_path(p, edge); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool { (a - b).abs() < 1e-3 }

    #[::core::prelude::v1::test]
    fn at_zoom_one_the_short_side_fills_the_stage() {
        let c = Crop::default();
        // a 400 x 200 landscape: 256/200 = 1.28 per pixel, so 512 wide; the middle 200 x 200 shows
        let (l, t, dw, dh) = c.placed(400, 200);
        assert!(close(dw, 512.) && close(dh, 256.) && close(t, 0.) && close(l, -128.));
        let (sx, sy, side) = c.source(400, 200);
        assert!(close(sx, 100.) && close(sy, 0.) && close(side, 200.));
    }
    #[::core::prelude::v1::test]
    fn zoom_shrinks_the_source_square() {
        let c = Crop { zoom: 2., ..Crop::default() };
        let (sx, sy, side) = c.source(300, 300);
        assert!(close(side, 150.) && close(sx, 75.) && close(sy, 75.));
        let c = Crop { zoom: 4., ..Crop::default() };
        assert!(close(c.source(512, 512).2, 128.));
    }
    #[::core::prelude::v1::test]
    fn the_picture_always_covers_the_stage() {
        // a drag far past the edge stops where the picture's edge meets the stage's
        let c = Crop { zoom: 1., x: 900., y: -900., circle: true }.clamped(400, 200);
        let (l, t, dw, dh) = c.placed(400, 200);
        assert!(l <= 1e-3 && l + dw >= 256. - 1e-3 && t <= 1e-3 && t + dh >= 256. - 1e-3, "{l} {t} {dw} {dh}");
        assert!(close(c.x, 128.) && close(c.y, 0.), "square-ish axis cannot move at zoom 1: {c:?}");
        let z = Crop { zoom: 9., x: 1e6, y: 1e6, circle: false }.clamped(300, 300);
        assert!(close(z.zoom, ZOOM_MAX));
        let (l, t, dw, dh) = z.placed(300, 300);
        assert!(l <= 1e-3 && t <= 1e-3 && l + dw >= 256. - 1e-3 && t + dh >= 256. - 1e-3);
        let n = Crop { zoom: f32::NAN, x: f32::NAN, y: 0., circle: true }.clamped(10, 10);
        assert!(close(n.zoom, 1.) && close(n.x, 0.));
    }
    #[::core::prelude::v1::test]
    fn dragging_right_shows_more_of_the_left() {
        let c = Crop { zoom: 2., x: 64., y: 0., circle: true }.clamped(256, 256);
        let (sx, _, side) = c.source(256, 256);
        assert!(close(side, 128.) && close(sx, 32.), "{sx} {side}");
    }
    fn gradient(w: u32, h: u32) -> RgbaImage { RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 128, 255])) }
    #[::core::prelude::v1::test]
    fn the_square_is_256_and_comes_from_the_chosen_part() {
        let src = gradient(600, 300);
        let left = render_square(&src, &Crop { x: 1000., ..Crop::default() });
        let right = render_square(&src, &Crop { x: -1000., ..Crop::default() });
        assert_eq!(left.dimensions(), (SIZE, SIZE));
        // dragging right (x > 0) shows the left of the picture: smaller red values than when dragging left
        assert!(left.get_pixel(128, 128)[0] < right.get_pixel(128, 128)[0], "{} {}", left.get_pixel(128, 128)[0], right.get_pixel(128, 128)[0]);
        assert!(left.get_pixel(0, 0)[0] < 5);
    }
    #[::core::prelude::v1::test]
    fn a_busy_photo_still_fits_in_forty_kilobytes() {
        // noise is the worst case for a JPEG
        let mut s = 12345u32;
        let noisy = RgbaImage::from_fn(300, 300, |_, _| { s = s.wrapping_mul(1664525).wrapping_add(1013904223); image::Rgba([(s >> 24) as u8, (s >> 16) as u8, (s >> 8) as u8, 255]) });
        let p = prepare(&noisy, &Crop::default()).expect("fits");
        assert!(p.bytes.len() <= MAX_BYTES && p.mime == "image/jpeg" && p.shape == "circle");
        let back = image::load_from_memory(&p.bytes).expect("a picture");
        assert!(back.width() <= SIZE && back.width() == back.height());
        let plain = prepare(&gradient(500, 500), &Crop { circle: false, ..Crop::default() }).unwrap();
        assert_eq!(plain.shape, "square");
        assert_eq!(image::load_from_memory(&plain.bytes).unwrap().width(), SIZE, "a smooth picture keeps its full size");
    }
    #[::core::prelude::v1::test]
    fn the_masks_cut_the_corners_and_keep_the_middle() {
        let mut c = RgbaImage::from_pixel(100, 100, image::Rgba([200, 100, 50, 255]));
        mask(&mut c, true);
        assert_eq!(c.get_pixel(0, 0)[3], 0);
        assert_eq!(c.get_pixel(50, 50)[3], 255);
        assert!(c.get_pixel(50, 0)[3] >= 250, "the middle of an edge touches the circle");
        let mut s = RgbaImage::from_pixel(100, 100, image::Rgba([200, 100, 50, 255]));
        mask(&mut s, false);
        assert_eq!(s.get_pixel(0, 0)[3], 0);
        assert_eq!(s.get_pixel(50, 50)[3], 255);
        // a squircle keeps more of its corner than a circle does
        assert!(s.get_pixel(10, 10)[3] > c.get_pixel(10, 10)[3]);
        assert_eq!(s.get_pixel(99, 99)[3], 0);
    }
    #[::core::prelude::v1::test]
    fn a_photo_turned_by_its_exif_tag_is_decoded_upright() {
        // a 40 x 20 JPEG tagged "rotate 90 degrees" decodes as 20 x 40
        let img = gradient(40, 20);
        let rgb = DynamicImage::ImageRgba8(img).to_rgb8();
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&rgb).unwrap();
        // splice an APP1 Exif segment with Orientation = 6 right after SOI
        let exif: Vec<u8> = [b"Exif\0\0".as_slice(), &[0x4d, 0x4d, 0, 0x2a, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]].concat();
        let mut with = vec![0xff, 0xd8, 0xff, 0xe1];
        with.extend(((exif.len() + 2) as u16).to_be_bytes());
        with.extend(&exif);
        with.extend(&jpg[2..]);
        let plain = decode_file(&jpg).unwrap();
        assert_eq!(plain.dimensions(), (40, 20));
        let turned = decode_file(&with).unwrap();
        assert_eq!(turned.dimensions(), (20, 40), "orientation 6 swaps the sides");
        assert!(decode_file(b"not a picture").is_err());
    }
    #[::core::prelude::v1::test]
    fn a_picture_made_here_reads_back_through_the_display_path() {
        let p = prepare(&gradient(400, 300), &Crop::default()).unwrap();
        let shown = Pic::build("b", p.shape, &p.bytes).unwrap();
        assert!(shown.circle && shown.blob == "b");
        let sq = Pic::build("b", "square", &p.bytes).unwrap();
        assert!(!sq.circle);
        // a picture that is not square (an old client) is cropped to its middle, not squeezed
        assert!(Pic::build("b", "circle", &{ let mut o = Vec::new(); image::codecs::png::PngEncoder::new(&mut o).write_image(gradient(80, 40).as_raw(), 80, 40, image::ExtendedColorType::Rgba8).unwrap(); o }).is_ok());
    }
    use image::ImageEncoder;
}
