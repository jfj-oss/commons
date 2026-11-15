//! The window: sign-in, the rooms, one room's messages (a virtual list fed by the core's differences) and the composer.
use crate::bench;
use crate::core::Core;
use crate::look::{self, Duo};
use crate::model::{Change, Msg, Snapshot, Window as MsgWindow};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Screen { Starting, Welcome, Locked, Main }

pub struct Mega {
    pub core: Arc<Core>,
    pub screen: Screen,
    pub error: Option<SharedString>,
    pub busy: bool,
    // sign-in
    pub pass: Entity<InputState>,
    pub name: Entity<InputState>,
    pub relay: Entity<InputState>,
    pub default_relay: String,
    // the rooms
    pub snap: Option<Snapshot>,
    snap_hash: String,
    pub cid: Option<String>,
    pub gid: Option<String>,
    // the open room
    pub win: MsgWindow,
    pub limit: usize,
    pub list: ListState,
    pub composer: Entity<TextareaState>,
    typing_sent: Option<Instant>,
    ticking: bool,
    again: bool,
    first_frame: bool,
    pub auto: crate::bench::Auto,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

struct Polled { snap: Result<Value, String>, msgs: Option<(String, Result<Value, String>)> }

impl Mega {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pass = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Passphrase"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Your name"));
        let relay = cx.new(|cx| InputState::new(window, cx).placeholder("Relay address (https://…)"));
        let composer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8).submit_on_enter(true).placeholder("Write a message"));
        let mut subs = vec![
            cx.subscribe_in(&composer, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                // Enter sends; Shift+Enter is a new line (the input inserts it itself and does not report it as a submit).
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                InputEvent::Change => this.typing(cx),
                _ => {}
            }),
            cx.subscribe_in(&pass, window, |this: &mut Self, _, ev: &InputEvent, window, cx| { if let InputEvent::PressEnter { .. } = ev { this.submit_sign_in(window, cx); } }),
        ];
        subs.push(cx.observe_window_activation(window, |this, window, cx| this.beat(window, cx)));
        let mut this = Mega {
            core, screen: Screen::Starting, error: None, busy: false, pass, name, relay, default_relay: String::new(), snap: None, snap_hash: String::new(), cid: None, gid: None,
            win: MsgWindow::default(), limit: std::env::var("MEGA_GPUI_LIMIT").ok().and_then(|x| x.parse().ok()).unwrap_or(200),
            list: ListState::new(0, ListAlignment::Bottom, px(600.)), composer, typing_sent: None, ticking: false, again: false, first_frame: true,
            auto: crate::bench::Auto::from_env(), _tasks: vec![], _subs: subs,
        };
        this.look_at_state(window, cx);
        // The page's clock: a look every half second (app.js `tick`), and the notifier's heartbeat every three (mega-notify.js).
        this._tasks.push(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            if this.update_in(cx, |this, window, cx| this.tick(window, cx)).is_err() { break; }
        }));
        this._tasks.push(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(3000)).await;
            if this.update_in(cx, |this, window, cx| this.beat(window, cx)).is_err() { break; }
        }));
        this
    }

    /// One call to the core off the UI thread, then `then` back on it.
    pub fn rpc(&mut self, method: &'static str, args: Value, window: &mut Window, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, Result<Value, String>, &mut Window, &mut Context<Self>) + 'static) {
        let core = self.core.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move { core.call(method, args) }).await;
            let _ = this.update_in(cx, |this, window, cx| then(this, r, window, cx));
        }).detach();
    }

    fn look_at_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("state", json!({}), window, cx, |this, r, window, cx| {
            match r {
                Ok(s) => {
                    this.default_relay = s["default_relay"].as_str().unwrap_or("").to_string();
                    if s["locked"].as_bool() == Some(false) { this.enter(window, cx); }
                    else if s["exists"].as_bool() == Some(true) { this.screen = Screen::Locked; this.pass.update(cx, |p, cx| p.focus(window, cx)); }
                    else { this.screen = Screen::Welcome; this.name.update(cx, |p, cx| p.focus(window, cx)); }
                    if this.screen == Screen::Locked { if let Some(p) = this.auto.pass.take() { this.unlock_with(p, window, cx); } }
                }
                Err(e) => this.error = Some(e.into()),
            }
            cx.notify();
        });
    }

    fn submit_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy { return; }
        let pass = self.pass.read(cx).value().to_string();
        match self.screen {
            Screen::Locked => self.unlock_with(pass, window, cx),
            Screen::Welcome => {
                let (name, relay) = (self.name.read(cx).value().to_string(), self.relay.read(cx).value().to_string());
                self.busy = true; self.error = None; cx.notify();
                self.rpc("create_account", json!({"relay": relay, "passphrase": pass, "name": name, "invite": ""}), window, cx, |this, r, window, cx| {
                    this.busy = false;
                    match r { Ok(_) => this.enter(window, cx), Err(e) => this.error = Some(e.into()) }
                    cx.notify();
                });
            }
            _ => {}
        }
    }
    fn unlock_with(&mut self, pass: String, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true; self.error = None; cx.notify();
        self.rpc("unlock", json!({"passphrase": pass}), window, cx, |this, r, window, cx| {
            this.busy = false;
            match r { Ok(_) => this.enter(window, cx), Err(e) => this.error = Some(e.into()) }
            cx.notify();
        });
    }
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pass.update(cx, |p, cx| p.set_value("", window, cx));
        self.screen = Screen::Main;
        bench::mark("unlocked");
        self.tick(window, cx);
    }

    // ---- the look every half second ----
    pub fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Main { return; }
        if self.ticking { self.again = true; return; }
        self.ticking = true;
        let core = self.core.clone();
        let snap_if = self.snap_hash.clone();
        let room = self.gid.clone().map(|gid| (gid, self.limit, self.win.hash.clone()));
        cx.spawn_in(window, async move |this, cx| {
            let out = cx.background_spawn(async move {
                // The core answers "same" to the hash of what this window already holds, which is nearly every time.
                let snap = core.call("snapshot", if snap_if.is_empty() { json!({}) } else { json!({"if": snap_if}) });
                let msgs = room.map(|(gid, limit, have)| {
                    let mut a = json!({"gid": gid, "limit": limit, "around": "", "delta": 1});
                    if !have.is_empty() { a["if"] = have.into(); }
                    (gid, core.call("messages", a))
                });
                Polled { snap, msgs }
            }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.ticking = false;
                this.polled(out, window, cx);
                if std::mem::take(&mut this.again) { this.tick(window, cx); }
            });
        }).detach();
    }

    fn polled(&mut self, out: Polled, window: &mut Window, cx: &mut Context<Self>) {
        match out.snap {
            Ok(v) if v["same"].as_bool() == Some(true) => {}
            Ok(v) => {
                self.snap_hash = v["hash"].as_str().unwrap_or("").to_string();
                let snap = Snapshot::parse(&v);
                let n = snap.total_unread();
                window.set_window_title(&if n > 0 { format!("({}) mega", if n > 99 { "99+".to_string() } else { n.to_string() }) } else { "mega".to_string() });
                self.snap = Some(snap);
                self.error = None;
                self.after_snapshot(window, cx);
                cx.notify();
            }
            Err(e) if e == "locked" => { self.screen = Screen::Locked; cx.notify(); }
            Err(e) => { self.error = Some(format!("Not reachable: {e}").into()); cx.notify(); }
        }
        if let Some((gid, Ok(r))) = out.msgs {
            if self.gid.as_deref() != Some(gid.as_str()) { return; }
            let was = self.win.msgs.len();
            match self.win.apply(&r) {
                Ok(Change::Same) => {}
                Ok(Change::InPlace { changed, appended }) => {
                    // a changed row is measured again; new rows are added at the end, and a list that was at the bottom stays there
                    for i in changed { self.list.splice(i..i + 1, 1); if i + 1 < was { self.list.splice(i + 1..i + 2, 1); } }
                    if appended > 0 { if was > 0 { self.list.splice(was - 1..was, 1); } self.list.splice(was..was, appended); }
                    cx.notify();
                }
                Ok(Change::Reset) => {
                    self.list.reset(self.win.msgs.len());
                    if was == 0 && !self.win.msgs.is_empty() { bench::mark_with("room_loaded", json!({"rows": self.win.msgs.len()})); self.auto.room_loaded = true; }
                    cx.notify();
                }
                Err(()) => { self.win.hash.clear(); self.again = true; }
            }
            self.mark_read(window, cx);
        }
    }

    /// Hook for what follows a new snapshot (automation opens its room here).
    fn after_snapshot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(want) = self.auto.open.clone() {
            let found = self.snap.as_ref().and_then(|s| s.communities.iter().find_map(|c| c.channels.iter().find(|ch| ch.name == want && ch.joined).map(|ch| (c.id.clone(), ch.gid.clone()))));
            if let Some((cid, gid)) = found { self.auto.open = None; self.open_room(cid, gid, window, cx); }
        }
    }

    fn mark_read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gid) = self.gid.clone() else { return };
        let unread = self.snap.as_ref().is_some_and(|s| s.communities.iter().flat_map(|c| c.channels.iter()).any(|ch| ch.gid == gid && ch.unread > 0));
        if !unread { return; }
        // `seen` is for the read receipt: a window behind another clears its count but tells nobody "read" (app.js).
        let seen = window.is_window_active();
        self.rpc("mark_read", json!({"gid": gid, "seen": seen}), window, cx, |this, _, _, _| { this.snap_hash.clear(); });
    }

    fn beat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Main { return; }
        let attended = window.is_window_active();
        self.rpc("notify_beat", json!({"attended": attended, "web": false}), window, cx, |_, _, _, _| {});
    }

    pub fn open_room(&mut self, cid: String, gid: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.gid.as_deref() == Some(gid.as_str()) { return; }
        self.cid = Some(cid);
        self.gid = Some(gid.clone());
        self.win = MsgWindow { key: gid, ..Default::default() };
        self.list = ListState::new(0, ListAlignment::Bottom, px(600.));
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        bench::mark("room_open");
        cx.notify();
        self.tick(window, cx);
    }

    // ---- the composer ----
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gid) = self.gid.clone() else { return };
        let text = self.composer.read(cx).value().to_string();
        if text.trim().is_empty() { return; }
        self.composer.update(cx, |c, cx| c.set_value("", window, cx));
        self.typing_sent = None;
        self.rpc("send_text", json!({"gid": gid, "text": text.trim_end()}), window, cx, |this, r, window, cx| {
            if let Err(e) = r { this.error = Some(e.into()); cx.notify(); }
            this.list.scroll_to_end();
            this.tick(window, cx);
        });
    }
    fn typing(&mut self, cx: &mut Context<Self>) {
        let Some(gid) = self.gid.clone() else { return };
        if !self.snap.as_ref().is_some_and(|s| s.typing_enabled) || self.composer.read(cx).value().is_empty() { return; }
        if self.typing_sent.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) { return; }
        self.typing_sent = Some(Instant::now());
        let core = self.core.clone();
        cx.background_spawn(async move { let _ = core.call("typing", json!({"gid": gid})); }).detach();
    }

    // ---- drawing ----
    fn render_sign_in(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let welcome = self.screen == Screen::Welcome;
        v_flex().size_full().items_center().justify_center().gap_3()
            .child(div().font_family(look::F_SHARP).text_size(px(96.)).line_height(px(96.)).child("mega"))
            .child(div().text_color(look::mute()).child(if welcome { "Make an account on this computer." } else { "Enter your passphrase." }))
            .child(v_flex().w(px(340.)).gap_2()
                .when(welcome, |d| d.child(Input::new(&self.name)).child(Input::new(&self.relay)))
                .child(Input::new(&self.pass))
                .child(Button::new("go").primary().label(if self.busy { "Working…" } else if welcome { "Create account" } else { "Unlock" }).disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.submit_sign_in(window, cx)))))
            .when_some(self.error.clone(), |d, e| d.child(div().max_w(px(520.)).text_color(rgb(look::DANGER)).child(e)))
            .when(welcome && !self.default_relay.is_empty(), |d| d.child(div().text_sm().text_color(look::mute()).child(format!("Relay: {} (leave the address empty to use it)", self.default_relay))))
    }

    fn render_rooms(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = v_flex().id("rooms").w(px(240.)).h_full().flex_none().overflow_y_scroll().border_r_1().border_color(look::line()).p_2().gap_1();
        let Some(snap) = &self.snap else { return col };
        for c in snap.communities.iter().filter(|c| !c.removed) {
            let duo = Duo::of(&c.id, c.colours.as_ref());
            col = col.child(h_flex().mt_2().px_2().gap_2().items_center()
                .child(div().size(px(10.)).rounded_full().bg(duo.dot()))
                .child(div().font_family(look::F_NAME).text_color(look::mute()).child(c.name.clone())));
            for ch in &c.channels {
                let (cid, gid, on) = (c.id.clone(), ch.gid.clone(), self.gid.as_deref() == Some(ch.gid.as_str()));
                let d = Duo::of(&ch.gid, c.room_colours.iter().find(|(g, _)| g == &ch.gid).map(|(_, d)| d));
                col = col.child(h_flex().id(SharedString::from(format!("room-{}", ch.gid))).px_2().py_1().gap_2().rounded_md().items_center().cursor_pointer()
                    .when(on, |x| x.bg(hsla(0., 0., 1., 0.08))).hover(|x| x.bg(hsla(0., 0., 1., 0.05)))
                    .when(!ch.joined, |x| x.opacity(0.5))
                    .child(div().size(px(8.)).rounded_full().bg(d.dot()))
                    .child(div().flex_1().overflow_hidden().text_ellipsis().child(format!("{}{}", if ch.kind == "voice" { "~ " } else { "" }, ch.name)))
                    .when(ch.unread > 0, |x| x.child(div().px_1p5().rounded_full().bg(d.dot()).text_color(rgb(0x0c0d12)).text_xs().child(ch.unread.to_string())))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_room(cid.clone(), gid.clone(), window, cx))));
            }
        }
        col
    }

    fn render_msg(&mut self, ix: usize, _window: &mut Window, _cx: &mut Context<Self>) -> AnyElement {
        let Some(m) = self.win.msgs.get(ix) else { return div().into_any_element() };
        let prev = ix.checked_sub(1).and_then(|i| self.win.msgs.get(i));
        row(m, prev)
    }

    fn render_chat(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.gid.as_ref().and_then(|g| self.snap.as_ref()?.communities.iter().flat_map(|c| c.channels.iter()).find(|ch| &ch.gid == g).map(|ch| ch.name.clone()));
        let Some(title) = title else {
            return v_flex().flex_1().h_full().items_center().justify_center().text_color(look::mute()).child("Pick a room.").into_any_element();
        };
        let typing = match self.win.typing.len() { 0 => String::new(), 1 => format!("{} is writing…", self.win.typing[0]), n => format!("{n} people are writing…") };
        v_flex().flex_1().h_full().min_w_0()
            .child(h_flex().flex_none().px_4().pt_3().pb_1().child(div().font_family(look::F_SHARP).text_size(px(36.)).line_height(px(36.)).child(title)))
            .child(div().flex_1().min_h_0().relative()
                .child(list(self.list.clone(), cx.processor(|this, ix: usize, window, cx| this.render_msg(ix, window, cx))).size_full())
                .child(div().absolute().top_0().right_0().bottom_0().w(px(12.)).child(Scrollbar::vertical(&self.list))))
            .child(div().flex_none().h(px(18.)).px_4().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(typing))
            .child(div().flex_none().mx_3().mb_3().px_1().py_1().rounded_xl().bg(rgb(look::SLIP)).border_1().border_color(look::line())
                .font_family(look::F_MSG).text_size(px(24.)).line_height(px(26.))
                .child(Textarea::new(&self.composer).appearance(false).text_size(px(24.)).line_height(px(26.))))
            .into_any_element()
    }
}

fn clock(ts: u64) -> String {
    use chrono::TimeZone as _;
    chrono::Local.timestamp_opt(ts as i64, 0).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

/// One message. `prev` decides whether the name is shown again (same person within five minutes: no).
pub fn row(m: &Msg, prev: Option<&Msg>) -> AnyElement {
    if m.kind == "member" || m.kind == "notice" {
        return div().px_4().py_1().font_family(look::F_NAME).text_size(px(12.)).text_color(look::mute()).child(m.text.clone()).into_any_element();
    }
    let head = !prev.is_some_and(|p| p.sender == m.sender && p.kind != "member" && p.kind != "notice" && m.ts.saturating_sub(p.ts) < 300);
    let mut col = v_flex().px_4().pb_1().when(head, |d| d.pt_2());
    if head {
        col = col.child(h_flex().gap_2().items_baseline().font_family(look::F_NAME).text_size(px(12.))
            .child(div().text_color(look::face(&m.name)).child(m.name.clone()))
            .child(div().text_color(look::mute()).child(clock(m.ts))));
    }
    if let Some((who, text)) = &m.reply {
        col = col.child(div().pl_2().border_l_2().border_color(look::line()).text_color(look::mute()).text_size(px(14.)).child(format!("{who}: {text}")));
    }
    let body: AnyElement = if m.deleted { div().text_color(look::mute()).text_size(px(16.)).child("(deleted)").into_any_element() }
        else if let Some((name, size)) = &m.file { div().text_color(look::mute()).text_size(px(16.)).child(format!("[file] {name} ({} KB){}", size / 1024, if m.text.is_empty() { String::new() } else { format!("  {}", m.text) })).into_any_element() }
        else { div().font_family(look::F_MSG).text_size(px(24.)).line_height(px(26.)).child(m.text.clone()).into_any_element() };
    col = col.child(body);
    let mut foot = h_flex().gap_1().flex_wrap();
    for r in &m.reactions {
        foot = foot.child(div().px_1p5().rounded_full().border_1().border_color(if r.mine { look::paper() } else { look::line() }).text_size(px(13.)).child(format!("{} {}", r.emoji, r.count)));
    }
    if m.edited { foot = foot.child(div().text_color(look::mute()).text_size(px(11.)).child("edited")); }
    if m.mine && (m.status == "pending" || m.status == "failed") { foot = foot.child(div().text_color(look::mute()).text_size(px(11.)).child(m.status.clone())); }
    if !m.read_by.is_empty() { foot = foot.child(div().text_color(look::mute()).text_size(px(11.)).child(format!("read by {}", m.read_by.join(", ")))); }
    col.child(foot).into_any_element()
}

impl Render for Mega {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.first_frame) { window.on_next_frame(|_, _| bench::mark("first_frame")); }
        crate::bench::drive(self, window, cx);
        let base = div().size_full().bg(rgb(look::VOID)).text_color(look::paper()).font_family(look::F_UI).text_size(px(15.));
        match self.screen {
            Screen::Starting => base,
            Screen::Welcome | Screen::Locked => base.child(self.render_sign_in(cx)),
            Screen::Main => base.child(h_flex().size_full().items_start()
                .child(self.render_rooms(cx))
                .child(self.render_chat(cx))),
        }
    }
}
