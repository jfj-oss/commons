//! What is on screen: the community cards, a community's room cards, a room beside its colour dots, and settings.
use crate::app::{clock, Level, Mega, Screen};
use crate::ascii::{self, hole, Holes};
use crate::look::{self, Duo};
use crate::cards::chip;
use crate::model::Msg;
use crate::prefs::{ChatFont, Saver};
use crate::squircle as sq;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;
use std::rc::Rc;

/// Corner of a card (mega.css --m-sq is 76 px on the web's big cards, a true squircle, which reads rounder at the same number than a circle does).
const SQ: f32 = 60.;

pub(crate) fn face_dot(name: &str, size: f32) -> Div {
    div().size(px(size)).flex_none().rounded_full().bg(look::face(name)).border_1().border_color(hsla(0., 0., 0., 0.35))
}
fn avatar(name: &str) -> Div {
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    div().relative().size(px(36.)).flex_none().text_color(rgb(0x0c0d12)).font_family(look::F_SHARP).text_size(px(24.)).line_height(px(36.))
        .flex().items_center().justify_center().child(sq::back(11., look::face(name))).child(div().relative().child(initial))
}

impl Mega {
    /// The page's `.mbar`: the mark (or the community's name) at the left, a back chip before it, chips at the right.
    pub(crate) fn top_bar(&mut self, title: String, back: Option<Level>, cx: &mut Context<Self>) -> impl IntoElement {
        let net = self.snap.as_ref().map(|s| s.net.clone()).unwrap_or_default();
        let mut extra: Vec<AnyElement> = vec![];
        // the colours and draw chips belong to the focused card (the page's `[data-m="paint"]` and `[data-m="draw"]`)
        if matches!(self.level, Level::Home | Level::Server(_)) {
            if self.nav.focus_card.as_ref().is_some_and(|t| self.can_paint(t)) {
                extra.push(chip("b-paint", "colours").on_click(cx.listener(|this, _, window, cx| this.paint_toggle(window, cx))).into_any_element());
            }
            if self.wall.key.as_ref().is_some_and(|k| self.wall_drawable(k)) {
                extra.push(chip("b-draw", if self.wall.on { "drawing" } else { "draw" }).on_click(cx.listener(|this, _, window, cx| this.wall_toggle(window, cx))).into_any_element());
            }
        }
        extra.extend(match self.level.clone() { Level::Server(cid) => self.community_buttons(&cid, cx), Level::Home => self.home_buttons(cx), _ => vec![] });
        h_flex().flex_none().w_full().px(px(16.)).py(px(12.)).gap(px(10.)).items_center()
            .when_some(back, |d, to| d.child(chip("back", "\u{2190} back").on_click(cx.listener(move |this, _, window, cx| this.go(to.clone(), window, cx)))))
            .child(div().font_family(look::F_SHARP).text_size(px(36.)).line_height(px(36.)).max_w(px(560.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(title))
            .when(net != "online", |d| d.child(div().font_family(look::F_UI).text_size(px(13.)).text_color(rgb(look::DANGER)).child(if net == "connecting" { "connecting…".to_string() } else { "offline".to_string() })))
            .when(self.still(), |d| d.child(div().font_family(look::F_UI).text_size(px(13.)).text_color(look::mute()).child("battery saver")))
            .child(div().flex_1())
            .children(extra)
            .child(chip("settings", "settings").on_click(cx.listener(|this, _, window, cx| this.go(Level::Settings, window, cx))))
    }

    /// An owner's choice of colours for the open room: three bold presets, and the default back.
    fn colour_picker(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut row = h_flex().relative().gap_2().items_center().pt_1();
        if self.colours_open {
            for (i, (_name, main, hi)) in look::PRESETS.iter().enumerate() {
                let (m, h) = (*main, *hi);
                row = row.child(div().id(SharedString::from(format!("preset-{i}"))).size(px(30.)).rounded_full().cursor_pointer().overflow_hidden().border_1().border_color(look::line())
                    .child(div().size_full().bg(linear_gradient(135., linear_color_stop(rgb(m), 0.5), linear_color_stop(rgb(h), 0.5))))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_room_colours(Some((m, h)), window, cx))));
            }
            row = row.child(div().id("preset-default").px_2().rounded_full().border_1().border_color(look::line()).font_family(look::F_NAME).text_size(px(12.)).cursor_pointer().child("default")
                .on_click(cx.listener(|this, _, window, cx| this.set_room_colours(None, window, cx))));
        }
        row.child(div().id("preset-open").px_2().py_0p5().rounded_full().border_1().border_color(look::line()).font_family(look::F_NAME).text_size(px(12.)).cursor_pointer().child("colours")
            .on_click(cx.listener(|this, _, _, cx| { this.colours_open = !this.colours_open; cx.notify(); })))
    }

    fn render_msg(&mut self, ix: usize) -> AnyElement {
        let Some(m) = self.win.msgs.get(ix) else { return div().into_any_element() };
        let prev = ix.checked_sub(1).and_then(|i| self.win.msgs.get(i));
        let star = self.dm_marks.0 == self.gid.clone().unwrap_or_default() && self.dm_marks.1 > 0 && ix + self.dm_marks.1 >= self.win.msgs.len() && !m.mine;
        let me = self.snap.as_ref().map(|s| s.me_name.as_str()).unwrap_or("");
        let now = self.now_secs();
        let fx = RowFx {
            tier_px: crate::arrive::tier_px(crate::arrive::size_tier(m, me)),
            arrive: self.arrivals.progress(&m.id, now).map(|k| (k, (now * 60.) as u64)),
            hi: rgb(self.room_duo(self.gid.as_deref().unwrap_or("")).hi).into(),
            flights: self.flights.live.iter().filter(|f| f.msg == m.id).cloned().collect(),
            now,
        };
        row(m, prev, self.chat_font(), &self.holes, star, &fx)
    }

    fn render_chat(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(gid) = self.gid.clone() else { return div().flex_1().into_any_element() };
        let found = self.channel(&gid).map(|(c, ch)| (if c.direct { c.name.clone() } else { ch.name.clone() }, if c.direct { "just the two of you".to_string() } else { format!("{} · {} people", c.name, c.members) }));
        let Some((title, sub)) = found else { return v_flex().flex_1().h_full().items_center().justify_center().text_color(look::mute()).child("This room is gone.").into_any_element() };
        let duo = self.room_duo(&gid);
        let is_dm = self.channel(&gid).is_some_and(|(c, _)| c.direct);
        let owner = self.channel(&gid).is_some_and(|(c, _)| !c.direct && (c.role == "owner" || c.role == "admin"));
        let chat_cid = self.channel(&gid).map(|(c, _)| c.id.clone());
        let typing = match self.win.typing.len() { 0 => String::new(), 1 => format!("{} is writing…", self.win.typing[0]), n => format!("{n} people are writing…") };
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
        v_flex().flex_1().h_full().min_w_0().relative().bg(rgb(bd_back))
            .children(backdrop.map(|b| div().absolute().inset_0().child(b)))
            .child(wall_layer)
            .child(h_flex().flex_none().items_start().px_5().pt_3().pb_1()
                .child(v_flex().items_start()
                    .child(div().relative().font_family(look::F_SHARP).text_size(px(36.)).line_height(px(36.)).child(title.clone()).child(hole(&holes)))
                    .child(div().relative().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(sub).child(hole(&holes))))
                .child(div().flex_1())
                .children(wall_btn)
                .children(if is_dm { self.call_person_button(&gid, cx) } else { None })
                .when(!is_dm, |d| d.children(chat_cid.map(|cid| self.community_buttons(&cid, cx)).unwrap_or_default()))
                .when(owner, |d| d.child(self.colour_picker(cx))))
            .child(div().flex_1().min_h_0().relative()
                .child(list(self.list.clone(), cx.processor(|this, ix: usize, _, _| this.render_msg(ix))).size_full())
                .child(div().absolute().top_0().right_0().bottom_0().w(px(12.)).child(Scrollbar::vertical(&self.list))))
            .child(h_flex().flex_none().h(px(18.)).px_5().child(div().relative().font_family(look::F_NAME).text_size(px(12.)).text_color(duo.dot()).when(!typing.is_empty(), |d| d.child(hole(&holes))).child(typing)))
            .child(div().relative().flex_none().mx_4().mb_4().px_2().py_1().border_1().border_color(hsla(0., 0., 0., 0.))
                .font_family(font)
                // a translucent slip over the backdrop: a squircle fill and border, never a cut (the backdrop moves behind it)
                .child(sq::back(22., hsla(0.71, 0.22, 0.07, 0.93))).child(hole(&holes))
                .child(Textarea::new(&self.composer).appearance(false).text_size(px(size)).line_height(px(lh)))
                .child(sq::front(22., None, Some((1., look::line())), None)))
            .children(wall_hit)
            .children(wall_bar)
            .into_any_element()
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snap.clone().unwrap_or_default();
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
                        "set_listening_sharing" => this.set_setting(method, json!({"on": v, "rune": ""}), window, cx),
                        _ => this.set_setting(method, json!({"on": v}), window, cx),
                    }
                });
            })
        };
        let body = v_flex().id("settings").flex_1().min_h_0().w_full().overflow_y_scroll().px_6().pb_8().gap_2().max_w(px(640.))
            .child(label("your name"))
            .child(h_flex().gap_2().child(div().w(px(280.)).child(Input::new(&self.set_name)))
                .child(Button::new("save-name").label("Save").on_click(cx.listener(|this, _, window, cx| { let n = this.set_name.read(cx).value().to_string(); this.set_setting("set_name", json!({"name": n}), window, cx); }))))
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
            .child(label("notifications"))
            .child(h_flex().gap_2()
                .child(choice("nm-all", mode == "all", "Every message").on_click(set_mode("all")))
                .child(choice("nm-men", mode == "mentions", "Mentions only").on_click(set_mode("mentions")))
                .child(choice("nm-off", mode == "off", "Off").on_click(set_mode("off"))))
            .child(toggle("nt-text", show_text, "Show the message text in notifications", "notify_set"))
            .child(toggle("nt-ring", self.prefs.sound, "Play a sound when someone rings", "ring_sound"))
            .child(label("privacy"))
            .child(toggle("p-typing", snap.typing_enabled, "Show others when I am writing", "set_typing_enabled"))
            .child(toggle("p-read", snap.read_receipts, "Read receipts", "set_read_receipts"))
            .child(toggle("p-pres", presence, "Show where I am on the cards", "presence"))
            .child(toggle("p-listen", snap.share_listening, "Share what I am listening to", "set_listening_sharing"))
            .child(label("this device"))
            .child(note(&format!("Relay: {}", snap.relay)))
            .child(note(&format!("Account folder: {}", self.core.dir.display())))
            .child(note(&format!("mega-gpui {} (spike), core {}", env!("CARGO_PKG_VERSION"), "0.3.11")))
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
pub struct RowFx { pub tier_px: Option<f32>, pub arrive: Option<(f64, u64)>, pub hi: Hsla, pub flights: Vec<crate::arrive::Flight>, pub now: f64 }

/// One message. `prev` decides whether the face and name are shown again (same person within five minutes: no).
pub fn row(m: &Msg, prev: Option<&Msg>, (font, size, lh): (&'static str, f32, f32), holes: &Holes, star: bool, fx: &RowFx) -> AnyElement {
    let (size, lh) = match fx.tier_px { Some(p) => (p, (p * 1.05).round()), None => (size, lh) };
    if m.kind == "member" || m.kind == "notice" {
        return h_flex().w_full().pl(px(68.)).pr_4().py_1().child(div().relative().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(m.text.clone()).child(hole(holes))).into_any_element();
    }
    let head = !prev.is_some_and(|p| p.sender == m.sender && p.kind != "member" && p.kind != "notice" && m.ts.saturating_sub(p.ts) < 300);
    // every piece is as wide as its words (items_start), so the backdrop is cut out around the words and not the row
    let mut col = v_flex().flex_1().min_w_0().items_start();
    if head {
        col = col.child(h_flex().relative().child(hole(holes)).gap_2().items_baseline().font_family(look::F_NAME).text_size(px(12.))
            .child(div().text_color(look::face(&m.name)).child(m.name.clone()))
            .child(div().text_color(look::mute()).child(clock(m.ts))));
    }
    if let Some((who, text)) = &m.reply {
        col = col.child(div().relative().child(hole(holes)).max_w_full().pl_2().border_l_2().border_color(look::line()).text_color(look::mute()).font_family(font).text_size(px(size * 0.7)).child(format!("{who}: {text}")));
    }
    let body: AnyElement = if m.deleted { div().relative().child(hole(holes)).text_color(look::mute()).text_size(px(16.)).child("(deleted)").into_any_element() }
        else if let Some((name, bytes)) = &m.file { div().relative().child(hole(holes)).max_w_full().text_color(look::mute()).text_size(px(16.)).child(format!("[file] {name} ({} KB){}", bytes / 1024, if m.text.is_empty() { String::new() } else { format!("  {}", m.text) })).into_any_element() }
        else if let Some((k, tick)) = fx.arrive {
            // the real words stay where they are (invisible, so the row keeps its size); a copy made of glyphs sits over them and resolves
            let (copy, real) = crate::arrive::compose(&m.text, k, tick);
            let ink = look::paper();
            div().relative().child(hole(holes)).max_w_full().font_family(font).text_size(px(size)).line_height(px(lh))
                .child(div().text_color(hsla(0., 0., 0., 0.)).child(m.text.clone()))
                .child(div().absolute().top_0().left_0().size_full().overflow_hidden().text_color(fx.hi)
                    .child(StyledText::new(copy).with_highlights([(0..real, HighlightStyle { color: Some(ink), ..Default::default() })])))
                .into_any_element()
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
        let pill = div().relative().px_1p5().border_1().border_color(hsla(0., 0., 0., 0.)).text_size(px(13.)).child(format!("{} {}", r.emoji, shown.unwrap_or(r.count)))
            .child(sq::front(999., None, Some((1., if r.mine { look::paper() } else { look::line() })), None));
        foot = foot.child(if shown.is_none() { pill.opacity(0.) } else { pill });
    }
    if !flying.is_empty() { foot = foot.child(flight_spot(landing.clone())); }
    if m.edited { foot = foot.child(div().text_color(look::mute()).text_size(px(11.)).child("edited")); }
    if m.mine && (m.status == "pending" || m.status == "failed") { foot = foot.child(div().text_color(look::mute()).text_size(px(11.)).child(m.status.clone())); }
    if !m.read_by.is_empty() { foot = foot.child(div().text_color(look::mute()).text_size(px(11.)).child(format!("read by {}", m.read_by.join(", ")))); }
    let has_foot = !m.reactions.is_empty() || m.edited || !m.read_by.is_empty() || (m.mine && (m.status == "pending" || m.status == "failed"));
    if has_foot { foot = foot.child(hole(holes)); }
    h_flex().relative().w_full().items_start().gap_3().px_5().pb_1().when(head, |d| d.pt_3())
        .when(!flying.is_empty(), |d| d.child(flight_layer(flying, landing, fx.now, fx.hi)))
        .when(star, |d| d.child(div().absolute().left(px(5.)).top(px(if head { 22. } else { 8. })).text_size(px(13.)).text_color(look::paper()).opacity(0.8).child("✦")))
        .child(if head { avatar(&m.name).mt_1() } else { div().w(px(36.)).flex_none() })
        .child(col.child(foot)).into_any_element()
}

impl Render for Mega {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !std::mem::replace(&mut self.first_frame_done, true) { window.on_next_frame(|_, _| crate::bench::mark("first_frame")); }
        crate::bench::drive(self, window, cx);
        // arrivals and thrown reactions ask for a frame only while one is in the air; the frame after the last one draws the finished state
        if !(self.arrivals.is_idle() && self.flights.is_idle()) {
            let now = self.now_secs();
            self.arrivals.step(now); self.flights.step(now);
            window.request_animation_frame();
        }
        self.holes.borrow_mut().clear();
        let base = div().id("root").size_full().bg(rgb(look::VOID)).text_color(look::paper()).font_family(look::F_UI).text_size(px(15.))
            // dragging the column's edge, and push-to-talk: the key never reaches the composer while that mode is on
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                if this.paint.drag.is_some() { if ev.pressed_button.is_none() { this.paint_drag_end(cx); } else { this.paint_drag_to(ev.position, window, cx); } }
                if this.wall.pen { if ev.pressed_button.is_none() { this.wall_pen_up(); } else { this.wall_pen(ev.position, false, window, cx); } }
                if this.call.resize.is_some() { if ev.pressed_button.is_none() { this.resize_end(window, cx); } else { this.resize_move(f32::from(ev.position.x), window, cx); } } }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| { this.paint_drag_end(cx); this.wall_pen_up(); this.resize_end(window, cx) }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| { if this.wall.on && this.wall.room_mode && ev.keystroke.key == "escape" { this.wall_off(); cx.notify(); cx.stop_propagation(); return; } this.ptt_key_event(ev.keystroke.key.as_str(), true, cx); }))
            .capture_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| { this.ptt_key_event(ev.keystroke.key.as_str(), false, cx); }));
        match self.screen {
            Screen::Starting => base,
            Screen::Welcome | Screen::Locked => base.child(self.render_sign_in(cx)),
            Screen::Main => {
                let body: AnyElement = match self.level.clone() {
                    Level::Home => self.render_home(window, cx),
                    Level::Server(cid) => self.render_server(&cid, window, cx),
                    Level::Chat => {
                        let deck = self.deck_column(window, cx);
                        h_flex().size_full().items_start().child(self.render_rail(window, cx)).children(deck).child(self.render_chat(cx)).into_any_element()
                    }
                    Level::Settings => self.render_settings(cx).into_any_element(),
                };
                let bar = self.call_bar(cx);
                let pop = self.call_popover(cx);
                let rings = self.ring_overlay(cx);
                let people = self.members_overlay(cx);
                base.child(v_flex().size_full().child(div().flex_1().min_h_0().w_full().child(body)).children(bar)).children(pop).children(people).children(rings)
            }
        }
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
