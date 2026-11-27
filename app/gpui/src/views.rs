//! What is on screen: the community cards, a community's room cards, a room beside its colour dots, and settings.
use crate::app::{clock, Level, Mega, Screen};
use crate::ascii::{self, hole, Holes};
use crate::look::{self, Duo};
use crate::cards::chip;
use crate::model::Msg;
use crate::prefs::{ChatFont, Saver};
use crate::squircle as sq;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, Paste, Textarea};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::rc::Rc;

/// Corner of a card (mega.css --m-sq is 76 px on the web's big cards, a true squircle, which reads rounder at the same number than a circle does).
const SQ: f32 = 60.;

/// The padlock before the room's small print (a 9 by 12 drawing).
fn lock_icon() -> Div {
    v_flex().items_center().flex_none().child(div().w(px(6.)).h(px(5.)).border_1().border_b_0().border_color(look::mute()).rounded_t(px(3.))).child(div().w(px(9.)).h(px(6.)).rounded(px(1.5)).bg(look::mute()))
}

pub(crate) fn face_dot(name: &str, size: f32) -> Div {
    div().size(px(size)).flex_none().rounded_full().bg(look::face(name)).border_1().border_color(hsla(0., 0., 0., 0.35))
}
/// A person's picture where they have one, else their letter on the seal's squircle.
pub(crate) fn avatar(account: &str, name: &str, size: f32) -> Div {
    if let Some(p) = crate::avatars::pic_view(account, size) { return p; }
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    div().relative().size(px(size)).flex_none().text_color(rgb(0x0c0d12)).font_family(look::F_SHARP).text_size(px(size * 24. / 36.)).line_height(px(size))
        .flex().items_center().justify_center().child(sq::back(11., look::face(name))).child(div().relative().child(initial))
}

impl Mega {
    /// The page's `.mbar`: the mark (or the community's name) at the left, a back chip before it, chips at the right.
    pub(crate) fn top_bar(&mut self, title: String, back: Option<Level>, cx: &mut Context<Self>) -> impl IntoElement {
        let net = self.snap.as_ref().map(|s| s.net.clone()).unwrap_or_default();
        let net_detail = self.snap.as_ref().map(|s| s.net_detail.clone()).unwrap_or_default();
        let mut extra: Vec<AnyElement> = vec![];
        // the colours and draw chips belong to the focused card (the page's `[data-m="paint"]` and `[data-m="draw"]`)
        if matches!(self.level, Level::Home | Level::Server(_)) {
            if self.nav.focus_card.as_ref().is_some_and(|t| self.can_paint(t)) {
                extra.push(chip("b-paint", "colours").on_click(cx.listener(|this, _, window, cx| this.paint_toggle(window, cx))).into_any_element());
            }
            if self.wall.key.as_ref().is_some_and(|k| self.wall_drawable(k)) {
                extra.push(chip("b-draw", if self.wall.on { "drawing" } else { "draw" }).relative().child(self.tour.hits.track("tour-draw")).on_click(cx.listener(|this, _, window, cx| this.wall_toggle(window, cx))).into_any_element());
            }
        }
        extra.extend(match self.level.clone() { Level::Server(cid) => self.community_buttons(&cid, cx), Level::Home => self.home_buttons(cx), _ => vec![] });
        let phone = crate::phone::on();
        // a phone: the name, then settings at the right; the other chips on a row of their own below
        let settings = chip("settings", "settings").on_click(cx.listener(|this, _, window, cx| this.go(Level::Settings, window, cx)));
        let (settings_first, settings_last) = if phone { (Some(settings), None) } else { (None, Some(settings)) };
        // a phone folds the other chips into a + at the right of the bar: a menu that floats over the page, never over the card's top
        let more = phone && !extra.is_empty();
        let (extra_top, extra_menu) = if more { (vec![], std::mem::take(&mut extra)) } else { (std::mem::take(&mut extra), vec![]) };
        let open = more && self.phone_more;
        // the chip sets the state from what was drawn (a tap on the open menu's × also counts as a tap outside, which closes it first)
        let plus = more.then(|| {
            div().relative()
                .child(chip("phone-more", if open { "\u{00d7}" } else { "+" }).min_w(px(44.)).on_click(cx.listener(move |this, _, _, cx| { this.phone_more = !open; cx.notify(); })))
                // pinned to the button's bottom-right corner: the menu drops straight down from the +, right edges aligned
                .when(open, |d| d.child(div().absolute().right_0().bottom_0().size_0().child(deferred(
                    anchored().anchor(Anchor::TopRight).snap_to_window().child(
                        v_flex().id("phone-more-menu").mt(px(6.)).p(px(8.)).gap(px(8.)).items_end().rounded(px(18.)).bg(hsla(0., 0., 0.06, 0.96)).border_1().border_color(hsla(0., 0., 1., 0.12))
                            .children(extra_menu)
                            // any choice closes the menu (the chip's own click runs first); a tap outside closes it too
                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| { this.phone_more = false; cx.notify(); }))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| { this.phone_more = false; cx.notify(); })),
                    ),
                ).with_priority(3))))
        });
        let top = h_flex().flex_none().w_full().px(px(if phone { 12. } else { 16. })).py(px(if phone { 8. } else { 12. })).gap(px(10.)).items_center()
            .when_some(back, |d, to| d.child(chip("back", "\u{2190} back").on_click(cx.listener(move |this, _, window, cx| this.go(to.clone(), window, cx)))))
            .child(div().font_family(look::F_SHARP).text_size(px(if phone { 30. } else { 36. })).line_height(px(if phone { 34. } else { 36. })).max_w(px(if phone { 300. } else { 560. })).overflow_hidden().text_ellipsis().whitespace_nowrap().child(title))
            .when(net != "online", |d| d.child(div().id("net-state").font_family(look::F_UI).text_size(px(13.)).text_color(rgb(look::DANGER)).max_w(px(420.)).overflow_hidden().text_ellipsis().whitespace_nowrap()
                .child(if net == "connecting" { "connecting…".to_string() } else if net_detail.is_empty() { "offline".to_string() } else { format!("offline: {net_detail}") })
                .when(!net_detail.is_empty(), |d| { let t = net_detail.clone(); d.tooltip(move |window, cx| Tooltip::new(t.clone()).build(window, cx)) })))
            .when(self.still(), |d| d.child(div().font_family(look::F_UI).text_size(px(13.)).text_color(look::mute()).child("battery saver")))
            .child(div().flex_1())
            .children(settings_first)
            .children(plus)
            .children(extra_top)
            .children(settings_last);
        v_flex().flex_none().w_full().child(top)
    }

    /// An owner's colours for the open room: a chip that opens the colours panel (square, hue, hex, presets, reset) on this room.
    fn colour_picker(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        chip("preset-open", "colours").on_click(cx.listener(|this, _, window, cx| {
            if this.paint.open { this.paint_close(cx); return; }
            let (Some(cid), Some(gid)) = (this.cid.clone(), this.gid.clone()) else { return };
            this.paint_open_for(&cid, &gid, window, cx);
        }))
    }

    /// Where the visible rows were drawn last time, taken before the list lays out (it cannot be asked while it does): each row's top
    /// and height against the room's top, the room's size and how many pixels are left to scroll.
    fn reading_look(&mut self) {
        let vp = self.list.viewport_bounds();
        let remaining = f32::from(self.list.max_offset_for_scrollbar().y) + f32::from(self.list.scroll_px_offset_for_scrollbar().y);
        let g = &mut self.reading.geo;
        g.clear();
        // at the end of the room the list's top item reads as one past the last message: look back from there for the rows on screen
        let n = self.win.msgs.len();
        let start = self.list.logical_scroll_top().item_ix.min(n);
        for ix in start.saturating_sub(40)..n {
            let Some(b) = self.list.bounds_for_item(ix) else { continue };
            if b.bottom() < vp.top() - px(10.) { continue; }
            if b.top() > vp.bottom() + px(10.) { break; }
            g.insert(ix, (f32::from(b.top() - vp.top()), f32::from(b.size.height)));
        }
        // the rows painted last frame fill in what the list didn't report
        for (ix, (top, h)) in self.reading.painted.borrow_mut().drain() {
            if ix < n && top + h > f32::from(vp.top()) - 10. && top < f32::from(vp.bottom()) + 10. { g.entry(ix).or_insert((top - f32::from(vp.top()), h)); }
        }
        self.reading.size = (f32::from(vp.size.width), f32::from(vp.size.height));
        self.reading.top = f32::from(vp.top());
        // the first time the room is laid out with its unread line far up: that line a third of the way down, as the web does
        if let Some(ix) = self.new_place { if vp.size.height > px(0.) {
            self.new_place = None;
            self.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
            self.list.scroll_by(-vp.size.height * 0.3);
        } }
        self.reading.remaining = remaining;
    }

    /// For the test port: the rows drawn last time with their scale, the room's height and how far it is from its end.
    pub fn reading_dump(&self) -> Value {
        let (vh, remaining) = (self.reading.size.1, self.reading.remaining);
        let mut rows: Vec<_> = self.reading.last.iter().filter(|(ix, _)| self.reading.geo.get(*ix).is_some_and(|(t, h)| t + h > 0. && *t < vh))
            .map(|(ix, (k, op))| json!({"ix": ix, "k": k, "opacity": op, "top": self.reading.geo.get(ix).map(|g| g.0)})).collect();
        rows.sort_by_key(|r| r["ix"].as_u64());
        json!({"height": vh, "width": self.reading.size.0, "remaining": remaining, "msgs": self.win.msgs.len(), "rows": rows})
    }

    fn render_msg(&mut self, ix: usize) -> AnyElement {
        let Some(m) = self.win.msgs.get(ix) else { return div().into_any_element() };
        let prev = ix.checked_sub(1).and_then(|i| self.win.msgs.get(i));
        let star = self.dm_marks.0 == self.gid.clone().unwrap_or_default() && self.dm_marks.1 > 0 && ix + self.dm_marks.1 >= self.win.msgs.len() && !m.mine;
        let me_name_owned = self.snap.as_ref().map(|s| s.me_name.clone()).unwrap_or_default();
        let me_name = me_name_owned.as_str();
        let me = me_name;
        let now = self.now_secs();
        let today = chrono::Local::now();
        let day = (m.ts > 0 && prev.map_or(true, |p| p.ts == 0 || day_words(p.ts, today) != day_words(m.ts, today))).then(|| day_words(m.ts, today));
        let new = self.new_line.as_ref().filter(|(id, _)| *id == m.id).map(|(_, n)| *n);
        // The reading zone (reading.rs): where this row was last drawn tells how far it is from the middle of the room. A row drawn smaller
        // keeps the box it has at full size (so the list does not move under the reader) and sits in the middle of it, as the web's
        // `transform: scale()` from the left centre does. Not while the battery saver holds still, nor at the end of the room.
        let ((width, vh), remaining) = (self.reading.size, self.reading.remaining);
        if (self.reading.width - width).abs() > 0.5 { self.reading.width = width; self.reading.full.clear(); self.reading.scaled.clear(); }
        let geo = self.reading.geo.get(&ix).copied();
        let mut k = 1.;
        let mut op = 1.;
        let mut full = None;
        if let Some((top, height)) = geo {
            let was_scaled = self.reading.scaled.contains(&m.id);
            if !was_scaled { self.reading.full.insert(m.id.clone(), height); }
            let whole = crate::select::keeps_whole(ix, self.sel.hover.as_ref().and_then(|h| self.win.msgs.iter().position(|x| &x.id == h)), self.sel.now);
            if !self.still() && self.reading.on && !whole {
                if let (Some((sc, o)), Some(h)) = (crate::reading::effect(top, top + height, vh, remaining), self.reading.full.get(&m.id)) { k = sc; op = o; full = Some(*h); }
            }
        }
        self.reading.last.insert(ix, (k, op));
        // where this row's middle was painted last frame (the list's own positions are missing at the end of a room)
        let fringe = match self.reading.mids.borrow().get(&m.id) { Some(&mid) if vh > 1. && !self.still() => (1. - (mid - self.reading.top) / (vh / 3.)).clamp(0., 1.), _ => 0. };
        if full.is_some() { self.reading.scaled.insert(m.id.clone()); } else { self.reading.scaled.remove(&m.id); }
        let dens = self.dens_params();
        let fx = RowFx {
            dens,
            tier_px: match &dens { Some(d) => match crate::arrive::size_tier(m, me) { 0 => None, t => Some(d.tier[t as usize - 1]) }, None => crate::arrive::tier_px(crate::arrive::size_tier(m, me)) },
            arrive: self.arrivals.progress(&m.id, now).map(|k| (k, (now * 60.) as u64)),
            hi: rgb(self.room_duo(self.gid.as_deref().unwrap_or("")).hi).into(),
            flights: self.flights.live.iter().filter(|f| f.msg == m.id).cloned().collect(),
            now, k, day, new, pick: self.sel.now.and_then(|s| crate::select::range_in(s, ix, m.shown().len())), texts: self.sel.texts.clone(), file: self.file_ctx(m).map(|mut c| { c.k = k; c }),
            act: self.files.me.clone().map(|me| {
                let src = m.reply.as_ref().and_then(|r| self.win.msgs.iter().find(|x| x.id == r.id));
                crate::msgact::ActCtx { me, hits: self.files.hits.clone(), can_edit: m.mine && m.kind == "text" && !m.deleted, can_delete: (m.mine || self.is_admin_here()) && !m.deleted, flash: self.flash_of(&m.id, now), mention: !m.mine && !m.deleted && (crate::arrive::mentions(&m.text, me_name) || src.is_some_and(|x| x.mine)),
                    quote_gone: src.is_some_and(|x| x.deleted), older: ix == 0 && self.win.more, hi: rgb(self.room_duo(self.gid.as_deref().unwrap_or("")).hi).into() }
            }),
            fringe
        };
        let el = row(m, prev, self.chat_font(), &self.holes, star, &fx);
        // The box is at least the row's full-size height (so the list does not move under the reader) but never less than the row
        // needs: `full` was noted the last time the row was drawn whole, and a row can grow after that (the unread line above it, a
        // reaction, a wrap). A fixed height clipped such a row at its top and bottom (2026-11-25).
        let el = match full { Some(h) => div().w_full().min_h(px(h)).flex().flex_col().justify_center().opacity(op).child(el).into_any_element(), None => el };
        // note where this row's slot is painted: next frame's colour fringe, and the reading zone's scale where the list can't say
        // (at the very end of a room it gives no positions)
        let (mids, painted, mid_id) = (self.reading.mids.clone(), self.reading.painted.clone(), m.id.clone());
        div().relative().w_full().child(el)
            .child(canvas(move |b, _, _| { mids.borrow_mut().insert(mid_id.clone(), f32::from(b.center().y)); painted.borrow_mut().insert(ix, (f32::from(b.top()), f32::from(b.size.height))); }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .into_any_element()
    }

    /// The room's header chips (the web's search, aa, draw, people...): the ones that fit, never wrapping or cut off; the least important go first.
    fn room_bar(&mut self, gid: &str, is_dm: bool, owner: bool, wall_btn: Option<AnyElement>, chat_cid: Option<String>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let chip_w = |label: &str| 32. + label.chars().count() as f32 * 7.8;
        // (importance, place from the left, width, the chip): lower importance goes first
        let mut items: Vec<(u8, u8, f32, AnyElement)> = vec![];
        if let Some(b) = wall_btn { items.push((1, 1, chip_w("drawing"), b)); }
        if is_dm { if let Some(b) = self.call_person_button(gid, cx) { items.push((0, 0, 120., b)); } }
        if !is_dm {
            for (i, b) in chat_cid.map(|cid| self.community_buttons(&cid, cx)).unwrap_or_default().into_iter().enumerate() {
                items.push(([0, 3, 4][i.min(2)], 10 + i as u8, chip_w(["people", "invite", "manage"][i.min(2)]), b));
            }
        }
        items.push((1, 4, chip_w("search"), self.search_chip(cx)));
        items.push((2, 2, chip_w("aa"), crate::cards::chip("b-aa", "aa").on_click(cx.listener(|this, _, _, cx| {
            this.prefs.chat_font = match this.prefs.chat_font { ChatFont::Bellefair => ChatFont::Raster, ChatFont::Raster => ChatFont::Bellefair }; this.prefs.save(); this.list.remeasure(); this.reading.full.clear(); cx.notify();
        })).into_any_element()));
        items.push((3, 3, chip_w("density"), crate::cards::chip("b-dens", "density").on_click(cx.listener(|this, _, window, cx| this.dens_toggle(window, cx))).into_any_element()));
        if owner { items.push((5, 20, 96., self.colour_picker(cx).into_any_element())); }
        // everything sits behind one + pill; open, the chips fall out under it in their places, and any pick closes it
        items.sort_by_key(|i| i.1);
        let open = self.msg.bar_open;
        let plus = div().id("room-plus").flex().flex_none().items_center().justify_center().size(px(34.)).rounded_full().cursor_pointer()
            .bg(if open { hsla(0., 0., 1., 0.16) } else { hsla(0.71, 0.22, 0.07, 0.93) }).border_1().border_color(look::line())
            .hover(|d| d.bg(hsla(0., 0., 1., 0.12)))
            .child(crate::icons::icon(if open { crate::icons::Icon::Close } else { crate::icons::Icon::Plus }, 16., if open { look::paper() } else { look::mute() }))
            .on_click(cx.listener(|this, _, _, cx| { this.msg.bar_open = !this.msg.bar_open; cx.notify(); }));
        vec![div().relative().flex_none()
            .when(open, |d| d.on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, _, cx| { if this.msg.bar_box.get().is_some_and(|b| b.contains(&ev.position)) { return; } this.msg.bar_open = false; cx.notify(); })))
            .child(plus)
            .children(open.then(|| v_flex().id("room-plus-menu").absolute().top(px(42.)).right_0().items_end().gap(px(6.))
                .children(items.into_iter().map(|i| i.3))
                .child({ let bx = self.msg.bar_box.clone(); canvas(move |b, _, _| bx.set(Some(b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full() })
                .on_click(cx.listener(|this, _, _, cx| { this.msg.bar_open = false; cx.notify(); }))))
            .into_any_element()]
    }

    fn render_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(gid) = self.gid.clone() else { return div().flex_1().into_any_element() };
        let found = self.channel(&gid).map(|(c, ch)| (if c.direct { c.name.clone() } else { ch.name.clone() }, if c.direct { "just the two of you".to_string() } else { format!("{} · {} people", c.name, c.members) }));
        let Some((title, _sub)) = found else { return v_flex().flex_1().h_full().items_center().justify_center().text_color(look::mute()).child("This room is gone.").into_any_element() };
        let duo = self.room_duo(&gid);
        let is_dm = self.channel(&gid).is_some_and(|(c, _)| c.direct);
        let owner = self.channel(&gid).is_some_and(|(c, _)| !c.direct && (c.role == "owner" || c.role == "admin"));
        let chat_cid = self.channel(&gid).map(|(c, _)| c.id.clone());
        let want = self.typing_sync(&gid);
        let typing_strip = self.typing_strip(&want, duo, &self.holes.clone());
        let (font, size, lh) = self.chat_font();
        // The room's backdrop: the room's ASCII field in the owner's chat pattern, colours and strength, dim behind the chat (backdrop.rs).
        // The owner's own backdrop colours, else the room's; the chat pattern, else the room's automatic one; strength 100 percent unless set.
        let (cid_look, auto) = match self.channel(&gid).filter(|(c, _)| !c.direct) {
            Some((c, _)) => { let i = c.channels.iter().position(|x| x.gid == gid).unwrap_or(0); (Some(c.id.clone()), [0, 3, 4, 5, 2][(look::h(&c.id) as usize + i) % 5]) }
            None => (None, ascii::auto_fx(&title)),
        };
        let lk = cid_look.as_ref().map(|c| self.look_for(c, &gid)).unwrap_or_default();
        let bd_duo = lk.back.clone().and_then(|(m, h)| Some(Duo { main: look::parse_hex(&m)?, hi: look::parse_hex(&h)? })).unwrap_or(duo);
        let bd_dim = self.bd_dim(&gid);
        let bd_fx = lk.chat.filter(|n| *n < ascii::FX_NAMES.len()).unwrap_or(auto);
        let marks = Rc::new(self.wall.marks_of(&gid));
        let (rw, rh) = { let r = self.wall.room_rect.get(); (r.2.max(1.), r.3.max(1.)) };
        let avoid = (!marks.is_empty()).then(|| (crate::wall::grid_in(rw, rh, false), marks));
        let still = self.still();
        let bd_back = crate::backdrop::wash(bd_duo.main, crate::backdrop::lv(bd_dim));
        let backdrop = (bd_dim > 0).then(|| ascii::field(crate::backdrop::field(&gid, bd_fx, bd_duo, bd_dim, &self.bd, still, &self.holes, avoid, rw < 560.)));
        let holes = self.holes.clone();
        // the room's wall: behind the chat, over the backdrop; the pen's layer and the tools over everything while drawing
        self.wall_room_focus(if is_dm { None } else { Some(gid.clone()) });
        let wall_layer = self.wall_room_layer(&gid);
        let wall_hit = self.wall_room_hit(cx);
        let wall_bar = self.wall_toolbar(cx);
        let wall_btn = if is_dm { None } else { self.wall_room_button(cx) };
        self.reading_look();
        self.sel.texts.borrow_mut().clear();
        self.sel_resolve();
        // the day of the first row in view, and the way back to the end (the web's `#day-float` and `#jump`): only while reading back
        let float_day = (!crate::reading::at_end(self.reading.remaining)).then(|| {
            let ix = self.reading.geo.iter().filter(|(_, (t, h))| t + h > 0.).map(|(i, _)| *i).min()?;
            let m = self.win.msgs.get(ix)?; (m.ts > 0).then(|| day_words(m.ts, chrono::Local::now()))
        }).flatten();
        let jump = self.reading.remaining >= 260.;
        let has_text = !self.composer.read(cx).value().trim().is_empty();
        let editing = self.msg.edit.is_some();
        let plus_open = self.msg.plus;
        let box_fill: Hsla = match self.prefs.box_colour.as_str() {
            "default" => hsla(0.71, 0.22, 0.07, 0.93),
            "room" => { let mut c: Hsla = rgb(look::mix(duo.main, 0x000000, 0.7)).into(); c.a = 0.93; c }
            hex => { let mut c: Hsla = rgb(look::parse_hex(hex).unwrap_or(0x14121a)).into(); c.a = 0.93; c }
        };
        let reply_bar = self.reply_bar(cx);
        let react_pop = self.react_pop(cx);
        if crate::reading::at_end(self.reading.remaining) { self.msg.below = 0; }
        let below = self.msg.below;
        let phone = crate::phone::on();
        let tap = crate::phone::pick(34., crate::phone::TAP);
        let back = phone.then(|| { let cid = self.cid.clone(); div().id("room-back").flex_none().flex().items_center().justify_center().size(px(crate::phone::TAP)).rounded_full().bg(rgba(0x1a1820db)).border_1().border_color(look::line())
            .child(crate::icons::icon(crate::icons::Icon::Left, 20., look::paper()))
            .on_click(cx.listener(move |this, _, window, cx| { match cid.clone() { Some(c) => this.go(Level::Server(c), window, cx), None => this.go(Level::Home, window, cx) } })) });
        let bar_chips = self.room_bar(&gid, is_dm, owner, wall_btn, chat_cid, cx);
        let drag_seen = self.files.drag_seen.clone();
        let drop_hint = self.drop_overlay(rgb(duo.hi).into(), &title);
        let files_layer = self.files_layer(window, cx);
        let vbar = self.voice_bar(cx);
        let recording = vbar.is_some();
        let vwatch = if recording {
            let me = cx.entity().downgrade();
            let (m1, m2, m3) = (me.clone(), me.clone(), me);
            Some(canvas(|_, _, _| {}, move |_, _, window, _| {
                let m1 = m1.clone(); let m2 = m2.clone(); let m3 = m3.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, ph, w, cx| { if ph == DispatchPhase::Capture { let _ = m1.update(cx, |t, cx| t.voice_release(w, cx)); } });
                window.on_mouse_event(move |ev: &MouseMoveEvent, ph, _, cx| { if ph == DispatchPhase::Capture { let p = ev.position; let _ = m2.update(cx, |t, cx| t.voice_move(p, cx)); } });
                window.on_key_event(move |ev: &KeyDownEvent, ph, w, cx| { if ph == DispatchPhase::Capture && ev.keystroke.key == "escape" { let _ = m3.update(cx, |t, cx| t.voice_finish(false, w, cx)); } });
            }).absolute().size_0())
        } else { None };
        v_flex().id("room").flex_1().h_full().min_w_0().relative().bg(rgb(bd_back))
            .drag_over::<ExternalPaths>(move |s, _, _, _| { drag_seen.set(true); s })
            .on_drop(cx.listener(|this, p: &ExternalPaths, window, cx| this.files_dropped(p.paths().to_vec(), window, cx)))
            .children(backdrop.map(|b| div().absolute().inset_0().child(b)))
            .child(wall_layer)
            .child(h_flex().flex_none().items_center().gap(px(10.)).when(phone, |d| d.px_3().flex_wrap()).when(!phone, |d| d.px_5()).pt_3().pb_1()
                .children(back)
                .child(div().flex_1().min_w_0().flex().child(v_flex().relative().max_w_full().items_start().overflow_hidden().px(px(16.)).pt(px(4.)).pb(px(5.))
                    // the same slip as the composer's: the room's name and who is in it, on a squircle over the backdrop
                    .child(sq::back(16., hsla(0.71, 0.22, 0.07, 0.93))).child(hole(&holes))
                    // the room's name alone, its letters set apart (no letter-spacing in gpui: one box per letter)
                    .child(h_flex().relative().max_w_full().overflow_hidden().gap(px(3.)).font_family(look::F_SHARP).text_size(px(26.)).line_height(px(30.))
                        .children(title.chars().map(|ch| div().flex_none().child(if ch == ' ' { "\u{2002}".to_string() } else { ch.to_string() }))))
                    .child(sq::front(16., None, Some((1., look::line())), None))))
                .children(bar_chips))
            .child(div().id("room-text").flex_1().min_h_0().relative().track_focus(&self.sel.focus).key_context("ChatText")
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.sel_key(ev, window, cx)))
                .child(sel_watch(self.sel.area.clone(), cx))
                .child(list(self.list.clone(), cx.processor(|this, ix: usize, _, _| this.render_msg(ix))).size_full().pb(px(42.)))
                .child(div().absolute().top_0().right_0().bottom_0().w(px(12.)).child(Scrollbar::vertical(&self.list)))
                .children(float_day.map(|t| h_flex().absolute().top(px(6.)).left_0().right_0().justify_center().child(div().px(px(14.)).py(px(6.)).rounded_full().bg(rgb(0x1b1922)).border_1().border_color(look::line()).font_family(look::F_SHARP).text_size(px(14.)).text_color(look::mute()).child(t))))
                .children(jump.then(|| h_flex().absolute().bottom(px(10.)).left_0().right_0().justify_center().child(div().id("jump").px(px(14.)).py(px(crate::phone::pick(7., 12.))).rounded_full().bg(rgb(0x1b1922)).border_1().border_color(look::line()).shadow_lg().font_family(look::F_UI).text_size(px(12.)).cursor_pointer().when(below > 0, |d| d.bg(rgb(duo.hi)).text_color(rgb(look::VOID))).child(if below > 0 { format!("\u{2193} {below} new") } else { "\u{2193} latest".to_string() })
                    .on_click(cx.listener(|this, _, _, cx| { this.list.scroll_to_end(); cx.notify(); })))))
                .children(react_pop).children(self.emoji_overlay(cx)).children(self.sel_menu()))
            .child(h_flex().w_full().flex_none().justify_center().px_4().pb(px(8.))
                .child(v_flex().w_full().max_w(px(COLUMN - 20.))
                    .children(typing_strip)
                    // the composer, kept small: one ball-round pill, + (file, emoji, voice clip behind it) on the left and send on the right
                    .child(h_flex().w_full().max_w(px(560.)).mx_auto().items_center().font_family(font)
                        .child(v_flex().flex_1().min_w_0().relative().p(px(5.))
                            // a round-ended pill over the backdrop (translucent, the backdrop moves behind it)
                            .rounded_full().bg(box_fill).border_1().border_color(look::line()).child(hole(&holes))
                            .children(reply_bar)
                            .children(vbar)
                            .child(h_flex().relative().items_center().gap_2().when(recording, |d| d.hidden())
                                .child(div().relative().flex_none()
                                    .when(plus_open, |d| d.on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, _, cx| { if this.msg.plus_box.get().is_some_and(|b| b.contains(&ev.position)) { return; } this.msg.plus = false; cx.notify(); })))
                                    .child(div().id("plus").relative().flex().items_center().justify_center().size(px(tap)).rounded_full().cursor_pointer()
                                        .bg(if plus_open { hsla(0., 0., 1., 0.16) } else { hsla(0., 0., 1., 0.06) })
                                        .hover(|d| d.bg(hsla(0., 0., 1., 0.12)))
                                        .child(crate::icons::icon(crate::icons::Icon::Plus, 17., if plus_open { look::paper() } else { look::mute() }))
                                        .on_click(cx.listener(|this, _, _, cx| { this.msg.plus = !this.msg.plus; cx.notify(); })))
                                    .children(plus_open.then(|| {
                                        let row = |id: &'static str, ic: crate::icons::Icon, label: &'static str| div().id(id).relative().h(px(tap)).px(px(12.)).flex().items_center().gap(px(10.)).rounded(px(8.)).cursor_pointer()
                                            .hover(|d| d.bg(hsla(0., 0., 1., 0.08))).child(crate::icons::icon(ic, 16., look::mute())).child(label).child(self.files.hits.track(id));
                                        v_flex().absolute().bottom(px(tap + 10.)).left(px(-5.)).w(px(170.)).p(px(4.)).rounded(px(12.)).bg(rgb(0x1b1922)).border_1().border_color(look::line()).shadow_lg()
                                            .font_family(look::F_UI).text_size(px(13.)).text_color(look::paper())
                                            .child({ let bx = self.msg.plus_box.clone(); canvas(move |b, _, _| bx.set(Some(b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full() })
                                            .child(row("attach", crate::icons::Icon::Clip, "file").on_click(cx.listener(|this, _, window, cx| { this.msg.plus = false; this.files_attach(window, cx); })))
                                            .child(row("emoji-btn", crate::icons::Icon::Smile, "emoji").on_click(cx.listener(|this, ev: &ClickEvent, window, cx| { this.msg.plus = false; let at = ev.mouse_position().unwrap_or_default(); this.emoji_open(crate::emoji::Target::Composer, at, window, cx); })))
                                            .child(row("mic", crate::icons::Icon::Mic, "voice clip")
                                                .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, w, cx| { cx.stop_propagation(); this.msg.plus = false; this.voice_press(Some(ev.position), w, cx); })))
                                    })))
                                .child(div().flex_1().min_w_0().child(Textarea::new(&self.composer).appearance(false).text_size(px(if phone { size.max(17.) } else { size.min(17.) })).line_height(px(22.))))
                                .child(div().id("send").flex_none().flex().items_center().justify_center().h(px(tap)).px(px(16.)).rounded_full().cursor_pointer().whitespace_nowrap()
                                    .font_family(look::F_UI).text_size(px(13.))
                                    .when(has_text, |d| d.bg(look::paper()).text_color(rgb(look::VOID)))
                                    .when(!has_text, |d| d.bg(hsla(0., 0., 1., 0.06)).text_color(look::mute()))
                                    .child(if editing { "save" } else { "send" }).on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))))))))
            .children(self.search_overlay(cx))
            .children(wall_hit)
            .children(wall_bar)
            .children(files_layer)
            .children(vwatch)
            .child(drop_hint)
            .into_any_element()
    }

    fn render_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snap.clone().unwrap_or_default();
        let alerts = self.alerts_settings(window, cx);
        let relay_box = self.relay_settings(cx);
        let avatar_row = self.avatar_settings(cx);
        let back = if self.gid.is_some() { Level::Chat } else { Level::Home };
        let label = |t: &str| div().mt_4().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(t.to_string());
        let note = |t: &str| div().text_size(px(13.)).text_color(look::mute()).child(t.to_string());
        // a row of choices, one of them on
        fn choice(id: &'static str, on: bool, text: &'static str) -> Stateful<Div> {
            div().id(id).relative().px_3().py_1().border_1().border_color(hsla(0., 0., 0., 0.)).cursor_pointer()
                .when(on, |d| d.text_color(rgb(0x0c0d12)).child(sq::back(999., look::paper())))
                .child(div().relative().child(text))
                .child(sq::front(999., None, Some((1., if on { look::paper() } else { look::line() })), None))
        }
        let mode = self.notify_settings["mode"].as_str().unwrap_or("all").to_string();
        let show_text = self.notify_settings["show_text"].as_bool().unwrap_or(false);
        let (saver, font, presence) = (self.prefs.saver, self.prefs.chat_font, self.prefs.presence);
        let set_saver = |s: Saver| cx.listener(move |this: &mut Self, _: &ClickEvent, window, cx| { this.prefs.saver = s; this.prefs.save(); this.power_now(window, cx); cx.notify(); });
        let set_font = |f: ChatFont| cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| { this.prefs.chat_font = f; this.prefs.save(); this.list.remeasure(); cx.notify(); });
        let set_mode = |m: &'static str| cx.listener(move |this: &mut Self, _: &ClickEvent, window, cx| this.set_setting("notify_set", json!({"mode": m}), window, cx));
        let entity = cx.entity().downgrade();
        let toggle = move |id: &'static str, on: bool, text: &'static str, method: &'static str| {
            let e = entity.clone();
            Switch::new(id).checked(on).label(text).on_click(move |v: &bool, window, cx| {
                let v = *v;
                let _ = e.update(cx, |this, cx| {
                    match method {
                        "presence" => { this.prefs.presence = v; this.prefs.save(); cx.notify(); }
                        "ring_sound" => { this.prefs.sound = v; this.prefs.save(); cx.notify(); }
                        "notify_set" => this.set_setting("notify_set", json!({"show_text": v}), window, cx),
                        "set_listening_sharing" => { let r = this.snap.as_ref().map(|s| s.rune_url.clone()).unwrap_or_default(); this.set_setting(method, json!({"on": v, "rune": r}), window, cx) }
                        _ => this.set_setting(method, json!({"on": v}), window, cx),
                    }
                });
            })
        };
        let body = v_flex().id("settings").flex_1().min_h_0().w_full().overflow_y_scroll().px_6().pb_8().gap_2().max_w(px(640.))
            .child(label("your name"))
            .child(h_flex().gap_2().child(div().w(px(280.)).child(Input::new(&self.set_name)))
                .child(Button::new("save-name").label("Save").on_click(cx.listener(|this, _, window, cx| { let n = this.set_name.read(cx).value().to_string(); this.set_setting("set_name", json!({"name": n}), window, cx); }))))
            .child(label("profile picture"))
            .child(avatar_row)
            .child(label("battery saver"))
            .child(h_flex().gap_2()
                .child(choice("sv-auto", saver == Saver::Auto, "Auto").on_click(set_saver(Saver::Auto)))
                .child(choice("sv-on", saver == Saver::On, "On").on_click(set_saver(Saver::On)))
                .child(choice("sv-off", saver == Saver::Off, "Off").on_click(set_saver(Saver::Off))))
            .child(note(if self.still() { "On now: the moving backgrounds hold still." } else if saver == Saver::Auto { "Auto: holds the moving backgrounds still while this computer runs on battery." } else { "Off: the backgrounds move." }))
            .child(label("chat font"))
            .child(h_flex().gap_2()
                .child(choice("cf-b", font == ChatFont::Bellefair, "Bellefair").on_click(set_font(ChatFont::Bellefair)))
                .child(choice("cf-r", font == ChatFont::Raster, "Raster").on_click(set_font(ChatFont::Raster))))
            .child(label("chatbox colour"))
            .child(h_flex().gap(px(10.)).flex_wrap().children(BOX_COLOURS.iter().map(|(name, c)| {
                let on = self.prefs.box_colour == *name;
                let fill: Hsla = match *name { "default" => hsla(0.71, 0.22, 0.07, 1.), "room" => look::line(), hex => rgb(look::parse_hex(hex).unwrap_or(0)).into() };
                div().id(SharedString::from(format!("box-{name}"))).size(px(30.)).rounded_full().cursor_pointer().bg(fill).border_2()
                    .border_color(if on { look::paper() } else { look::line() })
                    .when(*name == "room", |d| d.flex().items_center().justify_center().font_family(look::F_UI).text_size(px(9.)).text_color(look::paper()).child("room"))
                    .tooltip(move |window, cx| Tooltip::new(*c).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| { this.prefs.box_colour = name.to_string(); this.prefs.save(); cx.notify(); }))
            })))
            .child(label("notifications"))
            .child(h_flex().gap_2()
                .child(choice("nm-all", mode == "all", "Every message").on_click(set_mode("all")))
                .child(choice("nm-men", mode == "mentions", "Mentions only").on_click(set_mode("mentions")))
                .child(choice("nm-off", mode == "off", "Off").on_click(set_mode("off"))))
            .child(toggle("nt-text", show_text, "Show the message text in notifications", "notify_set"))
            .child(toggle("nt-ring", self.prefs.sound, "Play a sound when someone rings", "ring_sound"))
            .child(alerts)
            .child(label("privacy"))
            .child(toggle("p-typing", snap.typing_enabled, "Show others when I am writing", "set_typing_enabled"))
            .child(toggle("p-read", snap.read_receipts, "Read receipts", "set_read_receipts"))
            .child(toggle("p-pres", presence, "Show where I am on the cards", "presence"))
            .child(toggle("p-listen", snap.share_listening, "Share what I am listening to", "set_listening_sharing"))
            .child(label("rune address"))
            .child(h_flex().gap_2().child(div().w(px(280.)).child(Input::new(&self.set_rune)))
                .child(Button::new("save-rune").label("Save").on_click(cx.listener(|this, _, window, cx| this.rune_address_save(window, cx)))))
            .child(note("Where rune answers on this device. Song title and artist go to your communities, end to end encrypted. Nothing else is read or sent."))
            .children((!cfg!(target_os = "ios")).then(|| label("calls")))
            .children((!cfg!(target_os = "ios")).then(|| h_flex().gap_2().child(div().id("set-gamekeys").relative().px_3().py_1().cursor_pointer().child(sq::back(999., rgb(0x0b0a10))).child(sq::front(999., None, Some((1., look::line())), None)).child(div().relative().child("Game keys\u{2026}"))
                .on_click(cx.listener(|this, _, window, cx| this.gk_open(window, cx))))))
            .children((!cfg!(target_os = "ios")).then(|| note("Mute yourself in mega while you talk to your game team.")))
            .child(label("quick tour"))
            .child(h_flex().gap_2().child(div().id("tour-again").relative().px_3().py_1().cursor_pointer().child(sq::back(999., rgb(0x0b0a10))).child(sq::front(999., None, Some((1., look::line())), None)).child(div().relative().child("show the tour again"))
                .on_click(cx.listener(|this, _, window, cx| { this.go(Level::Home, window, cx); this.tour_start(window, cx); }))))
            .child(note("A short look at the cards, the dots and the call dock."))
            .child(label("this device"))
            .child(h_flex().gap_2().flex_wrap().mt_2()
                .child(Button::new("set-devices").label("Devices and recovery").on_click(cx.listener(|this, _, window, cx| this.dev_open(crate::members::Panel::Devices, window, cx))))
                .child(Button::new("set-log").label("Activity log").on_click(cx.listener(|this, _, window, cx| this.dev_open(crate::members::Panel::Activity, window, cx)))))
            .child(relay_box)
            .child(note(&format!("Account folder: {}", self.core.dir.display())))
            .child(label("version"))
            .child(h_flex().gap_3().items_center()
                .child(div().text_size(px(14.)).text_color(look::paper()).child(format!("mega {}", self.upd.current)))
                .child(Button::new("check-updates").small().label("Check for updates").on_click(cx.listener(|this, _, window, cx| this.update_check_now(window, cx)))))
            .when_some(self.upd.checked.clone(), |d, t| d.child(note(&t)))
            .when(snap.net == "offline" && !snap.net_detail.is_empty(), |d| d.child(div().text_size(px(13.)).text_color(rgb(look::DANGER)).child(format!("Not connected to the server: {}", snap.net_detail))))
            .child(h_flex().mt_2().child(Button::new("lock").label("Lock").on_click(cx.listener(|this, _, window, cx| this.lock(window, cx)))))
            .when_some(self.error.clone(), |d, e| d.child(div().text_color(rgb(look::DANGER)).child(e)));
        v_flex().size_full().child(self.top_bar("settings".into(), Some(back), cx)).child(body)
    }

    /// The saver choice changed: ask about the battery at once rather than at the next minute.
    fn power_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("power_status", json!({}), window, cx, |this, r, _, cx| { this.on_battery = r.ok().and_then(|v| v["battery"].as_bool()).unwrap_or(false); cx.notify(); });
    }
}

/// What is happening to one message right now (arrive.rs): its size tier, its glyph copy while it arrives, reactions flying to it.
/// `k` is the reading zone's scale (reading.rs; 1 = whole), `day` the day line above it, `new` the "N new" line above it.
pub struct RowFx { pub dens: Option<crate::density::Dens>, pub tier_px: Option<f32>, pub arrive: Option<(f64, u64)>, pub hi: Hsla, pub flights: Vec<crate::arrive::Flight>, pub now: f64, pub k: f32, pub day: Option<String>, pub new: Option<usize>, /** the selected part of the text, and where to keep its painted layout (select.rs) */ pub pick: Option<std::ops::Range<usize>>, pub texts: crate::select::Texts, /** the file of this message, if it has one (files.rs) */ pub file: Option<crate::files::FileCtx>, /** the tools, chips and quote of this message (msgact.rs) */ pub act: Option<crate::msgact::ActCtx>, /** the colour fringe on rows near the top of the room: 1 at the top, 0 from a third of the way down */ pub fringe: f32 }

/// The chatbox colours in settings: the stored value and what the tooltip calls it. Dark and deep, so the words stay light on them.
const BOX_COLOURS: [(&str, &str); 9] = [("default", "Default"), ("room", "The room's colour"), ("#1c2a4a", "Navy"), ("#3a1f4d", "Plum"), ("#1f3a2c", "Forest"), ("#4a1c24", "Wine"), ("#2b3036", "Slate"), ("#4a3518", "Amber"), ("#0f3d3e", "Teal")];

/// The widest the message column gets (mega-design.css: `--gutter` 58 + `--measure` 800 + 44 around it).
pub const COLUMN: f32 = 720.;

/// The double check of a read message (app.js `RR_ICON`, a 15 by 10 drawing).
thread_local! {
    /// Where each message's seen mark was last laid out, in window pixels (the UI test points at it).
    pub static SEEN_AT: std::cell::RefCell<std::collections::HashMap<String, Bounds<Pixels>>> = Default::default();
}

/// The double check, with the names of who has read up to here when it is pointed at (like the web's read mark).
fn seen_mark(m: &Msg, k: f32) -> impl IntoElement {
    let tip = SharedString::from(format!("Seen by {}", m.read_by.join(", ")));
    let id = m.id.clone();
    div().id(SharedString::from(format!("seen-{}", m.id))).relative().flex().items_center().px(px(2.)).py(px(2.)).cursor_default()
        .child(ticks(look::mute(), k))
        .child(canvas(move |b, _, _| SEEN_AT.with(|s| { s.borrow_mut().insert(id.clone(), b); }), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
}

fn ticks(color: Hsla, k: f32) -> impl IntoElement {
    canvas(|_, _, _| {}, move |b, _, window, _| {
        let (sx, sy) = (f32::from(b.size.width) / 15., f32::from(b.size.height) / 10.);
        let pt = |x: f32, y: f32| point(b.origin.x + px(x * sx), b.origin.y + px(y * sy));
        for line in [[(1., 5.4), (4., 8.4), (9.6, 1.4)].as_slice(), [(7.4, 8.4), (13.9, 1.4)].as_slice()] {
            let mut p = gpui_kit::PathBuilder::stroke(px(1.5 * sx));
            p.move_to(pt(line[0].0, line[0].1));
            for q in &line[1..] { p.line_to(pt(q.0, q.1)); }
            if let Ok(path) = p.build() { window.paint_path(path, color); }
        }
    }).w(px(13. * k)).h(px(9. * k)).flex_none()
}

/// The row's painted text layout, looked up when a click comes (it is stored while the row paints).
fn el_layout(id: &str, texts: &crate::select::Texts) -> LayoutCell { LayoutCell { id: id.to_string(), texts: texts.clone() } }
struct LayoutCell { id: String, texts: crate::select::Texts }
impl LayoutCell { fn get(&self) -> Option<gpui_kit::TextLayout> { self.texts.borrow().get(&self.id).map(|t| t.layout.clone()) } }

/// The runs of a message's words: the room's font, with bold, italic, code, links and mentions picked out, and the selection behind (rich.rs).
fn text_runs(p: &crate::rich::Parsed, family: &'static str, hi: Hsla, ink: Hsla, pick: Option<std::ops::Range<usize>>, pick_bg: Hsla) -> Vec<TextRun> {
    use crate::rich::Kind;
    let n = p.text.len();
    let mut cuts = vec![0, n];
    for s in &p.spans { cuts.push(s.r.start.min(n)); cuts.push(s.r.end.min(n)); }
    if let Some(r) = &pick { cuts.push(r.start.min(n)); cuts.push(r.end.min(n)); }
    cuts.sort_unstable(); cuts.dedup(); cuts.retain(|c| p.text.is_char_boundary(*c));
    let mut runs = Vec::with_capacity(cuts.len());
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b { continue; }
        let (mut f, mut color, mut bg, mut ul) = (font(family), ink, None, None);
        for s in &p.spans {
            if !(s.r.start <= a && b <= s.r.end) { continue; }
            match &s.k {
                Kind::Bold => f.weight = FontWeight::BOLD,
                Kind::Italic => f.style = FontStyle::Italic,
                Kind::Code => { f = font(look::F_MONO); bg = Some(hsla(0., 0., 1., 0.07)); }
                Kind::Link(_) => { color = hi; ul = Some(UnderlineStyle { thickness: px(1.), color: Some(hi), wavy: false }); }
                Kind::At(me) => { f.weight = FontWeight::MEDIUM; if *me { bg = Some(hi); color = rgb(look::VOID).into(); } else { color = hi; } }
            }
        }
        if pick.as_ref().is_some_and(|r| r.start <= a && b <= r.end) { bg = Some(pick_bg); }
        runs.push(TextRun { len: b - a, font: f, color, background_color: bg, underline: ul, strikethrough: None });
    }
    runs
}

/// A small outlined tag next to a name (mega-polish.css `.chip`: "owner", "admin").
fn tag(text: String, k: f32) -> Div {
    div().px(px(8. * k)).rounded_full().border_1().border_color(look::line()).text_color(look::mute()).text_size(px(11. * k)).line_height(px(16. * k)).child(text)
}

/// The line above the first message of a day (mega-polish.css `.day`): the day in the room's small type between two hairlines.
fn day_line(text: String, holes: &Holes) -> Div {
    let hair = || div().flex_1().h(px(1.)).bg(hsla(0., 0., 1., 0.08));
    h_flex().w_full().items_center().gap(px(14.)).mt(px(18.)).mb(px(8.)).px_2().text_color(look::mute())
        .child(hair()).child(div().relative().font_family(look::F_SHARP).text_size(px(14.)).line_height(px(14.)).child(text).child(hole(holes))).child(hair())
}

/// The line above the oldest message held while there are older ones (app.js `Show earlier messages`).
fn older_line(c: &crate::msgact::ActCtx) -> impl IntoElement {
    let me = c.me.clone();
    h_flex().w_full().justify_center().mt(px(10.)).child(div().id("older").relative().px(px(14.)).py(px(6.)).rounded_full().cursor_pointer().font_family(look::F_UI).text_size(px(12.)).text_color(look::mute())
        .bg(hsla(0., 0., 1., 0.06)).hover(|s| s.bg(hsla(0., 0., 1., 0.12))).child("Show earlier messages").child(c.hits.track("older"))
        .on_click(crate::msgact::on(&me, |this, window, cx| this.msg_older(window, cx))))
}

/// The "N new" line (mega-polish.css `.newline`): in the room's bright colour, a line after it.
fn new_line(n: usize, hi: Hsla, holes: &Holes) -> Div {
    h_flex().w_full().items_center().gap(px(10.)).mt(px(14.)).mb(px(8.)).pl(px(58.)).pr_2()
        .child(div().relative().font_family(look::F_SHARP).text_size(px(14.)).line_height(px(14.)).text_color(hi).child(format!("{n} new")).child(hole(holes)))
        .child(div().flex_1().h(px(1.)).bg(hi.opacity(0.6)))
}

/// What a day line says (app.js `fmtDay`, lowercased by the room's type): today, yesterday, or the weekday and date.
pub fn day_words(ts: u64, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::TimeZone as _;
    let Some(d) = chrono::Local.timestamp_opt(ts as i64, 0).single() else { return String::new() };
    let (a, b) = (d.date_naive(), now.date_naive());
    if a == b { return "today".into(); }
    if b.pred_opt() == Some(a) { return "yesterday".into(); }
    let fmt = if chrono::Datelike::year(&a) == chrono::Datelike::year(&b) { "%A, %B %-d" } else { "%A, %B %-d, %Y" };
    d.format(fmt).to_string().to_lowercase()
}

/// One message. `prev` decides whether the face and name are shown again (same person within five minutes: no).
pub fn row(m: &Msg, prev: Option<&Msg>, (font, size, lh): (&'static str, f32, f32), holes: &Holes, star: bool, fx: &RowFx) -> AnyElement {
    let k = fx.k;
    let (size, lh) = match &fx.dens { Some(d) => (d.base, (d.base * lh / size).round()), None => (size, lh) };
    let (size, lh) = match fx.tier_px { Some(p) => (p, (p * 1.05).round()), None => (size, lh) };
    let (size, lh) = (size * k, (lh * k).round());
    let line = |inner: Div| h_flex().w_full().justify_center().child(inner.w_full().max_w(px(COLUMN)));
    if m.deleted { return div().into_any_element(); }
    if m.kind == "member" || m.kind == "notice" {
        return v_flex().w_full().when_some(fx.day.clone(), |d, t| d.child(line(day_line(t, holes)))).when_some(fx.new, |d, n| d.child(line(new_line(n, fx.hi, holes))))
            .child(line(h_flex().pl(px(68.)).pr_4().py_1().child(div().relative().font_family(look::F_NAME).text_size(px(12. * k)).text_color(look::mute()).child(m.text.clone()).child(hole(holes))))).into_any_element();
    }
    let head = fx.day.is_some() || fx.new.is_some() || m.reply.is_some() || !prev.is_some_and(|p| p.sender == m.sender && p.kind != "member" && p.kind != "notice" && m.ts.saturating_sub(p.ts) < 300);
    // every piece is as wide as its words (items_start), so the backdrop is cut out around the words and not the row
    let mut col = v_flex().flex_1().min_w_0().items_start();
    if head {
        col = col.child(h_flex().relative().child(hole(holes)).gap_2().items_center().font_family(look::F_NAME).text_size(px(12. * k))
            .child(div().text_color(hsla(0., 0., 0.94, 0.78)).child(m.name.clone()))
            .when(m.role == "owner" || m.role == "admin", |d| d.child(tag(m.role.clone(), k)))
            .when(m.same_name && !m.tag.is_empty(), |d| d.child(tag(m.tag.clone(), k)))
            .child(div().text_color(look::mute()).child(clock(m.ts)))
            .when(!m.read_by.is_empty(), |d| d.child(seen_mark(m, k))));
    }
    if let (Some(r), Some(c)) = (&m.reply, &fx.act) {
        if !c.quote_gone { col = col.child(div().relative().child(hole(holes)).max_w_full().child(crate::msgact::quote(r, font, c))); }
    }
    let body: AnyElement = if let (Some(fi), Some(ctx)) = (&m.file, &fx.file) { crate::files::body(m, fi, ctx, holes) }
        else if let Some((kk, tick)) = fx.arrive {
            // the real words stay where they are (invisible, so the row keeps its size); a copy made of glyphs sits over them and resolves
            let (copy, real) = crate::arrive::compose(m.shown(), kk, tick);
            let ink = look::paper();
            div().relative().child(hole(holes)).max_w_full().font_family(font).text_size(px(size)).line_height(px(lh))
                .child(div().text_color(hsla(0., 0., 0., 0.)).child(m.shown().to_string()))
                .child(div().absolute().top_0().left_0().size_full().overflow_hidden().text_color(fx.hi)
                    .child(StyledText::new(copy).with_highlights([(0..real, HighlightStyle { color: Some(ink), ..Default::default() })])))
                .into_any_element()
        }
        else if m.kind == "text" && m.file.is_none() && !m.text.is_empty() {
            // the words are a StyledText so a selection can colour part of them and the painted layout can be asked where a pointer is;
            // links, mentions, bold, italic and code are runs of it (rich.rs)
            let runs = text_runs(&m.rich, font, fx.hi, look::paper(), fx.pick.clone(), fx.hi.opacity(0.4));
            let st = StyledText::new(m.shown().to_string()).with_runs(runs);
            let (layout, texts, id) = (st.layout().clone(), fx.texts.clone(), m.id.clone());
            let links = crate::rich::links(&m.rich);
            // the colour fringe: the same words twice behind, red a little left and cyan a little right, strongest at the top of the room
            let fr = fx.fringe;
            let ghost = |c: Hsla, dx: f32| {
                let runs: Vec<TextRun> = text_runs(&m.rich, font, fx.hi, c, None, fx.hi).into_iter().map(|mut r| { r.color = c; r.background_color = None; r.underline = None; r }).collect();
                div().absolute().top_0().left(px(dx)).w_full().child(StyledText::new(m.shown().to_string()).with_runs(runs))
            };
            let el = div().id(SharedString::from(format!("txt-{}", m.id))).relative().child(hole(holes)).max_w_full().font_family(font).text_size(px(size)).line_height(px(lh))
                .when(fr > 0.01, |d| d.child(ghost(hsla(0.98, 1., 0.6, 0.75 * fr), -3. * fr)).child(ghost(hsla(0.52, 1., 0.6, 0.75 * fr), 3. * fr)))
                .child(div().relative().child(st))
                .child(canvas(|_, _, _| {}, move |b, _, _, _| { texts.borrow_mut().insert(id.clone(), crate::select::RowText { layout: layout.clone(), bounds: layout.bounds() }); let _ = b; }).absolute().top_0().left_0().size_full());
            match (&fx.act, links.is_empty()) {
                (Some(c), false) => {
                    let lay = el_layout(&m.id, &fx.texts);
                    el.cursor_pointer().on_click(crate::msgact::on_at(&c.me, move |this, p, window, cx| {
                        let Some(off) = lay.get().and_then(|l| l.index_for_position(p).ok()) else { return };
                        if let Some((_, url)) = links.iter().find(|(r, _)| r.contains(&off)) { this.open_link(url, window, cx); }
                    })).into_any_element()
                }
                _ => el.into_any_element(),
            }
        }
        else { div().relative().child(hole(holes)).max_w_full().font_family(font).text_size(px(size)).line_height(px(lh)).child(m.text.clone()).into_any_element() };
    col = col.child(body);
    let mut foot = h_flex().relative().gap_1().flex_wrap();
    let landing = std::rc::Rc::new(std::cell::Cell::new(Bounds::<Pixels>::default()));
    let mut flying: Vec<(usize, crate::arrive::Flight)> = vec![];
    for (i, r) in m.reactions.iter().enumerate() {
        // a thrown emoji is still in the air: the pill keeps its old number (or stays hidden when it is new)
        let shown = if fx.flights.is_empty() { Some(r.count) } else {
            let mut t = crate::arrive::Flights::default(); t.live = fx.flights.clone(); t.shown(&m.id, &r.emoji, fx.now, r.count)
        };
        if let Some(f) = fx.flights.iter().find(|f| f.emoji == r.emoji) { flying.push((i, f.clone())); }
        let Some(c) = &fx.act else { continue };
        let pill = crate::msgact::chip(m, r, shown.unwrap_or(r.count), k, c);
        foot = foot.child(if shown.is_none() { pill.opacity(0.) } else { pill });
    }
    if let (Some(c), false) = (&fx.act, m.reactions.is_empty()) { foot = foot.child(crate::msgact::add_chip(m, k, c)); }
    if !flying.is_empty() { foot = foot.child(flight_spot(landing.clone())); }
    if m.edited { foot = foot.child(div().text_color(look::mute()).text_size(px(11. * k)).child("edited")); }
    if m.mine && (m.status == "pending" || m.status == "failed") { foot = foot.child(div().text_color(look::mute()).text_size(px(11. * k)).child(m.status.clone())); }
    // a continuation row has no name line to carry the double check: it sits in the small print
    if !m.read_by.is_empty() && !head { foot = foot.child(div().flex().items_center().h(px(14.)).child(seen_mark(m, k))); }
    let has_foot = !m.reactions.is_empty() || m.edited || (!m.read_by.is_empty() && !head) || (m.mine && (m.status == "pending" || m.status == "failed"));
    if has_foot { foot = foot.child(hole(holes)); }
    let group = SharedString::from(format!("row-{}", m.id));
    let older = fx.act.as_ref().is_some_and(|c| c.older);
    v_flex().w_full().when(older, |d| d.child(older_line(fx.act.as_ref().unwrap()))).when_some(fx.day.clone(), |d, t| d.child(line(day_line(t, holes)))).when_some(fx.new, |d, n| d.child(line(new_line(n, fx.hi, holes))))
        .child(line(h_flex().relative().group(group.clone()).items_start().gap_3().px(px(10.)).pb_1().when(head, |d| d.pt(px(10.)))
            .when_some(fx.dens, |d, n| d.pb(px(4. + n.pad)).pt(px(n.pad)).when(head, |d| d.pt(px(n.gap + n.pad))))
            .when_some(fx.act.as_ref().filter(|c| c.mention), |d, c| d.child(div().absolute().inset_0().bg(linear_gradient(90., linear_color_stop(c.hi.opacity(0.14), 0.), linear_color_stop(c.hi.opacity(0.), 0.86)))).child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(c.hi)))
            .when_some(fx.act.as_ref().filter(|c| c.flash > 0.), |d, c| d.child(div().absolute().inset_0().rounded(px(10.)).bg(c.hi.opacity(0.22 * c.flash))))
            .when(!flying.is_empty(), |d| d.child(flight_layer(flying, landing, fx.now, fx.hi)))
            .when(star, |d| d.child(div().absolute().left(px(-12.)).top(px(if head { 22. } else { 8. })).text_size(px(13.)).text_color(look::paper()).opacity(0.8).child("✦")))
            .child(if head { avatar(&m.sender, &m.name, 36. * k).mt_1() } else { div().w(px(36. * k)).flex_none() })
            .child(col.child(foot))
            .when_some(fx.act.as_ref().filter(|c| c.can_act()), |d, c| d.child(crate::msgact::tools(m, c, group.clone()))))).into_any_element()
}

impl Render for Mega {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if crate::phone::sync(window) { crate::titlebar::set_edge(window.is_window_active(), cx); }
        let phone = crate::phone::on();
        if !std::mem::replace(&mut self.first_frame_done, true) { window.on_next_frame(|_, _| crate::bench::mark("first_frame")); }
        crate::bench::drive(self, window, cx);
        self.fly_step(window, cx);
        self.sel_autoscroll(window, cx);
        // arrivals and thrown reactions ask for a frame only while one is in the air; the frame after the last one draws the finished state
        if !(self.arrivals.is_idle() && self.flights.is_idle()) {
            let now = self.now_secs();
            self.arrivals.step(now); self.flights.step(now);
            window.request_animation_frame();
        }
        if self.msg.flash.as_ref().is_some_and(|(_, t0)| self.now_secs() - t0 < crate::msgact::FLASH) { window.request_animation_frame(); }
        self.holes.borrow_mut().clear();
        self.call.hits.clear(); self.files.hits.clear();
        let base = div().id("root").flex_1().min_h_0().w_full().relative().bg(rgb(look::VOID)).text_color(look::paper()).font_family(look::F_UI).text_size(px(if phone { 17. } else { 15. }))
            // dragging the column's edge, and push-to-talk: the key never reaches the composer while that mode is on
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| this.press_begin(ev.position, window, cx)))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                this.press_move(ev.position);
                if this.paint.drag.is_some() { if ev.pressed_button.is_none() { this.paint_drag_end(cx); } else { this.paint_drag_to(ev.position, window, cx); } }
                if this.wall.pen { if ev.pressed_button.is_none() { this.wall_pen_up(); } else { this.wall_pen(ev.position, false, window, cx); } }
                if this.call.resize.is_some() { if ev.pressed_button.is_none() { this.resize_end(window, cx); } else { this.resize_move(f32::from(ev.position.x), window, cx); } } }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| { this.press_end(); if crate::titlebar::released() { cx.notify(); } this.paint_drag_end(cx); this.wall_pen_up(); this.resize_end(window, cx) }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.paint_key(ev, window, cx) { cx.stop_propagation(); return; }
                if this.files_key(ev, window, cx) { cx.stop_propagation(); return; }
                if this.msg_key(ev, window, cx) { cx.stop_propagation(); return; }
                if this.wall.on && this.wall.room_mode && ev.keystroke.key == "escape" { this.wall_off(); cx.notify(); cx.stop_propagation(); return; } this.ptt_key_event(ev.keystroke.key.as_str(), true, cx); }))
            .capture_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| { this.ptt_key_event(ev.keystroke.key.as_str(), false, cx); }))
            // Ctrl/Cmd+V in the box with a picture (or copied files) on the clipboard sends it; text still pastes as text. The box's own Paste
            // action runs at the focused element, so this listens before it, on the way down.
            .capture_action(cx.listener(|this, _: &Paste, window, cx| {
                let fh = this.composer.focus_handle(cx);
                if (fh.is_focused(window) || fh.contains_focused(window, cx)) && this.files_paste(window, cx) { cx.stop_propagation(); }
            }));
        let page: AnyElement = match self.screen {
            Screen::Starting => base.into_any_element(),
            Screen::Welcome => base.child(self.render_onboard(cx)).into_any_element(),
            Screen::Pairing => base.child(self.render_pairing(cx)).into_any_element(),
            Screen::Locked => base.child(self.render_sign_in(cx)).into_any_element(),
            Screen::Main => {
                let body: AnyElement = match self.level.clone() {
                    Level::Home => self.render_home(window, cx),
                    Level::Server(cid) => self.render_server(&cid, window, cx),
                    Level::Chat if phone => self.render_chat(window, cx),
                    Level::Chat => {
                        h_flex().size_full().items_start().child(self.render_rail(window, cx)).child(self.render_chat(window, cx)).into_any_element()
                    }
                    Level::Settings => self.render_settings(window, cx).into_any_element(),
                };
                let deck = self.deck_column(window, cx);
                let show = self.deck_show_button(cx);
                let dock = self.call_dock(window, cx);
                let settings = self.settings_modal(window, cx);
                let pr = if self.dock_on() { crate::calldock::GUTTER } else { 0. };
                let pop = self.call_popover(cx);
                let rings = self.ring_overlay(cx);
                let people = self.members_overlay(cx);
                let picker = self.paint_panel(true, cx);
                let cropper = self.avatar_overlay(cx);
                let lightbox = self.light_overlay(window, cx);
                let save = self.save_overlay(cx);
                let reminder = self.recovery_card(cx);
                base.child(h_flex().size_full().items_start().children(deck).child(div().flex_1().min_w_0().h_full().pr(px(pr)).child(body))).children(show).children(dock).children(pop).children(settings).children(people).children(picker).children(rings).children(cropper).children(save).children(reminder).children(lightbox).children(self.dens_popup(cx)).children(self.room_menu_view(cx)).children(self.gk_modal(window, cx)).children(self.tour_view(window, cx)).children(self.fly_layer()).into_any_element()
            }
        };
        crate::titlebar::frame(window, page, cx.entity())
    }
}

/// Watches the pointer over the room's list (the window's events, so a drag that leaves the list keeps going) and keeps the list's bounds.
fn sel_watch(area: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>, cx: &mut Context<Mega>) -> impl IntoElement {
    let (down, mv, up) = (cx.listener(|this, ev: &MouseDownEvent, window, cx| this.sel_down(ev, window, cx)), cx.listener(|this, ev: &MouseMoveEvent, window, cx| this.sel_move(ev, window, cx)), cx.listener(|this, ev: &MouseUpEvent, window, cx| this.sel_up(ev, window, cx)));
    canvas(|_, _, _| {}, move |b, _, window, _| {
        area.set(b);
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, w, cx| { if phase == DispatchPhase::Capture { down(ev, w, cx); } });
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, w, cx| { if phase == DispatchPhase::Capture { mv(ev, w, cx); } });
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, w, cx| { if phase == DispatchPhase::Capture { up(ev, w, cx); } });
    }).absolute().top_0().left_0().size_full()
}

impl Mega {
    /// The right-click menu over a message: Copy (a selection), Copy message, Select all. Items are read by position in `sel_down`.
    fn sel_menu(&self) -> Option<AnyElement> {
        let (at, id) = self.sel.menu.clone()?;
        let area = self.sel.area.get();
        let (emoji, items) = self.menu_layout(&id);
        let h = if emoji { crate::select::MENU_EMOJI } else { 0. } + crate::select::menu_row() * items.len() as f32 + 8.;
        let o = self.menu_origin(at, h);
        let (x, y) = (f32::from(o.x - area.left()), f32::from(o.y - area.top()));
        let mine: Vec<String> = self.msg_of(&id).map(|m| m.reactions.iter().filter(|r| r.mine).map(|r| r.emoji.clone()).collect()).unwrap_or_default();
        Some(v_flex().id("msg-menu").absolute().left(px(x)).top(px(y)).w(px(crate::select::MENU_W)).py(px(4.)).rounded(px(10.)).bg(rgb(0x1b1922)).border_1().border_color(look::line()).shadow_lg().font_family(look::F_UI).text_size(px(crate::phone::pick(13., 16.)))
            .when(emoji, |d| d.child(h_flex().h(px(crate::select::MENU_EMOJI)).px(px(8.)).items_center().children(crate::msgact::QUICK.iter().map(|e| div().flex().items_center().justify_center().w(px((crate::select::MENU_W - 16.) / 8.)).h(px(36.)).rounded(px(10.)).text_size(px(20.)).when(mine.iter().any(|m| m == e), |d| d.bg(hsla(0., 0., 1., 0.16))).child(e.to_string())))))
            .children(items.into_iter().map(|t| div().h(px(crate::select::menu_row())).px(px(14.)).flex().items_center().when(t == "Delete", |d| d.text_color(rgb(look::DANGER))).child(t))).into_any_element())
    }
}

/// Inside the footer of a row: tells the flight layer where the pills are.
fn flight_spot(at: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |b, _, _| at.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

/// Over the whole row: emoji in the air from the right edge to their pill, then a burst of glyphs where they land (mega.js `throwReaction`).
fn flight_layer(flights: Vec<(usize, crate::arrive::Flight)>, foot: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>, now: f64, hi: Hsla) -> impl IntoElement {
    use crate::arrive::{burst_at, burst_glyph, flight_at, BURST_LIFE, BURST_N, FLIGHT};
    canvas(|_, _, _| (), move |b, _, window, cx| {
        let f = foot.get();
        let to_f = |v: Pixels| f32::from(v);
        for (i, fl) in &flights {
            // pills are about 56 px apart, each about 28 px from its own left edge
            let target = (to_f(f.origin.x) + 28. + 56. * *i as f32, to_f(f.origin.y) + 10.);
            let from = (to_f(b.origin.x + b.size.width) - 40., to_f(b.origin.y) + 4.);
            let t = (now - fl.t0) as f32;
            let paint = |window: &mut Window, cx: &mut App, text: &str, px_size: f32, at: (f32, f32), color: Hsla| {
                let s: SharedString = text.to_string().into();
                let run = TextRun { len: s.len(), font: font(look::F_UI), color, background_color: None, underline: None, strikethrough: None };
                let line = window.text_system().shape_line(s, px(px_size), &[run], None);
                let _ = line.paint(point(px(at.0 - px_size * 0.5), px(at.1 - px_size * 0.55)), px(px_size * 1.2), TextAlign::Left, None, window, cx);
            };
            if t < FLIGHT as f32 {
                let (p, sc) = flight_at(from, target, t / FLIGHT as f32);
                paint(window, cx, &fl.emoji, 20. * sc, p, hi);
            } else {
                let k = (t - FLIGHT as f32) / BURST_LIFE as f32;
                if k < 1. {
                    for n in 0..BURST_N {
                        let (d, a) = burst_at(n, k);
                        let mut c = hi; c.a = a;
                        paint(window, cx, &burst_glyph(n).to_string(), 16., (target.0 + d.0, target.1 + d.1), c);
                    }
                }
            }
        }
    }).absolute().top_0().left_0().size_full()
}
