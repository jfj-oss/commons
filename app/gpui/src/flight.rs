//! The card flight between levels (app/ui/mega.js `enterCommunity`, `closeIntoDots`, `openFromDots`, mega.css `.mfly`): a squircle overlay
//! per card, animated by hand over the window while the level underneath changes.
//!   * Expand   a community card opens up to fill the column (320 ms, cubic-bezier(.7,0,.2,1)); the level changes at the end of it and the
//!              overlay fades out (200 ms).
//!   * Close    opening a room: every visible card flies into its dot in the rail (460 ms, cubic-bezier(.76,0,.18,1), the chosen one 50 ms
//!              late), turning from the card's colour to the dot's, while the chat is already there.
//!   * Open     leaving a room for the community's rooms: the dots fly out into the cards (380 ms, cubic-bezier(.3,1.1,.3,1), which
//!              overshoots); the cards are held back until they land.
use crate::app::{Level, Mega};
use crate::cards::{NavItem, Open};
use crate::nav;
use crate::rail;
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind { Expand, Close, Open }

#[derive(Clone)]
pub struct Item { pub from: [f32; 4], pub to: [f32; 4], pub r0: f32, pub r1: f32, pub c0: Rgba, pub c1: Rgba, pub delay: f64 }

#[derive(Clone)]
pub struct Fly { pub kind: Kind, pub t0: f64, pub dur: f64, pub ease: [f32; 4], pub items: Vec<Item>, pub then: Option<Level>, pub fade_from: Option<f64> }

#[derive(Default)]
pub struct FlyUi { pub cur: Option<Fly>, pub count: u32 }

/// CSS `cubic-bezier(x1, y1, x2, y2)` at time `t` (0..1).
pub fn bezier(e: [f32; 4], t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    let b = |a: f32, c: f32, s: f32| 3. * a * s * (1. - s) * (1. - s) + 3. * c * s * s * (1. - s) + s * s * s;
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..28 { let m = (lo + hi) / 2.; if b(e[0], e[2], m) < t { lo = m } else { hi = m } }
    b(e[1], e[3], (lo + hi) / 2.)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }
fn mixc(a: Rgba, b: Rgba, t: f32) -> Rgba { Rgba { r: lerp(a.r, b.r, t), g: lerp(a.g, b.g, t), b: lerp(a.b, b.b, t), a: lerp(a.a, b.a, t) } }

impl Fly {
    /// Where each item is at `now`: its box, radius, colour. The chosen item's delay holds it at the start.
    pub fn frame(&self, now: f64) -> Vec<([f32; 4], f32, Rgba)> {
        self.items.iter().map(|it| {
            let p = (((now - self.t0 - it.delay) / self.dur) as f32).clamp(0., 1.);
            let e = bezier(self.ease, p);
            let rect = [lerp(it.from[0], it.to[0], e), lerp(it.from[1], it.to[1], e), lerp(it.from[2], it.to[2], e), lerp(it.from[3], it.to[3], e)];
            (rect, lerp(it.r0, it.r1, e.clamp(0., 1.)), mixc(it.c0, it.c1, e.clamp(0., 1.)))
        }).collect()
    }
    pub fn total(&self) -> f64 { self.dur + self.items.iter().map(|i| i.delay).fold(0., f64::max) }
}

/// Tests slow every flight down by this factor (MEGA_GPUI_FLY_SLOW), so screenshots can catch the middle of it.
fn slow() -> f64 { std::env::var("MEGA_GPUI_FLY_SLOW").ok().and_then(|v| v.parse().ok()).filter(|v: &f64| *v >= 1.).unwrap_or(1.) }

impl Mega {
    pub fn fly_active(&self) -> bool { self.fly.cur.is_some() }
    /// Cards are held back while the dots are still flying out to them.
    pub fn fly_hides_cards(&self) -> bool { self.fly.cur.as_ref().is_some_and(|f| f.kind == Kind::Open) }

    /// Window-space boxes of the cards a column of `items` would show with `start` in the middle (the layout of `render_cards`, at rest).
    fn card_boxes(&self, items: &[NavItem], start: usize, vw: f32, vh: f32) -> Vec<(usize, [f32; 4])> {
        let sections: Vec<bool> = items.iter().enumerate().map(|(i, x)| x.section.is_some() && (i == 0 || items[i - 1].section != x.section)).collect();
        let offs = nav::offsets(vh, &sections);
        let (cw, ch) = nav::card_box(vw, vh);
        let slot = nav::slot(vh);
        let o = self.nav.origin.get();
        let at = offs.get(start).copied().unwrap_or(0.);
        let mut out = vec![];
        for (i, _) in items.iter().enumerate() {
            let a = nav::dist(at, offs[i], slot);
            if !nav::shown(a) { continue; }
            let s = nav::scale(a) * nav::UNSETTLED;
            let cyy = vh / 2. + (offs[i] - at);
            out.push((i, [o.0 + (vw - cw * s) / 2., o.1 + cyy - ch * s / 2., cw * s, ch * s]));
        }
        out
    }
    fn column_size(&self, window: &Window) -> (f32, f32) {
        let (mw, mh) = self.nav.vp.get();
        if mw > 50. && mh > 50. { (mw, mh) } else { let s = window.viewport_size(); (f32::from(s.width), f32::from(s.height)) }
    }
    fn rail_boxes(&self, items: &[NavItem], active_gid: Option<&str>, vh: f32) -> Vec<[f32; 4]> {
        let active: Vec<bool> = items.iter().map(|x| matches!(&x.open, Open::Room(_, g) if Some(g.as_str()) == active_gid)).collect();
        let g = rail::geometry(items.len(), &active, vh, 0., 0.);
        let o = self.nav.origin.get();
        g.items.iter().map(|(x, y, w, h)| [o.0 + x, o.1 + y, *w, *h]).collect()
    }
    fn sqr(c: Hsla) -> Rgba { Rgba::from(c) }

    /// A community card was opened: it grows to fill the column, then `then` is shown.
    pub fn fly_expand(&mut self, key: &str, then: Level, window: &Window) -> bool {
        if self.still() { return false; }
        let Some((_, b, main, _)) = self.nav.rects.iter().find(|r| r.0 == key).cloned() else { return false };
        let (vw, vh) = self.column_size(window);
        let o = self.nav.origin.get();
        let now = self.epoch.elapsed().as_secs_f64();
        let c = Self::sqr(rgb(main).into());
        self.fly.cur = Some(Fly { kind: Kind::Expand, t0: now, dur: 0.32 * slow(), ease: [0.7, 0., 0.2, 1.], then: Some(then), fade_from: None,
            items: vec![Item { from: b, to: [o.0, o.1, vw, vh], r0: nav::SQ * (b[2] / nav::card_box(vw, vh).0.max(1.)), r1: 0., c0: c, c1: c, delay: 0. }] });
        self.fly.count += 1;
        true
    }

    /// A room was opened from the cards: they fly into the rail's dots (the rail is already there, as the chat is).
    pub fn fly_close(&mut self, cid: &str, gid: &str) {
        if self.still() || crate::phone::on() { return; }
        let Some(c) = self.snap.as_ref().and_then(|s| s.community(cid)).cloned() else { return };
        let mut items = self.room_items(&c);
        if c.direct { items.retain(|x| matches!(x.open, Open::Room(..))); }
        let vh = self.nav.vp.get().1.max(self.nav.vh).max(100.);
        let dots = self.rail_boxes(&items, Some(gid), vh);
        let now = self.epoch.elapsed().as_secs_f64();
        let mut out = vec![];
        for (key, b, main, dotc) in self.nav.rects.clone() {
            let Some(j) = items.iter().position(|x| x.key == key) else { continue };
            let Some(d) = dots.get(j) else { continue };
            let chosen = matches!(&items[j].open, Open::Room(_, g) if g == gid);
            out.push(Item { from: b, to: *d, r0: nav::SQ * 0.5, r1: (d[3].min(d[2]) / 2.).min(8.).max(d[2] / 2.), c0: Self::sqr(rgb(main).into()), c1: Self::sqr(rgb(dotc).into()), delay: if chosen { 0.05 * slow() } else { 0. } });
        }
        if out.is_empty() { return; }
        self.fly.cur = Some(Fly { kind: Kind::Close, t0: now, dur: 0.46 * slow(), ease: [0.76, 0., 0.18, 1.], items: out, then: None, fade_from: None });
        self.fly.count += 1;
    }

    /// Leaving a chat for the community's rooms: the dots fly out into the cards.
    pub fn fly_open(&mut self, cid: &str, window: &Window) {
        // no rail on a phone: nothing to fly out of
        if self.still() || crate::phone::on() { return; }
        let Some(c) = self.snap.as_ref().and_then(|s| s.community(cid)).cloned() else { return };
        let items = self.room_items(&c);
        let (vw, vh) = self.column_size(window);
        let start = self.gid.as_ref().and_then(|g| items.iter().position(|x| &x.room == g)).unwrap_or(0);
        let rail_items = { let mut v = self.room_items(&c); if c.direct { v.retain(|x| matches!(x.open, Open::Room(..))); } v };
        let dots = self.rail_boxes(&rail_items, self.gid.as_deref(), f32::from(window.viewport_size().height));
        let now = self.epoch.elapsed().as_secs_f64();
        let mut out = vec![];
        for (i, b) in self.card_boxes(&items, start, vw, vh) {
            let Some(j) = rail_items.iter().position(|x| x.key == items[i].key) else { continue };
            let Some(d) = dots.get(j) else { continue };
            out.push(Item { from: *d, to: b, r0: d[2] / 2., r1: nav::SQ * 0.9, c0: Self::sqr(items[i].duo.dot()), c1: Self::sqr(rgb(items[i].duo.main).into()), delay: 0. });
        }
        if out.is_empty() { return; }
        self.fly.cur = Some(Fly { kind: Kind::Open, t0: now, dur: 0.38 * slow(), ease: [0.3, 1.1, 0.3, 1.], items: out, then: None, fade_from: None });
        self.fly.count += 1;
    }

    /// Once a frame: ends a flight, or hands an Expand over to its level and starts the fade. Asks for the next frame while one is in the air.
    pub fn fly_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.fly.cur.clone() else { return };
        let now = self.epoch.elapsed().as_secs_f64();
        match f.kind {
            Kind::Expand if f.fade_from.is_none() => {
                if now - f.t0 >= f.total() {
                    if let Some(l) = f.then.clone() { self.go(l, window, cx); }
                    if let Some(c) = self.fly.cur.as_mut() { c.fade_from = Some(now); }
                }
            }
            Kind::Expand => { if now - f.fade_from.unwrap_or(now) >= 0.2 * slow() { self.fly.cur = None; } }
            _ => { if now - f.t0 >= f.total() { self.fly.cur = None; } }
        }
        if self.fly.cur.is_some() { window.request_animation_frame(); } else { cx.notify(); }
    }

    /// The overlay: one squircle per item at its place now.
    pub fn fly_layer(&self) -> Option<AnyElement> {
        let f = self.fly.cur.clone()?;
        let now = self.epoch.elapsed().as_secs_f64();
        let fade = f.fade_from.map(|t| (1. - ((now - t) / 0.2) as f32).clamp(0., 1.)).unwrap_or(1.);
        let frame = f.frame(if f.fade_from.is_some() { f.t0 + f.total() } else { now });
        Some(canvas(|_, _, _| (), move |b, _, window, _| {
            for (r, rad, c) in &frame {
                let col = Rgba { a: c.a * fade, ..*c };
                crate::squircle::fill(window, Bounds { origin: point(b.origin.x + px(r[0]), b.origin.y + px(r[1])), size: size(px(r[2].max(1.)), px(r[3].max(1.))) }, *rad, col);
            }
        }).absolute().top_0().left_0().size_full().into_any_element())
    }

    pub fn fly_dump(&self) -> serde_json::Value {
        serde_json::json!({"frame": self.fly.cur.as_ref().map(|f| f.frame(self.epoch.elapsed().as_secs_f64()).iter().take(2).map(|(r, rad, _)| serde_json::json!({"rect": r, "r": rad})).collect::<Vec<_>>()), "active": self.fly.cur.is_some(), "kind": self.fly.cur.as_ref().map(|f| format!("{:?}", f.kind)), "count": self.fly.count, "items": self.fly.cur.as_ref().map(|f| f.items.len())})
    }
}

#[cfg(test)]
mod tests {
    use super::bezier;
    #[test] fn the_curves_start_at_zero_and_end_at_one() { for e in [[0.7, 0., 0.2, 1.], [0.76, 0., 0.18, 1.], [0.3, 1.1, 0.3, 1.]] { assert!(bezier(e, 0.).abs() < 1e-3); assert!((bezier(e, 1.) - 1.).abs() < 1e-3); } }
    #[test] fn the_spring_overshoots_and_the_slow_one_waits() { assert!((0..100).any(|i| bezier([0.3, 1.1, 0.3, 1.], i as f32 / 100.) > 1.0)); assert!(bezier([0.7, 0., 0.2, 1.], 0.2) < 0.1); }
}
