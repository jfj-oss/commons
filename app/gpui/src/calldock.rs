//! The call's controls as the web draws them in mega on a computer (mega.css `#call-dock.standalone`, `#call-ppl`): a narrow card
//! on the right edge in the voice room's two colours (a square mark, the room's name running upward, the lock, one icon button
//! each for mute, deafen, share, camera, settings, ring and leave), and under it one square picture per person, the one talking
//! ringed in the room's highlight. The window makes room for it on the right. Notes (errors, the song playing on rune) sit to
//! the left of the card, as the page's `.cd-note`.
//! The engine does everything; this file draws its snapshot and sends commands.
use crate::app::Mega;
use crate::calls::engine::{self, Command};
use crate::icons::{self, Icon};
use crate::look;
use crate::squircle as sq;
use crate::vtext;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// The width the window gives up to the call card on the right (mega.css `html.mega.mg-dock #app { padding-right: 72px }`).
pub const GUTTER: f32 = 72.;
pub const DOCK_W: f32 = 60.;
pub const BTN_W: f32 = 48.;
pub const BTN_H: f32 = 32.;
/// A person's square.
pub const FACE: f32 = 48.;

/// Where each control was last painted (window pixels), by name: what a check with real pointer events clicks on.
#[derive(Clone, Default)]
pub struct Hits(pub Rc<RefCell<HashMap<String, (f32, f32, f32, f32)>>>);
impl Hits {
    pub fn clear(&self) { self.0.borrow_mut().clear(); }
    pub fn get(&self, k: &str) -> Option<(f32, f32, f32, f32)> { self.0.borrow().get(k).copied() }
    pub fn dump(&self) -> serde_json::Value {
        serde_json::Value::Object(self.0.borrow().iter().map(|(k, v)| (k.clone(), serde_json::json!({"x": v.0, "y": v.1, "w": v.2, "h": v.3}))).collect())
    }
    /// An invisible layer filling its (relative) parent that tells the book where the parent is.
    pub fn track(&self, key: impl Into<String>) -> impl IntoElement {
        let (book, key) = (self.0.clone(), key.into());
        canvas(|_, _, _| (), move |b, _, _, _| { book.borrow_mut().insert(key.clone(), (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))); })
            .absolute().top_0().left_0().size_full()
    }
}

fn rgba(c: u32, a: f32) -> Hsla { let mut h: Hsla = rgb(c).into(); h.a = a; h }

/// One person's square: their picture where they have one, else their letter on the colour their name gives. The call's
/// own function for a face (the page's `.cp .seal`), so a picture takes its place here and nowhere else.
pub fn face(account: Option<&str>, name: &str, size: f32, r: f32) -> Div {
    if let Some(pic) = account.and_then(|a| crate::avatars::pic_view(a, size)) { return div().relative().size(px(size)).flex_none().child(sq::back(r, rgb(0x15151b))).child(pic); }
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    div().relative().size(px(size)).flex_none().text_color(rgb(0x0c0d12)).font_family(look::F_SHARP).text_size(px(size * 0.5)).line_height(px(size))
        .flex().items_center().justify_center().child(sq::back(r, look::face(name))).child(div().relative().child(initial))
}

impl Mega {
    /// Show the card while a call is open (the page's `inCall`).
    pub fn dock_on(&self) -> bool { !matches!(self.call.snap.phase.as_str(), "" | "idle") }

    fn dock_button(&self, id: &'static str, icon: Icon, label: String, on: bool, leave: bool, duo: &look::Duo, _cx: &mut Context<Self>) -> Stateful<Div> {
        let paper: Hsla = rgb(0xefedf2).into();
        let dark: Hsla = rgb(0x050407).into();
        let (bg, ink) = if leave { (rgb(duo.hi).into(), look::ink_on(duo.hi)) } else if on { (paper, dark) } else { (rgba(0x050407, 0.86), paper) };
        let hits = self.call.hits.clone();
        div().id(id).relative().flex_none().w(px(BTN_W)).h(px(BTN_H)).flex().items_center().justify_center().cursor_pointer()
            .child(sq::back(22., bg)).child(div().relative().child(icons::icon(icon, 16., ink))).child(hits.track(id))
            .hover(|d| d.opacity(0.88))
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
    }

    /// The card, with the people under it. `None` when there is no call.
    pub fn call_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.dock_on() { return None; }
        let snap: engine::Snapshot = self.call.snap.clone();
        let gid = snap.gid.clone().unwrap_or_default();
        let duo = self.room_duo(&gid);
        let (main, hi) = (duo.field(), rgb(duo.hi));
        let ink = duo.ink();
        let (muted, deaf) = (snap.muted || snap.game_muted, snap.deafened);
        let sharing = snap.share.state != "idle" && !snap.share.state.is_empty();
        let cam_on = !matches!(snap.camera.state.as_str(), "" | "off");
        let ptt = snap.push_to_talk;
        let name = if snap.room_name.is_empty() { "Call".to_string() } else { snap.room_name.clone() };
        let hits = self.call.hits.clone();

        // top: the square mark, the room's name running upward, the lock
        let name_len = vtext::length(&name, 24., -0.024).min(150.);
        let name_box = {
            let (n, ink2) = (name.clone(), ink);
            canvas(|_, _, _| (), move |b, _, window, _| {
                // 24 px wide line box; the baseline sits a little left of its middle, the text starts at the bottom
                vtext::paint(window, &n, 24., -0.024, f32::from(b.origin.x) + 17., f32::from(b.origin.y + b.size.height), 150., ink2);
            }).w(px(24.)).h(px(name_len.max(24.)))
        };
        let enc_ok = snap.encryption.active;
        let lock = if enc_ok {
            div().id("c-enc").relative().flex_none().w(px(26.)).h(px(20.)).flex().items_center().justify_center().cursor_pointer().text_color(ink)
                .child(sq::front(10., None, Some((1., ink)), None)).child(icons::icon(Icon::Lock, 13., ink)).child(hits.track("c-enc"))
                .tooltip(|window, cx| Tooltip::new("Encrypted. What the call server can see").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| { this.call.explain = !this.call.explain; cx.notify(); })).into_any_element()
        } else {
            div().relative().flex_none().px(px(6.)).py(px(2.)).rounded(px(10.)).border_1().border_color(rgb(look::DANGER)).text_color(rgb(look::DANGER)).font_family(look::F_NAME).text_size(px(9.)).child("NOT").child(hits.track("c-enc")).into_any_element()
        };
        let mut acts = v_flex().gap(px(4.)).items_center();
        acts = acts.child(self.dock_button("c-mute", if muted { Icon::MicOff } else { Icon::Mic }, if muted { "Unmute".into() } else { "Mute".into() }, muted, false, &duo, cx)
            .on_click(cx.listener(move |this, _, _, cx| { this.call_cmd(Command::SetMuted(!this.call.snap.muted)); cx.notify(); })));
        acts = acts.child(self.dock_button("c-deaf", Icon::Phones, if deaf { "Undeafen".into() } else { "Deafen".into() }, deaf, false, &duo, cx)
            .on_click(cx.listener(move |this, _, _, cx| { this.call_cmd(Command::SetDeafened(!this.call.snap.deafened)); cx.notify(); })));
        acts = acts.child(self.dock_button("c-share", Icon::Screen, if sharing { "Stop sharing your screen".into() } else { "Share your screen".into() }, sharing, false, &duo, cx)
            .on_click(cx.listener(|this, _, _, cx| { this.toggle_share(); cx.notify(); })));
        acts = acts.child(self.dock_button("c-cam", Icon::Cam, if cam_on { "Turn the camera off".into() } else { "Turn the camera on".into() }, cam_on, false, &duo, cx)
            .on_click(cx.listener(|this, _, _, cx| { this.toggle_camera(); cx.notify(); })));
        acts = acts.child(self.dock_button("c-set", Icon::Gear, "Call settings".into(), self.call.settings_open, false, &duo, cx)
            .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))));
        // rune: one ♪ button in the dock; its card opens beside the dock only when asked for (it used to float over the room)
        if self.rune.now.is_some() {
            let along = self.rune.along.is_some();
            let label = if self.call.rune_open { "Hide rune".to_string() } else if along { "rune: listening along".to_string() } else { "rune: song, controls, listen along".to_string() };
            acts = acts.child(self.dock_button("c-rune", Icon::Music, label, self.call.rune_open || along, false, &duo, cx)
                .on_click(cx.listener(|this, _, _, cx| { this.call.rune_open = !this.call.rune_open; cx.notify(); })));
        }
        if ptt {
            let held = snap.ptt_held;
            acts = acts.child(self.dock_button("c-ptt", Icon::Mic, "Hold to talk (or hold your push-to-talk key)".into(), held, false, &duo, cx)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { this.call_cmd(Command::PushToTalk(true)); cx.notify(); }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| { this.call_cmd(Command::PushToTalk(false)); cx.notify(); }))
                .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| { this.call_cmd(Command::PushToTalk(false)); cx.notify(); })));
        }
        if self.ring_all_room_open() {
            let on = self.ring.book.outs.contains_key("*");
            acts = acts.child(self.dock_button("c-ring", Icon::Bell, if on { "Stop ringing".into() } else { "Ring everyone in this community who is not in the call".into() }, on, false, &duo, cx)
                .on_click(cx.listener(|this, _, window, cx| this.ring_all(window, cx))));
        }
        acts = acts.child(self.dock_button("c-pop", Icon::Pop, "Open the call in its own window".into(), self.call.popped, false, &duo, cx)
            .on_click(cx.listener(|this, _, window, cx| this.pop_call(window, cx))));
        acts = acts.child(self.dock_button("c-mini", Icon::Mini, "A small always-on-top window with the faces and mute".into(), self.call.mini.is_some(), false, &duo, cx)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_mini(window, cx))));
        acts = acts.child(self.dock_button("c-leave", Icon::Leave, "Leave the call".into(), false, true, &duo, cx)
            .on_click(cx.listener(|this, _, _, cx| this.leave_call(cx))));

        let dock = v_flex().id("call-dock").relative().w(px(DOCK_W)).flex_none().items_center().gap(px(6.)).pt(px(10.)).px(px(6.)).pb(px(6.)).text_color(ink)
            .child(sq::back(40., main))
            .child(div().relative().size(px(10.)).child(sq::back(3., hi)))
            .child(div().relative().child(name_box))
            .child(div().relative().child(lock))
            .child(div().relative().child(acts));

        // the people
        let mut people = v_flex().id("call-ppl").flex_none().items_center().gap(px(8.)).py(px(4.)).max_h_full().overflow_y_scroll();
        for p in &snap.peers {
            let ident = p.identity.clone();
            let who = if p.me { "you".to_string() } else { p.name.clone() };
            let state = if p.speaking { ", speaking" } else if p.muted { ", muted" } else { "" };
            let tip = format!("{who}{state}");
            let dim = p.muted || p.deafened;
            let (acct, nm, me) = (p.account.clone(), p.name.clone(), p.me);
            let ringed = p.speaking;
            let mut cell = div().id(SharedString::from(format!("peer-{ident}"))).relative().flex_none().size(px(FACE)).cursor_pointer()
                .child(hits.track(format!("peer-{ident}")))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
            if ringed {
                cell = cell.child(div().absolute().left(px(-4.)).top(px(-4.)).size(px(FACE + 8.)).child(sq::back(26., hi)))
                    .child(div().absolute().left(px(-2.)).top(px(-2.)).size(px(FACE + 4.)).child(sq::back(24., rgb(look::VOID))));
            }
            cell = cell.child(div().relative().size(px(FACE)).when(dim, |d| d.opacity(0.55)).child(face(p.account.as_deref(), &p.name, FACE, 22.)));
            if !me && acct.is_some() {
                let (a1, n1) = (acct.clone().unwrap(), nm.clone());
                cell = cell.on_click(cx.listener(move |this, _, window, cx| this.open_person(a1.clone(), n1.clone(), window, cx)));
                let (a2, n2) = (acct.unwrap(), nm);
                cell = cell.on_mouse_down(MouseButton::Right, cx.listener(move |this, _, window, cx| { this.call.menu = None; this.open_person(a2.clone(), n2.clone(), window, cx); }));
            }
            people = people.child(cell);
        }

        // notes to the left of the card
        let mut notes = v_flex().id("call-notes").absolute().right(px(DOCK_W + 10.)).top_0().w(px(240.)).gap(px(6.));
        let note = |t: String, bad: bool| div().relative().px(px(10.)).py(px(8.)).font_family(look::F_UI).text_size(px(12.)).line_height(px(16.)).text_color(if bad { rgb(look::DANGER).into() } else { look::paper() })
            .child(sq::back(12., rgb(0x14131a))).child(div().relative().child(t));
        if let Some(e) = snap.error.clone().filter(|e| self.call.err_hidden.as_deref() != Some(e.msg.as_str())) { let m = e.msg.clone(); notes = notes.child(div().id("c-err").cursor_pointer().child(note(e.msg, true)).on_click(cx.listener(move |this, _, _, cx| { this.call.err_hidden = Some(m.clone()); cx.notify(); }))); }
        if snap.game_muted { notes = notes.child(note("Muted by game key. Friends see you as muted.".into(), false)); }
        if !snap.blocked.is_empty() { notes = notes.child(note(format!("Blocked {} stream(s) that are not from a current member or not encrypted.", snap.blocked.len()), true)); }
        if !muted && (snap.mic_quiet || snap.mic_system_muted == Some(true)) {
            notes = notes.child(div().id("c-mic-warn").child(note(if snap.mic_system_muted == Some(true) { "Others cannot hear you: your microphone is muted in your computer's sound settings.".into() } else { "Others cannot hear you: no sound is coming from your microphone.".into() }, true)));
        }
        if self.call.rune_open { if let Some(r) = self.rune_segment(cx) { notes = notes.child(r); } }
        let explain = self.call.explain.then(|| self.explainer(&snap));

        // the column: the card centred on the window's height, the people hanging under it
        Some(div().id("call-gutter").absolute().top_0().bottom_0().right(px(10.)).w(px(DOCK_W)).flex().flex_col().occlude()
            .child(div().flex_1().min_h_0())
            .child(div().relative().flex_none().child(dock).child(notes).children(explain))
            .child(div().flex_1().min_h_0().pt(px(10.)).child(people))
            .into_any_element())
    }

    fn explainer(&self, snap: &engine::Snapshot) -> AnyElement {
        let e = &snap.encryption;
        v_flex().id("call-explain").absolute().right(px(DOCK_W + 10.)).bottom(px(0.)).w(px(290.)).gap(px(6.)).px(px(12.)).py(px(10.)).font_family(look::F_UI).text_size(px(12.)).line_height(px(17.)).occlude()
            .child(sq::back(16., rgb(0x14131a)))
            .child(div().relative().font_weight(FontWeight::BOLD).child("What the call server can see"))
            .child(div().relative().child(if e.active { format!("Every audio and video frame is encrypted on this device before it is sent. Key epoch {}.", e.epoch.map(|x| x.to_string()).unwrap_or("?".into())) } else { "This call's media is NOT encrypted.".to_string() }))
            .child(div().relative().child("Your IP address, when you join and leave, how much you send, and who is speaking. It cannot hear or see anything."))
            .child(div().relative().text_color(look::mute()).child(format!("Keys: {} device keys from the room, rotated {} time(s) this call.", e.devices, e.rotations)))
            .into_any_element()
    }

    /// Whether the card has a ring button: a connected call in a voice room that has others to ring.
    pub fn ring_all_room_open(&self) -> bool { self.ring_all_room().is_some() }
}

