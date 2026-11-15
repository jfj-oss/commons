//! The window: sign-in, the rooms, one room's messages (a virtual list fed by the core's differences) and the composer.
use crate::bench;
use crate::core::Core;
use crate::look::{self, Duo};
use crate::model::{Change, Channel, Community, Snapshot, Window as MsgWindow};
use crate::prefs::{ChatFont, Prefs, Saver};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::component::{v_flex, Disableable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Screen { Starting, Welcome, Locked, Main }

/// Where the window is, once signed in: the community cards, one community's room cards, a room, or settings.
#[derive(Clone, PartialEq, Debug)]
pub enum Level { Home, Server(String), Chat, Settings }

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
    pub level: Level,
    pub prefs: Prefs,
    pub on_battery: bool,
    pub pres: crate::presence::Presence,
    /// The cards' clock and the open room's backdrop (ascii.rs).
    pub clock: f64,
    pub bd_t: f64,
    pub heat: f64,
    pub ripples: Vec<crate::ascii::Ripple>,
    pub holes: crate::ascii::Holes,
    next_ripple: f64,
    last_anim: Instant,
    pub notify_settings: Value,
    pub set_name: Entity<InputState>,
    // the open room
    pub win: MsgWindow,
    pub limit: usize,
    pub list: ListState,
    pub composer: Entity<TextareaState>,
    typing_sent: Option<Instant>,
    ticking: bool,
    again: bool,
    pub first_frame_done: bool,
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
        let set_name = cx.new(|cx| InputState::new(window, cx).placeholder("Your name"));
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
            level: Level::Home, prefs: Prefs::load(&core.dir), on_battery: false, pres: Default::default(), clock: 0., bd_t: 0., heat: HEAT_FLOOR, ripples: vec![], holes: Default::default(), next_ripple: 8., last_anim: Instant::now(), notify_settings: Value::Null, set_name,
            core, screen: Screen::Starting, error: None, busy: false, pass, name, relay, default_relay: String::new(), snap: None, snap_hash: String::new(), cid: None, gid: None,
            win: MsgWindow::default(), limit: std::env::var("MEGA_GPUI_LIMIT").ok().and_then(|x| x.parse().ok()).unwrap_or(200),
            list: ListState::new(0, ListAlignment::Bottom, px(600.)), composer, typing_sent: None, ticking: false, again: false, first_frame_done: false,
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
        this._tasks.push(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(700)).await;
            if this.update_in(cx, |this, window, cx| this.presence_tick(window, cx)).is_err() { break; }
        }));
        // The fields move at 20 frames a second on the cards and 10 behind a chat (25 while a ripple runs), as in mega.js;
        // not at all with the battery saver on, or while the window is not the active one.
        this._tasks.push(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(40)).await;
            if this.update_in(cx, |this, window, cx| this.animate(window, cx)).is_err() { break; }
        }));
        this._tasks.push(cx.spawn_in(window, async move |this, cx| loop {
            if this.update_in(cx, |this, window, cx| this.power_tick(window, cx)).is_err() { break; }
            cx.background_executor().timer(Duration::from_secs(60)).await;
        }));
        this
    }

    /// Battery saver: "auto" (the default) is on while the computer runs on battery, which the core reads.
    pub fn still(&self) -> bool { self.prefs.saver == Saver::On || (self.prefs.saver == Saver::Auto && self.on_battery) }
    fn power_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prefs.saver != Saver::Auto { return; }
        self.rpc("power_status", json!({}), window, cx, |this, r, _, cx| {
            let b = r.ok().and_then(|v| v["battery"].as_bool()).unwrap_or(false);
            if b != this.on_battery { this.on_battery = b; cx.notify(); }
        });
    }
    fn animate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Main || self.still() || !window.is_window_active() { self.last_anim = Instant::now(); return; }
        let dt = self.last_anim.elapsed().as_secs_f64();
        match self.level {
            Level::Home | Level::Server(_) => { if dt < 0.05 { return; } self.clock += dt.min(0.1); }
            Level::Chat => {
                if dt < if self.ripples.is_empty() { 0.1 } else { 0.04 } { return; }
                let dt = dt.min(0.25);
                self.heat = (self.heat * 0.5f64.powf(dt / 18.)).max(HEAT_FLOOR);
                self.bd_t += dt * (0.1 + self.heat * 1.5);
                self.next_ripple -= dt;
                if self.next_ripple <= 0. {   // now and then a gentle ring from somewhere in the room
                    self.next_ripple = 14. + 14. * nh01(self.bd_t);
                    self.ripples.push(crate::ascii::Ripple { x: 20. + 60. * nh01(self.bd_t * 3.1), y: 8. + 20. * nh01(self.bd_t * 7.7), r: 0., s: 0.5, v: 26., band: 8., fade: 0.3 });
                }
                for q in &mut self.ripples { q.r += q.v * dt; q.s *= q.fade.powf(dt); }
                self.ripples.retain(|q| q.s > 0.04 && q.r < 220.);
            }
            Level::Settings => return,
        }
        self.last_anim = Instant::now();
        cx.notify();
    }
    /// Activity in the open room: the backdrop warms up and a ring leaves from where the newest line is.
    fn stir(&mut self, n: usize) {
        self.heat = (self.heat + 0.18 * n as f64).min(1.);
        if self.still() { return; }
        if self.ripples.len() > 6 { self.ripples.remove(0); }
        self.ripples.push(crate::ascii::Ripple { x: 14., y: 36., r: 0., s: 0.9, v: 44., band: 4., fade: 0.12 });
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
        self.rpc("notify_get", json!({}), window, cx, |this, r, _, _| { if let Ok(v) = r { this.notify_settings = v; } });
        self.tick(window, cx);
    }

    // ---- the look every half second ----
    pub fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Main { return; }
        if self.ticking { self.again = true; return; }
        self.ticking = true;
        let core = self.core.clone();
        let snap_if = self.snap_hash.clone();
        // nothing is read (or marked read) while the cards are showing
        let room = self.gid.clone().filter(|_| self.level == Level::Chat).map(|gid| (gid, self.limit, self.win.hash.clone()));
        cx.spawn_in(window, async move |this, cx| {
            let out = core.clone().look(move |app| {
                // The core answers "same" to the hash of what this window already holds, which is nearly every time.
                let snap = app.call("snapshot", if snap_if.is_empty() { json!({}) } else { json!({"if": snap_if}) });
                let msgs = room.map(|(gid, limit, have)| {
                    let mut a = json!({"gid": gid, "limit": limit, "around": "", "delta": 1});
                    if !have.is_empty() { a["if"] = have.into(); }
                    (gid, app.call("messages", a))
                });
                Polled { snap, msgs }
            }).await;
            let Ok(out) = out else { return };
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
                    if appended > 0 { if was > 0 { self.list.splice(was - 1..was, 1); } self.list.splice(was..was, appended); self.stir(appended); }
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
            let found = self.snap.as_ref().and_then(|s| s.communities.iter().chain(s.direct.iter()).find_map(|c| c.channels.iter().find(|ch| ch.name == want && ch.joined).map(|ch| (c.id.clone(), ch.gid.clone()))));
            if let Some((cid, gid)) = found { self.auto.open = None; self.open_room(cid, gid, window, cx); }
        }
    }

    /// A room and the community (or one-to-one conversation) it is in.
    pub fn channel(&self, gid: &str) -> Option<(&Community, &Channel)> {
        let s = self.snap.as_ref()?;
        s.communities.iter().chain(s.direct.iter()).find_map(|c| c.channels.iter().find(|ch| ch.gid == gid).map(|ch| (c, ch)))
    }
    pub fn room_duo(&self, gid: &str) -> Duo {
        match self.channel(gid) { Some((c, _)) => Duo::of(gid, c.room_colours.iter().find(|(g, _)| g == gid).map(|(_, d)| d)), None => Duo::of(gid, None) }
    }

    fn mark_read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gid) = self.gid.clone().filter(|_| self.level == Level::Chat) else { return };
        let unread = self.channel(&gid).is_some_and(|(_, ch)| ch.unread > 0);
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
        self.level = Level::Chat;
        if self.gid.as_deref() == Some(gid.as_str()) { cx.notify(); return; }
        self.bd_t = (look::h(&gid) % 90) as f64; self.ripples.clear(); self.heat = HEAT_FLOOR;
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
    pub fn render_sign_in(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
}

pub fn clock(ts: u64) -> String {
    use chrono::TimeZone as _;
    chrono::Local.timestamp_opt(ts as i64, 0).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

pub const HEAT_FLOOR: f64 = 0.12;
fn nh01(x: f64) -> f64 { let s = (x * 127.1).sin() * 43758.5453; s - s.floor() }

impl Mega {
    pub fn go(&mut self, level: Level, window: &mut Window, cx: &mut Context<Self>) {
        if level == Level::Settings { let n = self.snap.as_ref().map(|s| s.me_name.clone()).unwrap_or_default(); self.set_name.update(cx, |i, cx| i.set_value(n, window, cx)); }
        if level == Level::Chat { self.composer.update(cx, |c, cx| c.focus(window, cx)); }
        self.level = level;
        self.focus_card(None);
        cx.notify();
    }
    pub fn lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc("lock", json!({}), window, cx, |this, _, window, cx| {
            this.screen = Screen::Locked; this.level = Level::Home; this.snap = None; this.snap_hash.clear(); this.gid = None; this.cid = None; this.win = MsgWindow::default();
            this.list = ListState::new(0, ListAlignment::Bottom, px(600.));
            this.pass.update(cx, |p, cx| p.focus(window, cx));
            window.set_window_title("mega");
            cx.notify();
        });
    }
    pub fn chat_font(&self) -> (&'static str, f32, f32) { match self.prefs.chat_font { ChatFont::Bellefair => (look::F_BELLEFAIR, 30., 36.), ChatFont::Raster => (look::F_MSG, 24., 26.) } }
    pub fn set_setting(&mut self, method: &'static str, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        self.rpc(method, args, window, cx, move |this, r, window, cx| {
            match r { Ok(v) => { if method.starts_with("notify_") { this.notify_settings = v; } this.error = None; } Err(e) => this.error = Some(e.into()) }
            this.snap_hash.clear();
            this.tick(window, cx);
            cx.notify();
        });
    }
}
