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
    pub snap_hash: String,
    pub cid: Option<String>,
    pub gid: Option<String>,
    pub level: Level,
    pub prefs: Prefs,
    pub on_battery: bool,
    pub pres: crate::presence::Presence,
    /// The cards' clock and the open room's backdrop (ascii.rs).
    pub clock: f64,
    /// The open room's backdrop (backdrop.rs): the waves of new activity, and its own clock, which only runs while one does.
    pub waves: Vec<crate::backdrop::Wave>,
    pub bd_clock: f64,
    pub bd_cache: crate::backdrop::Shared,
    pub holes: crate::ascii::Holes,
    /// Messages resolving out of glyphs and reactions flying to their pills (arrive.rs); empty almost always, and then no frames are asked for.
    pub arrivals: crate::arrive::Arrivals,
    pub flights: crate::arrive::Flights,
    pub epoch: Instant,
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
    pub call: crate::callui::CallUi,
    /// Rings, direct calls and the missed-call notes (ringui.rs), rune in the call bar (runebar.rs), and the test port (uitest.rs).
    pub ring: crate::ringui::RingUi,
    pub rune: crate::runebar::RuneUi,
    pub uitest: Option<Arc<crate::uitest::Queue>>,
    /// People, admin, invites and joining (members.rs), and an invite link given on the command line, waiting for an account.
    pub mem: crate::members::MembersUi,
    pub pending_link: Option<String>,
    /// The owner's colour presets are showing in the open room's header.
    pub colours_open: bool,
    /// Checks only (`dmunread=N` step): pretend this many unread messages in every direct conversation, for the core reports none yet.
    pub fake_dm_unread: usize,
    /// A direct conversation's unread messages when it was opened: the last few rows carry a small star until it is left.
    pub dm_marks: (String, usize),
    /// A backdrop picture per direct-conversation row on the home screen.
    pub dm_caches: std::collections::HashMap<String, crate::backdrop::Shared>,
    /// The updater's state and cards (updateui.rs).
    pub upd: crate::updateui::UpdateUi,
    /// The card column of the home and community levels (cards.rs).
    pub nav: crate::cards::NavUi,
    pub rail: crate::rail::RailUi,
    /// The colours panel (paint.rs) and the shared ASCII wall (wall.rs).
    pub paint: crate::paint::PaintUi,
    pub wall: crate::wall::WallUi,
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
        let (mem, mem_subs) = crate::members::MembersUi::new(window, cx);
        let mut subs = vec![
            cx.subscribe_in(&composer, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                // Enter sends; Shift+Enter is a new line (the input inserts it itself and does not report it as a submit).
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                InputEvent::Change => this.typing(cx),
                _ => {}
            }),
            cx.subscribe_in(&pass, window, |this: &mut Self, _, ev: &InputEvent, window, cx| { if let InputEvent::PressEnter { .. } = ev { this.submit_sign_in(window, cx); } }),
        ];
        subs.extend(mem_subs);
        subs.push(cx.observe_window_activation(window, |this, window, cx| this.beat(window, cx)));
        let prefs = Prefs::load(&core.dir);
        let call = crate::callui::CallUi::new(prefs.deck_w, window, cx);
        let upd = crate::updateui::UpdateUi::new(&core.dir);
        let nav = crate::cards::NavUi::new(cx);
        let (paint, paint_sub) = crate::paint::PaintUi::new(window, cx);
        subs.push(paint_sub);
        let mut this = Mega {
            nav, rail: Default::default(), paint, wall: crate::wall::WallUi::new(), call, upd, mem, pending_link: crate::members::link_from_args(), ring: Default::default(), rune: Default::default(), uitest: crate::uitest::from_env(), colours_open: false, fake_dm_unread: 0, dm_marks: (String::new(), 0), dm_caches: Default::default(),
            level: Level::Home, prefs, on_battery: false, pres: Default::default(), clock: 0., waves: vec![], bd_clock: 0., bd_cache: Default::default(), holes: Default::default(), arrivals: Default::default(), flights: Default::default(), epoch: Instant::now(), last_anim: Instant::now(), notify_settings: Value::Null, set_name,
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
            if this.update_in(cx, |this, window, cx| { this.presence_tick(window, cx); this.call_tick(window, cx); this.preview_tick(window, cx); }).is_err() { break; }
        }));
        // Rings, what notifications ask of the window, rune in the call bar, the test port: a quarter-second clock.
        this._tasks.push(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            if this.update_in(cx, |this, window, cx| { this.ring_tick(window, cx); this.nav_focus_tick(window, cx); this.wall_tick(window, cx); this.paint_tick(window, cx); this.mem_tick(window, cx); this.rune_tick(window, cx); this.uitest_tick(window, cx); }).is_err() { break; }
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
        this._tasks.push(crate::updateui::task(window, cx));
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
                // the backdrop moves only while a wave runs (about 15 pictures a second), never otherwise
                if self.waves.is_empty() || dt < 0.066 { return; }
                let dt = dt.min(0.25);
                self.bd_clock += dt;
                let now = self.bd_clock;
                self.waves.retain(|w| now - w.t0 < crate::backdrop::LIFE);
            }
            Level::Settings => return,
        }
        self.last_anim = Instant::now();
        cx.notify();
    }
    /// Seconds on the arrival clock.
    /// (Tests slow it down with MEGA_GPUI_ARRIVE_SLOW=10 so a screenshot can catch it half way.)
    pub fn now_secs(&self) -> f64 {
        static SLOW: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
        let slow = *SLOW.get_or_init(|| std::env::var("MEGA_GPUI_ARRIVE_SLOW").ok().and_then(|x| x.parse::<f64>().ok()).filter(|x| *x >= 1.).unwrap_or(1.));
        self.epoch.elapsed().as_secs_f64() / slow
    }
    /// Effects run only with the saver off, in a window that is showing the room and that someone can see.
    fn showing(&self, window: &Window) -> bool { self.level == Level::Chat && !self.still() && window.is_window_active() }
    /// Messages that were not in the open room a moment ago resolve out of glyphs (arrive.rs).
    fn arrive(&mut self, known: &std::collections::HashSet<String>, window: &Window) {
        let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mo = crate::arrive::Moment { now: self.now_secs(), wall, showing: self.showing(window), still: self.still() };
        let fresh: Vec<_> = self.win.msgs.iter().filter(|m| !known.contains(&m.id)).cloned().collect();
        self.arrivals.arrive(&fresh, mo);
    }
    /// A count that went up on a message that was already showing: its emoji flies to the pill.
    fn throw_reactions(&mut self, old: &[(String, Vec<crate::arrive::Rx>)], known: &std::collections::HashSet<String>, window: &Window) {
        let ok = self.showing(window);
        let now = self.now_secs();
        for m in &self.win.msgs {
            if !known.contains(&m.id) || m.reactions.is_empty() { continue; }
            let before: &[crate::arrive::Rx] = old.iter().find(|(id, _)| id == &m.id).map(|(_, r)| r.as_slice()).unwrap_or(&[]);
            let after: Vec<crate::arrive::Rx> = m.reactions.iter().map(|x| (x.emoji.clone(), x.count, x.mine)).collect();
            self.flights.throw(&m.id, before, &after, now, ok);
        }
    }

    /// Activity in the open room: a wave leaves from where the newest line is and the speckle organises into bands around it.
    pub fn stir(&mut self, n: usize) {
        if self.still() || self.gid.as_deref().and_then(|g| self.channel(g)).is_some_and(|(c, _)| c.direct) { return; }
        if self.waves.len() > 3 { self.waves.remove(0); }
        let strength = (0.7 + 0.1 * n as f64).min(1.);
        self.waves.push(crate::backdrop::Wave { x: 0.12, y: 0.86, t0: self.bd_clock, s: strength });
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
                // an invite link given on the command line: its server and sign-up secret make the account, and it is used to join afterwards
                if let Some(link) = self.pending_link.clone() {
                    self.rpc("parse_link", json!({"link": link}), window, cx, move |this, r, window, cx| {
                        let (relay, secret) = match r { Ok(v) => (v["relay"].as_str().unwrap_or("").to_string(), v["secret"].as_str().unwrap_or("").to_string()), Err(e) => { this.busy = false; this.error = Some(e.into()); cx.notify(); return; } };
                        this.rpc("create_account", json!({"relay": relay, "passphrase": pass, "name": name, "invite": secret}), window, cx, |this, r, window, cx| {
                            this.busy = false;
                            match r { Ok(_) => this.enter(window, cx), Err(e) => this.error = Some(e.into()) }
                            cx.notify();
                        });
                    });
                    return;
                }
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
        self.start_ui_steps(window, cx);
        if let Some(l) = self.pending_link.take() { self.join_with(l, window, cx); }
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
            // reactions as they were, to see which counts went up (thrown reactions)
            let old_rx: Vec<(String, Vec<crate::arrive::Rx>)> = self.win.msgs.iter().filter(|m| !m.reactions.is_empty()).map(|m| (m.id.clone(), m.reactions.iter().map(|x| (x.emoji.clone(), x.count, x.mine)).collect())).collect();
            let known: std::collections::HashSet<String> = self.win.msgs.iter().map(|m| m.id.clone()).collect();
            let loaded = !self.win.hash.is_empty();
            let applied = self.win.apply(&r);
            // new messages in a room that was already showing resolve out of glyphs, however the answer described them (a difference or the whole window)
            if loaded && matches!(applied, Ok(Change::InPlace { .. }) | Ok(Change::Reset)) { self.arrive(&known, window); self.throw_reactions(&old_rx, &known, window); }
            match applied {
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
        match self.channel(gid) { Some((c, _)) => self.duo_for(gid, c.room_colours.iter().find(|(g, _)| g == gid).map(|(_, d)| d)), None => Duo::of(gid, None) }
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
        let attended = self.attended(window);
        self.rpc("notify_beat", json!({"attended": attended, "web": false}), window, cx, |_, _, _, _| {});
    }

    pub fn open_room(&mut self, cid: String, gid: String, window: &mut Window, cx: &mut Context<Self>) {
        self.level = Level::Chat;
        if self.gid.as_deref() == Some(gid.as_str()) { cx.notify(); return; }
        self.waves.clear(); self.arrivals.clear(); self.flights.clear();
        let unread = self.channel(&gid).filter(|(c, _)| c.direct).map(|(_, ch)| ch.unread as usize).unwrap_or(0).max(if self.channel(&gid).is_some_and(|(c, _)| c.direct) { self.fake_dm_unread } else { 0 });
        self.dm_marks = (gid.clone(), unread);
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


impl Mega {
    pub fn go(&mut self, level: Level, window: &mut Window, cx: &mut Context<Self>) {
        if level == Level::Settings { let n = self.snap.as_ref().map(|s| s.me_name.clone()).unwrap_or_default(); self.set_name.update(cx, |i, cx| i.set_value(n, window, cx)); }
        if level == Level::Chat { self.composer.update(cx, |c, cx| c.focus(window, cx)); }
        if let Level::Server(c) = &level { self.nav.last_home = Some(c.clone()); }
        if matches!(level, Level::Home | Level::Server(_)) { window.focus(&self.nav.focus, cx); }
        self.level = level;
        if self.level != Level::Chat { self.wall.room_mode = false; if !matches!(self.level, Level::Home | Level::Server(_)) { self.wall_focus(None, Default::default(), vec![]); } }
        self.focus_card(None);
        cx.notify();
    }
    /// An owner's colour choice for the open room (`None` puts the default back).
    pub fn set_room_colours(&mut self, duo: Option<(u32, u32)>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(cid), Some(gid)) = (self.cid.clone(), self.gid.clone()) else { return };
        let (main, hi) = duo.map(|(m, h)| (look::hex(m), look::hex(h))).unwrap_or_default();
        self.colours_open = false;
        self.set_setting("set_style", json!({"cid": cid, "room": gid, "main": main, "hi": hi}), window, cx);
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
