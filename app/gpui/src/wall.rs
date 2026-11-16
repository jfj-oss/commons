//! The shared ASCII wall (app/ui/mega-wall.js): anyone in a room can draw on the room's card (and, in a room, its backdrop), live, for
//! everyone. The wall itself lives in the encrypted core (crates/core/src/wall.rs: merging, the owner's lock and clear, validation); this
//! file is the picture and the pen, and the pacing, exactly as the page has them:
//!  * a fixed grid of 120 x 54 cells in every room on every device; on a card about that shape it is stretched over the card, else centred;
//!  * nothing is sent while the pen is idle; a stroke is batched and goes out at most twice a second, 200 cells at most per event;
//!  * nothing is sent into a room where nobody is around (cells stay here and reach others in a snapshot when they ask);
//!  * only the focused card is polled; the cards next to it are read once when they appear;
//!  * a question for the wall (`wall.q`) is answered with a snapshot after a short random wait unless somebody else gets there first.
use crate::app::{Level, Mega};
use crate::look::{self, Duo};
use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const COLS: usize = 120;
pub const ROWS: usize = 54;
const CW: f32 = 10.;
const CH: f32 = 17.;
/// The glyph of each brush index; index 0 is "erase".
pub const GLYPHS: &str = " #*+.o=~/|-x";
/// The ten fixed colours after the room's own two (colours 2 to 11), chosen to read on the dark backdrop (mega-wall.js `PAL`).
pub const PAL: [(&str, u32); 10] = [("white", 0xffffff), ("light grey", 0xc4c8d4), ("red", 0xff5a5f), ("orange", 0xff9f43), ("yellow", 0xffe14d), ("green", 0x5fe08a), ("teal", 0x2fd8c8), ("blue", 0x5aa8ff), ("purple", 0xb58cff), ("pink", 0xff7ac8)];
const GAP: Duration = Duration::from_millis(500);
const BATCH: usize = 200;
const PRESENT: Duration = Duration::from_secs(180);
const ASK_EVERY: Duration = Duration::from_secs(600);
const CWP: &str = "cw:";

// ---------------------------------------------------------------- geometry (pure)
/// Where the grid goes inside a box: cell size, offset and font size, in the box's own pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid { pub s: f32, pub cw: f32, pub ch: f32, pub ox: f32, pub oy: f32, pub fs: f32 }

/// A card about the grid's shape stretches it over the whole card; any other box centres it (mega-wall.js `gridIn`).
pub fn grid_in(w: f32, h: f32, card: bool) -> Grid {
    let s = (w / (COLS as f32 * CW)).min(h / (ROWS as f32 * CH));
    let ratio = (w / h) / ((COLS as f32 * CW) / (ROWS as f32 * CH));
    if card && ratio > 0.6 && ratio < 1.8 {
        let (cw, ch) = (w / COLS as f32, h / ROWS as f32);
        return Grid { s, cw, ch, ox: 0., oy: 0., fs: (ch * 0.86).min(cw * 1.5).max(6.) };
    }
    Grid { s, cw: CW * s, ch: CH * s, ox: (w - COLS as f32 * CW * s) / 2., oy: (h - ROWS as f32 * CH * s) / 2., fs: (14. * s).max(6.) }
}
impl Grid {
    /// Every length multiplied by `k` (a card shown at scale `k`).
    pub fn scaled(&self, k: f32) -> Grid { Grid { s: self.s * k, cw: self.cw * k, ch: self.ch * k, ox: self.ox * k, oy: self.oy * k, fs: self.fs * k } }
}
/// The cell under a point given relative to the box (may be outside the grid: the caller checks).
pub fn cell_at(g: &Grid, x: f32, y: f32) -> (i32, i32) { (((x - g.ox) / g.cw).floor() as i32, ((y - g.oy) / g.ch).floor() as i32) }
/// The cells a pen covers moving from one cell to another, ends included (mega-wall.js `stroke`: `Math.round` along the longer side).
pub fn line(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let n = (b.0 - a.0).abs().max((b.1 - a.1).abs()).max(1);
    (0..=n).map(|i| { let f = i as f32 / n as f32; ((a.0 as f32 + (b.0 - a.0) as f32 * f).round() as i32, (a.1 as f32 + (b.1 - a.1) as f32 * f).round() as i32) }).collect()
}
/// The colour a stroke index means: 0 and 1 are the room's own two inks, 2 to 11 the fixed palette.
pub fn ink_of(ink: [u32; 2], c: u8) -> u32 { if c < 2 { ink[c as usize] } else { PAL.get(c as usize - 2).map(|p| p.1).unwrap_or(PAL[0].1) } }
/// The rectangle of a wall cell (left, top, width, height) in the box's pixels.
pub fn cell_rect(g: &Grid, p: u16) -> (f32, f32, f32, f32) { let (x, y) = ((p as usize % COLS) as f32, (p as usize / COLS) as f32); (g.ox + x * g.cw, g.oy + y * g.ch, g.cw, g.ch) }
/// Which cells of a field grid (`cols` x `rows` cells of `fcw` x `fch` pixels) lie under the wall's marks, so the field's own ASCII flows around the
/// drawing (mega-wall.js `cardMask`).
pub fn field_mask(g: &Grid, marks: impl Iterator<Item = u16>, fcw: f32, fch: f32, cols: usize, rows: usize) -> Vec<bool> {
    let mut m = vec![false; cols * rows];
    if cols == 0 || rows == 0 || fcw <= 0. || fch <= 0. { return m; }
    for p in marks {
        let (l, t, w, h) = cell_rect(g, p);
        let x0 = ((l / fcw).floor().max(0.) as usize).min(cols - 1);
        let x1 = (((l + w) / fcw).floor().max(0.) as usize).min(cols - 1);
        let y0 = ((t / fch).floor().max(0.) as usize).min(rows - 1);
        let y1 = (((t + h) / fch).floor().max(0.) as usize).min(rows - 1);
        for y in y0..=y1 { for x in x0..=x1 { m[y * cols + x] = true; } }
    }
    m
}
/// The brush a person has chosen: glyph (0 = erase) and colour index; an erase has no colour.
pub fn stroke_value(brush: u8, col: u8) -> (u8, u8) { (brush, if brush == 0 { 0 } else { col }) }
/// A wall key's community when it is a community card's wall.
pub fn cw_key(cid: &str) -> String { format!("{CWP}{cid}") }

// ---------------------------------------------------------------- the state
#[derive(Default)]
pub struct Entry { pub cells: HashMap<u16, (u8, u8)>, pub rev: u64, pub at: Option<Instant>, pub fetching: bool, pub locked: bool }

/// Where the focused card is in the window, for the pen: its top left and its size at full scale, and the scale it is shown at.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spot { pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub s: f32 }

#[derive(Default)]
pub struct WallUi {
    pub entries: HashMap<String, Entry>,
    /// The wall under the focus (a card's, or the open room's) and the card's place on screen.
    pub key: Option<String>,
    pub spot: Spot,
    pub room_mode: bool,
    /// Where the open room's area was last painted (x, y, w, h): its wall's grid is centred in it.
    pub room_rect: Rc<std::cell::Cell<(f32, f32, f32, f32)>>,
    /// Wall keys of the cards on screen, for reading each once.
    pub visible: Vec<String>,
    pub on: bool,
    pub brush: u8,
    pub col: u8,
    pub pend: HashMap<u16, (u8, u8)>,
    pub out: Vec<u16>,
    pub from: Option<(i32, i32)>,
    pub pen: bool,
    sending: bool,
    send_queued: bool,
    last_send: Option<Instant>,
    tries: u32,
    polled: Option<Instant>,
    busy: bool,
    peer_at: u64,
    pub snap_at: u64,
    heard: Option<Instant>,
    pub carrier: Option<String>,
    local_only: Option<String>,
    asked: HashMap<String, Instant>,
    resp: HashMap<String, Instant>,
    share_back: Option<(String, Instant)>,
    pub sent: u32,
    pub gen: u64,
    /// The clear button has been pressed once ("clear for everyone?") until this time.
    pub sure: Option<Instant>,
}

impl WallUi {
    pub fn new() -> WallUi { WallUi { brush: 1, col: 0, ..Default::default() } }
    pub fn entry(&mut self, key: &str) -> &mut Entry { self.entries.entry(key.to_string()).or_default() }
    /// What to show: the core's wall with this device's not-yet-confirmed cells on top.
    pub fn marks_of(&self, key: &str) -> Vec<(u16, u8, u8)> {
        let mut m: HashMap<u16, (u8, u8)> = self.entries.get(key).map(|e| e.cells.clone()).unwrap_or_default();
        if self.key.as_deref() == Some(key) { for (p, v) in &self.pend { if v.0 == 0 { m.remove(p); } else { m.insert(*p, *v); } } }
        let mut v: Vec<(u16, u8, u8)> = m.into_iter().map(|(p, (g, c))| (p, g, c)).collect();
        v.sort_unstable();
        v
    }
    /// Put one cell under the pen (mega-wall.js `put`). Returns whether it changed anything.
    pub fn put(&mut self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= COLS as i32 || y >= ROWS as i32 { return false; }
        let Some(key) = self.key.clone() else { return false };
        let p = (y as usize * COLS + x as usize) as u16;
        let v = stroke_value(self.brush, self.col);
        let cur = self.pend.get(&p).copied().or_else(|| self.entries.get(&key).and_then(|e| e.cells.get(&p).copied()));
        if cur == Some(v) || (cur.is_none() && v.0 == 0) { return false; }
        self.pend.insert(p, v);
        if !self.out.contains(&p) { self.out.push(p); }
        true
    }
    /// A stroke from the last cell to this one.
    pub fn stroke(&mut self, to: (i32, i32)) -> bool {
        let from = self.from.unwrap_or(to);
        let mut any = false;
        for (x, y) in line(from, to) { any |= self.put(x, y); }
        self.from = Some(to);
        any
    }
    /// The next batch: cells of one kind only (the room's own two colours, or a palette colour), at most `BATCH` of them (mega-wall.js `flush`).
    pub fn batch(&self) -> Vec<u16> {
        let pal = |p: &u16| self.pend.get(p).map(|v| v.1 > 1).unwrap_or(false);
        let Some(first) = self.out.first() else { return vec![] };
        let k = pal(first);
        self.out.iter().filter(|p| pal(p) == k).take(BATCH).copied().collect()
    }
}

fn is_cw(k: &str) -> bool { k.starts_with(CWP) }
fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }

impl Mega {
    fn wall_cid(&self, key: &str) -> Option<String> {
        if let Some(c) = key.strip_prefix(CWP) { return Some(c.to_string()); }
        self.snap.as_ref()?.communities.iter().find(|c| c.channels.iter().any(|x| x.gid == key)).map(|c| c.id.clone())
    }
    pub fn wall_is_owner(&self, key: &str) -> bool { self.wall_cid(key).and_then(|c| self.snap.as_ref()?.community(&c).map(|c| c.role == "owner")).unwrap_or(false) }
    pub fn wall_locked(&self, key: &str) -> bool {
        let Some(c) = self.wall_cid(key).and_then(|c| self.snap.as_ref()?.community(&c).cloned()) else { return false };
        let id = if is_cw(key) { c.id.clone() } else { key.to_string() };
        c.locked.contains(&id)
    }
    /// Whether this person may draw on the wall right now (mega-wall.js `drawable`): not locked, unless they own it.
    pub fn wall_drawable(&self, key: &str) -> bool { !self.wall_locked(key) || self.wall_is_owner(key) }
    /// The room that carries a wall's live events, and the kind prefix.
    fn wall_prefix(key: &str) -> &'static str { if is_cw(key) { "cwall." } else { "wall." } }
    fn wall_send_gid(key: &str, carrier: Option<&String>) -> Option<String> { if is_cw(key) { carrier.cloned() } else { Some(key.to_string()) } }
    pub fn wall_present(&self) -> bool {
        let w = &self.wall;
        let now = now_ms();
        if now.saturating_sub(w.peer_at) < PRESENT.as_millis() as u64 { return true; }
        if w.heard.is_some_and(|t| t.elapsed() < PRESENT) { return true; }
        let Some(key) = &w.key else { return false };
        if is_cw(key) { return false; }
        // somebody else wrote in this room lately (only known for the open room)
        if self.gid.as_deref() == Some(key.as_str()) {
            let wall = now / 1000;
            return self.win.msgs.iter().rev().take(30).any(|m| !m.mine && m.kind == "text" && wall.saturating_sub(m.ts) < 600);
        }
        false
    }

    // ---- which wall is under the focus
    /// Called as each frame is laid out: the focused card (or the open room) and where it is.
    pub fn wall_focus(&mut self, key: Option<String>, spot: Spot, visible: Vec<String>) {
        self.wall.spot = spot;
        self.wall.visible = visible;
        if self.wall.key == key { return; }
        if self.wall.on { self.wall_off(); }
        let w = &mut self.wall;
        w.key = key; w.pend.clear(); w.out.clear(); w.from = None; w.tries = 0; w.carrier = None; w.local_only = None; w.heard = None; w.peer_at = 0; w.snap_at = 0; w.polled = None;
    }

    // ---- reading
    /// The quarter-second clock: read the focused wall every second, the cards next to it once, and send what the pen left.
    pub fn wall_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != crate::app::Screen::Main || !window.is_window_active() { return; }
        let cards = matches!(self.level, Level::Home | Level::Server(_));
        if !cards && !self.wall.room_mode { return; }
        let every = if self.still() { Duration::from_secs(3) } else { Duration::from_secs(1) };
        if let Some(key) = self.wall.key.clone() {
            if !self.wall.busy && self.wall.polled.is_none_or(|t| t.elapsed() >= every) { self.wall_refresh(key, window, cx); }
        }
        // the other cards on screen: once, and again only after a while
        if let Some(k) = self.wall.visible.clone().into_iter().find(|k| Some(k) != self.wall.key.as_ref() && { let e = self.wall.entries.get(k); e.is_none_or(|e| !e.fetching && e.at.is_none_or(|t| t.elapsed() > Duration::from_secs(6))) }) {
            self.wall_fetch(k, window, cx);
        }
        if let Some((k, t)) = self.wall.share_back.clone() { if Instant::now() >= t { self.wall.share_back = None; self.wall_send_snapshot(k, window, cx); } }
        let due: Vec<String> = self.wall.resp.iter().filter(|(_, t)| Instant::now() >= **t).map(|(k, _)| k.clone()).collect();
        for k in due { self.wall.resp.remove(&k); self.wall_respond(k, window, cx); }
    }
    fn wall_fetch(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let e = self.wall.entry(&key); e.fetching = true; e.at = Some(Instant::now());
        self.rpc("wall_get", json!({"gid": key.clone()}), window, cx, move |this, r, _, cx| {
            this.wall.entry(&key).fetching = false;
            if let Ok(r) = r { if r["busy"].as_bool() != Some(true) && this.wall_apply(&key, &r) { cx.notify(); } }
        });
    }
    fn wall_apply(&mut self, key: &str, r: &Value) -> bool {
        let e = self.wall.entry(key);
        e.at = Some(Instant::now());
        let rev = r["rev"].as_u64().unwrap_or(0);
        e.locked = r["locked"].as_bool().unwrap_or(false);
        if r["same"].as_bool() == Some(true) && e.rev == rev { return false; }
        e.rev = rev;
        e.cells = r["cells"].as_array().map(|l| l.iter().filter_map(|c| { let c = c.as_array()?; let g = c.get(1)?.as_u64()? as u8; (g != 0).then(|| (c.first().and_then(|p| p.as_u64()).unwrap_or(0) as u16, (g, c.get(2).and_then(|x| x.as_u64()).unwrap_or(0) as u8))) }).collect()).unwrap_or_default();
        true
    }
    fn wall_refresh(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        self.wall.busy = true; self.wall.polled = Some(Instant::now());
        let rev = self.wall.entry(&key).rev;
        self.rpc("wall_get", json!({"gid": key.clone(), "rev": rev}), window, cx, move |this, r, window, cx| {
            this.wall.busy = false;
            let Ok(r) = r else { return };
            if this.wall.key.as_deref() != Some(key.as_str()) || r["busy"].as_bool() == Some(true) { return; }
            this.wall.gen = r["gen"].as_u64().unwrap_or(0);
            this.wall.peer_at = r["peer_at"].as_u64().unwrap_or(0) * 1000;
            this.wall.snap_at = r["snap_at"].as_u64().unwrap_or(0) * 1000;
            this.wall.carrier = r["carrier"].as_str().map(String::from);
            this.wall_share_back(window, cx);
            let was_locked = this.wall.entries.get(&key).is_some_and(|e| e.locked);
            let changed = this.wall_apply(&key, &r);
            if changed {
                // what was drawn here has arrived: the pending cells that the core now shows are no longer pending
                let cells = this.wall.entries.get(&key).map(|e| e.cells.clone()).unwrap_or_default();
                let out: Vec<u16> = this.wall.out.clone();
                this.wall.pend.retain(|p, v| out.contains(p) || !(cells.get(p) == Some(v) || (v.0 == 0 && !cells.contains_key(p))));
                cx.notify();
            }
            let locked = this.wall.entries.get(&key).is_some_and(|e| e.locked);
            if locked != was_locked || (locked && !this.wall_is_owner(&key)) { if this.wall.on && locked && !this.wall_is_owner(&key) { this.wall_off(); this.say("The owner locked the wall", cx); } cx.notify(); }
        });
    }

    // ---- the pen
    /// The draw chip / the room's button: drawing mode on or off (mega-wall.js `drawCard` / `on` / `off`).
    pub fn wall_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.wall.on { self.wall_off(); } else { self.wall_on(window, cx); }
        cx.notify();
    }
    pub fn wall_on(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.wall.key.clone() else { return };
        if self.wall.on || !self.wall_drawable(&key) { return; }
        self.wall.on = true; self.wall.from = None; self.wall.pen = false;
        if !self.wall_present() { self.wall_ask(true, window, cx); }
        cx.notify();
    }
    pub fn wall_off(&mut self) {
        if !self.wall.on { return; }
        self.wall.on = false; self.wall.pen = false; self.wall.from = None; self.wall.sure = None;
    }
    /// A press or a move of the pen at a window position. `down` starts a stroke (no line from where it last was).
    pub fn wall_pen(&mut self, pos: Point<Pixels>, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.wall.on { return; }
        if down { self.wall.from = None; self.wall.pen = true; } else if !self.wall.pen { return; }
        let sp = self.wall.spot;
        if sp.s <= 0. { return; }
        let g = grid_in(sp.w, sp.h, !self.wall.room_mode);
        // the box is at scale `s`: the pointer in the box's own pixels
        let (x, y) = ((f32::from(pos.x) - sp.x) / sp.s, (f32::from(pos.y) - sp.y) / sp.s);
        let c = cell_at(&g, x, y);
        if self.wall.stroke(c) { self.wall_queue(window, cx); cx.notify(); }
    }
    pub fn wall_pen_up(&mut self) { self.wall.pen = false; self.wall.from = None; }
    fn wall_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.wall.send_queued || self.wall.sending || self.wall.out.is_empty() { return; }
        self.wall.send_queued = true;
        let wait = self.wall.last_send.map(|t| (t + GAP).saturating_duration_since(Instant::now())).unwrap_or_default().max(Duration::from_millis(60));
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update_in(cx, |this, window, cx| { this.wall.send_queued = false; this.wall_flush(window, cx); });
        }).detach();
    }
    fn wall_flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.wall.key.clone() else { return };
        if self.wall.out.is_empty() || self.wall.sending { return; }
        self.wall.sending = true; self.wall.last_send = Some(Instant::now());
        let keys = self.wall.batch();
        let cells: Vec<Value> = keys.iter().map(|p| { let v = self.wall.pend.get(p).copied().unwrap_or((0, 0)); json!([*p as usize % COLS, *p as usize / COLS, v.0, v.1]) }).collect();
        let share = self.wall_present();
        if !share { self.wall.local_only = Some(key.clone()); self.wall_ask(true, window, cx); }
        self.rpc("wall_draw", json!({"gid": key.clone(), "cells": cells, "share": share}), window, cx, move |this, r, window, cx| {
            this.wall.sending = false;
            if this.wall.key.as_deref() != Some(key.as_str()) { return; }
            match r {
                Err(_) => { for p in &keys { this.wall.out.retain(|q| q != p); this.wall.pend.remove(p); } }   // refused (locked, bad cell): nothing is kept
                Ok(v) if v["sent"].as_bool() == Some(true) => {
                    this.wall.out.retain(|q| !keys.contains(q)); this.wall.tries = 0; if share { this.wall.sent += 1; }
                    this.wall.polled = None;
                }
                Ok(_) => { this.wall.tries += 1; if this.wall.tries > 12 { this.wall.out.retain(|q| !keys.contains(q)); this.wall.tries = 0; } }
            }
            this.wall_queue(window, cx); cx.notify();
        });
    }
    /// Owner: lock or unlock the wall; clear it for everyone (asks once).
    pub fn wall_lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.wall.key.clone() else { return };
        let (Some(cid), locked) = (self.wall_cid(&key), self.wall_locked(&key)) else { return };
        let room = if is_cw(&key) { cid.clone() } else { key.clone() };
        self.rpc("set_wall", json!({"cid": cid, "room": room, "lock": if locked { "0" } else { "1" }}), window, cx, |this, r, window, cx| {
            if let Err(e) = r { this.say(e, cx); } else { this.snap_hash.clear(); this.tick(window, cx); }
            cx.notify();
        });
    }
    pub fn wall_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.wall.key.clone() else { return };
        if !self.wall.sure.is_some_and(|t| t > Instant::now()) { self.wall.sure = Some(Instant::now() + Duration::from_secs(4)); cx.notify(); return; }
        self.wall.sure = None;
        let Some(cid) = self.wall_cid(&key) else { return };
        let room = if is_cw(&key) { cid.clone() } else { key.clone() };
        self.rpc("set_wall", json!({"cid": cid, "room": room, "clear": true}), window, cx, move |this, r, window, cx| {
            match r {
                Ok(_) => { this.wall.pend.clear(); this.wall.out.clear(); let e = this.wall.entry(&key); e.cells.clear(); e.rev = 0; this.wall.polled = None; this.snap_hash.clear(); this.tick(window, cx); }
                Err(e) => this.say(e, cx),
            }
            cx.notify();
        });
    }

    // ---- asking for, and answering with, a snapshot (mega-wall.js `ask`, `onLive`, `respondLater`, `shareBack`)
    fn wall_ask(&mut self, draw: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.wall.key.clone() else { return };
        let gap = if draw { Duration::from_secs(120) } else { ASK_EVERY };
        if self.wall.asked.get(&key).is_some_and(|t| t.elapsed() < gap) { return; }
        self.wall.asked.insert(key.clone(), Instant::now());
        self.wall_live(&key, "q", "{}".into(), window, cx);
    }
    fn wall_live(&mut self, key: &str, kind: &str, data: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = Self::wall_send_gid(key, self.wall.carrier.as_ref()) else { return };
        if data.len() > 3900 { return; }
        self.rpc("live_send", json!({"gid": g, "kind": format!("{}{}", Self::wall_prefix(key), kind), "data": data}), window, cx, |_, _, _, _| {});
    }
    /// A live event from the poll in presence.rs: somebody asked for the wall, or answered.
    pub fn wall_event(&mut self, e: &Value) {
        let (Some(kind), Some(gid)) = (e["kind"].as_str(), e["gid"].as_str()) else { return };
        let cw = kind.starts_with("cwall."); if !cw && !kind.starts_with("wall.") { return; }
        let Some(mine) = self.wall.key.clone() else { return };
        let key = if cw { match self.wall_cid(gid) { Some(c) => cw_key(&c), None => return } } else { gid.to_string() };
        if key != mine { return; }
        let k = &kind[kind.find('.').unwrap() + 1..];
        if k == "q" || k == "h" { self.wall.heard = Some(Instant::now()); }
        if k == "q" { self.wall.resp.entry(key).or_insert_with(|| Instant::now() + Duration::from_millis(300 + (now_ms() % 1200))); }
        else if k == "h" { self.wall.resp.remove(&key); }
        // somebody is here now: what was drawn while nobody seemed to be around goes out once
    }
    fn wall_share_back(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        let Some(key) = self.wall.key.clone() else { return };
        if self.wall.local_only.as_deref() != Some(key.as_str()) || !self.wall_present() { return; }
        self.wall.local_only = None;
        self.wall.share_back = Some((key, Instant::now() + Duration::from_millis(300 + (now_ms() % 700))));
    }
    fn wall_respond(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.wall.key.as_deref() != Some(key.as_str()) { return; }
        let since = now_ms().saturating_sub(2000);
        if self.wall.snap_at > since { return; }   // somebody else already answered
        self.wall_send_snapshot(key, window, cx);
    }
    fn wall_send_snapshot(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let k2 = key.clone();
        self.rpc("wall_snapshot", json!({"gid": key.clone()}), window, cx, move |this, r, window, cx| {
            let chunks: Vec<String> = r.ok().and_then(|v| v["chunks"].as_array().map(|l| l.iter().filter_map(|c| c.as_str().map(String::from)).collect())).unwrap_or_default();
            if this.wall.key.as_deref() != Some(k2.as_str()) { return; }
            if chunks.is_empty() { this.wall_live(&k2, "h", "{}".into(), window, cx); return; }
            let (k3, carrier) = (k2.clone(), this.wall.carrier.clone());
            let core = this.core.clone();
            // one chunk every 650 ms, so the relay is not flooded
            cx.background_spawn(async move {
                let Some(g) = Mega::wall_send_gid(&k3, carrier.as_ref()) else { return };
                for (i, c) in chunks.iter().enumerate() {
                    if i > 0 { std::thread::sleep(Duration::from_millis(650)); }
                    let _ = core.call("live_send", json!({"gid": g, "kind": format!("{}s", Self::wall_prefix(&k3)), "data": c}));
                }
            }).detach();
        });
    }

    // ---- for the test port
    pub fn wall_dump(&self) -> Value {
        let w = &self.wall;
        let key = w.key.clone().unwrap_or_default();
        let marks = w.marks_of(&key);
        let own: Vec<Value> = marks.iter().map(|(p, g, c)| json!([p, g, c])).collect();
        json!({"key": key, "on": w.on, "locked": self.wall_locked(&key), "owner": self.wall_is_owner(&key), "drawable": self.wall_drawable(&key), "brush": w.brush, "col": w.col,
            "cells": own, "pending": w.pend.len(), "out": w.out.len(), "sent": w.sent, "present": self.wall_present(), "spot": [w.spot.x, w.spot.y, w.spot.w, w.spot.h, w.spot.s],
            "visible": w.visible, "entries": w.entries.iter().map(|(k, e)| (k.clone(), e.cells.len())).collect::<HashMap<_, _>>(), "gen": w.gen})
    }
}

// ---------------------------------------------------------------- the layer that catches the pen, and the toolbar
impl Mega {
    /// The layer over the focused card while drawing (mega.css `#mega-wallhit-card`): catches the pen, and the wheel so the column does not move.
    pub fn wall_hit(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.wall.on || self.wall.room_mode || self.wall.key.is_none() { return None; }
        let sp = self.wall.spot;
        let r = crate::nav::SQ * sp.s;
        let _ = r;
        Some(div().id("wall-hit").absolute().left(px(sp.x)).top(px(sp.y)).w(px(sp.w * sp.s)).h(px(sp.h * sp.s)).cursor_crosshair().occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| { cx.stop_propagation(); this.wall_pen(ev.position, true, window, cx); }))
            .child(canvas(|_, _, _| (), move |b, _, window, _| { crate::squircle::stroke(window, b, crate::nav::SQ * sp.s, 2., hsla(0., 0., 1., 0.5)); }).absolute().top_0().left_0().size_full())
            .into_any_element())
    }

    /// In a room: the layer that holds the room's wall behind the chat (the marks), and measures the area the grid is centred in.
    pub fn wall_room_layer(&mut self, gid: &str) -> AnyElement {
        let rect = self.wall.room_rect.clone();
        let (w, h) = { let r = rect.get(); (r.2, r.3) };
        let marks_cells = Rc::new(self.wall.marks_of(gid));
        let ink = self.wall_inks(gid);
        let live = Some((self.holes.clone(), 0., false));
        let grid = grid_in(w.max(1.), h.max(1.), false);
        div().absolute().inset_0()
            .child(canvas(move |b, _, _| rect.set((f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .when(!marks_cells.is_empty(), |d| d.child(marks(Marks { cells: marks_cells, grid, ink, holes: Rc::new(vec![]), live })))
            .into_any_element()
    }
    /// The layer over the open room while drawing: catches the pen (and the wheel).
    pub fn wall_room_hit(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.wall.on || !self.wall.room_mode { return None; }
        Some(div().id("wall-hit-room").absolute().inset_0().cursor_crosshair().occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| { cx.stop_propagation(); this.wall_pen(ev.position, true, window, cx); }))
            .into_any_element())
    }
    /// The room's "draw" button (mega-wall.js `chrome`): hidden when the owner locked the wall and this is not the owner.
    pub fn wall_room_button(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.wall.key.clone()?;
        if !self.wall_drawable(&key) { return None; }
        Some(crate::cards::chip("b-draw-room", if self.wall.on { "drawing" } else { "draw" }).on_click(cx.listener(|this, _, window, cx| this.wall_toggle(window, cx))).into_any_element())
    }
    /// A room is open (or not): its wall is the one under the focus.
    pub fn wall_room_focus(&mut self, gid: Option<String>) {
        let r = self.wall.room_rect.get();
        self.wall.room_mode = true;
        self.wall_focus(gid, Spot { x: r.0, y: r.1, w: r.2, h: r.3, s: 1. }, vec![]);
    }

    /// The tools (mega.css `#mega-wallbar`): glyphs, colours, the owner's lock and clear, done.
    pub fn wall_toolbar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.wall.on { return None; }
        let key = self.wall.key.clone()?;
        let owner = self.wall_is_owner(&key);
        let locked = self.wall_locked(&key);
        let ink = self.wall_inks(&key);
        let (brush, col) = (self.wall.brush, self.wall.col);
        let paper = look::paper();
        let sure = self.wall.sure.is_some_and(|t| t > Instant::now());
        let btn = |id: &'static str, label: &'static str| div().id(id).flex().items_center().justify_center().h(px(30.)).min_w(px(30.)).px(px(10.)).rounded_full().cursor_pointer().bg(hsla(0., 0., 1., 0.1)).font_family(look::F_UI).text_size(px(12.)).child(label);
        let glyphs = h_flex().flex_wrap().justify_center().gap(px(2.)).p(px(3.)).rounded(px(20.)).bg(hsla(0., 0., 1., 0.07))
            .children(GLYPHS.chars().enumerate().skip(1).map(|(i, c)| {
                let on = brush as usize == i;
                div().id(("mw-g", i)).flex().items_center().justify_center().h(px(30.)).min_w(px(30.)).px(px(10.)).rounded_full().cursor_pointer().font_family(look::F_MONO).font_weight(FontWeight::BOLD).text_size(px(14.))
                    .when(on, |d| d.bg(paper).text_color(rgb(look::VOID))).hover(|d| d.bg(hsla(0., 0., 1., if on { 1. } else { 0.12 }))).child(c.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| { this.wall.brush = i as u8; cx.notify(); }))
            }))
            .child({ let on = brush == 0; div().id(("mw-g", 0usize)).flex().items_center().justify_center().h(px(30.)).min_w(px(30.)).px(px(10.)).rounded_full().cursor_pointer().font_family(look::F_MONO).font_weight(FontWeight::BOLD).text_size(px(14.))
                .when(on, |d| d.bg(paper).text_color(rgb(look::VOID))).child("erase").on_click(cx.listener(|this, _, _, cx| { this.wall.brush = 0; cx.notify(); })) });
        let swatches: Vec<(String, u32)> = [("room colour".to_string(), ink[0]), ("room highlight".to_string(), ink[1])].into_iter().chain(PAL.iter().map(|(n, c)| (n.to_string(), *c))).collect();
        let colours = h_flex().flex_wrap().justify_center().gap(px(3.)).p(px(3.)).rounded(px(20.)).bg(hsla(0., 0., 1., 0.07))
            .children(swatches.into_iter().enumerate().map(|(i, (_n, c))| {
                let on = col as usize == i;
                div().id(("mw-c", i)).flex().items_center().justify_center().size(px(32.)).rounded_full().cursor_pointer().when(on, |d| d.bg(hsla(0., 0., 1., 0.22)))
                    .child(div().size(px(20.)).rounded_full().bg(rgb(c)).border_1().border_color(hsla(0., 0., 1., 0.35)).when(on, |d| d.size(px(23.6)).border_2().border_color(paper)))
                    .on_click(cx.listener(move |this, _, _, cx| { this.wall.col = i as u8; cx.notify(); }))
            }));
        let room = self.wall.room_mode;
        let place = if room { div().absolute().left_0().right_0().top(px(58.)).flex().justify_center() } else { div().absolute().left_0().right_0().bottom(px(14.)).flex().justify_center() };
        Some(place.child(
            h_flex().id("mega-wallbar").flex_wrap().justify_center().items_center().gap(px(8.)).max_w(px(if room { (self.wall.room_rect.get().2 / 2.).max(300.) } else { self.nav.vp.get().0.max(400.) / 2. })).px(px(12.)).py(px(8.)).rounded(px(26.)).bg(rgb(0x17151e)).border_1().border_color(hsla(0., 0., 1., 0.14))
                .font_family(look::F_UI).font_weight(FontWeight::SEMIBOLD).text_size(px(12.)).text_color(paper).occlude()
                .child(div().px(px(4.)).opacity(0.6).font_weight(FontWeight::MEDIUM).child("everyone sees this"))
                .child(glyphs).child(colours)
                .when(owner, |d| d.child(btn("mw-lock", if locked { "locked" } else { "lock" }).when(locked, |b| b.bg(paper).text_color(rgb(look::VOID))).on_click(cx.listener(|this, _, window, cx| this.wall_lock(window, cx))))
                    .child(btn("mw-clear", if sure { "clear for everyone?" } else { "clear" }).on_click(cx.listener(|this, _, window, cx| this.wall_clear(window, cx)))))
                .child(btn("mw-done", "done").on_click(cx.listener(|this, _, _, cx| { this.wall_off(); cx.notify(); })))).into_any_element())
    }

    /// The two inks of the wall's card (the first two colours of the toolbar).
    pub fn wall_inks(&self, key: &str) -> [u32; 2] {
        let Some(c) = self.wall_cid(key).and_then(|c| self.snap.as_ref()?.community(&c).cloned()) else { return [0xffffff, 0xffffff] };
        let (id, chosen) = if is_cw(key) { (c.id.clone(), c.colours.clone()) } else { (key.to_string(), c.room_colours.iter().find(|(g, _)| g == key).map(|(_, d)| d.clone())) };
        let d = self.duo_for(&id, chosen.as_ref());
        if self.wall.room_mode { [look::lift(d.main, 5.), look::lift(d.hi, 6.)] } else { look::wall_ink(&d) }
    }
}

// ---------------------------------------------------------------- painting
struct Glyphs { font: FontId, ids: Vec<Option<GlyphId>> }
fn glyphs(window: &mut Window) -> Glyphs {
    let ts = window.text_system().clone();
    let f = font(look::F_MONO);
    let id = ts.resolve_font(&f);
    let ids = GLYPHS.chars().map(|c| {
        if c == ' ' { return None; }
        let s: SharedString = c.to_string().into();
        let run = TextRun { len: s.len(), font: f.clone(), color: gpui_kit::black(), background_color: None, underline: None, strikethrough: None };
        ts.shape_line(s, px(14.), &[run], None).runs.first().and_then(|r| r.glyphs.first()).map(|g| g.id)
    }).collect();
    Glyphs { font: id, ids }
}

/// What a marks layer paints.
#[derive(Clone)]
pub struct Marks { pub cells: Rc<Vec<(u16, u8, u8)>>, pub grid: Grid, pub ink: [u32; 2], /** boxes (relative to the layer) with words on them: marks under them are not painted */ pub holes: Rc<Vec<Bounds<Pixels>>>, /** boxes in window coordinates gathered during layout (ascii::hole), widened by .1 each side, and whether a cell is under one by its centre (the cards, as the page maps its text mask) or by any overlap (a room) */ pub live: Option<(crate::ascii::Holes, f32, bool)> }

/// The element: fills its parent, which is the box the grid was made for.
pub fn marks(m: Marks) -> impl IntoElement {
    canvas(|_, _, _| (), move |bounds, _, window, _| {
        if m.cells.is_empty() { return; }
        let gl = glyphs(window);
        let g = &m.grid;
        let (ox, oy) = ((g.cw - g.fs * 0.6) / 2., g.ch * 0.72);
        let live: Vec<(f32, f32, f32, f32)> = m.live.as_ref().map(|(h, pad, _)| h.borrow().iter().map(|b| (f32::from(b.origin.x - bounds.origin.x) - pad, f32::from(b.origin.y - bounds.origin.y), f32::from(b.size.width) + 2. * pad, f32::from(b.size.height))).collect()).unwrap_or_default();
        let by_centre = m.live.as_ref().is_some_and(|l| l.2);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for &(p, glyph, col) in m.cells.iter() {
                let Some(Some(id)) = gl.ids.get(glyph as usize) else { continue };
                let (l, t, w, h) = cell_rect(g, p);
                if m.holes.iter().any(|b| { let (bx, by, bw, bh) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)); l + w / 2. > bx && l + w / 2. < bx + bw && t + h / 2. > by && t + h / 2. < by + bh }) { continue; }
                if live.iter().any(|&(bx, by, bw, bh)| text_box_hides(by_centre, (l, t, w, h), (bx, by, bw, bh))) { continue; }
                let c: Hsla = rgb(ink_of(m.ink, col)).into();
                let _ = window.paint_glyph(point(bounds.origin.x + px(l + ox), bounds.origin.y + px(t + oy)), gl.font, *id, px(g.fs), c);
            }
        });
    }).absolute().top_0().left_0().size_full()
}

/// Is a mark's cell (l, t, w, h) under a text box? The page hides a card's mark when its centre is under the padded box,
/// and a room's when any part of the cell is.
pub fn text_box_hides(by_centre: bool, (l, t, w, h): (f32, f32, f32, f32), (bx, by, bw, bh): (f32, f32, f32, f32)) -> bool {
    if by_centre { l + w / 2. > bx && l + w / 2. < bx + bw && t + h / 2. > by && t + h / 2. < by + bh }
    else { l + w > bx && l < bx + bw && t + h > by && t < by + bh }
}

/// The two inks of the card the wall is on.
pub fn ink_for(d: &Duo) -> [u32; 2] { look::wall_ink(d) }

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn the_grid_stretches_over_a_card_of_about_its_shape_and_centres_in_any_other_box() {
        // the grid is 1200 x 918 units: a 648 x 648 card is a little taller than that, within the 0.6..1.8 band, so it is stretched
        let g = grid_in(648., 648., true);
        assert_eq!((g.ox, g.oy), (0., 0.));
        assert!((g.cw - 648. / 120.).abs() < 1e-4 && (g.ch - 648. / 54.).abs() < 1e-4);
        // the same box as a room (not a card) is centred at one scale
        let r = grid_in(648., 648., false);
        assert!((r.cw / CW - r.ch / CH).abs() < 1e-4, "one scale on both axes");
        assert!(r.ox.abs() < 1e-3 && r.oy > 0., "fills the width, centred in the height");
        // a very wide card is centred, not stretched
        let w = grid_in(1600., 200., true);
        assert!(w.ox > 0. && (w.cw / CW - w.ch / CH).abs() < 1e-4);
        assert!(g.fs >= 6.);
    }
    #[::core::prelude::v1::test]
    fn a_point_maps_to_its_cell() {
        let g = grid_in(1200., 918., true);   // exactly the grid's own size: cells are 10 x 17
        assert_eq!(cell_at(&g, 0., 0.), (0, 0));
        assert_eq!(cell_at(&g, 9.9, 16.9), (0, 0));
        assert_eq!(cell_at(&g, 10., 17.), (1, 1));
        assert_eq!(cell_at(&g, 1199., 917.), (119, 53));
        assert_eq!(cell_at(&g, -1., 5.).0, -1, "outside to the left");
        // a card shown at half scale maps the same cells
        let s = g.scaled(0.5);
        assert_eq!(cell_at(&s, 5., 8.5), (1, 1));
    }
    #[::core::prelude::v1::test]
    fn a_stroke_covers_every_cell_between() {
        assert_eq!(line((2, 2), (2, 2)), vec![(2, 2), (2, 2)], "a click is one cell (twice, harmlessly)");
        assert_eq!(line((0, 0), (3, 0)), vec![(0, 0), (1, 0), (2, 0), (3, 0)]);
        let d = line((0, 0), (4, 2));
        assert_eq!(d.first(), Some(&(0, 0)));
        assert_eq!(d.last(), Some(&(4, 2)));
        assert!(d.windows(2).all(|w| (w[1].0 - w[0].0).abs() <= 1 && (w[1].1 - w[0].1).abs() <= 1), "no gaps: {d:?}");
        assert_eq!(line((3, 0), (0, 0)), vec![(3, 0), (2, 0), (1, 0), (0, 0)]);
    }
    #[::core::prelude::v1::test]
    fn the_brush_puts_cells_and_an_erase_removes_them() {
        let mut w = WallUi::new();
        w.key = Some("room".into());
        w.brush = 2; w.col = 5;
        assert!(w.put(3, 1));
        assert_eq!(w.pend.get(&(COLS as u16 + 3)), Some(&(2, 5)));
        assert!(!w.put(3, 1), "the same cell again changes nothing");
        assert!(!w.put(-1, 0) && !w.put(COLS as i32, 0) && !w.put(0, ROWS as i32), "outside the grid");
        w.brush = 0;
        assert!(w.put(3, 1), "erasing a pending cell");
        assert_eq!(w.pend.get(&(COLS as u16 + 3)), Some(&(0, 0)), "an erase has no colour");
        assert!(!w.put(50, 50), "erasing an empty cell is nothing");
        // what the core holds counts as the current value
        w.entry("room").cells.insert(7, (1, 1));
        assert!(!{ w.brush = 1; w.col = 1; w.put(7, 0) }, "same glyph and colour as the wall already has");
        w.col = 0;
        assert!(w.put(7, 0));
        // the picture shows the core's wall with pending cells on top; a pending erase hides a cell
        w.brush = 0;
        assert!(w.put(7, 0));
        assert!(w.marks_of("room").iter().all(|m| m.0 != 7));
    }
    #[::core::prelude::v1::test]
    fn a_batch_is_one_kind_and_at_most_two_hundred() {
        let mut w = WallUi::new();
        w.key = Some("r".into());
        w.brush = 1; w.col = 0;
        for x in 0..120 { w.put(x, 0); }
        w.col = 4;
        for x in 0..120 { w.put(x, 1); }
        let b = w.batch();
        assert_eq!(b.len(), 120, "the room's own colours first, the palette colours in the next event");
        assert!(b.iter().all(|p| w.pend[p].1 <= 1));
        w.out.retain(|p| !b.contains(p));
        let b2 = w.batch();
        assert_eq!(b2.len(), 120);
        assert!(b2.iter().all(|p| w.pend[p].1 > 1));
        for y in 2..6 { for x in 0..120 { w.put(x, y); } }
        assert!(w.batch().len() <= BATCH);
    }
    #[::core::prelude::v1::test]
    fn inks_are_the_rooms_two_then_the_palette() {
        let ink = [0x111111, 0x222222];
        assert_eq!(ink_of(ink, 0), 0x111111);
        assert_eq!(ink_of(ink, 1), 0x222222);
        assert_eq!(ink_of(ink, 2), 0xffffff);
        assert_eq!(ink_of(ink, 11), 0xff7ac8);
        assert_eq!(ink_of(ink, 200), 0xffffff, "an unknown colour is white");
    }
    #[::core::prelude::v1::test]
    fn the_fields_ascii_flows_around_the_drawing() {
        let g = grid_in(1200., 918., true);
        // one wall cell at (10, 2): 100..110 px across, 34..51 px down; a field of 10 x 17 px cells holds it in exactly one cell
        let m = field_mask(&g, [2 * COLS as u16 + 10].into_iter(), 10., 17., 120, 54);
        assert_eq!(m.iter().filter(|b| **b).count(), 4, "touching the next cell's edge counts, as in the page: {:?}", m.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| (i % 120, i / 120)).collect::<Vec<_>>());
        assert!(m[2 * 120 + 10]);
        // no marks, no mask; a field of nothing is fine
        assert!(field_mask(&g, [].into_iter(), 10., 17., 120, 54).iter().all(|b| !*b));
        assert!(field_mask(&g, [5u16].into_iter(), 10., 17., 0, 0).is_empty());
        // a bigger cell grid covers more of the field
        let m = field_mask(&g, [0u16].into_iter(), 5., 8.5, 240, 108);
        assert!(m[0] && m[1] && m[240]);
    }
    #[::core::prelude::v1::test]
    fn the_stroke_value_of_an_erase_has_no_colour() { assert_eq!(stroke_value(0, 7), (0, 0)); assert_eq!(stroke_value(3, 7), (3, 7)); }
}

#[cfg(test)]
mod text_tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn marks_under_words_are_hidden_by_centre_on_cards_and_by_overlap_in_rooms() {
        let b = (100., 100., 50., 20.);
        // a cell whose centre is outside the box but which overlaps it
        let cell = (90., 100., 12., 10.);
        assert!(!text_box_hides(true, cell, b));
        assert!(text_box_hides(false, cell, b));
        assert!(text_box_hides(true, (110., 105., 6., 6.), b));
        assert!(!text_box_hides(false, (200., 100., 6., 6.), b));
    }
}
