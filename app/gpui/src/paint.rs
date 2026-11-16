//! The colours panel (app/ui/mega.js `paint`): the owner's editor for a card's colour and highlight (a square, a hue strip, a hex box, presets),
//! the card's and the chat's animation, the backdrop's own colours and strength for a text room, and the Oracle's two switches. Everything
//! changes on this screen at once and is sent to everyone only when the person lets go (one request at a time, the newest value per card),
//! through the same `set_style` / `set_look` / `set_oracle_*` calls the page makes, so web and iOS clients see it.
use crate::app::{Level, Mega};
use crate::ascii;
use crate::cards::chip;
use crate::look::{self, Duo};
use crate::model::Look;
use crate::squircle as sq;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The most a backdrop's strength can be (mega.js `DIM_MAX`).
pub const DIM_MAX: u32 = 160;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind { Community, Text, Voice, Options }

/// The card the panel is about: the focused card of the column.
#[derive(Clone, Debug, PartialEq)]
pub struct Target { pub id: String, pub cid: String, pub gid: String, pub kind: Kind, pub name: String, pub fx_auto: usize, pub duo: Duo }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Drag { Sv, Hue, Dim }

/// A look being edited on this screen, kept until the snapshot catches up.
#[derive(Clone, Debug)]
pub struct LookHold { pub look: Look, pub back_null: bool, pub until: Instant }

enum Save { Style { cid: String, gid: String, main: String, hi: String }, Look { cid: String, gid: String, look: Look, back_null: bool }, Oracle { cid: String, gid: String, method: &'static str, on: bool } }

pub struct PaintUi {
    pub open: bool,
    pub target: Option<Target>,
    pub back_mode: bool,
    pub well: usize,
    pub hsv: [f32; 3],
    pub hex: Entity<InputState>,
    setting_hex: bool,
    /// Colours on this screen now, by id (a card's or room's), until the snapshot shows the same.
    pub shown: HashMap<String, ((u32, u32), Instant)>,
    pub looks: HashMap<String, LookHold>,
    pub oracle: HashMap<(String, bool), (bool, Instant)>,
    queue: Vec<Save>,
    saving: bool,
    commit_at: Option<Instant>,
    pub sv: Rc<Cell<Bounds<Pixels>>>,
    pub hue: Rc<Cell<Bounds<Pixels>>>,
    pub dim: Rc<Cell<Bounds<Pixels>>>,
    pub drag: Option<Drag>,
    pub scroll: ScrollHandle,
}

impl PaintUi {
    pub fn new(window: &mut Window, cx: &mut Context<Mega>) -> (PaintUi, Subscription) {
        let hex = cx.new(|cx| InputState::new(window, cx).placeholder("#rrggbb"));
        let sub = cx.subscribe_in(&hex, window, |this: &mut Mega, _, ev: &InputEvent, window, cx| { if let InputEvent::Change = ev { this.paint_hex_changed(window, cx); } });
        (PaintUi { open: false, target: None, back_mode: false, well: 0, hsv: [0.; 3], hex, setting_hex: false, shown: HashMap::new(), looks: HashMap::new(), oracle: HashMap::new(), queue: vec![], saving: false, commit_at: None,
            sv: Default::default(), hue: Default::default(), dim: Default::default(), drag: None, scroll: ScrollHandle::new() }, sub)
    }
}

fn hexs(c: u32) -> String { look::hex(c) }
fn hold_for() -> Instant { Instant::now() + Duration::from_secs(6) }

impl Mega {
    /// Whether this person may change the card's colours: the owner of its community.
    pub fn can_paint(&self, t: &Target) -> bool { t.kind != Kind::Options && self.snap.as_ref().and_then(|s| s.community(&t.cid)).is_some_and(|c| c.role == "owner") }

    /// The colours a card shows: what the owner is editing here right now, else what they chose, else the default for its id (mega.js `themeOf`).
    pub fn duo_for(&self, id: &str, chosen: Option<&(String, String)>) -> Duo {
        if let Some(((m, h), until)) = self.paint.shown.get(id) { if *until > Instant::now() { return Duo { main: *m, hi: *h }; } }
        Duo::of(id, chosen)
    }
    /// A look as it stands: held while being edited, else from the snapshot.
    pub fn look_for(&self, cid: &str, id: &str) -> Look {
        if let Some(h) = self.paint.looks.get(id) { if h.until > Instant::now() { return h.look.clone(); } }
        self.snap.as_ref().and_then(|s| s.community(cid)).map(|c| c.look_of(id)).unwrap_or_default()
    }
    fn target_id(t: &Target) -> String { if t.kind == Kind::Community { t.cid.clone() } else { t.gid.clone() } }
    /// The two colours being edited: the backdrop's own in the backdrop mode (when it has them), else the card's.
    pub fn paint_cur(&self) -> (u32, u32) {
        let Some(t) = &self.paint.target else { return (0, 0) };
        if self.paint.back_mode && t.kind == Kind::Text { if let Some((m, h)) = self.look_for(&t.cid, &t.gid).back.and_then(|(m, h)| Some((look::parse_hex(&m)?, look::parse_hex(&h)?))) { return (m, h); } }
        let d = self.duo_for(&Self::target_id(t), self.snap.as_ref().and_then(|s| s.community(&t.cid)).and_then(|c| if t.kind == Kind::Community { c.colours.as_ref() } else { c.room_colours.iter().find(|(g, _)| g == &t.gid).map(|(_, d)| d) }));
        (d.main, d.hi)
    }
    fn paint_well_colour(&self) -> u32 { let (m, h) = self.paint_cur(); if self.paint.well == 0 { m } else { h } }

    // ---- opening, closing, following the focus
    pub fn paint_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.paint.open { self.paint_close(cx); return; }
        let Some(t) = self.nav.focus_card.clone() else { return };
        if !self.can_paint(&t) { return; }
        self.paint.open = true; self.paint.back_mode = false; self.paint.well = 0;
        self.paint_set(t, window, cx);
        cx.notify();
    }
    pub fn paint_close(&mut self, cx: &mut Context<Self>) { self.paint.open = false; self.paint.drag = None; cx.notify(); }
    /// The focus moved to another card while the panel is open (mega.js `PT.set`).
    pub fn paint_follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.paint.open { return; }
        match self.nav.focus_card.clone() {
            None => self.paint_close(cx),
            Some(t) if self.paint.target.as_ref().is_some_and(|o| o.id == t.id) => { self.paint.target = Some(t); }
            Some(t) if !self.can_paint(&t) && t.kind != Kind::Options => self.paint_close(cx),
            Some(t) => { self.paint.back_mode = false; self.paint_set(t, window, cx); cx.notify(); }
        }
    }
    fn paint_set(&mut self, t: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.paint.target = Some(t);
        self.paint_resync(window, cx);
    }
    /// The picker's square and the hex box show the colour being edited.
    pub fn paint_set_well(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) { self.paint.well = i.min(1); self.paint_resync(window, cx); cx.notify(); }
    pub fn paint_set_dim(&mut self, v: u32, cx: &mut Context<Self>) { self.paint_look(|l| l.dim = Some(v.min(DIM_MAX)), false, true); cx.notify(); }
    pub fn paint_same(&mut self, window: &mut Window, cx: &mut Context<Self>) { self.paint_look(|l| l.back = None, true, true); self.paint_resync(window, cx); cx.notify(); }
    pub fn paint_queue_len(&self) -> usize { self.paint.queue.len() + self.paint.saving as usize }
    fn paint_resync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let c = self.paint_well_colour();
        self.paint.hsv = look::to_hsv(c);
        self.paint_set_hex(hexs(c), window, cx);
    }
    fn paint_set_hex(&mut self, v: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.paint.hex.read(cx).value().as_ref() == v { return; }
        self.paint.setting_hex = true;
        self.paint.hex.update(cx, |h, cx| h.set_value(v, window, cx));
        self.paint.setting_hex = false;
    }

    // ---- changing the colours
    /// Colours changed here: live on this screen at once (the card, or the backdrop), sent when committed.
    fn paint_apply(&mut self, d: (u32, u32), window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.paint.target.clone() else { return };
        if !self.can_paint(&t) { return; }
        if self.paint.back_mode && t.kind == Kind::Text {
            let mut l = self.look_for(&t.cid, &t.gid); l.back = Some((hexs(d.0), hexs(d.1)));
            self.paint.looks.insert(t.gid.clone(), LookHold { look: l, back_null: false, until: hold_for() });
        } else {
            self.paint.shown.insert(Self::target_id(&t), (d, hold_for()));
        }
        let _ = window;
        cx.notify();
    }
    fn paint_edit(&mut self, d: (u32, u32), later: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.paint_apply(d, window, cx);
        if later { self.paint.commit_at = Some(Instant::now() + Duration::from_millis(450)); }
    }
    fn paint_commit(&mut self) {
        self.paint.commit_at = None;
        let Some(t) = self.paint.target.clone() else { return };
        if self.paint.back_mode && t.kind == Kind::Text { self.paint_send_look(&t, false); }
        else if let Some(((m, h), _)) = self.paint.shown.get(&Self::target_id(&t)).cloned() {
            self.paint.queue.retain(|s| !matches!(s, Save::Style { cid, gid, .. } if *cid == t.cid && *gid == t.gid));
            self.paint.queue.push(Save::Style { cid: t.cid.clone(), gid: t.gid.clone(), main: hexs(m), hi: hexs(h) });
        }
    }
    fn paint_send_look(&mut self, t: &Target, back_null: bool) {
        let id = Self::target_id(t);
        let held = self.paint.looks.get(&id).cloned();
        let look = held.map(|h| h.look).unwrap_or_else(|| self.look_for(&t.cid, &id));
        self.paint.queue.retain(|s| !matches!(s, Save::Look { cid, gid, .. } if *cid == t.cid && *gid == t.gid));
        self.paint.queue.push(Save::Look { cid: t.cid.clone(), gid: t.gid.clone(), look, back_null });
    }
    /// A look edited here (mega.js `editLook`): shown at once, and sent now when `commit`.
    fn paint_look(&mut self, f: impl FnOnce(&mut Look), back_null: bool, commit: bool) {
        let Some(t) = self.paint.target.clone() else { return };
        if !self.can_paint(&t) { return; }
        let id = Self::target_id(&t);
        let mut l = self.look_for(&t.cid, &id);
        f(&mut l);
        self.paint.looks.insert(id, LookHold { look: l, back_null, until: hold_for() });
        if commit { self.paint_send_look(&t, back_null); }
    }
    pub fn paint_hex_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.paint.setting_hex || !self.paint.open { return; }
        let mut v = self.paint.hex.read(cx).value().trim().to_string();
        if !v.is_empty() && !v.starts_with('#') { v = format!("#{v}"); }
        if !look::is_hex(&v) { return; }
        let c = look::parse_hex(&v).unwrap_or(0);
        self.paint.hsv = look::to_hsv(c);
        let mut d = self.paint_cur();
        if self.paint.well == 0 { d.0 = c; } else { d.1 = c; }
        self.paint_edit(d, true, window, cx);
    }
    /// A pick in the square or on the hue strip, or a drag of the strength.
    pub fn paint_drag_to(&mut self, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(k) = self.paint.drag else { return };
        let (x, y) = (f32::from(pos.x), f32::from(pos.y));
        match k {
            Drag::Sv => {
                let b = self.paint.sv.get(); let (l, t, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width).max(1.), f32::from(b.size.height).max(1.));
                self.paint.hsv[1] = ((x - l) / w).clamp(0., 1.); self.paint.hsv[2] = 1. - ((y - t) / h).clamp(0., 1.);
                self.paint_pick(window, cx);
            }
            Drag::Hue => {
                let b = self.paint.hue.get(); let (l, w) = (f32::from(b.origin.x), f32::from(b.size.width).max(1.));
                self.paint.hsv[0] = (((x - l) / w).clamp(0., 1.) * 359.).round();
                self.paint_pick(window, cx);
            }
            Drag::Dim => {
                let b = self.paint.dim.get(); let (l, w) = (f32::from(b.origin.x), f32::from(b.size.width).max(1.));
                let v = ((((x - l) / w).clamp(0., 1.) * DIM_MAX as f32 / 5.).round() * 5.) as u32;
                self.paint_look(|l| l.dim = Some(v), false, false);
                cx.notify();
            }
        }
    }
    fn paint_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let c = look::from_hsv(self.paint.hsv[0], self.paint.hsv[1], self.paint.hsv[2]);
        let mut d = self.paint_cur();
        if self.paint.well == 0 { d.0 = c; } else { d.1 = c; }
        self.paint_set_hex(hexs(c), window, cx);
        self.paint_apply(d, window, cx);
    }
    pub fn paint_drag_end(&mut self, cx: &mut Context<Self>) {
        let Some(k) = self.paint.drag.take() else { return };
        match k { Drag::Dim => self.paint_look(|_| {}, false, true), _ => self.paint_commit() }
        cx.notify();
    }
    pub fn paint_preset(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&(m, h)) = look::presets().get(i) else { return };
        let c = if self.paint.well == 0 { m } else { h };
        self.paint.hsv = look::to_hsv(c); self.paint_set_hex(hexs(c), window, cx);
        self.paint_apply((m, h), window, cx);
        self.paint_commit();
        self.paint_resync(window, cx);
    }
    pub fn paint_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.paint.target.clone() else { return };
        if !self.can_paint(&t) { return; }
        let id = Self::target_id(&t);
        let d = look::DEF[(look::h(&id) as usize) % look::DEF.len()];
        self.paint.shown.insert(id.clone(), (d, hold_for()));
        self.paint.queue.retain(|s| !matches!(s, Save::Style { cid, gid, .. } if *cid == t.cid && *gid == t.gid));
        self.paint.queue.push(Save::Style { cid: t.cid.clone(), gid: t.gid.clone(), main: String::new(), hi: String::new() });
        self.paint.looks.insert(id, LookHold { look: Look::default(), back_null: true, until: hold_for() });
        self.paint_send_look(&t, true);
        self.paint.back_mode = false;
        self.paint_resync(window, cx);
        cx.notify();
    }
    pub fn paint_pattern(&mut self, which: &str, n: Option<usize>, cx: &mut Context<Self>) {
        match which { "card" => self.paint_look(|l| l.card = n, false, true), _ => self.paint_look(|l| l.chat = n, false, true) }
        cx.notify();
    }
    pub fn paint_oracle(&mut self, draw: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.paint.target.clone() else { return };
        if t.kind != Kind::Text { return; }
        let l = self.look_for(&t.cid, &t.gid);
        let now = if draw { l.oracle_draw } else { l.oracle_quiet };
        let cur = self.paint.oracle.get(&(t.gid.clone(), draw)).filter(|(_, u)| *u > Instant::now()).map(|(o, _)| *o).unwrap_or(now);
        let on = !cur;
        self.paint.oracle.insert((t.gid.clone(), draw), (on, Instant::now() + Duration::from_secs(15)));
        self.paint.queue.push(Save::Oracle { cid: t.cid.clone(), gid: t.gid.clone(), method: if draw { "set_oracle_draw" } else { "set_oracle_quiet" }, on });
        let _ = window; cx.notify();
    }

    /// The quarter-second clock: commit what was changed a moment ago, send one saved change at a time, forget what the snapshot now shows.
    pub fn paint_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.paint.commit_at.is_some_and(|t| Instant::now() >= t) { self.paint_commit(); }
        let now = Instant::now();
        let (a, b, c) = (self.paint.shown.len(), self.paint.looks.len(), self.paint.oracle.len());
        self.paint.shown.retain(|_, (_, u)| *u > now);
        self.paint.looks.retain(|_, h| h.until > now);
        self.paint.oracle.retain(|_, (_, u)| *u > now);
        if (a, b, c) != (self.paint.shown.len(), self.paint.looks.len(), self.paint.oracle.len()) { cx.notify(); }
        if self.paint.saving || self.paint.queue.is_empty() { return; }
        self.paint.saving = true;
        let s = self.paint.queue.remove(0);
        let (method, args): (&'static str, Value) = match s {
            Save::Style { cid, gid, main, hi } => ("set_style", json!({"cid": cid, "room": gid, "main": main, "hi": hi})),
            Save::Oracle { cid, gid, method, on } => (method, json!({"cid": cid, "room": gid, "on": on})),
            Save::Look { cid, gid, look, back_null } => {
                let mut a = json!({"cid": cid, "room": gid});
                if let Some(n) = look.card { a["card"] = json!(n); }
                if let Some(n) = look.chat { a["chat"] = json!(n); }
                if let Some(n) = look.dim { a["dim"] = json!(n); }
                if back_null { a["back"] = Value::Null; } else if let Some((m, h)) = &look.back { a["back"] = json!({"main": m, "hi": h}); }
                ("set_look", a)
            }
        };
        self.rpc(method, args, window, cx, move |this, r, window, cx| {
            this.paint.saving = false;
            if let Err(e) = r { this.say(e, cx); }
            // the held values stay for a few seconds more, until the snapshot shows them
            let until = Instant::now() + Duration::from_secs(4);
            for v in this.paint.shown.values_mut() { v.1 = v.1.max(until); }
            for v in this.paint.looks.values_mut() { v.until = v.until.max(until); }
            this.snap_hash.clear(); this.tick(window, cx); cx.notify();
        });
    }

    // ---- drawing the panel
    /// The panel, over the right of the card column (mega.css `#mega-paint`).
    pub fn paint_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.paint.open || !matches!(self.level, Level::Home | Level::Server(_)) { return None; }
        let t = self.paint.target.clone()?;
        let paintable = self.can_paint(&t);
        let text = t.kind == Kind::Text;
        let back_mode = self.paint.back_mode && text;
        let cur = self.paint_cur();
        let lk = self.look_for(&t.cid, &Self::target_id(&t));
        let has_back = lk.back.is_some();
        let mute = look::mute();
        let paper = look::paper();
        let well = self.paint.well;
        let hsv = self.paint.hsv;
        let note = if t.kind == Kind::Options { "The options room is private to each person, so it stays grey.".to_string() }
            else if back_mode { if has_back { "These colours are only for the backdrop behind this chat.".into() } else { "The backdrop uses the card colours until you pick its own.".into() } }
            else { "Everyone in the community sees these colours. Only you, the owner, can change them.".to_string() };
        let tile_duo = Duo { main: cur.0, hi: cur.1 };
        let warn = if paintable && look::contrast(cur.0, cur.1) < 1.6 { "These two are too close, so the highlight will barely show." } else { "" };

        let head = h_flex().w_full().items_center().justify_between().gap_2()
            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().font_weight(FontWeight::SEMIBOLD).child(if t.kind == Kind::Options { "options".to_string() } else { t.name.clone() }))
            .child(chip("mp-done", "done").on_click(cx.listener(|this, _, _, cx| this.paint_close(cx))));
        let mut body = v_flex().w_full().gap(px(12.));
        body = body.child(div().text_size(px(13.)).line_height(px(18.2)).text_color(mute).child(note));
        if paintable {
            if text {
                let seg = |id: &'static str, label: &'static str, on: bool| div().id(id).flex_1().flex().items_center().justify_center().py(px(7.)).rounded_full().text_size(px(12.)).line_height(px(14.4)).cursor_pointer()
                    .when(on, |d| d.bg(paper).text_color(rgb(0x050407))).child(label);
                body = body.child(h_flex().w_full().gap(px(4.)).p(px(3.)).mb(px(8.)).rounded_full().bg(hsla(0., 0., 1., 0.06))
                    .child(seg("mp-card", "card", !back_mode).on_click(cx.listener(|this, _, window, cx| { this.paint.back_mode = false; this.paint_set_well(this.paint.well, window, cx); })))
                    .child(seg("mp-back", "chat backdrop", back_mode).on_click(cx.listener(|this, _, window, cx| { this.paint.back_mode = true; this.paint_set_well(this.paint.well, window, cx); }))));
            }
            let wellb = |i: usize, label: &'static str, c: u32| div().id(("mp-well", i)).flex_1().flex().items_center().gap(px(9.)).p(px(7.)).rounded(px(16.)).border_1().cursor_pointer().font_weight(FontWeight::MEDIUM)
                .border_color(if well == i { paper } else { hsla(0., 0., 0., 0.) }).bg(hsla(0., 0., 1., if well == i { 0.1 } else { 0.05 }))
                .child(div().size(px(30.)).flex_none().rounded(px(10.)).bg(rgb(c)).border_1().border_color(hsla(0., 0., 1., 0.22))).child(label)
                .on_click(cx.listener(move |this, _, window, cx| { this.paint.well = i; this.paint_resync(window, cx); cx.notify(); }));
            body = body.child(h_flex().w_full().gap(px(6.)).child(wellb(0, "colour", cur.0)).child(wellb(1, "highlight", cur.1)));
            // the square: the hue at full strength, white fading in from the left, black from the bottom
            let sv_bounds = self.paint.sv.clone();
            let hue_col = look::from_hsv(hsv[0], 1., 1.);
            let sv = div().id("mp-sv").relative().w_full().h(px(150.)).cursor_crosshair()
                .child(canvas({ let sv_bounds = sv_bounds.clone(); move |b, _, _| sv_bounds.set(b) }, move |b, _, window, _| {
                    sq::fill(window, b, 18., rgb(hue_col));
                    sq::fill(window, b, 18., linear_gradient(90., linear_color_stop(rgb(0xffffff), 0.), linear_color_stop(hsla(0., 0., 1., 0.), 1.)));
                    sq::fill(window, b, 18., linear_gradient(180., linear_color_stop(hsla(0., 0., 0., 0.), 0.), linear_color_stop(rgb(0x000000), 1.)));
                }).absolute().top_0().left_0().size_full())
                .child(div().absolute().left(relative(hsv[1])).top(relative(1. - hsv[2])).ml(px(-10.)).mt(px(-10.)).size(px(20.)).rounded_full().border_3().border_color(rgb(0xffffff)).bg(rgb(if well == 0 { cur.0 } else { cur.1 })))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| { this.paint.drag = Some(Drag::Sv); this.paint_drag_to(ev.position, window, cx); }));
            body = body.child(sv);
            let hue_bounds = self.paint.hue.clone();
            let hue = div().id("mp-hue").relative().w_full().h(px(16.)).mt(px(2.)).cursor_pointer()
                .child(canvas({ let hb = hue_bounds.clone(); move |b, _, _| hb.set(b) }, move |b, _, window, _| {
                    let stops = [0xff0000u32, 0xffff00, 0x00ff00, 0x00ffff, 0x0000ff, 0xff00ff, 0xff0000];
                    let (x, y, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
                    let seg = w / 6.;
                    let quad = |window: &mut Window, i: usize, l: f32, r: f32, round: bool| {
                        let bb = Bounds::new(point(px(x + l), px(y)), size(px(r - l), px(h)));
                        let g = linear_gradient(90., linear_color_stop(rgb(stops[i]), 0.), linear_color_stop(rgb(stops[i + 1]), 1.));
                        if round { sq::fill(window, bb, h / 2., g); } else { window.paint_quad(fill(bb, g)); }
                    };
                    quad(window, 0, 0., seg + h, true); quad(window, 5, seg * 5. - h, w, true);
                    for i in 1..5 { quad(window, i, seg * i as f32, seg * (i + 1) as f32, false); }
                }).absolute().top_0().left_0().size_full())
                .child(div().absolute().left(relative(hsv[0] / 359.)).top(px(-2.)).ml(px(-10.)).size(px(20.)).rounded_full().bg(rgb(0xffffff)).border_1().border_color(hsla(0., 0., 0., 0.5)))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| { this.paint.drag = Some(Drag::Hue); this.paint_drag_to(ev.position, window, cx); }));
            body = body.child(hue);
            body = body.child(h_flex().w_full().items_center().gap(px(10.)).mt(px(16.)).text_color(mute).child("hex")
                .child(div().flex_1().min_w_0().px(px(10.)).py(px(2.)).rounded(px(12.)).bg(hsla(0., 0., 1., 0.06)).border_1().border_color(look::line()).text_color(paper).font_family(look::F_MONO).text_size(px(14.))
                    .child(Input::new(&self.paint.hex).appearance(false))));
            let pre = look::presets();
            body = body.child(h_flex().w_full().gap(px(6.)).children(pre.iter().enumerate().map(|(i, (m, h))| {
                let (m, h) = (*m, *h);
                div().id(("mp-pre", i)).flex_1().mt(px(5.)).h(px((296. - 7. * 6.) / 8.)).rounded(px(10.)).cursor_pointer().overflow_hidden().border_1().border_color(hsla(0., 0., 1., 0.18))
                    .child(div().size_full().bg(linear_gradient(135., linear_color_stop(rgb(m), 0.5), linear_color_stop(rgb(h), 0.5))))
                    .on_click(cx.listener(move |this, _, window, cx| this.paint_preset(i, window, cx)))
            })));
            // the choosers: card animation for any card, chat animation and the backdrop for text rooms
            let label = |s: &'static str| div().text_size(px(12.)).line_height(px(16.8)).text_color(mute).child(s);
            body = body.child(v_flex().w_full().gap(px(8.))
                .child(label("card animation")).child(self.paint_tiles("card", lk.card, t.fx_auto, tile_duo, cx)));
            if text {
                let d = lk.dim.map(|v| v.min(DIM_MAX)).unwrap_or(100);
                body = body.child(v_flex().w_full().gap(px(8.)).child(label("chat animation")).child(self.paint_tiles("chat", lk.chat, t.fx_auto, tile_duo, cx)));
                let dim_bounds = self.paint.dim.clone();
                let frac = d as f32 / DIM_MAX as f32;
                body = body.child(h_flex().w_full().items_center().gap(px(10.))
                    .child(label("backdrop"))
                    .child(div().id("mp-dim").relative().flex_1().h(px(20.)).cursor_pointer()
                        .child(canvas({ let db = dim_bounds.clone(); move |b, _, _| db.set(b) }, move |b, _, window, _| {
                            let (x, y, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
                            let tr = Bounds::new(point(px(x), px(y + h / 2. - 2.)), size(px(w), px(4.)));
                            sq::fill(window, tr, 2., hsla(0., 0., 1., 0.2));
                            sq::fill(window, Bounds::new(tr.origin, size(px(w * frac), px(4.))), 2., rgb(look::PAPER));
                        }).absolute().top_0().left_0().size_full())
                        .child(div().absolute().left(relative(frac)).top(px(2.)).ml(px(-8.)).size(px(16.)).rounded_full().bg(paper))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| { this.paint.drag = Some(Drag::Dim); this.paint_drag_to(ev.position, window, cx); })))
                    .child(div().min_w(px(51.)).text_right().child(if d == 0 { "off".to_string() } else { format!("{d}%") })));
                let qh = self.paint.oracle.get(&(t.gid.clone(), false)).filter(|(_, u)| *u > Instant::now()).map(|(o, _)| *o).unwrap_or(lk.oracle_quiet);
                let dr = self.paint.oracle.get(&(t.gid.clone(), true)).filter(|(_, u)| *u > Instant::now()).map(|(o, _)| *o).unwrap_or(lk.oracle_draw);
                let sw = |id: &'static str, on: bool, label: &'static str| div().id(id).w_full().px(px(12.)).py(px(9.)).rounded(px(12.)).text_size(px(12.)).line_height(px(14.4)).cursor_pointer()
                    .when(on, |d| d.bg(paper).text_color(rgb(0x050407))).when(!on, |d| d.bg(hsla(0., 0., 1., 0.08))).child(label);
                body = body.child(v_flex().w_full().gap(px(8.))
                    .child(sw("mp-oracle", qh, if qh { "oracle stays quiet here" } else { "oracle can join in here" }).on_click(cx.listener(|this, _, window, cx| this.paint_oracle(false, window, cx))))
                    .child(sw("mp-odraw", dr, if dr { "oracle doodles on the backdrop" } else { "no oracle doodles" }).on_click(cx.listener(|this, _, window, cx| this.paint_oracle(true, window, cx)))));
            }
            body = body.child(div().min_h(px(18.)).text_size(px(13.)).text_color(mute).child(warn));
            body = body.child(h_flex().w_full().justify_end().gap(px(8.))
                .when(back_mode && has_back, |d| d.child(chip("mp-same", "same as card").on_click(cx.listener(|this, _, window, cx| this.paint_same(window, cx)))))
                .child(chip("mp-reset", "reset").on_click(cx.listener(|this, _, window, cx| this.paint_reset(window, cx)))));
        }
        let (bx, _) = (0., 0.);
        let _ = bx;
        Some(div().id("mp-panel").absolute().top(px(64.)).right(px(16.)).w(px(330.)).max_w_full().max_h(relative(1.)).occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| { cx.stop_propagation(); }))
            .child(div().id("mp-box").relative().w_full().max_h(px(self.nav.vp.get().1.max(300.) - 80.)).overflow_y_scroll().track_scroll(&self.paint.scroll).p(px(17.)).gap(px(12.)).flex().flex_col()
                .font_family(look::F_UI).text_size(px(14.)).line_height(px(19.6)).text_color(paper)
                .child(sq::back(30., rgb(0x15131b)))
                .child(head).child(body)
                .child(sq::front(30., None, Some((1., look::line())), None)))
            .into_any_element())
    }

    /// A row of animation tiles: "auto" and every pattern, each a small picture of the pattern in the colours being edited.
    fn paint_tiles(&mut self, which: &'static str, sel: Option<usize>, auto: usize, d: Duo, cx: &mut Context<Self>) -> impl IntoElement {
        let sel = sel.filter(|n| *n < ascii::FX_NAMES.len());
        let mut cells: Vec<(Option<usize>, &'static str)> = vec![(None, "auto")];
        for (i, n) in ascii::FX_NAMES.iter().enumerate() { cells.push((Some(i), *n)); }
        let rows: Vec<Vec<(Option<usize>, &'static str)>> = cells.chunks(4).map(|c| c.to_vec()).collect();
        v_flex().w_full().gap(px(6.)).children(rows.into_iter().enumerate().map(|(ri, row)| h_flex().w_full().gap(px(6.)).children((0..4).map(|ci| {
            match row.get(ci).copied() {
                None => div().flex_1().into_any_element(),
                Some((n, name)) => {
                    let on = n == sel;
                    let show = n.unwrap_or(auto);
                    div().id(SharedString::from(format!("tile-{which}-{ri}-{ci}"))).relative().flex_1().min_w_0().h(px(35.)).rounded(px(12.)).cursor_pointer().overflow_hidden()
                        .bg(rgb(d.main)).border_1().border_color(hsla(0., 0., 1., 0.2))
                        .when(on, |t| t.border_2().border_color(look::paper()))
                        .child(canvas(|_, _, _| (), move |b, _, window, _| thumb(window, b, show, d)).absolute().top_0().left_0().size_full())
                        .child(div().absolute().left(px(4.)).bottom(px(4.)).px(px(6.)).py(px(2.)).rounded_full().bg(rgba(0x08080cd1)).text_size(px(11.)).line_height(px(13.2)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(0xffffff)).child(name))
                        .on_click(cx.listener(move |this, _, _, cx| this.paint_pattern(which, n, cx)))
                        .into_any_element()
                }
            }
        }))))
    }
}

/// A tile's picture: the pattern at one frame, 22 x 8 cells (mega.js `fxThumb`).
fn thumb(window: &mut Window, b: Bounds<Pixels>, n: usize, d: Duo) {
    let ts = window.text_system().clone();
    let f = font(look::F_MONO);
    let fid = ts.resolve_font(&f);
    let (cols, rows) = (22usize, 8usize);
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let (cw, ch) = (w / cols as f32, h / rows as f32);
    let fs = (ch * 0.9).min(cw * 1.4);
    let col = ascii::cols_for(cols, 5);
    for y in 0..rows { for x in 0..cols {
        let v = ascii::fx(n, x as f64, y as f64, 7.3, cols as f64, rows as f64, &col);
        if v < 0.2 { continue; }
        let k = if v < 0.64 { 0.45 } else if v < 0.85 { 0.75 } else { 1. };
        let c: Hsla = rgb(look::mix(d.main, d.hi, k)).into();
        let ch_ = ascii::RAMP.chars().nth(((v * 10.) as usize).min(9)).unwrap_or(' ');
        if ch_ == ' ' { continue; }
        let s: SharedString = ch_.to_string().into();
        let run = TextRun { len: s.len(), font: f.clone(), color: c, background_color: None, underline: None, strikethrough: None };
        let line = ts.shape_line(s, px(fs), &[run], None);
        let Some(g) = line.runs.first().and_then(|r| r.glyphs.first()).map(|g| g.id) else { continue };
        let _ = window.paint_glyph(point(b.origin.x + px(x as f32 * cw + (cw - fs * 0.6) / 2.), b.origin.y + px(y as f32 * ch + ch * 0.76)), fid, g, px(fs), c);
    } }
}
