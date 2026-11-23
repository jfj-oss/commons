//! The rail beside a chat (app/ui/mega.js "the dots", mega.css `#mega-rail`): a narrow strip with a home dot and one dot per room
//! (a round dot for a text room, a ring for a voice room, a tall pill for the open room, glowing while unread). The wheel over it
//! beside a chat goes to the community's rooms (the card column) and scrolls there; elsewhere it walks the text rooms on the dots; the room it stops on opens after a short pause, so passing over
//! a room does not mark it read. While the pointer is over the rail every dot stretches to the right into a label with its room's
//! name; otherwise the dot the wheel is on and a room just opened do.
use crate::app::{Level, Mega};
use crate::ascii::{self, Field};
use crate::cards::{NavItem, Open};
use crate::look;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::Instant;

pub const CLOSED_W: f32 = 56.;
pub const OPEN_W: f32 = 210.;
/// The wheel must add up to this much before it steps a room, and steps come at least this far apart (seconds).
const STEP_PX: f32 = 50.;
const STEP_GAP: f64 = 0.14;
/// The room under the wheel opens after this long without another step.
const PICK_AFTER: f64 = 0.32;
const LEAVE_AFTER: f64 = 0.38;
/// A room just opened shows its name this long; a label grows out of its dot in this long.
const JUST_OPENED: f64 = 1.8;
const FLAG_GROW: f64 = 0.18;

/// The rail's state: how far it is open (0..1, a spring), the list's scroll while open, and the wheel's pick.
pub struct RailUi {
    pub open: bool,
    pub k: f32,
    pub hover: bool,
    leave_at: Option<f64>,
    pub off: f32,
    sum: f32,
    step_at: f64,
    pub pick: Option<usize>,
    pick_at: f64,
    last: Option<(String, f64)>,
    /** the dot under the pointer */
    pub over: Option<usize>,
    /** the room open now and when it became so (for its name label) */
    seen_gid: Option<String>,
    opened_at: f64,
    /** which dot wears the label, and since when (it grows out of the dot) */
    flag: Option<(String, f64)>,
    /** when the pointer came over the rail (all the labels grow from then) */
    hover_at: f64,
    /** the pointer is over one of the labels (they lie outside the rail's strip) */
    label_hover: bool,
}
impl Default for RailUi {
    fn default() -> Self { RailUi { open: false, k: 0., hover: false, leave_at: None, off: 0., sum: 0., step_at: -1., pick: None, pick_at: 0., last: None, over: None, seen_gid: None, opened_at: -10., flag: None, hover_at: 0., label_hover: false } }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

/// Geometry of the rail for `k` open (0 closed, 1 open): the width, the home dot, and for each item its top and size.
pub struct RailGeo { pub w: f32, pub home: (f32, f32, f32), pub items: Vec<(f32, f32, f32, f32)> }
/// `active` says which items are the open room (tall pill when closed). `height` is the rail's. `off` the open list's scroll.
pub fn geometry(n: usize, active: &[bool], height: f32, k: f32, off: f32) -> RailGeo {
    let w = lerp(CLOSED_W, OPEN_W, k);
    let home = (lerp(20., w - 28., k), lerp(20., 44., k), lerp(7., 22., k));
    let y0 = 24. + home.1;
    let gap = lerp(12., 10., k);
    let card_w = w - 28.;
    let hs: Vec<f32> = (0..n).map(|i| lerp(if active.get(i).copied().unwrap_or(false) { 38. } else { 14. }, card_w * 0.92, k)).collect();
    let total = hs.iter().sum::<f32>() + gap * n.saturating_sub(1) as f32 + 32.;
    let closed_start = y0 + ((height - y0 - total) / 2.).max(0.) + 16.;
    let open_start = y0 + 16. - off;
    let mut y = lerp(closed_start, open_start, k);
    let mut items = vec![];
    for h in hs { let iw = lerp(14., card_w, k); items.push(((w - iw) / 2., y, iw, h)); y += h + gap; }
    RailGeo { w, home, items }
}
/// How tall the open list is, for scrolling it.
pub fn open_content(n: usize) -> f32 { let h = (OPEN_W - 28.) * 0.92; n as f32 * h + 10. * n.saturating_sub(1) as f32 + 32. }
/// The scale of an open card by its distance from the middle of the list (`1 - .16 d`).
pub fn open_scale(center: f32, mid: f32, half: f32) -> f32 { 1. - 0.16 * ((center - mid).abs() / half.max(1.)).min(1.) }

impl Mega {
    fn rail_now(&self) -> f64 { self.epoch.elapsed().as_secs_f64() }
    pub fn rail_open(&mut self) { if !self.rail.open { self.rail.open = true;  } self.rail.leave_at = None; }
    pub fn rail_close_now(&mut self) { self.rail_close(); }
    fn rail_close(&mut self) { if self.rail.open { self.rail.open = false;  self.rail.off = 0.; self.rail.pick = None; self.rail.leave_at = None; } }

    /// The wheel over the rail beside a chat goes to the community's rooms (the card column, centred on this room, the dots
    /// flying out into the cards); the wheel then scrolls the cards there. Beside anything else it walks the text rooms on the dots.
    fn rail_wheel(&mut self, ev: &ScrollWheelEvent, items: &[NavItem], window: &mut Window, cx: &mut Context<Self>) {
        let now = self.rail_now();
        if self.level == Level::Chat {
            if let Some(cid) = self.cid.clone() { self.rail.over = None; self.rail.hover = false; self.go(Level::Server(cid), window, cx); }
            return;
        }
        let dy = match ev.delta { ScrollDelta::Pixels(p) => -f32::from(p.y), ScrollDelta::Lines(l) => -l.y * 100. };
        self.rail.sum += dy;
        if self.rail.sum.abs() < STEP_PX || now - self.rail.step_at < STEP_GAP { cx.notify(); return; }
        let step: i32 = if self.rail.sum > 0. { 1 } else { -1 };
        self.rail.sum = 0.; self.rail.step_at = now;
        let text: Vec<usize> = items.iter().enumerate().filter(|(_, x)| !x.voice && !x.locked && matches!(x.open, Open::Room(..))).map(|(i, _)| i).collect();
        if text.is_empty() { return; }
        let here = self.gid.as_ref().and_then(|g| text.iter().position(|&i| &items[i].room == g)).unwrap_or(0);
        let last = self.rail.last.as_ref().filter(|(_, t)| now - t < 4.).and_then(|(k, _)| text.iter().position(|&i| &items[i].key == k));
        let from = self.rail.pick.and_then(|p| text.iter().position(|&i| i == p)).or(last).unwrap_or(here);
        let next = (from as i32 + step).clamp(0, text.len() as i32 - 1) as usize;
        if next == from && self.rail.pick.is_none() && !self.rail.open { return; }
        if next == from && self.rail.pick.is_none() && self.rail.open { self.rail.off = (self.rail.off + step as f32 * 110.).clamp(0., (open_content(items.len()) - 200.).max(0.)); cx.notify(); return; }
        self.rail.pick = Some(text[next]); self.rail.pick_at = now;
        cx.notify();
    }

    /// Opens what the wheel stopped on, closes the rail after the pointer left, and keeps frames coming while any of it moves.
    fn rail_tick(&mut self, items: &[NavItem], window: &mut Window, cx: &mut Context<Self>) {
        let now = self.rail_now();
        if let Some(p) = self.rail.pick {
            if now - self.rail.pick_at >= PICK_AFTER {
                self.rail.pick = None;
                if let Some(it) = items.get(p) { self.rail.last = Some((it.key.clone(), now)); if let Open::Room(c, g) = it.open.clone() { if self.gid.as_deref() != Some(g.as_str()) { self.open_room(c, g, window, cx); } } }
            }
        }
        if let Some(t) = self.rail.leave_at { if now >= t && !self.rail.hover { self.rail_close(); } }
        if self.rail.seen_gid != self.gid { self.rail.seen_gid = self.gid.clone(); if self.gid.is_some() { self.rail.opened_at = now; } }
        let just = now - self.rail.opened_at < JUST_OPENED;
        let growing = self.rail.flag.as_ref().is_some_and(|(_, t)| now - t < FLAG_GROW) || (self.rail.hover && now - self.rail.hover_at < FLAG_GROW + 0.05);
        if just || growing { window.request_animation_frame(); }
        let want = if self.rail.open { 1. } else { 0. };
        // eases toward the target, a fifth of the way a frame (about half a second at 60 frames)
        if (self.rail.k - want).abs() > 1e-3 {
            self.rail.k += (want - self.rail.k) * 0.22;
            if (self.rail.k - want).abs() < 0.004 { self.rail.k = want; }
        }
        if self.still() { self.rail.k = want; }
        if (self.rail.k - want).abs() > 1e-3 || self.rail.pick.is_some() || self.rail.leave_at.is_some() { window.request_animation_frame(); }
    }

    pub fn render_rail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(c) = self.cid.as_ref().and_then(|cid| self.snap.as_ref()?.community(cid)).cloned() else { return div().w(px(CLOSED_W)).h_full().flex_none().bg(rgb(look::VOID)).into_any_element() };
        let mut items = self.room_items_for_rail(&c);
        if c.direct { items.retain(|x| matches!(x.open, Open::Room(..))); }
        self.rail_tick(&items, window, cx);
        let k = self.rail.k.clamp(0., 1.);
        let vh = f32::from(crate::titlebar::viewport(window).height);
        let active: Vec<bool> = items.iter().map(|x| matches!(&x.open, Open::Room(_, g) if self.gid.as_deref() == Some(g.as_str()))).collect();
        let g = geometry(items.len(), &active, vh, k, self.rail.off);
        let many = self.snap.as_ref().map(|s| s.communities.iter().filter(|c| !c.removed).count() > 1).unwrap_or(true) && !c.direct;
        let others = self.snap.as_ref().map(|s| s.communities.iter().any(|o| !o.removed && o.id != c.id && o.unread > 0)).unwrap_or(false);
        let still = self.still();
        let clock = self.clock;
        let (listh, mid) = (vh - (24. + g.home.1), 24. + g.home.1 + (vh - 24. - g.home.1) / 2.);
        let tour_hits = self.tour.hits.clone();
        let tour_on = self.tour.open;
        let mut kids: Vec<AnyElement> = vec![];
        // the home dot: a paper square that opens into a pill that says where it goes
        let (hw, hh, hr) = g.home;
        let home_label = if many { "all communities" } else { "all rooms" };
        let cid = c.id.clone();
        kids.push(div().id("rail-home").absolute().left(px((g.w - hw) / 2.)).top(px(24.)).w(px(hw)).h(px(hh)).cursor_pointer()
            .child(crate::squircle::back(hr, look::paper()))
            .child(div().absolute().inset_0().flex().items_center().justify_center().font_family(look::F_UI).text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::VOID)).opacity(((k - 0.4) / 0.6).clamp(0., 1.)).child(home_label))
            .when(others && k < 0.5, |d| d.child(div().absolute().top(px(-3.)).right(px(-3.)).size(px(9.)).rounded_full().bg(rgb(look::DANGER)).border_2().border_color(rgb(look::VOID))))
            .tooltip(move |window, cx| Tooltip::new(home_label).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| { if many { this.go(Level::Home, window, cx) } else { this.go(Level::Server(cid.clone()), window, cx) } }))
            .into_any_element());
        // names: every dot stretches into its name while the pointer is over the rail; otherwise the dot the wheel is on, or the room just opened
        let now = self.rail_now();
        let just = now - self.rail.opened_at < JUST_OPENED;
        let all = (self.rail.hover || self.rail.label_hover) && k < 0.05;
        let who: Vec<usize> = if k > 0.05 { vec![] } else if all { (0..items.len()).collect() } else {
            self.rail.pick.or_else(|| just.then(|| active.iter().position(|a| *a)).flatten()).into_iter().collect()
        };
        let one_since = match (who.first(), all) {
            (Some(&i), false) => { let key = items[i].key.clone(); Some(match &self.rail.flag { Some((k_, t)) if *k_ == key => *t, _ => { self.rail.flag = Some((key, now)); now } }) }
            _ => { self.rail.flag = None; None }
        };
        let since = if all { self.rail.hover_at } else { one_since.unwrap_or(now) };
        let t = if still { 1. } else { ((now - since) / FLAG_GROW).clamp(0., 1.) as f32 };
        let e = 1. - (1. - t) * (1. - t) * (1. - t);
        let over = self.rail.over;
        let mut reach = 0f32;
        for (i, it) in items.iter().enumerate() {
            let (x, y, w, h) = g.items[i];
            let on = active[i];
            let picked = self.rail.pick == Some(i);
            // the open list scales cards by their distance from its middle
            let sc = if k > 0. { let s = open_scale(y + h / 2., mid, listh / 2.); lerp(1., s, k) } else { 1. };
            // a mention beats the dot larger and rings it in the room's highlight; the open list gets a small badge instead
            let at_ring = it.at > 0 && it.num > 0 && !it.voice && !on && k < 0.5;
            let sc = if at_ring { sc * 1.14 } else { sc };
            let (cx_, cy_) = (x + w / 2., y + h / 2.);
            let (w2, h2) = (w * sc, h * sc);
            let (x2, y2) = (cx_ - w2 / 2., cy_ - h2 / 2.);
            if y2 + h2 < 0. || y2 > vh { continue; }
            let col = it.duo.dot();
            let unread = it.num > 0 && !it.voice && !on;
            let r = lerp(if it.title == "options" { 4. } else if on { 8. } else { 7. }, 62., k) * sc;
            let voice = it.voice && !it.locked;
            let open = it.open.clone();
            let keep = k > 0.01;
            // closed: a coloured dot; open: the room's own colour with a live field and its name
            let mesh = if keep { it.duo.field() } else { col };
            let live = k > 0.98 && !still && !it.locked;
            let f = (k > 0.98).then(|| {
                let seed = look::h(&it.key);
                let mut f = Field::card(it.fx, seed, it.duo, if still || it.hi_t > 1.5 { 11.7 } else { clock + (seed % 40) as f64 }, true);
                f.radius = r; f.hi_t = it.hi_t; f.emboss = still; if still { f.gate = 0.1; }
                f.holes = vec![Bounds::new(point(px(10.), px(h2 - 44.)), size(px(w2 - 20.), px(40.)))];
                f
            });
            let _ = live;
            let ink = it.duo.ink();
            let label_op = ((k - 0.4) / 0.6).clamp(0., 1.);
            // a named dot keeps its height, colour and corners and only grows to the right, with the name inside it
            let named = who.contains(&i) && !it.title.is_empty();
            let (w3, name) = if named {
                let fs = (h2 - 3.).clamp(9., 12.);
                let text = SharedString::from(if it.voice && !it.here.is_empty() { format!("{}  \u{00b7}  {}", it.title, it.here.join(", ")) } else { it.title.clone() });
                let run = TextRun { len: text.len(), font: font(look::F_UI), color: gpui_kit::black(), background_color: None, underline: None, strikethrough: None };
                let tw = f32::from(window.text_system().shape_line(text.clone(), px(fs), &[run], None).width);
                let full = (7. + tw + 8.).max(w2);
                reach = reach.max(x2 + full);
                (lerp(w2, full, e), Some((text, fs)))
            } else { (w2, None) };
            let lit = on || self.rail.over == Some(i) || picked;
            let name_ink: Hsla = if it.voice { look::paper() } else if col.l > 0.55 { rgb(0x0c0d12).into() } else { rgb(0xffffff).into() };
            let num_s = if it.num > 0 { if it.num > 99 { "99+".to_string() } else { it.num.to_string() } } else { String::new() };
            kids.push(div().id(("rail-item", i)).absolute().left(px(x2)).top(px(y2)).w(px(w3)).h(px(h2)).cursor_pointer()
                .opacity(if it.locked && k < 0.5 { 0.35 } else if k > 0.5 || on || unread || named { 1. } else { 0.68 })
                .child(div().absolute().inset_0().overflow_hidden()
                    .when(it.voice && k < 0.5, |d| d.child(crate::squircle::back(r, hsla(0., 0., 0., 0.))).child(div().absolute().inset_0().rounded(px(r.min(h2 / 2.))).border_2().border_color(col)))
                    .when(!(it.voice && k < 0.5), |d| d.child(crate::squircle::back(r, mesh)))
                    .when_some(f, |d, f| d.child(div().absolute().inset_0().child(ascii::field(f))))
                    .when(k > 0.02, |d| d.child(div().absolute().left(px(14.)).right(px(46.)).bottom(px(10.)).opacity(label_op).font_family(look::F_SHARP).text_size(px(24.)).line_height(px(22.)).max_h(px(44.)).text_color(ink).overflow_hidden().child(it.title.clone()))
                        .when(!num_s.is_empty(), |d| d.child(div().absolute().right(px(12.)).bottom(px(8.)).opacity(label_op).font_family(look::F_SHARP).text_size(px(36.)).line_height(px(30.)).text_color(ink).child(num_s.clone())))
                        .when(it.voice, |d| d.child(div().absolute().left(px(14.)).right(px(14.)).top(px(11.)).opacity(label_op).font_family(look::F_UI).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(ink).overflow_hidden().whitespace_nowrap().text_ellipsis().child(if it.here.is_empty() { "empty".to_string() } else { it.here.join(", ") }))))
                    .when_some(name, |d, (text, fs)| d.child(div().absolute().left(px(7.)).top_0().h_full().flex().items_center().whitespace_nowrap()
                        .opacity(((t - 0.35) / 0.65).clamp(0., 1.)).font_family(look::F_UI).text_size(px(fs)).font_weight(if lit || unread { FontWeight::BOLD } else { FontWeight::SEMIBOLD }).text_color(name_ink).child(text)))
                    .when(unread && k < 0.5, |d| d.child(div().absolute().inset_0().rounded_full().shadow(vec![BoxShadow { color: col.opacity(0.7), offset: point(px(0.), px(0.)), blur_radius: px(12.), spread_radius: px(0.), inset: false }]))))
                .when(tour_on && it.title == "options", |d| d.child(tour_hits.track("tour-opts")))
                .when(at_ring, |d| d.child(div().absolute().inset(px(-4.)).rounded_full().border_2().border_color(col)))
                .when(it.at > 0 && !on && k > 0.5, |d| d.child(div().absolute().top(px(-7.)).right(px(-9.)).min_w(px(15.)).h(px(15.)).px(px(3.)).rounded(px(8.)).bg(look::paper()).text_color(rgb(look::VOID)).font_family(look::F_UI).text_size(px(10.)).line_height(px(15.)).font_weight(FontWeight::BOLD).child(if it.at > 99 { "99+".to_string() } else { it.at.to_string() })))
                .when(on && k > 0.5, |d| d.child(div().absolute().inset(px(-4.)).rounded(px(r + 4.)).border_2().border_color(look::paper())))
                .when(picked, |d| d.child(div().absolute().inset(px(-4.)).rounded(px(r + 4.)).border_2().border_color(look::paper())))
                .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                    if *hovered { this.rail.over = Some(i); } else if this.rail.over == Some(i) { this.rail.over = None; }
                    cx.notify();
                }))
                .when_some(match &it.open { Open::Room(_, g) if !voice && !it.locked => Some((g.clone(), it.title.clone())), _ => None }, |d, (gid, name)| d.on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.room_menu_open(&gid, &name, ev.position, window, cx);
                })))
                .on_click(cx.listener(move |this, _, window, cx| {
                    match open.clone() {
                        // a click on a voice dot is an explicit join (or leave), as on the page: a voice room has no chat to open
                        Open::Room(_, g) if voice => { if this.call.in_call() && this.call.snap.gid.as_deref() == Some(g.as_str()) { this.leave_call(cx); } else { this.join_call(g, window, cx); } }
                        Open::Room(c, g) => { if this.gid.as_deref() != Some(g.as_str()) { this.open_room(c, g, window, cx); } }
                        Open::Settings => this.go(Level::Settings, window, cx),
                        Open::Admin(c) => this.open_panel(crate::members::Panel::Admin(c), window, cx),
                        Open::Server(c) => this.go(Level::Server(c), window, cx),
                    }
                }))
                .into_any_element());
        }
        // while the pointer is over it the rail widens to hold every stretched dot
        let wide = if all { lerp(g.w, (reach + 14.).max(g.w), e) } else { g.w };
        let shadow = if k > 0.02 { vec![BoxShadow { color: hsla(0., 0., 0., 0.6 * k), offset: point(px(0.), px(0.)), blur_radius: px(60.), spread_radius: px(0.), inset: false }] } else { vec![] };
        let items_for_wheel = items.clone();
        let panel = div().id("rail").absolute().left_0().top_0().w(px(wide)).h(px(vh)).when(k > 0.05, |d| d.overflow_hidden()).bg(rgba(if k > 0.5 { 0x08070bf0 } else { 0x08070be6 })).shadow(shadow)
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if *hovered && !this.rail.hover { this.rail.hover_at = this.rail_now(); }
                this.rail.hover = *hovered;
                if *hovered { this.rail.leave_at = None; } else if this.rail.open { this.rail.leave_at = Some(this.rail_now() + LEAVE_AFTER); cx.notify(); }
            }))
            .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, window, cx| { this.rail_wheel(ev, &items_for_wheel, window, cx); cx.stop_propagation(); }))
            .when(tour_on, |d| d.child(tour_hits.track("tour-rail")))
            .children(kids);
        // the strip keeps its closed width in the layout; the open rail floats over the chat
        div().relative().w(px(CLOSED_W)).h_full().flex_none().child(deferred(panel).with_priority(2))
            .into_any_element()
    }

    fn room_items_for_rail(&self, c: &crate::model::Community) -> Vec<NavItem> {
        let mut v = self.room_items(c);
        if c.direct { v.retain(|x| matches!(x.open, Open::Room(..))); }
        v
    }

    /// For the test port.
    pub fn rail_dump(&self) -> serde_json::Value {
        serde_json::json!({"open": self.rail.open, "k": self.rail.k, "off": self.rail.off, "pick": self.rail.pick, "hover": self.rail.hover, "over": self.rail.over, "flag": self.rail.flag.as_ref().map(|f| f.0.clone()), "gid": self.gid})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn closed_the_dots_are_a_centred_column_with_the_open_room_tall() {
        let g = geometry(4, &[false, true, false, false], 900., 0., 0.);
        assert_eq!(g.w, 56.);
        assert_eq!(g.home, (20., 20., 7.));
        let (x, _, w, h) = g.items[0];
        assert_eq!((w, h), (14., 14.));
        assert_eq!(x, 21., "centred in 56");
        assert_eq!(g.items[1].3, 38.);
        // 12 px apart
        assert!((g.items[1].1 - (g.items[0].1 + 14. + 12.)).abs() < 1e-3);
        // the column is centred in what is left under the home dot
        let first = g.items[0].1; let last = g.items[3].1 + 14.;
        let y0 = 24. + 20.;
        assert!(((first - 16.) - y0 - (900. - last - 16.)).abs() < 1e-2, "{first} {last}");
    }
    #[::core::prelude::v1::test]
    fn open_the_dots_become_cards_a_little_taller_than_wide() {
        let g = geometry(3, &[false; 3], 900., 1., 0.);
        assert_eq!(g.w, 210.);
        assert_eq!(g.home, (182., 44., 22.));
        let (x, y, w, h) = g.items[0];
        assert_eq!((x, w), (14., 182.));
        assert!((h - 182. * 0.92).abs() < 1e-3);
        assert!((y - (24. + 44. + 16.)).abs() < 1e-3, "the list starts under the home pill");
        assert!((g.items[1].1 - (y + h + 10.)).abs() < 1e-3);
        // scrolling moves them up
        assert!((geometry(3, &[false; 3], 900., 1., 110.).items[0].1 - (y - 110.)).abs() < 1e-3);
    }
    #[::core::prelude::v1::test]
    fn the_open_list_scales_by_distance_from_its_middle() {
        assert_eq!(open_scale(450., 450., 400.), 1.);
        assert!((open_scale(850., 450., 400.) - 0.84).abs() < 1e-6);
        assert!((open_scale(5000., 450., 400.) - 0.84).abs() < 1e-6, "never below .84");
    }
    #[::core::prelude::v1::test]
    fn halfway_open_is_halfway_between() {
        let g = geometry(2, &[false; 2], 900., 0.5, 0.);
        assert_eq!(g.w, 133.);
        assert_eq!(g.items[0].2, 14. + (105. - 14.) * 0.5);
    }
}
