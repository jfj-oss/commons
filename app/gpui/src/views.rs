//! What is on screen: the community cards, a community's room cards, a room beside its colour dots, and settings.
use crate::app::{clock, Level, Mega, Screen, HEAT_FLOOR};
use crate::ascii::{self, hole, Field, Holes};
use crate::look::{self, Duo};
use crate::model::Msg;
use crate::prefs::{ChatFont, Saver};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

/// Corner of a card (mega.css --m-sq). The web UI uses a true squircle where the browser has one; elsewhere this radius.
const SQ: f32 = 46.;
const RAIL: f32 = 56.;

/// What a card shows.
struct Card { key: String, cid: String, room: String, title: String, sub: String, unread: u64, duo: Duo, fx: usize, big: bool, dim: bool, people: Vec<String>, here: Vec<String> }

fn face_dot(name: &str, size: f32) -> Div {
    div().size(px(size)).flex_none().rounded_full().bg(look::face(name)).border_1().border_color(hsla(0., 0., 0., 0.35))
}
fn avatar(name: &str) -> Div {
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    div().size(px(36.)).flex_none().rounded(px(11.)).bg(look::face(name)).text_color(rgb(0x0c0d12)).font_family(look::F_SHARP).text_size(px(24.)).line_height(px(36.))
        .flex().items_center().justify_center().child(initial)
}

impl Mega {
    fn card(&mut self, c: Card, cx: &mut Context<Self>) -> impl IntoElement {
        let h = if c.big { 220. } else { 150. };
        let (title_px, title_lh) = if c.big { (72., 66.) } else { (48., 46.) };
        let ink = c.duo.ink();
        // the pattern flows around the words: the title and the line under it (their boxes are known from the layout below)
        let title_w = (c.title.chars().count() as f32 * title_px * 0.5 + 24.).min(900.);
        let mut f = Field::card(c.fx, look::h(&c.key), c.duo, if self.still() { 11.7 } else { self.clock + (look::h(&c.key) % 40) as f64 }, !c.big);
        f.radius = SQ;
        f.hi_t = 0.9 - 0.22 * (c.unread.min(12) as f64 / 12.);
        f.holes = vec![
            Bounds::new(point(px(18.), px(h - 30. - title_lh - 18.)), size(px(title_w), px(title_lh + 44.))),
            Bounds::new(point(px(18.), px(14.)), size(px(c.sub.chars().count() as f32 * 7.5 + 16.), px(22.))),
        ];
        let (cid, room, key) = (c.cid.clone(), c.room.clone(), c.key.clone());
        let focus = (c.cid.clone(), c.room.clone());
        let shown: Vec<&String> = c.people.iter().take(crate::presence::SHOW).collect();
        let more = c.people.len().saturating_sub(shown.len());
        div().id(SharedString::from(format!("card-{key}"))).relative().w_full().h(px(h)).flex_none().rounded(px(SQ)).bg(c.duo.field()).overflow_hidden().cursor_pointer()
            .when(c.dim, |d| d.opacity(0.55))
            .child(div().absolute().inset_0().child(ascii::field(f)))
            .child(h_flex().absolute().top(px(14.)).left(px(24.)).right(px(24.)).gap_2().items_center().font_family(look::F_NAME).text_size(px(12.)).text_color(ink)
                .child(div().child(c.sub.clone()))
                .child(div().flex_1())
                // friends looking at this card right now, in their colours
                .children(shown.iter().map(|n| face_dot(n, 12.)))
                .when(more > 0, |d| d.child(format!("+{more}"))))
            .child(h_flex().absolute().bottom(px(20.)).left(px(24.)).right(px(24.)).items_end().gap_3().text_color(ink)
                .child(v_flex().flex_1().min_w_0()
                    .when(!c.here.is_empty(), |d| d.child(h_flex().gap_1().mb_1().font_family(look::F_NAME).text_size(px(12.))
                        .children(c.here.iter().take(5).map(|n| face_dot(n, 14.))).child(format!("{} in the call", c.here.join(", ")))))
                    .child(div().font_family(look::F_SHARP).text_size(px(title_px)).line_height(px(title_lh)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(c.title.clone())))
                .when(c.unread > 0, |d| d.child(div().flex_none().font_family(look::F_SHARP).text_size(px(if c.big { 120. } else { 72. })).line_height(px(if c.big { 96. } else { 60. })).opacity(0.85)
                    .child(if c.unread > 99 { "99+".to_string() } else { c.unread.to_string() }))))
            .on_hover(cx.listener(move |this, hovered: &bool, _, _| { if *hovered { this.focus_card(Some(focus.clone())); } else if this.pres.cand.as_ref() == Some(&focus) { this.focus_card(None); } }))
            .on_click(cx.listener(move |this, _, window, cx| { if room.is_empty() { this.go(Level::Server(cid.clone()), window, cx) } else { this.open_room(cid.clone(), room.clone(), window, cx) } }))
    }

    fn top_bar(&mut self, title: String, back: Option<Level>, cx: &mut Context<Self>) -> impl IntoElement {
        let net = self.snap.as_ref().map(|s| s.net.clone()).unwrap_or_default();
        h_flex().flex_none().w_full().px_6().pt_5().pb_3().gap_3().items_end()
            .when_some(back, |d, to| d.child(div().id("back").size(px(22.)).mb_2().rounded(px(6.)).bg(look::paper()).cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.go(to.clone(), window, cx)))))
            .child(div().font_family(look::F_SHARP).text_size(px(48.)).line_height(px(44.)).child(title))
            .child(div().flex_1())
            .when(net != "online", |d| d.child(div().mb_1().font_family(look::F_NAME).text_size(px(12.)).text_color(rgb(look::DANGER)).child(if net == "connecting" { "connecting…".to_string() } else { "offline".to_string() })))
            .when(self.still(), |d| d.child(div().mb_1().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child("battery saver")))
            .child(Button::new("settings").ghost().small().label("settings").on_click(cx.listener(|this, _, window, cx| this.go(Level::Settings, window, cx))))
    }

    fn render_home(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snap.clone().unwrap_or_default();
        let mut col = v_flex().id("home").flex_1().min_h_0().w_full().overflow_y_scroll().px_6().pb_6().gap_4();
        for c in snap.communities.iter().filter(|c| !c.removed) {
            let here: Vec<String> = c.channels.iter().filter(|x| x.kind == "voice").filter_map(|x| self.pres.calls.get(&x.gid)).flatten().cloned().collect();
            let card = Card { key: c.id.clone(), cid: c.id.clone(), room: String::new(), title: c.name.clone(), sub: format!("{} rooms · {} people", c.channels.len(), c.members), unread: c.unread,
                duo: Duo::of(&c.id, c.colours.as_ref()), fx: ascii::auto_fx(&c.name), big: true, dim: false, people: self.people_on(&c.id, ""), here };
            col = col.child(self.card(card, cx));
        }
        if snap.communities.iter().all(|c| c.removed) {
            col = col.child(div().py_8().text_color(look::mute()).child("No communities yet. Creating one and joining with an invite are not in this build yet: use the web or Tauri app for that."));
        }
        // people: one-to-one conversations, named after the other person
        if !snap.direct.is_empty() {
            col = col.child(div().mt_2().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child("people"));
            for d in &snap.direct {
                let (cid, room, on) = (d.id.clone(), d.room.clone(), d.channels.iter().any(|ch| ch.gid == d.room && ch.joined));
                col = col.child(h_flex().id(SharedString::from(format!("dm-{}", d.id))).flex_none().gap_3().px_3().py_2().rounded(px(18.)).bg(rgb(look::SLIP)).border_1().border_color(look::line()).items_center().cursor_pointer()
                    .when(!on, |x| x.opacity(0.5))
                    .child(avatar(&d.name))
                    .child(div().flex_1().font_family(look::F_SHARP).text_size(px(24.)).line_height(px(24.)).child(d.name.clone()))
                    .child(div().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(if on { "" } else { "waiting for them" }))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_room(cid.clone(), room.clone(), window, cx))));
            }
        }
        let me = snap.me_name.clone();
        v_flex().size_full().child(self.top_bar(if me.is_empty() { "mega".into() } else { format!("mega · {me}") }, None, cx)).child(col)
    }

    fn render_server(&mut self, cid: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(c) = self.snap.as_ref().and_then(|s| s.community(cid)).cloned() else { return v_flex().size_full().child(self.top_bar("…".into(), Some(Level::Home), cx)) };
        let mut cards: Vec<AnyElement> = vec![];
        for ch in &c.channels {
            let voice = ch.kind == "voice";
            let here = self.pres.calls.get(&ch.gid).cloned().unwrap_or_default();
            let card = Card { key: ch.gid.clone(), cid: c.id.clone(), room: ch.gid.clone(), title: ch.name.clone(),
                sub: if !ch.joined { "not let in yet".into() } else if voice { "voice".into() } else { "text".to_string() },
                unread: ch.unread, duo: Duo::of(&ch.gid, c.room_colours.iter().find(|(g, _)| g == &ch.gid).map(|(_, d)| d)), fx: ascii::auto_fx(&ch.name), big: false, dim: !ch.joined,
                people: self.people_on(&c.id, &ch.gid), here };
            cards.push(self.card(card, cx).into_any_element());
        }
        // two cards to a row
        let mut grid = v_flex().id("server").flex_1().min_h_0().w_full().overflow_y_scroll().px_6().pb_6().gap_4();
        let mut it = cards.into_iter();
        while let Some(a) = it.next() {
            let b = it.next();
            grid = grid.child(h_flex().flex_none().w_full().gap_4().child(div().flex_1().min_w_0().child(a)).child(div().flex_1().min_w_0().children(b)));
        }
        v_flex().size_full().child(self.top_bar(c.name.clone(), Some(Level::Home), cx)).child(grid)
    }

    /// The colour dots beside a chat: home, the community's cards, then one dot per room (the open one is a pill).
    fn render_rail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = v_flex().id("rail").w(px(RAIL)).h_full().flex_none().items_center().pt_5().gap_3().bg(rgb(look::VOID));
        col = col.child(div().id("rail-home").size(px(20.)).rounded(px(6.)).bg(look::paper()).cursor_pointer().on_click(cx.listener(|this, _, window, cx| this.go(Level::Home, window, cx))));
        let Some(c) = self.cid.as_ref().and_then(|cid| self.snap.as_ref()?.community(cid)).cloned() else { return col };
        if !c.direct {
            let (cid, duo) = (c.id.clone(), Duo::of(&c.id, c.colours.as_ref()));
            col = col.child(div().id("rail-server").size(px(16.)).rounded(px(5.)).bg(duo.dot()).cursor_pointer().on_click(cx.listener(move |this, _, window, cx| this.go(Level::Server(cid.clone()), window, cx))));
        }
        col = col.child(div().h(px(18.)));
        for ch in c.channels.iter().filter(|ch| ch.joined) {
            let (cid, gid) = (c.id.clone(), ch.gid.clone());
            let dot = Duo::of(&ch.gid, c.room_colours.iter().find(|(g, _)| g == &ch.gid).map(|(_, d)| d)).dot();
            let on = self.gid.as_deref() == Some(ch.gid.as_str());
            // unread: a ring in the room's colour around a dark centre; a voice room: a hollow dot
            let ring = ch.unread > 0 && !on;
            col = col.child(div().id(SharedString::from(format!("dot-{}", ch.gid))).w(px(14.)).h(px(if on { 38. } else { 14. })).rounded_full().cursor_pointer()
                .when(!ring && ch.kind != "voice", |d| d.bg(dot))
                .when(ring || ch.kind == "voice", |d| d.border_color(dot).when(ring, |d| d.border_4()).when(!ring, |d| d.border_2()))
                .on_click(cx.listener(move |this, _, window, cx| this.open_room(cid.clone(), gid.clone(), window, cx))));
        }
        col
    }

    fn render_msg(&mut self, ix: usize) -> AnyElement {
        let Some(m) = self.win.msgs.get(ix) else { return div().into_any_element() };
        let prev = ix.checked_sub(1).and_then(|i| self.win.msgs.get(i));
        row(m, prev, self.chat_font(), &self.holes)
    }

    fn render_chat(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(gid) = self.gid.clone() else { return div().flex_1().into_any_element() };
        let found = self.channel(&gid).map(|(c, ch)| (if c.direct { c.name.clone() } else { ch.name.clone() }, if c.direct { "just the two of you".to_string() } else { format!("{} · {} people", c.name, c.members) }));
        let Some((title, sub)) = found else { return v_flex().flex_1().h_full().items_center().justify_center().text_color(look::mute()).child("This room is gone.").into_any_element() };
        let duo = self.room_duo(&gid);
        let typing = match self.win.typing.len() { 0 => String::new(), 1 => format!("{} is writing…", self.win.typing[0]), n => format!("{n} people are writing…") };
        let (font, size, lh) = self.chat_font();
        // The room's dim backdrop: its pattern in its two colours, warmer after activity, with rings travelling through it.
        let backdrop = Field { fx: ascii::auto_fx(&title), seed: look::h(&gid), duo, t: self.bd_t, cell: (10., 17.), font_px: 14., gate: 0.46 - 0.26 * self.heat.max(HEAT_FLOOR), hi_t: 0.86, alpha: 0.42, radius: 0.,
            ripples: if self.still() { vec![] } else { self.ripples.clone() }, holes: vec![], live_holes: Some(self.holes.clone()) };
        let holes = self.holes.clone();
        v_flex().flex_1().h_full().min_w_0().relative().bg(rgb(0x14111a))
            .child(div().absolute().inset_0().child(ascii::field(backdrop)))
            .child(v_flex().flex_none().items_start().px_5().pt_3().pb_1()
                .child(div().relative().font_family(look::F_SHARP).text_size(px(36.)).line_height(px(36.)).child(title.clone()).child(hole(&holes)))
                .child(div().relative().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(sub).child(hole(&holes))))
            .child(div().flex_1().min_h_0().relative()
                .child(list(self.list.clone(), cx.processor(|this, ix: usize, _, _| this.render_msg(ix))).size_full())
                .child(div().absolute().top_0().right_0().bottom_0().w(px(12.)).child(Scrollbar::vertical(&self.list))))
            .child(h_flex().flex_none().h(px(18.)).px_5().child(div().relative().font_family(look::F_NAME).text_size(px(12.)).text_color(duo.dot()).when(!typing.is_empty(), |d| d.child(hole(&holes))).child(typing)))
            .child(div().relative().child(hole(&holes)).flex_none().mx_4().mb_4().px_2().py_1().rounded(px(22.)).bg(hsla(0.71, 0.22, 0.07, 0.93)).border_1().border_color(look::line())
                .font_family(font)
                .child(Textarea::new(&self.composer).appearance(false).text_size(px(size)).line_height(px(lh))))
            .into_any_element()
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snap.clone().unwrap_or_default();
        let back = if self.gid.is_some() { Level::Chat } else { Level::Home };
        let label = |t: &str| div().mt_4().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(t.to_string());
        let note = |t: &str| div().text_size(px(13.)).text_color(look::mute()).child(t.to_string());
        // a row of choices, one of them on
        fn choice(id: &'static str, on: bool, text: &'static str) -> Stateful<Div> {
            div().id(id).px_3().py_1().rounded_full().border_1().cursor_pointer().border_color(if on { look::paper() } else { look::line() })
                .when(on, |d| d.bg(look::paper()).text_color(rgb(0x0c0d12))).child(text)
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

/// One message. `prev` decides whether the face and name are shown again (same person within five minutes: no).
pub fn row(m: &Msg, prev: Option<&Msg>, (font, size, lh): (&'static str, f32, f32), holes: &Holes) -> AnyElement {
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
        else { div().relative().child(hole(holes)).max_w_full().font_family(font).text_size(px(size)).line_height(px(lh)).child(m.text.clone()).into_any_element() };
    col = col.child(body);
    let mut foot = h_flex().relative().gap_1().flex_wrap();
    for r in &m.reactions {
        foot = foot.child(div().px_1p5().rounded_full().border_1().border_color(if r.mine { look::paper() } else { look::line() }).text_size(px(13.)).child(format!("{} {}", r.emoji, r.count)));
    }
    if m.edited { foot = foot.child(div().text_color(look::mute()).text_size(px(11.)).child("edited")); }
    if m.mine && (m.status == "pending" || m.status == "failed") { foot = foot.child(div().text_color(look::mute()).text_size(px(11.)).child(m.status.clone())); }
    if !m.read_by.is_empty() { foot = foot.child(div().text_color(look::mute()).text_size(px(11.)).child(format!("read by {}", m.read_by.join(", ")))); }
    let has_foot = !m.reactions.is_empty() || m.edited || !m.read_by.is_empty() || (m.mine && (m.status == "pending" || m.status == "failed"));
    if has_foot { foot = foot.child(hole(holes)); }
    h_flex().w_full().items_start().gap_3().px_5().pb_1().when(head, |d| d.pt_3())
        .child(if head { avatar(&m.name).mt_1() } else { div().w(px(36.)).flex_none() })
        .child(col.child(foot)).into_any_element()
}

impl Render for Mega {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !std::mem::replace(&mut self.first_frame_done, true) { window.on_next_frame(|_, _| crate::bench::mark("first_frame")); }
        crate::bench::drive(self, window, cx);
        self.holes.borrow_mut().clear();
        let base = div().id("root").size_full().bg(rgb(look::VOID)).text_color(look::paper()).font_family(look::F_UI).text_size(px(15.))
            // dragging the column's edge, and push-to-talk: the key never reaches the composer while that mode is on
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| { if this.call.resize.is_some() { this.resize_move(f32::from(ev.position.x), window, cx); } }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| this.resize_end(window, cx)))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| { this.ptt_key_event(ev.keystroke.key.as_str(), true, cx); }))
            .capture_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| { this.ptt_key_event(ev.keystroke.key.as_str(), false, cx); }));
        match self.screen {
            Screen::Starting => base,
            Screen::Welcome | Screen::Locked => base.child(self.render_sign_in(cx)),
            Screen::Main => {
                let body: AnyElement = match self.level.clone() {
                    Level::Home => self.render_home(cx).into_any_element(),
                    Level::Server(cid) => self.render_server(&cid, cx).into_any_element(),
                    Level::Chat => {
                        let deck = self.deck_column(window, cx);
                        h_flex().size_full().items_start().child(self.render_rail(cx)).children(deck).child(self.render_chat(cx)).into_any_element()
                    }
                    Level::Settings => self.render_settings(cx).into_any_element(),
                };
                let bar = self.call_bar(cx);
                let pop = self.call_popover(cx);
                base.child(v_flex().size_full().child(div().flex_1().min_h_0().w_full().child(body)).children(bar)).children(pop)
            }
        }
    }
}
