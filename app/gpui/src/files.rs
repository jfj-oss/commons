//! Files, pictures and sound in the window (PORT-PLAN.md section 5; web reference: app.js file rows, `lightbox`, the composer's
//! drop and paste, `#transfers`, mega-voice.js).
//!   * SEND: drop files from the file manager on the room, paste a picture (or copied files) into the box, the paperclip, several
//!     at once. What is sent and how is xfer.rs: the page's sizes and limits (no resizing, nothing is compressed), large files in
//!     resumable pieces with pause and cancel, the Transfers card.
//!   * SHOW: a picture inline (media.rs decodes off the window's thread), a click opens the lightbox (wheel or + / - to zoom, drag to
//!     move, the arrows between pictures, Esc). Video shows its first frame with a play button and plays in the row (ffmpeg, playback.rs);
//!     audio and voice messages are a player row with the voice message's waveform. Without ffmpeg a video opens in the system's player.
//!     Download saves any file to a folder; a large file is not fetched until Download.
use crate::app::{Level, Mega};
use crate::ascii::{hole, Holes};
use crate::icons::{self, Icon};
use crate::look;
use crate::media::{self, Pic};
use crate::model::{FileInfo, Msg};
use crate::playback;
use crate::squircle as sq;
use crate::xfer::{self, Kind};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SPEEDS: [f32; 3] = [1., 1.5, 2.];
/// What a picture or video takes in the column at most (style.css `.imgslot img`, `.mediaslot video`), in logical pixels.
const IMG_MAX: (f32, f32) = (460., 320.);
const VIDEO_MAX: (f32, f32) = (520., 380.);

pub fn media_kind(mime: &str, ext: &str) -> Option<&'static str> {
    if mime.starts_with("video/") { return Some("video"); }
    if mime.starts_with("audio/") { return Some("audio"); }
    match ext.to_lowercase().as_str() {
        "mp4" | "m4v" | "mov" | "webm" | "mkv" => Some("video"),
        "mp3" | "m4a" | "aac" | "wav" | "ogg" | "oga" | "opus" | "flac" => Some("audio"),
        _ => None,
    }
}
fn ext_of(name: &str) -> String { std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_string() }
/// mega-voice.js `fmtClock`.
pub fn fmt_clock(ms: u64) -> String { let s = (ms as f64 / 1000.).round() as u64; format!("{}:{:02}", s / 60, s % 60) }
/// The largest a picture is drawn in the column, never enlarged.
pub fn fit(w: u32, h: u32, max: (f32, f32)) -> (f32, f32) {
    let k = (max.0 / w.max(1) as f32).min(max.1 / h.max(1) as f32).min(1.);
    ((w as f32 * k).round().max(1.), (h as f32 * k).round().max(1.))
}

// ---------------------------------------------------------------------------------------------- state

pub struct Light { pub gid: String, pub ids: Vec<String>, pub at: usize, pub zoom: f32, pub pan: (f32, f32), pub drag: Option<(Point<Pixels>, (f32, f32), bool)> }
pub struct SaveCard { pub gid: String, pub id: String, pub name: String, pub size: u64, pub large: bool, pub folder: Entity<InputState>, pub error: Option<String> }

#[derive(Default)]
pub struct PlayUi {
    /// The message that is loaded in the player (playing or paused), and whether it plays now.
    pub cur: Option<String>, pub playing: bool,
    /// A play that waits for the decrypted copy.
    pub want: Option<String>,
    pub pos: HashMap<String, u64>, pub err: HashMap<String, String>,
    pub speed: usize,
}

#[derive(Default)]
pub struct FilesUi {
    pub me: Option<WeakEntity<Mega>>,
    /// Tests: draw the drop overlay without a real drag.
    pub fake_drag: bool,
    /// Set while painting when a file drag is over the room (the overlay reads it).
    pub drag_seen: Rc<Cell<bool>>,
    pub gens: (u64, u64),
    pub states: HashMap<String, u32>,
    pub light: Option<Light>,
    pub save: Option<SaveCard>,
    pub play: PlayUi,
    pub parked_at: Option<Instant>,
    pub shown: Option<Arc<RenderImage>>,
    pub hits: crate::calldock::Hits,
    pub frames_seen: u64,
}

/// What a message's row needs to draw its file, copied out of the window's state.
#[derive(Clone)]
pub struct FileCtx {
    pub me: WeakEntity<Mega>, pub gid: String, pub hi: Hsla, pub k: f32, pub font: (&'static str, f32, f32),
    pub hits: crate::calldock::Hits,
    pub playing: bool, pub loading: bool, pub pos_ms: u64, pub err: Option<String>, pub speed: f32,
    pub frame: Option<Arc<RenderImage>>,
}

fn act(me: &WeakEntity<Mega>, f: impl Fn(&mut Mega, &mut Window, &mut Context<Mega>) + 'static) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let me = me.clone();
    move |_, window, cx| { let _ = me.update(cx, |this, cx| f(this, window, cx)); }
}

// ---------------------------------------------------------------------------------------------- the window's side

impl Mega {
    pub fn files_init(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files.me = Some(cx.entity().downgrade());
        media::init(self.core.clone());
        // the picture clock: playback moves at the speed of its frames, nothing else asks for them
        let t = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(40)).await;
            if this.update_in(cx, |this, window, cx| this.files_frame(window, cx)).is_err() { break; }
        });
        self._tasks.push(t);
        cx.on_app_quit(|_, _| async { playback::stop(); media::clean_up(); }).detach();
    }

    /// A message row's view of the player and of its frame.
    pub fn file_ctx(&self, m: &Msg) -> Option<FileCtx> {
        m.file.as_ref()?;
        let me = self.files.me.clone()?;
        let p = &self.files.play;
        let on = p.cur.as_deref() == Some(m.id.as_str());
        let playing = on && p.playing;
        let pos_ms = if playing { playback::status().filter(|s| s.id == m.id).map(|s| s.pos_ms).unwrap_or(0) } else { p.pos.get(&m.id).copied().unwrap_or(0) };
        Some(FileCtx {
            me, gid: self.gid.clone().unwrap_or_default(), hi: rgb(self.room_duo(self.gid.as_deref().unwrap_or("")).hi).into(), k: 1., font: self.chat_font(), hits: self.files.hits.clone(),
            playing, loading: p.want.as_deref() == Some(m.id.as_str()), pos_ms, err: p.err.get(&m.id).cloned(), speed: SPEEDS[p.speed % SPEEDS.len()],
            frame: if on { self.files.shown.clone() } else { None },
        })
    }

    /// Every 40 ms: the picture of what plays, the end of it, new pictures (their rows change height), thumbnails handed back to the atlas.
    pub fn files_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != crate::app::Screen::Main { return; }
        let mut redraw = false;
        for old in media::take_retired() { let _ = window.drop_image(old); }
        if let Some(st) = playback::status() {
            if self.files.play.cur.as_deref() == Some(st.id.as_str()) && self.files.play.playing {
                if let Some(f) = st.frame.clone() {
                    let same = self.files.shown.as_ref().is_some_and(|o| Arc::ptr_eq(o, &f));
                    if !same { if let Some(old) = self.files.shown.replace(f) { let _ = window.drop_image(old); } }
                }
                if let Some(e) = st.err.clone() { playback::stop(); self.files.play.playing = false; self.files.play.err.insert(st.id.clone(), e); }
                else if st.ended { playback::stop(); self.files.play.playing = false; self.files.play.pos.insert(st.id.clone(), 0); }
                redraw = true;
            }
        } else if self.files.play.playing { self.files.play.playing = false; redraw = true; }
        // a play that was waiting for its decrypted copy
        if let Some(id) = self.files.play.want.clone() {
            match media::media_of(&id) {
                Some(m) if m.state == "ready" => { self.files.play.want = None; self.play_ready(&id, window, cx); redraw = true; }
                Some(m) if m.state == "failed" => { self.files.play.want = None; self.files.play.err.insert(id.clone(), m.err.clone()); media::forget_media(&id); redraw = true; }
                _ => {}
            }
        }
        let g = (xfer::generation(), media::generation());
        if g != self.files.gens {
            self.files.gens = g;
            self.files_relayout(cx);
            redraw = true;
        }
        // the core's list of unfinished transfers, every five seconds
        if self.snap.is_some() && self.files.parked_at.map_or(true, |t| t.elapsed() > Duration::from_secs(5)) {
            self.files.parked_at = Some(Instant::now());
            let core = self.core.clone();
            std::thread::Builder::new().name("mega-xfers".into()).spawn(move || xfer::refresh_parked(&core)).ok();
        }
        // a drag over the window: the drop hint is painted from the drag, so frames must keep coming
        if redraw || cx.has_active_drag() { cx.notify(); }
    }

    /// A picture that arrived, a poster, a download that began or ended: the row measures itself again.
    fn files_relayout(&mut self, _cx: &mut Context<Self>) {
        let mut changed = vec![];
        for (i, m) in self.win.msgs.iter().enumerate() {
            let Some(f) = &m.file else { continue };
            let s = row_state(&m.id, f);
            match self.files.states.insert(m.id.clone(), s) { Some(old) if old != s => changed.push(i), _ => {} }
        }
        for i in changed { if let Some(m) = self.win.msgs.get(i) { self.reading.full.remove(&m.id); self.reading.scaled.remove(&m.id); } self.list.splice(i..i + 1, 1); }
    }

    // ---- sending

    pub fn files_dropped(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gid) = self.gid.clone().filter(|_| self.level == Level::Chat) else { xfer::toast("Open a room first."); cx.notify(); return };
        let _ = window;
        if paths.is_empty() { return; }
        xfer::send_paths(self.core.clone(), gid, paths);
        self.list.scroll_to_end();
        cx.notify();
    }

    /// The paperclip: the system's file dialog, several files.
    pub fn files_attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Send".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await { let _ = this.update_in(cx, |this, window, cx| this.files_dropped(paths, window, cx)); }
        }).detach();
    }

    /// Ctrl/Cmd+V in the box: a picture on the clipboard (or files copied in the file manager) is sent, text pastes as text.
    /// Returns true when it took the paste.
    pub fn files_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.level != Level::Chat { return false; }
        let Some(item) = cx.read_from_clipboard() else { return false };
        let mut pics: Vec<&Image> = vec![]; let mut paths: Vec<PathBuf> = vec![];
        for e in &item.entries { match e { ClipboardEntry::Image(i) => pics.push(i), ClipboardEntry::ExternalPaths(p) => paths.extend(p.paths().iter().cloned()), _ => {} } }
        let Some(gid) = self.gid.clone() else { return false };
        if pics.is_empty() && paths.is_empty() { return false; }
        for i in pics { xfer::send_bytes(self.core.clone(), gid.clone(), xfer::pasted_name(i.format.extension().replace("jpeg", "jpg").as_str()), i.format.mime_type().to_string(), i.bytes.clone()); }
        if !paths.is_empty() { self.files_dropped(paths, window, cx); }
        self.list.scroll_to_end();
        cx.notify();
        true
    }

    // ---- saving

    /// Download: the Save-to card (folder from Settings first), then the file goes there in pieces.
    pub fn files_download(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.win.msgs.iter().find(|m| m.id == id).cloned() else { return };
        let Some(f) = m.file.clone() else { return };
        let gid = self.gid.clone().unwrap_or_default();
        if xfer::download_of(id).is_some() { return; }
        // a partly saved file carries on where it is, without asking again
        let part = xfer::parked_download(id).and_then(|d| d["done"].as_u64()).unwrap_or(0) > 0;
        if f.large.is_some() && part { xfer::download_large(self.core.clone(), gid, id.to_string(), f.name.clone(), String::new()); cx.notify(); return; }
        let start = self.prefs_dl_folder();
        let folder = cx.new(|cx| InputState::new(window, cx).placeholder("a folder"));
        folder.update(cx, |i, cx| i.set_value(start, window, cx));
        let sub = cx.subscribe_in(&folder, window, |this: &mut Self, _, ev: &InputEvent, window, cx| { if let InputEvent::PressEnter { .. } = ev { this.files_save_go(window, cx); } });
        self._subs.push(sub);
        self.files.save = Some(SaveCard { gid, id: id.to_string(), name: f.name.clone(), size: f.size, large: f.large.is_some(), folder, error: None });
        cx.notify();
    }
    fn prefs_dl_folder(&self) -> String {
        let last = std::fs::read_to_string(self.core.dir.join("gpui-dlfolder")).unwrap_or_default();
        let last = last.trim();
        if !last.is_empty() { return last.to_string(); }
        self.snap.as_ref().map(|s| s.download_dir.clone()).unwrap_or_default()
    }
    pub fn files_save_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.files.save.as_ref() else { return };
        let folder = c.folder.read(cx).value().trim().to_string();
        if folder.is_empty() { if let Some(c) = self.files.save.as_mut() { c.error = Some("Choose a folder.".into()); } cx.notify(); return; }
        let (gid, id, name, large) = (c.gid.clone(), c.id.clone(), c.name.clone(), c.large);
        self.files.save = None;
        let _ = std::fs::write(self.core.dir.join("gpui-dlfolder"), &folder);
        if large { xfer::download_large(self.core.clone(), gid, id, name, folder); } else { xfer::save_small(self.core.clone(), gid, id, PathBuf::from(folder)); }
        let _ = window;
        cx.notify();
    }
    /// "Browse…": the core's own folder picker (kdialog / zenity / the system's), as the page asks it.
    pub fn files_browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.files.save.as_ref() else { return };
        let start = c.folder.read(cx).value().to_string();
        self.rpc("pick_folder", json!({"start": start}), window, cx, |this, r, window, cx| {
            match r {
                Ok(v) => if let (Some(p), Some(c)) = (v["path"].as_str().filter(|p| !p.is_empty()), this.files.save.as_ref()) { let p = p.to_string(); c.folder.update(cx, |i, cx| i.set_value(p, window, cx)); },
                Err(e) => if let Some(c) = this.files.save.as_mut() { c.error = Some(e); },
            }
            cx.notify();
        });
    }
    pub fn files_show_in_folder(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.win.msgs.iter().find(|m| m.id == id).and_then(|m| m.file.as_ref()).and_then(|f| f.download.as_ref()).map(|d| d.path.clone()).unwrap_or_default();
        self.rpc("file_show_in_folder", json!({"id": id}), window, cx, move |_, r, _, cx| {
            if !r.map(|v| v["opened"].as_bool() == Some(true)).unwrap_or(false) { xfer::toast(format!("The file is at {}.", if path.is_empty() { "your Downloads folder".to_string() } else { path.clone() })); }
            cx.notify();
        });
    }

    // ---- playing

    /// Play or pause the clip of message `id`.
    pub fn files_play(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.win.msgs.iter().find(|m| m.id == id).cloned() else { return };
        let Some(f) = m.file.clone() else { return };
        if self.files.play.cur.as_deref() == Some(id) && self.files.play.playing {
            // pause: remember the place
            if let Some(s) = playback::status() { self.files.play.pos.insert(id.to_string(), s.pos_ms); }
            playback::stop(); self.files.play.playing = false; cx.notify(); return;
        }
        self.files.play.err.remove(id);
        match media::media_of(id) {
            Some(md) if md.state == "ready" => self.play_ready(id, window, cx),
            Some(md) if md.state == "failed" => { media::forget_media(id); self.files.play.want = Some(id.to_string()); media::want_media(&self.gid.clone().unwrap_or_default(), id, &f.name); }
            _ => { self.files.play.want = Some(id.to_string()); media::want_media(&self.gid.clone().unwrap_or_default(), id, &f.name); }
        }
        cx.notify();
    }

    /// The decrypted copy is here: play it in the window, or hand it to the system's player when this computer cannot.
    fn play_ready(&mut self, id: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let (Some(md), Some(f)) = (media::media_of(id), self.win.msgs.iter().find(|m| m.id == id).and_then(|m| m.file.clone())) else { return };
        let Some(path) = md.path.clone() else { return };
        let video = md.has_video || media_kind(&f.mime, &ext_of(&f.name)) == Some("video") && md.poster.is_some();
        let from = { let p = self.files.play.pos.get(id).copied().unwrap_or(0); if md.dur_ms > 0 && p + 300 >= md.dur_ms { 0 } else { p } };
        if !playback::can_play() {
            if playback::null_sink() { return; }
            cx.open_with_system(&path);
            xfer::toast(if media::has_ffmpeg() { "No sound player was found, so it opened in your system's player.".to_string() } else { format!("Opened in your system's {} player: this computer has no ffmpeg for playing it here.", if video { "video" } else { "audio" }) });
            return;
        }
        let size = md.poster.as_ref().map(|p| { let (dw, _) = fit(p.w, p.h, VIDEO_MAX); playback::video_size(p.w, p.h, ((dw * 1.5) as u32).min(800).max(160)) });
        let rate = if video { 1. } else { SPEEDS[self.files.play.speed % SPEEDS.len()] as f64 };
        let has_audio = md.has_audio || !video;
        match playback::start(id, &path, from, rate, if video { size } else { None }, has_audio) {
            Ok(()) => { self.files.play.cur = Some(id.to_string()); self.files.play.playing = true; self.files.shown = None; }
            Err(e) => { self.files.play.err.insert(id.to_string(), e); }
        }
        cx.notify();
    }

    /// A click on the waveform or the bar: go to that place (playing continues from there).
    pub fn files_seek(&mut self, id: &str, frac: f32, window: &mut Window, cx: &mut Context<Self>) {
        let dur = self.clip_ms(id);
        if dur == 0 { return; }
        let to = (frac.clamp(0., 1.) as f64 * dur as f64) as u64;
        self.files.play.pos.insert(id.to_string(), to);
        if self.files.play.cur.as_deref() == Some(id) && self.files.play.playing { playback::stop(); self.files.play.playing = false; self.play_ready(id, window, cx); }
        cx.notify();
    }
    pub fn files_speed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files.play.speed = (self.files.play.speed + 1) % SPEEDS.len();
        // sound that plays now carries on at the new speed from where it is
        if let (Some(id), true) = (self.files.play.cur.clone(), self.files.play.playing) {
            if let Some(s) = playback::status() { self.files.play.pos.insert(id.clone(), s.pos_ms); }
            playback::stop(); self.files.play.playing = false; self.play_ready(&id, window, cx);
        }
        cx.notify();
    }
    /// The clip's length: what the voice message says, else what ffprobe found.
    fn clip_ms(&self, id: &str) -> u64 {
        self.win.msgs.iter().find(|m| m.id == id).and_then(|m| m.file.as_ref()).and_then(|f| f.voice.as_ref().map(|v| v.0)).filter(|d| *d > 0).or_else(|| media::media_of(id).map(|m| m.dur_ms).filter(|d| *d > 0)).unwrap_or(0)
    }

    // ---- the lightbox

    /// Pictures of the open room in order, the one clicked first shown.
    pub fn light_open(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(gid) = self.gid.clone() else { return };
        let ids: Vec<String> = self.win.msgs.iter().filter(|m| !m.deleted && m.file.as_ref().is_some_and(|f| f.mime.starts_with("image/") && f.large.is_none())).map(|m| m.id.clone()).collect();
        let Some(at) = ids.iter().position(|i| i == id) else { return };
        self.files.light = Some(Light { gid: gid.clone(), ids, at, zoom: 1., pan: (0., 0.), drag: None });
        let _ = media::full(&gid, id);
        cx.notify();
    }
    pub fn light_step(&mut self, d: isize, cx: &mut Context<Self>) {
        let Some(l) = self.files.light.as_mut() else { return };
        let n = l.ids.len() as isize; if n == 0 { return; }
        let to = (l.at as isize + d).clamp(0, n - 1) as usize;
        if to == l.at { return; }
        l.at = to; l.zoom = 1.; l.pan = (0., 0.); l.drag = None;
        let (gid, id) = (l.gid.clone(), l.ids[to].clone());
        let _ = media::full(&gid, &id);
        cx.notify();
    }
    pub fn light_zoom(&mut self, f: f32, cx: &mut Context<Self>) {
        if let Some(l) = self.files.light.as_mut() { l.zoom = (l.zoom * f).clamp(1., 8.); if l.zoom <= 1.001 { l.pan = (0., 0.); } cx.notify(); }
    }
    pub fn light_close(&mut self, cx: &mut Context<Self>) { self.files.light = None; cx.notify(); }

    /// Keys while something is open over the room: the lightbox, the Save-to card. True when the key was used.
    pub fn files_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        let k = ev.keystroke.key.as_str();
        if self.files.light.is_some() {
            match k {
                "escape" => self.light_close(cx),
                "left" | "pageup" => self.light_step(-1, cx),
                "right" | "pagedown" => self.light_step(1, cx),
                "+" | "=" | "plus" => self.light_zoom(1.25, cx),
                "-" | "minus" => self.light_zoom(0.8, cx),
                "0" => { if let Some(l) = self.files.light.as_mut() { l.zoom = 1.; l.pan = (0., 0.); } cx.notify(); }
                _ => {}
            }
            return true;
        }
        if self.files.save.is_some() && k == "escape" { self.files.save = None; cx.notify(); return true; }
        false
    }

    // ---- what the window draws on top

    /// The drop hint, the Transfers card, notices: over the room.
    pub fn files_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let _ = window;
        let mut out: Vec<AnyElement> = vec![];
        if let Some(p) = self.transfers_card(cx) { out.push(p); }
        let toasts = xfer::toasts();
        if !toasts.is_empty() {
            out.push(v_flex().absolute().bottom(px(92.)).left_0().right_0().items_center().gap(px(6.)).children(toasts.into_iter().map(|t|
                div().relative().px_4().py_2().max_w(px(520.)).font_family(look::F_NAME).text_size(px(13.)).child(sq::back(18., rgba(0x100e16f2))).child(sq::front(18., None, Some((1., look::line())), None)).child(div().relative().child(t)))).into_any_element());
        }
        out
    }

    /// The card over the room while files are dragged over it (web: `#composer.dropping`): a scrim, the room's highlight round the edge, and what it does.
    pub fn drop_overlay(&self, hi: Hsla, room: &str) -> impl IntoElement {
        let (seen, fake) = (self.files.drag_seen.clone(), self.files.fake_drag);
        let room = room.to_string();
        canvas(|_, _, _| (), move |b, _, window, cx| {
            if !(seen.replace(false) || fake) { return; }
            window.paint_quad(fill(b, hsla(0.72, 0.3, 0.04, 0.72)));
            let inset = Bounds { origin: point(b.origin.x + px(14.), b.origin.y + px(14.)), size: size(b.size.width - px(28.), b.size.height - px(28.)) };
            sq::stroke(window, inset, 26., 2.5, hi);
            let paint = |window: &mut Window, cx: &mut App, text: &str, font: &str, px_size: f32, dy: f32, color: Hsla| {
                let s: SharedString = text.to_string().into();
                let run = TextRun { len: s.len(), font: gpui_kit::font(font), color, background_color: None, underline: None, strikethrough: None };
                let line = window.text_system().shape_line(s, px(px_size), &[run], None);
                let w = f32::from(line.width());
                let _ = line.paint(point(b.origin.x + b.size.width / 2. - px(w / 2.), b.origin.y + b.size.height / 2. + px(dy)), px(px_size * 1.2), TextAlign::Left, None, window, cx);
            };
            paint(window, cx, "drop to send", look::F_SHARP, 54., -46., look::paper());
            paint(window, cx, &format!("to {room}, end-to-end encrypted"), look::F_NAME, 15., 24., look::mute());
        }).absolute().top_0().left_0().size_full()
    }

    fn transfers_card(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let live = xfer::live(); let parked = xfer::parked();
        if live.is_empty() && parked.uploads.is_empty() && parked.downloads.is_empty() { return None; }
        let me = cx.entity().downgrade();
        let mut col = v_flex().absolute().right(px(18.)).bottom(px(96.)).w(px(340.)).p_4().gap(px(10.)).font_family(look::F_NAME)
            .child(sq::back(26., rgb(0x17151e))).child(sq::front(26., None, Some((1., look::line())), None))
            .child(div().relative().font_family(look::F_SHARP).text_size(px(18.)).child("transfers"));
        let tiny = |id: String, ic: Icon| div().id(SharedString::from(id)).relative().size(px(26.)).flex().items_center().justify_center().cursor_pointer().child(sq::back(999., hsla(0., 0., 1., 0.1))).child(div().relative().child(icons::icon(ic, 13., look::paper())));
        for x in &live {
            let s = x.snap();
            let (key, paused) = (x.key.clone(), x.is_paused());
            let (pause_key, cancel_key, keep_key, sure_key) = (key.clone(), key.clone(), key.clone(), key.clone());
            col = col.child(v_flex().relative().gap(px(5.))
                .child(h_flex().gap(px(8.)).items_center()
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(12.)).child(x.name.clone()))
                    .child(tiny(format!("xp-{key}"), if paused { Icon::Play } else { Icon::Pause }).on_click({ let me = me.clone(); let core = self.core.clone(); move |_, _, cx| { let _ = me.update(cx, |_, _| xfer::set_paused(&core, &pause_key)); } }))
                    .child(tiny(format!("xc-{key}"), Icon::Close).on_click({ let me = me.clone(); move |_, _, cx| { let _ = me.update(cx, |_, cx| { xfer::cancel(&cancel_key, false); cx.notify(); }); } })))
                .child(bar(xfer_frac(&s)))
                .child(div().relative().text_size(px(11.)).text_color(look::mute()).child(xfer_note(x, &s)))
                .child(div().relative().text_size(px(11.)).text_color(look::mute()).child(xfer_state(x, &s)))
                .when(s.confirm, |d| d.child(v_flex().relative().gap(px(6.))
                    .child(div().text_size(px(11.)).child(if x.kind == Kind::Up { format!("{} of {} is already on the relay. Cancelling removes it, and you would have to start from the beginning.", xfer::fmt_size(s.bytes.max(s.safe)), x.name) } else { format!("{} of {} is already saved. Cancelling deletes that part file.", xfer::fmt_size(s.bytes), x.name) }))
                    .child(h_flex().gap(px(8.))
                        .child(small_pill(format!("xk-{key}"), "keep it").on_click({ let me = me.clone(); move |_, _, cx| { let _ = me.update(cx, |_, cx| { xfer::keep(&keep_key); cx.notify(); }); } }))
                        .child(small_pill(format!("xs-{key}"), "cancel and delete").on_click({ let me = me.clone(); move |_, _, cx| { let _ = me.update(cx, |_, cx| { xfer::cancel(&sure_key, true); cx.notify(); }); } }))))));
        }
        for (up, list) in [(true, &parked.uploads), (false, &parked.downloads)] {
            for u in list {
                let id = u["id"].as_str().unwrap_or("").to_string();
                let (name, size) = (u["name"].as_str().unwrap_or("").to_string(), u["size"].as_u64().unwrap_or(0));
                let done = if up { (u["uploaded"].as_u64().unwrap_or(0) * u["chunk_size"].as_u64().unwrap_or(0)).min(size) } else { (u["done"].as_u64().unwrap_or(0) * u["chunk_size"].as_u64().unwrap_or(0)).min(size) };
                let (rid, did) = (id.clone(), id.clone());
                col = col.child(v_flex().relative().gap(px(5.))
                    .child(h_flex().gap(px(8.)).items_center()
                        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(12.)).child(name.clone()))
                        .child(tiny(format!("xr-{id}"), Icon::Play).on_click({ let me = me.clone(); move |_, window, cx| { let rid = rid.clone(); let _ = me.update(cx, |this, cx| this.files_resume(up, &rid, window, cx)); } }))
                        .child(tiny(format!("xd-{id}"), Icon::Close).on_click({ let me = me.clone(); let core = self.core.clone(); move |_, _, cx| { let (core, did) = (core.clone(), did.clone()); let _ = me.update(cx, |_, _| { std::thread::spawn(move || xfer::drop_parked(&core, up, &did)); }); } })))
                    .child(bar(if size > 0 { done as f32 / size as f32 } else { 0. }))
                    .child(div().relative().text_size(px(11.)).text_color(look::mute()).child(format!("{} · {} of {} {} · {} on the relay", if u["paused"].as_bool() == Some(true) { "Paused" } else { "Stopped" }, xfer::fmt_size(done), xfer::fmt_size(size), if up { "safe on the relay" } else { "saved" }, xfer::fmt_left(u["expires"].as_f64().unwrap_or(0.)))))
                    .child(div().relative().text_size(px(11.)).text_color(look::mute()).child(if up { format!("Press continue, then choose the same file again: {name}, {}. Only the unsent parts go.", xfer::fmt_size(size)) } else { "Press continue to carry on from the last saved part.".to_string() })));
            }
        }
        Some(col.into_any_element())
    }

    /// Continue a parked transfer: a download carries on; an upload asks for the same file again and checks it is the same one.
    pub fn files_resume(&mut self, up: bool, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !up {
            if let Some(d) = xfer::parked_download(id) {
                xfer::download_large(self.core.clone(), d["gid"].as_str().unwrap_or("").to_string(), id.to_string(), d["name"].as_str().unwrap_or("").to_string(), String::new());
            }
            return;
        }
        let Some(u) = xfer::parked_upload(id) else { return };
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Choose the same file".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await { if let Some(p) = paths.into_iter().next() { let _ = this.update_in(cx, |this, window, cx| this.files_resume_with(u.clone(), p, window, cx)); } }
        }).detach();
    }
    pub fn files_resume_with(&mut self, u: Value, path: PathBuf, _window: &mut Window, _cx: &mut Context<Self>) {
        let core = self.core.clone();
        std::thread::spawn(move || {
            match xfer::local(&path).and_then(|l| xfer::resume_check(&u, &l).map(|_| l)) {
                Ok(l) => { xfer::upload_large(&core, u["gid"].as_str().unwrap_or(""), &l, u["name"].as_str()); }
                Err(e) => xfer::toast(e),
            }
        });
    }

    /// The lightbox over everything (app.js `lightbox`, with the zoom and arrows the page does not have).
    pub fn light_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let l = self.files.light.as_ref()?;
        let (gid, id) = (l.gid.clone(), l.ids.get(l.at)?.clone());
        let (n, at, zoom, pan) = (l.ids.len(), l.at, l.zoom, l.pan);
        let m = self.win.msgs.iter().find(|m| m.id == id);
        let (name, bytes) = m.and_then(|m| m.file.as_ref()).map(|f| (f.name.clone(), f.size)).unwrap_or_default();
        let big = media::full(&gid, &id);
        let (pic, nat) = match (&big, media::thumb_peek(&id)) {
            (Some(r), _) => (Some(r.img.clone()), (r.w, r.h)),
            (None, Some(Pic::Ready(r))) => (Some(r.img.clone()), (r.w, r.h)),
            _ => (None, (0, 0)),
        };
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let (aw, ah) = ((vw - 120.).max(100.), (vh - 150.).max(100.));
        // the whole picture fits the window at zoom 1 (a small one is shown at its own size, as the page does)
        let (bw, bh) = fit(nat.0.max(1), nat.1.max(1), (aw, ah));
        let (dw, dh) = (bw * zoom, bh * zoom);
        let (px_, py_) = ((vw - dw) / 2. + pan.0, (vh - dh) / 2. + pan.1 - 6.);
        let me = cx.entity().downgrade();
        let dl_id = id.clone();
        let pill = |id: &'static str, ic: Icon| div().id(id).relative().size(px(44.)).flex().items_center().justify_center().cursor_pointer().child(sq::back(999., hsla(0., 0., 1., 0.14))).child(div().relative().child(icons::icon(ic, 18., look::paper())));
        let scrim = div().id("lb-back").absolute().inset_0().bg(hsla(0.72, 0.3, 0.03, 0.9)).occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, _| { if let Some(l) = this.files.light.as_mut() { l.drag = Some((ev.position, l.pan, false)); } }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(l) = this.files.light.as_mut() {
                    if let Some((p0, pan0, _)) = l.drag {
                        if ev.pressed_button.is_none() { l.drag = None; } else if l.zoom > 1.001 {
                            let d = (f32::from(ev.position.x - p0.x), f32::from(ev.position.y - p0.y));
                            l.pan = (pan0.0 + d.0, pan0.1 + d.1); l.drag = Some((p0, pan0, d.0.abs() + d.1.abs() > 3.)); cx.notify();
                        }
                    }
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                // a click on the dark closes it (the page's `w.onclick = done`); the end of a drag does not
                let moved = this.files.light.as_ref().and_then(|l| l.drag).map(|d| d.2).unwrap_or(false);
                if let Some(l) = this.files.light.as_mut() { l.drag = None; }
                if !moved { this.light_close(cx); }
            }))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let dy = match ev.delta { ScrollDelta::Pixels(p) => f32::from(p.y), ScrollDelta::Lines(l) => l.y * 20. };
                if dy != 0. { this.light_zoom(if dy < 0. { 1.12 } else { 1. / 1.12 }, cx); }
            }));
        let picture = match pic {
            Some(i) => img(ImageSource::Render(i)).absolute().left(px(px_)).top(px(py_)).w(px(dw)).h(px(dh)).rounded(px(14.)).object_fit(ObjectFit::Fill).into_any_element(),
            None => div().absolute().left(px(vw / 2. - 80.)).top(px(vh / 2. - 12.)).w(px(160.)).font_family(look::F_NAME).text_size(px(13.)).text_color(look::mute()).child("loading the picture…").into_any_element(),
        };
        let caption = h_flex().absolute().left_0().right_0().bottom(px(26.)).justify_center().items_center().gap(px(14.)).font_family(look::F_NAME).text_size(px(13.))
            .child(div().max_w(px(420.)).overflow_hidden().text_ellipsis().whitespace_nowrap().text_color(look::paper()).child(name))
            .child(div().text_color(look::mute()).child(xfer::fmt_size(bytes)))
            .when(n > 1, |d| d.child(div().text_color(look::mute()).child(format!("{} / {}", at + 1, n))))
            .child(div().text_color(look::mute()).child(format!("{}%", (zoom * 100.).round())))
            .child(small_pill("lb-dl".into(), "download").on_click({ let me = me.clone(); move |_, window, cx| { let id = dl_id.clone(); let _ = me.update(cx, |this, cx| { this.light_close(cx); this.files_download(&id, window, cx); }); } }));
        Some(div().id("lightbox").absolute().inset_0().child(scrim).child(picture).child(caption)
            .child(pill("lb-x", Icon::Close).absolute().top(px(18.)).right(px(18.)).on_click(cx.listener(|this, _, _, cx| this.light_close(cx))))
            .child(pill("lb-minus", Icon::Minus).absolute().top(px(18.)).right(px(122.)).on_click(cx.listener(|this, _, _, cx| this.light_zoom(0.8, cx))))
            .child(pill("lb-plus", Icon::Plus).absolute().top(px(18.)).right(px(70.)).on_click(cx.listener(|this, _, _, cx| this.light_zoom(1.25, cx))))
            .when(at > 0, |d| d.child(pill("lb-prev", Icon::Left).absolute().left(px(18.)).top(px(vh / 2. - 22.)).on_click(cx.listener(|this, _, _, cx| this.light_step(-1, cx)))))
            .when(at + 1 < n, |d| d.child(pill("lb-next", Icon::Right).absolute().right(px(18.)).top(px(vh / 2. - 22.)).on_click(cx.listener(|this, _, _, cx| this.light_step(1, cx)))))
            .into_any_element())
    }

    /// "Save to": the folder, Browse…, Save here (app.js `askSaveFolder`).
    pub fn save_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.files.save.as_ref()?;
        let (name, size, folder, err) = (c.name.clone(), c.size, c.folder.clone(), c.error.clone());
        let card = v_flex().id("save-card").w(px(460.)).p_5().gap_3().relative().font_family(look::F_NAME)
            .child(sq::back(26., rgb(0x17151e))).child(sq::front(26., None, Some((1., look::line())), None))
            .child(div().relative().font_family(look::F_SHARP).text_size(px(24.)).child("save to"))
            .child(div().relative().text_size(px(12.)).text_color(look::mute()).child(format!("{name} · {}", xfer::fmt_size(size))))
            .child(div().relative().text_size(px(12.)).text_color(look::mute()).child("Folder"))
            .child(h_flex().relative().gap_2().child(div().flex_1().child(Input::new(&folder))).child(Button::new("save-browse").label("Browse…").on_click(cx.listener(|this, _, window, cx| this.files_browse(window, cx)))))
            .when_some(err, |d, e| d.child(div().relative().text_size(px(12.)).text_color(rgb(look::DANGER)).child(e)))
            .child(h_flex().relative().gap_2().justify_end()
                .child(Button::new("save-cancel").label("Cancel").on_click(cx.listener(|this, _, _, cx| { this.files.save = None; cx.notify(); })))
                .child(Button::new("save-go").primary().label("Save here").on_click(cx.listener(|this, _, window, cx| this.files_save_go(window, cx)))));
        Some(div().id("save-back").absolute().inset_0().flex().items_center().justify_center().bg(hsla(0., 0., 0., 0.55)).occlude().child(card).into_any_element())
    }

    /// Tests (uitest.rs): what is held and where things are.
    pub fn files_dump(&self) -> Value {
        let play = playback::status();
        json!({
            "xfers": xfer::live().iter().map(|x| { let s = x.snap(); json!({"key": x.key, "kind": if x.kind == Kind::Up { "up" } else { "down" }, "name": x.name, "bytes": s.bytes, "total": s.total, "paused": x.is_paused(), "idle": x.idle.load(std::sync::atomic::Ordering::Acquire), "cancelled": x.is_cancelled(), "state": s.state, "confirm": s.confirm, "path": s.path}) }).collect::<Vec<_>>(),
            "parked": { "uploads": xfer::parked().uploads.iter().map(|u| json!({"id": u["id"], "name": u["name"], "uploaded": u["uploaded"], "paused": u["paused"]})).collect::<Vec<_>>(), "downloads": xfer::parked().downloads.iter().map(|u| json!({"id": u["id"], "name": u["name"], "done": u["done"]})).collect::<Vec<_>>() },
            "toasts": xfer::toast_log(),
            "pics": media::held().into_iter().map(|(id, st, w, h)| json!({"id": id, "state": st, "w": w, "h": h})).collect::<Vec<_>>(),
            "light": self.files.light.as_ref().map(|l| json!({"at": l.at, "n": l.ids.len(), "zoom": l.zoom, "pan": [l.pan.0, l.pan.1], "id": l.ids.get(l.at)})),
            "save": self.files.save.as_ref().map(|c| json!({"name": c.name, "large": c.large, "error": c.error})),
            "play": { "cur": self.files.play.cur, "playing": self.files.play.playing, "want": self.files.play.want, "pos": self.files.play.pos, "err": self.files.play.err,
                      "status": play.as_ref().map(|s| json!({"id": s.id, "pos_ms": s.pos_ms, "ended": s.ended, "err": s.err, "frames": s.frames, "skipped": s.skipped})) },
            "media": self.win.msgs.iter().filter(|m| m.file.is_some()).filter_map(|m| media::media_of(&m.id).map(|d| json!({"id": m.id, "state": d.state, "err": d.err, "dur_ms": d.dur_ms, "audio": d.has_audio, "video": d.has_video, "poster": d.poster.as_ref().map(|p| [p.w, p.h])}))).collect::<Vec<_>>(),
            "ffmpeg": media::has_ffmpeg(), "can_play": playback::can_play(),
            "hits": self.files.hits.dump(),
            "msgs": self.win.msgs.iter().filter_map(|m| m.file.as_ref().map(|f| json!({"id": m.id, "name": f.name, "size": f.size, "mime": f.mime, "large": f.large.is_some(), "voice": f.voice.as_ref().map(|v| v.0), "mine": m.mine, "download": f.download.as_ref().map(|d| json!({"finished": d.finished, "path": d.path}))}))).collect::<Vec<_>>(),
        })
    }
}


impl Mega {
    /// The test port's `files_*` commands (uitest.rs): the same handlers a drop, a paste or a click reaches.
    pub fn files_cmd(&mut self, cmd: &Value, window: &mut Window, cx: &mut Context<Self>) -> Value {
        let s = |k: &str| cmd[k].as_str().unwrap_or("").to_string();
        let by_name = |this: &Self, n: &str| this.win.msgs.iter().filter(|m| m.file.as_ref().is_some_and(|f| f.name == n || n.is_empty())).last().map(|m| m.id.clone());
        let key_of = |n: &str| xfer::live().into_iter().find(|x| x.name == n || x.key == n).map(|x| x.key.clone());
        match s("cmd").as_str() {
            "files_dump" => self.files_dump(),
            "files_send" => { let p: Vec<PathBuf> = cmd["paths"].as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(PathBuf::from)).collect()).unwrap_or_default(); self.files_dropped(p, window, cx); json!({"ok": true}) }
            "files_clip_image" => {
                let bytes = std::fs::read(s("path")).unwrap_or_default();
                let fmt = if bytes.starts_with(&[0xff, 0xd8]) { ImageFormat::Jpeg } else { ImageFormat::Png };
                cx.write_to_clipboard(ClipboardItem::new_image(&Image { format: fmt, bytes, id: 1 }));
                json!({"ok": true})
            }
            "files_clip_text" => { cx.write_to_clipboard(ClipboardItem::new_string(s("text"))); json!({"ok": true}) }
            "files_paste" => json!({"took": self.files_paste(window, cx)}),
            "files_pause" => match key_of(&s("name")) { Some(k) => { xfer::set_paused(&self.core, &k); json!({"ok": true}) } None => json!({"error": "no such transfer"}) },
            "files_cancel" => match key_of(&s("name")) { Some(k) => json!({"asked": xfer::cancel(&k, cmd["sure"].as_bool() == Some(true))}), None => json!({"error": "no such transfer"}) },
            "files_keep" => { if let Some(k) = key_of(&s("name")) { xfer::keep(&k); } json!({"ok": true}) }
            "files_open" => match by_name(self, &s("name")) { Some(id) => { self.light_open(&id, cx); json!({"ok": true}) } None => json!({"error": "no such file"}) },
            "files_play" => match by_name(self, &s("name")) { Some(id) => { self.files_play(&id, window, cx); json!({"ok": true}) } None => json!({"error": "no such file"}) },
            "files_seek" => match by_name(self, &s("name")) { Some(id) => { self.files_seek(&id, cmd["frac"].as_f64().unwrap_or(0.) as f32, window, cx); json!({"ok": true}) } None => json!({"error": "no such file"}) },
            "files_download" => match by_name(self, &s("name")) { Some(id) => { self.files_download(&id, window, cx); json!({"ok": true}) } None => json!({"error": "no such file"}) },
            "files_save" => {
                if self.files.save.is_none() { return json!({"error": "no Save-to card"}); }
                if let Some(f) = cmd["folder"].as_str() { let f = f.to_string(); if let Some(c) = self.files.save.as_ref() { c.folder.update(cx, |i, cx| i.set_value(f, window, cx)); } }
                self.files_save_go(window, cx); json!({"ok": true})
            }
            "files_light" => {
                match s("do").as_str() { "close" => self.light_close(cx), "next" => self.light_step(1, cx), "prev" => self.light_step(-1, cx), "in" => self.light_zoom(1.25, cx), "out" => self.light_zoom(0.8, cx), _ => {} }
                json!({"ok": true, "light": self.files_dump()["light"]})
            }
            "files_drag" => { self.files.fake_drag = cmd["on"].as_bool() == Some(true); cx.notify(); json!({"ok": true}) }
            "files_resume" => match xfer::parked().uploads.into_iter().find(|u| u["name"] == s("name").as_str() || u["id"] == s("name").as_str()) {
                Some(u) => { self.files_resume_with(u, PathBuf::from(s("path")), window, cx); json!({"ok": true}) }
                None => json!({"error": "no parked upload"}),
            },
            "files_drop_parked" => { let core = self.core.clone(); let (up, id) = (cmd["up"].as_bool() != Some(false), s("id")); std::thread::spawn(move || xfer::drop_parked(&core, up, &id)); json!({"ok": true}) }
            other => json!({"error": format!("unknown command {other}")}),
        }
    }
}

/// A number that changes when the shape of a file row does: its picture arrived, its poster, a download began or ended.
fn row_state(id: &str, f: &FileInfo) -> u32 {
    let pic = if f.mime.starts_with("image/") && f.large.is_none() { match media::thumb_peek(id) { None => 0, Some(Pic::Loading) => 1, Some(Pic::Ready(_)) => 2, Some(Pic::Failed(_)) => 3 } } else { 0 };
    let post = media::media_of(id).map(|m| match m.state { "ready" => 1 + m.poster.is_some() as u32, "failed" => 3, _ => 0 }).unwrap_or(0);
    let dl = xfer::download_of(id).is_some() as u32 + 2 * f.download.as_ref().is_some_and(|d| d.finished) as u32;
    pic | post << 2 | dl << 4
}

// ---------------------------------------------------------------------------------------------- pieces

fn rgba(c: u32) -> Hsla { let r = (c >> 24) & 255; let g = (c >> 16) & 255; let b = (c >> 8) & 255; let mut h: Hsla = rgb((r << 16) | (g << 8) | b).into(); h.a = (c & 255) as f32 / 255.; h }

fn bar(frac: f32) -> impl IntoElement {
    let f = frac.clamp(0., 1.);
    div().relative().h(px(8.)).w_full().child(sq::back(999., hsla(0., 0., 1., 0.1)))
        .child(canvas(|_, _, _| (), move |b, _, window, _| {
            let w = f32::from(b.size.width) * f;
            if w >= 1. { sq::fill(window, Bounds { origin: b.origin, size: size(px(w.max(f32::from(b.size.height))), b.size.height) }, 999., look::paper()); }
        }).absolute().top_0().left_0().size_full())
}
fn small_pill(id: String, label: &str) -> Stateful<Div> {
    div().id(SharedString::from(id)).relative().flex().items_center().h(px(26.)).px(px(11.)).cursor_pointer().flex_none().font_family(look::F_UI).text_size(px(12.))
        .child(sq::back(999., hsla(0., 0., 1., 0.1))).child(div().relative().child(label.to_string()))
}
fn pill(id: String, label: &str, ic: Option<Icon>) -> Stateful<Div> {
    div().id(SharedString::from(id)).relative().flex().items_center().gap(px(6.)).h(px(28.)).px(px(12.)).cursor_pointer().flex_none().font_family(look::F_UI).text_size(px(12.))
        .child(sq::back(999., hsla(0., 0., 1., 0.1)))
        .when_some(ic, |d, i| d.child(div().relative().child(icons::icon(i, 14., look::paper()))))
        .child(div().relative().child(label.to_string()))
}

fn xfer_frac(s: &xfer::XState) -> f32 { if s.total == 0 { 0. } else { (s.bytes.min(s.total) as f32 / s.total as f32).min(1.) } }
/// "42% · 21.0 MB of 50.0 MB · 3.1 MB/s · about 10s left · 2 days left on the relay" (app.js `xferBody`).
pub fn xfer_note(x: &xfer::Xfer, s: &xfer::XState) -> String {
    let pct = if s.total > 0 { (s.bytes.min(s.total) * 100 / s.total).min(100) } else { 0 };
    let mut t = format!("{pct}% · {} of {}", xfer::fmt_size(s.bytes.min(s.total)), xfer::fmt_size(s.total));
    if let Some((speed, left)) = x.rate() { t += &format!(" · {}/s · about {} left", xfer::fmt_size(speed as u64), xfer::fmt_dur(left)); }
    if s.expires > 0. { t += &format!(" · {} on the relay", xfer::fmt_left(s.expires)); }
    t
}
/// app.js `xferStateText`.
pub fn xfer_state(x: &xfer::Xfer, s: &xfer::XState) -> String {
    use std::sync::atomic::Ordering::Acquire;
    if x.is_cancelled() { "Cancelling…".into() }
    else if x.is_paused() { if x.idle.load(Acquire) { "Paused. Nothing more is sent or fetched.".into() } else { "Pausing after this part…".into() } }
    else if s.state == "waiting" { format!("Waiting for connection ({}). Trying again in {}s, no need to do anything.", s.err, ((s.retry_at - xfer::unix()).ceil() as i64).max(1)) }
    else if s.state == "checking" { if x.kind == Kind::Up { "Checking the file against what was already sent…".into() } else { "Checking the saved part…".into() } }
    else if x.kind == Kind::Up { "Sending to the relay".into() } else { "Saving to your Downloads folder".into() }
}

fn file_glass(r: f32) -> impl IntoElement { div().absolute().top_0().left_0().size_full().child(sq::back(r, rgba(0x1a1820db))) }

/// The meta line: size and kind, and how long a large file stays on the relay.
fn meta_line(f: &FileInfo, img: bool) -> String {
    let ext = ext_of(&f.name);
    let kind = if ext.is_empty() { f.mime.split('/').nth(1).unwrap_or("file").to_string() } else { ext };
    let mut t = xfer::fmt_size(f.size);
    if !img { t += &format!(" · {}", kind.to_uppercase().chars().take(12).collect::<String>()); }
    if let Some(e) = f.large { t += &format!(" · {} on the relay", xfer::fmt_left(e)); }
    t
}

/// A message's file as the page draws it: a picture, a voice message, a file row with a player under it, a large file with its progress.
pub fn body(m: &Msg, f: &FileInfo, c: &FileCtx, holes: &Holes) -> AnyElement {
    let k = c.k;
    let large = f.large.is_some();
    let saved = f.download.as_ref().is_some_and(|d| d.finished);
    let img = f.mime.starts_with("image/") && !large;
    let media = if large { None } else { media_kind(&f.mime, &ext_of(&f.name)) };
    let id = m.id.clone();
    let live = xfer::download_of(&m.id);
    let mut col = v_flex().relative().items_start().gap(px(4.)).max_w_full();
    if let (Some((ms, peaks)), false) = (&f.voice, large) {
        col = col.child(voice_row(m, f, *ms, peaks, c, holes));
    } else if img {
        col = col.child(picture(m, f, c, holes));
    } else {
        // the file row
        let dl = if large {
            if live.is_some() { let me = c.me.clone(); let key = format!("d:{id}"); pill(format!("f-cx-{id}"), "cancel", Some(Icon::Close)).on_click(act(&me, move |this, _, cx| { xfer::cancel(&key, false); cx.notify(); let _ = this; })).into_any_element() }
            else if saved {
                let (me, a, b) = (c.me.clone(), id.clone(), id.clone());
                h_flex().gap(px(6.)).child(pill(format!("f-show-{id}"), "show in folder", Some(Icon::Folder)).on_click(act(&me, move |this, w, cx| this.files_show_in_folder(&a, w, cx))))
                    .child(pill(format!("f-dl-{id}"), "download again", Some(Icon::Download)).on_click(act(&me, move |this, w, cx| this.files_download(&b, w, cx)))).into_any_element()
            } else { let (me, a) = (c.me.clone(), id.clone()); pill(format!("f-dl-{id}"), "download", Some(Icon::Download)).on_click(act(&me, move |this, w, cx| this.files_download(&a, w, cx))).into_any_element() }
        } else { let (me, a) = (c.me.clone(), id.clone()); pill(format!("f-dl-{id}"), "download", Some(Icon::Download)).on_click(act(&me, move |this, w, cx| this.files_download(&a, w, cx))).into_any_element() };
        col = col.child(h_flex().relative().items_center().gap(px(12.)).px(px(14.)).py(px(12.)).max_w(px(460.))
            .child(file_glass(22.)).child(hole(holes))
            .child(div().relative().flex_none().child(icons::icon(Icon::File, 20., c.hi)))
            .child(v_flex().relative().flex_1().min_w_0().gap(px(2.))
                .child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().font_family(look::F_NAME).text_size(px(13. * k)).text_color(look::paper()).child(f.name.clone()))
                .child(div().font_family(look::F_NAME).text_size(px(11. * k)).text_color(look::mute()).child(meta_line(f, false))))
            .child(div().relative().flex_none().child(dl))
            .child(div().absolute().top_0().left_0().size_full().child(sq::front(22., None, Some((1., look::line())), None))));
        if let Some(x) = live {
            let s = x.snap();
            col = col.child(v_flex().relative().w(px(400.)).gap(px(4.)).child(hole(holes)).child(bar(xfer_frac(&s)))
                .child(div().font_family(look::F_NAME).text_size(px(11.)).text_color(look::mute()).child(xfer_note(&x, &s))));
        }
        match media {
            Some("video") => col = col.child(video_slot(m, f, c, holes)),
            Some("audio") => col = col.child(audio_row(m, f, c, holes)),
            _ => {}
        }
    }
    if !m.text.is_empty() { col = col.child(div().relative().max_w_full().font_family(c.font.0).text_size(px(c.font.1 * k)).line_height(px((c.font.2 * k).round())).child(m.text.clone()).child(hole(holes))); }
    if saved { col = col.child(div().relative().font_family(look::F_NAME).text_size(px(11. * k)).text_color(look::mute()).child(format!("✓ Saved to {}", f.download.as_ref().map(|d| if d.save_name.is_empty() { f.name.clone() } else { d.save_name.clone() }).unwrap_or_default())).child(hole(holes))); }
    col.into_any_element()
}

/// A picture in the column, its name and size under it; a click opens the lightbox.
fn picture(m: &Msg, f: &FileInfo, c: &FileCtx, holes: &Holes) -> AnyElement {
    let id = m.id.clone();
    let k = c.k;
    let slot: AnyElement = match media::thumb(&c.gid, &m.id) {
        Pic::Ready(r) => {
            let (w, h) = fit(r.w, r.h, IMG_MAX);
            let (me, a) = (c.me.clone(), id.clone());
            div().id(SharedString::from(format!("f-img-{id}"))).relative().w(px(w * k)).h(px(h * k)).flex_none().rounded(px(18.)).overflow_hidden().cursor(CursorStyle::PointingHand)
                .border_1().border_color(look::line())
                .child(img(ImageSource::Render(r.img.clone())).size_full().rounded(px(18.)).object_fit(ObjectFit::Fill))
                .child(c.hits.track(format!("f-img:{id}")))
                .on_click(act(&me, move |this, _, cx| this.light_open(&a, cx))).into_any_element()
        }
        Pic::Loading => div().relative().w(px(240. * k)).h(px(150. * k)).flex().items_center().justify_center().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute())
            .child(sq::back(18., rgba(0x0d0c12ff))).child(sq::front(18., None, Some((1., look::line())), None)).child(div().relative().child("loading…")).child(hole(holes)).into_any_element(),
        Pic::Failed(e) => {
            let (me, a) = (c.me.clone(), id.clone());
            div().id(SharedString::from(format!("f-imgx-{id}"))).relative().px(px(14.)).py(px(10.)).max_w(px(420.)).cursor_pointer().font_family(look::F_NAME).text_size(px(12.)).text_color(rgb(look::DANGER))
                .child(sq::back(18., rgba(0x0d0c12ff))).child(sq::front(18., None, Some((1., look::line())), None)).child(div().relative().child(format!("⚠ {e}"))).child(hole(holes))
                .on_click(act(&me, move |_, _, cx| { media::retry(&a); cx.notify(); })).into_any_element()
        }
    };
    let (me, a) = (c.me.clone(), id.clone());
    v_flex().relative().gap(px(5.)).items_start()
        .child(slot)
        .child(h_flex().relative().gap(px(8.)).items_center().child(hole(holes))
            .child(div().max_w(px(300.)).overflow_hidden().text_ellipsis().whitespace_nowrap().font_family(look::F_NAME).text_size(px(12. * k)).text_color(look::paper()).child(f.name.clone()))
            .child(div().font_family(look::F_NAME).text_size(px(11. * k)).text_color(look::mute()).child(meta_line(f, true)))
            .child(small_pill(format!("f-dl-{id}"), "download").on_click(act(&me, move |this, w, cx| this.files_download(&a, w, cx)))))
        .into_any_element()
}

/// A click on a bar or waveform: the share of its width.
fn click_frac(spot: &Rc<Cell<Bounds<Pixels>>>, ev: &ClickEvent) -> Option<f32> {
    let p = ev.mouse_position()?; let b = spot.get();
    let w = f32::from(b.size.width); if w <= 0. { return None; }
    Some((f32::from(p.x - b.origin.x) / w).clamp(0., 1.))
}

fn play_button(id: &str, c: &FileCtx, size: f32, filled: bool) -> Stateful<Div> {
    let (me, a) = (c.me.clone(), id.to_string());
    let ink = if filled { rgb(look::VOID).into() } else { look::paper() };
    div().id(SharedString::from(format!("f-play-{id}"))).relative().flex_none().size(px(size)).flex().items_center().justify_center().cursor_pointer()
        .child(sq::back(999., if filled { look::paper() } else { hsla(0., 0., 1., 0.14) }))
        .child(div().relative().child(icons::icon(if c.playing { Icon::Pause } else { Icon::Play }, size * 0.5, ink)))
        .child(c.hits.track(format!("f-play:{id}")))
        .on_click(act(&me, move |this, w, cx| this.files_play(&a, w, cx)))
        .when(c.loading, |d| d.opacity(0.6))
}

/// The voice message's own row: play, the waveform that fills as it plays, the time, speed, download (mega-voice.js `row`).
fn voice_row(m: &Msg, _f: &FileInfo, ms: u64, peaks: &[u8], c: &FileCtx, holes: &Holes) -> AnyElement {
    let id = m.id.clone();
    let frac = if ms > 0 { (c.pos_ms as f32 / ms as f32).min(1.) } else { 0. };
    let spot: Rc<Cell<Bounds<Pixels>>> = Default::default();
    let bars: Vec<f32> = (0..64).map(|i| peaks.get(i).map(|p| 0.14 + 0.86 * *p as f32 / 255.).unwrap_or(0.18)).collect();
    let (on, off) = (look::paper(), hsla(0., 0., 1., 0.28));
    let wave = { let s = spot.clone(); canvas(move |b, _, _| s.set(b), move |b, _, window, _| {
        let n = bars.len() as f32; let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        let gap = 1.5; let bw = ((w - gap * (n - 1.)) / n).max(1.);
        for (i, v) in bars.iter().enumerate() {
            let bh = (v * h).max(3.); let x = i as f32 * (bw + gap);
            let col = if (i as f32 + 0.5) / n <= frac { on } else { off };
            window.paint_quad(fill(Bounds { origin: point(b.origin.x + px(x), b.origin.y + px((h - bh) / 2.)), size: size(px(bw), px(bh)) }, col).corner_radii(px(2.)));
        }
    }).h(px(34.)).flex_1().min_w(px(80.)) };
    let (me, a, b2, d) = (c.me.clone(), id.clone(), id.clone(), id.clone());
    let sp = spot.clone();
    h_flex().relative().items_center().gap(px(10.)).pl(px(8.)).pr(px(12.)).py(px(8.)).w(px(420.)).max_w_full()
        .child(file_glass(26.)).child(hole(holes))
        .child(play_button(&id, c, 36., true))
        .child(div().id(SharedString::from(format!("f-wave-{id}"))).relative().flex_1().min_w(px(80.)).h(px(34.)).cursor_pointer().child(wave)
            .child(c.hits.track(format!("f-wave:{id}")))
            .on_click({ let me = me.clone(); move |ev, w, cx| { if let Some(fr) = click_frac(&sp, ev) { let a = a.clone(); let _ = me.update(cx, |this, cx| this.files_seek(&a, fr, w, cx)); } } }))
        .child(div().relative().flex_none().font_family(look::F_UI).text_size(px(12.)).text_color(look::mute()).child(format!("{} / {}", fmt_clock(c.pos_ms), fmt_clock(ms))))
        .child(div().id(SharedString::from(format!("f-speed-{id}"))).relative().flex_none().h(px(26.)).min_w(px(38.)).px(px(8.)).flex().items_center().justify_center().cursor_pointer().font_family(look::F_UI).text_size(px(12.))
            .child(sq::front(999., None, Some((1., look::line())), None)).child(div().relative().child(format!("{}×", c.speed)))
            .on_click(act(&me, move |this, w, cx| this.files_speed(w, cx))))
        .child(div().id(SharedString::from(format!("f-dl-{id}"))).relative().flex_none().size(px(28.)).flex().items_center().justify_center().cursor_pointer().child(icons::icon(Icon::Download, 15., look::mute()))
            .on_click(act(&me, move |this, w, cx| this.files_download(&b2, w, cx))))
        .when_some(c.err.clone(), |d2, e| d2.child(div().absolute().left(px(12.)).bottom(px(-16.)).font_family(look::F_NAME).text_size(px(11.)).text_color(rgb(look::DANGER)).child(e)))
        .child(div().absolute().top_0().left_0().size_full().child(sq::front(26., None, Some((1., look::line())), None)))
        .child(div().hidden().child(d))
        .into_any_element()
}

/// An audio file: a row under the file's own, play, a bar, the time (the page's `<audio controls>` after Play).
fn audio_row(m: &Msg, f: &FileInfo, c: &FileCtx, holes: &Holes) -> AnyElement {
    let id = m.id.clone();
    let _ = f;
    let dur = media::media_of(&id).map(|d| d.dur_ms).unwrap_or(0);
    let frac = if dur > 0 { (c.pos_ms as f32 / dur as f32).min(1.) } else { 0. };
    let spot: Rc<Cell<Bounds<Pixels>>> = Default::default();
    let (me, a) = (c.me.clone(), id.clone());
    let sp = spot.clone();
    let track = { let s = spot.clone(); canvas(move |b, _, _| s.set(b), move |b, _, window, _| {
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        let y = b.origin.y + px(h / 2. - 3.);
        sq::fill(window, Bounds { origin: point(b.origin.x, y), size: size(px(w), px(6.)) }, 999., hsla(0., 0., 1., 0.14));
        if frac > 0. { sq::fill(window, Bounds { origin: point(b.origin.x, y), size: size(px((w * frac).max(6.)), px(6.)) }, 999., look::paper()); }
    }).absolute().top_0().left_0().size_full() };
    h_flex().relative().items_center().gap(px(10.)).pl(px(8.)).pr(px(14.)).py(px(8.)).w(px(420.)).max_w_full()
        .child(file_glass(26.)).child(hole(holes))
        .child(play_button(&id, c, 34., true))
        .child(div().id(SharedString::from(format!("f-bar-{id}"))).relative().flex_1().min_w(px(80.)).h(px(24.)).cursor_pointer().child(track).child(c.hits.track(format!("f-bar:{id}")))
            .on_click({ let me = me.clone(); move |ev, w, cx| { if let Some(fr) = click_frac(&sp, ev) { let a = a.clone(); let _ = me.update(cx, |this, cx| this.files_seek(&a, fr, w, cx)); } } }))
        .child(div().relative().flex_none().font_family(look::F_UI).text_size(px(12.)).text_color(look::mute()).child(format!("{} / {}", fmt_clock(c.pos_ms), if dur > 0 { fmt_clock(dur) } else { "–:––".into() })))
        .when(c.loading, |d| d.child(div().relative().flex_none().font_family(look::F_NAME).text_size(px(11.)).text_color(look::mute()).child("decrypting…")))
        .when_some(c.err.clone(), |d, e| d.child(div().relative().flex_none().max_w(px(160.)).font_family(look::F_NAME).text_size(px(11.)).text_color(rgb(look::DANGER)).child(e)))
        .child(div().absolute().top_0().left_0().size_full().child(sq::front(26., None, Some((1., look::line())), None)))
        .into_any_element()
}

/// A video: its first frame with a play button, playing in the row; a plain card where there is no ffmpeg to read it.
fn video_slot(m: &Msg, f: &FileInfo, c: &FileCtx, holes: &Holes) -> AnyElement {
    let id = m.id.clone();
    let k = c.k;
    if media::has_ffmpeg() { media::want_media(&c.gid, &id, &f.name); }
    let md = media::media_of(&id);
    let poster = md.as_ref().and_then(|d| d.poster.clone());
    let dur = md.as_ref().map(|d| d.dur_ms).unwrap_or(0);
    let (me, a) = (c.me.clone(), id.clone());
    let sized = poster.as_ref().map(|p| fit(p.w, p.h, VIDEO_MAX)).unwrap_or((360., 202.));
    let shown = if c.playing || c.pos_ms > 0 { c.frame.clone() } else { None }.or_else(|| poster.as_ref().map(|p| p.img.clone()));
    let stage = div().id(SharedString::from(format!("f-vid-{id}"))).relative().w(px(sized.0 * k)).h(px(sized.1 * k)).flex_none().rounded(px(18.)).overflow_hidden().cursor_pointer().bg(rgb(0x0d0c12))
        .border_1().border_color(look::line())
        .when_some(shown, |d, i| d.child(img(ImageSource::Render(i)).size_full().object_fit(ObjectFit::Fill)))
        .when(poster.is_none(), |d| d.flex().flex_col().items_center().justify_center().gap(px(6.)).font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute())
            .child(div().child(if md.as_ref().is_some_and(|d| d.state == "failed") { "can't read this video".to_string() } else if media::has_ffmpeg() && md.is_none() { "loading…".to_string() } else { "video".to_string() }))
            .child(div().max_w(px(300.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(f.name.clone())))
        .when(!c.playing, |d| d.child(div().absolute().inset_0().flex().items_center().justify_center().child(
            div().relative().size(px(56.)).flex().items_center().justify_center().child(sq::back(999., hsla(0., 0., 0.94, 0.92))).child(div().relative().child(icons::icon(if c.loading { Icon::Mini } else { Icon::Play }, 26., rgb(look::VOID).into()))))))
        .child(c.hits.track(format!("f-vid:{id}")))
        .on_click(act(&me, move |this, w, cx| this.files_play(&a, w, cx)));
    let spot: Rc<Cell<Bounds<Pixels>>> = Default::default();
    let (sp, me2, a2) = (spot.clone(), c.me.clone(), id.clone());
    let frac = if dur > 0 { (c.pos_ms as f32 / dur as f32).min(1.) } else { 0. };
    let track = { let s = spot.clone(); canvas(move |b, _, _| s.set(b), move |b, _, window, _| {
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        let y = b.origin.y + px(h / 2. - 3.);
        sq::fill(window, Bounds { origin: point(b.origin.x, y), size: size(px(w), px(6.)) }, 999., hsla(0., 0., 1., 0.14));
        if frac > 0. { sq::fill(window, Bounds { origin: point(b.origin.x, y), size: size(px((w * frac).max(6.)), px(6.)) }, 999., look::paper()); }
    }).absolute().top_0().left_0().size_full() };
    v_flex().relative().gap(px(6.)).items_start().mt(px(2.)).child(hole(holes)).child(stage)
        .when(poster.is_some(), |d| d.child(h_flex().relative().items_center().gap(px(10.)).w(px(sized.0 * k)).child(play_button(&id, c, 30., false))
            .child(div().id(SharedString::from(format!("f-bar-{id}"))).relative().flex_1().h(px(24.)).cursor_pointer().child(track).child(c.hits.track(format!("f-bar:{id}")))
                .on_click({ let me = me2.clone(); move |ev, w, cx| { if let Some(fr) = click_frac(&sp, ev) { let a = a2.clone(); let _ = me.update(cx, |this, cx| this.files_seek(&a, fr, w, cx)); } } }))
            .child(div().relative().flex_none().font_family(look::F_UI).text_size(px(12.)).text_color(look::mute()).child(format!("{} / {}", fmt_clock(c.pos_ms), if dur > 0 { fmt_clock(dur) } else { "–:––".into() })))))
        .when_some(c.err.clone(), |d, e| d.child(div().relative().font_family(look::F_NAME).text_size(px(11.)).text_color(rgb(look::DANGER)).child(e)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn the_kind_comes_from_the_type_then_the_ending() {
        assert_eq!(media_kind("video/mp4", ""), Some("video")); assert_eq!(media_kind("audio/webm", ""), Some("audio"));
        assert_eq!(media_kind("application/octet-stream", "MKV"), Some("video")); assert_eq!(media_kind("application/octet-stream", "flac"), Some("audio"));
        assert_eq!(media_kind("application/pdf", "pdf"), None);
    }
    #[::core::prelude::v1::test]
    fn a_picture_fits_the_column_and_is_never_enlarged() {
        assert_eq!(fit(2000, 1000, IMG_MAX), (460., 230.)); assert_eq!(fit(300, 1200, IMG_MAX), (80., 320.)); assert_eq!(fit(100, 50, IMG_MAX), (100., 50.));
        assert_eq!(fit(1920, 1080, VIDEO_MAX), (520., 293.));
    }
    #[::core::prelude::v1::test]
    fn the_clock_reads_as_the_voice_row_writes_it() { assert_eq!(fmt_clock(0), "0:00"); assert_eq!(fmt_clock(7400), "0:07"); assert_eq!(fmt_clock(75_000), "1:15"); }
    #[::core::prelude::v1::test]
    fn a_picture_arriving_changes_its_rows_state() {
        let f = FileInfo { name: "a.png".into(), mime: "image/png".into(), size: 10, ..Default::default() };
        let a = row_state("never-asked-for", &f);
        assert_eq!(a, 0, "a picture nobody asked for yet");
        let big = FileInfo { large: Some(1.), ..f.clone() };
        assert_eq!(row_state("never-asked-for", &big) & 3, 0, "a large picture is never fetched");
    }
}
