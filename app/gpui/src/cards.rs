//! The two card levels, drawn the way the web draws them (app/ui/mega.js `List`, mega.css `.mlist`): ONE vertical column of
//! squircle cards. The card nearest the middle is focused (full size, its field moving, its words big), the others shrink and
//! fade with their distance. The rules are in nav.rs; this file lays the cards out and handles the wheel, the keys and clicks.
//!
//! GPUI cannot scale an element, so a card is laid out at its size on screen: every length of the full-size card (padding,
//! font sizes, the squircle's corner, the field's cells) is multiplied by the card's scale, which is what the page's
//! `transform: scale()` does to the same numbers.
use crate::app::{Level, Mega};
use crate::ascii::{self, Field};
use crate::look::{self, Duo};
use crate::nav::{self, Scroll};
use crate::squircle as sq;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

/// Bellefair's size here against the page's (see `nav_card`).
const BELLE: f32 = 1.3;

/// What opening a card does.
#[derive(Clone, PartialEq, Debug)]
pub enum Open { Server(String), Room(String, String), Settings, Admin(String) }

/// One card of the column.
#[derive(Clone)]
pub struct NavItem {
    /// The account whose picture the card shows (a direct conversation: the other person; "you": me), or empty.
    pub face: String,
    pub key: String,
    pub cid: String,
    pub room: String,
    pub title: String,
    pub sub: String,
    pub num: u64,
    /// Unread messages that mention me (alerts.rs); drawn as `@N`.
    pub at: u64,
    pub duo: Duo,
    pub fx: usize,
    /// The pattern the card would have if its owner had chosen none.
    pub auto: usize,
    pub locked: bool,
    pub voice: bool,
    /// Friends looking at this card right now (presence), and who is in its call.
    pub looking: Vec<String>,
    pub here: Vec<String>,
    pub hi_t: f64,
    /// The line the card shows once settled: the newest message (who, text), or a plain sentence.
    pub preview: Option<(String, String)>,
    pub note: Option<String>,
    /// A section label ("voice") that sits before this card.
    pub section: Option<&'static str>,
    pub open: Open,
    /// The "what's new" card: one row per unread room.
    pub news: Vec<NewsRow>,
}

#[derive(Clone)]
pub struct NewsRow { pub cid: String, pub gid: String, pub comm: String, pub room: String, pub n: u64, pub who: String, pub text: String, pub col: Hsla }

/// The newest line of a room, for its card (mega.js `PREV`).
#[derive(Clone, Default)]
pub struct Prev { pub sig: String, pub at: Option<Instant>, pub who: String, pub text: String, pub ts: u64 }

/// The column's state: where it is scrolled, what it holds, how big it is.
pub struct NavUi {
    pub scroll: Scroll,
    pub key: String,
    pub n: usize,
    pub vh: f32,
    /// The column's size as the last frame painted it, so the next lays out to it (the call bar takes some of the window).
    pub vp: Rc<Cell<(f32, f32)>>,
    pub offs: Vec<f32>,
    pub opens: Vec<Open>,
    pub voices: Vec<bool>,
    pub told: Option<usize>,
    pub focus: FocusHandle,
    /// The community last opened from the home column, so going back lands on it.
    pub last_home: Option<String>,
    pub acc: f32,
    pub previews: HashMap<String, Prev>,
    pub prev_busy: bool,
    /// Frame intervals of the column while it moved, in ms (for the scroll check in the test port).
    pub frames: Vec<f64>,
    pub last_frame: Option<Instant>,
    /// Tests: a synthetic scroll (until, pixels per second, direction), turned each frame like a wheel that never stops.
    pub play: Option<(f64, f32, f32)>,
    /// The card in focus as the colours panel and the draw chip see it, and where the column sits in the window.
    pub focus_card: Option<crate::paint::Target>,
    /// The glow under the pointer on the focused card: its cell and strength (ascii.rs `bloom`).
    pub bloom: (f64, f64, f64),
    pub origin: Rc<Cell<(f32, f32)>>,
}

impl NavUi {
    pub fn new(cx: &mut App) -> NavUi {
        NavUi { scroll: Scroll::default(), key: String::new(), n: 0, vh: 0., vp: Rc::new(Cell::new((0., 0.))), offs: vec![], opens: vec![], voices: vec![], told: None, focus: cx.focus_handle(), last_home: None, acc: 0.,
            previews: HashMap::new(), prev_busy: false, frames: vec![], last_frame: None, play: None, focus_card: None, bloom: (0., 0., 0.), origin: Rc::new(Cell::new((0., 0.))) }
    }
}

/// A pill like the page's `.mchip`: glass, a hairline, 36 px high.
pub fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).flex().flex_none().items_center().gap_2().h(px(36.)).px(px(14.)).rounded_full().bg(rgba(0x1a1820db)).border_1().border_color(look::line())
        .font_family(look::F_UI).text_size(px(14.)).text_color(look::paper()).cursor_pointer().whitespace_nowrap()
        .hover(|d| d.border_color(hsla(0., 0., 1., 0.34))).child(label.into())
}

impl Mega {
    /// The pattern a card shows: the owner's choice if it names one this version knows, else the automatic one (mega.js `patOf`).
    pub fn look_fx(&self, cid: &str, id: &str, auto: usize) -> usize { self.look_for(cid, id).card.filter(|n| *n < ascii::FX_NAMES.len()).unwrap_or(auto) }
    /// The rooms (or the people) a card names under it: "3 people", "caught up", "waiting for keys"...
    pub fn community_items_pub(&self) -> Vec<NavItem> { self.community_items() }
    fn community_items(&self) -> Vec<NavItem> {
        let snap = self.snap.clone().unwrap_or_default();
        let mut out = vec![];
        if let Some(n) = self.news_item() { out.push(n); }
        for c in snap.communities.iter().filter(|c| !c.removed) {
            let voice_ch = c.channels.iter().find(|x| x.kind == "voice" && self.pres.calls.get(&x.gid).is_some_and(|p| !p.is_empty()));
            let here: Vec<String> = voice_ch.and_then(|x| self.pres.calls.get(&x.gid)).cloned().unwrap_or_default();
            let mut pv: Option<&crate::cards::Prev> = None;
            for x in c.channels.iter().filter(|x| x.kind != "voice" && x.joined) { if let Some(p) = self.nav.previews.get(&x.gid).filter(|p| !p.text.is_empty()) { if pv.is_none_or(|q| p.ts > q.ts) { pv = Some(p); } } }
            let sub = if c.members > 0 { format!("{} {}", c.members, if c.members == 1 { "person" } else { "people" }) } else { format!("{} room{}", c.channels.len(), if c.channels.len() == 1 { "" } else { "s" }) };
            out.push(NavItem { face: String::new(), key: format!("c:{}", c.id), cid: c.id.clone(), room: String::new(), title: c.name.clone(), sub, num: c.unread, at: c.channels.iter().map(|x| crate::alerts::mention_of(&x.gid)).sum(), duo: self.duo_for(&c.id, c.colours.as_ref()), fx: self.look_fx(&c.id, &c.id, ascii::auto_fx(&c.name)), auto: ascii::auto_fx(&c.name),
                locked: false, voice: false, looking: self.people_on(&c.id, ""), here: here.clone(), hi_t: 0.9 - 0.22 * (c.unread.min(12) as f64 / 12.),
                preview: pv.map(|p| (p.who.clone(), p.text.clone())),
                note: voice_ch.filter(|_| !here.is_empty()).map(|x| format!("{} in {}", here.len(), x.name)),
                section: None, open: Open::Server(c.id.clone()), news: vec![] });
        }
        // one-to-one conversations come after the communities, named after the other person
        for d in &snap.direct {
            let on = d.channels.iter().any(|ch| ch.gid == d.room && ch.joined);
            let unread = d.channels.iter().map(|ch| ch.unread).sum::<u64>().max(d.unread).max(self.fake_dm_unread as u64);
            let duo = Duo::of(&d.room, d.room_colours.iter().find(|(g, _)| g == &d.room).map(|(_, x)| x));
            let pv = self.nav.previews.get(&d.room).filter(|p| !p.text.is_empty());
            out.push(NavItem { face: d.peer.clone(), key: format!("d:{}", d.id), cid: d.id.clone(), room: d.room.clone(), title: d.name.clone(), sub: if on { "direct".into() } else { "waiting for them".into() }, num: unread, at: 0, duo,
                fx: ascii::auto_fx(&d.name), auto: ascii::auto_fx(&d.name), locked: !on, voice: false, looking: vec![], here: vec![], hi_t: 0.9 - 0.22 * (unread.min(12) as f64 / 12.), preview: pv.map(|p| (p.who.clone(), p.text.clone())),
                note: None, section: None, open: Open::Room(d.id.clone(), d.room.clone()), news: vec![] });
        }
        // "you": the profile and the device's own options (the page's last card)
        out.push(NavItem { face: snap.me_account.clone(), key: "you".into(), cid: String::new(), room: String::new(), title: "you".into(), sub: "settings".into(), num: 0, at: 0, duo: Duo { main: 0xd9dce4, hi: 0x2b3bff }, fx: 4, auto: 4, locked: false, voice: false,
            looking: vec![], here: vec![], hi_t: 0.9, preview: None, note: Some("your profile and account".into()), section: None, open: Open::Settings, news: vec![] });
        out
    }

    /// The "what's new" card: the unread rooms of every community, newest first (mega-new.js `rowsNow`); nothing when everything is read.
    fn news_item(&self) -> Option<NavItem> {
        let snap = self.snap.as_ref()?;
        let mut rows: Vec<(u64, NewsRow)> = vec![];
        for c in snap.communities.iter().filter(|c| !c.removed) {
            for ch in c.channels.iter().filter(|x| x.kind != "voice" && x.joined && x.unread > 0) {
                let duo = self.duo_for(&ch.gid, c.room_colours.iter().find(|(g, _)| g == &ch.gid).map(|(_, d)| d));
                let p = self.nav.previews.get(&ch.gid);
                rows.push((p.map(|p| p.ts).unwrap_or(0), NewsRow { cid: c.id.clone(), gid: ch.gid.clone(), comm: c.name.clone(), room: ch.name.clone(), n: ch.unread, who: p.map(|p| p.who.clone()).unwrap_or_default(), text: p.map(|p| p.text.clone()).unwrap_or_default(), col: duo.dot() }));
            }
        }
        if rows.is_empty() { return None; }
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        let rows: Vec<NewsRow> = rows.into_iter().map(|r| r.1).collect();
        let total: u64 = rows.iter().map(|r| r.n).sum();
        let rows_at: u64 = rows.iter().map(|r| crate::alerts::mention_of(&r.gid)).sum();
        let first = rows[0].clone();
        Some(NavItem { face: String::new(), key: "news".into(), cid: String::new(), room: String::new(), title: "what\u{2019}s new".into(), sub: format!("{} unread room{}", rows.len(), if rows.len() == 1 { "" } else { "s" }), num: total, at: rows_at,
            duo: Duo { main: 0xff8a5c, hi: 0xffe45e }, fx: 12, auto: 12, locked: false, voice: false, looking: vec![], here: vec![], hi_t: 0.9 - 0.22 * (total.min(12) as f64 / 12.), preview: None, note: None, section: None,
            open: Open::Room(first.cid, first.gid), news: rows })
    }

    pub(crate) fn room_items(&self, c: &crate::model::Community) -> Vec<NavItem> {
        let off = look::h(&c.id) as usize;
        let mut out = vec![];
        let order: Vec<(usize, &crate::model::Channel)> = c.channels.iter().enumerate().filter(|(_, x)| x.kind != "voice").chain(c.channels.iter().enumerate().filter(|(_, x)| x.kind == "voice")).collect();
        for (i, ch) in order {
            let voice = ch.kind == "voice";
            let here = self.pres.calls.get(&ch.gid).cloned().unwrap_or_default();
            let duo = self.duo_for(&ch.gid, c.room_colours.iter().find(|(g, _)| g == &ch.gid).map(|(_, d)| d));
            let auto = if voice { 1 } else { [0, 3, 4, 5, 2][(off + i) % 5] };
            let fx = self.look_fx(&c.id, &ch.gid, auto);
            let pv = self.nav.previews.get(&ch.gid).filter(|p| !p.text.is_empty());
            let sub = if !ch.joined { "waiting for keys".to_string() } else if !ch.status.is_empty() && ch.status != "ok" { ch.status.clone() } else if voice { "voice".into() } else if ch.unread > 0 { "unread".into() } else { "caught up".into() };
            let sub = match self.room_mark(&ch.gid).filter(|_| !voice) { Some(m) => format!("{sub} \u{b7} {m}"), None => sub };
            let num = if voice { here.len() as u64 } else { ch.unread };
            let hi_t = if !ch.joined || (voice && here.is_empty()) { 2. } else if voice { 0.76 } else { 0.9 - 0.22 * (ch.unread.min(12) as f64 / 12.) };
            out.push(NavItem { face: String::new(), key: ch.gid.clone(), cid: c.id.clone(), room: ch.gid.clone(), title: ch.name.clone(), sub, num, at: if voice { 0 } else { crate::alerts::mention_of(&ch.gid) }, duo, fx, auto, locked: !ch.joined, voice,
                looking: self.people_on(&c.id, &ch.gid), here: here.clone(), hi_t, preview: if voice { None } else { pv.map(|p| (p.who.clone(), p.text.clone())) },
                note: if !ch.joined { Some("this device does not have the keys for this room yet".into()) } else if voice && here.is_empty() { Some("nobody here yet".into()) } else { None },
                section: if voice { Some("voice") } else { None }, open: Open::Room(c.id.clone(), ch.gid.clone()), news: vec![] });
        }
        if crate::members::is_admin(c) {
            out.push(NavItem { face: String::new(), key: format!("adm:{}", c.id), cid: c.id.clone(), room: String::new(), title: "server settings".into(), sub: "owners and admins".into(), num: 0, at: 0, duo: Duo::of(&format!("adm:{}", c.id), None), fx: 2, auto: 2, locked: false, voice: false,
                looking: vec![], here: vec![], hi_t: 0.9, preview: None, note: Some("name, rooms, members, invites. Everyone here sees what you change.".into()), section: None, open: Open::Admin(c.id.clone()), news: vec![] });
        }
        out.push(NavItem { face: String::new(), key: format!("opt:{}", c.id), cid: c.id.clone(), room: String::new(), title: "options".into(), sub: "only on this device".into(), num: 0, at: 0, duo: Duo { main: 0xa9adb8, hi: 0x4a4e5c }, fx: 6, auto: 6, locked: false, voice: false,
            looking: vec![], here: vec![], hi_t: 0.9, preview: None, note: Some("alerts, sound and look. Nobody else can see these.".into()), section: None, open: Open::Settings, news: vec![] });
        out
    }

    pub fn render_home(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let items = self.community_items();
        let start = self.nav.last_home.as_ref().and_then(|c| items.iter().position(|x| &x.cid == c && x.room.is_empty())).unwrap_or(0);
        let me = String::new();
        let bar = self.top_bar("mega".into(), None, cx).into_any_element();
        let _ = me;
        self.render_cards("home".into(), items, start, bar, true, window, cx)
    }

    pub fn render_server(&mut self, cid: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(c) = self.snap.as_ref().and_then(|s| s.community(cid)).cloned() else { return v_flex().size_full().child(self.top_bar("…".into(), Some(Level::Home), cx)).into_any_element() };
        let items = self.room_items(&c);
        let start = if self.cid.as_deref() == Some(cid) { self.gid.as_ref().and_then(|g| items.iter().position(|x| &x.room == g)).unwrap_or(0) } else { 0 };
        let many = self.snap.as_ref().map(|s| s.communities.iter().filter(|c| !c.removed).count() > 1).unwrap_or(true);
        let bar = self.top_bar(c.name.clone(), many.then_some(Level::Home), cx).into_any_element();
        self.render_cards(format!("server:{cid}"), items, start, bar, false, window, cx)
    }

    /// The column. `bar` floats over its top (the page's `.mbar`); notes about joining and updates float over its bottom (`.mnotes`).
    fn render_cards(&mut self, key: String, items: Vec<NavItem>, start: usize, bar: AnyElement, home: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = self.epoch.elapsed().as_secs_f64();
        let (mw, mh) = self.nav.vp.get();
        let (vw, vh) = if mw > 50. && mh > 50. { (mw, mh) } else { let s = crate::titlebar::viewport(window); (f32::from(s.width), f32::from(s.height)) };
        let sections: Vec<bool> = items.iter().enumerate().map(|(i, x)| x.section.is_some() && (i == 0 || items[i - 1].section != x.section)).collect();
        let offs = nav::offsets(vh, &sections);
        let still = self.still();
        if self.nav.key != key {
            self.nav.scroll = Scroll::start(start.min(items.len().saturating_sub(1)), &offs, now);
            self.nav.key = key.clone(); self.nav.acc = 0.; self.nav.told = None;
        } else if self.nav.n != items.len() || (self.nav.vh - vh).abs() > 0.5 {
            // the set of cards or the column's height changed: keep the card, jump to where it now sits (the page's ResizeObserver does `go(cur, true)`)
            let cur = self.nav.scroll.cur.min(items.len().saturating_sub(1));
            self.nav.scroll.go(cur, &offs, now, true);
        }
        self.nav.n = items.len(); self.nav.vh = vh;
        self.nav.scroll.calm = still;
        if let Some((until, speed, dir)) = self.nav.play {
            if now >= until { self.nav.play = None; } else {
                let dt = self.nav.last_frame.map(|t| t.elapsed().as_secs_f32().min(0.1)).unwrap_or(0.016);
                let max = offs.last().copied().unwrap_or(0.);
                let mut dir = dir;
                if (self.nav.scroll.offset >= max && dir > 0.) || (self.nav.scroll.offset <= 0. && dir < 0.) { dir = -dir; }
                self.nav.scroll.wheel_px(dir * speed * dt, &offs, now);
                self.nav.play = Some((until, speed, dir));
                window.request_animation_frame();
            }
        }
        let more = self.nav.scroll.tick(now, &offs);
        if more { window.request_animation_frame(); }
        // frame intervals while the column moves, for measuring
        if self.nav.scroll.moving() || self.nav.play.is_some() { if let Some(t) = self.nav.last_frame { self.nav.frames.push(t.elapsed().as_secs_f64() * 1000.); if self.nav.frames.len() > 4000 { self.nav.frames.remove(0); } } self.nav.last_frame = Some(Instant::now()); } else { self.nav.last_frame = None; }
        // the focused card tells friends where we are once it has settled
        let told = self.nav.scroll.settled.filter(|i| *i < items.len());
        if told != self.nav.told { self.nav.told = told; self.focus_card(told.map(|i| (items[i].cid.clone(), items[i].room.clone()))); }
        let sc = self.nav.scroll.clone();
        self.nav.offs = offs.clone();
        self.nav.opens = items.iter().map(|x| x.open.clone()).collect();
        self.nav.voices = items.iter().map(|x| x.voice && !x.locked).collect();
        // the card in focus: what the colours panel and the draw chip are about, and the wall under it
        let fc = items.get(sc.cur).and_then(|it| self.card_target(it));
        let moved = fc.as_ref().map(|t| &t.id) != self.nav.focus_card.as_ref().map(|t| &t.id);
        self.nav.focus_card = fc;
        if moved {
            cx.spawn_in(window, async move |this, cx| { let _ = this.update_in(cx, |this, window, cx| { this.paint_follow(window, cx); cx.notify(); }); }).detach();
        }

        let (cw, ch) = nav::card_box(vw, vh);
        let slot = nav::slot(vh);
        let mut cards: Vec<AnyElement> = vec![];
        let mut labels: Vec<AnyElement> = vec![];
        let clock = self.clock;
        let mut wall_spot = crate::wall::Spot::default();
        let mut wall_visible: Vec<String> = vec![];
        for (i, it) in items.iter().enumerate() {
            let a = nav::dist(sc.offset, offs[i], slot);
            if let Some(sec) = it.section.filter(|_| sections[i]) {
                let cy = vh / 2. + (offs[i] - sc.offset) - 0.35 * vh - 0.08 * vh;
                if cy > -80. && cy < vh + 80. {
                    let px_ = nav::snap_down((0.06 * vw).clamp(30., 70.), 12.);
                    labels.push(div().absolute().left_0().w(px(vw)).top(px(cy - px_ / 2.)).flex().justify_center().font_family(look::F_SHARP).text_size(px(px_)).line_height(px(px_)).text_color(look::mute()).child(sec).into_any_element());
                }
            }
            if !nav::shown(a) { continue; }
            let extra = if i == sc.focus { sc.grown() } else { nav::UNSETTLED };
            let s = nav::scale(a) * extra;
            let cyy = vh / 2. + (offs[i] - sc.offset);
            let geo = Geo { x: (vw - cw * s) / 2., y: cyy - ch * s / 2., cw, ch, s, op: nav::opacity(a), live: nav::live(a), focus: i == sc.cur, settled: sc.settled == Some(i), grow: if i == sc.focus { sc.grow } else { 0. }, t: clock };
            if i == sc.cur { let o = self.nav.origin.get(); wall_spot = crate::wall::Spot { x: o.0 + geo.x, y: o.1 + geo.y, w: cw, h: ch, s }; }
            if let Some(k) = card_wall(it) { wall_visible.push(k); }
            cards.push(self.nav_card(i, it, geo, cx));
        }
        let focus_wall = items.get(sc.cur).and_then(card_wall);
        self.wall.room_mode = false;
        self.wall_focus(focus_wall, wall_spot, wall_visible);
        // the dots down the left edge: one per card, the one in the middle taller (`.mscrub`)
        let n = items.len();
        let dots_h = n as f32 * 7. + (n.max(1) - 1) as f32 * 8. + 13.;
        let dots = v_flex().absolute().left(px(4.)).top(px(((vh - dots_h) / 2.).max(0.))).gap(px(8.)).max_h(px(vh * 0.8)).overflow_hidden()
            .children(items.iter().enumerate().map(|(i, it)| {
                let on = i == sc.cur;
                let col: Hsla = it.duo.dot();
                let unread = it.num > 0 && !it.voice;
                div().id(("dot", i)).relative().w(px(7.)).h(px(if on { 20. } else { 7. })).flex_none().rounded(px(if on { 4. } else { 3.5 })).cursor_pointer()
                    .bg(col).opacity(if on || unread { 1. } else { 0.4 })
                    .when(it.voice && !on, |d| d.bg(hsla(0., 0., 0., 0.)).border_1().border_color(col))
                    .on_click(cx.listener(move |this, _, _, cx| { this.nav_go(i); cx.notify(); }))
            }));

        let notes: Vec<AnyElement> = if home { let mut v = self.joining_notes(cx); v.extend(self.update_cards(cx)); v } else { vec![] };
        let measure = { let vp = self.nav.vp.clone(); let used = (vw, vh); let origin = self.nav.origin.clone();
            canvas(move |b, window, _| { origin.set((f32::from(b.origin.x), f32::from(b.origin.y))); let nw = (f32::from(b.size.width), f32::from(b.size.height)); if (vp.get().0 - nw.0).abs() > 0.5 || (vp.get().1 - nw.1).abs() > 0.5 { vp.set(nw); if (used.0 - nw.0).abs() > 0.5 || (used.1 - nw.1).abs() > 0.5 { window.request_animation_frame(); } } }, |_, _, _, _| {}).absolute().top_0().left_0().size_full() };
        div().id("cards").relative().size_full().overflow_hidden().track_focus(&self.nav.focus)
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, window, cx| this.nav_wheel(ev, window, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| { window.focus(&this.nav.focus, cx); }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.nav_key(ev, window, cx)))
            .child(measure)
            .children(labels)
            .children(cards)
            .child(dots)
            .child(div().absolute().top_0().left_0().right_0().child(bar))
            .children(self.wall_hit(cx))
            .children(self.wall_toolbar(cx))
            .children(self.paint_panel(false, cx))
            .when(!notes.is_empty(), |d| d.child(v_flex().absolute().bottom(px(12.)).left(px(16.)).right(px(16.)).gap(px(6.)).items_center().children(notes.into_iter().map(|n| div().w_full().max_w(px(640.)).child(n)))))
            .into_any_element()
    }

    /// One card at its size on screen.
    fn nav_card(&mut self, i: usize, it: &NavItem, g: Geo, cx: &mut Context<Self>) -> AnyElement {
        let s = g.s;
        let (w, h) = (g.cw * s, g.ch * s);
        let ink = it.duo.ink();
        let pad = nav::pad(g.cw) * s;
        let top_px = nav::q(nav::top_px(g.cw) * s);
        let tpx = nav::q(nav::title_px(g.cw) * s);
        let tlh = tpx * 0.9;
        let npx = nav::q(nav::number_px(g.cw) * s);
        let ppx = nav::q(nav::preview_px(g.cw) * s);
        // Bellefair in this text system sets about 1.3 times narrower than the page's at the same size (the chat uses 30 for the page's 24)
        let belle = ppx * BELLE;
        let r = nav::SQ * s;
        let inner_w = (w - 2. * pad).max(10.);
        let num_s = if it.num > 0 { if it.num > 99 { "99+".to_string() } else { it.num.to_string() } } else { String::new() };
        let num_w = if num_s.is_empty() { 0. } else { num_s.chars().count() as f32 * npx * 0.52 + 16. * s };
        let title_w = (inner_w - num_w).max(10.);
        // how the title wraps, for the pattern to flow around it (a pixel font's advance is about half its size)
        let adv = tpx * 0.52;
        let lines = wrap_lines(&it.title, adv, title_w);
        let title_h = lines as f32 * tlh;
        let still = self.still();
        let small = g.cw < 560.;
        let (cell, fs) = if small { ((8., 13.), 11.) } else { ((10., 17.), 14.) };
        // the words' boxes come from layout itself (ascii::hole), as the page cuts every text box out of the pattern
        let live: crate::ascii::Holes = Default::default();
        let show_mid = g.settled || g.grow > 0.02;
        let mid_op = g.grow.min(1.);
        let mid_text: Option<(String, String)> = it.preview.clone().or_else(|| it.note.clone().map(|n| (String::new(), n)));
        let wall_key = card_wall(it);
        let marks_cells: Rc<Vec<(u16, u8, u8)>> = Rc::new(wall_key.as_ref().map(|k| self.wall.marks_of(k)).unwrap_or_default());
        let wall_grid = crate::wall::grid_in(g.cw, g.ch, true).scaled(s);
        let f = if g.live {
            let seed = look::h(&it.key);
            let t = if still || it.hi_t > 1.5 { 11.7 } else { g.t + (seed % 40) as f64 };
            let mut f = Field::card(it.fx, seed, it.duo, t, small);
            f.cell = (cell.0 * s, cell.1 * s); f.font_px = nav::q(fs * s); f.radius = r; f.emboss = still; if still { f.gate = 0.1; }
            f.hi_t = it.hi_t; f.live_holes = Some(live.clone());
            if g.settled && self.nav.scroll.cur == i && self.nav.bloom.2 > 0.02 && !still { f.bloom = Some(self.nav.bloom); }
            if !marks_cells.is_empty() { f.avoid = Some((wall_grid, marks_cells.clone())); }
            Some(f)
        } else { None };
        let spot = sq::spot();
        let spot2 = spot.clone();
        let room_target = match &it.open { Open::Room(_, g) => Some((g.clone(), it.title.clone())), _ => None };
        let title_font = if g.settled { look::F_SHARP } else { look::F_BLEND };
        let looking: Vec<AnyElement> = it.looking.iter().take(crate::presence::SHOW).map(|n| crate::views::face_dot(n, 12. * s).into_any_element()).collect();
        let more = it.looking.len().saturating_sub(crate::presence::SHOW);
        let here_faces: Vec<AnyElement> = it.here.iter().take(5).map(|n| crate::views::face_dot(n, 22. * s).into_any_element()).collect();
        let top = h_flex().w_full().items_start().gap(px(12. * s)).font_family(look::F_UI).text_size(px(top_px)).line_height(px(top_px * 1.4)).text_color(ink)
            .child(div().relative().child(it.sub.clone()).child(ascii::hole(&live))).child(div().flex_1()).when(it.at > 0, |d| d.child(div().relative().flex().items_center().justify_center().flex_none().min_w(px(44. * s)).h(px(44. * s)).px(px(10. * s)).rounded_full().bg(ink).text_color(it.duo.field()).font_weight(FontWeight::SEMIBOLD).child(format!("@{}", if it.at > 99 { "99+".to_string() } else { it.at.to_string() })).child(ascii::hole(&live)))).when(!it.face.is_empty(), |d| d.child(div().relative().children(crate::avatars::pic_view(&it.face, 30. * s)).child(ascii::hole(&live)))).children(looking.into_iter().map(|f| div().relative().child(f).child(ascii::hole(&live)).into_any_element())).when(more > 0, |d| d.child(div().relative().child(format!("+{more}")).child(ascii::hole(&live))));
        // the "what's new" card holds a row per unread room instead of one line
        let news_rows: Vec<AnyElement> = if it.news.is_empty() || !show_mid { vec![] } else {
            let fit = (((h - 2. * pad - tpx * lines as f32 * 0.95 - top_px * 2.5) / ((56. + 8.) * s)).floor().max(1.)) as usize;
            it.news.iter().take(fit).enumerate().map(|(ri, r)| {
                let (cid, gid) = (r.cid.clone(), r.gid.clone());
                let name_px = nav::q(17. * s); let sm = nav::q(13. * s);
                h_flex().id(("news-row", ri)).w_full().flex_none().items_center().gap(px(14. * s)).min_h(px(56. * s)).px(px(16. * s)).py(px(9. * s)).rounded(px(20. * s)).bg(it.duo.field().opacity(0.8)).text_color(ink).cursor_pointer().relative().child(ascii::hole(&live))
                    .hover(|d| d.bg(it.duo.field().opacity(0.96)))
                    .child(div().size(px(12. * s)).flex_none().rounded_full().bg(r.col).border_2().border_color(ink))
                    .child(v_flex().flex_1().min_w_0().gap(px(2. * s))
                        .child(h_flex().items_baseline().gap(px(8. * s)).min_w_0().overflow_hidden().whitespace_nowrap()
                            .child(div().flex_none().max_w(px(w * 0.3)).overflow_hidden().text_ellipsis().font_family(look::F_NAME).text_size(px(sm)).opacity(0.7).child(r.comm.clone()))
                            .child(div().min_w_0().overflow_hidden().text_ellipsis().font_family(look::F_NAME).text_size(px(name_px)).font_weight(FontWeight::SEMIBOLD).child(r.room.clone())))
                        .when(!r.text.is_empty(), |d| d.child(h_flex().items_baseline().gap(px(6. * s)).overflow_hidden().whitespace_nowrap().opacity(0.88)
                            .child(div().flex_none().font_family(look::F_UI).text_size(px(sm)).font_weight(FontWeight::SEMIBOLD).child(r.who.clone()))
                            .child(div().min_w_0().overflow_hidden().text_ellipsis().font_family(look::F_BELLEFAIR).text_size(px(nav::q(15. * s * BELLE))).child(r.text.replace('\n', " "))))))
                    .child(div().flex_none().font_family(look::F_SHARP).text_size(px(nav::q(24. * s))).child(r.n.to_string()))
                    .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.open_room(cid.clone(), gid.clone(), window, cx); }))
                    .into_any_element()
            }).collect()
        };
        let mid = v_flex().flex_1().min_h_0().justify_center().gap(px(12. * s)).opacity(if show_mid { mid_op } else { 0. })
            .when(!it.here.is_empty() && it.voice, |d| d.child(h_flex().relative().child(ascii::hole(&live)).gap(px(10. * s)).items_center().font_family(look::F_SHARP).text_size(px(nav::snap_down((0.03 * g.cw).clamp(22., 42.), 12.) * s)).text_color(ink)
                .children(here_faces).child(it.here.join(", "))))
            .when(!it.here.is_empty() && !it.voice, |d| d.child(h_flex().relative().child(ascii::hole(&live)).gap(px(10. * s)).items_center().font_family(look::F_UI).text_size(px(top_px * 1.2)).text_color(ink)
                .children(it.here.iter().take(5).map(|n| crate::views::face_dot(n, 22. * s).into_any_element())).child(it.note.clone().unwrap_or_default())))
            .when_some(mid_text.clone().filter(|_| !(it.voice && !it.here.is_empty()) && !(!it.here.is_empty() && !it.voice)), |d, (who, text)| d.child(
                h_flex().relative().child(ascii::hole(&live)).max_w(px(ppx * 34.)).items_baseline().gap(px(ppx * 0.3)).text_color(ink).overflow_hidden().max_h(px(ppx * 1.25 * 3.2))
                    .when(!who.is_empty(), |d| d.child(div().flex_none().font_family(look::F_UI).text_size(px(ppx * 0.82)).line_height(px(ppx * 1.25)).font_weight(FontWeight::SEMIBOLD).child(who.clone())))
                    .child(div().min_w_0().font_family(look::F_BELLEFAIR).text_size(px(belle)).line_height(px(ppx * 1.25)).child(text.replace('\n', " ")))));
        // a voice card has its own join button (a click on its body does nothing; Enter joins), like the page's `.mjoin`
        let me_in = it.voice && self.call.in_call() && self.call.snap.gid.as_deref() == Some(it.room.as_str());
        let mid = if it.voice && !it.locked && show_mid {
            let gid = it.room.clone();
            mid.child(h_flex().child(div().id(("join", i)).relative().child(ascii::hole(&live)).flex().items_center().justify_center().h(px(44. * s)).px(px(22. * s)).rounded_full().bg(ink).text_color(it.duo.field()).font_family(look::F_UI).text_size(px(nav::q(15. * s))).font_weight(FontWeight::SEMIBOLD).cursor_pointer()
                .child(if me_in { "leave" } else { "join" })
                .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); if this.call.in_call() && this.call.snap.gid.as_deref() == Some(gid.as_str()) { this.leave_call(cx); } else { this.join_call(gid.clone(), window, cx); } }))))
        } else { mid };
        let mid = if news_rows.is_empty() { mid } else { v_flex().flex_1().min_h_0().gap(px(8. * s)).opacity(mid_op).children(news_rows) };
        let bottom = h_flex().w_full().items_end().gap(px(16. * s)).text_color(ink)
            .child(v_flex().flex_1().min_w_0().items_start().child(div().relative().font_family(title_font).text_size(px(tpx)).line_height(px(tlh)).child(it.title.clone()).child(ascii::hole(&live))))
            .when(!num_s.is_empty(), |d| d.child(div().relative().flex_none().font_family(look::F_SHARP).text_size(px(npx)).line_height(px(npx * 0.78)).opacity(0.85).pr(px(npx * 0.08)).child(num_s.clone()).child(ascii::hole(&live))));
        let slab = div().absolute().inset_0().overflow_hidden().bg(it.duo.field()).opacity(if it.locked { g.op * 0.55 } else { g.op })
            .when_some(f, |d, f| d.child(div().absolute().inset_0().child(ascii::field(f))))
            .when(!marks_cells.is_empty(), |d| d.child(crate::wall::marks(crate::wall::Marks { cells: marks_cells.clone(), grid: wall_grid, ink: look::wall_ink(&it.duo), holes: Rc::new(vec![]), live: Some((live.clone(), nav::q(cell.0 * s), true)) })))
            .child(v_flex().absolute().inset_0().p(px(pad)).gap(px(12. * s)).child(top).child(mid).child(bottom));
        let cur = self.nav.scroll.cur;
        let tour_hits = self.tour.hits.clone();
        div().id(("card", i)).absolute().left(px(g.x)).top(px(g.y)).w(px(w)).h(px(h)).cursor_pointer()
            .when(i == cur && g.settled, |d| { let o = self.nav.origin.clone(); let (cw_, ch_, gx, gy) = (cell.0 * s, cell.1 * s, g.x, g.y);
                d.on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| { let p = o.get(); this.nav.bloom = (((f32::from(ev.position.x) - p.0 - gx) / cw_) as f64, ((f32::from(ev.position.y) - p.1 - gy) / ch_) as f64, 1.); cx.notify(); })) })
            .when(self.tour.open && i == cur, |d| d.child(tour_hits.track("tour-card")))
            .when(self.tour.open && i == cur + 1, |d| d.child(tour_hits.track("tour-card2")))
            .child(slab)
            .child(sq::front(r, Some(rgb(look::VOID).into()), None, Some(spot)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                if !sq::clicked(&spot2, ev, r) { return; }
                this.nav_click(i, window, cx);
            }))
            // a right-click on a room: its notification menu (mute, mentions only)
            .when_some(room_target.filter(|_| !it.voice && !it.locked), |d, (gid, name)| d.on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.room_menu_open(&gid, &name, ev.position, window, cx);
            })))
            .into_any_element()
    }

    // ---- input ----
    fn nav_now(&self) -> f64 { self.epoch.elapsed().as_secs_f64() }
    pub fn nav_go(&mut self, i: usize) { if self.wall.on { return; } let now = self.nav_now(); let offs = self.nav.offs.clone(); self.nav.scroll.go(i, &offs, now, false); }
    pub fn nav_step(&mut self, d: i32) { let now = self.nav_now(); let offs = self.nav.offs.clone(); self.nav.scroll.step(d, &offs, now); }
    pub fn nav_wheel(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.wall.on { cx.stop_propagation(); return; }
        let now = self.nav_now();
        let offs = self.nav.offs.clone();
        match ev.delta {
            ScrollDelta::Pixels(p) => { let dy = -f32::from(p.y); if dy != 0. { self.nav.scroll.wheel_px(dy, &offs, now); } }
            ScrollDelta::Lines(l) => {
                let dir = crate::nav::wheel_notch(&mut self.nav.acc, -l.y);
                if dir != 0 { self.nav.scroll.step(dir, &offs, now); }
            }
        }
        cx.stop_propagation(); cx.notify();
    }
    pub fn nav_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.mem.panel.is_some() { return; }
        let k = ev.keystroke.key.as_str();
        // drawing: Escape ends it (mega.js), and the column stays where it is
        if self.wall.on { if k == "escape" { self.wall_off(); cx.stop_propagation(); cx.notify(); } return; }
        let m = &ev.keystroke.modifiers;
        if m.control || m.alt || m.platform { return; }
        let now = self.nav_now(); let offs = self.nav.offs.clone();
        match k {
            "down" | "j" => self.nav.scroll.step(1, &offs, now),
            "up" | "k" => self.nav.scroll.step(-1, &offs, now),
            "pagedown" => self.nav.scroll.step(3, &offs, now),
            "pageup" => self.nav.scroll.step(-3, &offs, now),
            "home" => self.nav.scroll.go(0, &offs, now, false),
            "end" => self.nav.scroll.go(offs.len().saturating_sub(1), &offs, now, false),
            "enter" => { let i = self.nav.scroll.target(&offs); self.nav_open(i, window, cx); }
            "escape" | "backspace" => { if matches!(self.level, Level::Server(_)) && self.snap.as_ref().map(|s| s.communities.iter().filter(|c| !c.removed).count() > 1).unwrap_or(true) { self.go(Level::Home, window, cx); } }
            _ => return,
        }
        cx.stop_propagation(); cx.notify();
    }
    /// A click on card `i`: another card is scrolled to; the focused one, once settled, opens.
    pub fn nav_click(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.nav.focus, cx);
        if i != self.nav.scroll.cur { self.nav_go(i); cx.notify(); return; }
        if self.nav.scroll.settled != Some(i) || self.nav.voices.get(i).copied().unwrap_or(false) { return; }
        self.nav_open(i, window, cx);
    }
    pub fn nav_open(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(o) = self.nav.opens.get(i).cloned() else { return };
        // Enter on a voice card is an explicit join; its body is never joined by a click
        if self.nav.voices.get(i).copied().unwrap_or(false) {
            if let Open::Room(_, gid) = o { if !self.call.in_call() || self.call.snap.gid.as_deref() != Some(gid.as_str()) { self.join_call(gid, window, cx); } }
            return;
        }
        match o {
            Open::Server(cid) => self.go(Level::Server(cid), window, cx),
            Open::Room(cid, gid) => self.open_room(cid, gid, window, cx),
            Open::Settings => self.go(Level::Settings, window, cx),
            Open::Admin(cid) => self.open_panel(crate::members::Panel::Admin(cid), window, cx),
        }
    }
    /// A column that is showing keeps the keyboard when nothing else has it.
    pub fn nav_focus_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.level, Level::Home | Level::Server(_)) && self.mem.panel.is_none() && window.focused(cx).is_none() { window.focus(&self.nav.focus, cx); }
    }

    /// What the column holds, for the test port.
    pub fn nav_dump(&self) -> serde_json::Value {
        let sc = &self.nav.scroll;
        let (w, h) = self.nav.vp.get();
        let mut fr = self.nav.frames.clone();
        let sum = crate::bench::summary(&{ fr.sort_by(|a, b| a.partial_cmp(b).unwrap()); fr.clone() });
        json!({"level": format!("{:?}", self.level), "key": self.nav.key, "n": self.nav.n, "cur": sc.cur, "settled": sc.settled, "offset": sc.offset, "grow": sc.grow, "moving": sc.moving(), "offs": self.nav.offs, "vp": [w, h],
            "opens": self.nav.opens.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>(), "frames": sum})
    }

    // ---- the newest line of each room, for the cards (mega.js `prevOf`, `homePreviews`) ----
    pub fn preview_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav.prev_busy || !matches!(self.level, Level::Home | Level::Server(_)) { return; }
        let Some(snap) = self.snap.clone() else { return };
        let mut rooms: Vec<(String, String)> = vec![];   // (gid, signature)
        let comms: Vec<&crate::model::Community> = match &self.level {
            Level::Server(cid) => snap.communities.iter().filter(|c| &c.id == cid).collect(),
            _ => snap.communities.iter().filter(|c| !c.removed).chain(snap.direct.iter()).collect(),
        };
        for c in comms { for ch in c.channels.iter().filter(|x| x.kind != "voice" && x.joined) { rooms.push((ch.gid.clone(), format!("{}:{}", ch.unread, ch.status))); } }
        let ttl = if matches!(self.level, Level::Server(_)) { 4. } else { 30. };
        let mut picks: Vec<(String, String)> = rooms.into_iter().filter(|(g, sig)| self.nav.previews.get(g).is_none_or(|p| &p.sig != sig || p.at.is_none_or(|t| t.elapsed().as_secs_f64() > ttl))).collect();
        picks.truncate(4);
        if picks.is_empty() { return; }
        self.nav.prev_busy = true;
        let core = self.core.clone();
        cx.spawn_in(window, async move |this, cx| {
            let job = picks.clone();
            let out = core.look(move |app| job.into_iter().map(|(gid, sig)| {
                let r = app.call("messages", json!({"gid": gid, "limit": 8, "around": ""}));
                (gid, sig, r)
            }).collect::<Vec<_>>()).await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.nav.prev_busy = false;
                let Ok(out) = out else { return };
                let mut changed = false;
                for (gid, sig, r) in out {
                    let mut p = Prev { sig, at: Some(Instant::now()), ..Default::default() };
                    if let Ok(r) = r {
                        let last = r["messages"].as_array().and_then(|l| l.iter().rev().find(|m| m["deleted"].as_bool() != Some(true) && !matches!(m["kind"].as_str(), Some("notice") | Some("member"))).cloned());
                        if let Some(m) = last {
                            p.who = m["name"].as_str().unwrap_or("").to_string();
                            p.text = m["text"].as_str().filter(|t| !t.is_empty()).map(String::from).or_else(|| m["file"]["name"].as_str().map(String::from)).unwrap_or_default();
                            p.ts = m["ts"].as_u64().unwrap_or(0);
                        }
                    }
                    let old = this.nav.previews.get(&gid);
                    if old.is_none_or(|o| o.who != p.who || o.text != p.text) { changed = true; }
                    this.nav.previews.insert(gid, p);
                }
                if changed { cx.notify(); }
            });
        }).detach();
    }
}

/// The wall a card carries (mega-wall.js `keyOf`): the community card's own, or a room's; nothing for the other cards.
pub fn card_wall(it: &NavItem) -> Option<String> {
    if it.key.starts_with("c:") { return Some(crate::wall::cw_key(&it.cid)); }
    if !it.room.is_empty() && it.key == it.room && !it.locked { return Some(it.room.clone()); }
    None
}

impl Mega {
    /// What the colours panel is about when this card is the focused one.
    pub fn card_target(&self, it: &NavItem) -> Option<crate::paint::Target> {
        use crate::paint::{Kind, Target};
        let (kind, id, gid) = if it.key.starts_with("c:") { (Kind::Community, it.cid.clone(), String::new()) }
            else if it.key.starts_with("opt:") { (Kind::Options, it.key.clone(), String::new()) }
            else if !it.room.is_empty() && it.key == it.room { (if it.voice { Kind::Voice } else { Kind::Text }, it.room.clone(), it.room.clone()) }
            else { return None };
        Some(Target { id, cid: it.cid.clone(), gid, kind, name: it.title.clone(), fx_auto: it.auto, duo: it.duo })
    }
}

/// A card as it is on screen.
pub struct Geo { pub x: f32, pub y: f32, pub cw: f32, pub ch: f32, pub s: f32, pub op: f32, pub live: bool, pub focus: bool, pub settled: bool, pub grow: f32, pub t: f64 }

/// How many lines a title takes at this advance in this width (words wrap; a word longer than the line breaks).
pub fn wrap_lines(text: &str, adv: f32, width: f32) -> usize {
    if adv <= 0. || width <= 0. { return 1; }
    let per = (width / adv).floor().max(1.) as usize;
    let mut lines = 1; let mut cur = 0usize;
    for word in text.split_whitespace() {
        let n = word.chars().count();
        let need = if cur == 0 { n } else { cur + 1 + n };
        if need <= per { cur = need; } else if cur == 0 { lines += (n - 1) / per; cur = n % per; if cur == 0 { cur = per; } } else { lines += 1; cur = n.min(per); if n > per { lines += (n - 1) / per; cur = n % per; if cur == 0 { cur = per; } } }
    }
    lines
}
/// The widest line of the same wrapping, in pixels.
pub fn lines_w(text: &str, adv: f32, width: f32) -> f32 {
    if adv <= 0. { return 0.; }
    let per = (width / adv).floor().max(1.) as usize;
    let mut best = 0usize; let mut cur = 0usize;
    for word in text.split_whitespace() {
        let n = word.chars().count().min(per);
        let need = if cur == 0 { n } else { cur + 1 + n };
        if need <= per { cur = need; } else { cur = n; }
        best = best.max(cur);
    }
    best as f32 * adv
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn titles_wrap_by_words() {
        assert_eq!(wrap_lines("Test club", 10., 1000.), 1);
        assert_eq!(wrap_lines("Test club", 10., 60.), 2, "six letters to a line: Test / club");
        assert_eq!(wrap_lines("extraordinarily", 10., 60.), 3, "a word longer than the line breaks");
        assert_eq!(wrap_lines("", 10., 60.), 1);
        assert_eq!(lines_w("Test club", 10., 60.), 40.);
    }
}
